
//! Visual theme: the colour palette, the coloured-section styling, and the small
//! reusable widget helpers (primary button, danger button, description label).
//! Pure presentation — no application state.

use eframe::egui;

// ---- palette ---------------------------------------------------------------

pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(74, 125, 205); // primary, safe
pub const ACCENT_AMBER: egui::Color32 = egui::Color32::from_rgb(199, 138, 46); // update / caution
pub const DANGER: egui::Color32 = egui::Color32::from_rgb(170, 62, 62); // destructive
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(166, 171, 180); // secondary text
pub const CARD_BG: egui::Color32 = egui::Color32::from_rgb(33, 37, 46);
pub const BANNER_BG: egui::Color32 = egui::Color32::from_rgb(52, 45, 30);

// Per-section accent hues. Each command section gets its own colour: a coloured
// header, a tinted bordered frame, and tinted buttons. This turns the long list of
// look-alike grey buttons into colour-grouped clusters you can scan at a glance.
// Red is deliberately *not* used as a section hue — it's reserved app-wide for
// destructive actions (see `danger_button`) so "careful" always looks the same.
pub const HUE_INFO: egui::Color32 = egui::Color32::from_rgb(96, 142, 196); // steel blue
pub const HUE_SENSOR: egui::Color32 = egui::Color32::from_rgb(70, 168, 162); // teal
pub const HUE_MAG: egui::Color32 = egui::Color32::from_rgb(150, 124, 206); // violet
pub const HUE_TEMP: egui::Color32 = egui::Color32::from_rgb(202, 150, 74); // amber
pub const HUE_CONN: egui::Color32 = egui::Color32::from_rgb(106, 168, 96); // green
pub const HUE_SYSTEM: egui::Color32 = egui::Color32::from_rgb(120, 140, 158); // slate
pub const HUE_RESET: egui::Color32 = egui::Color32::from_rgb(192, 110, 92); // clay (destructive-ish)
pub const HUE_TEST: egui::Color32 = egui::Color32::from_rgb(196, 116, 170); // magenta
pub const HUE_STATS: egui::Color32 = egui::Color32::from_rgb(86, 156, 188); // cyan
pub const HUE_DATA: egui::Color32 = egui::Color32::from_rgb(170, 150, 96); // ochre
pub const HUE_REMOTE: egui::Color32 = egui::Color32::from_rgb(126, 150, 210); // periwinkle

// ---- colour math -----------------------------------------------------------

/// Mix a colour toward black by `t` (0 = unchanged, 1 = black).
pub fn darken(c: egui::Color32, t: f32) -> egui::Color32 {
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

// ---- coloured collapsing sections ------------------------------------------

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
pub fn section(
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

// ---- reusable widgets ------------------------------------------------------

/// A large, accent-filled primary button (used in the Common Tasks cards).
/// Disabled when `enabled` is false so it greys out before connecting.
pub fn primary(ui: &mut egui::Ui, enabled: bool, label: &str, fill: egui::Color32) -> egui::Response {
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

/// A red-filled button for destructive / reboot / DFU actions, with a tooltip.
pub fn danger_button(ui: &mut egui::Ui, text: &str, tip: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(egui::Color32::WHITE)).fill(DANGER),
    )
    .on_hover_text(tip)
    .clicked()
}

/// A muted, wrapping description label shown beside a primary button.
pub fn desc(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(MUTED)).wrap());
}