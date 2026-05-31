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

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use eframe::egui;

use crate::dfu::{
    drive_key_set, find_uf2_drives, run_dfu_update, DfuProgress,
};
use crate::serial::{list_ports, run_worker, FromWorker, Mode, PortInfo, ToWorker};

// ---- palette ---------------------------------------------------------------

const ACCENT: egui::Color32 = egui::Color32::from_rgb(74, 125, 205); // primary, safe
const ACCENT_AMBER: egui::Color32 = egui::Color32::from_rgb(199, 138, 46); // update / caution
const DANGER: egui::Color32 = egui::Color32::from_rgb(170, 62, 62); // destructive
const MUTED: egui::Color32 = egui::Color32::from_rgb(166, 171, 180); // secondary text
const CARD_BG: egui::Color32 = egui::Color32::from_rgb(33, 37, 46);
const BANNER_BG: egui::Color32 = egui::Color32::from_rgb(52, 45, 30);

// Per-section accent hues. Each command section gets its own colour: a coloured
// header, a tinted bordered frame, and tinted buttons. This turns the long list of
// look-alike grey buttons into colour-grouped clusters you can scan at a glance.
// Red is deliberately *not* used as a section hue — it's reserved app-wide for
// destructive actions (see `danger_button`) so "careful" always looks the same.
const HUE_INFO: egui::Color32 = egui::Color32::from_rgb(96, 142, 196); // steel blue
const HUE_SENSOR: egui::Color32 = egui::Color32::from_rgb(70, 168, 162); // teal
const HUE_MAG: egui::Color32 = egui::Color32::from_rgb(150, 124, 206); // violet
const HUE_TEMP: egui::Color32 = egui::Color32::from_rgb(202, 150, 74); // amber
const HUE_CONN: egui::Color32 = egui::Color32::from_rgb(106, 168, 96); // green
const HUE_SYSTEM: egui::Color32 = egui::Color32::from_rgb(120, 140, 158); // slate
const HUE_RESET: egui::Color32 = egui::Color32::from_rgb(192, 110, 92); // clay (destructive-ish)
const HUE_TEST: egui::Color32 = egui::Color32::from_rgb(196, 116, 170); // magenta
const HUE_STATS: egui::Color32 = egui::Color32::from_rgb(86, 156, 188); // cyan
const HUE_DATA: egui::Color32 = egui::Color32::from_rgb(170, 150, 96); // ochre
const HUE_REMOTE: egui::Color32 = egui::Color32::from_rgb(126, 150, 210); // periwinkle

// ---- console line model ----------------------------------------------------

/// Category of a console line — drives both colour and the show/hide filters.
#[derive(Copy, Clone, PartialEq)]
enum Kind {
    Tx,
    Rx,
    Info,
    Warn,
    Err,
}

impl Kind {
    /// Default colour for text that carries no explicit ANSI colour of its own.
    fn default_color(self) -> egui::Color32 {
        match self {
            Kind::Tx => egui::Color32::from_rgb(120, 170, 255),
            Kind::Rx => egui::Color32::from_rgb(222, 224, 228),
            Kind::Info => egui::Color32::from_rgb(150, 160, 172),
            Kind::Warn => egui::Color32::from_rgb(222, 192, 92),
            Kind::Err => egui::Color32::from_rgb(240, 104, 104),
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Kind::Tx => "» ",
            Kind::Rx => "",
            Kind::Info => "· ",
            Kind::Warn => "⚠ ",
            Kind::Err => "✖ ",
        }
    }
}

/// A run of text sharing one colour. `color == None` means "use the line's Kind
/// colour" (i.e. the firmware didn't colour this run with an ANSI escape).
struct Segment {
    text: String,
    color: Option<egui::Color32>,
}

struct LogLine {
    kind: Kind,
    segments: Vec<Segment>,
}

// ---- ANSI / xterm-256 colour handling --------------------------------------

/// Map an xterm-256 colour index to RGB (standard 16 + 6×6×6 cube + greyscale).
fn xterm_color(n: u8) -> (u8, u8, u8) {
    match n {
        0 => (0, 0, 0),
        1 => (187, 0, 0),
        2 => (0, 187, 0),
        3 => (187, 187, 0),
        4 => (0, 0, 187),
        5 => (187, 0, 187),
        6 => (0, 187, 187),
        7 => (200, 200, 200),
        8 => (110, 110, 110),
        9 => (255, 80, 80),
        10 => (90, 230, 90),
        11 => (240, 230, 80),
        12 => (110, 150, 255),
        13 => (240, 120, 240),
        14 => (80, 220, 230),
        15 => (255, 255, 255),
        16..=231 => {
            let m = n - 16;
            let r = m / 36;
            let g = (m % 36) / 6;
            let b = m % 6;
            let conv = |c: u8| if c == 0 { 0 } else { 55 + 40 * c };
            (conv(r), conv(g), conv(b))
        }
        232..=255 => {
            let v = (8u16 + 10 * (n as u16 - 232)) as u8;
            (v, v, v)
        }
    }
}

fn lighten(rgb: (u8, u8, u8), amt: f32) -> (u8, u8, u8) {
    let l = |c: u8| (c as f32 + (255.0 - c as f32) * amt) as u8;
    (l(rgb.0), l(rgb.1), l(rgb.2))
}

/// Keep very dark colours visible against the dark console background.
fn readable(rgb: (u8, u8, u8)) -> egui::Color32 {
    let (r, g, b) = rgb;
    let maxc = r.max(g).max(b);
    if maxc == 0 {
        return egui::Color32::from_rgb(140, 140, 140);
    }
    if maxc < 90 {
        let factor = 120.0 / maxc as f32;
        let s = |c: u8| (c as f32 * factor).min(255.0) as u8;
        return egui::Color32::from_rgb(s(r), s(g), s(b));
    }
    egui::Color32::from_rgb(r, g, b)
}

/// Apply one SGR (`\x1b[...m`) parameter list to the running colour/bold state.
fn apply_sgr(params: &str, base: &mut Option<(u8, u8, u8)>, bold: &mut bool) {
    let nums: Vec<i64> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split(';')
            .map(|s| s.parse::<i64>().unwrap_or(-1))
            .collect()
    };

    let mut k = 0;
    while k < nums.len() {
        let p = nums[k];
        match p {
            0 => {
                *base = None;
                *bold = false;
            }
            1 => *bold = true,
            22 => *bold = false,
            39 => *base = None,
            30..=37 => *base = Some(xterm_color((p - 30) as u8)),
            90..=97 => *base = Some(xterm_color((p - 90 + 8) as u8)),
            38 => match nums.get(k + 1).copied() {
                Some(5) => {
                    if let Some(&n) = nums.get(k + 2) {
                        if (0..=255).contains(&n) {
                            *base = Some(xterm_color(n as u8));
                        }
                    }
                    k += 2;
                }
                Some(2) => {
                    let comp = |idx: usize| nums.get(idx).copied().unwrap_or(0).clamp(0, 255) as u8;
                    *base = Some((comp(k + 2), comp(k + 3), comp(k + 4)));
                    k += 4;
                }
                _ => {}
            },
            48 => match nums.get(k + 1).copied() {
                Some(5) => k += 2,
                Some(2) => k += 4,
                _ => {}
            },
            _ => {} // background (40-47/100-107), reverse, etc. — ignored
        }
        k += 1;
    }
}

/// Split a firmware line into coloured [`Segment`]s, interpreting ANSI SGR escapes
/// and discarding other control sequences (cursor moves, erases, …).
fn parse_ansi(input: &str) -> Vec<Segment> {
    let chars: Vec<char> = input.chars().collect();
    let mut segs: Vec<Segment> = Vec::new();
    let mut buf = String::new();
    let mut base: Option<(u8, u8, u8)> = None;
    let mut bold = false;

    let flush = |buf: &mut String,
                 base: Option<(u8, u8, u8)>,
                 bold: bool,
                 segs: &mut Vec<Segment>| {
        if buf.is_empty() {
            return;
        }
        let color = base.map(|rgb| readable(if bold { lighten(rgb, 0.18) } else { rgb }));
        segs.push(Segment {
            text: std::mem::take(buf),
            color,
        });
    };

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{1b}' {
            if i + 1 < chars.len() && chars[i + 1] == '[' {
                // CSI sequence: scan to the final byte in 0x40..=0x7e.
                let mut j = i + 2;
                while j < chars.len() && !('\u{40}'..='\u{7e}').contains(&chars[j]) {
                    j += 1;
                }
                if j < chars.len() {
                    if chars[j] == 'm' {
                        flush(&mut buf, base, bold, &mut segs);
                        let params: String = chars[i + 2..j].iter().collect();
                        apply_sgr(&params, &mut base, &mut bold);
                    }
                    i = j + 1;
                    continue;
                }
                break; // unterminated sequence
            }
            i += 1; // lone ESC
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, base, bold, &mut segs);
    if segs.is_empty() {
        segs.push(Segment {
            text: String::new(),
            color: None,
        });
    }
    segs
}

/// Classify a received line. Only explicit Zephyr log markers promote a line to
/// warning/error — the firmware also uses colour decoratively, so colour alone
/// must not hide a line.
fn classify_rx(plain: &str) -> Kind {
    let l = plain.to_ascii_lowercase();
    if l.contains("<err>") {
        Kind::Err
    } else if l.contains("<wrn>") {
        Kind::Warn
    } else {
        Kind::Rx
    }
}

// ---- worker connection -----------------------------------------------------

struct Connection {
    cmd_tx: mpsc::Sender<ToWorker>,
    evt_rx: mpsc::Receiver<FromWorker>,
    connected: bool,
    port_name: String,
}

pub struct App {
    // Connection
    ports: Vec<PortInfo>,
    selected_port: Option<String>,
    baud: u32,
    line_ending_crlf: bool,
    connection: Option<Connection>,

    // View / console
    mode: Mode,
    log: Vec<LogLine>,
    autoscroll: bool,
    show_tx: bool,
    show_warn: bool,
    show_err: bool,
    raw_input: String,

    // Branding: the app icon, uploaded once as a texture for the in-app logo.
    icon_texture: Option<egui::TextureHandle>,

    // Tracker parameter fields
    t_debug_dur: String,
    t_sens_x: String,
    t_sens_y: String,
    t_sens_z: String,
    t_set_addr: String,
    t_channel: String,
    t_tcal_test: String,
    t_tcal_remove: String,

    // Receiver parameter fields
    r_add_addr: String,
    r_pair_count: String,
    r_stats_sec: String,
    r_channel: String,
    r_collect_id: String,
    r_ota_info: String,

    // Receiver -> tracker remote command fields
    rem_target_all: bool,
    rem_target_id: String,
    rem_channel: String,
    rem_sens_x: String,
    rem_sens_y: String,
    rem_sens_z: String,

    // Batch firmware updater (DFU). The worker streams DfuProgress while running.
    dfu_firmware: String,
    dfu_include_existing: bool,
    dfu_rx: Option<mpsc::Receiver<DfuProgress>>,
    dfu_running: bool,
    dfu_log: Vec<(DfuLevel, String)>,
    dfu_result: Option<(usize, usize)>,
}

/// Severity for a line in the DFU progress log (styling only).
#[derive(Copy, Clone, PartialEq)]
enum DfuLevel {
    Info,
    Good,
    Warn,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: None,
            baud: 115_200,
            line_ending_crlf: false,
            connection: None,
            mode: Mode::Tracker,
            log: Vec::new(),
            autoscroll: true,
            show_tx: true,
            show_warn: false, // warnings hidden by default
            show_err: true,
            raw_input: String::new(),
            icon_texture: None,
            t_debug_dur: String::new(),
            t_sens_x: String::new(),
            t_sens_y: String::new(),
            t_sens_z: String::new(),
            t_set_addr: String::new(),
            t_channel: String::new(),
            t_tcal_test: String::new(),
            t_tcal_remove: String::new(),
            r_add_addr: String::new(),
            r_pair_count: String::new(),
            r_stats_sec: String::new(),
            r_channel: String::new(),
            r_collect_id: String::new(),
            r_ota_info: String::new(),
            rem_target_all: true,
            rem_target_id: "0".to_owned(),
            rem_channel: String::new(),
            rem_sens_x: String::new(),
            rem_sens_y: String::new(),
            rem_sens_z: String::new(),
            dfu_firmware: String::new(),
            dfu_include_existing: false,
            dfu_rx: None,
            dfu_running: false,
            dfu_log: Vec::new(),
            dfu_result: None,
        }
    }
}

// ---- section styling helpers ----------------------------------------------

/// Mix a colour toward black by `t` (0 = unchanged, 1 = black).
fn darken(c: egui::Color32, t: f32) -> egui::Color32 {
    let f = 1.0 - t.clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (c.r() as f32 * f) as u8,
        (c.g() as f32 * f) as u8,
        (c.b() as f32 * f) as u8,
    )
}

/// Blend two colours by `t` (0 = a, 1 = b).
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    egui::Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Coloured, bold header text for a section's CollapsingHeader.
fn section_header(text: &str, hue: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .color(mix(hue, egui::Color32::WHITE, 0.18))
        .strong()
        .size(14.5)
}

/// A bordered, faintly-tinted frame that boxes a section's body in its hue.
fn section_frame(hue: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(darken(hue, 0.86)) // very dark wash of the hue
        .stroke(egui::Stroke::new(1.0, darken(hue, 0.35)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .outer_margin(egui::Margin {
            left: 0,
            right: 0,
            top: 2,
            bottom: 6,
        })
}

/// Tint every default `ui.button(...)` in the current scope with the section hue,
/// for the inactive / hovered / active states. Buttons that set an explicit
/// `.fill(...)` (e.g. the red `danger_button`) are unaffected and still stand out.
fn tint_buttons(ui: &mut egui::Ui, hue: egui::Color32) {
    let v = ui.visuals_mut();
    let text = mix(hue, egui::Color32::WHITE, 0.72);
    // inactive
    v.widgets.inactive.weak_bg_fill = darken(hue, 0.62);
    v.widgets.inactive.fg_stroke.color = text;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, darken(hue, 0.45));
    // hovered (brighter)
    v.widgets.hovered.weak_bg_fill = darken(hue, 0.42);
    v.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, mix(hue, egui::Color32::WHITE, 0.15));
    // active (pressed, brightest)
    v.widgets.active.weak_bg_fill = darken(hue, 0.28);
    v.widgets.active.fg_stroke.color = egui::Color32::WHITE;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, mix(hue, egui::Color32::WHITE, 0.30));
}

/// Render one coloured, framed collapsing section.
///
/// Free function (not a method) so the `body` closure can capture `&mut self` at
/// the call site without conflicting with a `&mut self` receiver here.
fn section(
    ui: &mut egui::Ui,
    id_salt: &str,
    title: &str,
    hue: egui::Color32,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(section_header(title, hue))
        .id_salt(id_salt)
        .default_open(default_open)
        .show_unindented(ui, |ui| {
            section_frame(hue).show(ui, |ui| {
                tint_buttons(ui, hue);
                body(ui);
            });
        });
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        // A little more breathing room than the default makes dense panels readable.
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
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

    fn refresh_ports(&mut self) {
        self.ports = list_ports();
        let still_present = self
            .selected_port
            .as_ref()
            .map_or(false, |s| self.ports.iter().any(|p| &p.name == s));
        if !still_present {
            self.selected_port = self
                .ports
                .iter()
                .find(|p| p.guessed_mode.is_some())
                .or_else(|| self.ports.first())
                .map(|p| p.name.clone());
        }
    }

    fn current_port_info(&self) -> Option<&PortInfo> {
        let name = self.selected_port.as_deref()?;
        self.ports.iter().find(|p| p.name == name)
    }

    fn is_connected(&self) -> bool {
        self.connection.as_ref().map_or(false, |c| c.connected)
    }

    fn connect(&mut self, ctx: &egui::Context) {
        if self.connection.is_some() {
            return;
        }
        let Some(name) = self.selected_port.clone() else {
            self.push_warn("No port selected.".to_owned());
            return;
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<ToWorker>();
        let (evt_tx, evt_rx) = mpsc::channel::<FromWorker>();
        let baud = self.baud;
        let ending: &'static str = if self.line_ending_crlf { "\r\n" } else { "\n" };
        let ctx2 = ctx.clone();
        let worker_name = name.clone();

        let spawned = thread::Builder::new()
            .name("serial-worker".to_owned())
            .spawn(move || run_worker(worker_name, baud, ending, cmd_rx, evt_tx, ctx2));

        match spawned {
            Ok(_handle) => {
                self.connection = Some(Connection {
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

    fn disconnect(&mut self) {
        if let Some(c) = &self.connection {
            let _ = c.cmd_tx.send(ToWorker::Disconnect);
        }
        self.connection = None;
        self.push_info("Disconnected.".to_owned());
    }

    fn send_cmd(&mut self, cmd: String) {
        let ready = self.connection.as_ref().map_or(false, |c| c.connected);
        if ready {
            if let Some(c) = &self.connection {
                let _ = c.cmd_tx.send(ToWorker::Send(cmd));
            }
        } else {
            self.push_warn(format!("Not connected — cannot send: {cmd}"));
        }
    }

    // ---- console log -------------------------------------------------------

    fn push_line(&mut self, line: LogLine) {
        self.log.push(line);
        const MAX: usize = 5000;
        if self.log.len() > MAX {
            let excess = self.log.len() - MAX;
            self.log.drain(0..excess);
        }
    }

    fn log_simple(&mut self, kind: Kind, text: String) {
        self.push_line(LogLine {
            kind,
            segments: vec![Segment { text, color: None }],
        });
    }

    fn push_info(&mut self, t: String) {
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
        if let Some(c) = &self.connection {
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
                    if let Some(c) = &mut self.connection {
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

        if dead && self.connection.is_some() {
            self.connection = None;
            self.push_info("Connection closed.".to_owned());
        }
    }

    // ---- batch firmware update (DFU) ---------------------------------------

    fn dfu_log_push(&mut self, level: DfuLevel, msg: String) {
        self.dfu_log.push((level, msg));
        const MAX: usize = 400;
        if self.dfu_log.len() > MAX {
            let excess = self.dfu_log.len() - MAX;
            self.dfu_log.drain(0..excess);
        }
    }

    /// Drain DfuProgress events from the worker into the DFU log.
    fn poll_dfu(&mut self) {
        let mut events = Vec::new();
        let mut closed = false;
        if let Some(rx) = &self.dfu_rx {
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
                    self.dfu_result = Some((flashed, expected));
                    self.dfu_running = false;
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
            self.dfu_rx = None;
            if self.dfu_running {
                self.dfu_running = false;
            }
        }
    }

    /// Tracker serial ports currently present (USB-connected trackers only).
    fn tracker_ports(&self) -> Vec<String> {
        self.ports
            .iter()
            .filter(|p| p.guessed_mode == Some(Mode::Tracker))
            .map(|p| p.name.clone())
            .collect()
    }

    /// Kick off the batch update on a worker thread.
    fn start_dfu_update(&mut self, ctx: &egui::Context) {
        if self.dfu_running {
            return;
        }

        // Validate the firmware path. Own the string so we can also log (&mut self).
        let fw = self.dfu_firmware.trim().to_owned();
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

        if ports.is_empty() && !(self.dfu_include_existing && !pre_existing.is_empty()) {
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
        if self.connection.is_some() {
            self.disconnect();
        }

        // Reset run state.
        self.dfu_log.clear();
        self.dfu_result = None;
        self.dfu_running = true;
        self.dfu_log_push(
            DfuLevel::Info,
            format!(
                "Starting update: {} tracker port(s){}.",
                ports.len(),
                if self.dfu_include_existing && !pre_existing.is_empty() {
                    format!(", plus {} drive(s) already in DFU", pre_existing.len())
                } else {
                    String::new()
                }
            ),
        );

        let (tx, rx) = mpsc::channel::<DfuProgress>();
        self.dfu_rx = Some(rx);

        let line_ending: &'static str = if self.line_ending_crlf { "\r\n" } else { "\n" };
        let include_existing = self.dfu_include_existing;
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
            self.dfu_running = false;
            self.dfu_rx = None;
            self.dfu_log_push(DfuLevel::Warn, format!("Could not start updater: {e}"));
        }
    }

    /// The "Update all trackers" card UI.
    fn dfu_update_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT_AMBER, 0.45)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("UPDATE ALL TRACKERS")
                            .size(13.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Live count of trackers we'd flash.
                        let n = self.tracker_ports().len();
                        let txt = if n == 1 {
                            "1 tracker detected".to_owned()
                        } else {
                            format!("{n} trackers detected")
                        };
                        ui.label(egui::RichText::new(txt).color(MUTED));
                    });
                });
                Self::desc(
                    ui,
                    "Puts every USB-connected tracker into its UF2 bootloader and copies the \
                     selected firmware onto each one. Trackers paired wirelessly to a receiver \
                     are not updated this way — connect them by USB.",
                );
                ui.add_space(6.0);

                // Firmware picker row.
                ui.horizontal(|ui| {
                    ui.label("Firmware:");
                    let avail = ui.available_width();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dfu_firmware)
                            .desired_width((avail - 96.0).max(120.0))
                            .hint_text("path to firmware .uf2"),
                    );
                    if ui.add_enabled(!self.dfu_running, egui::Button::new("Browse…")).clicked() {
                        // Native file dialog (XDG portal on Linux — no GTK needed).
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter("UF2 firmware", &["uf2"])
                            .set_title("Select tracker firmware (.uf2)");
                        // Start in the directory of the current entry, if any.
                        let cur = self.dfu_firmware.trim().to_owned();
                        if !cur.is_empty() {
                            if let Some(parent) = PathBuf::from(&cur).parent() {
                                if parent.is_dir() {
                                    dlg = dlg.set_directory(parent);
                                }
                            }
                        }
                        if let Some(path) = dlg.pick_file() {
                            self.dfu_firmware = path.display().to_string();
                        }
                    }
                });

                ui.add_space(2.0);
                ui.checkbox(
                    &mut self.dfu_include_existing,
                    "Also flash drives already in DFU mode",
                )
                .on_hover_text(
                    "Off: only trackers that enter DFU from this action are flashed.\n\
                     On: also flash any UF2 drive already mounted (e.g. a manually-reset board).",
                );

                ui.add_space(8.0);

                // Action button + spinner.
                ui.horizontal(|ui| {
                    let label = if self.dfu_running {
                        "Updating…"
                    } else {
                        "⬆  Update all trackers"
                    };
                    if Self::primary(ui, !self.dfu_running, label, ACCENT_AMBER).clicked() {
                        let ctx = ui.ctx().clone();
                        self.start_dfu_update(&ctx);
                    }
                    if self.dfu_running {
                        ui.add(egui::Spinner::new());
                    }
                    if !self.dfu_running && !self.dfu_log.is_empty() {
                        if ui.button("Clear log").clicked() {
                            self.dfu_log.clear();
                            self.dfu_result = None;
                        }
                    }
                });

                // Progress log.
                if !self.dfu_log.is_empty() {
                    ui.add_space(6.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(24, 27, 34))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .auto_shrink([false, true])
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for (lvl, msg) in &self.dfu_log {
                                        let color = match lvl {
                                            DfuLevel::Info => MUTED,
                                            DfuLevel::Good => {
                                                egui::Color32::from_rgb(120, 200, 130)
                                            }
                                            DfuLevel::Warn => {
                                                egui::Color32::from_rgb(222, 180, 90)
                                            }
                                        };
                                        ui.label(
                                            egui::RichText::new(msg)
                                                .color(color)
                                                .font(egui::FontId::monospace(12.0)),
                                        );
                                    }
                                });
                        });
                }
            });
    }

    /// The current `send` target string (`"all"` or a tracker id).
    fn remote_target(&self) -> String {
        if self.rem_target_all {
            "all".to_owned()
        } else {
            let id = self.rem_target_id.trim();
            if id.is_empty() {
                "0".to_owned()
            } else {
                id.to_owned()
            }
        }
    }

    // ---- small widgets -----------------------------------------------------

    /// A large, accent-filled primary button (used in the Common Tasks cards).
    /// Disabled when `enabled` is false so it greys out before connecting.
    fn primary(ui: &mut egui::Ui, enabled: bool, label: &str, fill: egui::Color32) -> egui::Response {
        ui.add_enabled(
            enabled,
            egui::Button::new(
                egui::RichText::new(label)
                    .size(15.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(fill)
            .min_size(egui::vec2(178.0, 36.0)),
        )
    }

    /// A red-filled button for destructive / reboot / DFU actions.
    fn danger_button(ui: &mut egui::Ui, text: &str) -> bool {
        ui.add(
            egui::Button::new(egui::RichText::new(text).color(egui::Color32::WHITE)).fill(DANGER),
        )
        .clicked()
    }

    /// A muted, wrapping description label shown beside a primary button.
    fn desc(ui: &mut egui::Ui, text: &str) {
        ui.add(egui::Label::new(egui::RichText::new(text).color(MUTED)).wrap());
    }

    // ---- top connection bar ------------------------------------------------

    fn connection_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            if let Some(tex) = &self.icon_texture {
                ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(26.0, 26.0)));
                ui.add_space(4.0);
            }
            ui.heading("SlimeNRF Serial Control");
            ui.separator();
            if self.is_connected() {
                let name = self
                    .connection
                    .as_ref()
                    .map(|c| c.port_name.clone())
                    .unwrap_or_default();
                ui.colored_label(egui::Color32::from_rgb(80, 200, 120), format!("● {name}"));
            } else if self.connection.is_some() {
                ui.colored_label(egui::Color32::from_rgb(220, 190, 90), "◌ connecting…");
            } else {
                ui.colored_label(egui::Color32::GRAY, "○ disconnected");
            }
        });

        ui.add_space(4.0);

        // Precompute dropdown contents so the combo closure doesn't borrow self.
        let port_items: Vec<(String, String)> =
            self.ports.iter().map(|p| (p.name.clone(), p.label())).collect();
        let current = self.selected_port.clone();

        ui.horizontal_wrapped(|ui| {
            ui.label("Port:");
            let mut new_selection: Option<String> = None;
            egui::ComboBox::from_id_salt("port_combo")
                .selected_text(current.clone().unwrap_or_else(|| "<no port detected>".to_owned()))
                .width(380.0)
                .show_ui(ui, |ui| {
                    if port_items.is_empty() {
                        ui.label("(no serial ports found)");
                    }
                    for (name, label) in &port_items {
                        let selected = current.as_deref() == Some(name.as_str());
                        if ui.selectable_label(selected, label.as_str()).clicked() {
                            new_selection = Some(name.clone());
                        }
                    }
                });
            if let Some(sel) = new_selection {
                self.selected_port = Some(sel);
            }

            if ui.button("⟳ Refresh").clicked() {
                self.refresh_ports();
            }

            ui.separator();
            ui.label("Baud:");
            egui::ComboBox::from_id_salt("baud_combo")
                .selected_text(self.baud.to_string())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for b in [9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1_000_000] {
                        ui.selectable_value(&mut self.baud, b, b.to_string());
                    }
                })
                .response
                .on_hover_text("Ignored for USB devices — they enumerate as virtual COM ports.");

            ui.separator();
            if self.connection.is_some() {
                if ui.button("⏹ Disconnect").clicked() {
                    self.disconnect();
                }
            } else {
                let enabled = self.selected_port.is_some();
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(
                            egui::RichText::new("▶ Connect").strong().color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT),
                    )
                    .clicked()
                {
                    self.connect(ctx);
                }
            }

            ui.checkbox(&mut self.line_ending_crlf, "CRLF")
                .on_hover_text("Append \\r\\n instead of \\n to each command");
        });

        ui.add_space(2.0);

        let detected = self.current_port_info().and_then(|p| p.guessed_mode);
        ui.horizontal(|ui| {
            ui.label("Mode:");
            ui.selectable_value(&mut self.mode, Mode::Tracker, "🛰  Tracker");
            ui.selectable_value(&mut self.mode, Mode::Receiver, "📡  Receiver");
            if let Some(m) = detected {
                ui.separator();
                ui.label(egui::RichText::new(format!("auto-detected: {}", m.label())).weak());
            }
        });

        ui.add_space(6.0);
    }

    // ---- right-hand console ------------------------------------------------

    fn console_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("console_header").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Console");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        self.log.clear();
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Show:").color(MUTED));
                ui.checkbox(&mut self.show_tx, "sent")
                    .on_hover_text("Echo the commands you send");
                ui.checkbox(&mut self.show_err, "errors")
                    .on_hover_text("Failures and <err> log lines");
                ui.checkbox(&mut self.show_warn, "warnings")
                    .on_hover_text("Validation notices and <wrn> log lines (off by default)");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.autoscroll, "auto-scroll");
                });
            });
            ui.add_space(2.0);
        });

        egui::Panel::bottom("console_input").show_inside(ui, |ui| {
            ui.add_space(4.0);
            let connected = self.is_connected();
            ui.add_enabled_ui(connected, |ui| {
                ui.horizontal(|ui| {
                    let hint = if connected {
                        "raw command — Enter to send"
                    } else {
                        "connect a device to send commands"
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.raw_input)
                            .desired_width(ui.available_width() - 64.0)
                            .hint_text(hint),
                    );
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.button("Send").clicked();
                    if enter || clicked {
                        let cmd = self.raw_input.trim().to_owned();
                        if !cmd.is_empty() {
                            self.send_cmd(cmd);
                        }
                        self.raw_input.clear();
                        resp.request_focus();
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(self.autoscroll)
                .show(ui, |ui| {
                    if self.log.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "No output yet. Connect, then try Calibrate — or type `help`.",
                            )
                            .color(MUTED),
                        );
                    }
                    let maxw = ui.available_width();
                    let mono = egui::FontId::monospace(12.5);
                    for line in &self.log {
                        match line.kind {
                            Kind::Tx if !self.show_tx => continue,
                            Kind::Warn if !self.show_warn => continue,
                            Kind::Err if !self.show_err => continue,
                            _ => {}
                        }
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = maxw;
                        let prefix = line.kind.prefix();
                        if !prefix.is_empty() {
                            job.append(
                                prefix,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: mono.clone(),
                                    color: line.kind.default_color(),
                                    ..Default::default()
                                },
                            );
                        }
                        for seg in &line.segments {
                            let color = seg.color.unwrap_or_else(|| line.kind.default_color());
                            job.append(
                                &seg.text,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: mono.clone(),
                                    color,
                                    ..Default::default()
                                },
                            );
                        }
                        ui.label(job);
                    }
                });
        });
    }

    // ---- tracker view ------------------------------------------------------

    fn tracker_panel(&mut self, ui: &mut egui::Ui) {
        let en = self.is_connected();

        // Common Tasks card
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT, 0.4)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("COMMON TASKS").size(13.0).strong().color(MUTED));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "🧭  Calibrate", ACCENT)
                        .on_hover_text("Sends: calibrate")
                        .clicked()
                    {
                        self.send_cmd("calibrate".into());
                    }
                    Self::desc(ui, "Lay the tracker flat and keep it still, then run this to zero the gyroscope (ZRO).");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "🔗  Pair", ACCENT)
                        .on_hover_text("Sends: pair")
                        .clicked()
                    {
                        self.send_cmd("pair".into());
                    }
                    Self::desc(ui, "Put the tracker into pairing mode so a receiver can bond with it.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "⬆  Update (DFU)", ACCENT_AMBER)
                        .on_hover_text("Sends: dfu")
                        .clicked()
                    {
                        self.send_cmd("dfu".into());
                    }
                    Self::desc(ui, "Reboot into the UF2 bootloader to flash new firmware. The device disconnects afterwards.");
                });
            });

        ui.add_space(10.0);
        self.dfu_update_card(ui);

        ui.add_space(10.0);
        ui.label(egui::RichText::new("ALL COMMANDS").size(13.0).strong().color(MUTED));
        ui.label(egui::RichText::new("Every console command, grouped — expand a section to use it.").color(MUTED));
        ui.add_space(6.0);

        ui.add_enabled_ui(en, |ui| {
            section(ui, "t_info", "📋  Device information", HUE_INFO, true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("info").clicked() { self.send_cmd("info".into()); }
                    if ui.button("uptime").clicked() { self.send_cmd("uptime".into()); }
                    if ui.button("battery").clicked() { self.send_cmd("battery".into()); }
                    if ui.button("nvs").clicked() { self.send_cmd("nvs".into()); }
                    if ui.button("help").clicked() { self.send_cmd("help".into()); }
                    if ui.button("ping").clicked() { self.send_cmd("ping".into()); }
                    if ui.button("meow 🐱").clicked() { self.send_cmd("meow".into()); }
                });
            });

            section(ui, "t_sensor", "🎛  Sensors & calibration", HUE_SENSOR, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("scan").clicked() { self.send_cmd("scan".into()); }
                    if ui.button("calibrate (ZRO)").clicked() { self.send_cmd("calibrate".into()); }
                    if ui.button("6-side").clicked() { self.send_cmd("6-side".into()); }
                    if ui.button("range").clicked() { self.send_cmd("range".into()); }
                    if ui.button("range reset").clicked() { self.send_cmd("range reset".into()); }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("debug duration (1–60 s):");
                    ui.add(egui::TextEdit::singleline(&mut self.t_debug_dur).desired_width(50.0).hint_text("1"));
                    if ui.button("debug").clicked() {
                        let d = self.t_debug_dur.trim().to_owned();
                        if d.is_empty() { self.send_cmd("debug".into()); } else { self.send_cmd(format!("debug {d}")); }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("gyro sensitivity (deg diff) X/Y/Z:");
                    ui.add(egui::TextEdit::singleline(&mut self.t_sens_x).desired_width(56.0).hint_text("x"));
                    ui.add(egui::TextEdit::singleline(&mut self.t_sens_y).desired_width(56.0).hint_text("y"));
                    ui.add(egui::TextEdit::singleline(&mut self.t_sens_z).desired_width(56.0).hint_text("z"));
                    if ui.button("set sens").clicked() {
                        let x = self.t_sens_x.trim().to_owned();
                        let y = self.t_sens_y.trim().to_owned();
                        let z = self.t_sens_z.trim().to_owned();
                        if x.is_empty() || y.is_empty() || z.is_empty() {
                            self.push_info("Enter all three sens values (X, Y, Z).".into());
                        } else {
                            self.send_cmd(format!("sens {x},{y},{z}"));
                        }
                    }
                    if ui.button("sens reset").clicked() { self.send_cmd("sens reset".into()); }
                });
            });

            section(ui, "t_mag", "🧭  Magnetometer", HUE_MAG, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("status (mag)").clicked() { self.send_cmd("mag".into()); }
                    if ui.button("mag on").clicked() { self.send_cmd("mag on".into()); }
                    if ui.button("mag off").clicked() { self.send_cmd("mag off".into()); }
                    if ui.button("mag clear").clicked() { self.send_cmd("mag clear".into()); }
                    if ui.button("mag cal").clicked() { self.send_cmd("mag cal".into()); }
                });
            });

            section(ui, "t_tcal", "🌡  Temperature calibration (tcal)", HUE_TEMP, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal status").clicked() { self.send_cmd("tcal status".into()); }
                    if ui.button("tcal on").clicked() { self.send_cmd("tcal on".into()); }
                    if ui.button("tcal off").clicked() { self.send_cmd("tcal off".into()); }
                    if ui.button("tcal dump").clicked() { self.send_cmd("tcal dump".into()); }
                    if ui.button("tcal check").clicked() { self.send_cmd("tcal check".into()); }
                    if ui.button("tcal clear").clicked() { self.send_cmd("tcal clear".into()); }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal auto on").clicked() { self.send_cmd("tcal auto on".into()); }
                    if ui.button("tcal auto off").clicked() { self.send_cmd("tcal auto off".into()); }
                    if ui.button("tcal boot on").clicked() { self.send_cmd("tcal boot on".into()); }
                    if ui.button("tcal boot off").clicked() { self.send_cmd("tcal boot off".into()); }
                });
                ui.horizontal(|ui| {
                    ui.label("test temp (°C):");
                    ui.add(egui::TextEdit::singleline(&mut self.t_tcal_test).desired_width(60.0).hint_text("current"));
                    if ui.button("tcal test").clicked() {
                        let t = self.t_tcal_test.trim().to_owned();
                        if t.is_empty() { self.send_cmd("tcal test".into()); } else { self.send_cmd(format!("tcal test {t}")); }
                    }
                    ui.separator();
                    ui.label("remove index:");
                    ui.add(egui::TextEdit::singleline(&mut self.t_tcal_remove).desired_width(50.0).hint_text("0"));
                    if ui.button("tcal remove").clicked() {
                        let i = self.t_tcal_remove.trim().to_owned();
                        if i.is_empty() { self.push_info("Enter an index to remove.".into()); } else { self.send_cmd(format!("tcal remove {i}")); }
                    }
                });
            });

            section(ui, "t_conn", "🔗  Connection & pairing", HUE_CONN, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("receiver address (16 hex):");
                    ui.add(egui::TextEdit::singleline(&mut self.t_set_addr).desired_width(170.0).hint_text("0011223344556677"));
                    if ui.button("set").clicked() {
                        let a = self.t_set_addr.trim().to_owned();
                        if a.is_empty() { self.push_info("Enter a 16 hex-digit address.".into()); } else { self.send_cmd(format!("set {a}")); }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("pair").clicked() { self.send_cmd("pair".into()); }
                    if Self::danger_button(ui, "clear pairing") { self.send_cmd("clear".into()); }
                    ui.separator();
                    if ui.button("tdma on").clicked() { self.send_cmd("tdma on".into()); }
                    if ui.button("tdma off").clicked() { self.send_cmd("tdma off".into()); }
                });
                ui.horizontal(|ui| {
                    ui.label("RF channel (1–100):");
                    ui.add(egui::TextEdit::singleline(&mut self.t_channel).desired_width(56.0).hint_text("25"));
                    if ui.button("set channel").clicked() {
                        let c = self.t_channel.trim().to_owned();
                        if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("channel {c}")); }
                    }
                    if ui.button("clearchannel").clicked() { self.send_cmd("clearchannel".into()); }
                });
            });

            section(ui, "t_system", "⚙  System", HUE_SYSTEM, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reboot").clicked() { self.send_cmd("reboot".into()); }
                    if Self::danger_button(ui, "shutdown") { self.send_cmd("shutdown".into()); }
                    if Self::danger_button(ui, "dfu (UF2)") { self.send_cmd("dfu".into()); }
                    if Self::danger_button(ui, "dfu ota") { self.send_cmd("dfu ota".into()); }
                });
            });

            section(ui, "t_reset", "♻  Reset / clear (careful)", HUE_RESET, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reset zro").clicked() { self.send_cmd("reset zro".into()); }
                    if ui.button("reset acc").clicked() { self.send_cmd("reset acc".into()); }
                    if ui.button("reset sens").clicked() { self.send_cmd("reset sens".into()); }
                    if ui.button("reset tcal").clicked() { self.send_cmd("reset tcal".into()); }
                    if ui.button("reset mag").clicked() { self.send_cmd("reset mag".into()); }
                    if ui.button("reset bat").clicked() { self.send_cmd("reset bat".into()); }
                    if ui.button("reset fusion").clicked() { self.send_cmd("reset fusion".into()); }
                    if Self::danger_button(ui, "reset all") { self.send_cmd("reset all".into()); }
                });
            });

            section(ui, "t_test", "🧪  Test mode", HUE_TEST, false, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("test on").clicked() { self.send_cmd("test on".into()); }
                    if ui.button("test off").clicked() { self.send_cmd("test off".into()); }
                });
            });
        });

        ui.add_space(8.0);
    }

    // ---- receiver view -----------------------------------------------------

    fn receiver_panel(&mut self, ui: &mut egui::Ui) {
        let en = self.is_connected();

        // Common Tasks card
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT, 0.4)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("COMMON TASKS").size(13.0).strong().color(MUTED));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "🔗  Pair a tracker", ACCENT)
                        .on_hover_text("Sends: pair")
                        .clicked()
                    {
                        self.send_cmd("pair".into());
                    }
                    Self::desc(ui, "Listen for nearby trackers in pairing mode and bond them to this receiver.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "🧭  Calibrate all", ACCENT)
                        .on_hover_text("Sends: send all calibrate")
                        .clicked()
                    {
                        self.send_cmd("send all calibrate".into());
                    }
                    Self::desc(ui, "Tell every connected tracker to zero its gyroscope at once. Lay them all flat and still first.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if Self::primary(ui, en, "⬆  Update (DFU)", ACCENT_AMBER)
                        .on_hover_text("Sends: dfu")
                        .clicked()
                    {
                        self.send_cmd("dfu".into());
                    }
                    Self::desc(ui, "Reboot the receiver into its bootloader to flash new firmware. It disconnects afterwards.");
                });
            });

        ui.add_space(10.0);
        ui.label(egui::RichText::new("ALL COMMANDS").size(13.0).strong().color(MUTED));
        ui.label(egui::RichText::new("Local dongle commands, plus an over-the-air relay to paired trackers at the bottom.").color(MUTED));
        ui.add_space(6.0);

        ui.add_enabled_ui(en, |ui| {
            section(ui, "r_info", "📋  Device information", HUE_INFO, true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("info").clicked() { self.send_cmd("info".into()); }
                    if ui.button("uptime").clicked() { self.send_cmd("uptime".into()); }
                    if ui.button("list (paired)").clicked() { self.send_cmd("list".into()); }
                    if ui.button("help").clicked() { self.send_cmd("help".into()); }
                    if ui.button("meow 🐱").clicked() { self.send_cmd("meow".into()); }
                });
            });

            section(ui, "r_paired", "🔗  Paired devices", HUE_CONN, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("add address (12 hex):");
                    ui.add(egui::TextEdit::singleline(&mut self.r_add_addr).desired_width(150.0).hint_text("001122334455"));
                    if ui.button("add").clicked() {
                        let a = self.r_add_addr.trim().to_owned();
                        if a.is_empty() { self.push_info("Enter a 12 hex-digit address.".into()); } else { self.send_cmd(format!("add {a}")); }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("pair count (blank = until timeout):");
                    ui.add(egui::TextEdit::singleline(&mut self.r_pair_count).desired_width(50.0).hint_text("∞"));
                    if ui.button("pair").clicked() {
                        let c = self.r_pair_count.trim().to_owned();
                        if c.is_empty() { self.send_cmd("pair".into()); } else { self.send_cmd(format!("pair {c}")); }
                    }
                    if ui.button("exit pairing").clicked() { self.send_cmd("exit".into()); }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("remove last").clicked() { self.send_cmd("remove".into()); }
                    if Self::danger_button(ui, "clear all pairings") { self.send_cmd("clear".into()); }
                });
            });

            section(ui, "r_stats", "📊  Statistics", HUE_STATS, false, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("stats (toggle)").clicked() { self.send_cmd("stats".into()); }
                    ui.separator();
                    ui.label("for N seconds:");
                    ui.add(egui::TextEdit::singleline(&mut self.r_stats_sec).desired_width(50.0).hint_text("30"));
                    if ui.button("stats N").clicked() {
                        let s = self.r_stats_sec.trim().to_owned();
                        if s.is_empty() { self.push_info("Enter a duration in seconds.".into()); } else { self.send_cmd(format!("stats {s}")); }
                    }
                    if ui.button("resetstats").clicked() { self.send_cmd("resetstats".into()); }
                });
            });

            section(ui, "r_channel", "📡  RF channel (local receiver)", HUE_SENSOR, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("channel (1–100):");
                    ui.add(egui::TextEdit::singleline(&mut self.r_channel).desired_width(56.0).hint_text("25"));
                    if ui.button("set channel").clicked() {
                        let c = self.r_channel.trim().to_owned();
                        if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("channel {c}")); }
                    }
                    if ui.button("clearchannel").clicked() { self.send_cmd("clearchannel".into()); }
                    ui.separator();
                    if ui.button("rssi_scan").clicked() { self.send_cmd("rssi_scan".into()); }
                });
            });

            section(ui, "r_system", "⚙  System", HUE_SYSTEM, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reboot").clicked() { self.send_cmd("reboot".into()); }
                    if Self::danger_button(ui, "dfu (UF2)") { self.send_cmd("dfu".into()); }
                    if Self::danger_button(ui, "dfu ota") { self.send_cmd("dfu ota".into()); }
                });
            });

            section(ui, "r_data", "💾  Data collection & OTA", HUE_DATA, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("collect from tracker id:");
                    ui.add(egui::TextEdit::singleline(&mut self.r_collect_id).desired_width(50.0).hint_text("0"));
                    if ui.button("collect").clicked() {
                        let i = self.r_collect_id.trim().to_owned();
                        if i.is_empty() { self.push_info("Enter a tracker id.".into()); } else { self.send_cmd(format!("collect {i}")); }
                    }
                    if ui.button("collect off").clicked() { self.send_cmd("collect off".into()); }
                    if ui.button("collect status").clicked() { self.send_cmd("collect".into()); }
                });
                ui.horizontal(|ui| {
                    if ui.button("ota status").clicked() { self.send_cmd("ota".into()); }
                    ui.separator();
                    ui.label("ota info id:");
                    ui.add(egui::TextEdit::singleline(&mut self.r_ota_info).desired_width(50.0).hint_text("0"));
                    if ui.button("ota info").clicked() {
                        let i = self.r_ota_info.trim().to_owned();
                        if i.is_empty() { self.push_info("Enter a tracker id.".into()); } else { self.send_cmd(format!("ota info {i}")); }
                    }
                    if Self::danger_button(ui, "ota abort") { self.send_cmd("ota abort".into()); }
                });
            });

            section(ui, "r_remote", "🛰  Remote commands → tracker(s)", HUE_REMOTE, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Target:");
                    ui.selectable_value(&mut self.rem_target_all, true, "All active");
                    ui.selectable_value(&mut self.rem_target_all, false, "By ID");
                    let allow_id_edit = !self.rem_target_all;
                    ui.add_enabled(
                        allow_id_edit,
                        egui::TextEdit::singleline(&mut self.rem_target_id).desired_width(50.0).hint_text("0"),
                    );
                });

                let target = self.remote_target();
                ui.label(egui::RichText::new(format!("→ send {target} <command>")).color(MUTED).monospace());
                ui.separator();

                ui.horizontal_wrapped(|ui| {
                    if ui.button("calibrate").clicked() { self.send_cmd(format!("send {target} calibrate")); }
                    if ui.button("6-side").clicked() { self.send_cmd(format!("send {target} 6-side")); }
                    if ui.button("scan").clicked() { self.send_cmd(format!("send {target} scan")); }
                    if ui.button("ping").clicked() { self.send_cmd(format!("send {target} ping")); }
                    if ui.button("meow 🐱").clicked() { self.send_cmd(format!("send {target} meow")); }
                    if ui.button("reboot").clicked() { self.send_cmd(format!("send {target} reboot")); }
                    if ui.button("fusion reset").clicked() { self.send_cmd(format!("send {target} fusion")); }
                    if Self::danger_button(ui, "shutdown") { self.send_cmd(format!("send {target} shutdown")); }
                    if Self::danger_button(ui, "clear pairing") { self.send_cmd(format!("send {target} clear")); }
                });

                ui.add_space(2.0);
                ui.label("Magnetometer:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("mag on").clicked() { self.send_cmd(format!("send {target} mag on")); }
                    if ui.button("mag off").clicked() { self.send_cmd(format!("send {target} mag off")); }
                    if ui.button("mag clear").clicked() { self.send_cmd(format!("send {target} mag clear")); }
                    if ui.button("mag cal").clicked() { self.send_cmd(format!("send {target} mag cal")); }
                });

                ui.add_space(2.0);
                ui.label("Reset:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reset zro").clicked() { self.send_cmd(format!("send {target} reset zro")); }
                    if ui.button("reset acc").clicked() { self.send_cmd(format!("send {target} reset acc")); }
                    if ui.button("reset bat").clicked() { self.send_cmd(format!("send {target} reset bat")); }
                    if ui.button("reset mag").clicked() { self.send_cmd(format!("send {target} reset mag")); }
                    if ui.button("reset tcal").clicked() { self.send_cmd(format!("send {target} reset tcal")); }
                    if ui.button("reset fusion").clicked() { self.send_cmd(format!("send {target} reset fusion")); }
                });

                ui.add_space(2.0);
                ui.label("Temperature calibration:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal on").clicked() { self.send_cmd(format!("send {target} tcal on")); }
                    if ui.button("tcal off").clicked() { self.send_cmd(format!("send {target} tcal off")); }
                    if ui.button("tcal auto on").clicked() { self.send_cmd(format!("send {target} tcal auto on")); }
                    if ui.button("tcal auto off").clicked() { self.send_cmd(format!("send {target} tcal auto off")); }
                    if ui.button("tcal boot on").clicked() { self.send_cmd(format!("send {target} tcal boot on")); }
                    if ui.button("tcal boot off").clicked() { self.send_cmd(format!("send {target} tcal boot off")); }
                    if ui.button("tcal clear").clicked() { self.send_cmd(format!("send {target} tcal clear")); }
                });

                ui.add_space(2.0);
                ui.label("Scheduling / test / bootloader:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tdma on").clicked() { self.send_cmd(format!("send {target} tdma on")); }
                    if ui.button("tdma off").clicked() { self.send_cmd(format!("send {target} tdma off")); }
                    if ui.button("test on").clicked() { self.send_cmd(format!("send {target} test on")); }
                    if ui.button("test off").clicked() { self.send_cmd(format!("send {target} test off")); }
                    if Self::danger_button(ui, "dfu") { self.send_cmd(format!("send {target} dfu")); }
                    if Self::danger_button(ui, "dfu ota") { self.send_cmd(format!("send {target} dfu ota")); }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("sens X/Y/Z:");
                    ui.add(egui::TextEdit::singleline(&mut self.rem_sens_x).desired_width(56.0).hint_text("x"));
                    ui.add(egui::TextEdit::singleline(&mut self.rem_sens_y).desired_width(56.0).hint_text("y"));
                    ui.add(egui::TextEdit::singleline(&mut self.rem_sens_z).desired_width(56.0).hint_text("z"));
                    if ui.button("send sens").clicked() {
                        let x = self.rem_sens_x.trim().to_owned();
                        let y = self.rem_sens_y.trim().to_owned();
                        let z = self.rem_sens_z.trim().to_owned();
                        if x.is_empty() || y.is_empty() || z.is_empty() {
                            self.push_info("Enter all three sens values.".into());
                        } else {
                            self.send_cmd(format!("send {target} sens {x},{y},{z}"));
                        }
                    }
                    if ui.button("send sens reset").clicked() { self.send_cmd(format!("send {target} sens reset")); }
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Channel commands apply to ALL trackers + receiver (firmware restriction):")
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.rem_channel).desired_width(56.0).hint_text("25"));
                    if ui.button("send all channel").clicked() {
                        let c = self.rem_channel.trim().to_owned();
                        if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("send all channel {c}")); }
                    }
                    if ui.button("send all clearchannel").clicked() { self.send_cmd("send all clearchannel".into()); }
                });
            });
        });

        ui.add_space(8.0);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.poll();
        self.poll_dfu();

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
                                        egui::RichText::new("①")
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

        if self.connection.is_some() || self.dfu_running {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}