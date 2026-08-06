//! A LABEL IS A PART. That is the whole design, and these are the two claims
//! that follow from it.
//!
//! It replaced a version that lived in room state beside panels and label
//! boxes. Everything a part gets free — selection, deletion, copy and paste,
//! dragging with a group, rotation, undo, the gate, saving — had to be
//! hand-written there, and each capability only appeared when somebody hit
//! its absence. None of that code exists any more.

use sim_core::{ElementKind as K, ElementSpec, Engine, Wave};
use sim_golden::*;

const DT: f64 = 20e-6;

fn label(id: u32, name: &str, at: (i32, i32)) -> ElementSpec {
    ElementSpec {
        id,
        kind: K::Label,
        pins: vec![at],
        name: name.to_string(),
        ..Default::default()
    }
}

/// A supply on the left, an orphan load twenty units away, and nothing
/// between them but a name on each.
fn split() -> Vec<ElementSpec> {
    vec![
        spec(1, K::VoltageSource { dc: 5.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }, (0, 0), (0, 8)),
        gnd(2, (0, 8)),
        spec(3, r(1_000.0), (20, 0), (20, 8)),
        gnd(4, (20, 8)),
    ]
}

#[test]
fn two_labels_with_one_name_are_one_node() {
    let mut d = split();
    let mut off = Engine::new(DT);
    off.set_elements(&d);
    off.advance(2_000);
    assert!(off.voltage_at((20, 0)).unwrap().abs() < 1e-9, "unlabelled, the load is dead");

    d.push(label(10, "+5V", (0, 0)));
    d.push(label(11, "+5v ", (20, 0))); // case and padding must not matter
    assert_eq!(sim_core::check_document(&d, DT), Ok(()));
    let mut on = Engine::new(DT);
    on.set_elements(&d);
    on.advance(2_000);
    let v = on.voltage_at((20, 0)).unwrap();
    println!("  load top: 0 V unlabelled, {v:.4} V once both carry '+5V'");
    assert!((v - 5.0).abs() < 1e-6, "the named net must carry the supply, got {v}");
}

/// DELETING ONE PARTS THE NET. A label is a connection you can take away,
/// and taking it away is an ordinary `Remove` — which is exactly why delete,
/// undo and the rest needed no code of their own.
#[test]
fn removing_a_label_parts_the_net() {
    let mut d = split();
    d.push(label(10, "+5V", (0, 0)));
    d.push(label(11, "+5V", (20, 0)));
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(2_000);
    assert!((eng.voltage_at((20, 0)).unwrap() - 5.0).abs() < 1e-6);

    d.retain(|e| e.id != 11);
    eng.set_elements(&d);
    eng.advance(2_000);
    assert!(
        eng.voltage_at((20, 0)).unwrap().abs() < 1e-9,
        "one label alone joins nothing"
    );
}

/// Three labels are one net, not three pairs; and a blank name joins
/// nothing, because an unnamed label is one somebody has not finished typing.
#[test]
fn three_join_and_blanks_do_not() {
    let mut d = split();
    d.push(spec(5, r(1_000.0), (40, 0), (40, 8)));
    d.push(gnd(6, (40, 8)));
    for (i, at) in [(0, 0), (20, 0), (40, 0)].into_iter().enumerate() {
        d.push(label(10 + i as u32, "VCC", at));
    }
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(2_000);
    for p in [(20, 0), (40, 0)] {
        assert!((eng.voltage_at(p).unwrap() - 5.0).abs() < 1e-6, "{p:?} should be on the supply");
    }

    // Blank names: two unnamed labels must NOT short the room together.
    let mut b = split();
    b.push(label(10, "", (0, 0)));
    b.push(label(11, "   ", (20, 0)));
    let mut e2 = Engine::new(DT);
    e2.set_elements(&b);
    e2.advance(2_000);
    assert!(
        e2.voltage_at((20, 0)).unwrap().abs() < 1e-9,
        "blank names must join nothing"
    );
}

/// A label stamps NOTHING. It carries no current and dissipates no power —
/// it only decides which points are the same unknown, exactly as a wire does.
#[test]
fn a_label_stamps_nothing() {
    let mut d = split();
    d.push(label(10, "+5V", (0, 0)));
    d.push(label(11, "+5V", (20, 0)));
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(2_000);
    for f in eng.frame() {
        if f.id == 10 || f.id == 11 {
            assert!(f.power.abs() < 1e-12, "a label dissipates nothing, got {}", f.power);
        }
    }
}
