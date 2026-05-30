//! Serial port discovery and the background worker thread that owns the open port.
//!
//! The UI never touches the serial port directly. It spawns [`run_worker`] on its own
//! thread and talks to it over two channels: [`ToWorker`] (commands to send / disconnect)
//! and [`FromWorker`] (received lines, status, errors). The worker repaints the egui
//! context whenever something changes, so the UI updates without busy-polling.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use eframe::egui;
use serialport::SerialPortType;

/// Which firmware/role a connected device is running.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Tracker,
    Receiver,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Tracker => "Tracker",
            Mode::Receiver => "Receiver",
        }
    }
}

/// Known USB identifiers for the SlimeNRF firmware family.
/// VID 0x1209 is the community-shared pid.codes vendor ID used by the firmware.
pub const SLIMENRF_VID: u16 = 0x1209;
pub const RECEIVER_PID: u16 = 0x7690;
pub const TRACKER_PID: u16 = 0x7692;

/// A serial port discovered on the system, with USB metadata when available.
#[derive(Clone, Debug)]
pub struct PortInfo {
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub guessed_mode: Option<Mode>,
}

impl PortInfo {
    /// One-line label for the port dropdown, e.g.
    /// `COM7 — SlimeNRF Receiver ProMicro [1209:7690]`.
    pub fn label(&self) -> String {
        let mut parts = vec![self.name.clone()];
        if let Some(prod) = &self.product {
            parts.push(format!("— {prod}"));
        } else if let Some(man) = &self.manufacturer {
            parts.push(format!("— {man}"));
        }
        if let (Some(vid), Some(pid)) = (self.vid, self.pid) {
            parts.push(format!("[{vid:04x}:{pid:04x}]"));
        }
        parts.join(" ")
    }
}

/// Enumerate serial ports and tag any that look like SlimeNRF devices.
/// SlimeNRF devices are sorted to the top of the list.
pub fn list_ports() -> Vec<PortInfo> {
    let mut out = Vec::new();
    let ports = match serialport::available_ports() {
        Ok(p) => p,
        Err(_) => return out,
    };

    for p in ports {
        let (vid, pid, product, manufacturer) = match &p.port_type {
            SerialPortType::UsbPort(info) => (
                Some(info.vid),
                Some(info.pid),
                info.product.clone(),
                info.manufacturer.clone(),
            ),
            _ => (None, None, None, None),
        };

        let guessed_mode = guess_mode(vid, pid, product.as_deref());

        out.push(PortInfo {
            name: p.port_name,
            vid,
            pid,
            product,
            manufacturer,
            guessed_mode,
        });
    }

    // Recognised SlimeNRF devices first, then alphabetical by port name.
    out.sort_by(|a, b| {
        b.guessed_mode
            .is_some()
            .cmp(&a.guessed_mode.is_some())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn guess_mode(vid: Option<u16>, pid: Option<u16>, product: Option<&str>) -> Option<Mode> {
    match (vid, pid) {
        (Some(SLIMENRF_VID), Some(RECEIVER_PID)) => return Some(Mode::Receiver),
        (Some(SLIMENRF_VID), Some(TRACKER_PID)) => return Some(Mode::Tracker),
        _ => {}
    }
    // Fall back to the USB product string ("SlimeNRF Receiver ...", "SlimeNRF Tracker ...").
    if let Some(prod) = product {
        let p = prod.to_lowercase();
        if p.contains("receiver") {
            return Some(Mode::Receiver);
        }
        if p.contains("tracker") {
            return Some(Mode::Tracker);
        }
    }
    None
}

/// Messages sent from the UI to the worker thread.
pub enum ToWorker {
    Send(String),
    Disconnect,
}

/// Messages sent from the worker thread back to the UI.
pub enum FromWorker {
    Connected { name: String },
    Disconnected,
    Rx(String),
    Tx(String),
    Error(String),
}

/// Open `port_name` and run the read/write loop until told to disconnect or the UI
/// side hangs up. `line_ending` is appended to every command (`"\n"` or `"\r\n"`).
pub fn run_worker(
    port_name: String,
    baud: u32,
    line_ending: &'static str,
    cmd_rx: Receiver<ToWorker>,
    evt_tx: Sender<FromWorker>,
    ctx: egui::Context,
) {
    let opened = serialport::new(&port_name, baud)
        .timeout(Duration::from_millis(50))
        .open();

    let mut port = match opened {
        Ok(p) => p,
        Err(e) => {
            let _ = evt_tx.send(FromWorker::Error(format!("Failed to open {port_name}: {e}")));
            let _ = evt_tx.send(FromWorker::Disconnected);
            ctx.request_repaint();
            return;
        }
    };

    // The firmware's console waits for DTR before it starts talking on the USB-CDC
    // console, so assert DTR (and RTS) right after opening.
    let _ = port.write_data_terminal_ready(true);
    let _ = port.write_request_to_send(true);

    let _ = evt_tx.send(FromWorker::Connected {
        name: port_name.clone(),
    });
    ctx.request_repaint();

    let mut buf = [0u8; 2048];
    let mut acc: Vec<u8> = Vec::new();

    loop {
        // 1. Drain any pending outgoing commands.
        loop {
            match cmd_rx.try_recv() {
                Ok(ToWorker::Send(line)) => {
                    let payload = format!("{line}{line_ending}");
                    match port.write_all(payload.as_bytes()) {
                        Ok(()) => {
                            let _ = port.flush();
                            let _ = evt_tx.send(FromWorker::Tx(line));
                        }
                        Err(e) => {
                            let _ = evt_tx.send(FromWorker::Error(format!("Write error: {e}")));
                        }
                    }
                }
                Ok(ToWorker::Disconnect) => {
                    let _ = evt_tx.send(FromWorker::Disconnected);
                    ctx.request_repaint();
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return, // UI dropped the sender
            }
        }

        // 2. Read whatever is available and split it into complete lines.
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                let mut produced = false;
                while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = acc.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&line)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    let _ = evt_tx.send(FromWorker::Rx(text));
                    produced = true;
                }
                if produced {
                    ctx.request_repaint();
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                let _ = evt_tx.send(FromWorker::Error(format!("Read error: {e}")));
                let _ = evt_tx.send(FromWorker::Disconnected);
                ctx.request_repaint();
                return;
            }
        }
    }
}
