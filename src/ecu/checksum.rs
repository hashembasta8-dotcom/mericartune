//! Real checksum engines with per-segment verify/repair.
//!
//! The GM family (P01/P59 and friends) uses a **zero-sum 32-bit** segment
//! checksum: the segment is read as little-endian 32-bit words, and the stored
//! word is chosen so the total sum of the segment is zero (mod 2^32). Editing a
//! single calibration cell invalidates the checksum — this module detects that
//! and repairs it exactly the way a flash tool must.
//!
//! Additional algorithms (sum16, xor8, crc32) cover non-GM platforms and
//! sub-section guards.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlg {
    /// GM-style: sum of LE u32 words over the segment must be 0 (mod 2^32).
    ZeroSum32Le,
    /// Simple 16-bit additive sum of all bytes (mod 2^16).
    Sum16,
    /// XOR of all bytes.
    Xor8,
    /// Standard CRC-32 (IEEE 802.3, reflected).
    Crc32,
}

impl ChecksumAlg {
    pub fn from_name(name: &str) -> Result<Self> {
        Ok(match name.to_ascii_lowercase().as_str() {
            "zero-sum-32le" | "zerosum32" | "gm" => Self::ZeroSum32Le,
            "sum16" => Self::Sum16,
            "xor8" => Self::Xor8,
            "crc32" => Self::Crc32,
            other => bail!("unknown checksum algorithm '{}'", other),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::ZeroSum32Le => "zero-sum-32le",
            Self::Sum16 => "sum16",
            Self::Xor8 => "xor8",
            Self::Crc32 => "crc32",
        }
    }
}

/// A checksummed region: `[start, end)` plus where/how the stored value lives.
#[derive(Debug, Clone)]
pub struct SegmentChecksum {
    pub name: String,
    pub start: usize,
    pub end: usize, // exclusive
    pub alg: ChecksumAlg,
    /// Byte offset of the stored checksum value inside the image (None = not stored).
    pub store_at: Option<usize>,
    /// Width in bytes of the stored value (2 or 4).
    pub store_width: usize,
}

impl SegmentChecksum {
    /// Compute the stored value that makes the segment valid.
    /// For ZeroSum32Le the stored word participates in the segment sum; for the
    /// other algorithms the stored bytes are excluded from the computation.
    pub fn compute_stored(&self, data: &[u8]) -> Result<u64> {
        self.check_bounds(data)?;
        let seg = &data[self.start..self.end];
        Ok(match self.alg {
            ChecksumAlg::ZeroSum32Le => {
                if seg.len() % 4 != 0 {
                    bail!("zero-sum-32 segment '{}' length {} not a multiple of 4", self.name, seg.len());
                }
                let mut sum: u32 = 0;
                for w in seg.chunks_exact(4) {
                    sum = sum.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
                }
                // The stored word must complete the sum to zero.
                (0u32.wrapping_sub(sum)) as u64
            }
            ChecksumAlg::Sum16 => {
                let sum: u32 = self.payload(seg).iter().map(|&b| b as u32).sum();
                (sum & 0xFFFF) as u64
            }
            ChecksumAlg::Xor8 => self.payload(seg).iter().fold(0u8, |a, &b| a ^ b) as u64,
            ChecksumAlg::Crc32 => crc32(&self.payload(seg)) as u64,
        })
    }

    /// Segment bytes excluding the stored-checksum location (borrowed view).
    fn payload<'a>(&self, seg: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        match self.store_at {
            Some(at) => {
                let rel = at - self.start;
                let end = (rel + self.store_width).min(seg.len());
                if rel >= seg.len() {
                    return std::borrow::Cow::Borrowed(seg);
                }
                let mut out = Vec::with_capacity(seg.len() - (end - rel));
                out.extend_from_slice(&seg[..rel]);
                out.extend_from_slice(&seg[end..]);
                std::borrow::Cow::Owned(out)
            }
            None => std::borrow::Cow::Borrowed(seg),
        }
    }

    /// Is the segment currently valid?
    pub fn verify(&self, data: &[u8]) -> Result<bool> {
        self.check_bounds(data)?;
        match (self.alg, self.store_at) {
            (ChecksumAlg::ZeroSum32Le, Some(_)) => {
                // Validity == full-segment sum is zero, stored word included.
                let seg = &data[self.start..self.end];
                if seg.len() % 4 != 0 {
                    bail!("zero-sum-32 segment '{}' length not a multiple of 4", self.name);
                }
                let mut sum: u32 = 0;
                for w in seg.chunks_exact(4) {
                    sum = sum.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
                }
                Ok(sum == 0)
            }
            (_, Some(store_at)) => {
                // Computed over the payload (store bytes excluded), compared to stored.
                let stored = read_le(&data[store_at..store_at + self.store_width]);
                Ok(stored == self.compute_stored(data)?)
            }
            (_, None) => {
                // No stored location: validity is informational only (compute + compare externally).
                Ok(true)
            }
        }
    }

    /// Recompute and write the stored checksum. Returns (old, new) stored values.
    pub fn repair(&self, data: &mut [u8]) -> Result<(u64, u64)> {
        self.check_bounds(data)?;
        let store_at = match self.store_at {
            Some(a) => a,
            None => bail!("segment '{}' has no stored checksum location to repair", self.name),
        };
        let old = read_le(&data[store_at..store_at + self.store_width]);
        let new = self.compute_stored(data)?;
        // compute_stored for ZeroSum32Le already accounts for the stored word being
        // *part of* the segment — but only if we compute it over the segment with
        // the stored slot treated as part of the sum. To be exact: zero-sum stored
        // value = -(sum of all words except stored). Recompute that way.
        let new = if self.alg == ChecksumAlg::ZeroSum32Le {
            let mut sum: u32 = 0;
            for w in data[self.start..self.end].chunks_exact(4) {
                sum = sum.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
            }
            // Current sum includes the old stored word. New stored = old_stored - sum,
            // which zeroes the total.
            let cur = u32::from_le_bytes([
                data[store_at],
                data[store_at + 1],
                data[store_at + 2],
                data[store_at + 3],
            ]);
            (cur.wrapping_sub(sum)) as u64
        } else {
            new
        };
        for i in 0..self.store_width {
            data[store_at + i] = (new >> (8 * i)) as u8;
        }
        Ok((old, new))
    }

    fn check_bounds(&self, data: &[u8]) -> Result<()> {
        if self.start >= self.end || self.end > data.len() {
            bail!(
                "segment '{}' bounds 0x{:X}..0x{:X} invalid for {}-byte image",
                self.name,
                self.start,
                self.end,
                data.len()
            );
        }
        if let Some(a) = self.store_at {
            if a + self.store_width > data.len() {
                bail!("segment '{}' store location out of bounds", self.name);
            }
            if a < self.start || a + self.store_width > self.end {
                bail!("segment '{}' store location outside its own segment", self.name);
            }
        }
        Ok(())
    }
}

fn read_le(b: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, &x) in b.iter().enumerate() {
        v |= (x as u64) << (8 * i);
    }
    v
}

/// Standard reflected CRC-32 (IEEE 802.3).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_sum_segment() -> (Vec<u8>, SegmentChecksum) {
        // 64-byte segment, stored checksum in the LAST 4 bytes.
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        data[4..8].copy_from_slice(&0x0000_0001u32.to_le_bytes());
        data[12..16].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let seg = SegmentChecksum {
            name: "calibration".into(),
            start: 0,
            end: 64,
            alg: ChecksumAlg::ZeroSum32Le,
            store_at: Some(60),
            store_width: 4,
        };
        (data, seg)
    }

    #[test]
    fn zerosum_repair_makes_segment_valid() {
        let (mut data, seg) = zero_sum_segment();
        assert!(!seg.verify(&data).unwrap(), "raw buffer must start invalid");
        let (old, new) = seg.repair(&mut data).unwrap();
        assert_ne!(old, new);
        assert!(seg.verify(&data).unwrap(), "repaired buffer must verify");
    }

    #[test]
    fn zerosum_detects_single_cell_corruption() {
        let (mut data, seg) = zero_sum_segment();
        seg.repair(&mut data).unwrap();
        assert!(seg.verify(&data).unwrap());
        data[13] ^= 0x0F; // corrupt one byte inside the segment
        assert!(!seg.verify(&data).unwrap(), "corruption must be detected");
        seg.repair(&mut data).unwrap();
        assert!(seg.verify(&data).unwrap());
    }

    #[test]
    fn zerosum_repair_is_idempotent() {
        let (mut data, seg) = zero_sum_segment();
        seg.repair(&mut data).unwrap();
        let snapshot = data.clone();
        seg.repair(&mut data).unwrap();
        assert_eq!(snapshot, data, "second repair must be a no-op");
    }

    #[test]
    fn sum16_xor8_roundtrip() {
        let mut data = vec![0u8; 32];
        data[..6].copy_from_slice(b"engine");
        let seg = SegmentChecksum {
            name: "s".into(),
            start: 0,
            end: 32,
            alg: ChecksumAlg::Sum16,
            store_at: Some(28),
            store_width: 2,
        };
        seg.repair(&mut data).unwrap();
        assert!(seg.verify(&data).unwrap());

        let segx = SegmentChecksum {
            alg: ChecksumAlg::Xor8,
            store_at: Some(31),
            store_width: 1,
            ..seg.clone()
        };
        segx.repair(&mut data).unwrap();
        assert!(segx.verify(&data).unwrap());
    }

    #[test]
    fn crc32_known_vector() {
        // "123456789" -> 0xCBF43926 is the classic CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn zero_sum_requires_multiple_of_4() {
        let data = vec![0u8; 63];
        let seg = SegmentChecksum {
            name: "bad".into(),
            start: 0,
            end: 63,
            alg: ChecksumAlg::ZeroSum32Le,
            store_at: None,
            store_width: 4,
        };
        assert!(seg.compute_stored(&data).is_err());
    }
}
