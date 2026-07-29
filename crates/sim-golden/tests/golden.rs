//! Analytic golden tests: the simulator versus closed-form solutions.
//! Tolerances follow trapezoidal local truncation error at the chosen dt,
//! plus device-model tolerances for the semiconductor operating points.

use sim_core::{Engine, InteractOp};
use sim_golden::*;

const DT: f64 = 1e-6;

fn engine_with(elems: Vec<sim_core::ElementSpec>) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(&elems);
    e
}

/// Settle a DC circuit and return the engine (asserts no quarantine).
fn settled(elems: Vec<sim_core::ElementSpec>) -> Engine {
    let mut eng = engine_with(elems);
    eng.advance(200);
    assert!(!eng.is_quarantined(), "circuit quarantined during settle");
    eng
}

fn elem_current(eng: &Engine, id: u32, pin: usize) -> f64 {
    eng.frame().iter().find(|e| e.id == id).unwrap().i[pin]
}

#[test]
fn rc_step_matches_exponential() {
    let mut eng = engine_with(rc_step());
    let tau = 1e-3;
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
        let i_l = elem_current(&eng, 3, 0);
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
}

#[test]
fn rectifier_produces_dc_with_small_ripple() {
    let mut eng = engine_with(half_wave_rectifier());
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
    assert!(vmax > 8.0, "peak too low: {vmax}");
    assert!(vmin > 6.0, "trough too low: {vmin}");
    let ripple = vmax - vmin;
    assert!((0.5..3.0).contains(&ripple), "ripple {ripple}");
}

#[test]
fn no_nan_and_kcl_under_interaction() {
    let mut eng = engine_with(demo_lamp(false));
    let mut closed = false;
    for _ in 0..50 {
        closed = !closed;
        eng.interact(3, InteractOp::SetSwitch { closed });
        eng.advance(100);
        for f in eng.frame() {
            for p in 0..f.npins {
                assert!(f.v[p].is_finite() && f.i[p].is_finite());
            }
        }
        if closed {
            let src = elem_current(&eng, 1, 0);
            let lamp = elem_current(&eng, 4, 0);
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
    let mut elems = demo_lamp(true);
    elems.retain(|e| !matches!(e.kind, sim_core::ElementKind::Ground));
    let mut eng = engine_with(elems);
    eng.advance(1000);
    assert!(!eng.is_quarantined());
    let lamp = elem_current(&eng, 4, 0);
    assert!((lamp.abs() - 0.1).abs() < 1e-6, "lamp current {lamp}");
}

// ------------------------------------------------------- semiconductors

#[test]
fn npn_switch_saturates() {
    let eng = settled(npn_switch());
    let vc = eng.voltage_at((4, 2)).unwrap();
    assert!(vc < 0.5, "collector not saturated: {vc}");
    // Collector current ≈ (9 - Vce)/100 ≈ 86-90 mA, into the collector pin.
    let ic = elem_current(&eng, 4, 1);
    assert!((0.08..0.095).contains(&ic), "ic {ic}");
    // KCL across the transistor: pin currents sum to ~0.
    let f = eng.frame();
    let q = f.iter().find(|e| e.id == 4).unwrap();
    let sum: f64 = q.i.iter().sum();
    assert!(sum.abs() < 1e-9, "BJT KCL: {sum}");
}

#[test]
fn emitter_follower_tracks_base() {
    let eng = settled(emitter_follower());
    let vb = eng.voltage_at((4, 4)).unwrap();
    let ve = eng.voltage_at((8, 6)).unwrap();
    // Base pulled slightly under 4.5 by base current; V(be) ≈ 0.6-0.75.
    assert!((4.2..4.55).contains(&vb), "vb {vb}");
    assert!((vb - ve) > 0.55 && (vb - ve) < 0.8, "vbe {}", vb - ve);
    assert!((3.5..4.0).contains(&ve), "ve {ve}");
}

#[test]
fn nmos_switch_conducts() {
    let eng = settled(nmos_switch());
    let vds = eng.voltage_at((6, 2)).unwrap();
    assert!(vds < 1.0, "vds too high: {vds}");
    let i_load = elem_current(&eng, 3, 0);
    assert!((0.085..0.1).contains(&i_load), "load current {i_load}");
}

#[test]
fn opamp_follower_is_unity_gain() {
    let eng = settled(opamp_follower());
    let vout = eng.voltage_at((8, 0)).unwrap();
    assert!((vout - 2.0).abs() < 1e-3, "follower out {vout}");
}

#[test]
fn opamp_comparator_rails() {
    let eng = settled(opamp_comparator());
    let vout = eng.voltage_at((8, 0)).unwrap();
    assert!(
        (vout - 5.0).abs() < 1e-6,
        "comparator out {vout} (want +rail)"
    );
}

#[test]
fn opamp_relaxation_oscillates() {
    // Self-starts via the op-amp input offset (τ = RC = 1 ms to walk out
    // of the metastable point), then flips at ~2.2 ms period. Over 40 ms
    // expect a healthy number of rail-to-rail transitions.
    let mut eng = engine_with(opamp_relaxation());
    let mut flips = 0u32;
    let mut last_sign = 0i32;
    let mut railed = false;
    for _ in 0..400 {
        eng.advance(100); // 100 µs per observation
        let out = eng.voltage_at((4, 4)).unwrap();
        if out.abs() > 4.5 {
            railed = true;
        }
        let sign = if out > 1.0 {
            1
        } else if out < -1.0 {
            -1
        } else {
            0
        };
        if sign != 0 && last_sign != 0 && sign != last_sign {
            flips += 1;
        }
        if sign != 0 {
            last_sign = sign;
        }
    }
    assert!(!eng.is_quarantined(), "oscillator quarantined");
    assert!(railed, "output never reached the rails");
    assert!(
        (5..=40).contains(&flips),
        "expected ~13 flips in 40 ms, got {flips}"
    );
}

#[test]
fn zener_regulates() {
    let eng = settled(zener_regulator());
    let v = eng.voltage_at((6, 0)).unwrap();
    assert!((5.3..5.9).contains(&v), "zener node {v}");
}

#[test]
fn pot_divider_follows_wiper() {
    let mut eng = settled(pot_divider());
    let v = eng.voltage_at((4, 4)).unwrap();
    assert!((v - 6.3).abs() < 0.01, "wiper at 0.3: {v}");
    eng.interact(2, InteractOp::SetValue { value: 0.8 });
    eng.advance(50);
    let v = eng.voltage_at((4, 4)).unwrap();
    assert!((v - 1.8).abs() < 0.01, "wiper at 0.8: {v}");
}

#[test]
fn led_drops_about_two_volts() {
    let eng = settled(led_loop());
    let v_led = eng.voltage_at((6, 0)).unwrap();
    assert!((1.9..2.3).contains(&v_led), "LED drop {v_led}");
    let i = elem_current(&eng, 3, 0);
    assert!((0.018..0.023).contains(&i), "LED current {i}");
}

#[test]
fn all_golden_circuits_run_clean() {
    for (name, elems) in all_golden() {
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        eng.advance(2000);
        assert!(!eng.is_quarantined(), "{name} quarantined");
        for f in eng.frame() {
            for p in 0..f.npins {
                assert!(
                    f.v[p].is_finite() && f.i[p].is_finite(),
                    "{name} non-finite"
                );
            }
        }
    }
}
