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

/// Adafruit's USB vendor ID. When a SlimeNRF board running the UF2 (Adafruit)
/// bootloader is in DFU mode it enumerates under this VID — first as a CDC serial
/// port, then as a mass-storage drive — so a serial port with this VID is a device
/// sitting in the bootloader, not running firmware.
pub const ADAFRUIT_VID: u16 = 0x239A;

/// Nordic Semiconductor's USB vendor ID. The Nordic Open / secure DFU bootloader
/// (used by the nRF52840 dongle and Holyiot dongles) enumerates as 0x1915:0x521F
/// ("Open DFU Bootloader") while waiting for a serial DFU image.
pub const NORDIC_VID: u16 = 0x1915;
pub const NORDIC_DFU_PID: u16 = 0x521F;

/// If a port is a board sitting in a bootloader, which kind — so the UI can route
/// it to the right flasher (UF2 drive-copy vs Nordic serial DFU).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BootloaderKind {
    /// Adafruit UF2 bootloader (CDC + mass storage). Tracker / UF2 boards.
    Uf2,
    /// Nordic Open / secure DFU bootloader over serial. Receiver dongles.
    NordicDfu,
}

/// A serial port discovered on the system, with USB metadata when available.
#[derive(Clone, Debug)]
pub struct PortInfo {
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub guessed_mode: Option<Mode>,
    /// `Some(kind)` if this port looks like a board sitting in a bootloader.
    pub bootloader: Option<BootloaderKind>,
}

impl PortInfo {
    /// True if this port is a board in any bootloader/DFU mode.
    pub fn in_bootloader(&self) -> bool {
        self.bootloader.is_some()
    }

    /// The most human-readable name available for this port: the USB product
    /// string if present, otherwise the manufacturer, otherwise a generic label.
    pub fn display_name(&self) -> String {
        match self.bootloader {
            Some(BootloaderKind::NordicDfu) => {
                return "Nordic Open DFU bootloader (receiver)".to_owned()
            }
            Some(BootloaderKind::Uf2) => {
                return match &self.product {
                    Some(p) => format!("{p} (UF2 bootloader)"),
                    None => "UF2 bootloader".to_owned(),
                }
            }
            None => {}
        }
        if let Some(prod) = &self.product {
            prod.clone()
        } else if let Some(man) = &self.manufacturer {
            man.clone()
        } else if self.vid.is_some() {
            "USB serial device".to_owned()
        } else {
            "Serial port".to_owned()
        }
    }

    /// One-line label for the port dropdown's collapsed text, e.g.
    /// `COM7 — SlimeNRF Receiver nRF52840 Dongle`.
    pub fn label(&self) -> String {
        format!("{} — {}", self.name, self.display_name())
    }

    /// VID:PID as a string, e.g. `1209:7690`, or `—` if not a USB device.
    pub fn ids(&self) -> String {
        match (self.vid, self.pid) {
            (Some(v), Some(p)) => format!("{v:04x}:{p:04x}"),
            _ => "—".to_owned(),
        }
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
        let (vid, pid, product, manufacturer, serial_number) = match &p.port_type {
            SerialPortType::UsbPort(info) => (
                Some(info.vid),
                Some(info.pid),
                info.product.clone(),
                info.manufacturer.clone(),
                info.serial_number.clone(),
            ),
            _ => (None, None, None, None, None),
        };

        let bootloader = if vid == Some(NORDIC_VID) && pid == Some(NORDIC_DFU_PID) {
            Some(BootloaderKind::NordicDfu)
        } else if vid == Some(NORDIC_VID) {
            // Any other Nordic-VID serial device in this context is most likely the
            // DFU bootloader too (some builds report a different PID).
            Some(BootloaderKind::NordicDfu)
        } else if vid == Some(ADAFRUIT_VID) {
            Some(BootloaderKind::Uf2)
        } else {
            None
        };
        let guessed_mode = guess_mode(vid, pid, product.as_deref());

        out.push(PortInfo {
            name: p.port_name,
            vid,
            pid,
            product,
            manufacturer,
            serial_number,
            guessed_mode,
            bootloader,
        });
    }

    // Recognised SlimeNRF devices and boards in DFU first, then alphabetical.
    out.sort_by(|a, b| {
        let a_known = a.guessed_mode.is_some() || a.in_bootloader();
        let b_known = b.guessed_mode.is_some() || b.in_bootloader();
        b_known
            .cmp(&a_known)
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