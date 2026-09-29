//! OBD-II pipeline integration test: simulated vehicle → ELM327 engine →
//! CSV datalog → analysis. The same code path `americartune obd log` uses.

use americartune::datalog::Datalog;
use americartune::obd::dtc::DtcSource;
use americartune::obd::elm::ElmClient;
use americartune::obd::pid::lookup;
use americartune::obd::sim::{sim_mock, SimVehicle};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn sim_drive_to_analysis_pipeline() {
    let sim = Rc::new(RefCell::new(SimVehicle::new()));
    let mut client = ElmClient::new(sim_mock(sim.clone()));
    client.init().unwrap();

    // Record the drive at 10 Hz for 12 s through the real protocol engine.
    let pids = ["rpm", "tps", "speed", "map", "commanded_lambda", "iat", "coolant_temp"];
    let refs: Vec<_> = pids.iter().map(|n| lookup(n).unwrap()).collect();
    let dt = 0.1;
    let mut csv = String::from("Time,RPM,TPS,AFR,MAP,Speed,IAT,ECT\n");
    for i in 0..120usize {
        let t = i as f64 * dt;
        let vals: Vec<f64> = refs
            .iter()
            .map(|p| client.read_pid(p).unwrap().value)
            .collect();
        let afr = vals[4] * 14.7;
        csv.push_str(&format!(
            "{:.2},{:.0},{:.1},{:.2},{:.0},{:.0},{:.0},{:.0}\n",
            t, vals[0], vals[1], afr, vals[3], vals[2], vals[5], vals[6]
        ));
        sim.borrow_mut().advance(dt);
    }

    // Analyze exactly like `americartune datalog`.
    let dl = Datalog::parse_csv(&csv).unwrap();
    assert_eq!(dl.rows, 120);

    let pulls = dl.find_wot_pulls(85.0);
    assert_eq!(pulls.len(), 1, "the sim drive has exactly one WOT pull");
    // WOT starts at t=2.0s → row 20 at 10 Hz.
    assert_eq!(pulls[0].start_row, 20);
    assert!((pulls[0].rpm_gain - 3500.0).abs() < 150.0, "programmed RPM gain is 3500");

    // Commanded AFR is rich at WOT (lambda 0.85 → ~12.5) — no lean flags.
    assert!(dl.lean_wot_events(&pulls, 13.0).is_empty());
    // No knock channel in J1979 — none fabricated.
    assert!(dl.knock_events(2.0).is_empty());

    let t060 = dl.acceleration(0.0, 60.0).unwrap();
    assert!(t060 > 1.5 && t060 < 3.0, "0-60 km/h in ~2s of sim time, got {}", t060);
}

#[test]
fn dtc_and_vin_through_the_engine() {
    let sim = Rc::new(RefCell::new(SimVehicle::new()));
    let mut client = ElmClient::new(sim_mock(sim));
    client.init().unwrap();

    let codes = client.read_dtcs(DtcSource::Stored).unwrap();
    assert_eq!(codes.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(), vec!["P0301", "P2445"]);

    assert_eq!(client.read_vin().unwrap(), "1FAFP404X4F123456");

    client.clear_dtcs().unwrap();
}
