//! Diagnostic trouble codes (SAE J2012 encoding): Mode 03/07/0A decode, Mode 04 clear.

use serde::Serialize;

/// Which system a code belongs to (top 2 bits of the first byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DtcKind {
    Powertrain, // P
    Chassis,    // C
    Body,       // B
    Network,    // U
}

impl DtcKind {
    pub fn letter(self) -> char {
        match self {
            Self::Powertrain => 'P',
            Self::Chassis => 'C',
            Self::Body => 'B',
            Self::Network => 'U',
        }
    }

    pub fn from_bits(bits: u8) -> Self {
        match bits >> 6 {
            0 => Self::Powertrain,
            1 => Self::Chassis,
            2 => Self::Body,
            _ => Self::Network,
        }
    }
}

/// A decoded trouble code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dtc {
    pub kind: DtcKind,
    pub code: String, // e.g. "P0301"
    pub source: DtcSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DtcSource {
    Stored,      // Mode 03
    Pending,     // Mode 07
    Permanent,   // Mode 0A
}

impl Dtc {
    /// Decode one 2-byte DTC (e.g. [0x03, 0x01] -> "P0301").
    pub fn decode(bytes: [u8; 2], source: DtcSource) -> Option<Self> {
        if bytes == [0x00, 0x00] {
            return None; // 0000 means "no code" in response padding
        }
        let kind = DtcKind::from_bits(bytes[0]);
        let d1 = ((bytes[0] >> 4) & 0x3).as_char_digit();
        let d2 = (bytes[0] & 0x0F).as_char_digit();
        let d3 = ((bytes[1] >> 4) & 0x0F).as_char_digit();
        let d4 = (bytes[1] & 0x0F).as_char_digit();
        Some(Self {
            kind,
            code: format!("{}{}{}{}{}", kind.letter(), d1, d2, d3, d4),
            source,
        })
    }

    /// Encode a code string like "P0301" back into its two bytes (for tests/tools).
    pub fn encode(code: &str) -> Option<[u8; 2]> {
        let c = code.trim().to_ascii_uppercase();
        let b = c.as_bytes();
        if b.len() != 5 {
            return None;
        }
        let kind_bits = match b[0] {
            b'P' => 0u8,
            b'C' => 1,
            b'B' => 2,
            b'U' => 3,
            _ => return None,
        };
        let d1 = (b[1] as char).to_digit(16)? as u8;
        let d2 = (b[2] as char).to_digit(16)? as u8;
        let d3 = (b[3] as char).to_digit(16)? as u8;
        let d4 = (b[4] as char).to_digit(16)? as u8;
        Some([(kind_bits << 6) | (d1 << 4) | d2, (d3 << 4) | d4])
    }

    /// Decode the payload of a Mode 03/07/0A response (sequence of 2-byte codes).
    pub fn decode_stream(data: &[u8], source: DtcSource) -> Vec<Self> {
        data.chunks_exact(2)
            .filter_map(|c| Self::decode([c[0], c[1]], source))
            .collect()
    }
}

/// Helper: nibble → hex char.
trait CharDigit {
    fn as_char_digit(self) -> char;
}
impl CharDigit for u8 {
    fn as_char_digit(self) -> char {
        char::from_digit(self as u32, 16).unwrap_or('0').to_ascii_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_classic_codes() {
        assert_eq!(Dtc::decode([0x03, 0x01], DtcSource::Stored).unwrap().code, "P0301");
        assert_eq!(Dtc::decode([0x24, 0x45], DtcSource::Stored).unwrap().code, "P2445");
        // C1234: C = kind 01, digits 1,2,3,4 -> byte0 = 01_01_0010 = 0x52, byte1 = 0x34
        let d = Dtc::decode([0x52, 0x34], DtcSource::Pending).unwrap();
        assert_eq!(d.code, "C1234");
        assert_eq!(d.kind, DtcKind::Chassis);
    }

    #[test]
    fn network_and_body_kinds() {
        // U1234: U = kind 11 -> byte0 = 11_01_0010 = 0xD2
        let u = Dtc::decode([0xD2, 0x34], DtcSource::Stored).unwrap();
        assert_eq!(u.code, "U1234");
        assert_eq!(u.kind, DtcKind::Network);
        // B1111: B = kind 10 -> byte0 = 10_01_0001 = 0x91
        let b = Dtc::decode([0x91, 0x11], DtcSource::Stored).unwrap();
        assert_eq!(b.code, "B1111");
        assert_eq!(b.kind, DtcKind::Body);
    }

    #[test]
    fn encode_roundtrips() {
        for code in ["P0301", "P2445", "C1234", "U0121", "B0001"] {
            let bytes = Dtc::encode(code).unwrap();
            let back = Dtc::decode(bytes, DtcSource::Stored).unwrap();
            assert_eq!(back.code, code);
        }
    }

    #[test]
    fn stream_skips_padding_zeros() {
        let codes = Dtc::decode_stream(&[0x03, 0x01, 0x00, 0x00, 0x24, 0x45], DtcSource::Stored);
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0].code, "P0301");
        assert_eq!(codes[1].code, "P2445");
    }

    #[test]
    fn all_kinds_encode() {
        assert_eq!(Dtc::encode("X9999"), None);
        assert_eq!(Dtc::encode("P030"), None);
        assert_eq!(Dtc::encode("P0300").unwrap(), [0x03, 0x00]);
    }
}
