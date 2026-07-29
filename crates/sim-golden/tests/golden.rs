//! Analytic golden tests: the simulator versus closed-form solutions.
//! Tolerances follow trapezoidal local truncation error at the chosen dt.

use sim_core::{Engine, InteractOp};
use sim_golden::*;

const DT: f64 = 1e-6;

fn engine_with(elems: Vec<sim_core::ElementSpec>) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(&elems);
    e
}

#[test]
fn rc_step_matches_exponential() {
    let mut eng = engine_with(rc_step());
    let tau = 1e-3;
    // Sample at 0.5τ, 1τ, 2τ, 5τ.
    let mut t = 0.0;
    for target in [0.5e-3, 1e-3, 2e-3, 5e-3] {
        let steps = ((target - t) / DT).round() as u32;
        eng.advance(steps);
        t = target;
        let v_cap = eng.voltage_at((8, 0)).unwrap();
        let expect = 10.0 * (1.0 - libm::exp(-t / tau));
        assert!(
            (v_cap - expect).abs() < 1e-3,
            "t={t}: v_cap={v_cap} expected={expect}"
        );
    }
}

#[test]
fn rl_step_matches_exponential() {
    let mut eng = engine_with(rl_step());
    let tau = 10e-3 / 100.0;
    let mut t = 0.0;
    for target in [0.5e-4, 1e-4, 3e-4] {
        let steps = ((target - t) / DT).round() as u32;
        eng.advance(steps);
        t = target;
        let i_l = eng.frame().iter().find(|e| e.id == 3).unwrap().current;
        let expect = 0.05 * (1.0 - libm::exp(-t / tau));
        assert!(
            (i_l - expect).abs() < 1e-5,
            "t={t}: i_L={i_l} expected={expect}"
        );
    }
}

#[test]
fn rlc_rings_at_resonance() {
    let mut eng = engine_with(rlc_ring());
    // Count zero crossings of capacitor voltage minus final value (1 V)
    // over 2 ms; f0 = 1/(2π√(LC)) ≈ 5033 Hz -> ~20.1 crossings.
    let mut crossings = 0u32;
    let mut last = -1.0f64;
    for _ in 0..2000 {
        eng.advance(1);
        let v = eng.voltage_at((8, 0)).unwrap() - 1.0;
        if v * last < 0.0 {
            crossings += 1;
        }
        last = v;
    }
    assert!(
        (19..=22).contains(&crossings),
        "expected ~20 crossings in 2 ms, got {crossings}"
    );
    // Light damping (Q ≈ 31.6): the theoretical envelope e^(-R/2L · t)
    // must still be visible at 2 ms for the crossing count to mean much.
    let envelope = libm::exp(-1.0 / (2.0 * 1e-3) * 2e-3);
    assert!(envelope > 0.3, "test setup: damping too strong");
}

#[test]
fn rectifier_produces_dc_with_small_ripple() {
    let mut eng = engine_with(half_wave_rectifier());
    // Run 5 cycles at 60 Hz to settle, then measure ripple over one cycle.
    let cycle_steps = (1.0 / 60.0 / DT).round() as u32;
    eng.advance(5 * cycle_steps);
    let (mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for _ in 0..cycle_steps {
        eng.advance(1);
        let v = eng.voltage_at((8, 0)).unwrap();
        vmin = vmin.min(v);
        vmax = vmax.max(v);
    }
    assert!(!eng.is_quarantined(), "rectifier quarantined");
    // Peak ≈ 10 V minus a diode drop; RC = 0.1 s vs 16.7 ms cycle gives a
    // sawtooth ripple of very roughly V/(f·R·C) ≈ 1.5 V.
    assert!(vmax > 8.0, "peak too low: {vmax}");
    assert!(vmin > 6.0, "trough too low: {vmin}");
    let ripple = vmax - vmin;
    assert!((0.5..3.0).contains(&ripple), "ripple {ripple}");
}

#[test]
fn no_nan_and_kcl_under_interaction() {
    // Flip the demo switch every 100 steps while advancing; nothing may
    // go non-finite and the loop current must satisfy KCL (source current
    // equals lamp current).
    let mut eng = engine_with(demo_lamp(false));
    let mut closed = false;
    for _ in 0..50 {
        closed = !closed;
        eng.interact(3, InteractOp::SetSwitch { closed });
        eng.advance(100);
        for f in eng.frame() {
            assert!(f.va.is_finite() && f.vb.is_finite() && f.current.is_finite());
        }
        if closed {
            let f = eng.frame();
            let src = f.iter().find(|e| e.id == 1).unwrap().current;
            let lamp = f.iter().find(|e| e.id == 4).unwrap().current;
            assert!(
                (src.abs() - lamp.abs()).abs() < 1e-9,
                "KCL: {src} vs {lamp}"
            );
        }
    }
    assert!(!eng.is_quarantined());
}

#[test]
fn floating_circuit_does_not_explode() {
    // No ground element at all: gmin must keep the system solvable.
    let mut elems = demo_lamp(true);
    elems.retain(|e| !matches!(e.kind, sim_core::ElementKind::Ground));
    let mut eng = engine_with(elems);
    eng.advance(1000);
    assert!(!eng.is_quarantined());
    let f = eng.frame();
    let lamp = f.iter().find(|e| e.id == 4).unwrap();
    assert!(
        (lamp.current.abs() - 0.1).abs() < 1e-6,
        "lamp current {}",
        lamp.current
    );
}
