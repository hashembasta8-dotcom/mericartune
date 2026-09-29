//! End-to-end integration test: the complete AmericanTune workflow
//! (identify → checksum → edit → analyze → repair → diff) on one image.

use americartune::analyze;
use americartune::ai;
use americartune::diff;
use americartune::ecu::bin::EcuImage;
use americartune::ecu::platforms::{builtin_platforms, PlatformDef};
use americartune::ecu::tables::{EcuImageMut, EcuImageRef};
use americartune::ecu::xdf::XdfDocument;

const XDF: &str = r#"<?xml version="1.0"?>
<XDFFORMAT version="1.7">
  <XDFHEADER>
    <deftitle>Integration LS1</deftitle>
    <author>AmericanTune</author>
    <baseoffset>0x0</baseoffset>
  </XDFHEADER>
  <XDFTABLE id="0x21000">
    <title>VE Table</title>
    <XDFAXIS id="x"><units>RPM</units><indexcount>8</indexcount><math equation="X*500+1000"/></XDFAXIS>
    <XDFAXIS id="y"><units>kPa</units><indexcount>6</indexcount><math equation="X*20+20"/></XDFAXIS>
    <XDFAXIS id="z">
      <units>g/cyl</units>
      <math equation="X" tophysical="X"/>
      <embeddeddata mmedaddress="0x21000" mmedelementsizebits="8" mmedmajorstridebits="8">AAA=</embeddeddata>
    </XDFAXIS>
  </XDFTABLE>
  <XDFTABLE id="0x22000">
    <title>Spark Advance</title>
    <XDFAXIS id="x"><units>RPM</units><indexcount>8</indexcount><math equation="X*500+1000"/></XDFAXIS>
    <XDFAXIS id="y"><units>kPa</units><indexcount>6</indexcount><math equation="X*20+20"/></XDFAXIS>
    <XDFAXIS id="z">
      <units>deg</units>
      <math equation="X*0.35-10" tophysical="X*0.35-10"/>
      <embeddeddata mmedaddress="0x22000" mmedelementsizebits="8" mmedmajorstridebits="8">AAA=</embeddeddata>
    </XDFAXIS>
  </XDFTABLE>
</XDFFORMAT>"#;

#[test]
fn full_workflow_on_one_image() {
    // ---- build a stock image (OS ID + smooth tables + valid checksum) ----
    let mut img = EcuImage::new(vec![0u8; 0x8_0000], "integration.bin").unwrap();
    img.data_mut()[0x500..0x508].copy_from_slice(b"12208322");

    let doc = XdfDocument::parse(XDF).unwrap();
    let tables = doc.build_tables().unwrap();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].n_rows(), 6);
    assert_eq!(tables[0].n_cols(), 8);

    {
        let mut m = EcuImageMut(&mut img);
        for r in 0..6 {
            for c in 0..8 {
                tables[0].set(&mut m, r, c, 60.0 + r as f64 * 10.0 + c as f64 * 2.0).unwrap();
                tables[1].set(&mut m, r, c, 30.0 - r as f64 * 4.0 + c as f64 * 1.2).unwrap();
            }
        }
    }

    // ---- identify + checksum verify/repair ----
    let platforms = builtin_platforms();
    let hits = PlatformDef::detect(&platforms, img.data());
    assert_eq!(hits[0].0.id, "gm_p01_512k");
    assert_eq!(hits[0].1, vec!["12208322".to_string()]);

    let seg = hits[0].0.segments[0].to_checksum().unwrap();
    assert!(!seg.verify(img.data()).unwrap(), "unrepaired image must fail verification");
    seg.repair(&mut img.data_mut()).unwrap();
    assert!(seg.verify(img.data()).unwrap());

    // ---- baseline analysis is clean ----
    let report = analyze::analyze(&tables, &EcuImageRef(&img)).unwrap();
    assert!(report.findings.is_empty(), "baseline must be clean: {:?}", report.findings);
    assert_eq!(report.score(), 100);

    // ---- edit a cell like a tuner would; checksum is invalidated then repaired ----
    let stock = img.clone();
    let old = tables[0].get(&EcuImageRef(&img), 2, 3).unwrap();
    let new = tables[0].set(&mut EcuImageMut(&mut img), 2, 3, old + 6.0).unwrap();
    assert!(!seg.verify(img.data()).unwrap(), "editing must invalidate the checksum");
    seg.repair(&mut img.data_mut()).unwrap();
    assert!(seg.verify(img.data()).unwrap());

    // ---- diff attributes the change to the exact cell + checksum word ----
    let rep = diff::diff_with_tables(&stock, &img, &tables).unwrap();
    assert_eq!(rep.tables.len(), 1);
    assert_eq!(rep.tables[0].cell, (2, 3));
    assert!((rep.tables[0].b - new).abs() < 1e-9);

    // ---- inject a spike + bad timing; Aegis finds both; the engine repairs them ----
    {
        let mut m = EcuImageMut(&mut img);
        tables[0].set(&mut m, 2, 2, 230.0).unwrap();
        tables[1].set(&mut m, 1, 1, 250.0).unwrap();
    }
    let report = analyze::analyze(&tables, &EcuImageRef(&img)).unwrap();
    assert!(report.findings.iter().any(|f| f.rule == "outlier_spike"));
    assert!(report.findings.iter().any(|f| f.rule == "range_violation"));

    let applied = ai::apply(&report, &tables, &mut img, false).unwrap();
    assert!(applied.len() >= 2);
    seg.repair(&mut img.data_mut()).unwrap();

    // iterate to convergence like `americartune ai --apply`
    for _ in 0..5 {
        let rep = analyze::analyze(&tables, &EcuImageRef(&img)).unwrap();
        let s = ai::suggestions_from(&rep, &tables);
        if s.is_empty() {
            break;
        }
        ai::apply(&rep, &tables, &mut img, false).unwrap();
    }
    let final_report = analyze::analyze(&tables, &EcuImageRef(&img)).unwrap();
    assert!(
        final_report.findings.is_empty(),
        "repair pass must converge to clean: {:?}",
        final_report.findings
    );
    assert_eq!(final_report.score(), 100);
    assert!(seg.verify(img.data()).unwrap(), "final image must carry a valid checksum");
}

#[test]
fn datalog_workflow() {
    use americartune::datalog::Datalog;
    let csv = "Time,RPM,TPS,AFR,Knock Retard,Speed\n\
        0.0,900,0,14.7,0,0\n\
        0.1,905,0,14.7,0,0\n\
        0.2,1500,90,12.8,0,5\n\
        0.3,2500,100,12.5,0,12\n\
        0.4,3500,100,12.5,0,20\n\
        0.5,4500,100,12.4,5.5,29\n\
        0.6,5500,100,13.9,0,39\n\
        0.7,6500,100,12.5,0,50\n";
    let dl = Datalog::parse_csv(csv).unwrap();
    let pulls = dl.find_wot_pulls(85.0);
    assert_eq!(pulls.len(), 1);
    assert!(pulls[0].rpm_gain > 4000.0);
    assert_eq!(dl.knock_events(2.0).len(), 1);
    assert_eq!(dl.lean_wot_events(&pulls, 13.0).len(), 1);
    let t = dl.acceleration(10.0, 40.0).unwrap();
    assert!(t > 0.0);
}
