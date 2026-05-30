# SlimeNRF Serial Control

A cross-platform (Linux + Windows + macOS) desktop GUI for driving the USB-serial
consoles of the [`SlimeVR-Tracker-nRF`](https://github.com/jitingcn/SlimeVR-Tracker-nRF)
tracker firmware and the
[`SlimeVR-Tracker-nRF-Receiver`](https://github.com/jitingcn/SlimeVR-Tracker-nRF-Receiver)
receiver firmware.

It exposes **every** console command of both firmwares as buttons + input fields,
plus a live console and a raw-command box. Two modes — **Tracker** and **Receiver** —
are auto-selected from the device's USB ID.

Built with [`egui`/`eframe`](https://github.com/emilk/egui) (immediate-mode GUI, single
static binary, no system webview) and [`serialport`](https://crates.io/crates/serialport)
(cross-platform serial + USB VID/PID enumeration).

---

## Features

- **Auto-detect role.** SlimeNRF devices use VID `0x1209`; PID `0x7690` = receiver,
  `0x7692` = tracker. Detected devices sort to the top of the port list and pick the
  matching mode automatically (with a manual override).
- **Tracker mode** — info / uptime / battery / nvs, scan, ZRO + 6-side calibration,
  sensor debug & range stats, magnetometer control, temperature calibration (`tcal`,
  incl. auto/boot/test/remove), gyro sensitivity, pairing (`set`/`pair`/`clear`), TDMA,
  RF channel, reboot/shutdown/DFU, the full `reset …` set, and test mode.
- **Receiver mode** — info / uptime / paired list, add/remove/pair/exit/clear, packet
  stats, local RF channel + `rssi_scan`, reboot/DFU, data collection & ESB OTA, and a
  full **remote command relay** (`send <id|all> …`) to push commands over the air to
  trackers.
- **Live console** with TX/RX colouring, autoscroll, sent-line filter, and a raw
  command entry (Enter to send) for anything not surfaced as a button.
- Destructive actions (shutdown, clear, reset all, DFU, ota abort) are styled red.

> Commands are sent terminated with `\n` (toggle **CRLF** for `\r\n`). Baud rate is
> selectable but **irrelevant for USB-CDC** devices — it's a virtual COM port.

---

## Build & run

### 1. Install Rust
Get the toolchain from <https://rustup.rs>.

### 2. Platform prerequisites

**Linux** — port enumeration uses `libudev`, so install its dev package + `pkg-config`:

| Distro            | Command                                              |
|-------------------|------------------------------------------------------|
| Debian / Ubuntu   | `sudo apt install pkg-config libudev-dev`            |
| Fedora            | `sudo dnf install pkgconf-pkg-config systemd-devel`  |
| Arch              | `sudo pacman -S pkgconf systemd-libs`                |

(eframe also needs the usual X11/Wayland + OpenGL runtime, which is present on any
normal desktop install.)

**Windows** — no extra dependencies. Use the default MSVC toolchain
(`rustup default stable-x86_64-pc-windows-msvc`). The release build hides the console
window.

**macOS** — no extra dependencies.

### 3. Build

```bash
cargo run --release
```

The standalone binary lands in `target/release/`.

---

## Linux serial permissions

If the port shows up but won't open, your user probably lacks access to the tty:

```bash
sudo usermod -aG dialout "$USER"   # Debian/Ubuntu (use 'uucp' on Arch)
# then log out and back in
```

---

## Usage

1. Plug in a tracker or receiver and click **⟳ Refresh** if it isn't already listed.
2. Pick the port (SlimeNRF devices are listed first with their product name + USB ID).
3. The **Mode** is auto-selected; switch manually if needed.
4. Click **▶ Connect**.
5. Drive the device with the buttons, or type a raw command in the console and press
   Enter. Try `help` or `info` first.

### Receiver → tracker remote commands
In Receiver mode, the **"Remote commands → tracker(s)"** section sends commands over
the air. Choose **All active** or **By ID** (enter the tracker id), then click a command.
Channel/clearchannel are firmware-restricted to the `all` target and have dedicated
buttons.

---

## Command reference

These are the exact console commands the GUI emits (also runnable from the raw box).

### Tracker (direct)
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

### Receiver (local)
```
info  uptime  list  help  meow
add <12-hex>  remove  pair [count]  exit  clear
stats | stats <sec>  resetstats
channel <1-100>  clearchannel  rssi_scan
reboot  dfu  dfu ota
collect <id> | collect off | collect      ota | ota info <id> | ota abort | ota status
```

### Receiver → tracker (relayed: `send <id|all> …`)
```
shutdown  reboot  calibrate  6-side  scan  meow  ping  clear  fusion
mag on|off|clear|cal
reset zro|acc|bat|mag|tcal|fusion
sens <x>,<y>,<z> | sens reset
tcal on|off|auto on|auto off|boot on|boot off|clear
tdma on|off    test on|off    dfu | dfu ota
channel <1-100>   clearchannel       (all target only)
```

---

## Project layout

```
src/
  main.rs     eframe bootstrap (window, dark theme)
  serial.rs   port enumeration, USB-ID role detection, background worker thread
  app.rs      egui UI: connection bar, console, tracker & receiver command panels
```

## License

Dual-licensed under MIT or Apache-2.0, matching the firmware projects.
