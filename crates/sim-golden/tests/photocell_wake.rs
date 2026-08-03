//! A CAMERA MUST BE ABLE TO WAKE THE CIRCUIT IT IS POINTED AT.
//!
//! `photocell_divider_follows_the_light` in `golden.rs` writes a light the
//! instant after the circuit settles, while the island is still awake, so it
//! never exercised the case every real camera hits: a DC circuit goes quiet
//! within a second or so of being drawn, and every reading after that lands
//! on a sleeping island.
//!
//! `write_param` wakes the island only when it sets `changed`. The `Wiper`
//! arm sets it; the `Light` arm sets `invalidate` alone. So a knob moves a
//! settled circuit and a camera does not.

use sim_core::{Engine, ParamWrite};
use sim_golden::photocell_divider;

const DT: f64 = 1e-6;

fn engine_with(elems: Vec<sim_core::ElementSpec>) -> Engine {
    let mut eng = Engine::new(DT);
    eng.set_elements(&elems);
    eng
}

/// Advance until every island has gone quiet, or give up. Returns the number
/// of sleeping islands so the test can assert it actually got there.
fn sleep_it(eng: &mut Engine) -> usize {
    for _ in 0..400 {
        eng.advance(200);
        if eng.static_islands() > 0 {
            break;
        }
    }
    eng.static_islands()
}

#[test]
fn a_light_write_wakes_a_settled_circuit() {
    let expect = |l: f64| {
        let r = sim_core::photocell_ohms(1e6, 1e3, l);
        9.0 * r / (10_000.0 + r)
    };

    let mut eng = engine_with(photocell_divider(0.0));
    let asleep = sleep_it(&mut eng);
    assert!(asleep > 0, "the divider never went quiet; test proves nothing");

    let dark = eng.voltage_at((6, 0)).unwrap();
    assert!((dark - expect(0.0)).abs() < 1e-3, "dark {dark}");

    // The camera reports full light. This is exactly what the server does on
    // every accepted reading.
    assert!(eng.write_param(3, ParamWrite::Light { light: 1.0 }));
    eng.advance(200);

    let lit = eng.voltage_at((6, 0)).unwrap();
    assert!(
        (lit - expect(1.0)).abs() < 1e-3,
        "a settled circuit ignored the camera: v={lit}, expected {} (still reading {dark}, the DARK value)",
        expect(1.0)
    );
}

/// The same property stated the way the feature is actually used: a stream of
/// readings arriving one per frame at a circuit that has long since settled.
#[test]
fn a_stream_of_readings_keeps_moving_the_circuit() {
    let mut eng = engine_with(photocell_divider(0.0));
    assert!(sleep_it(&mut eng) > 0, "never went quiet");

    let mut seen: Vec<f64> = Vec::new();
    for step in 0..8 {
        let light = f64::from(step) / 7.0;
        assert!(eng.write_param(3, ParamWrite::Light { light }));
        eng.advance(200);
        seen.push(eng.voltage_at((6, 0)).unwrap());
    }
    let spread = seen.iter().cloned().fold(f64::MIN, f64::max)
        - seen.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        spread > 1.0,
        "eight readings from dark to full light moved the node by {spread} V: {seen:?}"
    );
}
