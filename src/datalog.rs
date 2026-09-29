//! Real CSV datalog analysis: WOT pulls, knock events, lean conditions,
//! acceleration estimates. Understands HP Tuners / TunerStudio-style logs via
//! header alias mapping and delimiter sniffing.

use anyhow::{bail, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Datalog {
    pub columns: HashMap<String, Vec<f64>>,
    pub rows: usize,
}

/// Canonical channel names we understand, with common aliases.
const ALIASES: &[(&str, &[&str])] = &[
    ("rpm", &["rpm", "engine rpm", "enginerpm", "n", "speed (rpm)"]),
    ("tps", &["tps", "throttle position", "throttleposition", "throttle %", "etctps", "accel pedal"]),
    ("afr", &["afr", "air fuel ratio", "airfuelratio", "wideband", "lambda afr", "eq ratio"]),
    ("knock", &["knock retard", "knockretard", "kr", "knock", "retard", "ignition retard"]),
    ("speed", &["speed", "vss", "vehicle speed", "vehiclespeed"]),
    ("map", &["map", "manifold pressure", "boost", "boost pressure"]),
    ("iat", &["iat", "intake temp", "intake air temp"]),
    ("ect", &["ect", "coolant", "engine coolant temp", "coolant temp"]),
    ("stft", &["stft", "short term fuel trim", "shortft", "fuel trim bank 1"]),
    ("ltft", &["ltft", "long term fuel trim", "longft"]),
];

impl Datalog {
    /// Parse a CSV/TSV/semicolon-separated datalog with a header row.
    pub fn parse_csv(text: &str) -> Result<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';') || l.contains(','))
            .collect();
        let header_line = *lines
            .first()
            .ok_or_else(|| anyhow::anyhow!("datalog is empty"))?;

        // Delimiter sniffing: prefer the one yielding the most fields.
        let delim = [',', '\t', ';']
            .into_iter()
            .max_by_key(|d| header_line.split(*d).count())
            .unwrap_or(',');

        let headers: Vec<String> = header_line
            .split(delim)
            .map(|h| h.trim().trim_matches('"').to_string())
            .collect();
        if headers.len() < 2 {
            bail!("datalog header has fewer than 2 columns");
        }

        let mut columns: HashMap<String, Vec<f64>> = headers
            .iter()
            .map(|h| (canonical(h), Vec::new()))
            .collect::<HashMap<_, _>>();

        // Keep raw names too (first-wins per canonical name).
        let mut raw_names: HashMap<String, String> = HashMap::new();
        for h in &headers {
            raw_names.entry(canonical(h)).or_insert_with(|| h.clone());
        }

        let mut rows = 0usize;
        for line in &lines[1..] {
            let cells: Vec<&str> = line.split(delim).collect();
            if cells.len() != headers.len() {
                continue; // tolerate ragged rows
            }
            for (h, cell) in headers.iter().zip(cells.iter()) {
                let v: f64 = cell.trim().trim_matches('"').parse().unwrap_or(f64::NAN);
                if let Some(col) = columns.get_mut(&canonical(h)) {
                    col.push(v);
                }
            }
            rows += 1;
        }
        if rows == 0 {
            bail!("datalog contains no data rows");
        }
        Ok(Self { columns, rows })
    }

    pub fn has(&self, name: &str) -> bool {
        self.columns.contains_key(name) && self.columns[name].iter().any(|v| v.is_finite())
    }

    pub fn col(&self, name: &str) -> Option<&Vec<f64>> {
        self.columns.get(name)
    }

    /// Detect WOT pulls: TPS ≥ `tps_threshold` sustained while RPM climbs.
    pub fn find_wot_pulls(&self, tps_threshold: f64) -> Vec<Pull> {
        let mut pulls = Vec::new();
        let (tps, rpm) = match (self.col("tps"), self.col("rpm")) {
            (Some(t), Some(r)) => (t, r),
            _ => return pulls,
        };
        let n = tps.len().min(rpm.len());
        let mut start: Option<usize> = None;
        for i in 0..n {
            let wide = tps[i].is_finite() && tps[i] >= tps_threshold;
            if wide && start.is_none() {
                start = Some(i);
            } else if !wide && start.is_some() {
                let s = start.unwrap();
                if i > s + 3 {
                    let rpm_gain = max_finite(&rpm[s..i]) - min_finite(&rpm[s..i]);
                    if rpm_gain >= 500.0 {
                        pulls.push(Pull { start_row: s, end_row: i, rpm_gain });
                    }
                }
                start = None;
            }
        }
        if let Some(s) = start {
            if n > s + 3 {
                let rpm_gain = max_finite(&rpm[s..n]) - min_finite(&rpm[s..n]);
                if rpm_gain >= 500.0 {
                    pulls.push(Pull { start_row: s, end_row: n, rpm_gain });
                }
            }
        }
        pulls
    }

    /// Knock events: retard values above `threshold_deg`.
    pub fn knock_events(&self, threshold_deg: f64) -> Vec<KnockEvent> {
        let mut out = Vec::new();
        if let Some(kr) = self.col("knock") {
            for (i, &v) in kr.iter().enumerate() {
                if v.is_finite() && v > threshold_deg {
                    let rpm = self.col("rpm").map(|r| r.get(i).copied().unwrap_or(f64::NAN));
                    out.push(KnockEvent { row: i, retard_deg: v, rpm: rpm.unwrap_or(f64::NAN) });
                }
            }
        }
        out
    }

    /// Lean-at-WOT check: AFR above `lean_limit` during detected pulls.
    pub fn lean_wot_events(&self, pulls: &[Pull], lean_limit: f64) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(afr) = self.col("afr") {
            for p in pulls {
                for i in p.start_row..p.end_row.min(afr.len()) {
                    if afr[i].is_finite() && afr[i] > lean_limit {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    /// Acceleration estimates (seconds) between speed thresholds, via linear
    /// interpolation on the speed channel. Speed unit follows the log (mph or km/h).
    pub fn acceleration(&self, from: f64, to: f64) -> Option<f64> {
        let speed = self.col("speed")?;
        let t = self.col("rpm")?; // time proxy unavailable; use row index as sample time
        let _ = t;
        // Assume fixed sample interval unknown — compute row-crossings and let
        // the caller scale; but many logs have a time column. Try canonical "time".
        let dt = self.columns.get("time").map(|c| c.as_slice());
        let t_of = |i: usize| -> f64 {
            match dt {
                Some(tcol) if tcol.len() > i && tcol[i].is_finite() => tcol[i],
                _ => i as f64, // row-index fallback (units: samples)
            }
        };
        let (i0, f0) = crossing(speed, from)?;
        let (i1, f1) = crossing_after(speed, from, to, i0)?;
        let t0 = t_of(i0) + (t_of(i0 + 1) - t_of(i0)) * f0;
        let t1 = t_of(i1) + (t_of(i1 + 1) - t_of(i1)) * f1;
        Some(t1 - t0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pull {
    pub start_row: usize,
    pub end_row: usize,
    pub rpm_gain: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct KnockEvent {
    pub row: usize,
    pub retard_deg: f64,
    pub rpm: f64,
}

fn canonical(header: &str) -> String {
    let h = header.trim().to_ascii_lowercase();
    for (canon, aliases) in ALIASES {
        if aliases.iter().any(|a| h == *a) {
            return canon.to_string();
        }
    }
    // time column special-case
    if h.contains("time") && !h.contains("timing") {
        return "time".to_string();
    }
    h.replace(' ', "_")
}

fn min_finite(v: &[f64]) -> f64 {
    v.iter().copied().filter(|x| x.is_finite()).fold(f64::INFINITY, f64::min)
}

fn max_finite(v: &[f64]) -> f64 {
    v.iter().copied().filter(|x| x.is_finite()).fold(f64::NEG_INFINITY, f64::max)
}

/// First index where the series crosses `target` upward, with fraction.
/// A rising crossing counts when `a <= target <= b` with `b > a`.
fn crossing(series: &[f64], target: f64) -> Option<(usize, f64)> {
    for i in 0..series.len().saturating_sub(1) {
        let (a, b) = (series[i], series[i + 1]);
        if a.is_finite() && b.is_finite() && a <= target && b >= target && b > a {
            let frac = if (b - a).abs() < f64::EPSILON { 0.0 } else { (target - a) / (b - a) };
            return Some((i, frac));
        }
    }
    None
}

/// Crossing of `to` after `from` was crossed at `after`.
fn crossing_after(series: &[f64], from: f64, to: f64, after: usize) -> Option<(usize, f64)> {
    for i in after..series.len().saturating_sub(1) {
        let (a, b) = (series[i], series[i + 1]);
        if a.is_finite() && b.is_finite() && a <= to && b >= to && b > a && a >= from {
            let frac = if (b - a).abs() < f64::EPSILON { 0.0 } else { (to - a) / (b - a) };
            return Some((i, frac));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_log() -> String {
        // 2-second idle, then a WOT pull with knock and lean spike.
        let mut s = String::from("Time,RPM,TPS,AFR,Knock Retard,Speed\n");
        for i in 0..20 {
            s.push_str(&format!("{:.1},800,0,14.7,0,0\n", i as f64 * 0.1));
        }
        // WOT pull rows: TPS 100, RPM climbs 3000->6500, one knock event, lean spike
        for i in 20..50 {
            let rpm = 3000.0 + (i - 20) as f64 * 116.0;
            let speed = (i - 20) as f64 * 1.5;
            let afr = if i == 35 { 13.9 } else { 12.5 };
            let kr = if i == 40 { 4.5 } else { 0.0 };
            s.push_str(&format!("{:.1},{:.0},100,{:.1},{:.1},{:.1}\n", i as f64 * 0.1, rpm, afr, kr, speed));
        }
        s
    }

    #[test]
    fn parses_headers_and_aliases() {
        let dl = Datalog::parse_csv(&sample_log()).unwrap();
        assert!(dl.has("rpm"));
        assert!(dl.has("tps"));
        assert!(dl.has("afr"));
        assert!(dl.has("knock"));
        assert!(dl.has("speed"));
        assert_eq!(dl.rows, 50);
    }

    #[test]
    fn detects_the_wot_pull() {
        let dl = Datalog::parse_csv(&sample_log()).unwrap();
        let pulls = dl.find_wot_pulls(85.0);
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].start_row, 20);
        assert!(pulls[0].rpm_gain > 3000.0);
    }

    #[test]
    fn detects_knock_and_lean() {
        let dl = Datalog::parse_csv(&sample_log()).unwrap();
        let pulls = dl.find_wot_pulls(85.0);
        let knocks = dl.knock_events(2.0);
        assert_eq!(knocks.len(), 1);
        assert!((knocks[0].retard_deg - 4.5).abs() < 1e-9);
        assert!((knocks[0].rpm - 5320.0).abs() < 1.0);

        let lean = dl.lean_wot_events(&pulls, 13.0);
        assert_eq!(lean.len(), 1);
    }

    #[test]
    fn acceleration_between_speed_thresholds() {
        let dl = Datalog::parse_csv(&sample_log()).unwrap();
        // Speed rises 1.5 per row from row 20; 10→40 spans exactly 20 rows = 2.0 s.
        let t = dl.acceleration(10.0, 40.0).unwrap();
        assert!((t - 2.0).abs() < 0.05, "expected ~2.0s, got {}", t);
    }

    #[test]
    fn tsv_and_semicolon_supported() {
        let tsv = "RPM\tTPS\n1000\t0\n2000\t50\n3000\t100\n";
        let dl = Datalog::parse_csv(tsv).unwrap();
        assert_eq!(dl.rows, 3);
        let semi = "RPM;TPS\n1000;0\n2000;50\n3000;100\n";
        let dl = Datalog::parse_csv(semi).unwrap();
        assert_eq!(dl.rows, 3);
    }
}
