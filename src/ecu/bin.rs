//! ECU image container: a full flash dump with real load/save/verify.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// Well-known flash image sizes across supported platforms.
pub const KNOWN_SIZES: [usize; 4] = [0x4_0000, 0x8_0000, 0x10_0000, 0x20_0000]; // 256K, 512K, 1M, 2M

/// A complete ECU flash image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcuImage {
    data: Vec<u8>,
    label: String,
}

impl EcuImage {
    /// Wrap raw bytes. `label` is the file path or source name (for messages).
    pub fn new(data: Vec<u8>, label: impl Into<String>) -> Result<Self> {
        if data.is_empty() {
            bail!("ECU image is empty");
        }
        Ok(Self { data, label: label.into() })
    }

    /// Load a flash dump from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)
            .with_context(|| format!("failed to read image {}", path.display()))?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".into());
        Self::new(data, name)
    }

    /// Save the (possibly edited) image to disk.
    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, &self.data)
            .with_context(|| format!("failed to write image {}", path.display()))
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// True when the size matches one of the known flash sizes.
    pub fn has_known_size(&self) -> bool {
        KNOWN_SIZES.contains(&self.data.len())
    }

    /// Human-readable size ("512 KiB" style).
    pub fn size_human(&self) -> String {
        if self.data.len() % (1024 * 1024) == 0 {
            format!("{} MiB", self.data.len() / (1024 * 1024))
        } else if self.data.len() % 1024 == 0 {
            format!("{} KiB", self.data.len() / 1024)
        } else {
            format!("{} bytes", self.data.len())
        }
    }

    /// Little-endian unsigned read of `width` bytes at `addr`.
    pub fn read_uint(&self, addr: usize, width: usize) -> Result<u64> {
        if addr + width > self.data.len() {
            bail!("read 0x{:X}+{} out of bounds (image {} bytes)", addr, width, self.data.len());
        }
        let mut v: u64 = 0;
        for i in 0..width {
            v |= (self.data[addr + i] as u64) << (8 * i);
        }
        Ok(v)
    }

    /// Little-endian signed read of `width` bytes at `addr` (two's complement).
    pub fn read_int(&self, addr: usize, width: usize) -> Result<i64> {
        let u = self.read_uint(addr, width)?;
        let bits = width * 8;
        Ok(((u << (64 - bits)) as i64) >> (64 - bits))
    }

    /// Little-endian write of `width` bytes at `addr`.
    pub fn write_uint(&mut self, addr: usize, width: usize, value: u64) -> Result<()> {
        if addr + width > self.data.len() {
            bail!("write 0x{:X}+{} out of bounds (image {} bytes)", addr, width, self.data.len());
        }
        for i in 0..width {
            self.data[addr + i] = (value >> (8 * i)) as u8;
        }
        Ok(())
    }

    /// Scan for 8-digit ASCII numeric OS IDs (GM style, e.g. b"12208322").
    /// Returns (offset, id) pairs; de-duplicated by id, sorted by offset.
    pub fn scan_os_ids(&self) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let d = &self.data;
        if d.len() < 8 {
            return found;
        }
        for i in 0..=d.len() - 8 {
            let window = &d[i..i + 8];
            if window.iter().all(|b| b.is_ascii_digit()) {
                let id = String::from_utf8_lossy(window).to_string();
                // Real GM OS IDs live in the 10M–13M range (they start with "12").
                if id.starts_with('1') && seen.insert(id.clone()) {
                    found.push((i, id));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_uint_int() {
        let mut img = EcuImage::new(vec![0u8; 64], "t").unwrap();
        img.write_uint(4, 2, 0xBEEF).unwrap();
        assert_eq!(img.read_uint(4, 2).unwrap(), 0xBEEF);
        img.write_uint(8, 2, (-5i32) as u16 as u64).unwrap();
        assert_eq!(img.read_int(8, 2).unwrap(), -5);
    }

    #[test]
    fn bounds_are_enforced() {
        let img = EcuImage::new(vec![0u8; 16], "t").unwrap();
        assert!(img.read_uint(15, 2).is_err());
    }

    #[test]
    fn known_sizes_detected() {
        let img = EcuImage::new(vec![0u8; 0x8_0000], "t").unwrap();
        assert!(img.has_known_size());
        assert_eq!(img.size_human(), "512 KiB");
    }

    #[test]
    fn os_id_scan_finds_gm_style_ids() {
        let mut data = vec![0u8; 32];
        data[4..12].copy_from_slice(b"12208322");
        data[16..24].copy_from_slice(b"12208322"); // duplicate
        data[24..28].copy_from_slice(b"9999"); // too short
        let img = EcuImage::new(data, "t").unwrap();
        let ids = img.scan_os_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], (4, "12208322".to_string()));
    }
}
