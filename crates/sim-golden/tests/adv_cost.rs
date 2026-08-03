//! COST AND CLASSIFICATION AUDIT.
//!
//! Two questions. Do the logic parts really get factorization reuse (against
//! the counterfactual of forcing them to refactor every substep)? And is the
//! reused matrix bitwise what a refactor would have produced — because reuse
//! that changes an answer is not an optimisation, it is a bug.

use sim_core::{ElementKind as K, ElementSpec, Engine, GateOp};
use sim_golden::*;
use std::time::Instant;

const DT: f64 = 20e-6;

fn eng_with(elems: &[ElementSpec]) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(elems);
    e
}

/// `n` independent self-correcting 4-bit ring counters off one shared square
/// clock, each with a 10 kΩ load on every output. 6 elements per ring plus a
/// 4-element clock, so `n = 16` is a 100-element digital room.
fn digital_room(n: i32, clock_hz: f64) -> Vec<ElementSpec> {
    let mut v = vec![
        spec(1, dc(5.0), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        spec(
            3,
            K::VoltageSource {
                dc: 2.5,
                amp: 2.5,
                hz: clock_hz,
                phase: 0.0,
            },
            (4, 12),
            (0, 24),
        ),
        ElementSpec::pins(
            4,
            K::Gate {
                op: GateOp::Buf,
                ins: 1,
            },
            &[(0, 0), (0, 24), (4, 12), (10, 12)],
        ),
    ];
    let mut id = 100u32;
    for k in 0..n {
        let x = 40 + 40 * k;
        let q = |j: i32| (x + 20, 4 + 6 * j);
        let ser = (x, 30);
        v.push(ElementSpec::pins(
            id,
            K::ShiftReg { bits: 4 },
            &[
                (0, 0),
                (0, 24),
                (10, 12),
                ser,
                (0, 0),
                q(0),
                q(1),
                q(2),
                q(3),
            ],
        ));
        v.push(ElementSpec::pins(
            id + 1,
            K::Gate {
                op: GateOp::Nor,
                ins: 3,
            },
            &[(0, 0), (0, 24), q(0), q(1), q(2), ser],
        ));
        for j in 0..4 {
            v.push(spec(id + 2 + j as u32, r(10_000.0), q(j), (0, 24)));
        }
        id += 10;
    }
    v
}

fn bench(name: &str, elems: &[ElementSpec], reuse: bool, steps: u32) -> (f64, f64, u64) {
    let mut e = eng_with(elems);
    e.set_reuse_pwl(reuse);
    e.advance(2000);
    let f0 = e.factorizations();
    let t = Instant::now();
    let rep = e.advance(steps);
    let el = t.elapsed().as_secs_f64();
    let us = el / f64::from(steps) * 1e6;
    let rt = f64::from(steps) * DT / el;
    let fac = e.factorizations() - f0;
    assert!(!e.is_quarantined(), "{name} quarantined");
    assert_eq!(rep.rescues, 0, "{name} needed rescues");
    println!(
        "  {name:<38} | {:>3} elems | {us:>7.3} | {rt:>7.2}x | {:>6.4} | {:.3}",
        elems.len(),
        fac as f64 / f64::from(steps),
        f64::from(rep.nr_iters) / f64::from(steps)
    );
    (us, rt, fac)
}

#[test]
#[ignore = "timing; cargo test --release -- --ignored --nocapture"]
fn cost_of_a_hundred_element_digital_room() {
    println!("\n  room                                   |   size   | µs/step | realtime | fac/step | nr/step");
    let steps = 100_000u32;
    for hz in [0.0f64, 1_000.0] {
        let r = digital_room(16, if hz == 0.0 { 0.001 } else { hz });
        let tag = if hz == 0.0 { "16 rings, clock ~static" } else { "16 rings, 1 kHz clock" };
        let (on_us, on_rt, on_f) = bench(&format!("{tag} [reuse ON]"), &r, true, steps);
        let (off_us, _, off_f) = bench(&format!("{tag} [reuse OFF]"), &r, false, steps);
        println!(
            "  -> reuse is worth {:.2}x here ({on_f} vs {off_f} factorizations); realtime {on_rt:.2}x\n",
            off_us / on_us
        );
    }
    // Scaling: logic cost is clock-rate x room-size^3, not gate count.
    println!("  clock sweep, 16 rings (100 elements):");
    for hz in [0.001f64, 100.0, 1_000.0, 5_000.0, 10_000.0] {
        let r = digital_room(16, hz);
        bench(&format!("16 rings @ {hz:>8.0} Hz"), &r, true, steps);
    }
}

/// THE CORRECTNESS GUARD on the `is_discrete_nonlinear` classification.
///
/// Reuse is only sound if a retained factorization is bitwise what a fresh
/// one would have been. Run every logic golden both ways and compare the
/// engine's own state hash — not a tolerance, the digest.
#[test]
fn reuse_never_changes_a_logic_answer() {
    for (name, elems) in all_golden() {
        let mut a = eng_with(&elems);
        a.set_reuse_pwl(true);
        let mut b = eng_with(&elems);
        b.set_reuse_pwl(false);
        for _ in 0..200 {
            a.advance(50);
            b.advance(50);
            assert_eq!(
                a.state_hash(),
                b.state_hash(),
                "{name}: reuse changed the answer"
            );
        }
    }
}

/// The classification itself, stated as a property rather than as a list:
/// every logic kind must be discrete-nonlinear and must NOT need Newton,
/// because `smooth_nonlinear` is GLOBAL — one misclassified gate disarms
/// reuse for every op-amp and 555 sharing the room.
#[test]
fn every_logic_kind_is_on_the_piecewise_linear_path() {
    let kinds = [
        K::Gate {
            op: GateOp::And,
            ins: 4,
        },
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        K::FlipFlop { edge: true },
        K::FlipFlop { edge: false },
        K::ShiftReg { bits: 4 },
        K::Counter {
            bits: 4,
            modulus: 5,
        },
        K::Mux { sel: 2 },
    ];
    for k in kinds {
        assert!(k.is_logic(), "{k:?}");
        assert!(k.is_nonlinear(), "{k:?} must be nonlinear (it has state)");
        assert!(
            k.is_discrete_nonlinear(),
            "{k:?} is NOT discrete-nonlinear: it will refactor every Newton iteration"
        );
        assert!(
            !k.needs_newton(),
            "{k:?} needs Newton: it disarms reuse for the WHOLE ROOM"
        );
        assert!(!k.is_branch(), "{k:?} must own no branch unknown");
    }
}

/// A logic room must not iterate: exactly one Newton pass per substep,
/// which is the whole performance claim.
#[test]
fn a_logic_room_never_iterates() {
    let mut e = eng_with(&digital_room(4, 1000.0));
    e.advance(2000);
    let rep = e.advance(20_000);
    assert_eq!(rep.steps, 20_000);
    assert_eq!(
        rep.nr_iters, rep.steps,
        "expected 1 NR pass per substep, got {} for {} steps",
        rep.nr_iters, rep.steps
    );
    assert_eq!(rep.rescues, 0);
}

/// ONE logic part in an otherwise ANALOG room: the honest scaling law. The
/// factorization is O(n^3) over the whole room, so a chip that changes state
/// makes the whole room refactor.
#[test]
#[ignore = "timing; cargo test --release -- --ignored --nocapture"]
fn one_chip_in_an_analog_room() {
    // A ladder of 200 resistors and 20 op-amps: piecewise-linear, so it
    // reuses perfectly on its own.
    let mut base = vec![spec(1, dc(5.0), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let mut id = 100u32;
    for k in 0..100i32 {
        let a = (10 + 4 * k, 4);
        let b = (10 + 4 * k, 12);
        base.push(spec(id, r(1000.0), (0, 0), a));
        base.push(spec(id + 1, r(2200.0), a, (0, 24)));
        base.push(spec3(
            id + 2,
            K::OpAmp {
                rail: 5.0,
                isc: sim_core::DEFAULT_OPAMP_ISC,
            },
            a,
            b,
            b,
        ));
        base.push(spec(id + 3, r(10_000.0), b, (0, 24)));
        id += 10;
    }
    println!("\n  analog baseline: {} elements", base.len());
    let steps = 20_000u32;
    bench("200-part analog room alone", &base, true, steps);
    let mut with_logic = base.clone();
    with_logic.push(spec(
        90,
        K::VoltageSource {
            dc: 2.5,
            amp: 2.5,
            hz: 1000.0,
            phase: 0.0,
        },
        (4, 12),
        (0, 24),
    ));
    with_logic.push(ElementSpec::pins(
        91,
        K::ShiftReg { bits: 4 },
        &[
            (0, 0),
            (0, 24),
            (4, 12),
            (0, 0),
            (0, 0),
            (900, 4),
            (900, 8),
            (900, 12),
            (900, 16),
        ],
    ));
    for j in 0..4 {
        with_logic.push(spec(92 + j as u32, r(10_000.0), (900, 4 + 4 * j), (0, 24)));
    }
    bench("...plus ONE 1 kHz shift register", &with_logic, true, steps);
}
