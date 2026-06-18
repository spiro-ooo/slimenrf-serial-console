//! The egui application.
//!
//! Layout: a top connection bar, a resizable console on the right, and a central
//! area whose contents switch between the Tracker and Receiver views. Each view
//! leads with a **Common Tasks** card (the handful of things most people want —
//! pair, calibrate, update) and then an "All commands" area with every console
//! command grouped into collapsible sections.
//!
//! Command buttons are disabled until a device is connected, so new users are
//! guided to connect first. The console renders ANSI-coloured firmware output and
//! can filter sent lines, warnings, and errors independently.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use eframe::egui;

use crate::console::{classify_rx, parse_ansi, Kind, LogLine, Segment};
use crate::dfu::{
    drive_key_set, find_uf2_drives, run_dfu_update, DfuProgress,
};
use crate::nrfdfu::{run_receiver_dfu, NrfProgress};
use crate::serial::{list_ports, run_worker, FromWorker, Mode, PortInfo, ToWorker};
use crate::theme::{ACCENT_AMBER, BANNER_BG};

// ---- worker connection -----------------------------------------------------

pub(crate) struct Connection {
    pub(crate) cmd_tx: mpsc::Sender<ToWorker>,
    pub(crate) evt_rx: mpsc::Receiver<FromWorker>,
    pub(crate) connected: bool,
    pub(crate) port_name: String,
}

/// Connection + port-selection state.
#[derive(Default)]
pub(crate) struct ConnectionState {
    pub(crate) ports: Vec<PortInfo>,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud: u32,
    pub(crate) line_ending_crlf: bool,
    pub(crate) connection: Option<Connection>,
}

/// Console output + its display filters and raw-entry box.
pub(crate) struct ConsoleState {
    pub(crate) log: Vec<LogLine>,
    pub(crate) autoscroll: bool,
    pub(crate) show_tx: bool,
    pub(crate) show_warn: bool,
    pub(crate) show_err: bool,
    pub(crate) raw_input: String,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            log: Vec::new(),
            autoscroll: true,
            show_tx: true,
            show_warn: false, // warnings hidden by default
            show_err: true,
            raw_input: String::new(),
        }
    }
}

/// Tracker-mode parameter input fields.
pub(crate) struct TrackerForms {
    pub(crate) debug_dur: String,
    pub(crate) sens_x: String,
    pub(crate) sens_y: String,
    pub(crate) sens_z: String,
    pub(crate) sens_auto_axis: String,
    pub(crate) sens_auto_rev: String,
    pub(crate) set_addr: String,
    pub(crate) channel: String,
    pub(crate) tcal_test: String,
    pub(crate) tcal_remove: String,
}

impl Default for TrackerForms {
    fn default() -> Self {
        Self {
            debug_dur: String::new(),
            sens_x: String::new(),
            sens_y: String::new(),
            sens_z: String::new(),
            sens_auto_axis: "y".to_owned(),
            sens_auto_rev: "5".to_owned(),
            set_addr: String::new(),
            channel: String::new(),
            tcal_test: String::new(),
            tcal_remove: String::new(),
        }
    }
}

/// Receiver-mode parameter fields, including the receiver→tracker remote relay.
pub(crate) struct ReceiverForms {
    pub(crate) add_addr: String,
    pub(crate) pair_count: String,
    pub(crate) stats_sec: String,
    pub(crate) channel: String,
    pub(crate) collect_id: String,
    pub(crate) ota_info: String,
    pub(crate) rem_target_all: bool,
    pub(crate) rem_target_id: String,
    pub(crate) rem_channel: String,
    pub(crate) rem_sens_x: String,
    pub(crate) rem_sens_y: String,
    pub(crate) rem_sens_z: String,
    pub(crate) rem_sens_auto_axis: String,
    pub(crate) rem_sens_auto_rev: String,
}

impl Default for ReceiverForms {
    fn default() -> Self {
        Self {
            add_addr: String::new(),
            pair_count: String::new(),
            stats_sec: String::new(),
            channel: String::new(),
            collect_id: String::new(),
            ota_info: String::new(),
            rem_target_all: true,
            rem_target_id: "0".to_owned(),
            rem_channel: String::new(),
            rem_sens_x: String::new(),
            rem_sens_y: String::new(),
            rem_sens_z: String::new(),
            rem_sens_auto_axis: "y".to_owned(),
            rem_sens_auto_rev: "5".to_owned(),
        }
    }
}

/// Tracker batch firmware updater (UF2 drive-copy). The worker streams DfuProgress.
#[derive(Default)]
pub(crate) struct TrackerDfu {
    pub(crate) firmware: String,
    pub(crate) include_existing: bool,
    pub(crate) rx: Option<mpsc::Receiver<DfuProgress>>,
    pub(crate) running: bool,
    pub(crate) log: Vec<(DfuLevel, String)>,
    pub(crate) result: Option<(usize, usize)>,
}

/// Receiver firmware updater (nRF secure DFU over serial).
pub(crate) struct ReceiverDfu {
    pub(crate) package: String,
    pub(crate) rx: Option<mpsc::Receiver<NrfProgress>>,
    pub(crate) running: bool,
    pub(crate) log: Vec<(DfuLevel, String)>,
    pub(crate) result: Option<(usize, usize)>,
    pub(crate) total: usize,
    pub(crate) done: usize,
    pub(crate) sd_req: String,
}

impl Default for ReceiverDfu {
    fn default() -> Self {
        Self {
            package: String::new(),
            rx: None,
            running: false,
            log: Vec::new(),
            result: None,
            total: 0,
            done: 0,
            sd_req: "0x00".to_owned(),
        }
    }
}

#[derive(Default)]
pub struct App {
    pub(crate) conn: ConnectionState,
    pub(crate) console: ConsoleState,
    pub(crate) tf: TrackerForms,
    pub(crate) rf: ReceiverForms,
    pub(crate) tdfu: TrackerDfu,
    pub(crate) rdfu: ReceiverDfu,

    pub(crate) mode: Mode,

    // Branding: the app icon, uploaded once as a texture for the in-app logo.
    pub(crate) icon_texture: Option<egui::TextureHandle>,
}

/// Severity for a line in the DFU progress log (styling only).
#[derive(Copy, Clone, PartialEq)]
pub(crate) enum DfuLevel {
    Info,
    Good,
    Warn,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        // A little more breathing room than the default makes dense panels readable.
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);

        // Explicit type scale. egui's defaults run small and flat for a dense control
        // panel; this lifts body text and gives headings / buttons a clear step up.
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();

        cc.egui_ctx.set_global_style(style);

        let mut app = App::default();

        // Decode the embedded icon once and upload it as a texture for the in-app
        // logo. Reuses eframe's PNG decoder (the `image` crate it already pulls in),
        // so this adds no dependency. ColorImage wants straight (unmultiplied) alpha,
        // which is exactly what from_png_bytes returns.
        if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            app.icon_texture = Some(cc.egui_ctx.load_texture(
                "app-icon",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }

        app.refresh_ports();
        if let Some(m) = app.current_port_info().and_then(|p| p.guessed_mode) {
            app.mode = m;
        }
        app
    }

    // ---- connection helpers ------------------------------------------------

    pub(crate) fn refresh_ports(&mut self) {
        self.conn.ports = list_ports();
        let still_present = self
            .conn.selected_port
            .as_ref()
            .map_or(false, |s| self.conn.ports.iter().any(|p| &p.name == s));
        if !still_present {
            self.conn.selected_port = self
                .conn.ports
                .iter()
                .find(|p| p.guessed_mode.is_some())
                .or_else(|| self.conn.ports.first())
                .map(|p| p.name.clone());
        }
    }

    pub(crate) fn current_port_info(&self) -> Option<&PortInfo> {
        let name = self.conn.selected_port.as_deref()?;
        self.conn.ports.iter().find(|p| p.name == name)
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.conn.connection.as_ref().map_or(false, |c| c.connected)
    }

    pub(crate) fn connect(&mut self, ctx: &egui::Context) {
        if self.conn.connection.is_some() {
            return;
        }
        let Some(name) = self.conn.selected_port.clone() else {
            self.push_warn("No port selected.".to_owned());
            return;
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<ToWorker>();
        let (evt_tx, evt_rx) = mpsc::channel::<FromWorker>();
        let baud = self.conn.baud;
        let ending: &'static str = if self.conn.line_ending_crlf { "\r\n" } else { "\n" };
        let ctx2 = ctx.clone();
        let worker_name = name.clone();

        let spawned = thread::Builder::new()
            .name("serial-worker".to_owned())
            .spawn(move || run_worker(worker_name, baud, ending, cmd_rx, evt_tx, ctx2));

        match spawned {
            Ok(_handle) => {
                self.conn.connection = Some(Connection {
                    cmd_tx,
                    evt_rx,
                    connected: false,
                    port_name: name.clone(),
                });
                self.push_info(format!("Opening {name} @ {baud} baud…"));
                if let Some(m) = self.current_port_info().and_then(|p| p.guessed_mode) {
                    self.mode = m;
                }
            }
            Err(e) => self.push_err(format!("Failed to start serial worker: {e}")),
        }
    }

    pub(crate) fn disconnect(&mut self) {
        if let Some(c) = &self.conn.connection {
            let _ = c.cmd_tx.send(ToWorker::Disconnect);
        }
        self.conn.connection = None;
        self.push_info("Disconnected.".to_owned());
    }

    pub(crate) fn send_cmd(&mut self, cmd: String) {
        let ready = self.conn.connection.as_ref().map_or(false, |c| c.connected);
        if ready {
            if let Some(c) = &self.conn.connection {
                let _ = c.cmd_tx.send(ToWorker::Send(cmd));
            }
        } else {
            self.push_warn(format!("Not connected — cannot send: {cmd}"));
        }
    }

    // ---- console log -------------------------------------------------------

    fn push_line(&mut self, line: LogLine) {
        self.console.log.push(line);
        const MAX: usize = 5000;
        if self.console.log.len() > MAX {
            let excess = self.console.log.len() - MAX;
            self.console.log.drain(0..excess);
        }
    }

    fn log_simple(&mut self, kind: Kind, text: String) {
        self.push_line(LogLine {
            kind,
            segments: vec![Segment { text, color: None }],
        });
    }

    pub(crate) fn push_info(&mut self, t: String) {
        self.log_simple(Kind::Info, t);
    }
    fn push_warn(&mut self, t: String) {
        self.log_simple(Kind::Warn, t);
    }
    fn push_err(&mut self, t: String) {
        self.log_simple(Kind::Err, t);
    }
    fn push_tx(&mut self, t: String) {
        self.log_simple(Kind::Tx, t);
    }
    fn push_rx(&mut self, t: String) {
        let segments = parse_ansi(&t);
        let plain: String = segments.iter().map(|s| s.text.as_str()).collect();
        let kind = classify_rx(&plain);
        self.push_line(LogLine { kind, segments });
    }

    /// Drain worker events into the log. Collects first to avoid overlapping borrows.
    fn poll(&mut self) {
        let mut events = Vec::new();
        let mut dead = false;
        if let Some(c) = &self.conn.connection {
            loop {
                match c.evt_rx.try_recv() {
                    Ok(e) => events.push(e),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
        }

        for e in events {
            match e {
                FromWorker::Connected { name } => {
                    if let Some(c) = &mut self.conn.connection {
                        c.connected = true;
                        c.port_name = name.clone();
                    }
                    self.push_info(format!("Connected to {name}"));
                }
                FromWorker::Disconnected => dead = true,
                FromWorker::Rx(s) => self.push_rx(s),
                FromWorker::Tx(s) => self.push_tx(s),
                FromWorker::Error(s) => self.push_err(s),
            }
        }

        if dead && self.conn.connection.is_some() {
            self.conn.connection = None;
            self.push_info("Connection closed.".to_owned());
        }
    }

    // ---- batch firmware update (DFU) ---------------------------------------

    fn dfu_log_push(&mut self, level: DfuLevel, msg: String) {
        self.tdfu.log.push((level, msg));
        const MAX: usize = 400;
        if self.tdfu.log.len() > MAX {
            let excess = self.tdfu.log.len() - MAX;
            self.tdfu.log.drain(0..excess);
        }
    }

    /// Drain DfuProgress events from the worker into the DFU log.
    fn poll_dfu(&mut self) {
        let mut events = Vec::new();
        let mut closed = false;
        if let Some(rx) = &self.tdfu.rx {
            loop {
                match rx.try_recv() {
                    Ok(e) => events.push(e),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        closed = true;
                        break;
                    }
                }
            }
        }

        for e in events {
            match e {
                DfuProgress::Status(s) => self.dfu_log_push(DfuLevel::Info, s),
                DfuProgress::PortTriggered { port, ok } => {
                    if ok {
                        self.dfu_log_push(DfuLevel::Good, format!("{port}: entering DFU"));
                    } else {
                        self.dfu_log_push(
                            DfuLevel::Warn,
                            format!("{port}: could not send dfu (already rebooted?)"),
                        );
                    }
                }
                DfuProgress::DriveFound(d) => {
                    self.dfu_log_push(DfuLevel::Info, format!("Found {} — flashing…", d.describe()));
                }
                DfuProgress::Flashed { mount } => {
                    self.dfu_log_push(DfuLevel::Good, format!("Flashed {}", mount.display()));
                }
                DfuProgress::Warn(s) => self.dfu_log_push(DfuLevel::Warn, s),
                DfuProgress::Finished { flashed, expected } => {
                    self.tdfu.result = Some((flashed, expected));
                    self.tdfu.running = false;
                    let lvl = if flashed == expected && expected > 0 {
                        DfuLevel::Good
                    } else {
                        DfuLevel::Warn
                    };
                    self.dfu_log_push(
                        lvl,
                        format!("Done — flashed {flashed} of {expected} device(s)."),
                    );
                }
            }
        }

        if closed {
            self.tdfu.rx = None;
            if self.tdfu.running {
                self.tdfu.running = false;
            }
        }
    }

    /// Tracker serial ports currently present (USB-connected trackers only).
    pub(crate) fn tracker_ports(&self) -> Vec<String> {
        self.conn.ports
            .iter()
            .filter(|p| p.guessed_mode == Some(Mode::Tracker))
            .map(|p| p.name.clone())
            .collect()
    }

    /// Kick off the batch update on a worker thread.
    pub(crate) fn start_dfu_update(&mut self, ctx: &egui::Context) {
        if self.tdfu.running {
            return;
        }

        // Validate the firmware path. Own the string so we can also log (&mut self).
        let fw = self.tdfu.firmware.trim().to_owned();
        if fw.is_empty() {
            self.dfu_log_push(DfuLevel::Warn, "Choose a .uf2 firmware file first.".to_owned());
            return;
        }
        let fw_path = PathBuf::from(&fw);
        if !fw_path.is_file() {
            self.dfu_log_push(DfuLevel::Warn, format!("File not found: {fw}"));
            return;
        }
        if fw_path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
            != Some("uf2".to_owned())
        {
            self.dfu_log_push(
                DfuLevel::Warn,
                "That isn't a .uf2 file — the UF2 bootloader only accepts .uf2 images."
                    .to_owned(),
            );
            return;
        }

        let ports = self.tracker_ports();
        let pre_existing = drive_key_set(&find_uf2_drives());

        if ports.is_empty() && !(self.tdfu.include_existing && !pre_existing.is_empty()) {
            self.dfu_log_push(
                DfuLevel::Warn,
                "No USB-connected trackers detected. Plug in a tracker (or tick \
                 \"also flash drives already in DFU\")."
                    .to_owned(),
            );
            return;
        }

        // The device is about to reboot; release our own serial connection if it's
        // one of the targets, so the DFU worker can open the port.
        if self.conn.connection.is_some() {
            self.disconnect();
        }

        // Reset run state.
        self.tdfu.log.clear();
        self.tdfu.result = None;
        self.tdfu.running = true;
        self.dfu_log_push(
            DfuLevel::Info,
            format!(
                "Starting update: {} tracker port(s){}.",
                ports.len(),
                if self.tdfu.include_existing && !pre_existing.is_empty() {
                    format!(", plus {} drive(s) already in DFU", pre_existing.len())
                } else {
                    String::new()
                }
            ),
        );

        let (tx, rx) = mpsc::channel::<DfuProgress>();
        self.tdfu.rx = Some(rx);

        let line_ending: &'static str = if self.conn.line_ending_crlf { "\r\n" } else { "\n" };
        let include_existing = self.tdfu.include_existing;
        let ctx2 = ctx.clone();

        let spawned = thread::Builder::new()
            .name("dfu-worker".to_owned())
            .spawn(move || {
                run_dfu_update(
                    ports,
                    fw_path,
                    pre_existing,
                    include_existing,
                    line_ending,
                    tx,
                    ctx2,
                );
            });

        if let Err(e) = spawned {
            self.tdfu.running = false;
            self.tdfu.rx = None;
            self.dfu_log_push(DfuLevel::Warn, format!("Could not start updater: {e}"));
        }
    }

    /// The "Update all trackers" card UI.
    // ---- receiver firmware update (nRF secure DFU over serial) -------------

    pub(crate) fn receiver_ports(&self) -> Vec<String> {
        self.conn.ports
            .iter()
            .filter(|p| {
                p.guessed_mode == Some(Mode::Receiver)
                    || p.bootloader == Some(crate::serial::BootloaderKind::NordicDfu)
            })
            .map(|p| p.name.clone())
            .collect()
    }

    fn rdfu_log_push(&mut self, level: DfuLevel, msg: String) {
        self.rdfu.log.push((level, msg));
        const MAX: usize = 400;
        if self.rdfu.log.len() > MAX {
            let excess = self.rdfu.log.len() - MAX;
            self.rdfu.log.drain(0..excess);
        }
    }

    fn poll_rdfu(&mut self) {
        let mut events = Vec::new();
        let mut closed = false;
        if let Some(rx) = &self.rdfu.rx {
            loop {
                match rx.try_recv() {
                    Ok(e) => events.push(e),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        closed = true;
                        break;
                    }
                }
            }
        }

        for e in events {
            match e {
                NrfProgress::Status(s) => self.rdfu_log_push(DfuLevel::Info, s),
                NrfProgress::PortTriggered { port, ok } => {
                    if ok {
                        self.rdfu_log_push(DfuLevel::Good, format!("{port}: entering bootloader"));
                    } else {
                        self.rdfu_log_push(
                            DfuLevel::Warn,
                            format!("{port}: could not send dfu (already in bootloader?)"),
                        );
                    }
                }
                NrfProgress::DeviceReady { port } => {
                    self.rdfu_log_push(DfuLevel::Info, format!("{port}: bootloader ready, flashing…"));
                }
                NrfProgress::Total { bytes } => {
                    self.rdfu.total = bytes;
                    self.rdfu.done = 0;
                }
                NrfProgress::Advance { bytes } => {
                    self.rdfu.done = self.rdfu.done.saturating_add(bytes);
                }
                NrfProgress::Flashed { port } => {
                    self.rdfu_log_push(DfuLevel::Good, format!("{port}: flash complete"));
                }
                NrfProgress::Warn(s) => self.rdfu_log_push(DfuLevel::Warn, s),
                NrfProgress::Finished { flashed, expected } => {
                    self.rdfu.result = Some((flashed, expected));
                    self.rdfu.running = false;
                    let lvl = if flashed == expected && expected > 0 {
                        DfuLevel::Good
                    } else {
                        DfuLevel::Warn
                    };
                    self.rdfu_log_push(lvl, format!("Done — flashed {flashed} of {expected} receiver(s)."));
                }
            }
        }

        if closed {
            self.rdfu.rx = None;
            if self.rdfu.running {
                self.rdfu.running = false;
            }
        }
    }

    pub(crate) fn start_rdfu_update(&mut self, ctx: &egui::Context) {
        if self.rdfu.running {
            return;
        }
        let pkg = self.rdfu.package.trim().to_owned();
        if pkg.is_empty() {
            self.rdfu_log_push(DfuLevel::Warn, "Choose a Nordic DFU .zip package first.".to_owned());
            return;
        }
        let pkg_path = PathBuf::from(&pkg);
        if !pkg_path.is_file() {
            self.rdfu_log_push(DfuLevel::Warn, format!("File not found: {pkg}"));
            return;
        }
        let ext = pkg_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "zip" && ext != "hex" {
            self.rdfu_log_push(
                DfuLevel::Warn,
                "Receiver firmware must be a .hex or a Nordic DFU .zip.".to_owned(),
            );
            return;
        }

        let mut ports = self.receiver_ports();
        let already: HashSet<String> = self
            .conn.ports
            .iter()
            .filter(|p| p.bootloader == Some(crate::serial::BootloaderKind::NordicDfu))
            .map(|p| p.name.clone())
            .collect();
        for p in &already {
            if !ports.contains(p) {
                ports.push(p.clone());
            }
        }

        if ports.is_empty() {
            self.rdfu_log_push(
                DfuLevel::Warn,
                "No receiver detected. Plug in the receiver dongle (or a board already in DFU)."
                    .to_owned(),
            );
            return;
        }

        if self.conn.connection.is_some() {
            self.disconnect();
        }

        self.rdfu.log.clear();
        self.rdfu.result = None;
        self.rdfu.total = 0;
        self.rdfu.done = 0;
        self.rdfu.running = true;
        self.rdfu_log_push(
            DfuLevel::Info,
            format!("Starting receiver update on {} port(s).", ports.len()),
        );

        let (tx, rx) = mpsc::channel::<NrfProgress>();
        self.rdfu.rx = Some(rx);
        let line_ending: &'static str = if self.conn.line_ending_crlf { "\r\n" } else { "\n" };

        // Parse the SoftDevice requirement field: comma-separated, hex (0x..) or
        // decimal. Empty or unparseable falls back to [0x00] (no SoftDevice).
        let sd_req: Vec<u16> = {
            let parsed: Vec<u16> = self
                .rdfu.sd_req
                .split(',')
                .filter_map(|tok| {
                    let t = tok.trim();
                    if t.is_empty() {
                        return None;
                    }
                    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                        u16::from_str_radix(hex, 16).ok()
                    } else {
                        t.parse::<u16>().ok()
                    }
                })
                .collect();
            if parsed.is_empty() {
                vec![0x0000]
            } else {
                parsed
            }
        };

        let ctx2 = ctx.clone();

        let spawned = thread::Builder::new()
            .name("nrf-dfu-worker".to_owned())
            .spawn(move || {
                run_receiver_dfu(ports, already, pkg_path, sd_req, line_ending, tx, ctx2);
            });

        if let Err(e) = spawned {
            self.rdfu.running = false;
            self.rdfu.rx = None;
            self.rdfu_log_push(DfuLevel::Warn, format!("Could not start updater: {e}"));
        }
    }

    /// The "Update receiver firmware" card (receiver mode only).
    /// The current `send` target string (`"all"` or a tracker id).
    pub(crate) fn remote_target(&self) -> String {
        if self.rf.rem_target_all {
            "all".to_owned()
        } else {
            let id = self.rf.rem_target_id.trim();
            if id.is_empty() {
                "0".to_owned()
            } else {
                id.to_owned()
            }
        }
    }

    // ---- small widgets -----------------------------------------------------

    // ---- top connection bar ------------------------------------------------

    // ---- right-hand console ------------------------------------------------

    // ---- tracker view ------------------------------------------------------

    // ---- receiver view -----------------------------------------------------

}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.poll();
        self.poll_dfu();
        self.poll_rdfu();

        egui::Panel::top("connection_bar").show_inside(ui, |ui| {
            self.connection_bar(ui, &ctx);
        });

        egui::Panel::right("console_panel")
            .resizable(true)
            .default_size(470.0)
            .size_range(320.0..=1000.0)
            .show_inside(ui, |ui| {
                self.console_panel(ui);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !self.is_connected() {
                        egui::Frame::group(ui.style())
                            .fill(BANNER_BG)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("1")
                                            .size(18.0)
                                            .strong()
                                            .color(ACCENT_AMBER),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Select your device in the bar above and click Connect to enable the commands below.",
                                        )
                                        .color(egui::Color32::from_rgb(212, 214, 218)),
                                    );
                                });
                            });
                        ui.add_space(8.0);
                    }

                    match self.mode {
                        Mode::Tracker => self.tracker_panel(ui),
                        Mode::Receiver => self.receiver_panel(ui),
                    }
                });
        });

        if self.conn.connection.is_some() || self.tdfu.running || self.rdfu.running {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}