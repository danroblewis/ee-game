//! Validate a hand-built room file before it goes anywhere near the server.
use sim_core::{ElementKind as K, ElementSpec, Engine};

#[test]
#[ignore = "operator tool: cargo test -p sim-golden --test roomfix -- --ignored --nocapture"]
fn check_the_fixed_room() {
    let raw = std::fs::read_to_string("/tmp/D4SWHQ-fixed.json").unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let els: Vec<ElementSpec> = serde_json::from_value(v["elements"].clone()).unwrap();
    println!("elements: {}", els.len());
    println!("gate: {:?}", sim_core::check_document(&els, 20e-6));

    let mut eng = Engine::new(20e-6);
    eng.set_elements(&els);
    let spk = els.iter().find(|e| matches!(e.kind, K::Speaker { .. })).unwrap();
    let tap = eng.tap(spk.id).unwrap();
    eng.advance(60_000);
    let (mut hi, mut lo) = (f64::MIN, f64::MAX);
    for _ in 0..40_000 {
        eng.advance(1);
        let v = eng.tap_delta(tap, 0, 1);
        hi = hi.max(v);
        lo = lo.min(v);
    }
    println!("quarantined: {}", eng.is_quarantined());
    println!("speaker swing: {:.4} V   ({lo:.4} .. {hi:.4})", hi - lo);

    // WHAT IS COOKING? The damage model judges dissipated power, and the
    // live server broke this chip while an identical offline run (which does
    // not run the damage model) looked fine.
    let mut worst: Vec<(f64, u32, &'static str)> = Vec::new();
    let mut peak: std::collections::BTreeMap<u32, f64> = Default::default();
    for _ in 0..5_000 {
        eng.advance(1);
        for f in eng.frame() {
            let e = peak.entry(f.id).or_insert(0.0);
            if f.power > *e { *e = f.power; }
        }
    }
    for (id, p) in peak {
        let k = els.iter().find(|e| e.id == id).map(|e| e.kind.tag()).unwrap_or("?");
        worst.push((p, id, k));
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("\nhottest parts (peak dissipation):");
    for (p, id, k) in worst.iter().take(8) {
        println!("   #{id:<9} {k:<12} {p:.4} W");
    }
}
