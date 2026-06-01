<div align="center">

# SlimeNRF Serial Control

**A cross-platform desktop app to configure, monitor, and _flash_ SlimeNRF trackers and receivers — over USB, with no extra tools.**

[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS-4a7dcd)](#build--run)
[![Built with egui](https://img.shields.io/badge/built%20with-egui%20%2F%20eframe-46a8a2)](https://github.com/emilk/egui)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c78a2e)](#license)

Talks to the [`SlimeVR-Tracker-nRF`](https://github.com/jitingcn/SlimeVR-Tracker-nRF) tracker firmware
and the [`SlimeVR-Tracker-nRF-Receiver`](https://github.com/jitingcn/SlimeVR-Tracker-nRF-Receiver) receiver firmware.

</div>

---

## What it does

Every console command of both firmwares is surfaced as a button or input field, alongside a
live colour console and a raw-command box — so routine setup needs no memorised commands, and
anything not on screen can still be typed. On top of the console, it **flashes firmware in
place**: UF2 trackers by drive-copy, and the receiver dongle over Nordic secure DFU (the job
that normally needs nRF Connect).

Two modes, **Tracker** and **Receiver**, are auto-selected from the device's USB ID.

|                              | Tracker | Receiver |
|------------------------------|:-------:|:--------:|
| Auto-detected by USB ID      |   ✓     |    ✓     |
| Full console command set     |   ✓     |    ✓     |
| Calibration (ZRO / 6-side / temperature) | ✓ | — (relayed) |
| Pairing, RF channel, TDMA    |   ✓     |    ✓     |
| Packet stats / RSSI scan     |   —     |    ✓     |
| Remote command relay to trackers | —   |    ✓     |
| **In-app firmware update**   | ✓ (UF2 drive) | ✓ (Nordic DFU, `.hex`/`.zip`) |

---

## Features

### Configure & monitor
- **Auto-detected role.** SlimeNRF devices use VID `0x1209` (PID `0x7692` = tracker, `0x7690`
  = receiver). A board sitting in a bootloader is recognised too (Adafruit UF2 `0x239A`,
  Nordic DFU `0x1915`). Recognised devices sort to the top and select the right mode, with a
  manual override and a **device line** showing product name, USB ID, and serial number.
- **Common Tasks** cards put the everyday actions — calibrate, pair, update — one click away.
- **Colour-grouped command sections.** Every console command is organised into collapsible,
  colour-coded sections; destructive actions (shutdown, clear, reset all, DFU) are always red.
- **Live console** with ANSI / xterm-256 colour, autoscroll, TX / warning / error filters, and
  a raw command box (press <kbd>Enter</kbd> to send).
- **Receiver → tracker relay.** Push any tracker command over the air with `send <id|all> …`.

### Flash firmware — no nRF Connect needed
- **Trackers (UF2).** Reboots every USB-connected tracker into its UF2 bootloader and copies
  the selected firmware onto each.
- **Receiver (Nordic secure DFU).** Flashes the dongle directly over serial from a `.hex` _or_
  a Nordic DFU `.zip`. For a `.hex`, the DFU init packet is generated in-app (SHA-256, no
  signing) — so you don't need `nrfutil` or nRF Connect at all. Live progress bar and log.

> **Putting the receiver in DFU:** the `dfu` command is unreliable on some hardware, so the
> dependable way is the **magnet** — hold a magnet to the dongle while plugging it in (or hold
> it there ~10 s until the LED changes). The app then detects it automatically.

---

## Build & run

### 1. Install Rust
Get the toolchain from <https://rustup.rs>. (egui 0.34 needs Rust 1.81 or newer.)

### 2. Platform prerequisites

**Linux** — port enumeration uses `libudev`:

| Distro            | Command                                              |
|-------------------|------------------------------------------------------|
| Debian / Ubuntu   | `sudo apt install pkg-config libudev-dev`            |
| Fedora            | `sudo dnf install pkgconf-pkg-config systemd-devel`  |
| Arch / CachyOS    | `sudo pacman -S pkgconf systemd-libs`                |

The file picker uses the XDG desktop portal (no GTK build dependency), and eframe needs the
usual X11 / Wayland + OpenGL runtime present on any normal desktop.

**Windows** — no extra dependencies; use the default MSVC toolchain
(`rustup default stable-x86_64-pc-windows-msvc`). The release build hides the console window
and embeds an application icon.

**macOS** — no extra dependencies.

### 3. Build

```bash
cargo run --release
```

The standalone binary lands in `target/release/`.

---

## Usage

1. Plug in a tracker or receiver and click **Refresh** if it isn't listed.
2. Pick the port — recognised devices are listed first with their product name and USB ID.
3. **Mode** is auto-selected; override it manually if needed.
4. Click **Connect**, then drive the device with the buttons or type a raw command
   (try `help` or `info` first).

### Updating a tracker
Switch to **Tracker** mode, open **Update all trackers**, choose the `.uf2`, and click
**Update**. Trackers paired only wirelessly aren't included — connect them by USB.

### Updating the receiver
Switch to **Receiver** mode and put the dongle in DFU with a magnet (see the note above). Open
**Update receiver firmware**, pick a `.hex` or DFU `.zip`, and click **Flash receiver**.

> Leave **SoftDevice req** at `0x00` — this receiver is ESB-based and has no SoftDevice. Only
> change it if a flash fails with DFU error `0x07`.

### Linux serial permissions
If a port appears but won't open, your user likely lacks tty access:

```bash
sudo usermod -aG dialout "$USER"   # Debian/Ubuntu  (use 'uucp' on Arch)
# then log out and back in
```

> Commands are sent terminated with `\n` (toggle **CRLF** for `\r\n`). Baud rate is selectable
> but **irrelevant for USB-CDC** devices — it's a virtual COM port.

---

## Command reference

The exact console commands the GUI emits (all also runnable from the raw box).

<details>
<summary><b>Tracker (direct)</b></summary>

```
info  uptime  battery  nvs  help  ping  meow
scan  calibrate  6-side  range  range reset  debug [1-60]
mag | mag on | mag off | mag clear | mag cal
sens <x>,<y>,<z> | sens reset
tcal status|on|off|dump|check|clear|auto on|auto off|boot on|boot off|test [temp]|remove <i>
set <16-hex>  pair  clear  tdma on|off  channel <1-100>  clearchannel
reboot  shutdown  dfu  dfu ota
reset zro|acc|sens|tcal|mag|bat|fusion|all
test on|off
```
</details>

<details>
<summary><b>Receiver (local)</b></summary>

```
info  uptime  list  help  meow
add <12-hex>  remove  pair [count]  exit  clear
stats | stats <sec>  resetstats
channel <1-100>  clearchannel  rssi_scan
reboot  dfu  dfu ota
collect <id> | collect off | collect      ota | ota info <id> | ota abort | ota status
```
</details>

<details>
<summary><b>Receiver → tracker (relayed: <code>send &lt;id|all&gt; …</code>)</b></summary>

```
shutdown  reboot  calibrate  6-side  scan  meow  ping  clear  fusion
mag on|off|clear|cal
reset zro|acc|bat|mag|tcal|fusion
sens <x>,<y>,<z> | sens reset
tcal on|off|auto on|auto off|boot on|boot off|clear
tdma on|off    test on|off    dfu | dfu ota
channel <1-100>   clearchannel       (all target only)
```
</details>

---

## How it works

Built with [`egui` / `eframe`](https://github.com/emilk/egui) (immediate-mode GUI, single
static binary, no system webview) and [`serialport`](https://crates.io/crates/serialport)
(cross-platform serial + USB VID/PID enumeration). A background worker thread owns the open
port and exchanges messages with the UI, so the interface never blocks on serial I/O.

Firmware flashing is implemented from the protocol up: UF2 trackers via bootloader drive
detection and file copy; the receiver via the Nordic secure DFU protocol over serial
(SLIP framing, the select/create/write/CRC/execute object flow), including Intel-HEX parsing
and in-app DFU init-packet generation.

```
src/
  main.rs      eframe bootstrap — window, dark theme, app icon
  app.rs       UI: connection bar, console, tracker & receiver panels, updater cards
  theme.rs     colour palette, coloured-section styling, reusable widgets
  console.rs   console text model + ANSI / xterm-256 parsing
  serial.rs    port enumeration, USB-ID role & bootloader detection, worker thread
  dfu.rs       tracker firmware update (UF2 drive-copy)
  nrfdfu.rs    receiver firmware update (Nordic secure DFU over serial; .hex/.zip)
```

---

## License

Dual-licensed under **MIT** or **Apache-2.0**, matching the firmware projects.