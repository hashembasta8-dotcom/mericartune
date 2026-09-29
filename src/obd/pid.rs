//! SAE J1979 Mode 01 PID registry with the real scaling formulas.

use serde::Serialize;

/// Decoded value of one PID sample.
#[derive(Debug, Clone, Serialize)]
pub struct PidValue {
    pub pid: u8,
    pub name: &'static str,
    pub value: f64,
    pub unit: &'static str,
}

/// One Mode 01 parameter: number, name, unit, byte count, formula.
#[derive(Debug, Clone, Copy)]
pub struct Pid {
    pub id: u8,
    pub name: &'static str,
    pub unit: &'static str,
    /// Expected data bytes (A, B, C, D…).
    pub bytes: usize,
    pub decode: fn(&[u8]) -> Option<f64>,
}

impl Pid {
    pub fn decode(&self, data: &[u8]) -> Option<PidValue> {
        if data.len() < self.bytes {
            return None;
        }
        (self.decode)(data).map(|value| PidValue {
            pid: self.id,
            name: self.name,
            value,
            unit: self.unit,
        })
    }

    /// PID request payload: mode 01, PID number.
    pub fn request(&self) -> String {
        format!("01{:02X}", self.id)
    }
}

fn a(d: &[u8]) -> f64 {
    d[0] as f64
}
fn ab(d: &[u8]) -> f64 {
    ((d[0] as u16) << 8 | d[1] as u16) as f64
}

// Named decoders (fn pointers, usable in a `static`).
fn dec_load(d: &[u8]) -> Option<f64> { Some(a(d) * 100.0 / 255.0) }
fn dec_coolant(d: &[u8]) -> Option<f64> { Some(a(d) - 40.0) }
fn dec_trim(d: &[u8]) -> Option<f64> { Some(a(d) / 1.28 - 100.0) }
fn dec_map(d: &[u8]) -> Option<f64> { Some(a(d)) }
fn dec_rpm(d: &[u8]) -> Option<f64> { Some(ab(d) / 4.0) }
fn dec_speed(d: &[u8]) -> Option<f64> { Some(a(d)) }
fn dec_advance(d: &[u8]) -> Option<f64> { Some(a(d) / 2.0 - 64.0) }
fn dec_temp(d: &[u8]) -> Option<f64> { Some(a(d) - 40.0) }
fn dec_maf(d: &[u8]) -> Option<f64> { Some(ab(d) / 100.0) }
fn dec_tps(d: &[u8]) -> Option<f64> { Some(a(d) * 100.0 / 255.0) }
fn dec_runtime(d: &[u8]) -> Option<f64> { Some(ab(d)) }
fn dec_fuel_level(d: &[u8]) -> Option<f64> { Some(a(d) * 100.0 / 255.0) }
fn dec_baro(d: &[u8]) -> Option<f64> { Some(a(d)) }
fn dec_voltage(d: &[u8]) -> Option<f64> { Some(ab(d) / 1000.0) }
fn dec_lambda(d: &[u8]) -> Option<f64> { Some(ab(d) / 32768.0) }
fn dec_fuel_rate(d: &[u8]) -> Option<f64> { Some(ab(d) / 20.0) }

/// The supported Mode 01 PIDs (formulas straight from SAE J1979).
pub static PID_REGISTRY: &[Pid] = &[
    Pid { id: 0x04, name: "engine_load", unit: "%", bytes: 1, decode: dec_load },
    Pid { id: 0x05, name: "coolant_temp", unit: "degC", bytes: 1, decode: dec_coolant },
    Pid { id: 0x06, name: "stft_b1", unit: "%", bytes: 1, decode: dec_trim },
    Pid { id: 0x07, name: "ltft_b1", unit: "%", bytes: 1, decode: dec_trim },
    Pid { id: 0x0B, name: "map", unit: "kPa", bytes: 1, decode: dec_map },
    Pid { id: 0x0C, name: "rpm", unit: "rpm", bytes: 2, decode: dec_rpm },
    Pid { id: 0x0D, name: "speed", unit: "km/h", bytes: 1, decode: dec_speed },
    Pid { id: 0x0E, name: "timing_advance", unit: "deg", bytes: 1, decode: dec_advance },
    Pid { id: 0x0F, name: "iat", unit: "degC", bytes: 1, decode: dec_temp },
    Pid { id: 0x10, name: "maf", unit: "g/s", bytes: 2, decode: dec_maf },
    Pid { id: 0x11, name: "tps", unit: "%", bytes: 1, decode: dec_tps },
    Pid { id: 0x1F, name: "run_time", unit: "s", bytes: 2, decode: dec_runtime },
    Pid { id: 0x2F, name: "fuel_level", unit: "%", bytes: 1, decode: dec_fuel_level },
    Pid { id: 0x33, name: "baro", unit: "kPa", bytes: 1, decode: dec_baro },
    Pid { id: 0x42, name: "module_voltage", unit: "V", bytes: 2, decode: dec_voltage },
    Pid { id: 0x44, name: "commanded_lambda", unit: "lambda", bytes: 2, decode: dec_lambda },
    Pid { id: 0x5C, name: "oil_temp", unit: "degC", bytes: 1, decode: dec_temp },
    Pid { id: 0x5E, name: "fuel_rate", unit: "L/h", bytes: 2, decode: dec_fuel_rate },
];

/// Look up a PID by hex number ("0C") or name ("rpm").
pub fn lookup(q: &str) -> Option<&'static Pid> {
    let q = q.trim();
    PID_REGISTRY
        .iter()
        .find(|p| p.name == q.to_ascii_lowercase())
        .or_else(|| {
            u8::from_str_radix(q.trim_start_matches("0x").trim_start_matches("0X"), 16)
                .ok()
                .and_then(|id| PID_REGISTRY.iter().find(|p| p.id == id))
        })
}

/// Decode the Mode 01 PID-support bitmask (PIDs 0x00/0x20/0x40/0x60 answers).
/// Returns the supported PID numbers in `range_base..range_base+0x20`.
pub fn supported_from_bitmask(range_base: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if data.len() < 4 {
        return out;
    }
    for (i, &byte) in data[..4].iter().enumerate() {
        for bit in 0..8 {
            if byte & (0x80 >> bit) != 0 {
                out.push(range_base + (i as u8) * 8 + bit as u8 + 1);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_formula_matches_j1979() {
        // 0x1AF8 -> (6904)/4 = 1726 rpm
        let p = lookup("rpm").unwrap();
        let v = p.decode(&[0x1A, 0xF8]).unwrap();
        assert!((v.value - 1726.0).abs() < 1e-9);
    }

    #[test]
    fn speed_and_tps_formulas() {
        assert!((lookup("speed").unwrap().decode(&[0x50]).unwrap().value - 80.0).abs() < 1e-9);
        // 0x80 -> 50.196... %
        let tps = lookup("tps").unwrap().decode(&[0x80]).unwrap().value;
        assert!((tps - 128.0 * 100.0 / 255.0).abs() < 1e-9);
    }

    #[test]
    fn coolant_and_iat_offsets() {
        assert!((lookup("coolant_temp").unwrap().decode(&[0x82]).unwrap().value - 90.0).abs() < 1e-9);
        assert!((lookup("iat").unwrap().decode(&[0x46]).unwrap().value - 30.0).abs() < 1e-9);
    }

    #[test]
    fn lambda_voltage_and_fuel_rate() {
        // 0x8000 -> 1.0 lambda (stoich)
        let l = lookup("commanded_lambda").unwrap().decode(&[0x80, 0x00]).unwrap().value;
        assert!((l - 1.0).abs() < 1e-9);
        // 0x0BB8 -> 3000/1000 = 3.0 V? wait 0x0BB8=3000 -> 3.0 V
        let v = lookup("module_voltage").unwrap().decode(&[0x0B, 0xB8]).unwrap().value;
        assert!((v - 3.0).abs() < 1e-9);
        // 0x01F4 = 500 -> 25 L/h
        let r = lookup("fuel_rate").unwrap().decode(&[0x01, 0xF4]).unwrap().value;
        assert!((r - 25.0).abs() < 1e-9);
    }

    #[test]
    fn short_data_is_rejected() {
        assert!(lookup("rpm").unwrap().decode(&[0x1A]).is_none());
    }

    #[test]
    fn support_bitmask_decodes() {
        // Classic 0x00 answer BE 3E A8 13: A=0xBE -> PIDs 1,3,4,5,6,7,8 set.
        let pids = supported_from_bitmask(0x00, &[0xBE, 0x3E, 0xA8, 0x13]);
        assert!(pids.contains(&0x01));
        assert!(pids.contains(&0x03));
        assert!(!pids.contains(&0x02), "bit 0x40 of 0xBE is clear -> PID 0x02 unsupported");
        assert!(pids.contains(&0x0C), "RPM (bit 13) must be flagged supported");
        assert!(pids.contains(&0x0D), "speed must be flagged supported");
    }

    #[test]
    fn lookup_by_hex_and_name() {
        assert_eq!(lookup("0C").unwrap().name, "rpm");
        assert_eq!(lookup("0x0c").unwrap().name, "rpm");
        assert_eq!(lookup("RPM").unwrap().id, 0x0C);
        assert!(lookup("nope").is_none());
    }
}
