//! Simulated vehicle for `--demo` and tests: answers ELM327/OBD requests from
//! a deterministic drive profile. Clearly labeled simulation — never presented
//! as real vehicle data.

use std::cell::RefCell;
use std::rc::Rc;

use super::transport::MockTransport;

/// Piecewise drive profile over time (seconds).
#[derive(Debug, Clone, Copy)]
pub struct SimVehicle {
    pub t: f64,
}

impl Default for SimVehicle {
    fn default() -> Self {
        Self::new()
    }
}

impl SimVehicle {
    pub fn new() -> Self {
        Self { t: 0.0 }
    }

    pub fn advance(&mut self, dt: f64) {
        self.t += dt;
    }

    /// Idle → WOT pull → coast, as a function of time.
    pub fn state(&self) -> SimState {
        let t = self.t;
        if t < 2.0 {
            // idle
            SimState {
                rpm: 850.0 + t * 25.0,
                tps: 2.0 + t,
                speed: 0.0,
                map: 35.0,
                ect: 88.0,
                iat: 32.0,
                lambda: 1.0,
            }
        } else if t < 6.0 {
            // WOT pull: 3000 -> 6500 rpm in 4 s
            let f = (t - 2.0) / 4.0;
            SimState {
                rpm: 3000.0 + f * 3500.0,
                tps: 100.0,
                speed: 15.0 + f * 95.0,
                map: 95.0 + f * 5.0,
                ect: 90.0,
                iat: 35.0,
                lambda: 0.85,
            }
        } else {
            // coast down
            let f = ((t - 6.0) / 6.0).min(1.0);
            SimState {
                rpm: 6500.0 - f * 4500.0,
                tps: 0.0,
                speed: 110.0 - f * 70.0,
                map: 30.0,
                ect: 91.0,
                iat: 36.0,
                lambda: 1.0,
            }
        }
    }

    /// Handle one ELM327 command (command text, no CR) → raw reply text.
    pub fn handle(&mut self, cmd: &str) -> String {
        let c = cmd.trim().to_ascii_uppercase();
        match c.as_str() {
            "ATZ" => "ELM327 v1.5".into(),
            "ATE0" | "ATE1" | "ATL0" | "ATL1" | "ATS0" | "ATS1" | "ATH0" | "ATH1" | "ATSP0" => {
                "OK".into()
            }
            "0100" => "4100BE3EA813".into(),
            "0120" => "4120A8130001".into(),
            "0140" => "NO DATA".into(),
            "010C" => {
                let raw = (self.state().rpm * 4.0) as u16;
                format!("410C{:04X}", raw)
            }
            "0111" => {
                let raw = (self.state().tps * 255.0 / 100.0) as u8;
                format!("4111{:02X}", raw)
            }
            "010D" => {
                let raw = self.state().speed as u8;
                format!("410D{:02X}", raw)
            }
            "010B" => {
                let raw = self.state().map as u8;
                format!("410B{:02X}", raw)
            }
            "0105" => {
                let raw = (self.state().ect + 40.0) as u8;
                format!("4105{:02X}", raw)
            }
            "010F" => {
                let raw = (self.state().iat + 40.0) as u8;
                format!("410F{:02X}", raw)
            }
            "0144" => {
                let raw = (self.state().lambda * 32768.0) as u16;
                format!("4144{:04X}", raw)
            }
            "03" => "4303012445".into(), // P0301 + P2445
            "07" => "470301".into(),     // pending P0301
            "04" => "44".into(),
            "0902" => {
                // ISO-TP multi-frame VIN: 1FAFP404X4F123456
                "7E81014490201314641\n7E82146503430345834\n7E82246313233343536".into()
            }
            _ => "?".into(),
        }
    }
}

/// Decoded instantaneous vehicle state.
#[derive(Debug, Clone, Copy)]
pub struct SimState {
    pub rpm: f64,
    pub tps: f64,
    pub speed: f64,
    pub map: f64,
    pub ect: f64,
    pub iat: f64,
    pub lambda: f64,
}

/// Build a MockTransport driven by a shared simulated vehicle.
/// The caller can advance time between sample ticks via the returned handle.
pub fn sim_mock(sim: Rc<RefCell<SimVehicle>>) -> MockTransport {
    MockTransport::new(move |cmd| sim.borrow_mut().handle(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obd::dtc::DtcSource;
    use crate::obd::elm::ElmClient;
    use crate::obd::pid::lookup;

    #[test]
    fn sim_answers_the_full_stack() {
        let sim = Rc::new(RefCell::new(SimVehicle::new()));
        let mut c = ElmClient::new(sim_mock(sim.clone()));
        c.init().unwrap();
        let rpm = c.read_pid(lookup("rpm").unwrap()).unwrap();
        assert!(rpm.value > 800.0 && rpm.value < 900.0);
        sim.borrow_mut().advance(4.0); // mid/late WOT
        let tps = c.read_pid(lookup("tps").unwrap()).unwrap();
        assert!((tps.value - 100.0).abs() < 1.0);
        let codes = c.read_dtcs(DtcSource::Stored).unwrap();
        assert_eq!(codes.len(), 2);
    }

    #[test]
    fn sim_vin_assembles() {
        let sim = Rc::new(RefCell::new(SimVehicle::new()));
        let mut c = ElmClient::new(sim_mock(sim));
        c.init().unwrap();
        assert_eq!(c.read_vin().unwrap(), "1FAFP404X4F123456");
    }
}
