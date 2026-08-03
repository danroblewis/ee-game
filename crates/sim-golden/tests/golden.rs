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

/// The honest output stage, in the three places it has to be honest.
///
/// A real op-amp's output folds back to `isc` and stays there — it is
/// short-circuit PROOF, not short-circuit fragile — so the failure a player
/// meets is "it stops delivering", not "it exploded". These are the closed
/// forms for that, and the reason an op-amp cannot drive a motor.
#[test]
fn opamp_output_current_folds_back_at_isc() {
    use sim_core::{ElementKind as K, DEFAULT_OPAMP_ISC as ISC};

    // Comparator railed high into a load, swept from light to dead short.
    // pins: [in+, in-, out]; in+ at +1 V, in- grounded, so it wants +rail.
    let build = |ohms: f64| {
        vec![
            spec3(
                1,
                K::OpAmp {
                    rail: 5.0,
                    isc: ISC,
                },
                (0, 0),
                (0, 4),
                (8, 2),
            ),
            spec(2, dc(1.0), (0, 0), (0, 8)),
            spec(3, K::Wire, (0, 4), (0, 8)),
            gnd(4, (0, 8)),
            spec(5, r(ohms), (8, 2), (8, 8)),
            gnd(6, (8, 8)),
        ]
    };
    // 1 kΩ wants 5 mA: comfortably inside the limit, so the output sits on
    // the rail exactly as it always did. (This is also why not one golden
    // hash moved when the limit was added.)
    let eng = settled(build(1000.0));
    let vout = eng.voltage_at((8, 2)).unwrap();
    assert!((vout - 5.0).abs() < 1e-6, "light load must still rail: {vout}");
    assert!((eng.pin_current(1, 2).unwrap() + 0.005).abs() < 1e-6);

    // 100 Ω would want 50 mA. It gets 25, and the output SAGS to 2.5 V —
    // the truthful answer, and one the player can see on a probe.
    let eng = settled(build(100.0));
    let vout = eng.voltage_at((8, 2)).unwrap();
    let iout = -eng.pin_current(1, 2).unwrap();
    assert!((iout - ISC).abs() < 1e-9, "must fold back to isc: {iout} A");
    assert!((vout - ISC * 100.0).abs() < 1e-4, "output sags to {vout} V");

    // A dead short: still 25 mA, forever. 0.125 W in a 0.35 W package.
    let eng = settled(build(1e-3));
    let iout = -eng.pin_current(1, 2).unwrap();
    assert!((iout - ISC).abs() < 1e-9, "shorted output: {iout} A");
    let p = eng
        .frame()
        .into_iter()
        .find(|f| f.id == 1)
        .unwrap()
        .power;
    assert!(
        (p - ISC * 5.0).abs() < 1e-4,
        "a shorted op-amp burns i·rail in its output stage, not zero: {p} W"
    );

    // Negative feedback is limited too, and it is limited SYMMETRICALLY: a
    // follower asked to sink 50 mA folds back at -25 mA.
    let sink = vec![
        spec3(
            1,
            K::OpAmp {
                rail: 5.0,
                isc: ISC,
            },
            (0, 0),
            (0, 4),
            (8, 2),
        ),
        spec(2, dc(-1.0), (0, 0), (0, 8)),
        spec(3, K::Wire, (0, 4), (0, 8)),
        gnd(4, (0, 8)),
        spec(5, r(100.0), (8, 2), (8, 8)),
        gnd(6, (8, 8)),
    ];
    let eng = settled(sink);
    let iout = -eng.pin_current(1, 2).unwrap();
    assert!((iout + ISC).abs() < 1e-9, "sinking limit: {iout} A");
}

/// A power MOSFET switching an inductive load with no freewheel path used
/// to have NO solution at all: the winding's stored current had nowhere to
/// go at turn-off, NR diverged, and the whole room quarantined with nothing
/// on screen to explain it. Every real power MOSFET is avalanche-rated, and
/// modelling that is what turns the freeze into a lesson.
#[test]
fn a_mosfet_avalanches_instead_of_stranding_an_inductor() {
    use sim_core::ElementKind as K;
    // 12 V -> 10 mH -> drain; gate driven from a switch, source grounded.
    let build = |gate_on: bool| {
        vec![
            spec(1, dc(12.0), (0, 0), (0, 12)),
            spec(2, K::Inductor { henries: 10e-3 }, (0, 0), (6, 0)),
            spec3(3, K::Nmos { vt: 2.0, k: 5.0 }, (6, 6), (6, 0), (6, 10)),
            spec(4, K::Wire, (6, 10), (0, 12)),
            gnd(5, (0, 12)),
            spec(6, dc(if gate_on { 10.0 } else { 0.0 }), (6, 6), (0, 12)),
        ]
    };
    let mut eng = Engine::new(1e-6);
    eng.set_elements(&build(true));
    eng.advance(20_000); // 20 ms: the choke charges up
    let i_on = eng.pin_current(2, 0).unwrap();
    assert!(i_on > 0.5, "the inductor must have real current in it: {i_on} A");

    // Now open the gate. Without the clamp this is where the solver died.
    eng.set_elements(&build(false));
    let report = eng.advance(2000);
    assert!(!eng.is_quarantined(), "turn-off must stay solvable");
    assert_eq!(report.steps, 2000);
    // The FET holds the drain at its avalanche voltage while the winding
    // dumps into it — tens of volts and amps, i.e. hundreds of watts, which
    // is exactly the bill the player is meant to see.
    let frames = eng.frame();
    let fet = frames.iter().find(|f| f.id == 3).unwrap();
    let vds = fet.v[1] - fet.v[2];
    assert!(
        (55.0..75.0).contains(&vds),
        "the drain should sit at the avalanche knee, not run away: {vds} V"
    );
    assert!(
        fet.power > 20.0,
        "and it should be dissipating the winding's energy: {} W",
        fet.power
    );
    // Left alone it decays: the energy is finite, so the clamp releases.
    eng.advance(200_000);
    assert!(!eng.is_quarantined());
    let after = eng.frame();
    let fet = after.iter().find(|f| f.id == 3).unwrap();
    assert!(
        fet.power.abs() < 1e-3,
        "avalanche must end when the current does: {} W",
        fet.power
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
            tier: 0,
            rot: 0,
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
        tier: 0,
        rot: 0,
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
        tier: 0,
        rot: 0,
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

// -------------------------------------------------------------- photocell

/// The cell's law, and the fact that it is the ONLY thing the solver knows
/// about an external input: R falls log-linearly with illumination, and the
/// divider follows it. Every number here is a closed form, not a recorded
/// value.
#[test]
fn photocell_resistance_is_log_linear_in_light() {
    use sim_core::photocell_ohms;
    let (d, t) = (1e6, 1e3);
    assert_eq!(photocell_ohms(d, t, 0.0), d, "dark is exactly r_dark");
    assert_eq!(photocell_ohms(d, t, 1.0), t, "full light is exactly r_lit");
    // Halfway in LOG space is the geometric mean: sqrt(1e6 * 1e3) ~ 31.6 kOhm.
    let mid = photocell_ohms(d, t, 0.5);
    assert!((mid - 31_622.776_6).abs() < 1e-3, "half light {mid}");
    // Clamped and NaN-proof: nothing outside 0..1 can reach the matrix.
    assert_eq!(photocell_ohms(d, t, -5.0), d);
    assert_eq!(photocell_ohms(d, t, 7.0), t);
    assert_eq!(photocell_ohms(d, t, f64::NAN), d, "NaN reads dark, not inf");
}

#[test]
fn photocell_divider_follows_the_light() {
    let expect = |l: f64| {
        let r = sim_core::photocell_ohms(1e6, 1e3, l);
        9.0 * r / (10_000.0 + r)
    };
    let mut eng = settled(photocell_divider(0.0));
    let v = eng.voltage_at((6, 0)).unwrap();
    assert!((v - expect(0.0)).abs() < 1e-3, "dark: {v} vs {}", expect(0.0));

    // The whole feature in one call: a scalar from outside becomes a
    // resistance, through the path that never recompiles and never touches
    // the solver's health flags.
    assert!(eng.write_param(3, sim_core::ParamWrite::Light { light: 0.37 }));
    eng.advance(200);
    let v = eng.voltage_at((6, 0)).unwrap();
    assert!((v - expect(0.37)).abs() < 1e-3, "dim: {v} vs {}", expect(0.37));

    assert!(eng.write_param(3, sim_core::ParamWrite::Light { light: 1.0 }));
    eng.advance(200);
    let v = eng.voltage_at((6, 0)).unwrap();
    assert!((v - expect(1.0)).abs() < 1e-3, "lit: {v} vs {}", expect(1.0));
    assert!(!eng.is_quarantined());
}

/// The property the design turns on: a light write is not a player edit. It
/// must never clear quarantine and never re-arm the post-event BE steps —
/// otherwise a driven part resurrects a diverged room 30 times a second.
#[test]
fn a_light_write_is_not_an_edit() {
    let mut eng = settled(photocell_divider(0.5));
    // Same guard `Wiper` has: an unchanged reading is free.
    assert!(eng.write_param(3, sim_core::ParamWrite::Light { light: 0.5 }));
    // Wrong device, wrong parameter: refused rather than silently applied.
    assert!(!eng.write_param(1, sim_core::ParamWrite::Light { light: 0.5 }));
    assert!(!eng.write_param(3, sim_core::ParamWrite::Wiper { frac: 0.5 }));
    // And the knob path cannot drive it at all: SetValue on a photocell is
    // not a thing, so no panel widget and no `Cmd::Interact` can pretend to
    // be a camera.
    let before = eng.voltage_at((6, 0)).unwrap();
    eng.interact(3, InteractOp::SetValue { value: 1.0 });
    eng.advance(200);
    let after = eng.voltage_at((6, 0)).unwrap();
    assert!((before - after).abs() < 1e-6, "SetValue moved a photocell");
}

/// Old saves still load, and a saved room loads DARK. `light` is
/// `serde(skip)`: a reading is world state, not document state.
#[test]
fn a_saved_photocell_loads_dark() {
    let lit = sim_core::ElementKind::Photocell {
        r_dark: 1e6,
        r_lit: 1e3,
        light: 1.0,
    };
    let json = serde_json::to_string(&lit).unwrap();
    assert!(
        !json.contains("light"),
        "a reading reached the save file: {json}"
    );
    let back: sim_core::ElementKind = serde_json::from_str(&json).unwrap();
    match back {
        sim_core::ElementKind::Photocell { light, r_dark, .. } => {
            assert_eq!(light, 0.0, "loaded lit");
            assert_eq!(
                r_dark, 1e6,
                "calibration is document state and must survive"
            );
        }
        _ => panic!("wrong kind"),
    }
    // A document written before photocells existed still parses, unchanged.
    let old: sim_core::ElementKind =
        serde_json::from_str(r#"{"t":"Resistor","ohms":1000.0}"#).unwrap();
    assert!(matches!(old, sim_core::ElementKind::Resistor { .. }));
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

/// Asking for a finer simulation must never make the answer worse. `dt` is
/// the accuracy the room asked for; an engine in which halving it degrades
/// the result has inverted the meaning of its own parameter.
///
/// Measured before the fix, worst error against the closed form over the
/// first 10 ms of `rc_step`:
///
/// | dt     | levers off | levers on |
/// |--------|------------|-----------|
/// | 20 µs  | 3.79e-3    | 3.79e-3   |
/// | 5 µs   | 2.47e-4    | 3.63e-3   |
/// | 1 µs   | 9.89e-6    | 1.09e-2   |  <- 1,100x worse than levers off,
///                                        and worse than 20x the step size
///
/// The mechanism was read-out staleness, not truncation: an absolute
/// per-step error budget let `k` rise in exact proportion as `dt` fell, so
/// the island kept ending its step up to `(k-1)·dt` of world time early and
/// the first-order lag `slew × lag` never shrank. `Tuning::local_dt_slew`
/// governs that lag directly, which is what puts the room dt back in charge.
#[test]
fn refining_the_room_dt_never_makes_the_answer_worse() {
    // Worst |v_cap - closed form| over the first 10 ms, sampled every 40 µs
    // — a whole number of substeps at every dt in the sweep, so all six
    // runs are compared on exactly the same instants.
    let worst = |dt: f64, tuning: sim_core::Tuning| {
        let mut eng = Engine::new(dt);
        eng.set_tuning(tuning);
        eng.set_elements(&rc_step());
        let per = (40e-6 / dt).round() as u32;
        let mut worst = 0.0f64;
        for _ in 0..250 {
            eng.advance(per);
            let t = eng.time();
            let v = eng.voltage_at((8, 0)).unwrap();
            worst = worst.max((v - 10.0 * (1.0 - libm::exp(-t / 1e-3))).abs());
        }
        worst
    };

    let mut prev: Option<(f64, f64)> = None;
    for dt in [40e-6, 20e-6, 10e-6, 5e-6, 2e-6, 1e-6] {
        let on = worst(dt, sim_core::Tuning::default());
        let off = worst(dt, sim_core::Tuning::off());
        // The levers may cost at most the staleness ceiling they are
        // allowed to spend — and on this circuit they cost nothing at all,
        // because a cap moving at kilovolts per second is never dilated.
        let ceiling = sim_core::Tuning::default().local_dt_slew * dt;
        assert!(
            on <= off + ceiling,
            "dt={dt:.0e}: levers cost {:.3e}, ceiling {ceiling:.3e}",
            on - off
        );
        if let Some((pdt, pworst)) = prev {
            assert!(
                on <= pworst,
                "refining dt from {pdt:.0e} to {dt:.0e} made the answer \
                 WORSE: {pworst:.4e} -> {on:.4e}"
            );
        }
        prev = Some((dt, on));
    }
    // ...and it really is converging, not just failing to get worse.
    assert!(prev.unwrap().1 < 1e-5, "dt refinement bought nothing");

    // The reproduction, kept: take the staleness governor away (which is
    // exactly the controller that shipped) and the inversion comes back —
    // the finest dt in the sweep lands worse than the coarsest but one.
    let ungoverned = sim_core::Tuning {
        local_dt_slew: f64::INFINITY,
        local_dt_slew_i: f64::INFINITY,
        ..sim_core::Tuning::default()
    };
    let (coarse, fine) = (worst(20e-6, ungoverned), worst(1e-6, ungoverned));
    assert!(
        fine > coarse,
        "the pre-fix controller no longer inverts ({fine:.3e} at 1 us vs \
         {coarse:.3e} at 20 us), so this test proves nothing"
    );
}

/// `Tuning::off()` is the yardstick every measurement of the levers is taken
/// against, so it has to be the SAME ENGINE that existed before islands and
/// the levers landed — not "close enough", the same bits.
///
/// These digests were produced by the unpartitioned engine at commit
/// 0475bbf, the merge base this work landed on. Every one of them survived
/// three separate things unchanged: per-island partitioning (the node
/// numbering a one-island document gets is exactly the numbering it had),
/// per-island ideal-constraint merging, and per-island piecewise-linear
/// factorization reuse. If a lever, or the partition, ever leaks a decision
/// into the path taken with the levers off, this test says so, and every
/// performance number in `docs/scale-baseline.md` stops meaning anything
/// until it passes again.
///
/// Bit-identity is also why `state_hash` still reproduces: it walks the
/// islands' solution vectors in island order and the elements in DOCUMENT
/// order, and for a single-island circuit both are what they always were.
///
/// KNOW WHAT THIS DOES NOT COVER. `every_golden_circuit_is_a_single_island`
/// is a guard, but it is also this test's ceiling: passing here says nothing
/// about a world that actually partitions. Measured separately on generated
/// multi-island rooms, levers off: linear ones stay bit-identical, but ones
/// containing a smooth nonlinearity land within 1.75e-8 V (57x inside
/// `NR_ABSTOL`) with an identical discrete trajectory — because `main` ran
/// ONE global Newton loop, where a still-moving diode dragged its converged
/// neighbours through extra iterations, and each island now converges alone.
/// Do not "fix" that by re-coupling the loops. See the exactness-scope table
/// in `docs/scale-baseline.md` before quoting "bit for bit" anywhere.
#[test]
fn with_both_levers_off_the_engine_is_the_pre_lever_engine_bit_for_bit() {
    let want: &[(&str, u64)] = &[
        ("demo_lamp", 0x8ea6635af49d7cac),
        ("rc_step", 0x81501c4d4de6f40d),
        ("rl_step", 0x3e87234f3f92f577),
        ("rlc_ring", 0x73c8d122d5f0385d),
        ("half_wave_rectifier", 0x80a7cfb690de1c7f),
        ("npn_switch", 0x854cefa9a46cb938),
        ("emitter_follower", 0x89aeff94fab2f084),
        ("nmos_switch", 0x4514efde3a618625),
        ("opamp_follower", 0x41e2e1d8df28af5c),
        ("opamp_comparator", 0x539b9cdfa8b92244),
        ("zener_regulator", 0xa3126d892895678f),
        ("opamp_relaxation", 0x61d0292acee94cc4),
        ("ota_vco", 0x6e972258d2b8fa35),
        ("timer555_astable", 0x3a6caa090f0e09ae),
        ("pot_divider", 0x690dcc11c656ea76),
        ("led_loop", 0x20adc6d1dbcb0cfe),
        ("motor_step", 0xef8d8ee93986bba0),
        ("noise_rc", 0xde7b8af17db3ee3e),
    ];
    // The determinism harness's own protocol: 1 us, 10k steps, every golden
    // circuit, in order.
    let mut seen = 0usize;
    for (name, elems) in all_golden() {
        let mut eng = Engine::new(1e-6);
        eng.set_tuning(sim_core::Tuning::off());
        eng.set_elements(&elems);
        let report = eng.advance(10_000);
        assert_eq!(report.steps, 10_000, "{name}");
        // A golden added AFTER this baseline was captured has no pre-lever
        // digest, and cannot: there was no pre-lever engine to run it on. Skip
        // it rather than panic — but the `seen == want.len()` assertion below
        // still means every one of the recorded circuits was exercised, so a
        // golden that is renamed or quietly dropped is caught, and this cannot
        // decay into a test that skips everything and passes.
        let Some((_, hash)) = want.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        assert_eq!(
            eng.state_hash(),
            *hash,
            "{name}: levers off no longer reproduces the pre-lever engine"
        );
        seen += 1;
    }
    assert_eq!(
        seen,
        want.len(),
        "a recorded golden circuit went missing from `all_golden()`"
    );
}

/// ...and the same circuits, run with factorization reuse switched off as
/// well, must land on the SAME digests. The three mechanisms this work
/// touches — the island partition, the constraint merging inside it, and the
/// piecewise-linear reuse on top of it — are each individually exact, and
/// this is the statement that they are jointly exact too.
#[test]
fn levers_off_and_reuse_off_is_the_same_engine_again() {
    for (name, elems) in all_golden() {
        let mut a = Engine::new(1e-6);
        a.set_tuning(sim_core::Tuning::off());
        a.set_elements(&elems);
        let mut b = Engine::new(1e-6);
        b.set_tuning(sim_core::Tuning::off());
        b.set_elements(&elems);
        b.set_reuse_pwl(false);
        a.advance(10_000);
        b.advance(10_000);
        assert_eq!(a.state_hash(), b.state_hash(), "{name}");
    }
}

/// Every golden circuit is ONE island, which is what makes the digest test
/// above a statement about partitioning rather than about luck: with one
/// island the island-local node numbering IS the global numbering, so the
/// unknown vector the digest walks is laid out exactly as it was.
///
/// If a golden is ever added that genuinely splits, this fails rather than
/// letting the digest test quietly start comparing a permuted vector.
#[test]
fn every_golden_circuit_is_a_single_island() {
    for (name, elems) in all_golden() {
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        let solving = eng.islands().iter().filter(|i| i.unknowns() > 0).count();
        assert_eq!(solving, 1, "{name} is not one island");
        assert_eq!(
            eng.islands()[0].unknowns(),
            eng.unknowns(),
            "{name}: the one island must hold every unknown"
        );
    }
}
