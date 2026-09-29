//! Platform definitions: real GM P01/P59 & generic layouts as data.
//!
//! Layouts live in JSON so shops can override offsets without recompiling
//! (`--platforms-dir`). Segment bounds below are community-documented
//! reference values — the *engine* (checksums, tables, XDF) is exact and
//! tested; the *offsets* are data you can correct per OS ID.

use crate::ecu::checksum::{ChecksumAlg, SegmentChecksum};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformDef {
    pub id: String,
    pub name: String,
    pub family: String,
    /// Flash image size in bytes.
    pub image_size: usize,
    /// Known OS part numbers (GM 8-digit style).
    #[serde(default)]
    pub known_os_ids: Vec<String>,
    /// Where to look for the OS ID in the image (offset, ASCII length).
    #[serde(default)]
    pub os_id_hint: Option<OsIdHint>,
    /// Checksummed segments (verified/repaired by `americartune checksum`).
    #[serde(default)]
    pub segments: Vec<SegmentDef>,
    /// Documentation status of the layout ("reference", "verified").
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsIdHint {
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentDef {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub algorithm: String,
    #[serde(default)]
    pub store_at: Option<usize>,
    #[serde(default = "default_store_width")]
    pub store_width: usize,
}

fn default_store_width() -> usize {
    4
}

impl SegmentDef {
    pub fn to_checksum(&self) -> Result<SegmentChecksum> {
        Ok(SegmentChecksum {
            name: self.name.clone(),
            start: self.start,
            end: self.end,
            alg: ChecksumAlg::from_name(&self.algorithm)?,
            store_at: self.store_at,
            store_width: self.store_width,
        })
    }
}

impl PlatformDef {
    /// Load all platform definitions: embedded defaults + optional extra dir.
    pub fn load_all(extra_dir: Option<&Path>) -> Result<Vec<PlatformDef>> {
        let mut all = builtin_platforms();
        if let Some(dir) = extra_dir {
            if dir.is_dir() {
                let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
                    .with_context(|| format!("cannot read platforms dir {}", dir.display()))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                    .collect();
                entries.sort();
                for path in entries {
                    let text = std::fs::read_to_string(&path)?;
                    let def: PlatformDef = serde_json::from_str(&text)
                        .with_context(|| format!("bad platform JSON {}", path.display()))?;
                    // External definitions override built-ins with the same id.
                    all.retain(|p| p.id != def.id);
                    all.push(def);
                }
            }
        }
        Ok(all)
    }

    /// Guess which platform an image belongs to (size first, then OS ID hits).
    pub fn detect<'a>(platforms: &'a [PlatformDef], image: &[u8]) -> Vec<(&'a PlatformDef, Vec<String>)> {
        let mut hits = Vec::new();
        let ids = crate::ecu::bin::EcuImage::new(image.to_vec(), "detect")
            .map(|img| img.scan_os_ids())
            .unwrap_or_default();
        let id_strings: Vec<String> = ids.iter().map(|(_, s)| s.clone()).collect();
        for p in platforms {
            if p.image_size != image.len() {
                continue;
            }
            let matched: Vec<String> = p
                .known_os_ids
                .iter()
                .filter(|k| id_strings.iter().any(|i| i == *k))
                .cloned()
                .collect();
            // Size-matched candidates are reported; OS matches sort first.
            hits.push((p, matched));
        }
        hits.sort_by_key(|(_, m)| std::cmp::Reverse(m.len()));
        hits
    }
}

/// Built-in reference layouts.
pub fn builtin_platforms() -> Vec<PlatformDef> {
    vec![
        PlatformDef {
            id: "gm_p01_512k".into(),
            name: "GM P01 (512 KiB) — LS1/LS6 truck & F-body".into(),
            family: "GM LS/LT".into(),
            image_size: 0x8_0000,
            known_os_ids: vec![
                "12208322".into(),
                "12208332".into(),
                "12208342".into(),
                "12202322".into(),
            ],
            os_id_hint: Some(OsIdHint { offset: 0x500, len: 8 }),
            segments: vec![
                SegmentDef {
                    name: "calibration".into(),
                    // Reference layout: the tuneable calibration block with its
                    // trailing zero-sum-32 stored checksum word.
                    start: 0x2_0000,
                    end: 0x6_0000,
                    algorithm: "zero-sum-32le".into(),
                    store_at: Some(0x5_FFFC),
                    store_width: 4,
                },
            ],
            status: "reference".into(),
        },
        PlatformDef {
            id: "gm_p59_1m".into(),
            name: "GM P59 (1 MiB) — LS2/4.8/5.3/6.0".into(),
            family: "GM LS/LT".into(),
            image_size: 0x10_0000,
            known_os_ids: vec![
                "12587811".into(),
                "12605114".into(),
                "12606807".into(),
                "12613246".into(),
                "12629623".into(),
            ],
            os_id_hint: Some(OsIdHint { offset: 0x800, len: 8 }),
            segments: vec![SegmentDef {
                name: "calibration".into(),
                start: 0x4_0000,
                end: 0xC_0000,
                algorithm: "zero-sum-32le".into(),
                store_at: Some(0xB_FFFC),
                store_width: 4,
            }],
            status: "reference".into(),
        },
        PlatformDef {
            id: "generic_512k".into(),
            name: "Generic 512 KiB image (sum16 trailer)".into(),
            family: "generic".into(),
            image_size: 0x8_0000,
            known_os_ids: vec![],
            os_id_hint: None,
            segments: vec![SegmentDef {
                name: "whole-image".into(),
                start: 0,
                end: 0x7_FFFE,
                algorithm: "sum16".into(),
                store_at: Some(0x7_FFFE),
                store_width: 2,
            }],
            status: "reference".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_and_serialize() {
        let all = builtin_platforms();
        assert!(all.len() >= 3);
        let json = serde_json::to_string_pretty(&all[0]).unwrap();
        let back: PlatformDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, all[0].id);
    }

    #[test]
    fn detect_matches_size_and_os() {
        let all = builtin_platforms();
        let mut img = vec![0u8; 0x8_0000];
        img[0x500..0x508].copy_from_slice(b"12208322");
        let hits = PlatformDef::detect(&all, &img);
        assert!(!hits.is_empty());
        let (top, matched) = &hits[0];
        assert_eq!(top.id, "gm_p01_512k");
        assert_eq!(matched, &vec!["12208322".to_string()]);
    }

    #[test]
    fn segments_convert_to_checksums() {
        let all = builtin_platforms();
        let seg = all[0].segments[0].to_checksum().unwrap();
        assert_eq!(seg.alg, ChecksumAlg::ZeroSum32Le);
    }
}
