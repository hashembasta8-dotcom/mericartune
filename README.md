# AmericanTune

**Open-source ECU tuning suite for American cars — GM LS/LT, Ford Coyote/Power Stroke, Dodge Hemi/Cummins.**

[![CI](https://github.com/hashembasta8-dotcom/mericartune/actions/workflows/ci.yml/badge.svg)](https://github.com/hashembasta8-dotcom/mericartune/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)

> **Status: v0.2.0 — real, tested file-based tuning workflows.**
> This README only claims what the code actually does. Anything not implemented says so.

---

## What it is

AmericanTune is a Rust CLI for working with real American-car ECU flash images:

- **Identify** flash dumps (size, platform, GM OS IDs, checksum state)
- **Verify & repair checksums** — GM zero-sum-32 segment checksums, sum16, xor8, crc32
- **Parse TunerPro XDF definitions** — tables, constants, breakpoints, linear scalings
- **Edit calibration tables** in physical units (scaled u8/u16/i16 storage handled correctly)
- **Analyze tunes** ("Aegis") — outlier spikes, range violations, jagged maps, timing-at-load risk
- **Suggest & apply fixes** — deterministic, explainable, iterative to convergence
- **Analyze datalogs** — WOT pull detection, knock events, lean-at-WOT, acceleration estimates
- **Diff images** — byte-level, attributed back to XDF table cells

Try the full tour in one command:

```bash
cargo run -- demo
```

It generates a synthetic 512 KiB P01-style image + XDF + datalog, injects damage, detects it,
suggests fixes, applies them, re-verifies the checksum, diffs the images, and analyzes the log.
Every number in the output comes from the engines in this repo.

## Quick start

```bash
git clone https://github.com/hashembasta8-dotcom/mericartune
cd mericartune
cargo build --release

# Identify a flash dump
./target/release/americartune identify my_car.bin

# Verify checksums (add --fix to repair, --out to write elsewhere)
./target/release/americartune checksum my_car.bin

# List tables from a TunerPro XDF definition
./target/release/americartune tables --xdf my_car.xdf

# Show a table with statistics
./target/release/americartune show my_car.bin --xdf my_car.xdf --table "VE"

# Edit one cell (physical units) — checksums are repaired automatically
./target/release/americartune set my_car.bin --xdf my_car.xdf \
    --table "VE" --cell 3,5 --value 92.5 --out my_car_stage1.bin

# Safety analysis
./target/release/americartune analyze my_car_stage1.bin --xdf my_car.xdf

# Suggested fixes (add --apply to write them)
./target/release/americartune ai my_car_stage1.bin --xdf my_car.xdf

# Datalog analysis (HP Tuners / TunerStudio-style CSV)
./target/release/americartune datalog pull.csv

# Diff two images, attributed to table cells
./target/release/americartune diff my_car.bin my_car_stage1.bin --xdf my_car.xdf

# Live OBD-II (ELM327 adapter) — or explore safely with --demo
./target/release/americartune obd scan --demo
./target/release/americartune obd read --demo --pid rpm,tps,speed,map
./target/release/americartune obd dtc --demo
./target/release/americartune obd log --demo --out pull.csv --rate 10 --duration 12
./target/release/americartune datalog pull.csv   # analyze what you recorded
```

## Honest status matrix

| Feature | Status | Notes |
|---|---|---|
| Image I/O (256K/512K/1M/2M) | ✅ Implemented | LE integer access, bounds-checked |
| Checksum verify/repair | ✅ Implemented | zero-sum-32le (GM), sum16, xor8, crc32 — 41 unit tests |
| TunerPro XDF parsing | ✅ Implemented | 1.7 subset: tables/constants/axes, base64 embedded data, linear math expressions |
| Table editing (scaled ints) | ✅ Implemented | u8/i8/u16/i16/u32, factor/offset scalings, interpolation, smoothing |
| Platform definitions | ✅ Implemented | GM P01 512K, GM P59 1M, generic (JSON-overrideable) |
| Aegis analyzer | ✅ Implemented | robust z-score outlier detection, range bands, Laplacian jaggedness, timing heuristics |
| Suggestion engine | ✅ Implemented | deterministic repairs with before/after values, iterative convergence |
| Datalog analysis | ✅ Implemented | WOT pulls, knock, lean, 0-60/60-130-style acceleration from CSV logs |
| Image diff | ✅ Implemented | byte-level + table-cell attribution |
| Live OBD-II / ELM327 I/O | ✅ Implemented | Real ELM327/J1979 engine: PID formulas, DTC codec, ISO-TP VIN, live CSV logging. Serial hardware behind `--features hardware`; `--demo` uses a clearly-labeled simulator |
| AI/ML calibration assistant | 🗺 Roadmap | current "AI" command = explainable rules + statistics, honestly labeled |
| Cloud marketplace | 🗺 Roadmap | not implemented; the `cloud` command does not exist yet on purpose |
| GUI (Tauri) | 🗺 Roadmap | CLI first |

## Why v0.2 exists

v0.1 of this repository was a design document with placeholder prints that faked success messages
("ECU binary written successfully!" while writing nothing). That was wrong. v0.2 replaces every
placeholder with a tested engine, deletes the fake badges and fake success output, and adopts this
rule: **if a command cannot do the work yet, it says so and exits non-zero — it never pretends.**

## Architecture

```
americartune/
├── src/
│   ├── ecu/
│   │   ├── bin.rs          ECU image container (load/save, LE access, OS ID scan)
│   │   ├── checksum.rs     segment checksum engines (verify/repair)
│   │   ├── xdf.rs          TunerPro XDF parser (XML-lite + base64, no deps)
│   │   ├── tables.rs       scaled table engine + linear scaling extraction
│   │   └── platforms.rs    GM P01/P59/generic layouts (JSON-overrideable)
│   ├── analyze/            Aegis safety analyzer (rules + robust statistics)
│   ├── ai/                 suggestion engine (findings -> concrete cell edits)
│   ├── obd/                live OBD-II stack
│   │   ├── transport.rs    Transport trait + mock + serial (`hardware` feature)
│   │   ├── elm.rs          ELM327 protocol engine (frames, ISO-TP, errors)
│   │   ├── pid.rs          SAE J1979 Mode 01 PID registry (real formulas)
│   │   ├── dtc.rs          trouble-code codec (P/C/B/U, Mode 03/07/0A/04)
│   │   └── sim.rs          simulated drive profile for --demo and tests
│   ├── datalog.rs          CSV log analysis (WOT/knock/lean/acceleration)
│   ├── diff.rs             image + table-aware diffing
│   └── main.rs             CLI
├── platforms/              drop-in JSON layout overrides
├── tests/                  end-to-end integration tests
└── .github/workflows/ci.yml  real CI: build + test + demo smoke test on 3 OSes
```

Dependencies are deliberately minimal: `clap`, `serde`, `serde_json`, `anyhow`. The XML and base64
handling is hand-rolled and tested so the binary stays lean and auditable.

## Platform layouts

Built-in layouts are **reference data** (community-documented segment bounds) — the engines are
exact and tested, but per-OS offsets can vary. Ship your own definitions without recompiling:

```bash
americartune checksum my_car.bin --platforms-dir ./platforms
```

See `platforms/gm_p01_512k.json` for the schema. **Always keep a backup of your stock dump before
writing anything to a vehicle.**

## Legal / safety notice

ECU calibration work can damage engines and may be regulated (emissions, road legality). This tool
is provided for legitimate tuning, research and educational use. Verify every write with a checksum
check and a bench test before flashing a vehicle. The authors accept no liability for damaged
engines, ECUs, or vehicles.

## License

Apache-2.0 — see [LICENSE](LICENSE).
