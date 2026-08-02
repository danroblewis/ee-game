//! Analytic golden tests: the simulator versus closed-form solutions.
//! Tolerances follow trapezoidal local truncation error at the chosen dt,
//! plus device-model tolerances for the semiconductor operating points.

use sim_core::{ElementSpec, Engine, InteractOp};
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

/// Count square-wave sign flips at the VCO's Schmitt output over 40 ms.
fn vco_flips(vctrl: f64) -> u32 {
    let mut eng = engine_with(ota_vco(vctrl));
    let mut flips = 0u32;
    let mut last_sign = 0i32;
    for _ in 0..800 {
        eng.advance(50); // 50 µs per observation
        let out = eng.voltage_at((12, 2)).unwrap();
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
    assert!(!eng.is_quarantined(), "VCO quarantined at vctrl={vctrl}");
    flips
}

#[test]
fn ota_vco_frequency_follows_control_voltage() {
    // f = (vctrl - Vbe)/(100k · 4 · 10n · 2.5): ~135 Hz at 2 V,
    // ~735 Hz at 8 V. Flips = 2f · 40 ms.
    let lo = vco_flips(2.0);
    let hi = vco_flips(8.0);
    assert!(lo >= 6, "VCO barely oscillates at 2 V: {lo} flips");
    assert!(
        (40..=80).contains(&hi),
        "VCO at 8 V: {hi} flips (expect ~59)"
    );
    let ratio = hi as f64 / lo as f64;
    // Ideal ratio (8-0.65)/(2-0.65) ≈ 5.4; allow model tolerance.
    assert!(
        (3.5..8.0).contains(&ratio),
        "frequency not tracking control voltage: lo={lo} hi={hi} ratio={ratio:.2}"
    );
}

#[test]
fn ota_output_current_saturates_at_iabc() {
    // Open-loop: drive the inputs hard; the output current into a load
    // resistor must saturate at Iabc = (5 - Vbe)/100k ≈ 43 µA.
    let elems = vec![
        ElementSpec {
            id: 1,
            kind: sim_core::ElementKind::Ota,
            pins: vec![(0, 0), (0, 2), (4, 1), (2, 4)],
        },
        spec(2, dc(3.0), (0, 0), (0, 6)), // in+ at +3 V (way past 2Vt)
        gnd(3, (0, 6)),
        gnd(4, (0, 2)),                     // in- grounded
        spec(5, r(1000.0), (4, 1), (4, 6)), // load to ground
        gnd(6, (4, 6)),
        spec(7, dc(5.0), (8, 4), (8, 8)),
        gnd(8, (8, 8)),
        spec(9, r(100_000.0), (8, 4), (2, 4)), // bias
    ];
    let mut eng = engine_with(elems);
    eng.advance(200);
    assert!(!eng.is_quarantined());
    let i_load = elem_current(&eng, 5, 0);
    assert!(
        (38e-6..48e-6).contains(&i_load),
        "saturated OTA output {i_load} (expect ~43 µA)"
    );
}

#[test]
fn timer555_astable_frequency_and_duty() {
    // f = 1.44/((RA + 2·RB)·C) = 480 Hz, duty (RA+RB)/(RA+2·RB) = 66.7 %.
    let mut eng = engine_with(timer555_astable());
    let half_rail = 4.5;
    let mut high = false;
    // (step index, level after the edge) for every OUT transition in 40 ms.
    let mut edges: Vec<(u32, bool)> = Vec::new();
    for k in 0..40_000u32 {
        eng.advance(1);
        let now = eng.voltage_at((10, 4)).unwrap() > half_rail;
        if now != high {
            edges.push((k, now));
            high = now;
        }
    }
    assert!(!eng.is_quarantined(), "555 astable quarantined");
    let rise: Vec<u32> = edges.iter().filter(|(_, h)| *h).map(|(k, _)| *k).collect();
    let fall: Vec<u32> = edges.iter().filter(|(_, h)| !*h).map(|(k, _)| *k).collect();
    // Skip the power-on cycle: the cap starts at 0 V, not at 1/3 Vcc.
    assert!(
        rise.len() >= 4 && fall.len() >= 3,
        "555 not oscillating: {} rising, {} falling edges",
        rise.len(),
        fall.len()
    );
    let cycles = (rise.len() - 2) as f64;
    let period = (rise[rise.len() - 1] - rise[1]) as f64 * DT / cycles;
    let f = 1.0 / period;
    let expect = 1.44 / ((10_000.0 + 2.0 * 10_000.0) * 100e-9);
    assert!(
        (f - expect).abs() / expect < 0.25,
        "555 astable at {f:.1} Hz, expected {expect:.1} Hz"
    );
    // Duty cycle: rising edge to the next falling edge, over the period.
    let mut sum = 0.0;
    let mut n = 0.0;
    for w in rise[1..].windows(2) {
        if let Some(f_edge) = fall.iter().find(|k| **k > w[0] && **k < w[1]) {
            sum += (f_edge - w[0]) as f64 / (w[1] - w[0]) as f64;
            n += 1.0;
        }
    }
    let duty = sum / n;
    assert!(
        (0.55..0.80).contains(&duty),
        "555 duty {:.1} % (expected ~67 %)",
        duty * 100.0
    );
    // The output really is a totem pole against the live rail.
    let vout = eng.voltage_at((10, 4)).unwrap();
    assert!(
        (vout > 7.0 && vout < 8.0) || vout < 0.2,
        "555 OUT idles at {vout} V"
    );
}

/// A 555 dropped on the canvas with nothing wired to it (the very first
/// thing a player does) must sit there quietly, not quarantine the room.
#[test]
fn unpowered_timer555_is_harmless() {
    let elems = vec![ElementSpec {
        id: 1,
        kind: sim_core::ElementKind::Timer555,
        pins: vec![(0, 0), (0, 4), (0, 1), (0, 3), (4, 3), (4, 1)],
    }];
    let mut eng = engine_with(elems);
    eng.advance(500);
    assert!(!eng.is_quarantined(), "floating 555 quarantined");
    for f in eng.frame() {
        for p in 0..f.npins {
            assert!(f.v[p].is_finite() && f.i[p].is_finite(), "non-finite pin");
        }
    }
}

/// Holding a pushbutton across TRIG pins the 555 output high (trigger
/// dominates the threshold comparator), and releasing it resumes the
/// oscillation — the manual-retrigger interaction from the demo map.
#[test]
fn timer555_button_holds_output_high() {
    let mut elems = timer555_astable();
    // Button from the THR/TRIG node to ground.
    elems.push(ElementSpec {
        id: 20,
        kind: sim_core::ElementKind::Button { closed: false },
        pins: vec![(2, 4), (2, 8)],
    });
    elems.push(gnd(21, (2, 8)));
    let mut eng = engine_with(elems);
    eng.advance(5_000);
    eng.interact(20, InteractOp::SetSwitch { closed: true });
    // Held down for 20 ms — many free-running periods.
    let mut lows = 0u32;
    for _ in 0..2_000 {
        eng.advance(10);
        if eng.voltage_at((10, 4)).unwrap() < 4.5 {
            lows += 1;
        }
    }
    assert!(
        !eng.is_quarantined(),
        "555 quarantined with the button held"
    );
    assert_eq!(lows, 0, "output dropped low {lows} times while triggered");
    eng.interact(20, InteractOp::SetSwitch { closed: false });
    let mut flips = 0u32;
    let mut high = true;
    for _ in 0..4_000 {
        eng.advance(10);
        let now = eng.voltage_at((10, 4)).unwrap() > 4.5;
        if now != high {
            flips += 1;
            high = now;
        }
    }
    assert!(!eng.is_quarantined(), "555 quarantined after release");
    assert!(flips >= 4, "oscillation did not resume: {flips} flips");
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

// ------------------------------------------------------------------ motor

#[test]
fn motor_armature_is_an_rl_branch_with_back_emf() {
    let mut eng = engine_with(motor_step());
    // i(t) = (V - bemf)/R_loop · (1 - e^(-t/τ)); R_loop = 1 + 2 Ω,
    // τ = L/R_loop = 0.5 ms, i_ss = 10/3 A.
    let (i_ss, tau) = (10.0 / 3.0, 1.5e-3 / 3.0);
    let mut t = 0.0;
    for target in [0.25e-3, 0.5e-3, 1e-3] {
        let steps = ((target - t) / DT).round() as u32;
        eng.advance(steps);
        t = target;
        let i = elem_current(&eng, 3, 0);
        let expect = i_ss * (1.0 - libm::exp(-t / tau));
        assert!(
            (i - expect).abs() < 3e-3,
            "t={t}: i={i} expected={expect} (backward Euler at h/τ = {})",
            DT / tau
        );
    }
    // Steady state: 60 τ of settling leaves the exponential tail below
    // 1e-26, so this is Ohm's law around the loop against the 2 V back-EMF
    // to the last bit the GMIN leak allows.
    eng.advance(30_000);
    let i = elem_current(&eng, 3, 0);
    assert!((i - i_ss).abs() < 1e-9, "steady armature current {i}");
    // The armature drop: 12 - 1·i - 2·i - 2 V(bemf) = 0.
    let v_arm = eng.voltage_at((4, 0)).unwrap();
    assert!(
        (v_arm - (2.0 * i + 2.0)).abs() < 1e-9,
        "armature terminal {v_arm} V vs R·i + bemf = {}",
        2.0 * i + 2.0
    );
    assert!(!eng.is_quarantined());
}

#[test]
fn motor_back_emf_write_retargets_the_current_without_a_recompile() {
    use sim_core::ParamWrite;
    let mut eng = engine_with(motor_step());
    eng.advance(30_000); // 60 τ: the L/R tail is gone
    assert!((elem_current(&eng, 3, 0) - 10.0 / 3.0).abs() < 1e-9);

    // The machine-side write path: back-EMF is RHS-only.
    assert!(eng.write_param(3, ParamWrite::Bemf { volts: 6.0 }));
    eng.advance(30_000);
    let i = elem_current(&eng, 3, 0);
    assert!(
        (i - 2.0).abs() < 1e-9,
        "bemf 6 V -> (12-6)/3 = 2 A, got {i}"
    );

    // Regenerating: back-EMF above the supply reverses the current.
    assert!(eng.write_param(3, ParamWrite::Bemf { volts: 15.0 }));
    eng.advance(30_000);
    let i = elem_current(&eng, 3, 0);
    assert!(
        (i + 1.0).abs() < 1e-9,
        "bemf 15 V -> (12-15)/3 = -1 A, got {i}"
    );

    // Wrong parameter for the device, and unknown ids, are refused.
    assert!(!eng.write_param(2, ParamWrite::Bemf { volts: 1.0 }));
    assert!(!eng.write_param(3, ParamWrite::Wiper { frac: 0.5 }));
    assert!(!eng.write_param(999, ParamWrite::Bemf { volts: 1.0 }));
}

#[test]
fn param_writes_move_wipers_and_switches() {
    use sim_core::ParamWrite;
    let mut eng = settled(pot_divider());
    assert!(eng.write_param(2, ParamWrite::Wiper { frac: 0.8 }));
    eng.advance(50);
    let v = eng.voltage_at((4, 4)).unwrap();
    assert!((v - 1.8).abs() < 0.01, "wiper written to 0.8: {v}");

    // A switch write is a topology change (the branch count moves).
    let mut eng = engine_with(demo_lamp(false));
    eng.advance(100);
    assert!(elem_current(&eng, 4, 0).abs() < 1e-9, "open switch: dark");
    assert!(eng.write_param(3, ParamWrite::Switch { closed: true }));
    eng.advance(100);
    let i = elem_current(&eng, 4, 0);
    assert!((i - 0.1).abs() < 1e-6, "closed switch: 0.1 A, got {i}");
    // Writing the same position again is a no-op, not a recompile.
    assert!(eng.write_param(3, ParamWrite::Switch { closed: true }));
    eng.advance(100);
    assert!((elem_current(&eng, 4, 0) - 0.1).abs() < 1e-6);
    assert!(!eng.is_quarantined());
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

/// Placement validation must never reject a valid circuit: every golden
/// circuit — including their worst case with every switch closed — passes
/// `check_document` at both the golden dt and the server's dt. This is the
/// contract that lets the server refuse ops through the same function
/// without ever refusing legitimate play.
#[test]
fn all_golden_circuits_pass_placement_validation() {
    for (name, elems) in all_golden() {
        for dt in [DT, 20e-6] {
            assert_eq!(
                sim_core::check_document(&elems, dt),
                Ok(()),
                "{name} must be placeable at dt={dt}"
            );
        }
    }
    // Both switch states of the demo lamp.
    assert_eq!(sim_core::check_document(&demo_lamp(false), DT), Ok(()));
}
