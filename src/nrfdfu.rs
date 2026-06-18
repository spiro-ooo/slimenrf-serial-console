//! Nordic **secure DFU over serial** — for flashing the receiver dongle.
//!
//! The SlimeNRF receiver on a Holyiot / nRF52840 dongle runs the Nordic Open
//! (secure) bootloader, which lives in a protected flash region at 0xE0000 and is
//! *not* a UF2 mass-storage bootloader. It speaks the nRF DFU protocol over a USB
//! CDC serial port, SLIP-framed — exactly what `nrfutil dfu serial` and the nRF
//! Connect Programmer use. This module reimplements that transport so the receiver
//! can be flashed from inside the app, without nRF Connect.
//!
//! ## What we transmit
//! A Nordic DFU package is a `.zip` containing a `manifest.json` plus, per image,
//! an **init packet** (`.dat`, a signed protobuf) and the **firmware** (`.bin`).
//! We do not generate or parse the init packet — it is signed at build time — we
//! transmit it byte-for-byte. So this is purely a transport, not a crypto/protobuf
//! implementation.
//!
//! ## Protocol (verified against Nordic's pc-nrfutil dfu_transport_serial.py)
//! Framing: SLIP (END=0xC0, ESC=0xDB, ESC_END=0xDC, ESC_ESC=0xDD).
//! Multi-byte fields are little-endian. Requests are `[opcode, params…]`; responses
//! are `[0x60, opcode, result, payload…]` with result 0x01 = success.
//!
//! Handshake: Ping → SetPRN(0) → GetSerialMTU. Then for the init packet (object
//! type 0x01) and again for the firmware (type 0x02): Select → for each object
//! Create → Write (chunked) → CRC-get (verify) → Execute.
//!
//! With PRN = 0 the target sends no mid-stream receipts, so we request a checksum
//! once per object rather than every N packets.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use eframe::egui;
use serde::Deserialize;

// ---- SLIP framing ----------------------------------------------------------

const SLIP_END: u8 = 0xC0;
const SLIP_ESC: u8 = 0xDB;
const SLIP_ESC_END: u8 = 0xDC;
const SLIP_ESC_ESC: u8 = 0xDD;

fn slip_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 2);
    for &b in data {
        match b {
            SLIP_END => {
                out.push(SLIP_ESC);
                out.push(SLIP_ESC_END);
            }
            SLIP_ESC => {
                out.push(SLIP_ESC);
                out.push(SLIP_ESC_ESC);
            }
            _ => out.push(b),
        }
    }
    out.push(SLIP_END);
    out
}

// ---- DFU opcodes / results -------------------------------------------------

mod op {
    pub const CREATE: u8 = 0x01;
    pub const SET_PRN: u8 = 0x02;
    pub const CALC_CRC: u8 = 0x03;
    pub const EXECUTE: u8 = 0x04;
    pub const SELECT: u8 = 0x06;
    pub const GET_MTU: u8 = 0x07;
    pub const WRITE: u8 = 0x08;
    pub const PING: u8 = 0x09;
    pub const RESPONSE: u8 = 0x60;
}

const OBJ_COMMAND: u8 = 0x01; // init packet (.dat)
const OBJ_DATA: u8 = 0x02; // firmware (.bin)
const RES_SUCCESS: u8 = 0x01;
const RES_EXT_ERROR: u8 = 0x0B;

fn result_name(code: u8) -> &'static str {
    match code {
        0x00 => "invalid code",
        0x01 => "success",
        0x02 => "opcode not supported",
        0x03 => "invalid parameter",
        0x04 => "insufficient resources",
        0x05 => "invalid object",
        0x06 => "invalid signature",
        0x07 => "unsupported type",
        0x08 => "operation not permitted",
        0x0A => "operation failed",
        0x0B => "extended error",
        _ => "unknown result",
    }
}

// ---- CRC32 (IEEE 802.3, same polynomial as zip) ----------------------------

fn crc32(initial: u32, data: &[u8]) -> u32 {
    let mut crc = !initial;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---- DFU package (.zip) parsing --------------------------------------------

#[derive(Debug, Deserialize)]
struct ManifestFirmware {
    bin_file: String,
    dat_file: String,
}

#[derive(Debug, Deserialize)]
struct ManifestBody {
    application: Option<ManifestFirmware>,
    softdevice: Option<ManifestFirmware>,
    bootloader: Option<ManifestFirmware>,
    #[serde(rename = "softdevice_bootloader")]
    softdevice_bootloader: Option<ManifestFirmware>,
}

#[derive(Debug, Deserialize)]
struct ManifestRoot {
    manifest: ManifestBody,
}

/// One (init-packet, firmware) image pair extracted from a DFU package.
pub struct DfuImage {
    pub name: String,
    pub init_packet: Vec<u8>,
    pub firmware: Vec<u8>,
}

/// Parse a Nordic DFU `.zip`, returning the image pairs to flash in order.
/// A combined package may carry softdevice+bootloader and/or application; we flash
/// whatever is present, softdevice_bootloader → softdevice → bootloader → app.
pub fn parse_dfu_zip(path: &std::path::Path) -> Result<Vec<DfuImage>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open package: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("not a valid .zip: {e}"))?;

    // Read manifest.json.
    let manifest_text = {
        let mut f = zip
        .by_name("manifest.json")
        .map_err(|_| "package has no manifest.json — is this a Nordic DFU .zip?".to_owned())?;
        let mut s = String::new();
        f.read_to_string(&mut s)
        .map_err(|e| format!("cannot read manifest.json: {e}"))?;
        s
    };
    let root: ManifestRoot = serde_json::from_str(&manifest_text)
    .map_err(|e| format!("manifest.json is malformed: {e}"))?;

    let read_file = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Result<Vec<u8>, String> {
        let mut f = zip
        .by_name(name)
        .map_err(|_| format!("package is missing '{name}' named in the manifest"))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
        .map_err(|e| format!("cannot read '{name}': {e}"))?;
        Ok(buf)
    };

    let mut images = Vec::new();
    let order = [
        ("softdevice+bootloader", root.manifest.softdevice_bootloader),
        ("softdevice", root.manifest.softdevice),
        ("bootloader", root.manifest.bootloader),
        ("application", root.manifest.application),
    ];
    for (label, fw) in order {
        if let Some(fw) = fw {
            let init = read_file(&mut zip, &fw.dat_file)?;
            let bin = read_file(&mut zip, &fw.bin_file)?;
            images.push(DfuImage {
                name: label.to_owned(),
                        init_packet: init,
                        firmware: bin,
            });
        }
    }

    if images.is_empty() {
        return Err("manifest.json lists no firmware images".to_owned());
    }
    Ok(images)
}

// ---- building an image from a raw .hex (no nrfutil needed) -----------------

/// Parse an Intel HEX file into a flat firmware binary.
///
/// Records carry 16-bit offsets; ExtendedLinearAddress / ExtendedSegmentAddress
/// records set the upper bits. We collect absolute byte addresses, then flatten
/// from the lowest address, zero-filling any gaps (standard for DFU images).
pub fn hex_to_bin(hex_text: &str) -> Result<Vec<u8>, String> {
    use ihex::Record;

    let mut upper: u32 = 0; // from Extended (Linear|Segment) Address records
    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();

    for rec in ihex::Reader::new(hex_text) {
        let rec = rec.map_err(|e| format!("malformed Intel HEX: {e}"))?;
        match rec {
            Record::Data { offset, value } => {
                let addr = upper.wrapping_add(offset as u32);
                chunks.push((addr, value));
            }
            Record::ExtendedLinearAddress(ela) => {
                upper = (ela as u32) << 16;
            }
            Record::ExtendedSegmentAddress(esa) => {
                upper = (esa as u32) << 4;
            }
            Record::EndOfFile => break,
            // Start-address records carry no payload data.
            Record::StartLinearAddress(_) | Record::StartSegmentAddress { .. } => {}
        }
    }

    if chunks.is_empty() {
        return Err("Intel HEX contained no data records".to_owned());
    }

    let base = chunks.iter().map(|(a, _)| *a).min().unwrap();
    let end = chunks
    .iter()
    .map(|(a, d)| *a + d.len() as u32)
    .max()
    .unwrap();
    let size = (end - base) as usize;
    // Guard against absurd images (a malformed HEX with a stray high address could
    // otherwise ask us to allocate gigabytes).
    if size > 8 * 1024 * 1024 {
        return Err(format!(
            "HEX spans {size} bytes (>8 MiB) — addresses look wrong for an nRF52 image"
        ));
    }

    let mut bin = vec![0xFFu8; size]; // erased flash is 0xFF
    for (addr, data) in chunks {
        let start = (addr - base) as usize;
        bin[start..start + data.len()].copy_from_slice(&data);
    }
    Ok(bin)
}

// ---- protobuf encoding for the DFU init packet -----------------------------
//
// The init packet is a `dfu.Packet` protobuf (see Nordic's dfu-cc.proto). For an
// UNSIGNED bootloader we send `Packet{ command = Command{ op_code=INIT,
// init=InitCommand{...} } }`. The message is tiny, so we hand-encode protobuf
// wire format rather than pulling in prost + a build step.

fn pb_key(field: u32, wire: u32) -> u8 {
    ((field << 3) | wire) as u8
}

fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

fn pb_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    out.push(pb_key(field, 0)); // wire type 0 = varint
    pb_varint(out, value);
}

fn pb_len_field(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    out.push(pb_key(field, 2)); // wire type 2 = length-delimited
    pb_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// Build the DFU init packet (`.dat`) for an APPLICATION image, using a SHA-256
/// hash and SHA-256 validation — matching what `nrfutil pkg generate` produces and
/// what the stock Nordic bootloader expects (it rejects the CRC hash type).
///
/// `app_hash_le` must be the SHA-256 of the firmware **byte-reversed** (little-
/// endian), exactly as nrfutil stores it (`digest[::-1]`).
fn build_init_packet(
    app_size: u32,
    app_hash_le: &[u8; 32],
    hw_version: u32,
    fw_version: u32,
    sd_req: &[u16],
) -> Vec<u8> {
    // Hash message: { hash_type=1: SHA256(=3), hash=2: <32 bytes, little-endian> }
    let mut hash_msg = Vec::new();
    pb_varint_field(&mut hash_msg, 1, 3); // hash_type = SHA256
    pb_len_field(&mut hash_msg, 2, app_hash_le);

    // InitCommand message.
    let mut init = Vec::new();
    pb_varint_field(&mut init, 1, fw_version as u64); // fw_version
    pb_varint_field(&mut init, 2, hw_version as u64); // hw_version
    // sd_req (repeated uint32): one varint field each (bootloader accepts this).
    for &sd in sd_req {
        pb_varint_field(&mut init, 3, sd as u64);
    }
    pb_varint_field(&mut init, 4, 0); // type = APPLICATION (0)
    pb_varint_field(&mut init, 7, app_size as u64); // app_size
    pb_len_field(&mut init, 8, &hash_msg); // hash

    // Command message: { op_code=1: INIT(=1), init=2: <InitCommand> }
    let mut command = Vec::new();
    pb_varint_field(&mut command, 1, 1); // op_code = INIT
    pb_len_field(&mut command, 2, &init); // init

    // Packet message: { command=1: <Command> }
    let mut packet = Vec::new();
    pb_len_field(&mut packet, 1, &command);
    packet
}

/// SHA-256 of `data`, byte-reversed to little-endian as Nordic's tooling stores it.
fn sha256_le(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize(); // 32-byte big-endian digest
    let mut out = [0u8; 32];
    for (i, b) in digest.iter().enumerate() {
        out[31 - i] = *b;
    }
    out
}

/// Build a DFU image directly from a firmware `.hex`, generating the init packet
/// in-app. Suitable for the stock unsigned Nordic bootloader (no signing key).
///
/// `sd_req` is the SoftDevice firmware-ID requirement. 0xFFFE is nrfutil's
/// "don't care" sentinel and is the safe default when flashing an application onto
/// an existing SoftDevice; if you know your SD's firmware id you can pass it.
pub fn image_from_hex(
    hex_text: &str,
    hw_version: u32,
    fw_version: u32,
    sd_req: &[u16],
) -> Result<DfuImage, String> {
    let firmware = hex_to_bin(hex_text)?;
    let app_hash = sha256_le(&firmware);
    let init_packet = build_init_packet(
        firmware.len() as u32,
                                        &app_hash,
                                        hw_version,
                                        fw_version,
                                        sd_req,
    );
    Ok(DfuImage {
        name: "application".to_owned(),
       init_packet,
       firmware,
    })
}

/// Load firmware from either a Nordic DFU `.zip` package or a raw application
/// `.hex`, dispatching on the file extension. `.hex` builds the init packet
/// in-app (using `sd_req`); `.zip` uses the packaged init packet as-is, so `sd_req`
/// is ignored for `.zip`.
pub fn load_images(path: &std::path::Path, sd_req: &[u16]) -> Result<Vec<DfuImage>, String> {
    let ext = path
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| e.to_ascii_lowercase())
    .unwrap_or_default();
    match ext.as_str() {
        "zip" => parse_dfu_zip(path),
        "hex" => {
            let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read .hex: {e}"))?;
            // hw_version 52 matches nrfutil's nRF52 default; fw_version 1 is fine for
            // an unversioned bootloader. sd_req comes from the caller (default 0x00 =
            // no SoftDevice, which is correct for the ESB-based receiver).
            let img = image_from_hex(&text, 52, 1, sd_req)?;
            Ok(vec![img])
        }
        other => Err(format!(
            "unsupported firmware type '.{other}' — use a Nordic DFU .zip or an application .hex"
        )),
    }
}

// ---- the serial DFU session ------------------------------------------------

struct DfuSerial {
    port: Box<dyn serialport::SerialPort>,
    mtu: usize,
}

impl DfuSerial {
    /// Open the port for DFU with the given flow-control mode.
    ///
    /// On the nRF52840 dongle this is a USB CDC ACM port, not a physical UART. Two
    /// things matter for Windows (where this otherwise silently fails while Linux
    /// works):
    ///
    /// * **DTR must be asserted.** The CDC bootloader only exchanges data once the
    ///   host raises Data-Terminal-Ready ("a terminal is present"). serialport-rs
    ///   raises DTR automatically on *Linux* open, but not on Windows — so we set
    ///   `dtr_on_open(true)` and also assert it explicitly after opening.
    /// * **The port needs a moment to settle.** On Windows `open()` can return
    ///   before the CDC pipe is ready, so the first writes are dropped; we sleep
    ///   briefly and clear any stale buffered bytes before the caller pings.
    ///
    /// Retries on a busy / access-denied error: right after we disconnect the
    /// console (or the device re-enumerates), the previous handle may not be fully
    /// released yet — Windows reports "Access is denied" for a brief window.
    fn open(port_name: &str, flow: serialport::FlowControl) -> Result<Self, String> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(1000))
            .flow_control(flow)
            .dtr_on_open(true)
            .open()
            {
                Ok(mut port) => {
                    // Assert DTR explicitly too (Windows doesn't do it on open),
                    // then let the CDC pipe settle and discard any stale bytes.
                    let _ = port.write_data_terminal_ready(true);
                    std::thread::sleep(Duration::from_millis(200));
                    let _ = port.clear(serialport::ClearBuffer::All);
                    return Ok(Self { port, mtu: 0 });
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(format!("cannot open {port_name}: {e}"));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), String> {
        let framed = slip_encode(payload);
        self.port
        .write_all(&framed)
        .map_err(|e| format!("serial write failed: {e}"))?;
        self.port.flush().map_err(|e| format!("serial flush failed: {e}"))?;
        Ok(())
    }

    /// Read one SLIP frame and return the decoded bytes, with an overall deadline.
    fn recv_frame(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
        let mut decoded = Vec::new();
        let mut esc = false;
        let mut byte = [0u8; 1];
        loop {
            if Instant::now() > deadline {
                return Err("timed out waiting for DFU response".to_owned());
            }
            match self.port.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) => {
                    let c = byte[0];
                    if esc {
                        match c {
                            SLIP_ESC_END => decoded.push(SLIP_END),
                            SLIP_ESC_ESC => decoded.push(SLIP_ESC),
                            _ => {} // protocol error; drop
                        }
                        esc = false;
                    } else {
                        match c {
                            SLIP_END => {
                                if decoded.is_empty() {
                                    continue; // leading delimiter
                                }
                                return Ok(decoded);
                            }
                            SLIP_ESC => esc = true,
                            _ => decoded.push(c),
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => return Err(format!("serial read failed: {e}")),
            }
        }
    }

    /// Send a request and validate the response opcode + result, returning payload.
    fn request(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let op = payload[0];
        self.send(payload)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let resp = self.recv_frame(deadline)?;
        if resp.len() < 3 {
            return Err("DFU response too short".to_owned());
        }
        if resp[0] != op::RESPONSE {
            return Err(format!("unexpected response marker 0x{:02X}", resp[0]));
        }
        if resp[1] != op {
            return Err(format!(
                "response for wrong opcode (sent 0x{:02X}, got 0x{:02X})",
                               op, resp[1]
            ));
        }
        match resp[2] {
            RES_SUCCESS => Ok(resp[3..].to_vec()),
            RES_EXT_ERROR => {
                let ext = resp.get(3).copied().unwrap_or(0);
                Err(format!("DFU extended error 0x{ext:02X}"))
            }
            code => Err(format!("DFU error: {} (0x{code:02X})", result_name(code))),
        }
    }

    fn ping(&mut self, id: u8) -> Result<(), String> {
        // Ping reply is [0x60, PING, ping_id]. Accept an exact id match; also accept
        // any framed reply as "alive" since some bootloader builds differ slightly.
        self.send(&[op::PING, id])?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let resp = self.recv_frame(deadline)?;
        if resp.is_empty() {
            return Err("no ping reply".to_owned());
        }
        let id_ok = resp.len() >= 3
        && resp[0] == op::RESPONSE
        && resp[1] == op::PING
        && resp[2] == id;
        if id_ok || !resp.is_empty() {
            Ok(())
        } else {
            Err("unexpected ping reply".to_owned())
        }
    }

    fn set_prn(&mut self, prn: u16) -> Result<(), String> {
        let mut msg = vec![op::SET_PRN];
        msg.extend_from_slice(&prn.to_le_bytes());
        self.request(&msg)?;
        Ok(())
    }

    fn get_mtu(&mut self) -> Result<(), String> {
        let resp = self.request(&[op::GET_MTU])?;
        if resp.len() < 2 {
            return Err("bad MTU response".to_owned());
        }
        self.mtu = u16::from_le_bytes([resp[0], resp[1]]) as usize;
        if self.mtu < 16 {
            return Err(format!("implausible DFU MTU {}", self.mtu));
        }
        Ok(())
    }

    /// Select an object type → (max_size, offset, crc).
    fn select(&mut self, obj_type: u8) -> Result<(u32, u32, u32), String> {
        let resp = self.request(&[op::SELECT, obj_type])?;
        if resp.len() < 12 {
            return Err("bad Select response".to_owned());
        }
        let max_size = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
        let offset = u32::from_le_bytes([resp[4], resp[5], resp[6], resp[7]]);
        let crc = u32::from_le_bytes([resp[8], resp[9], resp[10], resp[11]]);
        Ok((max_size, offset, crc))
    }

    fn create(&mut self, obj_type: u8, size: u32) -> Result<(), String> {
        let mut msg = vec![op::CREATE, obj_type];
        msg.extend_from_slice(&size.to_le_bytes());
        self.request(&msg)?;
        Ok(())
    }

    fn execute(&mut self) -> Result<(), String> {
        self.request(&[op::EXECUTE])?;
        Ok(())
    }

    /// CRC-get → (offset, crc).
    fn get_crc(&mut self) -> Result<(u32, u32), String> {
        let resp = self.request(&[op::CALC_CRC])?;
        if resp.len() < 8 {
            return Err("bad CRC response".to_owned());
        }
        let offset = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
        let crc = u32::from_le_bytes([resp[4], resp[5], resp[6], resp[7]]);
        Ok((offset, crc))
    }

    /// Stream one object's bytes as Write packets, chunked so the SLIP-encoded
    /// frame can't exceed the MTU (worst case every byte escapes → ×2), then verify
    /// the running CRC. Returns the updated running CRC.
    fn stream(&mut self, data: &[u8], mut running_crc: u32, base_offset: u32) -> Result<u32, String> {
        // Matches nrfutil: max data per Write = (mtu-1)/2 - 1.
        let chunk = ((self.mtu - 1) / 2).saturating_sub(1).max(1);
        let mut sent: u32 = 0;
        for piece in data.chunks(chunk) {
            let mut msg = Vec::with_capacity(piece.len() + 1);
            msg.push(op::WRITE);
            msg.extend_from_slice(piece);
            // Write requests get no per-packet response when PRN = 0.
            self.send(&msg)?;
            running_crc = crc32(running_crc, piece);
            sent += piece.len() as u32;
        }
        // Verify what the target received.
        let (offset, crc) = self.get_crc()?;
        let expected_offset = base_offset + sent;
        if offset != expected_offset {
            return Err(format!(
                "offset mismatch after write (expected {expected_offset}, target {offset})"
            ));
        }
        if crc != running_crc {
            return Err("CRC mismatch after write — transfer corrupted".to_owned());
        }
        Ok(running_crc)
    }

    /// Transfer one object type (init packet or firmware) in max_size-sized objects.
    fn transfer_object(
        &mut self,
        obj_type: u8,
        data: &[u8],
        progress: &mut dyn FnMut(usize),
    ) -> Result<(), String> {
        let (max_size, _offset, _crc) = self.select(obj_type)?;
        let max_size = max_size.max(1) as usize;

        // For simplicity and safety we always start the object fresh (no resume).
        let mut running_crc = 0u32;
        let mut pos = 0usize;
        while pos < data.len() {
            let end = (pos + max_size).min(data.len());
            let object = &data[pos..end];
            self.create(obj_type, object.len() as u32)?;
            running_crc = self.stream(object, running_crc, pos as u32)?;
            self.execute()?;
            progress(object.len());
            pos = end;
        }
        // Zero-length object (possible only for empty data) — nothing to do.
        Ok(())
    }
}

// ---- progress events -------------------------------------------------------

pub enum NrfProgress {
    Status(String),
    PortTriggered { port: String, ok: bool },
    DeviceReady { port: String },
    /// total firmware bytes to send for the current device (for a progress bar).
    Total { bytes: usize },
    /// incremental firmware bytes flashed.
    Advance { bytes: usize },
    Flashed { port: String },
    Warn(String),
    Finished { flashed: usize, expected: usize },
}

/// Flash one device that is already in (or being put into) DFU mode.
fn flash_one(
    port: &str,
    images: &[DfuImage],
    tx: &Sender<NrfProgress>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let send = |p: NrfProgress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };

    // Open + handshake. The real Windows fix is asserting DTR (done in
    // DfuSerial::open). Flow control is secondary: the dongle's USB-CDC bootloader
    // has no real RTS/CTS lines, and nrfutil talks to it with flow control *off*,
    // so try `None` first (the known-good combination) and fall back to `Hardware`
    // just in case a particular driver wants it. Either way DTR is asserted.
    let ping = |dfu: &mut DfuSerial| -> bool {
        let ping_deadline = Instant::now() + Duration::from_secs(3);
        let mut ping_id = 1u8;
        while Instant::now() < ping_deadline {
            if dfu.ping(ping_id).is_ok() {
                return true;
            }
            ping_id = ping_id.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    };

    let mut dfu = DfuSerial::open(port, serialport::FlowControl::None)?;
    if !ping(&mut dfu) {
        // Fall back to hardware flow control.
        drop(dfu);
        std::thread::sleep(Duration::from_millis(300));
        let mut dfu2 = DfuSerial::open(port, serialport::FlowControl::Hardware)?;
        if !ping(&mut dfu2) {
            return Err(
                "device did not answer DFU ping (is it in bootloader mode? \
try re-entering DFU with the magnet)"
.to_owned(),
            );
        }
        dfu = dfu2;
    }

    dfu.set_prn(0)?;
    dfu.get_mtu()?;

    let total: usize = images.iter().map(|i| i.firmware.len()).sum();
    send(NrfProgress::Total { bytes: total });

    for img in images {
        send(NrfProgress::Status(format!("Sending {} init packet…", img.name)));
        // Init packet: no progress accounting (tiny).
        dfu.transfer_object(OBJ_COMMAND, &img.init_packet, &mut |_| {})?;

        send(NrfProgress::Status(format!("Flashing {} ({} bytes)…", img.name, img.firmware.len())));
        let mut adv = |n: usize| {
            let _ = tx.send(NrfProgress::Advance { bytes: n });
            ctx.request_repaint();
        };
        dfu.transfer_object(OBJ_DATA, &img.firmware, &mut adv)?;
    }

    Ok(())
}

/// Run the receiver DFU flash on a worker thread.
///
/// * `ports` — receiver serial ports to flash. Each is sent `dfu` first (best
///   effort) to enter the bootloader, then flashed once it answers a ping.
/// * `package` — the Nordic DFU `.zip`.
/// * `already_in_dfu` — ports the caller knows are *already* in the bootloader, so
///   they are flashed directly without sending `dfu`.
#[allow(clippy::too_many_arguments)]
pub fn run_receiver_dfu(
    ports: Vec<String>,
    already_in_dfu: HashSet<String>,
    package: PathBuf,
    sd_req: Vec<u16>,
    line_ending: &'static str,
    tx: Sender<NrfProgress>,
    ctx: egui::Context,
) {
    let send = |p: NrfProgress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };

    // Parse the package once up front so a bad file fails fast, before rebooting.
    // Accepts a Nordic DFU .zip or a raw application .hex (init packet built in-app).
    let images = match load_images(&package, &sd_req) {
        Ok(i) => i,
        Err(e) => {
            send(NrfProgress::Warn(format!("Firmware error: {e}")));
            send(NrfProgress::Finished { flashed: 0, expected: 0 });
            return;
        }
    };
    let summary: Vec<String> = images
    .iter()
    .map(|i| format!("{} ({} bytes)", i.name, i.firmware.len()))
    .collect();
    send(NrfProgress::Status(format!("Package OK: {}", summary.join(", "))));

    let expected = ports.len();
    let mut flashed = 0usize;

    for port in ports {
        // 1. Put the device into the bootloader unless it's already there.
        let mut target_port = port.clone();
        if !already_in_dfu.contains(&port) {
            send(NrfProgress::Status(format!("Sending dfu to {port}…")));
            let ok = trigger_dfu(&port, line_ending);
            send(NrfProgress::PortTriggered { port: port.clone(), ok });

            // The bootloader re-enumerates, often under the SAME port name on
            // Linux, but may change (esp. on Windows). Wait for a usable port.
            match wait_for_bootloader_port(&port, Duration::from_secs(20)) {
                Some(p) => target_port = p,
                None => {
                    send(NrfProgress::Warn(format!(
                        "{port}: bootloader serial port did not appear within 20 s"
                    )));
                    continue;
                }
            }
        }

        send(NrfProgress::DeviceReady { port: target_port.clone() });
        match flash_one(&target_port, &images, &tx, &ctx) {
            Ok(()) => {
                flashed += 1;
                send(NrfProgress::Flashed { port: target_port });
            }
            Err(e) => send(NrfProgress::Warn(format!("{target_port}: {e}"))),
        }
    }

    send(NrfProgress::Finished { flashed, expected });
}

/// Send `dfu` to a receiver console port to trigger the bootloader. Best effort.
fn trigger_dfu(port: &str, line_ending: &str) -> bool {
    match serialport::new(port, 115_200)
    .timeout(Duration::from_millis(300))
    .open()
    {
        Ok(mut p) => {
            let _ = p.write_data_terminal_ready(true);
            let _ = p.write_request_to_send(true);
            std::thread::sleep(Duration::from_millis(50));
            let payload = format!("dfu{line_ending}");
            if p.write_all(payload.as_bytes()).is_ok() {
                let _ = p.flush();
                std::thread::sleep(Duration::from_millis(200));
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

/// Wait for the device to re-enumerate in its DFU bootloader after a `dfu` command.
///
/// A real reboot makes the running-firmware CDC port disappear and a Nordic Open
/// DFU port (VID 0x1915) appear — often, but not always, with the same OS name. We
/// therefore prioritise a port that actually reports the Nordic DFU bootloader, and
/// only as a last resort accept the original name if it comes back. This avoids the
/// false positive where the firmware never rebooted and we'd otherwise try to speak
/// DFU to the still-running application.
fn wait_for_bootloader_port(original: &str, timeout: Duration) -> Option<String> {
    use crate::serial::{list_ports, BootloaderKind};
    let deadline = Instant::now() + timeout;
    // Let the reboot + USB re-enumeration begin.
    std::thread::sleep(Duration::from_millis(1000));

    let mut saw_disappear = false;
    while Instant::now() < deadline {
        let ports = list_ports();

        // Best signal: a port explicitly in the Nordic DFU bootloader.
        if let Some(p) = ports
            .iter()
            .find(|p| p.bootloader == Some(BootloaderKind::NordicDfu))
            {
                return Some(p.name.clone());
            }

            // Track whether the original port went away (a sign the reboot happened).
            let original_present = ports.iter().any(|p| p.name == original);
        if !original_present {
            saw_disappear = true;
        }
        // If the original name came back *after* disappearing, the bootloader may
        // be using the same name without a distinct VID — accept it then.
        if saw_disappear && original_present {
            return Some(original.to_owned());
        }

        std::thread::sleep(Duration::from_millis(300));
    }

    // Last resort: if it's still the original port and never visibly rebooted, the
    // caller will try it and surface a clear DFU error rather than hanging.
    if list_ports().iter().any(|p| p.name == original) {
        Some(original.to_owned())
    } else {
        None
    }
}
