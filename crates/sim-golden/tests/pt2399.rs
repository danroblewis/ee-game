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
            // [IN, OUT, VCO, GND, OP1-IN/OUT, OP2-IN/OUT, LPF1-IN/OUT,
            //  LPF2-IN/OUT]. The op-amps and filters are left unwired here:
            //  these tests are about the delay, and an unwired stage must be
            //  harmless.
            pins: vec![
                (0, 0), (8, 0), (0, 8), (8, 8),
                (0, 12), (8, 12), (0, 16), (8, 16),
                (0, 20), (8, 20), (0, 24), (8, 24),
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
/// The chip's signal path rests at REF, so the line is flushed AT REF and
/// then stepped away from it. Measuring against 0 V — which is what this did
/// first — reports every delay as zero, because the output is already past
/// the threshold before the step is applied.
const REST: f64 = sim_core::PT_V_RT;
const STEP: f64 = REST + 2.0;
const TRIP: f64 = REST + 1.0;

fn measure(rt_ohms: f64) -> f64 {
    let mut eng = Engine::new(DT);
    let mut d = rig(rt_ohms, REST);
    eng.set_elements(&d);
    let want = expected(rt_ohms);
    eng.advance((want / DT * 1.5) as u32);
    let mut d2 = rig(rt_ohms, STEP);
    d2[1].id = 2;
    d = d2;
    eng.set_elements(&d);
    let t0 = eng.time();
    for _ in 0..((want / DT * 3.0) as u32) {
        eng.advance(1);
        if eng.voltage_at((8, 0)).unwrap() > TRIP {
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
    let float = rig(0.0, STEP);
    let mut d: Vec<sim_core::ElementSpec> = float.into_iter().filter(|e| e.id != 4 && e.id != 5).collect();
    assert_eq!(sim_core::check_document(&d, DT), Ok(()), "a floating RT must still be a legal part");
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(150_000); // three seconds
    // With no clock the chain never shifts, so the output holds the level it
    // powered up at — the chip's own reference — and the input never reaches
    // it. "Passes nothing" means "stays at rest", not "sits at zero".
    // Near rest, not exactly at it: the output has a real source impedance,
    // so a 100 kΩ load pulls 2.5 V down to 2.475. That divider is physics,
    // not slop, and a 1e-6 tolerance was asserting the output was ideal.
    assert!(
        (eng.voltage_at((8, 0)).unwrap() - REST).abs() < REST * 0.02,
        "an unwired VCO must pass NOTHING and hold near rest ({REST} V) — got {} V",
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
        if e2.voltage_at((8, 0)).unwrap() > TRIP {
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

/// AN OVERDRIVEN ECHO MUST CLIP, NOT RUN AWAY.
///
/// The repeats path is a feedback loop around the delay. Below unity it
/// decays; at or above unity it grows — and what a real chip does then is
/// SATURATE into a howl. Without clipping it grows without bound instead,
/// which cooks the part and eventually quarantines the room, and that was
/// the state this shipped in.
#[test]
fn too_much_feedback_clips_instead_of_running_away() {
    // 4.7 kΩ of repeats against a 10 kΩ mixer leg is a loop gain above 2 —
    // far past what any real echo would take.
    let mut d = vec![
        sim_core::ElementSpec {
            id: 1,
            kind: K::Pt2399,
            pins: vec![(0,0),(10,0),(0,2),(10,2),(0,5),(10,5),(0,7),(10,7),
                       (0,9),(10,9),(0,11),(10,11)],
            ..Default::default()
        },
        spec(2, r(10_000.0), (0, 2), (0, 24)),
        gnd(3, (0, 24)),
        gnd(4, (10, 2)),
        // A kick into OP1's summing junction, then silence.
        spec(5, K::VoltageSource { dc: 4.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }, (30, 5), (30, 20)),
        gnd(6, (30, 20)),
        spec(7, r(10_000.0), (30, 5), (0, 5)),
        spec(8, r(10_000.0), (0, 5), (10, 5)),      // OP1 feedback
        spec(9, K::Wire, (10, 5), (0, 0)),           // OP1 out -> delay in
        spec(10, r(10_000.0), (10, 0), (0, 7)),      // delay out -> OP2
        spec(11, r(10_000.0), (0, 7), (10, 7)),      // OP2 feedback
        spec(12, r(4_700.0), (10, 0), (0, 5)),       // RUNAWAY repeats
        spec(13, r(100_000.0), (10, 7), (10, 20)),
        gnd(14, (10, 20)),
    ];
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(20_000);
    if let K::VoltageSource { ref mut dc, .. } = d[4].kind {
        *dc = 2.5;
    }
    eng.set_elements(&d);

    // Five seconds of a loop that wants to grow forever.
    let mut worst = 0.0f64;
    for _ in 0..250 {
        eng.advance(1_000);
        for pt in [(10, 5), (10, 7), (10, 0)] {
            worst = worst.max(eng.voltage_at(pt).unwrap().abs());
        }
    }
    assert!(!eng.is_quarantined(), "an overdriven echo must not take the room down");
    // Every node in the chip is bounded by its own rails. Without clipping
    // this reached hundreds of volts and climbing.
    assert!(
        worst < 6.0,
        "an overdriven echo should saturate at the rails, not run away — peaked at {worst:.1} V"
    );
    println!("overdriven echo peaked at {worst:.2} V (rails are {} .. {})",
        sim_core::PT_OA_LO, sim_core::PT_OA_HI);
}

/// THE FILTERS ARE REAL FILTERS, built the way the datasheet builds them.
///
/// LPF1 and LPF2 are not fixed corners inside the chip — they are op-amp
/// stages whose response comes from CAPACITORS THE BUILDER ADDS, which is
/// why the application circuit is covered in 3900 pF and 0.082 uF parts
/// around pins 13-16. This wires one as a first-order low pass (R in, R||C
/// feedback) and measures the rolloff, because "we exposed some pins" is not
/// the same claim as "a filter can be built on them".
///
/// It matters more here than anywhere else in the game: this is the first
/// part where ALIASING IS REAL, so these pins are the difference between a
/// delay and a mess.
#[test]
fn a_filter_built_on_lpf1_actually_filters() {
    // Inverting stage: 10k in, 10k feedback with 1.6 nF across it.
    // f_c = 1/(2*pi*R*C) = 9.9 kHz.
    let rig = |hz: f64| -> f64 {
        let d = vec![
            sim_core::ElementSpec {
                id: 1,
                kind: K::Pt2399,
                pins: vec![(0,0),(10,0),(0,2),(10,2),(0,5),(10,5),(0,7),(10,7),
                           (0,9),(10,9),(0,11),(10,11)],
                ..Default::default()
            },
            spec(2, r(10_000.0), (0, 2), (0, 30)),
            gnd(3, (0, 30)),
            gnd(4, (10, 2)),
            // Signal into LPF1-IN through 10k, biased at the chip's rest.
            spec(5, K::VoltageSource { dc: sim_core::PT_V_RT, amp: 0.5, hz, phase: 0.0, wave: Wave::Sine }, (30, 9), (30, 30)),
            gnd(6, (30, 30)),
            spec(7, r(10_000.0), (30, 9), (0, 9)),
            spec(8, r(10_000.0), (0, 9), (10, 9)),          // feedback R
            spec(9, K::Capacitor { farads: 1.6e-9 }, (0, 9), (10, 9)), // feedback C
            spec(10, r(100_000.0), (10, 9), (10, 30)),
            gnd(11, (10, 30)),
        ];
        let mut eng = Engine::new(DT);
        eng.set_elements(&d);
        eng.advance((0.02 / DT) as u32);
        let (mut hi, mut lo) = (f64::MIN, f64::MAX);
        for _ in 0..((5.0 / hz / DT) as u32).max(2_000) {
            eng.advance(1);
            let v = eng.voltage_at((10, 9)).unwrap();
            hi = hi.max(v);
            lo = lo.min(v);
        }
        assert!(!eng.is_quarantined(), "{hz} Hz quarantined");
        hi - lo
    };
    // Well below the corner the stage is unity-inverting: 1.0 V p-p in,
    // ~1.0 V p-p out. An octave and two octaves above it, a first-order
    // pole gives -6 dB and -12 dB.
    // EVERY PROBE STAYS UNDER THE SIMULATOR'S OWN NYQUIST. At dt = 20 us the
    // engine samples at 50 kHz, so 25 kHz is the ceiling — and a first
    // attempt at this probed 40 kHz, which aliased down to 10 kHz and came
    // back showing LESS attenuation than 20 kHz. The filter was fine; the
    // measurement was above the sample rate. Corner here is 9.9 kHz.
    let pass = rig(500.0);
    let mid = rig(5_000.0);
    let stop = rig(20_000.0);
    println!("  500 Hz {pass:.4} V   5 kHz {mid:.4} V   20 kHz {stop:.4} V");
    println!(
        "  -> {:.1} dB at 5 kHz, {:.1} dB at 20 kHz",
        20.0 * (mid / pass).log10(),
        20.0 * (stop / pass).log10()
    );
    assert!(pass > 0.8, "the passband should come through, got {pass:.4} V p-p");
    assert!(mid < pass, "5 kHz should already be down: {mid:.4} vs {pass:.4}");
    assert!(
        stop < mid * 0.7,
        "20 kHz should be well down on 5 kHz: {stop:.4} vs {mid:.4}"
    );
}
