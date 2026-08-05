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
            // [IN, OUT, VCO, GND, OP1-IN, OP1-OUT, OP2-IN, OP2-OUT].
            // The op-amps are left unwired here: these tests are about the
            // delay, and an unwired op-amp must be harmless.
            pins: vec![
                (0, 0),
                (8, 0),
                (0, 8),
                (8, 8),
                (0, 12),
                (8, 12),
                (0, 16),
                (8, 16),
            ],
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

/// THE HEADLINE, and it is now checked against the DATASHEET'S OWN TABLE 1
/// rather than against a law I fitted. Princeton print resistor, clock and
/// delay for 28 settings; these are five of them spread across the range.
///
/// This matters more than it looks. The first version of this part used a
/// 1/R law, which is the obvious guess and is WRONG: the real table's clock
/// saturates at 22 MHz as R goes to zero, so there is resistance inside the
/// pin. Fitting `f = K/(R + R0)` at the two ENDS gives R0 = 2.76 kΩ — and
/// that constant then predicts the middle of the table, which is the only
/// evidence that the shape is right and not just bent to fit.
#[test]
fn the_delay_matches_the_datasheet_table() {
    // (external resistor Ω, printed delay ms) from PT2399 v1.4 Table 1.
    const TABLE: [(f64, f64); 5] = [
        (27_600.0, 342.0),
        (10_500.0, 151.0),
        (5_400.0, 92.2),
        (2_400.0, 56.6),
        (0.5, 31.3),
    ];
    let mut worst: f64 = 0.0;
    for (rt, printed) in TABLE {
        let got = measure(rt) * 1000.0;
        let err = (got - printed).abs() / printed * 100.0;
        println!("{rt:>8.1} Ω: printed {printed:>6.1} ms, model {got:>6.1} ms  ({err:.1} %)");
        worst = worst.max(err);
        assert!(
            err < 10.0,
            "{rt} Ω: datasheet says {printed} ms, model gives {got:.1} ms ({err:.1} % out)"
        );
    }
    println!("worst error across the table: {worst:.1} %");
}

/// And the direction is the one the datasheet promises: MORE resistance is
/// LESS current is a slower clock is a LONGER delay. A part that got this
/// backwards would still pass a single-point check.
#[test]
fn more_resistance_is_more_delay() {
    // Both ends of the datasheet's own range, rather than two numbers I
    // picked: R = 0 is its fastest setting and 27.6 kΩ its slowest.
    let short = measure(0.5);
    let long = measure(27_600.0);
    assert!(
        long > short * 5.0,
        "the slow end should be many times the fast end: {short:.4} s -> {long:.4} s"
    );
    // ...and the span covers what a real PT2399 covers: the datasheet's
    // own range is 31.3 ms at R=0 to 342 ms at 27.6 kΩ.
    assert!(
        (0.028..0.035).contains(&short) && (0.320..0.365).contains(&long),
        "the span should be the datasheet's 31.3 .. 342 ms, got {:.1} .. {:.1} ms",
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
    // The datasheet's fastest setting, R = 0.5 Ω, is 31.3 ms. Note this is
    // NOT the engine's substep floor (PT_STAGES * dt = 20.5 ms): with the
    // real internal resistance in the model, the chip reaches its own
    // minimum by physics and the clamp never engages anywhere in the legal
    // range. That is the outcome to protect.
    let floor = 0.0313;
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
        "VCO grounded should give the datasheet's fastest {floor:.4} s, got {got:.4} s"
    );
}
