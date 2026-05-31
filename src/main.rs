// Hide the extra console window on Windows release builds (GUI app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dfu;
mod serial;

use app::App;

fn main() -> eframe::Result<()> {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 820.0])
        .with_min_inner_size([900.0, 600.0])
        .with_title("SlimeNRF Serial Control")
        // Stable window identity. Becomes the X11 WM_CLASS, so tiling-WM users can
        // match this window in their rules. (No .desktop file is shipped — see the
        // note on the window icon below for the Wayland consequence.)
        .with_app_id("slimenrf-serial-gui");

    // Window icon, embedded in the binary (no external file needed). Shown in the
    // titlebar, Alt-Tab and taskbar under X11 window managers, and on Windows.
    // Wayland has no protocol to set a per-window icon from the client, so under a
    // Wayland session this has no effect — the only fix there is installing a
    // .desktop file, which this project deliberately does not require.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "SlimeNRF Serial Control",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)) as Box<dyn eframe::App>)),
    )
}