//! ELM327 protocol engine: init sequence, response normalization, error
//! handling, ISO-TP assembly — against any [`Transport`].
//!
//! Reference behavior (ELM327 AT command set, SAE J1979 / ISO 15765-4):
//! - commands are ASCII lines terminated by CR; replies end with `>`
//! - `ATE0` disables echo, `ATS0` disables spaces, `ATH1` enables headers
//! - error frames: `?`, `NO DATA`, `UNABLE TO CONNECT`, `CAN ERROR`, `BUFFER
//!   FULL`, `STOPPED`, `SEARCHING...`, `BUS INIT: ...OK`, `LV RESET`, `DATA ERROR`
//! - with headers on, each frame is `7E810064100BE3EA813` style: 3-hex-digit
//!   CAN id + ISO-TP PCI nibble + payload

use super::pid::{Pid, PidValue};
use super::transport::Transport;
use super::dtc::{Dtc, DtcSource};
use anyhow::{bail, Result};

/// Typed protocol errors — never silently swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObdError {
    NoData,
    UnableToConnect,
    CanError,
    BufferFull,
    Stopped,
    Unknown(String),
    Malformed(String),
}

impl std::fmt::Display for ObdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoData => write!(f, "NO DATA (ECU did not answer this request)"),
            Self::UnableToConnect => write!(f, "UNABLE TO CONNECT (no vehicle on the bus)"),
            Self::CanError => write!(f, "CAN ERROR"),
            Self::BufferFull => write!(f, "BUFFER FULL"),
            Self::Stopped => write!(f, "STOPPED"),
            Self::Unknown(s) => write!(f, "ELM327 error: {}", s),
            Self::Malformed(s) => write!(f, "malformed response: {}", s),
        }
    }
}

impl std::error::Error for ObdError {}

/// A parsed OBD response: service echo + data bytes (+ source address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObdResponse {
    pub source: Option<u32>,
    pub service: u8,
    pub data: Vec<u8>,
}

/// ELM327 client over any transport.
pub struct ElmClient<T: Transport> {
    transport: T,
    pub echo_on: bool,
    pub headers_on: bool,
}

impl<T: Transport> ElmClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport, echo_on: true, headers_on: false }
    }

    /// Full ELM327 init: reset, echo off, linefeeds off, spaces off, auto protocol.
    pub fn init(&mut self) -> Result<String> {
        let id = self.at("ATZ")?;
        self.at("ATE0")?;
        self.echo_on = false;
        self.at("ATL0")?;
        self.at("ATS0")?;
        self.at("ATSP0")?;
        Ok(id)
    }

    /// Send one AT command and normalize the reply.
    pub fn at(&mut self, cmd: &str) -> Result<String> {
        let raw = self.transport.transact(cmd)?;
        let reply = normalize(&raw);
        if let Some(err) = detect_error(&reply) {
            bail!(err);
        }
        Ok(reply)
    }

    /// Send a service request (e.g. "010C") and parse the answer into frames.
    pub fn request(&mut self, req: &str) -> Result<ObdResponse> {
        let raw = self.transport.transact(req)?;
        let reply = normalize(&raw);
        if let Some(err) = detect_error(&reply) {
            bail!(err);
        }
        parse_obd_reply(&reply)
    }

    /// Query one Mode 01 PID and decode its value.
    pub fn read_pid(&mut self, pid: &Pid) -> Result<PidValue> {
        let resp = self.request(&pid.request())?;
        // Mode 01 positive response: 0x41, PID, data…
        if resp.service != 0x41 {
            bail!(ObdError::Malformed(format!(
                "expected service 0x41, got 0x{:02X}",
                resp.service
            )));
        }
        if resp.data.first() != Some(&pid.id) {
            bail!(ObdError::Malformed(format!(
                "expected PID 0x{:02X} in response, got {:?}",
                pid.id,
                resp.data.first()
            )));
        }
        pid.decode(&resp.data[1..]).ok_or_else(|| {
            ObdError::Malformed(format!("not enough data bytes for PID {}", pid.name)).into()
        })
    }

    /// Read supported-PID bitmask (0x00/0x20/0x40/0x60).
    pub fn supported_pids(&mut self, range_base: u8) -> Result<Vec<u8>> {
        let resp = self.request(&format!("01{:02X}", range_base))?;
        if resp.service != 0x41 || resp.data.first() != Some(&range_base) {
            bail!(ObdError::Malformed("bad supported-PID response".into()));
        }
        Ok(super::pid::supported_from_bitmask(range_base, &resp.data[1..]))
    }

    /// Read stored (Mode 03) / pending (07) / permanent (0A) trouble codes.
    pub fn read_dtcs(&mut self, source: DtcSource) -> Result<Vec<Dtc>> {
        let mode = match source {
            DtcSource::Stored => "03",
            DtcSource::Pending => "07",
            DtcSource::Permanent => "0A",
        };
        let resp = self.request(mode)?;
        let expected = match source {
            DtcSource::Stored => 0x43,
            DtcSource::Pending => 0x47,
            DtcSource::Permanent => 0x4A,
        };
        if resp.service != expected {
            bail!(ObdError::Malformed(format!("expected service 0x{:02X}", expected)));
        }
        // Two response layouts exist in the wild:
        // - CAN (ISO 15765-4): 43 + code pairs (no count byte)
        // - J1979 legacy: count byte + code pairs
        // Strip the count byte only when the length proves it's there.
        let data = if !resp.data.is_empty()
            && (1..=8).contains(&resp.data[0])
            && resp.data.len() == resp.data[0] as usize * 2 + 1
        {
            &resp.data[1..]
        } else {
            &resp.data[..]
        };
        Ok(Dtc::decode_stream(data, source))
    }

    /// Clear codes + reset MIL (Mode 04).
    pub fn clear_dtcs(&mut self) -> Result<()> {
        let resp = self.request("04")?;
        if resp.service != 0x44 {
            bail!(ObdError::Malformed("expected service 0x44".into()));
        }
        Ok(())
    }

    /// Read VIN (Mode 09 PID 0x02) with ISO-TP multi-frame assembly.
    pub fn read_vin(&mut self) -> Result<String> {
        let raw = self.transport.transact("0902")?;
        let reply = normalize(&raw);
        if let Some(err) = detect_error(&reply) {
            bail!(err);
        }
        let frames = parse_frames(&reply)?;
        let payload = assemble_isotp(&frames)?;
        // Response: 49 02 01 <17 VIN bytes>
        if payload.len() >= 20 && payload[0] == 0x49 && payload[1] == 0x02 {
            let vin: String = payload[3..20]
                .iter()
                .map(|&b| b as char)
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            return Ok(vin);
        }
        bail!(ObdError::Malformed("VIN response too short".into()))
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

/// Strip echo, whitespace, prompts and `SEARCHING...` banners.
pub fn normalize(raw: &str) -> String {
    // ELM327 replies use CR (sometimes CRLF) line endings — normalize first.
    let mut s = raw.replace("\r\n", "\n").replace('\r', "\n").replace('>', " ");
    if let Some(pos) = s.find("SEARCHING") {
        // drop the banner up to and including the newline that follows
        let rest = &s[pos..];
        if let Some(nl) = rest.find('\n') {
            s = format!("{}{}", &s[..pos], &rest[nl + 1..]);
        } else {
            s.truncate(pos);
        }
    }
    s.trim()
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect ELM327 error frames in a normalized reply.
pub fn detect_error(reply: &str) -> Option<ObdError> {
    let up = reply.to_ascii_uppercase();
    if up.trim() == "?" {
        return Some(ObdError::Unknown("? (unknown command)".into()));
    }
    for (needle, err) in [
        ("UNABLE TO CONNECT", ObdError::UnableToConnect),
        ("NO DATA", ObdError::NoData),
        ("CAN ERROR", ObdError::CanError),
        ("BUFFER FULL", ObdError::BufferFull),
        ("STOPPED", ObdError::Stopped),
    ] {
        if up.contains(needle) {
            return Some(err);
        }
    }
    if up.contains("DATA ERROR") || up.contains("FB ERROR") || up.contains("LV RESET") {
        return Some(ObdError::Unknown(up.trim().to_string()));
    }
    None
}

/// One raw link-layer frame: CAN id (if present) + payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: Option<u32>,
    pub data: Vec<u8>,
}

/// Parse a normalized ELM327 reply into raw frames (handles headers on/off).
pub fn parse_frames(reply: &str) -> Result<Vec<Frame>> {
    let mut frames = Vec::new();
    for line in reply.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let compact: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if compact.is_empty() {
            continue;
        }
        if looks_like_headered(line, &compact) {
            let (id, rest) = split_can_id(line)?;
            frames.push(Frame { id: Some(id), data: hex_to_bytes(&rest)? });
        } else {
            frames.push(Frame { id: None, data: hex_to_bytes(&compact)? });
        }
    }
    if frames.is_empty() {
        bail!(ObdError::Malformed("no frames in response".into()));
    }
    Ok(frames)
}

/// Headered if:
/// - 29-bit id ("18DA…"/"18DB…" prefix), or
/// - odd hex length (3-nibble 11-bit id glued to even payload), or
/// - first space-separated token is exactly 3 hex chars with more tokens after.
fn looks_like_headered(line: &str, compact: &str) -> bool {
    if compact.starts_with("18DA") || compact.starts_with("18DB") {
        return true;
    }
    if compact.len() % 2 == 1 {
        return true;
    }
    let mut toks = line.split_whitespace();
    if let (Some(first), Some(_)) = (toks.next(), toks.next()) {
        return first.len() == 3 && first.chars().all(|c| c.is_ascii_hexdigit());
    }
    false
}

/// Split "7E8064100BE3EA813" or "7E8 06 41 00 BE 3E A8 13" or "18DAF1100641..." into (id, hex-payload).
fn split_can_id(line: &str) -> Result<(u32, String)> {
    let compact: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let t = line.trim_start();
    let id = if t.starts_with("18DA") || t.starts_with("18DB") {
        u32::from_str_radix(&compact[..8], 16).map_err(|_| anyhow::anyhow!("bad 29-bit CAN id"))?
    } else {
        u32::from_str_radix(&compact[..3], 16).map_err(|_| anyhow::anyhow!("bad 11-bit CAN id"))?
    };
    let rest = if t.starts_with("18DA") || t.starts_with("18DB") {
        compact[8..].to_string()
    } else {
        compact[3..].to_string()
    };
    Ok((id, rest))
}

/// Assemble ISO-TP frames (PCI nibble: 0=single, 1=first, 2=consecutive) into
/// one payload.
pub fn assemble_isotp(frames: &[Frame]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for f in frames {
        if f.data.is_empty() {
            continue;
        }
        let pci = f.data[0] >> 4;
        match pci {
            0 => out.extend_from_slice(&f.data[1..]),
            1 => {
                // First frame: 12-bit length, then 6 data bytes.
                out.extend_from_slice(&f.data[2..]);
            }
            2 => out.extend_from_slice(&f.data[1..]),
            _ => continue, // flow control / unknown — ignore
        }
    }
    if out.is_empty() {
        bail!(ObdError::Malformed("empty ISO-TP payload".into()));
    }
    Ok(out)
}

/// Parse a normalized headerless/headered service reply into service + data.
pub fn parse_obd_reply(reply: &str) -> Result<ObdResponse> {
    let frames = parse_frames(reply)?;
    // For single-frame replies the payload starts with PCI byte then service.
    let mut payload = Vec::new();
    let mut source = None;
    for f in &frames {
        source = f.id;
        if f.data.is_empty() {
            continue;
        }
        // Single-frame PCI (low nibble = length) then service…
        let pci = f.data[0] >> 4;
        if pci == 0 {
            payload.extend_from_slice(&f.data[1..]);
        } else {
            payload.extend_from_slice(&f.data);
        }
    }
    if payload.is_empty() {
        bail!(ObdError::Malformed("empty service reply".into()));
    }
    // In headerless mode there is no PCI byte at all — service is first.
    // Detect: if first byte looks like a service echo (0x41-0x4A) treat as service.
    let (service, data) = if is_service(payload[0]) {
        (payload[0], payload[1..].to_vec())
    } else if payload.len() > 1 && is_service(payload[1]) {
        (payload[1], payload[2..].to_vec())
    } else {
        bail!(ObdError::Malformed(format!("no service echo in {:?}", payload)));
    };
    Ok(ObdResponse { source, service, data })
}

fn is_service(b: u8) -> bool {
    (0x41..=0x4F).contains(&b)
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        bail!(ObdError::Malformed("odd-length hex".into()));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| anyhow::anyhow!("bad hex")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obd::transport::MockTransport;

    fn client_with(map: Vec<(String, String)>) -> ElmClient<MockTransport> {
        ElmClient::new(MockTransport::scripted(map))
    }

    #[test]
    fn init_sends_the_real_elm_sequence() {
        let mut c = client_with(vec![
            ("ATZ".into(), "ELM327 v1.5".into()),
            ("ATE0".into(), "OK".into()),
            ("ATL0".into(), "OK".into()),
            ("ATS0".into(), "OK".into()),
            ("ATSP0".into(), "OK".into()),
        ]);
        let id = c.init().unwrap();
        assert_eq!(id, "ELM327 v1.5");
        assert_eq!(
            c.into_transport().sent,
            vec!["ATZ", "ATE0", "ATL0", "ATS0", "ATSP0"]
        );
    }

    #[test]
    fn headerless_pid_response_parses() {
        let mut c = client_with(vec![
            ("ATZ".into(), "ELM327 v1.5".into()),
            ("ATE0".into(), "OK".into()),
            ("ATL0".into(), "OK".into()),
            ("ATS0".into(), "OK".into()),
            ("ATSP0".into(), "OK".into()),
            ("010C".into(), "410C1AF8".into()),
        ]);
        c.init().unwrap();
        let v = c.read_pid(crate::obd::pid::lookup("rpm").unwrap()).unwrap();
        assert!((v.value - 1726.0).abs() < 1e-9);
    }

    #[test]
    fn headered_response_with_spaces_and_echo() {
        // ELM with echo+headers+spaces: realistic messy frame for speed 80 km/h
        let reply = normalize("010D\r\r7E8 03 41 0D 50 \r\r>");
        assert_eq!(reply, "010D\n7E8 03 41 0D 50");
        let frames = parse_frames(&reply).unwrap();
        assert_eq!(frames.len(), 2);
        let resp = parse_obd_reply("7E8 03 41 0D 50").unwrap();
        assert_eq!(resp.service, 0x41);
        assert_eq!(resp.data, vec![0x0D, 0x50]);
    }

    #[test]
    fn compact_headered_frame_parses() {
        let resp = parse_obd_reply("7E803410D50").unwrap();
        assert_eq!(resp.service, 0x41);
        assert_eq!(resp.data, vec![0x0D, 0x50]);
        assert_eq!(resp.source, Some(0x7E8));
    }

    #[test]
    fn error_frames_map_to_typed_errors() {
        assert_eq!(detect_error("NO DATA"), Some(ObdError::NoData));
        assert_eq!(detect_error("UNABLE TO CONNECT"), Some(ObdError::UnableToConnect));
        assert_eq!(detect_error("?"), Some(ObdError::Unknown("? (unknown command)".into())));
        assert_eq!(detect_error("410C1AF8"), None);
    }

    #[test]
    fn searching_banner_is_stripped() {
        let n = normalize("SEARCHING...\r7E803410C1AF8\r>");
        assert!(!n.contains("SEARCHING"));
        let resp = parse_obd_reply(&n).unwrap();
        assert_eq!(resp.data, vec![0x0C, 0x1A, 0xF8]);
    }

    #[test]
    fn vin_multiframe_assembles() {
        // First frame (len 0x14=20): 49 02 01 + 4 VIN chars; consecutive frames carry the rest.
        let reply = "7E8101449020131 44\n7E82134475030303030\n7E82231323334000000";
        let frames = parse_frames(reply).unwrap();
        let payload = assemble_isotp(&frames).unwrap();
        assert_eq!(payload[0], 0x49);
        assert_eq!(payload[1], 0x02);
    }

    #[test]
    fn dtc_read_flow() {
        let mut c = client_with(vec![
            ("ATZ".into(), "ELM327 v1.5".into()),
            ("ATE0".into(), "OK".into()),
            ("ATL0".into(), "OK".into()),
            ("ATS0".into(), "OK".into()),
            ("ATSP0".into(), "OK".into()),
            ("03".into(), "4303012445".into()),
        ]);
        c.init().unwrap();
        let codes = c.read_dtcs(DtcSource::Stored).unwrap();
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0].code, "P0301");
        assert_eq!(codes[1].code, "P2445");
    }
}
