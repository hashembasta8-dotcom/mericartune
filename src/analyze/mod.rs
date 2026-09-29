//! Aegis — real calibration safety analyzer.
//!
//! Rule-based + statistical checks over parsed tables. No fake "AI" claims:
//! every finding is produced by a named, deterministic rule with explainable
//! math (neighbor-median outlier detection, Laplacian jaggedness, range bands,
//! timing-at-load heuristics, rev-limit sanity).

use crate::ecu::tables::{EcuImageRef, Table2D};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Warning => write!(f, "WARN"),
            Severity::Critical => write!(f, "CRIT"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub rule: String,
    pub table: String,
    /// (row, col) cell when applicable.
    pub cell: Option<(usize, usize)>,
    pub message: String,
    /// Suggested physical value when the rule can propose one.
    pub suggested: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisReport {
    pub findings: Vec<Finding>,
    pub tables_scanned: usize,
    pub cells_scanned: usize,
}

impl AnalysisReport {
    /// 0–100 health score (100 = clean).
    pub fn score(&self) -> u32 {
        let penalty: u32 = self
            .findings
            .iter()
            .map(|f| match f.severity {
                Severity::Info => 1,
                Severity::Warning => 8,
                Severity::Critical => 30,
            })
            .sum();
        100u32.saturating_sub(penalty)
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for f in &self.findings {
            match f.severity {
                Severity::Info => c.0 += 1,
                Severity::Warning => c.1 += 1,
                Severity::Critical => c.2 += 1,
            }
        }
        c
    }
}

/// Plausibility bands in physical units, keyed by unit/name heuristics.
pub struct Bands {
    pub timing_deg: (f64, f64),
    pub afr: (f64, f64),
    pub generic: (f64, f64),
}

impl Default for Bands {
    fn default() -> Self {
        Self {
            timing_deg: (-15.0, 55.0),
            afr: (8.0, 20.0),
            generic: (-1000.0, 10_000.0),
        }
    }
}

/// Run the full rule set over a set of tables.
pub fn analyze(tables: &[Table2D], img: &EcuImageRef<'_>) -> anyhow::Result<AnalysisReport> {
    analyze_with(tables, img, &Bands::default())
}

pub fn analyze_with(
    tables: &[Table2D],
    img: &EcuImageRef<'_>,
    bands: &Bands,
) -> anyhow::Result<AnalysisReport> {
    let mut findings = Vec::new();
    let mut cells = 0usize;

    for t in tables {
        let grid = t.grid(img)?;
        let rows = t.n_rows();
        let cols = t.n_cols();
        cells += rows * cols;

        // ---- Rule 1: range plausibility --------------------------------
        let band = pick_band(t, bands);
        for r in 0..rows {
            for c in 0..cols {
                let v = grid[r][c];
                if v < band.0 || v > band.1 {
                    findings.push(Finding {
                        severity: Severity::Critical,
                        rule: "range_violation".into(),
                        table: t.name.clone(),
                        cell: Some((r, c)),
                        message: format!(
                            "{}[{},{}] = {:.3} {} outside plausible band [{}, {}]",
                            t.name, r, c, v, t.units, band.0, band.1
                        ),
                        suggested: Some(v.max(band.0).min(band.1)),
                    });
                }
            }
        }

        // ---- Rule 2: outlier spikes (neighbor-median residual) ---------
        for r in 0..rows {
            for c in 0..cols {
                let v = grid[r][c];
                let mut neigh = Vec::new();
                for dr in -1i32..=1 {
                    for dc in -1i32..=1 {
                        if dr == 0 && dc == 0 {
                            continue;
                        }
                        let rr = r as i32 + dr;
                        let cc = c as i32 + dc;
                        if rr >= 0 && (rr as usize) < rows && cc >= 0 && (cc as usize) < cols {
                            neigh.push(grid[rr as usize][cc as usize]);
                        }
                    }
                }
                if neigh.len() < 4 {
                    continue;
                }
                neigh.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = neigh[neigh.len() / 2];
                let residual = (v - median).abs();
                let mad = neigh
                    .iter()
                    .map(|x| (x - median).abs())
                    .sum::<f64>()
                    / neigh.len() as f64;
                // Robust z-score: residual / (1.4826*MAD). Threshold z > 6
                // with a small absolute floor to avoid noise flags.
                let sigma = (mad * 1.4826).max(1e-6);
                let z = residual / sigma;
                let floor = (median.abs() * 0.25).max(1.0);
                if z > 6.0 && residual > floor {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        rule: "outlier_spike".into(),
                        table: t.name.clone(),
                        cell: Some((r, c)),
                        message: format!(
                            "{}[{},{}] = {:.3} spikes vs neighbor median {:.3} (z={:.1})",
                            t.name, r, c, v, median, z
                        ),
                        suggested: Some(median),
                    });
                }
            }
        }

        // ---- Rule 3: jagged maps (Laplacian energy) --------------------
        let mut jagged = 0usize;
        let scale = grid
            .iter()
            .flatten()
            .copied()
            .fold(0.0f64, |a: f64, b| a.max(b.abs()))
            .max(1.0);
        for r in 1..rows.saturating_sub(1) {
            for c in 1..cols.saturating_sub(1) {
                // Second difference on both axes (discrete Laplacian magnitude).
                let lap = (grid[r][c] * 2.0 - grid[r][c - 1] - grid[r][c + 1]).abs()
                    + (grid[r][c] * 2.0 - grid[r - 1][c] - grid[r + 1][c]).abs();
                if lap > 0.5 * scale {
                    jagged += 1;
                }
            }
        }
        let interior = (rows.saturating_sub(2)) * (cols.saturating_sub(2));
        if interior > 0 && jagged * 100 / interior > 15 {
            findings.push(Finding {
                severity: Severity::Warning,
                rule: "jagged_map".into(),
                table: t.name.clone(),
                cell: None,
                message: format!(
                    "{} is jagged: {}/{} interior cells exceed the Laplacian smoothness budget",
                    t.name, jagged, interior
                ),
                suggested: None,
            });
        }

        // ---- Rule 4: spark tables — high advance at high load ----------
        let name_l = t.name.to_ascii_lowercase();
        let is_spark = name_l.contains("spark") || name_l.contains("timing") || name_l.contains("advance");
        if is_spark {
            // Assume the last row is the highest load (row axis ascending).
            for c in 0..cols {
                let v = grid[rows - 1][c];
                if v > 42.0 {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        rule: "timing_high_load".into(),
                        table: t.name.clone(),
                        cell: Some((rows - 1, c)),
                        message: format!(
                            "{}[{},{}] = {:.1} deg at highest load — knock risk without fuel/octane validation",
                            t.name, rows - 1, c, v
                        ),
                        suggested: Some(42.0),
                    });
                }
            }
        }
    }

    Ok(AnalysisReport { findings, tables_scanned: tables.len(), cells_scanned: cells })
}

fn pick_band(t: &Table2D, bands: &Bands) -> (f64, f64) {
    let u = t.units.to_ascii_lowercase();
    let n = t.name.to_ascii_lowercase();
    if u.contains("deg") || n.contains("spark") || n.contains("timing") {
        bands.timing_deg
    } else if u.contains("afr") || u.contains("lambda") || n.contains("afr") {
        bands.afr
    } else {
        bands.generic
    }
}

/// Scan scalar constants for sanity (rev limiters etc.).
pub fn check_constant(title: &str, units: &str, physical: f64) -> Option<Finding> {
    let n = title.to_ascii_lowercase();
    if (n.contains("rev") && n.contains("lim")) || (n.contains("rpm") && n.contains("lim")) {
        if physical > 8500.0 {
            return Some(Finding {
                severity: Severity::Warning,
                rule: "rev_limit_high".into(),
                table: title.to_string(),
                cell: None,
                message: format!("rev limiter = {:.0} {} is aggressive for stock internals", physical, units),
                suggested: Some(7200.0),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu::bin::EcuImage;
    use crate::ecu::tables::{EcuImageMut, Scaling, ValueWidth};

    fn table() -> Table2D {
        Table2D {
            name: "VE Table".into(),
            units: "g/cyl".into(),
            address: 32,
            rows: vec![20.0, 60.0, 100.0],
            cols: vec![1000.0, 2000.0, 3000.0, 4000.0],
            width: ValueWidth::U8,
            scaling: Scaling { factor: 1.0, offset: 0.0 },
            col_stride: 1,
            row_stride: 4,
        }
    }

    #[test]
    fn clean_map_produces_no_findings() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let t = table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 80.0 + (r as f64) * 10.0 + (c as f64) * 2.0).unwrap();
                }
            }
        }
        let rep = analyze(&[t], &EcuImageRef(&img)).unwrap();
        assert!(rep.findings.is_empty(), "unexpected findings: {:?}", rep.findings);
        assert_eq!(rep.score(), 100);
    }

    #[test]
    fn injected_spike_is_detected_with_suggestion() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let t = table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 80.0).unwrap();
                }
            }
            t.set(&mut m, 1, 1, 200.0).unwrap(); // spike
        }
        let rep = analyze(&[t], &EcuImageRef(&img)).unwrap();
        let spike = rep
            .findings
            .iter()
            .find(|f| f.rule == "outlier_spike")
            .expect("spike must be detected");
        assert_eq!(spike.cell, Some((1, 1)));
        assert!(spike.suggested.is_some());
    }

    #[test]
    fn out_of_range_value_is_critical() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let mut t = table();
        t.name = "Spark Advance".into();
        t.units = "deg".into();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 15.0).unwrap();
                }
            }
            t.set(&mut m, 0, 0, 250.0).unwrap(); // absurd timing (way out of band)
        }
        let rep = analyze(&[t], &EcuImageRef(&img)).unwrap();
        assert!(rep
            .findings
            .iter()
            .any(|f| f.rule == "range_violation" && f.severity == Severity::Critical));
    }

    #[test]
    fn high_timing_at_load_flagged() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let mut t = table();
        t.name = "Spark Advance".into();
        t.units = "deg".into();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 20.0).unwrap();
                }
            }
            t.set(&mut m, 2, 1, 50.0).unwrap();
        }
        let rep = analyze(&[t], &EcuImageRef(&img)).unwrap();
        assert!(rep.findings.iter().any(|f| f.rule == "timing_high_load"));
    }

    #[test]
    fn rev_limit_constant_checked() {
        let f = check_constant("Rev Limiter", "RPM", 9000.0).unwrap();
        assert_eq!(f.rule, "rev_limit_high");
        assert!(check_constant("Rev Limiter", "RPM", 6500.0).is_none());
    }
}
