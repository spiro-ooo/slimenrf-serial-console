//! Batch firmware update over the Adafruit nRF52 UF2 bootloader.
//!
//! The SlimeNRF tracker firmware enters DFU by setting the Adafruit "UF2 reset"
//! magic and rebooting (the `dfu` console command). The chip then re-enumerates as
//! a USB **mass-storage** device exposing a small FAT volume — the UF2 bootloader.
//! Dropping a `.uf2` file onto that volume flashes it and the board reboots.
//!
//! So updating every plugged-in tracker is:
//!   1. send `dfu` to each tracker serial port,
//!   2. watch for new UF2 volumes to appear,
//!   3. copy the chosen `.uf2` onto each one.
//!
//! ## Detecting the volume (the "newly connected device" problem)
//! UF2 bootloaders are *not* identified by volume label (those differ per board).
//! Every UF2 volume contains a signature file, `INFO_UF2.TXT`, which also carries
//! the board model and id. We therefore detect a UF2 drive by the presence of that
//! file, and tell trackers apart from any unrelated UF2 device by diffing against a
//! snapshot of the volumes that existed *before* we sent `dfu`.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use eframe::egui;

/// Filename written by every UF2 bootloader — our detection signature.
const UF2_INFO_FILE: &str = "INFO_UF2.TXT";

/// A mounted UF2 bootloader volume.
#[derive(Clone, Debug)]
pub struct Uf2Drive {
    /// Mount point (Linux/macOS) or drive root like `E:\` (Windows).
    pub mount: PathBuf,
    /// Volume label / mount basename, for display.
    pub label: String,
    /// `Model:` line from INFO_UF2.TXT, if present.
    pub model: Option<String>,
    /// `Board-ID:` line from INFO_UF2.TXT, if present.
    pub board_id: Option<String>,
}

impl Uf2Drive {
    pub fn describe(&self) -> String {
        match (&self.model, &self.board_id) {
            (Some(m), _) => format!("{} ({})", m, self.mount.display()),
            (None, Some(b)) => format!("{} ({})", b, self.mount.display()),
            _ => format!("{} ({})", self.label, self.mount.display()),
        }
    }
}

/// Progress events streamed from the batch-update worker to the UI.
pub enum DfuProgress {
    /// A human-readable step / status line.
    Status(String),
    /// A tracker port was (or wasn't) successfully told to enter DFU.
    PortTriggered { port: String, ok: bool },
    /// A new UF2 volume appeared and we're about to flash it.
    DriveFound(Uf2Drive),
    /// Firmware copied successfully to this mount.
    Flashed { mount: PathBuf },
    /// A non-fatal problem with one drive/port; the batch continues.
    Warn(String),
    /// The whole operation finished. `flashed` of `expected` succeeded.
    Finished { flashed: usize, expected: usize },
}

/// Read INFO_UF2.TXT (if present) for the `Model:` and `Board-ID:` fields.
fn read_uf2_info(mount: &Path) -> (Option<String>, Option<String>) {
    let path = mount.join(UF2_INFO_FILE);
    let Ok(text) = fs::read_to_string(&path) else {
        return (None, None);
    };
    let mut model = None;
    let mut board = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Model:") {
            model = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("Board-ID:") {
            board = Some(rest.trim().to_owned());
        }
    }
    (model, board)
}

/// Does this directory look like a mounted UF2 bootloader volume?
fn is_uf2_mount(dir: &Path) -> bool {
    dir.join(UF2_INFO_FILE).is_file()
}

fn make_drive(mount: PathBuf) -> Uf2Drive {
    let label = mount
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| mount.display().to_string());
    let (model, board_id) = read_uf2_info(&mount);
    Uf2Drive {
        mount,
        label,
        model,
        board_id,
    }
}

/// Candidate mount points to probe on Unix: everything currently mounted (from
/// `/proc/mounts`) plus the usual removable-media roots, in case a probe races
/// the mount table.
#[cfg(unix)]
fn candidate_mounts() -> Vec<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();

    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            // fields: device mountpoint fstype opts ...
            let mut it = line.split_whitespace();
            let _dev = it.next();
            if let Some(mp) = it.next() {
                // octal-escaped spaces etc. appear as \040; good enough to unescape space.
                let mp = mp.replace("\\040", " ");
                cands.push(PathBuf::from(mp));
            }
        }
    }

    // Desktop auto-mount roots: /run/media/<user>/*, /media/<user>/*, /media/*, /mnt/*.
    let user = std::env::var("USER").unwrap_or_default();
    let roots = [
        format!("/run/media/{user}"),
        format!("/media/{user}"),
        "/media".to_owned(),
        "/mnt".to_owned(),
        "/Volumes".to_owned(), // macOS
    ];
    for root in roots {
        if let Ok(entries) = fs::read_dir(&root) {
            for e in entries.flatten() {
                cands.push(e.path());
            }
        }
    }

    cands.sort();
    cands.dedup();
    cands
}

/// On Windows, probe drive letters C..=Z for the signature file.
#[cfg(windows)]
fn candidate_mounts() -> Vec<PathBuf> {
    let mut cands = Vec::new();
    for letter in b'C'..=b'Z' {
        cands.push(PathBuf::from(format!("{}:\\", letter as char)));
    }
    cands
}

/// Scan the system for mounted UF2 bootloader volumes.
pub fn find_uf2_drives() -> Vec<Uf2Drive> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for mount in candidate_mounts() {
        if !seen.insert(mount.clone()) {
            continue;
        }
        // is_uf2_mount touches the filesystem; guard against slow/again-unmounted paths.
        if is_uf2_mount(&mount) {
            out.push(make_drive(mount));
        }
    }
    out
}

/// Set of mount paths, for diffing "before" vs "after".
pub fn drive_key_set(drives: &[Uf2Drive]) -> HashSet<PathBuf> {
    drives.iter().map(|d| d.mount.clone()).collect()
}

/// Send the `dfu` command to one serial port, then let it reboot. Best-effort:
/// returns whether the command was written. Retries briefly in case the port was
/// just released by the app's own connection.
fn trigger_dfu(port: &str, line_ending: &str) -> bool {
    for attempt in 0..3 {
        let opened = serialport::new(port, 115_200)
            .timeout(Duration::from_millis(200))
            .open();
        match opened {
            Ok(mut p) => {
                // The CDC console only accepts input once DTR is asserted.
                let _ = p.write_data_terminal_ready(true);
                let _ = p.write_request_to_send(true);
                std::thread::sleep(Duration::from_millis(50));
                let payload = format!("dfu{line_ending}");
                if p.write_all(payload.as_bytes()).is_ok() {
                    let _ = p.flush();
                    // Firmware waits ~100ms after setting the magic before resetting;
                    // hold the port open a touch longer so the write lands.
                    std::thread::sleep(Duration::from_millis(200));
                    return true;
                }
            }
            Err(_) if attempt < 2 => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Copy `firmware` into the UF2 volume at `mount`.
///
/// The UF2 bootloader flashes as it receives the file and then **resets the board**,
/// which makes the volume vanish — often *before* the OS finishes the final write or
/// close. So an error late in the copy, combined with the drive disappearing, is the
/// normal success signal, not a failure. We write the bytes, flush best-effort, and
/// then treat "drive is gone" as success.
fn copy_firmware(firmware: &Path, mount: &Path) -> Result<(), String> {
    let data = fs::read(firmware).map_err(|e| format!("cannot read firmware: {e}"))?;
    let total = data.len();

    // Bootloaders accept any *.uf2 filename; use the source name (fall back to fw.uf2).
    let fname = firmware
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("fw.uf2"));
    let dest = mount.join(&fname);

    // Write in chunks so a reset mid-transfer is distinguishable from a real failure.
    let mut wrote = 0usize;
    let mut write_err: Option<std::io::Error> = None;
    match fs::File::create(&dest) {
        Ok(mut f) => {
            const CHUNK: usize = 64 * 1024;
            for chunk in data.chunks(CHUNK) {
                match f.write_all(chunk) {
                    Ok(()) => wrote += chunk.len(),
                    Err(e) => {
                        write_err = Some(e);
                        break;
                    }
                }
            }
            let _ = f.flush();
            let _ = f.sync_all(); // may itself error if the device already reset
        }
        Err(e) => {
            // Couldn't even create the file — but if the drive vanished, the board
            // reset on its own (e.g. someone replugged); only fail if it's still there.
            if is_uf2_mount(mount) {
                return Err(format!("cannot open destination: {e}"));
            }
            return Ok(());
        }
    }

    // Give the bootloader a moment to flash + reset, then check whether the volume
    // disappeared. Disappearance == flashing started == success.
    for _ in 0..40 {
        if !is_uf2_mount(mount) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    // Still mounted after 10s. If we got all the bytes out, it very likely took;
    // some setups keep the (now stale) mount around briefly. Treat a complete write
    // as success, an incomplete one as failure.
    if wrote >= total && write_err.is_none() {
        Ok(())
    } else if let Some(e) = write_err {
        Err(format!(
            "write failed after {wrote}/{total} bytes: {e} (drive did not reset)"
        ))
    } else {
        Err(format!(
            "wrote {wrote}/{total} bytes but the bootloader never reset"
        ))
    }
}

/// Run the whole batch update on a worker thread.
///
/// * `ports` — tracker serial ports to push into DFU.
/// * `firmware` — the `.uf2` to flash onto each.
/// * `pre_existing` — UF2 volumes present *before* we started (so we only flash new
///   ones), unless `include_existing` is set.
/// * `include_existing` — also flash UF2 volumes that were already mounted at start.
/// * `line_ending` — `"\n"` or `"\r\n"`, matching the app's setting.
#[allow(clippy::too_many_arguments)]
pub fn run_dfu_update(
    ports: Vec<String>,
    firmware: PathBuf,
    pre_existing: HashSet<PathBuf>,
    include_existing: bool,
    line_ending: &'static str,
    tx: Sender<DfuProgress>,
    ctx: egui::Context,
) {
    let send = |p: DfuProgress, tx: &Sender<DfuProgress>, ctx: &egui::Context| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };

    // How many drives do we expect to flash?
    let already = if include_existing {
        pre_existing.len()
    } else {
        0
    };
    let expected = ports.len() + already;

    let mut handled: HashSet<PathBuf> = if include_existing {
        // Flash currently-present drives first, then the ones that newly appear.
        HashSet::new()
    } else {
        // Ignore everything that existed before we sent dfu.
        pre_existing.clone()
    };

    let mut flashed = 0usize;

    // If asked, flash the drives that were already in DFU mode up front.
    if include_existing {
        for d in find_uf2_drives() {
            if pre_existing.contains(&d.mount) && handled.insert(d.mount.clone()) {
                send(DfuProgress::DriveFound(d.clone()), &tx, &ctx);
                match copy_firmware(&firmware, &d.mount) {
                    Ok(()) => {
                        flashed += 1;
                        send(DfuProgress::Flashed { mount: d.mount.clone() }, &tx, &ctx);
                    }
                    Err(e) => send(
                        DfuProgress::Warn(format!("{}: {e}", d.describe())),
                        &tx,
                        &ctx,
                    ),
                }
            }
        }
    }

    // 1. Push every tracker port into DFU.
    if ports.is_empty() {
        send(
            DfuProgress::Status("No tracker serial ports to put into DFU.".to_owned()),
            &tx,
            &ctx,
        );
    }
    for port in &ports {
        send(
            DfuProgress::Status(format!("Sending dfu to {port}…")),
            &tx,
            &ctx,
        );
        let ok = trigger_dfu(port, line_ending);
        send(
            DfuProgress::PortTriggered {
                port: port.clone(),
                ok,
            },
            &tx,
            &ctx,
        );
    }

    if ports.is_empty() && !include_existing {
        send(DfuProgress::Finished { flashed, expected }, &tx, &ctx);
        return;
    }

    // 2. Watch for new UF2 volumes and flash each as it appears.
    send(
        DfuProgress::Status("Waiting for trackers to re-appear as UF2 drives…".to_owned()),
        &tx,
        &ctx,
    );

    let deadline = Instant::now() + Duration::from_secs(45);
    let new_target = ports.len(); // number of *newly appearing* drives we hope to see
    let mut new_seen = 0usize;

    while Instant::now() < deadline {
        for d in find_uf2_drives() {
            if handled.contains(&d.mount) {
                continue;
            }
            handled.insert(d.mount.clone());
            new_seen += 1;
            send(DfuProgress::DriveFound(d.clone()), &tx, &ctx);
            match copy_firmware(&firmware, &d.mount) {
                Ok(()) => {
                    flashed += 1;
                    send(
                        DfuProgress::Flashed {
                            mount: d.mount.clone(),
                        },
                        &tx,
                        &ctx,
                    );
                }
                Err(e) => send(
                    DfuProgress::Warn(format!("{}: {e}", d.describe())),
                    &tx,
                    &ctx,
                ),
            }
        }

        if new_seen >= new_target {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    if new_seen < new_target {
        send(
            DfuProgress::Warn(format!(
                "Timed out waiting for {} of {} tracker drive(s). On Linux they must \
                 auto-mount (or be mounted) to be flashed.",
                new_target - new_seen,
                new_target
            )),
            &tx,
            &ctx,
        );
    }

    send(DfuProgress::Finished { flashed, expected }, &tx, &ctx);
}
