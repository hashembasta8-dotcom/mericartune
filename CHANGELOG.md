# Changelog

## [0.3.0] — 2026-09-29 — "The Live Layer"

Real OBD-II vehicle communication — the engine behind `scan`/`read`/`dtc`/`vin`/`log`.

### Added
- `src/obd/` — full ELM327 / SAE J1979 protocol stack:
  - **Transport layer**: pluggable `Transport` trait; `MockTransport` (scripted/handler-based) for
    tests and demos; `SerialTransport` for real ELM327 adapters behind the `hardware` cargo
    feature (adds `serialport`, default features off for lean builds).
  - **ELM327 engine**: real AT init sequence (ATZ/ATE0/ATL0/ATS0/ATSP0), CR/LF normalization,
    echo handling, `SEARCHING...` banner stripping, typed error frames (`NO DATA`, `UNABLE TO
    CONNECT`, `CAN ERROR`, `BUFFER FULL`, `STOPPED`, `?`), headered + headerless frame parsing
    (11-bit `7E8` and 29-bit `18DAF110` CAN ids), **ISO-TP multi-frame assembly** (VIN).
  - **PID registry**: 18 Mode 01 PIDs with the exact SAE J1979 scaling formulas (RPM, TPS, speed,
    MAP, MAF, coolant/IAT/oil temps, STFT/LTFT, timing advance, fuel level/rate, module voltage,
    commanded lambda, run time, baro), support-bitmask decoding (0x00/0x20/0x40).
  - **DTC codec**: J2012 P/C/B/U encoding/decoding, stored/pending/permanent reads (Mode 03/07/0A),
    clear + MIL reset (Mode 04), count-byte and no-count response layouts.
  - **Simulated vehicle** (`obd::sim`): deterministic idle → WOT pull → coast drive profile for
    `--demo` and tests. Always labeled "SIMULATED VEHICLE".
- New CLI group `americartune obd`: `scan`, `read --pid`, `dtc [--clear|--pending]`, `vin`,
  `log --out pull.csv` (CSV is directly analyzable by `americartune datalog`).
- `obd log` writes `Time,RPM,TPS,AFR,MAP,Speed,IAT,ECT` — closes the loop: record with `obd log`,
  analyze with `datalog` (WOT pulls, lean-at-WOT, acceleration).
- 22 new tests (65 total now, all passing), incl. two end-to-end pipeline integrations.

### Fixed
- Datalog analyzer channel `ect` naming mismatch in the live logger (was `coolant_temp` in the
  registry) — lookups now fail gracefully with context instead of panicking.

## [0.2.0] — 2026-09-29 — "Truth and Engines"

The release that turns a design document into a working tool.

### Added
- Real ECU image engine: load/save flash dumps (256K/512K/1M/2M), bounds-checked LE integer access, GM-style OS ID scanning.
- Real checksum engines with verify/repair: **zero-sum-32le (GM segment checksum)**, sum16, xor8, crc32.
- Real TunerPro **XDF parser** (1.7 subset): tables, constants, axes, base64 embedded data, linear scaling extraction from math expressions (attribute and child forms).
- Real table engine: scaled u8/i8/u16/i16/u32 cells, physical units, bilinear interpolation, 3×3 smoothing, statistics.
- Platform definitions (GM P01 512K, GM P59 1M, generic) as JSON-overrideable data.
- **Aegis analyzer**: outlier spikes (robust z-score vs neighbor median), range violations, jagged maps (Laplacian), timing-at-load heuristics, rev-limit sanity, 0–100 score.
- **Suggestion engine**: deterministic, explainable cell repairs with iterative convergence; `--apply` writes and repairs checksums.
- **Datalog analysis**: delimiter sniffing, channel alias mapping, WOT pull detection, knock events, lean-at-WOT, acceleration estimates.
- **Image diff**: byte-level with table-cell attribution.
- `demo` command: self-contained end-to-end tour (fixtures + full workflow).
- Real CI (build + test + demo smoke test on Linux/macOS/Windows).
- Apache-2.0 LICENSE file.
- 41 unit tests + integration tests, all passing.

### Fixed
- **Removed all fake success output.** v0.1 printed "ECU binary read/written successfully!" without touching anything. Commands now do the work or exit non-zero with an honest message.
- Replaced the fake "build passing" badge (which pointed at a different repository) with a real CI badge backed by `.github/workflows/ci.yml`.
- Replaced JSON "definitions" that were mislabeled as XDF with a genuine XDF parser.
- Table edits now use scaled integer storage (real ECUs do not store f32 cells).
- Consistent naming: `americartune` crate and binary (was `mericartune`/`mercartune`/`AmericanTune` mixed).

### Removed
- Placeholder marketplace/revenue-projection content in the README (design-doc material does not belong in a product README).
- Unused dependency bloat (tokio, reqwest, uuid, chrono, memmap2, dialoguer, colored, tracing).

## [0.1.0] — 2026-09-24 — "The Design Document"
- Initial CLI skeleton with placeholder handlers and aspirational README. Kept for history; superseded entirely by 0.2.0.
