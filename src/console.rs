//! Console text model and ANSI handling.
//!
//! The serial console renders firmware output that may contain ANSI SGR colour
//! escapes (including xterm-256 and truecolour). This module turns a raw line into
//! coloured [`Segment`]s and classifies its [`Kind`] (which drives both the default
//! colour and the show/hide filters). It is pure: no UI or app state.

use eframe::egui;

/// Category of a console line — drives both colour and the show/hide filters.
#[derive(Copy, Clone, PartialEq)]
pub enum Kind {
    Tx,
    Rx,
    Info,
    Warn,
    Err,
}

impl Kind {
    /// Default colour for text that carries no explicit ANSI colour of its own.
    pub fn default_color(self) -> egui::Color32 {
        match self {
            Kind::Tx => egui::Color32::from_rgb(120, 170, 255),
            Kind::Rx => egui::Color32::from_rgb(222, 224, 228),
            Kind::Info => egui::Color32::from_rgb(150, 160, 172),
            Kind::Warn => egui::Color32::from_rgb(222, 192, 92),
            Kind::Err => egui::Color32::from_rgb(240, 104, 104),
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Tx => "» ",
            Kind::Rx => "",
            Kind::Info => "· ",
            Kind::Warn => "! ",
            Kind::Err => "x ",
        }
    }
}

/// A run of text sharing one colour. `color == None` means "use the line's Kind
/// colour" (i.e. the firmware didn't colour this run with an ANSI escape).
pub struct Segment {
    pub text: String,
    pub color: Option<egui::Color32>,
}

pub struct LogLine {
    pub kind: Kind,
    pub segments: Vec<Segment>,
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
pub fn parse_ansi(input: &str) -> Vec<Segment> {
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
pub fn classify_rx(plain: &str) -> Kind {
    let l = plain.to_ascii_lowercase();
    if l.contains("<err>") {
        Kind::Err
    } else if l.contains("<wrn>") {
        Kind::Warn
    } else {
        Kind::Rx
    }
}
