//! The bucket brigade, against its closed form.
//!
//! A delay line is unusually testable: its exact answer is
//!
//!     out(t) = in(t - stages/(2*f_clock))
//!
//! so these are not "it looks delayed" checks. They put a known signal in,
//! find it coming out, and compare the time it took against the formula.

use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;

const DT: f64 = 20e-6;

/// A BBD with its clock driven by an ideal square wave, its input driven by
/// a source, and its output loaded so the node is real.
///
/// Pins are [IN, OUT, CLK, GND].
fn rig(stages: u16, clk_hz: f64, input: K) -> Vec<sim_core::ElementSpec> {
    vec![
        // The delay itself.
        sim_core::ElementSpec {
            id: 1,
            kind: K::Bbd { stages },
            pins: vec![(0, 0), (8, 0), (0, 8), (8, 8)],
            ..Default::default()
        },
        // IN, driven.
        spec(2, input, (0, 0), (0, 16)),
        gnd(3, (0, 16)),
        // CLK: a 0/5 V square. `dc` sits at the midpoint so the wave crosses
        // both Schmitt thresholds.
        spec(
            4,
            K::VoltageSource {
                dc: 2.5,
                amp: 2.5,
                hz: clk_hz,
                phase: 0.0,
                wave: Wave::Square,
            },
            (0, 8),
            (0, 24),
        ),
        gnd(5, (0, 24)),
        // OUT into a load, and the part's own ground pin to ground.
        spec(6, r(10_000.0), (8, 0), (8, 16)),
        gnd(7, (8, 16)),
        gnd(8, (8, 8)),
    ]
}

/// Volts at the output node.
fn out_v(eng: &Engine) -> f64 {
    eng.voltage_at((8, 0)).unwrap()
}

#[test]
fn a_bucket_brigade_is_a_legal_document() {
    assert_eq!(
        sim_core::check_document(&rig(64, 5000.0, K::VoltageSource { dc: 1.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine }), DT),
        Ok(())
    );
    // The bounds are the document's problem and are refused, not clamped.
    for bad in [0u16, 1, 4097] {
        let d = rig(bad, 5000.0, K::VoltageSource { dc: 1.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine });
        assert!(
            sim_core::check_document(&d, DT).is_err(),
            "{bad} stages should be refused"
        );
    }
}

/// THE HEADLINE. A step change on the input must appear at the output after
/// exactly stages/(2*f_clock) seconds — the datasheet formula, measured.
#[test]
fn the_delay_is_the_formula() {
    // Several (stages, clock) pairs, so a coincidence cannot pass.
    for (stages, clk) in [(64u16, 5000.0), (64, 2500.0), (128, 5000.0), (256, 10000.0)] {
        let mut eng = Engine::new(DT);
        // DC input, initially 0 V, switched to 2 V by a switch we flip.
        let mut d = rig(
            stages,
            clk,
            K::VoltageSource { dc: 0.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine },
        );
        eng.set_elements(&d);

        // Let the (zero) input propagate all the way through, so the line is
        // full of a known value rather than of its power-up state.
        let expect = f64::from(stages) / (2.0 * clk);
        let fill = (expect / DT * 1.5) as u32;
        eng.advance(fill);
        assert!(out_v(&eng).abs() < 1e-6, "line should be flushed to 0 V");

        // Now step the input to 2 V and find the output edge.
        if let K::VoltageSource { ref mut dc, .. } = d[1].kind {
            *dc = 2.0;
        }
        eng.set_elements(&d);
        let t0 = eng.time();
        let mut arrived = None;
        for _ in 0..(fill * 2) {
            eng.advance(1);
            if out_v(&eng) > 1.0 {
                arrived = Some(eng.time() - t0);
                break;
            }
        }
        let got = arrived.unwrap_or_else(|| panic!("{stages} stages @ {clk} Hz: nothing came out"));
        // Tolerance is one clock period: the edge lands on whichever substep
        // the clock transition fell on, and a half-period of phase is the
        // honest resolution of a sampled device.
        let tol = 1.0 / clk;
        assert!(
            (got - expect).abs() <= tol,
            "{stages} stages @ {clk} Hz: delay {got:.6} s, formula says {expect:.6} s"
        );
    }
}

/// Doubling the clock halves the delay. This is what makes the CLOCK A PIN
/// worth the trouble: modulate it and you have a flanger.
#[test]
fn the_clock_sets_the_delay() {
    let measure = |clk: f64| -> f64 {
        let mut eng = Engine::new(DT);
        let mut d = rig(64, clk, K::VoltageSource { dc: 0.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine });
        eng.set_elements(&d);
        let expect = 64.0 / (2.0 * clk);
        eng.advance((expect / DT * 1.5) as u32);
        if let K::VoltageSource { ref mut dc, .. } = d[1].kind {
            *dc = 2.0;
        }
        eng.set_elements(&d);
        let t0 = eng.time();
        for _ in 0..200_000 {
            eng.advance(1);
            if out_v(&eng) > 1.0 {
                return eng.time() - t0;
            }
        }
        panic!("no output at {clk} Hz");
    };
    let slow = measure(2500.0);
    let fast = measure(5000.0);
    let ratio = slow / fast;
    assert!(
        (ratio - 2.0).abs() < 0.15,
        "doubling the clock should halve the delay: {slow:.6} / {fast:.6} = {ratio:.3}"
    );
}

/// A delay line with no clock is a delay line that does nothing. It must not
/// pass signal through, and it must not blow up either.
#[test]
fn no_clock_means_no_output() {
    let mut eng = Engine::new(DT);
    let mut d = rig(64, 5000.0, K::VoltageSource { dc: 3.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine });
    // Kill the clock: a DC level below the Schmitt low threshold.
    if let K::VoltageSource { ref mut dc, ref mut amp, ref mut hz, .. } = d[3].kind {
        *dc = 0.0;
        *amp = 0.0;
        *hz = 0.0;
    }
    eng.set_elements(&d);
    eng.advance(50_000);
    assert!(!eng.is_quarantined(), "an unclocked BBD must not quarantine");
    assert!(
        out_v(&eng).abs() < 1e-6,
        "an unclocked BBD holds its last sample (0 V here), got {}",
        out_v(&eng)
    );
}

/// The chain must SURVIVE an unrelated edit. A player wiring a resistor in
/// across the room while an echo is ringing must not have the echo emptied,
/// and this is the one property the side-table-keyed-by-id design exists to
/// give.
#[test]
fn an_unrelated_edit_does_not_empty_the_line() {
    let mut eng = Engine::new(DT);
    let mut d = rig(256, 5000.0, K::VoltageSource { dc: 4.0, amp: 0.0, hz: 0.0, phase: 0.0, wave: Wave::Sine });
    eng.set_elements(&d);
    // Part-fill the line: less than a full delay, so the 4 V has NOT yet
    // reached the output.
    let expect = 256.0 / (2.0 * 5000.0);
    eng.advance((expect / DT * 0.4) as u32);
    assert!(out_v(&eng).abs() < 1e-6, "4 V should still be in flight");

    // An edit that has nothing to do with the delay.
    d.push(spec(99, r(1000.0), (40, 40), (40, 48)));
    d.push(gnd(100, (40, 48)));
    eng.set_elements(&d);

    // THE WINDOW IS THE TEST. Samples entered the line from t=0, so the
    // first 4 V reaches the output one full delay after that — i.e. 0.6 of a
    // delay after the edit. A line that the edit RESET would have to fill
    // from scratch and could not deliver anything for a full 1.0. Waiting
    // 0.7 therefore passes only if the in-flight samples survived.
    //
    // (Waiting 2.0, which is what this test did first, passes either way —
    // it gave a re-created line all the time it needed, and a mutation that
    // dropped every chain on recompile sailed through it.)
    let mut got = false;
    for _ in 0..(expect / DT * 0.7) as u32 {
        eng.advance(1);
        if out_v(&eng) > 3.0 {
            got = true;
            break;
        }
    }
    assert!(got, "the edit emptied the delay line — the samples never arrived");
}
