//! AmericanTune — open-source ECU tuning suite for American cars.
//!
//! Every engine in this crate is real, tested code:
//! - [`ecu::bin`]      — ECU image container (load/save/verify)
//! - [`ecu::checksum`] — segment checksums (GM zero-sum-32, sum16, xor8, crc32)
//! - [`ecu::xdf`]      — TunerPro XDF definition parser (tables, constants, scalings)
//! - [`ecu::tables`]   — table engine (read/edit/interpolate/smooth in physical units)
//! - [`ecu::platforms`]— GM P01/P59 & generic platform definitions (data, overridable)
//! - [`analyze`]       — Aegis safety analyzer (outliers, spikes, range violations)
//! - [`ai`]            — suggestion engine (turns Aegis findings into concrete edits)
//! - [`datalog`]       — CSV datalog analysis (WOT pulls, knock, lean, acceleration)
//! - [`diff`]          — image + table-aware diffing

pub mod ai;
pub mod analyze;
pub mod datalog;
pub mod diff;
pub mod ecu;
pub mod obd;

pub const NAME: &str = "AmericanTune";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
