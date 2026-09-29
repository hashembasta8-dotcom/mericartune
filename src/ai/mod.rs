//! Suggestion engine ("AI" command) — turns Aegis findings into concrete,
//! reviewable cell edits. Deterministic and explainable: every suggestion
//! cites the rule that produced it and the before/after values.

use crate::analyze::AnalysisReport;
use crate::ecu::tables::{EcuImageMut, EcuImageRef, Table2D};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub table: String,
    pub cell: (usize, usize),
    pub old_value: f64,
    pub new_value: f64,
    pub reason: String,
}

/// Map findings to concrete cell edits (currently: spikes → neighbor median,
/// range violations → clamped value, high-load timing → capped value).
pub fn suggestions_from(report: &AnalysisReport, tables: &[Table2D]) -> Vec<Suggestion> {
    let mut out = Vec::new();
    for f in &report.findings {
        if let (Some((r, c)), Some(new_v)) = (f.cell, f.suggested) {
            let table = match tables.iter().find(|t| t.name == f.table) {
                Some(t) => t,
                None => continue,
            };
            // Old value is looked up by the caller when applying; store placeholder here.
            out.push(Suggestion {
                table: table.name.clone(),
                cell: (r, c),
                old_value: f64::NAN,
                new_value: new_v,
                reason: f.rule.clone(),
            });
        }
    }
    out
}

/// Resolve old values and (optionally) apply the suggestions to the image.
/// Returns applied suggestions with real before/after values.
pub fn apply(
    report: &AnalysisReport,
    tables: &[Table2D],
    img: &mut crate::ecu::bin::EcuImage,
    dry_run: bool,
) -> Result<Vec<Suggestion>> {
    let mut applied = Vec::new();
    for s in suggestions_from(report, tables) {
        let table = tables
            .iter()
            .find(|t| t.name == s.table)
            .expect("table exists");
        let old = table.get(&EcuImageRef(img), s.cell.0, s.cell.1)?;
        let mut done = Suggestion { old_value: old, ..s };
        if (old - done.new_value).abs() > 1e-9 {
            if !dry_run {
                done.new_value = table.set(
                    &mut EcuImageMut(img),
                    done.cell.0,
                    done.cell.1,
                    done.new_value,
                )?;
            }
            done.reason = done.reason.clone();
            applied.push(done);
        }
    }
    let _ = report;
    Ok(applied)
}

/// Human-readable suggestion list.
pub fn render(sugg: &[Suggestion]) -> String {
    let mut s = String::new();
    for (i, g) in sugg.iter().enumerate() {
        s.push_str(&format!(
            "  {}. {}[{},{}]: {:.3} -> {:.3}   ({})\n",
            i + 1,
            g.table,
            g.cell.0,
            g.cell.1,
            g.old_value,
            g.new_value,
            g.reason
        ));
    }
    s
}

/// Convenience: run analyze + suggestions in one call.
pub fn propose(
    tables: &[Table2D],
    img: &crate::ecu::bin::EcuImage,
) -> Result<(AnalysisReport, Vec<Suggestion>)> {
    let report = crate::analyze::analyze(tables, &EcuImageRef(img))?;
    let sugg = suggestions_from(&report, tables);
    Ok((report, sugg))
}

/// Public re-export for CLI rendering.
pub use crate::analyze::Severity;

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
    fn suggestions_fix_an_injected_spike() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let t = table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 80.0).unwrap();
                }
            }
            t.set(&mut m, 1, 1, 200.0).unwrap();
        }
        let report = crate::analyze::analyze(&[t.clone()], &EcuImageRef(&img)).unwrap();
        assert!(!report.findings.is_empty());

        let before = img.clone();
        let applied = apply(&report, &[t.clone()], &mut img, false).unwrap();
        assert!(!applied.is_empty());
        assert_ne!(before, img);

        // Re-analyze: the spike must be gone.
        let report2 = crate::analyze::analyze(&[t], &EcuImageRef(&img)).unwrap();
        assert!(
            !report2.findings.iter().any(|f| f.rule == "outlier_spike"),
            "spike should be fixed, still found: {:?}",
            report2.findings
        );
    }

    #[test]
    fn dry_run_leaves_image_untouched() {
        let mut img = EcuImage::new(vec![0u8; 128], "t").unwrap();
        let t = table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 80.0).unwrap();
                }
            }
            t.set(&mut m, 1, 1, 200.0).unwrap();
        }
        let before = img.clone();
        let report = crate::analyze::analyze(&[t.clone()], &EcuImageRef(&img)).unwrap();
        let _ = apply(&report, &[t], &mut img, true).unwrap();
        assert_eq!(before, img);
    }
}
