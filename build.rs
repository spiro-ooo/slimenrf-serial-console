//! Build script: embed the application icon into the Windows `.exe` so the file
//! itself shows the icon in Explorer (and the taskbar / Alt-Tab) — not just the
//! running window. On every other target this is a no-op.
//!
//! Note: we key off `CARGO_CFG_TARGET_OS` (the *target* being built) rather than
//! `cfg!(windows)`. A build script runs on the host, so when cross-compiling from
//! Linux to Windows (e.g. `cargo xwin build --target x86_64-pc-windows-msvc`),
//! `cfg!(windows)` would be false and the icon would be silently skipped.

fn main() {
    // Rebuild if the icon is swapped out.
    println!("cargo:rerun-if-changed=assets/icon.ico");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Don't fail the whole build if no resource compiler is present in a
            // given cross-compile setup — the runtime window icon still applies,
            // only the icon embedded in the .exe file is skipped.
            println!("cargo:warning=could not embed Windows .exe icon: {e}");
        }
    }
}