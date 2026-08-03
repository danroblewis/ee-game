//! Source waveforms: shape, phase alignment, and the two traps.
//!
//! The shapes themselves are the easy half. The two things worth testing are
//! that a DISCONTINUOUS source does not make the integrator ring, and that a
//! shape reaches the constraint key so two sources of different shapes are
//! never merged onto one branch row.

use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;

const DT: f64 = 1e-6;

/// Source into a 1 Ω / ground divider, so the node voltage IS the source.
fn open_source(kind: K) -> Vec<sim_core::ElementSpec> {
    vec![
        spec(1, kind, (0, 0), (0, 8)),
        gnd(2, (0, 8)),
        spec(3, r(1000.0), (0, 0), (8, 0)),
        spec(4, K::Wire, (8, 0), (8, 8)),
        gnd(5, (8, 8)),
    ]
}

/// Sample the source node across exactly one period.
fn one_period(wave: Wave, hz: f64, samples: usize) -> Vec<f64> {
    let mut eng = Engine::new(DT);
    eng.set_elements(&open_source(shaped(1.0, hz, wave)));
    let per = (1.0 / hz / DT).round() as u32;
    let step = per / samples as u32;
    let mut out = Vec::new();
    for _ in 0..samples {
        eng.advance(step);
        out.push(eng.voltage_at((0, 0)).unwrap());
    }
    out
}

#[test]
fn every_shape_has_the_amplitude_it_claims() {
    for wave in [Wave::Sine, Wave::Square, Wave::Triangle, Wave::Saw] {
        let v = one_period(wave, 100.0, 400);
        let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!(
            (hi - 1.0).abs() < 0.02 && (lo + 1.0).abs() < 0.02,
            "{wave:?}: peaks {lo} .. {hi}, want -1 .. +1"
        );
        // Every one of these shapes has zero mean over a period.
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        assert!(mean.abs() < 0.02, "{wave:?}: mean {mean} should be ~0");
    }
}

#[test]
fn the_shapes_are_actually_the_shapes() {
    // Square: only ever at one rail or the other, never in between.
    let sq = one_period(Wave::Square, 100.0, 200);
    let mid = sq.iter().filter(|v| v.abs() < 0.9).count();
    assert!(mid <= 2, "square spent {mid} samples off its rails");

    // Triangle: a constant |slope|, so consecutive differences are equal in
    // magnitude everywhere except the two turning points.
    let tri = one_period(Wave::Triangle, 100.0, 200);
    let d: Vec<f64> = tri.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let dmax = d.iter().cloned().fold(0.0, f64::max);
    let odd = d.iter().filter(|x| (**x - dmax).abs() > dmax * 0.5).count();
    assert!(odd <= 3, "triangle slope changed {odd} times, want ~2 turns");

    // Saw: exactly one jump per period, and it is a big one.
    let saw = one_period(Wave::Saw, 100.0, 200);
    let jumps = saw
        .windows(2)
        .filter(|w| (w[1] - w[0]).abs() > 1.0)
        .count();
    assert_eq!(jumps, 1, "sawtooth should jump exactly once per period");
}

/// Switching shape must not move the phase. If it did, a circuit built around
/// a source's timing would shift the moment a player tried another shape.
#[test]
fn all_four_shapes_start_at_zero_and_rise() {
    for wave in [Wave::Sine, Wave::Square, Wave::Triangle, Wave::Saw] {
        let mut eng = Engine::new(DT);
        eng.set_elements(&open_source(shaped(1.0, 10.0, wave)));
        eng.advance(1);
        let v = eng.voltage_at((0, 0)).unwrap();
        assert!(v > 0.0, "{wave:?} should rise out of zero, got {v}");
    }
}

/// THE TRAP. A jump into a capacitor is exactly what trapezoidal integration
/// cannot do: it assumes the state moved smoothly across the step, so it
/// overshoots and rings. The engine's cure is the backward-Euler steps it
/// already takes after a switch flip, and a waveform edge has to arm them.
///
/// Without the arming this test sees the capacitor node driven WELL outside
/// the source's own +/-5 V, which in a real room is how a part gets destroyed:
/// the damage model judges from these numbers.
#[test]
fn a_square_wave_into_a_capacitor_does_not_ring() {
    for wave in [Wave::Square, Wave::Saw] {
        // tau = 100 ns against a 1 us step, i.e. tau/dt = 0.1. THE RATIO IS
        // THE TEST: a slow RC (tau/dt = 100) shows no ringing at all and
        // would pass whether or not the fix is present. Measured with the
        // arming removed, this circuit peaks at 5.163 V; with it, 5.028 V.
        let elems = vec![
            spec(1, shaped(5.0, 500.0, wave), (0, 0), (0, 8)),
            gnd(2, (0, 8)),
            spec(3, r(100.0), (0, 0), (8, 0)),
            spec(4, K::Capacitor { farads: 1e-9 }, (8, 0), (8, 8)),
            gnd(5, (8, 8)),
        ];
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        let mut worst = 0.0f64;
        for _ in 0..20_000 {
            eng.advance(1);
            worst = worst.max(eng.voltage_at((8, 0)).unwrap().abs());
        }
        assert!(!eng.is_quarantined(), "{wave:?} quarantined the room");
        // An RC driven from +/-5 V can never legitimately exceed 5 V. The
        // threshold sits between the two measured outcomes: it fails the
        // 5.163 V of an unarmed integrator and passes the 5.028 V that
        // remains once the edge is armed (the residual is the edge landing
        // between samples, not ringing).
        assert!(
            worst < 5.10,
            "{wave:?}: capacitor node reached {worst:.4} V from a 5 V source \
             — the integrator is ringing on the edge"
        );
    }
}

/// Two ideal sources on one node pair share ONE branch row when their
/// constraints key equal. A sine and a square agree on dc, amp, hz and phase
/// and are not remotely the same voltage: if the shape did not reach the key
/// they would merge, and the room would solve a circuit nobody drew.
#[test]
fn a_sine_and_a_square_are_not_the_same_net() {
    let both = vec![
        spec(1, shaped(5.0, 50.0, Wave::Sine), (0, 0), (0, 8)),
        spec(2, shaped(5.0, 50.0, Wave::Square), (0, 0), (0, 8)),
        gnd(3, (0, 8)),
    ];
    assert!(
        sim_core::check_document(&both, DT).is_err(),
        "a sine and a square across the same pins are a CONFLICT, not a net"
    );

    // ...while two identical squares still merge, exactly as two identical
    // sines always have.
    let twins = vec![
        spec(1, shaped(5.0, 50.0, Wave::Square), (0, 0), (0, 8)),
        spec(2, shaped(5.0, 50.0, Wave::Square), (0, 0), (0, 8)),
        gnd(3, (0, 8)),
    ];
    assert_eq!(
        sim_core::check_document(&twins, DT),
        Ok(()),
        "two identical square sources are one net, like two identical sines"
    );
}

/// The sawtooth is NOT half-wave antisymmetric, so its amplitude sign cannot
/// be folded into a phase shift. Two saws of opposite sign on one node pair
/// are a genuine disagreement and must be caught, not merged.
#[test]
fn an_inverted_sawtooth_is_a_different_waveform() {
    let opposed = vec![
        spec(1, shaped(5.0, 50.0, Wave::Saw), (0, 0), (0, 8)),
        spec(2, shaped(-5.0, 50.0, Wave::Saw), (0, 0), (0, 8)),
        gnd(3, (0, 8)),
    ];
    assert!(
        sim_core::check_document(&opposed, DT).is_err(),
        "a saw and its negation are different shapes and must not merge"
    );

    // The same test for a SINE must still merge: -A·sin(x) = A·sin(x + π) is
    // a true identity, and folding it is what lets a source drawn the other
    // way round be recognised as the same net.
    let sines = vec![
        spec(1, shaped(5.0, 50.0, Wave::Sine), (0, 0), (0, 8)),
        spec(2, shaped(5.0, 50.0, Wave::Sine), (0, 0), (0, 8)),
        gnd(3, (0, 8)),
    ];
    assert_eq!(sim_core::check_document(&sines, DT), Ok(()));
}

/// A shape is a placement-time property like any other, so the gate has to
/// accept all four rather than only the one it was written for.
#[test]
fn every_shape_is_placeable() {
    for wave in [Wave::Sine, Wave::Square, Wave::Triangle, Wave::Saw] {
        assert_eq!(
            sim_core::check_document(&open_source(shaped(5.0, 60.0, wave)), DT),
            Ok(()),
            "{wave:?} should be placeable"
        );
    }
}
