#![cfg(test)]
#[test]
fn a_floating_vcc_latches_a_logic_chip() {
    let mut r = crate::e2e::Room::file("../../rooms/D4SWHQ.json");
    let mut eng = sim_core::Engine::new(crate::DT);
    eng.set_elements(&r.elements);
    let mut peak = std::collections::BTreeMap::new();
    for _ in 0..30 {
        eng.advance(2_000);
        for f in eng.frame() {
            let e = peak.entry(f.id).or_insert(0.0f64);
            if f.power > *e { *e = f.power; }
        }
    }
    for id in [91u32, 92, 93] {
        // VCC pin voltage and dissipation for each mux.
        let v = r.elements.iter().find(|e| e.id == id).map(|e| e.pins[0]).unwrap();
        println!(
            "  mux #{id}: VCC {:.3} V   peak {:.4} W",
            eng.voltage_at(v).unwrap_or(f64::NAN),
            peak.get(&id).copied().unwrap_or(0.0)
        );
    }
    let _ = &mut r;
}
