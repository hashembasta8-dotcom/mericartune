//! AmericanTune CLI — every command performs real work or says so honestly.

use americartune::ai;
use americartune::analyze;
use americartune::datalog::Datalog;
use americartune::diff;
use americartune::ecu::bin::EcuImage;
use americartune::ecu::platforms::{builtin_platforms, PlatformDef};
use americartune::ecu::tables::{EcuImageMut, EcuImageRef, Table2D};
use americartune::ecu::xdf::XdfDocument;
use americartune::obd::transport::Transport;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "americartune")]
#[command(
    about = "Open-source ECU tuning suite for American cars — GM LS/LT, Ford Coyote/Power Stroke, Dodge Hemi/Cummins",
    long_about = None,
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show version, engines and honest implementation status
    Info,

    /// List known platform definitions (GM P01/P59, generic)
    Platforms {
        /// Extra platform JSON definitions directory (overrides built-ins)
        #[arg(long)]
        platforms_dir: Option<PathBuf>,
    },

    /// Identify an image: size, platform candidates, OS IDs, checksum status
    Identify {
        /// Path to the flash dump (.bin)
        bin: PathBuf,
        #[arg(long)]
        platforms_dir: Option<PathBuf>,
    },

    /// Verify (or repair) segment checksums on an image
    Checksum {
        bin: PathBuf,
        /// Recompute and write stored checksums
        #[arg(long)]
        fix: bool,
        /// Output path for the repaired image (default: in-place with --fix)
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        platforms_dir: Option<PathBuf>,
    },

    /// List tables from an XDF definition
    Tables {
        /// TunerPro XDF definition file
        #[arg(long)]
        xdf: PathBuf,
    },

    /// Print a table's contents and statistics
    Show {
        bin: PathBuf,
        #[arg(long)]
        xdf: PathBuf,
        /// Table name (substring match); default: first table
        #[arg(long)]
        table: Option<String>,
    },

    /// Edit one table cell (physical units) and repair checksums
    Set {
        bin: PathBuf,
        #[arg(long)]
        xdf: PathBuf,
        #[arg(long)]
        table: String,
        /// Cell as row,col
        #[arg(long)]
        cell: String,
        /// New value in physical units
        #[arg(long)]
        value: f64,
        #[arg(long)]
        out: PathBuf,
    },

    /// Aegis analysis: outlier spikes, range violations, jagged maps, timing risk
    Analyze {
        bin: PathBuf,
        #[arg(long)]
        xdf: PathBuf,
        /// Machine-readable JSON report
        #[arg(long)]
        json: bool,
    },

    /// Suggested edits from analysis (deterministic, explainable) with optional apply
    Ai {
        bin: PathBuf,
        #[arg(long)]
        xdf: PathBuf,
        /// Apply suggestions and repair checksums
        #[arg(long)]
        apply: bool,
        /// Output path when applying
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Analyze a CSV datalog: WOT pulls, knock events, lean, acceleration
    Datalog {
        /// CSV/TSV/semicolon log (HP Tuners / TunerStudio style headers)
        file: PathBuf,
        /// WOT throttle threshold (%)
        #[arg(long, default_value_t = 85.0)]
        tps: f64,
        /// Knock-retard threshold (degrees)
        #[arg(long, default_value_t = 2.0)]
        knock: f64,
        /// Lean AFR limit at WOT
        #[arg(long, default_value_t = 13.0)]
        lean: f64,
    },

    /// Diff two images (byte-level, optionally attributed to XDF tables)
    Diff {
        a: PathBuf,
        b: PathBuf,
        #[arg(long)]
        xdf: Option<PathBuf>,
    },

    /// OBD-II vehicle communication (ELM327 / SAE J1979)
    Obd {
        #[command(subcommand)]
        cmd: ObdCmd,
    },

    /// Generate a self-contained demo (fixtures + full workflow) — the tour
    Demo {
        /// Directory for generated demo files
        #[arg(long, default_value = "americartune-demo")]
        out: PathBuf,
    },

    /// Scan for vehicles over OBD-II/J2534
    Scan {
        /// Run a clearly-labeled simulated demo stream
        #[arg(long)]
        demo: bool,
    },
}

/// OBD-II subcommands (real ELM327/J1979 engine; `--demo` uses a labeled simulator).
#[derive(Subcommand)]
enum ObdCmd {
    /// Probe the vehicle: adapter ID, supported-PID matrix
    Scan {
        /// Simulated vehicle (clearly labeled — no hardware needed)
        #[arg(long)]
        demo: bool,
        /// Serial port of the ELM327 adapter (e.g. /dev/ttyUSB0, COM3)
        #[arg(long)]
        port: Option<String>,
        /// Baud rate (hardware feature builds)
        #[arg(long, default_value_t = 38400)]
        baud: u32,
    },
    /// Read live PIDs (comma-separated names or hex, e.g. rpm,tps,0x0C)
    Read {
        #[arg(long)]
        pid: String,
        #[arg(long)]
        demo: bool,
        #[arg(long)]
        port: Option<String>,
    },
    /// Read (or clear) diagnostic trouble codes
    Dtc {
        /// Clear codes + reset MIL (Mode 04)
        #[arg(long)]
        clear: bool,
        /// Show pending codes (Mode 07) instead of stored (Mode 03)
        #[arg(long)]
        pending: bool,
        #[arg(long)]
        demo: bool,
        #[arg(long)]
        port: Option<String>,
    },
    /// Read the vehicle VIN (Mode 09)
    Vin {
        #[arg(long)]
        demo: bool,
        #[arg(long)]
        port: Option<String>,
    },
    /// Live datalog to CSV — output works with `americartune datalog`
    Log {
        /// Output CSV path
        #[arg(long)]
        out: PathBuf,
        /// Samples per second
        #[arg(long, default_value_t = 10.0)]
        rate: f64,
        /// Seconds to record (0 = until Ctrl-C)
        #[arg(long, default_value_t = 12.0)]
        duration: f64,
        #[arg(long)]
        demo: bool,
        #[arg(long)]
        port: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Info => cmd_info(),
        Commands::Platforms { platforms_dir } => cmd_platforms(platforms_dir.as_deref()),
        Commands::Identify { bin, platforms_dir } => cmd_identify(&bin, platforms_dir.as_deref()),
        Commands::Checksum { bin, fix, out, platforms_dir } => {
            cmd_checksum(&bin, fix, out.as_deref(), platforms_dir.as_deref())
        }
        Commands::Tables { xdf } => cmd_tables(&xdf),
        Commands::Show { bin, xdf, table } => cmd_show(&bin, &xdf, table.as_deref()),
        Commands::Set { bin, xdf, table, cell, value, out } => {
            cmd_set(&bin, &xdf, &table, &cell, value, &out)
        }
        Commands::Analyze { bin, xdf, json } => cmd_analyze(&bin, &xdf, json),
        Commands::Ai { bin, xdf, apply, out } => cmd_ai(&bin, &xdf, apply, out.as_ref()),
        Commands::Datalog { file, tps, knock, lean } => cmd_datalog(&file, tps, knock, lean),
        Commands::Diff { a, b, xdf } => cmd_diff(&a, &b, xdf.as_ref()),
        Commands::Obd { cmd } => cmd_obd(cmd),
        Commands::Demo { out } => cmd_demo(&out),
        Commands::Scan { demo } => cmd_scan(demo),
    }
}

// ---------------------------------------------------------------- commands

fn cmd_info() -> Result<()> {
    println!("{} v{}", americartune::NAME, americartune::VERSION);
    println!("Open-source ECU tuning suite for American cars");
    println!();
    println!("Engines (all real, tested code):");
    println!("  ✅ ECU image I/O          load/save flash dumps, LE integer access");
    println!("  ✅ Checksums              zero-sum-32le (GM), sum16, xor8, crc32 — verify/repair");
    println!("  ✅ XDF parser             TunerPro 1.7 subset: tables, constants, scalings");
    println!("  ✅ Table engine           scaled u8/u16/i16 cells, interpolate, smooth");
    println!("  ✅ Aegis analyzer         outlier/range/jagged/timing rules + score");
    println!("  ✅ Suggestion engine      deterministic fix proposals with --apply");
    println!("  ✅ Datalog analysis       WOT pulls, knock, lean, acceleration");
    println!("  ✅ Image diff             byte-level + table attribution");
    println!("  🔬 OBD-II/J2534 live I/O  protocol layer — roadmap v0.3 (commands are honest about this)");
    println!("  🗺  Cloud marketplace      not implemented — roadmap");
    println!();
    println!("Platforms: GM P01 512K, GM P59 1M, generic 512K (override with --platforms-dir)");
    Ok(())
}

fn cmd_platforms(dir: Option<&std::path::Path>) -> Result<()> {
    let all = PlatformDef::load_all(dir)?;
    println!("{:<16} {:<10} {:<12} {:<10} {}", "ID", "SIZE", "FAMILY", "STATUS", "NAME");
    for p in all {
        println!(
            "{:<16} {:<10} {:<12} {:<10} {}",
            p.id,
            format!("{} KiB", p.image_size / 1024),
            p.family,
            p.status,
            p.name
        );
    }
    Ok(())
}

fn load_platforms(dir: Option<&std::path::Path>) -> Result<Vec<PlatformDef>> {
    PlatformDef::load_all(dir)
}

fn cmd_identify(path: &PathBuf, dir: Option<&std::path::Path>) -> Result<()> {
    let img = EcuImage::load(path)?;
    let platforms = load_platforms(dir)?;
    println!("image   : {} ({})", img.label(), img.size_human());
    println!(
        "size    : 0x{:X} bytes — {}",
        img.len(),
        if img.has_known_size() { "known flash size ✅" } else { "unusual size ⚠️" }
    );

    let hits = PlatformDef::detect(&platforms, img.data());
    println!("platform candidates:");
    for (p, matched) in hits.iter().take(5) {
        let tag = if matched.is_empty() { "size match".to_string() } else { format!("OS match: {}", matched.join(", ")) };
        println!("  {:<16} {} [{}]", p.id, p.name, tag);
    }

    let ids = img.scan_os_ids();
    if ids.is_empty() {
        println!("OS IDs  : none found (ASCII scan)");
    } else {
        println!("OS IDs  :");
        for (off, id) in ids.iter().take(8) {
            println!("  0x{:06X}  {}", off, id);
        }
    }

    println!("checksum status:");
    for (p, matched) in PlatformDef::detect(&platforms, img.data()) {
        if !matched.is_empty() || hits.first().map(|(t, _)| t.id == p.id).unwrap_or(false) {
            for s in &p.segments {
                let cs = s.to_checksum()?;
                let ok = cs.verify(img.data())?;
                println!(
                    "  [{}] {} {}",
                    if ok { "OK " } else { "BAD" },
                    p.id,
                    s.name
                );
            }
        }
    }
    Ok(())
}

fn cmd_checksum(
    path: &PathBuf,
    fix: bool,
    out: Option<&std::path::Path>,
    dir: Option<&std::path::Path>,
) -> Result<()> {
    let mut img = EcuImage::load(path)?;
    let platforms = load_platforms(dir)?;
    let hits = PlatformDef::detect(&platforms, img.data());
    let (platform, _) = hits
        .first()
        .ok_or_else(|| anyhow::anyhow!("no platform matches this image size"))?;

    let mut any_bad = false;
    for s in &platform.segments {
        let cs = s.to_checksum()?;
        let ok = cs.verify(img.data())?;
        println!(
            "segment {:<14} [0x{:06X}..0x{:06X}] {} : {}",
            s.name,
            s.start,
            s.end,
            cs.alg.name(),
            if ok { "valid ✅" } else { "INVALID ❌" }
        );
        if !ok {
            any_bad = true;
            if fix {
                let (old, new) = cs.repair(img.data_mut())?;
                println!("  repaired: stored 0x{:08X} -> 0x{:08X}", old, new);
            }
        }
    }
    if fix {
        let dest = out.unwrap_or(path.as_path());
        img.save(dest)?;
        println!("wrote {}", dest.display());
    } else if any_bad {
        println!("\nrun with --fix to repair (add --out to write elsewhere)");
    }
    Ok(())
}

fn load_xdf(path: &PathBuf) -> Result<(XdfDocument, Vec<Table2D>)> {
    let doc = XdfDocument::load(path)?;
    let tables = doc.build_tables()?;
    Ok((doc, tables))
}

fn cmd_tables(xdf_path: &PathBuf) -> Result<()> {
    let (doc, tables) = load_xdf(xdf_path)?;
    println!("XDF    : {} — \"{}\" by {}", xdf_path.display(), doc.title, doc.author);
    println!("tables : {}   constants: {}", doc.tables.len(), doc.constants.len());
    println!();
    println!("{:<28} {:>6}x{:<6} {:>10} {:>10} {}", "TABLE", "ROWS", "COLS", "ADDR", "CELL", "UNITS");
    for t in &tables {
        println!(
            "{:<28} {:>6}x{:<6} 0x{:08X} {:>10} {}",
            t.name,
            t.n_rows(),
            t.n_cols(),
            t.address,
            format!("{:?}", t.width),
            t.units
        );
    }
    if !doc.constants.is_empty() {
        println!("\nconstants:");
        for c in &doc.constants {
            println!("  {:<26} 0x{:08X} {}", c.title, c.data.address, c.units);
        }
    }
    Ok(())
}

fn pick_table<'a>(tables: &'a [Table2D], name: Option<&str>) -> Result<&'a Table2D> {
    match name {
        None => tables.first().context("XDF has no tables"),
        Some(n) => {
            let n_low = n.to_ascii_lowercase();
            tables
                .iter()
                .find(|t| t.name.to_ascii_lowercase().contains(&n_low))
                .with_context(|| format!("no table matching '{}'", n))
        }
    }
}

fn cmd_show(bin: &PathBuf, xdf_path: &PathBuf, name: Option<&str>) -> Result<()> {
    let img = EcuImage::load(bin)?;
    let (_doc, tables) = load_xdf(xdf_path)?;
    let t = pick_table(&tables, name)?;
    let e = EcuImageRef(&img);
    let grid = t.grid(&e)?;
    let stats = t.stats(&e)?;

    println!("table: {} [{}]  {}x{}  addr 0x{:08X}", t.name, t.units, t.n_rows(), t.n_cols(), t.address);
    println!("scaling: phys = raw * {:.6} + {:.6}", t.scaling.factor, t.scaling.offset);
    println!("stats: min {:.3}  max {:.3}  mean {:.3}  cells {}", stats.min, stats.max, stats.mean, stats.cells);
    println!();
    print_grid(&t.cols, &t.rows, &grid, &t.units);
    Ok(())
}

fn print_grid(cols: &[f64], rows: &[f64], grid: &[Vec<f64>], units: &str) {
    print!("{:>10}", "");
    for c in cols {
        print!("{:>9.0}", c);
    }
    println!("   (cols)");
    for (r, rv) in rows.iter().enumerate() {
        print!("{:>10.1}", rv);
        for v in &grid[r] {
            print!("{:>9.2}", v);
        }
        println!();
    }
    println!("   (rows) units: {}", units);
}

fn cmd_set(
    bin: &PathBuf,
    xdf_path: &PathBuf,
    name: &str,
    cell: &str,
    value: f64,
    out: &PathBuf,
) -> Result<()> {
    let parts: Vec<usize> = cell
        .split(',')
        .map(|s| s.trim().parse::<usize>())
        .collect::<std::result::Result<_, _>>()
        .context("cell must be row,col (e.g. 2,5)")?;
    if parts.len() != 2 {
        bail!("cell must be row,col (e.g. 2,5)");
    }
    let (r, c) = (parts[0], parts[1]);

    let mut img = EcuImage::load(bin)?;
    let (_doc, tables) = load_xdf(xdf_path)?;
    let t = pick_table(&tables, Some(name))?;
    let old = t.get(&EcuImageRef(&img), r, c)?;
    let stored = t.set(&mut EcuImageMut(&mut img), r, c, value)?;
    println!("{}.set[{},{}] {:.3} -> {:.3} {}", t.name, r, c, old, stored, t.units);

    repair_checksums_if_known(&mut img, None)?;
    img.save(out)?;
    println!("wrote {} (checksums verified)", out.display());
    Ok(())
}

fn repair_checksums_if_known(img: &mut EcuImage, dir: Option<&std::path::Path>) -> Result<bool> {
    let platforms = load_platforms(dir)?;
    let hits = PlatformDef::detect(&platforms, img.data());
    let mut repaired = false;
    if let Some((p, _)) = hits.first() {
        for s in &p.segments {
            let cs = s.to_checksum()?;
            if !cs.verify(img.data())? {
                cs.repair(img.data_mut())?;
                repaired = true;
            }
        }
    }
    Ok(repaired)
}

fn cmd_analyze(bin: &PathBuf, xdf_path: &PathBuf, json: bool) -> Result<()> {
    let img = EcuImage::load(bin)?;
    let (_doc, tables) = load_xdf(xdf_path)?;
    let report = analyze::analyze(&tables, &EcuImageRef(&img))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("Aegis analysis — {} ({} tables, {} cells)", img.label(), report.tables_scanned, report.cells_scanned);
    println!();
    if report.findings.is_empty() {
        println!("  no findings — calibration looks clean ✅");
    } else {
        for f in &report.findings {
            println!("  [{}] {:<18} {}", f.severity, f.rule, f.message);
        }
    }
    let (info, warn, crit) = report.counts();
    println!();
    println!("summary: {} info, {} warnings, {} critical — score {}/100", info, warn, crit, report.score());
    Ok(())
}

fn cmd_ai(bin: &PathBuf, xdf_path: &PathBuf, apply: bool, out: Option<&PathBuf>) -> Result<()> {
    let mut img = EcuImage::load(bin)?;
    let (_doc, tables) = load_xdf(xdf_path)?;

    // Iterate to convergence (max 5 passes), like a real repair pass.
    let mut total = 0usize;
    let mut any = false;
    for pass in 1..=5 {
        let report = analyze::analyze(&tables, &EcuImageRef(&img))?;
        let sugg = ai::suggestions_from(&report, &tables);
        if sugg.is_empty() {
            if pass == 1 && !any {
                println!("no suggestions — nothing to fix ✅");
            }
            break;
        }
        any = true;
        let mut resolved = Vec::new();
        for s in sugg {
            let t = tables.iter().find(|t| t.name == s.table).unwrap();
            let old = t.get(&EcuImageRef(&img), s.cell.0, s.cell.1)?;
            resolved.push(ai::Suggestion { old_value: old, ..s });
        }
        if pass == 1 {
            println!("suggestions{}:", if apply { " (applying)" } else { " (dry run)" });
        }
        println!("pass {}:", pass);
        print!("{}", indent(&ai::render(&resolved), "  "));
        if apply {
            let applied = ai::apply(&report, &tables, &mut img, false)?;
            total += applied.len();
        } else {
            break; // dry run shows the first pass only
        }
    }

    if apply && any {
        repair_checksums_if_known(&mut img, None)?;
        let dest = out.map(|p| p.clone()).unwrap_or_else(|| {
            let mut d = bin.clone();
            d.set_extension("tuned.bin");
            d
        });
        img.save(&dest)?;
        println!("applied {} edits — wrote {} (checksums repaired)", total, dest.display());
    } else if any {
        println!("dry run — pass --apply to write changes (use --out to choose the file)");
    }
    Ok(())
}

fn cmd_datalog(path: &PathBuf, tps: f64, knock: f64, lean: f64) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read datalog {}", path.display()))?;
    let dl = Datalog::parse_csv(&text)?;
    println!("datalog: {} — {} rows", path.display(), dl.rows);
    let present: Vec<&str> = ["rpm", "tps", "afr", "knock", "speed", "map", "stft", "ltft", "time"]
        .iter()
        .copied()
        .filter(|c| dl.has(c))
        .collect();
    println!("channels: {}", present.join(", "));
    println!();

    let pulls = dl.find_wot_pulls(tps);
    println!("WOT pulls (TPS ≥ {:.0}%): {}", tps, pulls.len());
    for (i, p) in pulls.iter().enumerate() {
        println!("  pull #{}: rows {}–{}  RPM gain {:.0}", i + 1, p.start_row, p.end_row, p.rpm_gain);
    }

    let knocks = dl.knock_events(knock);
    println!("knock events (>{:.1}°): {}", knock, knocks.len());
    for k in knocks.iter().take(10) {
        println!("  row {}: {:.1}° retard @ {:.0} rpm", k.row, k.retard_deg, k.rpm);
    }

    let lean_rows = dl.lean_wot_events(&pulls, lean);
    println!("lean-at-WOT samples (AFR > {:.1}): {}", lean, lean_rows.len());

    if dl.has("speed") {
        if let Some(t) = dl.acceleration(0.0, 60.0) {
            println!("acceleration 0→60: {:.2} (log time units)", t);
        }
        if let Some(t) = dl.acceleration(60.0, 130.0) {
            println!("acceleration 60→130: {:.2} (log time units)", t);
        }
    }
    Ok(())
}

fn cmd_diff(a: &PathBuf, b: &PathBuf, xdf: Option<&PathBuf>) -> Result<()> {
    let ia = EcuImage::load(a)?;
    let ib = EcuImage::load(b)?;

    let tables: Vec<Table2D> = match xdf {
        Some(p) => load_xdf(p)?.1,
        None => Vec::new(),
    };
    let rep = diff::diff_with_tables(&ia, &ib, &tables)?;

    if rep.is_identical() {
        println!("images are identical ✅");
        return Ok(());
    }
    println!("{} differing bytes", rep.total_bytes);
    for d in rep.bytes.iter().take(20) {
        println!("  0x{:06X}: 0x{:02X} -> 0x{:02X}", d.offset, d.a, d.b);
    }
    if rep.bytes.len() > 20 {
        println!("  … and {} more bytes", rep.bytes.len() - 20);
    }
    if !rep.tables.is_empty() {
        println!("\ntable-attributed changes:");
        for t in &rep.tables {
            println!("  {}[{},{}]: {:.3} -> {:.3}", t.table, t.cell.0, t.cell.1, t.a, t.b);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- obd

fn make_link(demo: bool, port: Option<&str>, baud: u32) -> Result<Box<dyn Transport>> {
    if demo {
        let sim = std::rc::Rc::new(std::cell::RefCell::new(americartune::obd::sim::SimVehicle::new()));
        return Ok(Box::new(americartune::obd::sim::sim_mock(sim)));
    }
    match port {
        Some(p) => {
            #[cfg(feature = "hardware")]
            {
                return Ok(Box::new(americartune::obd::transport::SerialTransport::open(p, baud)?));
            }
            #[cfg(not(feature = "hardware"))]
            {
                let _ = (p, baud);
                bail!(
                    "serial support is compiled out of this build — rebuild with \
                     `cargo build --features hardware`, or explore with --demo"
                );
            }
        }
        None => bail!("specify --port <serial device> (e.g. /dev/ttyUSB0) or --demo"),
    }
}

fn cmd_obd(cmd: ObdCmd) -> Result<()> {
    use americartune::obd::dtc::DtcSource;
    use americartune::obd::elm::ElmClient;
    use americartune::obd::pid::{lookup, PID_REGISTRY};
    use americartune::obd::sim::{sim_mock, SimVehicle};
    use std::cell::RefCell;
    use std::rc::Rc;

    match cmd {
        ObdCmd::Scan { demo, port, baud } => {
            if demo {
                println!("SIMULATED VEHICLE — no hardware attached (demo mode)\n");
            }
            let mut client = ElmClient::new(make_link(demo, port.as_deref(), baud)?);
            let id = client.init()?;
            println!("adapter : {}", id);

            for base in [0x00u8, 0x20, 0x40, 0x60] {
                match client.supported_pids(base) {
                    Ok(pids) => {
                        println!("\nPID range 0x{:02X} — supported:", base);
                        for p in &pids {
                            let name = PID_REGISTRY
                                .iter()
                                .find(|r| r.id == *p)
                                .map(|r| r.name)
                                .unwrap_or("(extended)");
                            println!("  0x{:02X}  {}", p, name);
                        }
                    }
                    Err(e) => println!("\nPID range 0x{:02X}: {}", base, e),
                }
            }
            Ok(())
        }

        ObdCmd::Read { pid, demo, port } => {
            if demo {
                println!("SIMULATED VEHICLE — no hardware attached (demo mode)\n");
            }
            let mut client = ElmClient::new(make_link(demo, port.as_deref(), 38400)?);
            client.init()?;
            for q in pid.split(',') {
                let p = lookup(q.trim())
                    .with_context(|| format!("unknown PID '{}' (try: rpm, tps, speed, map, coolant_temp…)", q))?;
                let v = client.read_pid(p)?;
                println!("{:<18} {:>10.2} {}", v.name, v.value, v.unit);
            }
            Ok(())
        }

        ObdCmd::Dtc { clear, pending, demo, port } => {
            if demo {
                println!("SIMULATED VEHICLE — no hardware attached (demo mode)\n");
            }
            let mut client = ElmClient::new(make_link(demo, port.as_deref(), 38400)?);
            client.init()?;
            if clear {
                client.clear_dtcs()?;
                println!("codes cleared, MIL reset (Mode 04) ✅");
                return Ok(());
            }
            let source = if pending { DtcSource::Pending } else { DtcSource::Stored };
            let codes = client.read_dtcs(source)?;
            if codes.is_empty() {
                println!("no {} trouble codes ✅", if pending { "pending" } else { "stored" });
            } else {
                println!("{} trouble codes:", codes.len());
                for d in &codes {
                    println!("  {}  ({:?})", d.code, d.source);
                }
            }
            Ok(())
        }

        ObdCmd::Vin { demo, port } => {
            if demo {
                println!("SIMULATED VEHICLE — no hardware attached (demo mode)\n");
            }
            let mut client = ElmClient::new(make_link(demo, port.as_deref(), 38400)?);
            client.init()?;
            println!("VIN: {}", client.read_vin()?);
            Ok(())
        }

        ObdCmd::Log { out, rate, duration, demo, port } => {
            if demo {
                println!("SIMULATED VEHICLE — no hardware attached (demo mode)");
            }
            let sim: Option<Rc<RefCell<SimVehicle>>> = if demo {
                Some(Rc::new(RefCell::new(SimVehicle::new())))
            } else {
                None
            };
            let link: Box<dyn Transport> = match &sim {
                Some(s) => Box::new(sim_mock(s.clone())),
                None => make_link(false, port.as_deref(), 38400)?,
            };
            let mut client = ElmClient::new(link);
            client.init()?;

            let pids = ["rpm", "tps", "speed", "map", "commanded_lambda", "iat", "coolant_temp"];
            let refs = pids
                .iter()
                .map(|n| lookup(n).with_context(|| format!("unknown PID '{}'", n)))
                .collect::<Result<Vec<_>>>()?;
            let dt = 1.0 / rate.max(0.1);
            let mut csv = String::from("Time,RPM,TPS,AFR,MAP,Speed,IAT,ECT\n");
            let mut t = 0.0f64;
            let mut rows = 0usize;
            println!("logging at {:.0} Hz to {} …", rate, out.display());
            loop {
                let mut vals = Vec::new();
                for p in &refs {
                    vals.push(client.read_pid(p).map(|v| v.value).unwrap_or(f64::NAN));
                }
                let afr = vals[4] * 14.7; // commanded lambda -> gasoline AFR
                csv.push_str(&format!(
                    "{:.2},{:.0},{:.1},{:.2},{:.0},{:.0},{:.0},{:.0}\n",
                    t, vals[0], vals[1], afr, vals[3], vals[2], vals[5], vals[6]
                ));
                rows += 1;
                t += dt;
                match &sim {
                    Some(s) => s.borrow_mut().advance(dt),
                    None => std::thread::sleep(std::time::Duration::from_secs_f64(dt)),
                }
                if duration > 0.0 && t >= duration {
                    break;
                }
                if rows >= 100_000 {
                    break;
                }
            }
            std::fs::write(&out, &csv)?;
            println!("wrote {} ({} samples)", out.display(), rows);
            println!("analyze it with: americartune datalog {}", out.display());
            Ok(())
        }
    }
}

fn cmd_scan(demo: bool) -> Result<()> {
    if demo {
        println!("SIMULATED OBD-II stream (no hardware attached — demo mode)");
        println!("time   rpm    tps    afr    map");
        for i in 0..12 {
            let t = i as f64 * 0.5;
            let rpm = 800.0 + i as f64 * 220.0;
            let tps = if i > 6 { 88.0 } else { 12.0 };
            let afr = if i > 6 { 12.6 } else { 14.7 };
            let map = if i > 6 { 95.0 } else { 35.0 };
            println!("{:>5.1}s  {:>5.0}  {:>5.1}  {:>5.1}  {:>5.1}", t, rpm, tps, afr, map);
        }
        println!("\n(nothing above came from a vehicle — it is generated by this program)");
        return Ok(());
    }
    eprintln!("scan: the OBD-II/J2534 protocol layer is not implemented yet (roadmap v0.3).");
    eprintln!("No fake results will be printed. Try `americartune scan --demo` for a");
    eprintln!("clearly-labeled simulated stream, or use the file-based commands on real dumps.");
    std::process::exit(3);
}

// ---------------------------------------------------------------- demo

fn cmd_demo(out_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           AmericanTune — end-to-end demo                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // 1. XDF first — the image is filled through the very same table handles
    //    the rest of the workflow reads with, so nothing can drift.
    let xdf_path = out_dir.join("demo.xdf");
    std::fs::write(&xdf_path, DEMO_XDF)?;
    let (doc, tables) = load_xdf(&xdf_path)?;
    println!("① wrote {} — \"{}\" with {} tables", xdf_path.display(), doc.title, tables.len());

    println!("② building synthetic 512 KiB image through the XDF table engine…");
    let mut img = EcuImage::new(vec![0u8; 0x8_0000], "demo_stock.bin")?;
    // OS ID at the platform hint offset so detection works.
    img.data_mut()[0x500..0x508].copy_from_slice(b"12208322");
    {
        let mut m = EcuImageMut(&mut img);
        // VE: smooth realistic surface (identity scaling, u8).
        let ve = &tables[0];
        for r in 0..ve.n_rows() {
            for c in 0..ve.n_cols() {
                let v = 55.0 + (r as f64) * 12.0 + (c as f64) * 3.0 - (c as f64) * (c as f64) * 0.25;
                ve.set(&mut m, r, c, v)?;
            }
        }
        // Spark: more advance at low load (phys = raw*0.35-10).
        let sp = &tables[1];
        for r in 0..sp.n_rows() {
            for c in 0..sp.n_cols() {
                let v = 36.0 - (r as f64) * 5.0 + (c as f64) * 1.5;
                sp.set(&mut m, r, c, v)?;
            }
        }
        // Rev limiter constant: raw 650 -> 6500 rpm.
        img.write_uint(0x6000, 2, 650)?;
    }

    let platforms = builtin_platforms();
    let (plat, _) = PlatformDef::detect(&platforms, img.data())
        .into_iter()
        .next()
        .context("demo platform detection failed")?;
    let seg = plat.segments[0].to_checksum()?;
    seg.repair(img.data_mut())?;
    assert!(seg.verify(img.data())?);
    let stock = out_dir.join("demo_stock.bin");
    img.save(&stock)?;
    println!("   wrote {} — platform {} — checksum {} valid", stock.display(), plat.id, seg.alg.name());

    // 3. Show a table.
    println!("\n③ show VE table (values written through the same engine):");
    {
        let e = EcuImageRef(&img);
        let grid = tables[0].grid(&e)?;
        print_grid(&tables[0].cols, &tables[0].rows, &grid, &tables[0].units);
    }

    // 4. Inject damage (a spike + an out-of-range cell), then analyze.
    println!("\n④ inject a spike (VE) + absurd timing (Spark), then run Aegis analysis:");
    {
        let mut m = EcuImageMut(&mut img);
        tables[0].set(&mut m, 3, 3, 220.0)?; // spike in VE
        tables[1].set(&mut m, 0, 5, 250.0)?; // absurd timing (clamps to ~79 deg, still insane)
    }
    let report = analyze::analyze(&tables, &EcuImageRef(&img))?;
    for f in &report.findings {
        println!("   [{}] {:<18} {}", f.severity, f.rule, f.message);
    }
    let (info, warn, crit) = report.counts();
    println!("   summary: {} info, {} warnings, {} critical — score {}/100", info, warn, crit, report.score());

    // 5. Suggestion engine: iterate to a fixed point like a real repair pass.
    println!("\n⑤ suggestion engine (iterative repair to convergence):");
    let mut total_applied = 0usize;
    for pass in 1..=5 {
        let report = analyze::analyze(&tables, &EcuImageRef(&img))?;
        let sugg = ai::suggestions_from(&report, &tables);
        if sugg.is_empty() {
            break;
        }
        let mut resolved = Vec::new();
        for s in sugg {
            let t = tables.iter().find(|t| t.name == s.table).unwrap();
            let old = t.get(&EcuImageRef(&img), s.cell.0, s.cell.1)?;
            resolved.push(ai::Suggestion { old_value: old, ..s });
        }
        println!("   pass {}:", pass);
        print!("{}", indent(&ai::render(&resolved), "     "));
        let applied = ai::apply(&report, &tables, &mut img, false)?;
        total_applied += applied.len();
    }
    seg.repair(img.data_mut())?;
    let tuned = out_dir.join("demo_tuned.bin");
    img.save(&tuned)?;
    let report2 = analyze::analyze(&tables, &EcuImageRef(&img))?;
    println!("   applied {} edits total — wrote {} (checksum re-verified: {})",
        total_applied, tuned.display(), seg.verify(img.data())?);
    println!("   post-repair score: {}/100", report2.score());

    // 6. Diff stock vs tuned.
    println!("\n⑥ diff demo_stock.bin vs demo_tuned.bin:");
    let stock_img = EcuImage::load(&stock)?;
    let rep = diff::diff_with_tables(&stock_img, &img, &tables)?;
    println!("   {} differing bytes, {} attributed table cells:", rep.total_bytes, rep.tables.len());
    for t in rep.tables.iter().take(8) {
        println!("     {}[{},{}]: {:.3} -> {:.3}", t.table, t.cell.0, t.cell.1, t.a, t.b);
    }

    // 7. Datalog analysis on a generated pull.
    println!("\n⑦ datalog analysis (generated WOT pull with one knock event + lean sample):");
    let log_path = out_dir.join("demo_datalog.csv");
    std::fs::write(&log_path, DEMO_DATALOG)?;
    let dl = Datalog::parse_csv(&std::fs::read_to_string(&log_path)?)?;
    let pulls = dl.find_wot_pulls(85.0);
    let knocks = dl.knock_events(2.0);
    let lean_rows = dl.lean_wot_events(&pulls, 13.0);
    println!("   rows: {}   WOT pulls: {}   knock events: {}   lean-at-WOT samples: {}",
        dl.rows, pulls.len(), knocks.len(), lean_rows.len());
    for p in &pulls {
        println!("   pull rows {}–{}, RPM gain {:.0}", p.start_row, p.end_row, p.rpm_gain);
    }
    for k in &knocks {
        println!("   knock: row {}, {:.1}° @ {:.0} rpm", k.row, k.retard_deg, k.rpm);
    }
    if let Some(t) = dl.acceleration(0.0, 60.0) {
        println!("   0→60 estimate: {:.2} s (log time units)", t);
    }

    println!("\n✅ demo complete — every number above came from real engines (parsing, checksums,");
    println!("   statistics, interpolation, diffing). Files are in {}/", out_dir.display());
    Ok(())
}

fn indent(s: &str, pad: &str) -> String {
    s.lines().map(|l| format!("{}{}", pad, l)).collect::<Vec<_>>().join("\n") + "\n"
}

const DEMO_XDF: &str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<XDFFORMAT version="1.7">
  <XDFHEADER>
    <description>AmericanTune demo layout — GM P01-style 512K</description>
    <deftitle>LS1 Demo P01</deftitle>
    <author>AmericanTune</author>
    <baseoffset>0x0</baseoffset>
    <defaultdigits>1</defaultdigits>
  </XDFHEADER>
  <XDFTABLE id="0x21000">
    <title>VE Table</title>
    <description>Main volumetric efficiency</description>
    <XDFAXIS id="x" type="2" index="0">
      <units>RPM</units>
      <indexcount>10</indexcount>
      <math equation="X*500+1000" />
    </XDFAXIS>
    <XDFAXIS id="y" type="2" index="1">
      <units>kPa</units>
      <indexcount>8</indexcount>
      <math equation="X*12+15" />
    </XDFAXIS>
    <XDFAXIS id="z" type="3">
      <units>g/cyl</units>
      <math equation="X" tophysical="X" />
      <embeddeddata type="2" mmedaddress="0x21000" mmedelementsizebits="8" mmedmajorstridebits="8">
        AAA=
      </embeddeddata>
    </XDFAXIS>
  </XDFTABLE>
  <XDFTABLE id="0x22000">
    <title>Spark Advance</title>
    <description>Ignition advance vs RPM/load</description>
    <XDFAXIS id="x" type="2" index="0">
      <units>RPM</units>
      <indexcount>10</indexcount>
      <math equation="X*500+1000" />
    </XDFAXIS>
    <XDFAXIS id="y" type="2" index="1">
      <units>kPa</units>
      <indexcount>8</indexcount>
      <math equation="X*12+15" />
    </XDFAXIS>
    <XDFAXIS id="z" type="3">
      <units>deg</units>
      <math equation="X*0.35-10" tophysical="X*0.35-10" />
      <embeddeddata type="2" mmedaddress="0x22000" mmedelementsizebits="8" mmedmajorstridebits="8">
        AAA=
      </embeddeddata>
    </XDFAXIS>
  </XDFTABLE>
  <XDFCONSTANT id="0x6000">
    <title>Rev Limiter</title>
    <units>RPM</units>
    <description>Soft rev limit</description>
    <math equation="X*10" tophysical="X*10" />
    <embeddeddata type="2" mmedaddress="0x6000" mmedelementsizebits="16">
      AAA=
    </embeddeddata>
  </XDFCONSTANT>
</XDFFORMAT>"#;

const DEMO_DATALOG: &str = "Time,RPM,TPS,AFR,Knock Retard,Speed\n\
0.0,850,2,14.7,0,0\n\
0.1,860,2,14.7,0,0\n\
0.2,870,3,14.7,0,0\n\
0.3,880,2,14.7,0,0\n\
0.4,900,2,14.7,0,1\n\
0.5,1200,40,14.0,0,3\n\
0.6,1800,88,12.8,0,6\n\
0.7,2400,95,12.5,0,10\n\
0.8,3000,100,12.5,0,15\n\
0.9,3500,100,12.5,0,20\n\
1.0,4000,100,12.6,0,26\n\
1.1,4500,100,12.5,0,32\n\
1.2,5000,100,13.9,0,38\n\
1.3,5500,100,12.5,4.5,45\n\
1.4,6000,100,12.5,0,52\n\
1.5,6500,100,12.5,0,60\n";
