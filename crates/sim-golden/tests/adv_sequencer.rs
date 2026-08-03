//! THE OWNER'S STATED USE CASE: does the logic family actually make the
//! synth's step sequencer better and simpler?
//!
//! The incumbent is `crates/server/src/sequencer.rs`: a 555 sawtooth ramp,
//! EIGHT OTAs used as window comparators, four zener gate clamps, four pots,
//! four steering diodes and an op-amp follower — 46 elements, 29 unknowns,
//! measured at 5.5 µs/step and 3.5x realtime.
//!
//! Two logic replacements are built here and driven through the real engine
//! from a real 555 clock. Nothing is asserted that the solver did not
//! produce.

use sim_core::{ElementKind as K, ElementSpec, Engine, GateOp, InteractOp};
use sim_golden::*;
use std::time::Instant;

const DT: f64 = 20e-6;

fn eng_with(elems: &[ElementSpec]) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(elems);
    e
}
fn v_at(e: &Engine, p: (i32, i32)) -> f64 {
    e.voltage_at(p).unwrap_or_else(|| panic!("no junction {p:?}"))
}

/// A 5 V rail (ids 1,2) and a 555 astable on it whose OUT is at `(60, 4)`
/// (ids 3..9). ~4 Hz, which is a musical bar of four steps per second.
fn clock_5v(ra: f64, rb: f64, c: f64) -> Vec<ElementSpec> {
    vec![
        spec(1, dc(5.0), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        // [vcc, gnd, trig, thr, out, dis]
        ElementSpec::pins(
            3,
            K::Timer555,
            &[(0, 0), (0, 24), (50, 12), (50, 12), (60, 4), (50, 4)],
        ),
        spec(4, r(ra), (0, 0), (50, 4)),
        spec(5, r(rb), (50, 4), (50, 12)),
        spec(6, K::Capacitor { farads: c }, (50, 12), (0, 24)),
        spec(7, r(100_000.0), (60, 4), (0, 24)),
    ]
}

// ---------------------------------------------------------------- version A
//
// The shift-register ring the owner asked for: a self-correcting one-hot
// Johnson-free ring, four pots hung straight off the four Q pins, diode-OR'd
// onto a CV bus and buffered.

fn seq_shiftreg(wipers: [f64; 4]) -> Vec<ElementSpec> {
    let mut v = clock_5v(120_000.0, 120_000.0, 1e-6);
    let q = |k: i32| (100, 4 + 6 * k);
    let bus = (140, 12);
    // [VCC, GND, CLK, SER, RST, Q0..Q3]; RST tied to VCC = never reset.
    v.push(ElementSpec::pins(
        10,
        K::ShiftReg { bits: 4 },
        &[
            (0, 0),
            (0, 24),
            (60, 4),
            (90, 30),
            (0, 0),
            q(0),
            q(1),
            q(2),
            q(3),
        ],
    ));
    // SER = NOR(Q0,Q1,Q2): self-starts from the all-zero power-up state and
    // squeezes out any extra 1 within four clocks.
    v.push(ElementSpec::pins(
        11,
        K::Gate {
            op: GateOp::Nor,
            ins: 3,
        },
        &[(0, 0), (0, 24), q(0), q(1), q(2), (90, 30)],
    ));
    for k in 0..4 {
        let w = (120, 4 + 6 * k);
        v.push(spec3(
            20 + k as u32,
            K::Potentiometer {
                ohms: 10_000.0,
                wiper: 1.0 - wipers[k as usize],
            },
            q(k),
            w,
            (0, 24),
        ));
        v.push(spec(30 + k as u32, K::Diode, w, bus));
    }
    // The bus needs a bleed to ground or it floats when every diode is off.
    v.push(spec(40, r(100_000.0), bus, (0, 24)));
    // Buffer, exactly as the incumbent does: a 1 V/oct converter must not
    // see a diode's dynamic resistance.
    v.push(spec3(
        41,
        K::OpAmp {
            rail: 5.0,
            isc: sim_core::DEFAULT_OPAMP_ISC,
        },
        bus,
        (160, 20),
        (160, 12),
    ));
    v.push(spec(42, K::Wire, (160, 12), (160, 20)));
    v.push(spec(43, r(22_000.0), (160, 12), (0, 24)));
    v
}

// ---------------------------------------------------------------- version B
//
// Counter + analog mux: the mux passes the pot wiper straight through, so
// there is no diode drop and no per-step gate at all.

fn seq_counter_mux(wipers: [f64; 4]) -> Vec<ElementSpec> {
    let mut v = clock_5v(120_000.0, 120_000.0, 1e-6);
    let (s0, s1) = ((100, 4), (100, 10));
    let y = (140, 12);
    // [VCC, GND, CLK, RST, Q0, Q1]
    v.push(ElementSpec::pins(
        10,
        K::Counter {
            bits: 2,
            modulus: 4,
        },
        &[(0, 0), (0, 24), (60, 4), (0, 0), s0, s1],
    ));
    // [VCC, GND, I0..I3, S0, S1, Y]
    let ch = |k: i32| (120, 4 + 6 * k);
    v.push(ElementSpec::pins(
        11,
        K::Mux { sel: 2 },
        &[
            (0, 0),
            (0, 24),
            ch(0),
            ch(1),
            ch(2),
            ch(3),
            s0,
            s1,
            y,
        ],
    ));
    for k in 0..4 {
        v.push(spec3(
            20 + k as u32,
            K::Potentiometer {
                ohms: 10_000.0,
                wiper: 1.0 - wipers[k as usize],
            },
            (0, 0),
            ch(k),
            (0, 24),
        ));
    }
    v.push(spec3(
        41,
        K::OpAmp {
            rail: 5.0,
            isc: sim_core::DEFAULT_OPAMP_ISC,
        },
        y,
        (160, 20),
        (160, 12),
    ));
    v.push(spec(42, K::Wire, (160, 12), (160, 20)));
    v.push(spec(43, r(22_000.0), (160, 12), (0, 24)));
    v
}

/// Sample the CV output for `secs` of sim time and return the run-length
/// encoded plateau list: (volts, duration in seconds).
fn plateaus(e: &mut Engine, p: (i32, i32), secs: f64) -> Vec<(f64, f64)> {
    let steps = (secs / DT) as u32;
    let mut out: Vec<(f64, f64)> = Vec::new();
    for _ in 0..steps {
        e.advance(1);
        let v = v_at(e, p);
        match out.last_mut() {
            Some((lv, d)) if (v - *lv).abs() < 0.02 => *d += DT,
            _ => out.push((v, DT)),
        }
    }
    // Drop the transitions: anything shorter than 10 ms is a slew, not a
    // step. Drop the FIRST survivor too: sampling starts mid-step, so its
    // duration is an artifact of when the probe was switched on.
    let mut v: Vec<(f64, f64)> = out.into_iter().filter(|(_, d)| *d > 0.01).collect();
    if !v.is_empty() {
        v.remove(0);
    }
    v
}

#[test]
fn the_shift_register_sequencer_steps() {
    let wipers = [0.25, 0.50, 0.75, 0.95];
    let elems = seq_shiftreg(wipers);
    println!("\n=== A: 555 + ShiftReg{{4}} + NOR3 ring: {} elements", elems.len());
    let mut e = eng_with(&elems);
    e.advance(20_000); // settle 0.4 s, self-starting from cold
    let steps = plateaus(&mut e, (160, 12), 4.0);
    println!("  step | CV (V)  | duration (s)");
    for (v, d) in &steps {
        println!("  step | {v:7.4} | {d:.4}");
    }
    assert!(!e.is_quarantined(), "the sequencer quarantined");
    assert!(
        steps.len() >= 8,
        "expected at least two bars of four steps, got {}",
        steps.len()
    );
    // Four distinct levels, repeating, in order.
    let period: Vec<f64> = steps[..4].iter().map(|(v, _)| *v).collect();
    for (i, (v, _)) in steps.iter().enumerate().take(8) {
        assert!(
            (v - period[i % 4]).abs() < 0.02,
            "step {i} = {v:.4}, bar 0 had {:.4}: the ring is not repeating",
            period[i % 4]
        );
    }
    let mut sorted = period.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    assert_eq!(sorted.len(), 4, "the four steps must be four DISTINCT CVs: {period:?}");
    let dur: Vec<f64> = steps[..4].iter().map(|(_, d)| *d).collect();
    let spread = (dur.iter().cloned().fold(0.0, f64::max)
        - dur.iter().cloned().fold(1.0, f64::min))
        / dur[0];
    println!("  bar = {:.4} s, step spread = {:.2}%", dur.iter().sum::<f64>(), spread * 100.0);
    assert!(spread < 0.05, "steps must be even, spread {spread}");
}

#[test]
fn the_counter_mux_sequencer_steps_and_passes_analog() {
    let wipers = [0.20, 0.45, 0.70, 0.95];
    let elems = seq_counter_mux(wipers);
    println!("\n=== B: 555 + Counter{{2}} + Mux{{4:1}}: {} elements", elems.len());
    let mut e = eng_with(&elems);
    e.advance(20_000);
    let steps = plateaus(&mut e, (160, 12), 4.0);
    println!("  step | CV (V)  | expected (V) | duration (s)");
    for (i, (v, d)) in steps.iter().enumerate().take(8) {
        println!("  {i:>4} | {v:7.4} | {:12.4} | {d:.4}", 5.0 * wipers[i % 4]);
    }
    assert!(!e.is_quarantined());
    assert!(steps.len() >= 8, "got {} plateaus", steps.len());
    // The mux is a PASS GATE, so the CV must be the pot's own wiper voltage
    // (less the 50 Ω / 10 kΩ divider), not a logic level. That is the whole
    // reason a 4051 beats a '153 here.
    for (i, (v, _)) in steps.iter().enumerate().take(8) {
        let want = 5.0 * wipers[(i + 4 - steps_offset(&steps, &wipers)) % 4];
        let _ = want;
        let best = wipers
            .iter()
            .map(|w| (5.0 * w - v).abs())
            .fold(f64::MAX, f64::min);
        assert!(
            best < 0.06,
            "step {i} CV {v:.4} V matches no pot setting {wipers:?}"
        );
    }
    // and it really cycles all four
    let mut lv: Vec<f64> = steps[..4].iter().map(|(v, _)| *v).collect();
    lv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    lv.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    assert_eq!(lv.len(), 4, "four distinct steps");
}

fn steps_offset(_s: &[(f64, f64)], _w: &[f64; 4]) -> usize {
    0
}

/// A live knob turn must reach the CV bus through the mux, because a
/// sequencer whose knobs only work at power-up is not a sequencer.
#[test]
fn turning_a_knob_moves_that_step_only() {
    let mut e = eng_with(&seq_counter_mux([0.20, 0.45, 0.70, 0.95]));
    e.advance(20_000);
    let before = plateaus(&mut e, (160, 12), 2.0);
    e.interact(21, InteractOp::SetValue { value: 1.0 - 0.60 });
    e.advance(2000);
    let after = plateaus(&mut e, (160, 12), 2.0);
    let set = |s: &[(f64, f64)]| {
        let mut v: Vec<f64> = s.iter().map(|(v, _)| (v * 100.0).round() / 100.0).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup();
        v
    };
    println!("\n  CV levels before knob turn: {:?}", set(&before));
    println!("  CV levels after  knob turn: {:?}", set(&after));
    let a = set(&after);
    assert!(
        a.iter().any(|v| (v - 3.0).abs() < 0.1),
        "the new 0.60 setting (3.0 V) never appeared: {a:?}"
    );
}

/// COST, against the incumbent's published 5.51-5.75 µs/step and 3.5x
/// realtime for 46 elements.
#[test]
#[ignore = "timing; run with --ignored --nocapture in release"]
fn sequencer_cost_against_the_incumbent() {
    for (name, elems) in [
        ("A shiftreg ring", seq_shiftreg([0.25, 0.5, 0.75, 0.95])),
        ("B counter+mux", seq_counter_mux([0.2, 0.45, 0.7, 0.95])),
    ] {
        let n = elems.len();
        let mut e = eng_with(&elems);
        e.advance(20_000);
        let f0 = e.factorizations();
        let steps = 200_000u32;
        let t = Instant::now();
        let rep = e.advance(steps);
        let el = t.elapsed().as_secs_f64();
        let us = el / f64::from(steps) * 1e6;
        println!(
            "\n  {name}: {n} elements, {:.3} µs/substep, {:.2}x realtime, {:.4} factorizations/substep, {} rescues, quarantined={}",
            us,
            f64::from(steps) * DT / el,
            (e.factorizations() - f0) as f64 / f64::from(steps),
            rep.rescues,
            e.is_quarantined()
        );
    }
}
