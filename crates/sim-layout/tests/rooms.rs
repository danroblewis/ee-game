//! Golden tests on real saved rooms:
//! - the generated Analog Synthesizer (the room the layout exists to fix),
//! - the hand-authored Showcase (the control),
//! - the Main Room with reserved machine-fixture ids (frozen anchors).
//!
//! Hard invariants: netlist partition preserved (verified twice: inside
//! relayout and again here), `check_document` accepts the output, output is
//! byte-deterministic, no net degraded to the staircase tier, and the
//! element count stays under the client placement gate.

use sim_core::ElementSpec;

const GATE_MAX_ELEMENTS: usize = 800;

fn load(name: &str) -> Vec<ElementSpec> {
    let path = format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn run_room(fixture: &str) -> sim_layout::LayoutResult {
    let input = load(fixture);
    let r1 = sim_layout::relayout(&input).expect("layout failed");
    let r2 = sim_layout::relayout(&input).expect("second run failed");
    assert_eq!(
        serde_json::to_string(&r1.elements).unwrap(),
        serde_json::to_string(&r2.elements).unwrap(),
        "same input must produce byte-identical output"
    );
    sim_layout::extract::diff_partitions(&input, &r1.elements)
        .expect("netlist partition changed");
    sim_core::check_document(&r1.elements, 20e-6).expect("gate refused layout");
    assert!(r1.elements.len() <= GATE_MAX_ELEMENTS);
    assert_eq!(r1.report.tier_staircase, 0, "no degraded nets on these rooms");
    assert_eq!(r1.report.quality, sim_layout::Quality::Clean);
    r1
}

#[test]
fn synth_room() {
    let r = run_room("synth_elements.json");
    // the insane room: 64 parts, 1 wire, everything rubber-banded. After:
    // canonical parts + synthesized wires. Keep a loose ceiling so quality
    // regressions (wire explosions) fail loudly.
    assert!(
        r.report.elements_after < 420,
        "wire budget regression: {} elements",
        r.report.elements_after
    );
}

#[test]
fn showcase_room() {
    let r = run_room("showcase_elements.json");
    assert!(
        r.report.elements_after < 300,
        "wire budget regression: {} elements",
        r.report.elements_after
    );
}

#[test]
fn hoist_room_frozen_fixtures() {
    let input = load("hoist_elements.json");
    let r = run_room("hoist_elements.json");
    // reserved-id fixtures must be byte-identical in the output
    for e in input.iter().filter(|e| sim_layout::reserved_id(e.id)) {
        let out = r
            .elements
            .iter()
            .find(|o| o.id == e.id)
            .expect("fixture missing");
        assert_eq!(out.pins, e.pins, "fixture {} moved", e.id);
    }
}
