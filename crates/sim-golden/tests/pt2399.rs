//! The PT2399: one resistor, one delay.
//!
//! The claim under test is the whole design — that the delay is a
//! CONSEQUENCE of the resistor the player wired, not a field. So these
//! measure the delay for several resistors and check it against
//!     t = PT_STAGES / (PT_HZ_PER_AMP * PT_V_RT / R)

use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;

const DT: f64 = 20e-6;

/// Pins [IN, OUT, RT, GND]. `rt_ohms` is the player's delay resistor.
fn rig(rt_ohms: f64, dc: f64) -> Vec<sim_core::ElementSpec> {
    vec![
        sim_core::ElementSpec {
            id: 1,
            kind: K::Pt2399,
            pins: vec![(0, 0), (8, 0), (0, 8), (8, 8)],
            ..Default::default()
        },
        spec(2, K::VoltageSource { dc, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }, (0, 0), (0, 16)),
        gnd(3, (0, 16)),
        // THE DELAY KNOB: a plain resistor from RT to ground.
        spec(4, r(rt_ohms), (0, 8), (0, 24)),
        gnd(5, (0, 24)),
        spec(6, r(100_000.0), (8, 0), (8, 16)),
        gnd(7, (8, 16)),
        gnd(8, (8, 8)),
    ]
}

fn expected(rt_ohms: f64) -> f64 {
    let i = sim_core::PT_V_RT / rt_ohms;
    let f = (sim_core::PT_HZ_PER_AMP * i).min(1.0 / DT);
    f64::from(sim_core::PT_STAGES) / f
}

/// Measure the time from a step at IN to its arrival at OUT.
fn measure(rt_ohms: f64) -> f64 {
    let mut eng = Engine::new(DT);
    let mut d = rig(rt_ohms, 0.0);
    eng.set_elements(&d);
    let want = expected(rt_ohms);
    eng.advance((want / DT * 1.5) as u32);
    let mut d2 = rig(rt_ohms, 3.0);
    d2[1].id = 2;
    d = d2;
    eng.set_elements(&d);
    let t0 = eng.time();
    for _ in 0..((want / DT * 3.0) as u32) {
        eng.advance(1);
        if eng.voltage_at((8, 0)).unwrap() > 1.5 {
            return eng.time() - t0;
        }
    }
    panic!("{rt_ohms} Ω: nothing arrived (expected {want:.4} s)");
}

#[test]
fn a_pt2399_is_a_legal_document() {
    assert_eq!(sim_core::check_document(&rig(20_000.0, 1.0), DT), Ok(()));
}

/// THE HEADLINE: the resistor sets the delay, and the numbers are the
/// datasheet's. 5 kΩ should be tens of ms; 50 kΩ should be hundreds.
#[test]
fn the_resistor_sets_the_delay() {
    for rt in [5_000.0, 10_000.0, 22_000.0, 50_000.0] {
        let got = measure(rt);
        let want = expected(rt);
        assert!(
            (got - want).abs() < want * 0.15,
            "{rt} Ω: measured {got:.4} s, model says {want:.4} s"
        );
        println!("{rt:>7.0} Ω -> {:.1} ms", got * 1000.0);
    }
}

/// And the direction is the one the datasheet promises: MORE resistance is
/// LESS current is a slower clock is a LONGER delay. A part that got this
/// backwards would still pass a single-point check.
#[test]
fn more_resistance_is_more_delay() {
    let short = measure(5_000.0);
    let long = measure(50_000.0);
    assert!(
        long > short * 5.0,
        "10x the resistance should be ~10x the delay: {short:.4} s -> {long:.4} s"
    );
    // ...and the span covers what a real PT2399 covers.
    assert!(
        (0.020..0.060).contains(&short) && (0.250..0.450).contains(&long),
        "range should be roughly the chip's 30-340 ms, got {:.1} ms .. {:.1} ms",
        short * 1000.0,
        long * 1000.0
    );
}

/// THE THREE WAYS SOMEBODY ACTUALLY WIRES THIS, all of which must behave
/// sensibly. Two of them did not, and that is why the RT reference has a
/// real internal impedance now:
///
///   * RT LEFT FLOATING used to leak a trickle through the input tether and
///     produce a SIX-SECOND delay — a part that looks broken rather than
///     unwired. It must draw exactly nothing and pass exactly nothing.
///   * RT TIED STRAIGHT TO GROUND used to be two contradictory constraints
///     on one node, so the placement gate refused the edit as `Unsolvable`.
///     Tying a pin to ground is the FIRST thing anybody tries, and being
///     refused with no reason is the worst answer available. It must be
///     legal, and it must mean the shortest delay.
///   * RT through a resistor is the documented way and must land on the
///     model.
#[test]
fn every_plausible_wiring_of_rt_behaves() {
    // 1. Floating: legal, and silent — not a mystery delay.
    let float = rig(0.0, 3.0);
    let mut d: Vec<sim_core::ElementSpec> = float.into_iter().filter(|e| e.id != 4 && e.id != 5).collect();
    assert_eq!(sim_core::check_document(&d, DT), Ok(()), "a floating RT must still be a legal part");
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(150_000); // three seconds
    assert!(
        eng.voltage_at((8, 0)).unwrap().abs() < 1e-9,
        "an unwired RT must pass NOTHING, not a six-second delay — got {} V",
        eng.voltage_at((8, 0)).unwrap()
    );

    // 2. Straight to ground: legal, and the shortest delay the part has.
    d.push(gnd(9, (0, 8)));
    assert_eq!(
        sim_core::check_document(&d, DT),
        Ok(()),
        "RT tied to ground must be an ordinary circuit, not Unsolvable"
    );
    let mut e2 = Engine::new(DT);
    e2.set_elements(&d);
    let floor = f64::from(sim_core::PT_STAGES) * DT;
    let mut got = None;
    for k in 1..100_000u32 {
        e2.advance(1);
        if e2.voltage_at((8, 0)).unwrap() > 1.5 {
            got = Some(f64::from(k) * DT);
            break;
        }
    }
    let got = got.expect("RT grounded should give the shortest delay, not silence");
    assert!(
        (got - floor).abs() < floor * 0.2,
        "RT grounded should clamp to the shortest delay {floor:.4} s, got {got:.4} s"
    );
}
