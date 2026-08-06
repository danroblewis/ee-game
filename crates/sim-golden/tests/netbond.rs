//! NAMED NETS CONNECT. A label is a connection, not a caption.
//!
//! This is what a net name means on paper and in every schematic tool: put
//! "+5V" on two points and they are one node. It is also the whole reason to
//! have them — a supply dragged across a schematic on wires is exactly the
//! mess a name is supposed to replace.

use sim_core::{ElementKind as K, Engine, Point, Wave};
use sim_golden::*;

const DT: f64 = 20e-6;

/// A 5 V source on the left, a lone resistor to ground on the right, and
/// nothing between them but a NAME on each side.
fn split_room() -> Vec<sim_core::ElementSpec> {
    vec![
        spec(1, K::VoltageSource { dc: 5.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }, (0, 0), (0, 8)),
        gnd(2, (0, 8)),
        // The load, twenty units away, wired to nothing on its top end.
        spec(3, r(1_000.0), (20, 0), (20, 8)),
        gnd(4, (20, 8)),
    ]
}

#[test]
fn two_labels_with_one_name_are_one_node() {
    let d = split_room();
    let labels = vec![("+5V".to_string(), (0, 0)), ("+5v".to_string(), (20, 0))];
    let bonds = sim_core::net_bonds(&labels);
    assert_eq!(bonds.len(), 1, "one pair, one bond: {bonds:?}");

    // WITHOUT the bond the load sees nothing.
    let mut off = Engine::new(DT);
    off.set_elements(&d);
    off.advance(2_000);
    let unbonded = off.voltage_at((20, 0)).unwrap();

    // WITH it, the same point is the supply.
    let mut on = Engine::new(DT);
    on.set_bonds(&bonds);
    on.set_elements(&d);
    on.advance(2_000);
    let bonded = on.voltage_at((20, 0)).unwrap();

    println!("  load top: {unbonded:.4} V unlabelled, {bonded:.4} V once both are '+5V'");
    assert!(unbonded.abs() < 1e-9, "unbonded, the load is dead: {unbonded}");
    assert!((bonded - 5.0).abs() < 1e-6, "bonded, it is on the supply: {bonded}");
    // ...and current actually flows through it.
    let i = bonded / 1_000.0;
    assert!((i - 5e-3).abs() < 1e-6, "5 mA should flow, got {i}");
}

/// Case and surrounding space do not make a different net. "+5V" and " +5v "
/// are the same wire to everybody except a string comparison.
#[test]
fn names_match_case_insensitively_and_ignore_padding() {
    let b = sim_core::net_bonds(&[
        ("+5V".into(), (0, 0)),
        ("  +5v  ".into(), (20, 0)),
        ("GND_A".into(), (5, 5)),
    ]);
    assert_eq!(b.len(), 1, "only the two +5V anchors bond: {b:?}");
}

/// A blank name is somebody mid-type, not an instruction to short two points
/// together. Getting this wrong would connect every unnamed label in a room
/// the moment one was placed.
#[test]
fn blank_names_bond_nothing() {
    let b = sim_core::net_bonds(&[("".into(), (0, 0)), ("   ".into(), (20, 0))]);
    assert!(b.is_empty(), "blank names must bond nothing, got {b:?}");
}

/// Three labels on one name are one node, not three pairs.
#[test]
fn three_labels_make_one_net() {
    let pts: Vec<Point> = vec![(0, 0), (20, 0), (40, 0)];
    let b = sim_core::net_bonds(&pts.iter().map(|p| ("VCC".to_string(), *p)).collect::<Vec<_>>());
    assert_eq!(b.len(), 2, "n labels need n-1 bonds, not n^2: {b:?}");

    let mut d = split_room();
    d.push(spec(5, r(1_000.0), (40, 0), (40, 8)));
    d.push(gnd(6, (40, 8)));
    let mut eng = Engine::new(DT);
    eng.set_bonds(&b);
    eng.set_elements(&d);
    eng.advance(2_000);
    for p in [(20, 0), (40, 0)] {
        assert!(
            (eng.voltage_at(p).unwrap() - 5.0).abs() < 1e-6,
            "{p:?} should be on the supply"
        );
    }
}

/// A label naming a point no part occupies is a DANGLING label, not a broken
/// room. It must be ignored rather than refused — a player deletes a part
/// and its label outlives it by a moment.
#[test]
fn a_label_on_nothing_is_harmless() {
    let d = split_room();
    let bonds = sim_core::net_bonds(&[("X".into(), (0, 0)), ("X".into(), (999, 999))]);
    assert_eq!(sim_core::check_document_bonded(&d, DT, &bonds), Ok(()));
    let mut eng = Engine::new(DT);
    eng.set_bonds(&bonds);
    eng.set_elements(&d);
    eng.advance(1_000);
    assert!(!eng.is_quarantined());
}
