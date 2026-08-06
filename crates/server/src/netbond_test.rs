//! A NAME IS A CONNECTION, through the server's own path.
//!
//! `sim-core` proves the mechanism; this proves the wiring — that the bonds
//! the room's labels imply are what the gate judges and what the engine
//! runs, using the same helpers the tick uses.
#![cfg(test)]

use sim_core::{ElementKind as K, ElementSpec, Wave};

fn spec(id: u32, kind: K, a: (i32, i32), b: (i32, i32)) -> ElementSpec {
    ElementSpec { id, kind, pins: vec![a, b], ..Default::default() }
}

/// A supply on the left, an orphan load on the right, nothing between them.
fn split() -> Vec<ElementSpec> {
    vec![
        spec(1, K::VoltageSource { dc: 5.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }, (0, 0), (0, 8)),
        ElementSpec { id: 2, kind: K::Ground, pins: vec![(0, 8)], ..Default::default() },
        spec(3, K::Resistor { ohms: 1_000.0 }, (20, 0), (20, 8)),
        ElementSpec { id: 4, kind: K::Ground, pins: vec![(20, 8)], ..Default::default() },
    ]
}

#[test]
fn naming_two_points_the_same_joins_them_and_the_gate_agrees() {
    let els = split();
    // What the room's labels would produce, through the same helper the tick
    // calls — not a hand-built list that could drift from it.
    let labels = vec![("+5V".to_string(), (0, 0)), ("+5v ".to_string(), (20, 0))];
    let bonds = sim_core::net_bonds(&labels);

    // The GATE must judge the bonded circuit, or it refuses edits that are
    // fine and accepts ones that are not.
    assert_eq!(sim_core::check_document_bonded(&els, crate::DT, &bonds), Ok(()));

    let mut eng = sim_core::Engine::new(crate::DT);
    eng.set_bonds(&bonds);
    eng.set_elements(&els);
    eng.advance(2_000);
    let v = eng.voltage_at((20, 0)).unwrap();
    println!("  load top with both points named '+5V': {v:.4} V");
    assert!((v - 5.0).abs() < 1e-6, "the named net must carry the supply, got {v}");

    // Un-name one of them and they part again — a name is a connection you
    // can also take away.
    let mut eng2 = sim_core::Engine::new(crate::DT);
    eng2.set_bonds(&sim_core::net_bonds(&[("+5V".to_string(), (0, 0))]));
    eng2.set_elements(&els);
    eng2.advance(2_000);
    let v2 = eng2.voltage_at((20, 0)).unwrap();
    println!("  and with only one named:                 {v2:.4} V");
    assert!(v2.abs() < 1e-9, "one label alone joins nothing, got {v2}");
}

/// The TR-808's supply, tidied the way Daniel wanted: name the rail, name
/// the chip's VCC pin, delete the wire between them. The room must behave
/// exactly as it shipped.
#[test]
fn a_named_supply_replaces_the_wire_that_dragged_it_around() {
    let r = crate::e2e::Room::template("tr-808");
    let vcc = r
        .elements
        .iter()
        .find(|e| matches!(e.kind, K::Mux { .. }))
        .map(|e| e.pins[0])
        .unwrap();
    let rail = r
        .elements
        .iter()
        .find(|e| e.id == crate::tr808::ID_LOGIC_RAIL)
        .map(|e| e.pins[0])
        .unwrap();
    // Cut the wire that fed this pin, and replace it with a NAME on both ends.
    let mut d: Vec<ElementSpec> = r.elements.clone();
    d.retain(|e| !(matches!(e.kind, K::Wire) && e.pins.contains(&vcc)));
    let bonds = sim_core::net_bonds(&[("+5V".to_string(), rail), ("+5V".to_string(), vcc)]);
    assert_eq!(sim_core::check_document_bonded(&d, crate::DT, &bonds), Ok(()));

    let mut eng = sim_core::Engine::new(crate::DT);
    eng.set_bonds(&bonds);
    eng.set_elements(&d);
    eng.advance(20_000);
    let v = eng.voltage_at(vcc).unwrap();
    println!("  mux VCC, fed only by the name '+5V': {v:.4} V");
    assert!(!eng.is_quarantined());
    assert!(
        (v - 5.0).abs() < 1e-3,
        "the mux should be on the 5 V logic rail through the name alone, got {v}"
    );
}
