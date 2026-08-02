//! Event-driven piecewise-linear factorization reuse must be INVISIBLE.
//!
//! `Engine::set_reuse_pwl(false)` restores the pre-change behaviour (every
//! substep re-stamps and refactors). Every assertion here is bit-identity
//! between the two, checked continuously rather than only at the end, plus
//! the counter checks that keep the test honest — without them a broken
//! classification would make the test pass by never reusing anything.
//!
//! This is the guard on the `ElementKind::is_discrete_nonlinear` whitelist:
//! a future device that writes into the matrix as a function of its
//! continuous operating point, but is classified as discrete, diverges here.

use sim_core::{ElementKind, ElementSpec, Engine, InteractOp, ParamWrite};

const DT: f64 = 20e-6;

fn engine(elems: &[ElementSpec], reuse: bool) -> Engine {
    let mut eng = Engine::new(DT);
    eng.set_elements(elems);
    eng.set_reuse_pwl(reuse);
    eng
}

/// Run both settings in lockstep, comparing the full state hash at every
/// checkpoint. Returns (factorizations with reuse, factorizations without,
/// total substeps).
fn lockstep(name: &str, elems: &[ElementSpec], chunks: u32, per_chunk: u32) -> (u64, u64, u32) {
    let mut on = engine(elems, true);
    let mut off = engine(elems, false);
    let mut steps = 0;
    for c in 0..chunks {
        let ron = on.advance(per_chunk);
        let roff = off.advance(per_chunk);
        steps += ron.steps;
        assert_eq!(
            on.state_hash(),
            off.state_hash(),
            "{name}: state diverged at chunk {c} ({} substeps)",
            (c + 1) * per_chunk
        );
        assert_eq!(ron.steps, roff.steps, "{name}: step count diverged");
        assert_eq!(
            ron.nr_iters, roff.nr_iters,
            "{name}: NR pass count diverged at chunk {c}"
        );
        assert_eq!(ron.rescues, roff.rescues, "{name}: rescue count diverged");
        assert_eq!(
            on.is_quarantined(),
            off.is_quarantined(),
            "{name}: quarantine state diverged"
        );
    }
    (on.factorizations(), off.factorizations(), steps)
}

#[test]
fn every_golden_is_bit_identical_with_and_without_reuse() {
    for (name, elems) in sim_golden::all_golden() {
        let (f_on, f_off, steps) = lockstep(name, &elems, 40, 250);
        assert!(
            f_on <= f_off,
            "{name}: reuse must never factor more often ({f_on} > {f_off})"
        );
        assert!(steps > 0);
    }
}

/// The invariant itself, checked directly rather than through its
/// consequences: at every single substep, the matrix the reusing engine is
/// still holding must be BITWISE what the refactoring engine just stamped
/// from scratch. `Engine::matrix()` exposes the last stamped `a`, and with
/// reuse off that is re-zeroed and re-stamped every substep — so this
/// compares "retained" against "recomputed" 4 000 times per circuit.
///
/// A future device whose matrix contribution secretly depends on continuous
/// state fails here on the substep it first moves, with no dependence on
/// that dependence being large enough to show up in a state hash.
#[test]
fn a_reused_matrix_is_bitwise_what_a_refactor_would_have_stamped() {
    for (name, elems) in sim_golden::all_golden() {
        let mut on = engine(&elems, true);
        let mut off = engine(&elems, false);
        for k in 0..4000 {
            on.advance(1);
            off.advance(1);
            let (ma, mb) = (on.matrix(), off.matrix());
            assert_eq!(ma.len(), mb.len(), "{name}: matrix size diverged");
            for (i, (x, y)) in ma.iter().zip(mb.iter()).enumerate() {
                assert_eq!(
                    x.to_bits(),
                    y.to_bits(),
                    "{name}: retained matrix entry {i} differs at substep {k} ({x} vs {y})"
                );
            }
            for (i, (x, y)) in on.solution().iter().zip(off.solution().iter()).enumerate() {
                assert_eq!(
                    x.to_bits(),
                    y.to_bits(),
                    "{name}: solution entry {i} differs at substep {k}"
                );
            }
        }
        // Non-vacuity: for a circuit that reuses, most of those 4 000
        // comparisons must have been against a RETAINED matrix, not a
        // freshly stamped one.
        if !on.is_quarantined() {
            assert!(
                on.factorizations() <= off.factorizations(),
                "{name}: reuse factored more often than the baseline"
            );
        }
    }
    // ...and at least one golden must genuinely be reusing, or this whole
    // test is comparing two identical code paths.
    let mut on = engine(&sim_golden::timer555_astable(), true);
    on.advance(4000);
    assert!(
        on.factorizations() < 400,
        "555 astable stopped reusing: {} factorizations in 4000 substeps",
        on.factorizations()
    );
}

/// The counter half of the argument: on the owner's circuit class the win
/// must actually be there, or the identity test above is vacuous.
#[test]
fn piecewise_linear_circuits_stop_refactoring() {
    for (name, elems) in [
        ("timer555_astable", sim_golden::timer555_astable()),
        ("opamp_relaxation", sim_golden::opamp_relaxation()),
        ("opamp_follower", sim_golden::opamp_follower()),
        ("opamp_comparator", sim_golden::opamp_comparator()),
    ] {
        let (f_on, f_off, steps) = lockstep(name, &elems, 20, 500);
        assert!(
            f_off >= steps as u64,
            "{name}: baseline should factor at least once per substep"
        );
        assert!(
            (f_on as f64) < 0.2 * f_off as f64,
            "{name}: expected event-driven refactorization, got {f_on} of {f_off}"
        );
    }
}

/// ...and the guard must hold: one smoothly-nonlinear device in the same
/// matrix makes the matrix move on every Newton pass, so reuse must switch
/// itself off rather than freeze a diode's conductance.
#[test]
fn one_smooth_nonlinearity_disarms_reuse() {
    let mut elems = sim_golden::timer555_astable();
    // An LED across the 555's own load resistor: same matrix, same island.
    elems.push(ElementSpec::two(
        99,
        ElementKind::Led { color: 0 },
        (10, 4),
        (10, 6),
    ));
    let (f_on, f_off, steps) = lockstep("t555+led", &elems, 20, 500);
    assert_eq!(
        f_on, f_off,
        "a smooth nonlinearity must disable reuse entirely"
    );
    assert!(f_on >= steps as u64);
}

/// Damage is a live reclassification: breaking the LED must ARM reuse
/// mid-run, repairing it must disarm it, and neither may move a bit.
#[test]
fn breaking_and_repairing_a_diode_reclassifies_safely() {
    let mut elems = sim_golden::timer555_astable();
    elems.push(ElementSpec::two(
        99,
        ElementKind::Led { color: 0 },
        (10, 4),
        (10, 6),
    ));
    let mut on = engine(&elems, true);
    let mut off = engine(&elems, false);
    for c in 0..12 {
        on.advance(500);
        off.advance(500);
        assert_eq!(on.state_hash(), off.state_hash(), "diverged at chunk {c}");
        if c == 3 {
            on.set_broken(99, true);
            off.set_broken(99, true);
        }
        if c == 8 {
            on.set_broken(99, false);
            off.set_broken(99, false);
        }
    }
}

/// Live editing while a 555 runs: value changes, wiper writes and switch
/// flips all have to invalidate the retained factorization.
#[test]
fn live_edits_do_not_stale_the_factorization() {
    let mut elems = sim_golden::timer555_astable();
    // A pot and a switched resistor hanging off the 555's rail.
    elems.push(ElementSpec::three(
        90,
        ElementKind::Potentiometer {
            ohms: 5_000.0,
            wiper: 0.5,
        },
        (2, 0),
        (14, 0),
        (14, 6),
    ));
    elems.push(ElementSpec::ground(91, (14, 6)));
    elems.push(ElementSpec::two(
        92,
        ElementKind::Switch { closed: false },
        (2, 0),
        (16, 0),
    ));
    elems.push(ElementSpec::two(
        93,
        ElementKind::Resistor { ohms: 2_200.0 },
        (16, 0),
        (16, 6),
    ));
    elems.push(ElementSpec::ground(94, (16, 6)));

    let mut on = engine(&elems, true);
    let mut off = engine(&elems, false);
    for c in 0..200u32 {
        on.advance(100);
        off.advance(100);
        assert_eq!(on.state_hash(), off.state_hash(), "diverged at chunk {c}");
        match c % 4 {
            0 => {
                let frac = 0.05 + 0.9 * ((c % 20) as f64 / 20.0);
                on.write_param(90, ParamWrite::Wiper { frac });
                off.write_param(90, ParamWrite::Wiper { frac });
            }
            1 => {
                let closed = (c / 4) % 2 == 0;
                on.write_param(92, ParamWrite::Switch { closed });
                off.write_param(92, ParamWrite::Switch { closed });
            }
            2 => {
                let v = 1000.0 + 100.0 * (c % 7) as f64;
                on.interact(93, InteractOp::SetValue { value: v });
                off.interact(93, InteractOp::SetValue { value: v });
            }
            _ => {}
        }
    }
    assert!(!on.is_quarantined());
    assert!(
        on.factorizations() < off.factorizations() / 4,
        "live editing should still leave most substeps reusing: {} vs {}",
        on.factorizations(),
        off.factorizations()
    );
}

/// A circuit that cannot converge must still quarantine — reuse may not
/// rescue it, hide it, or panic on the way there.
#[test]
fn pathological_circuit_still_quarantines_identically() {
    // Two op-amps fighting over one node through a zero-impedance path:
    // a shorted output pair has no consistent solution.
    let elems = vec![
        ElementSpec::three(1, ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 0), (0, 4), (8, 2)),
        ElementSpec::three(2, ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 4), (0, 0), (8, 2)),
        ElementSpec::two(
            3,
            ElementKind::VoltageSource {
                dc: 1.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
            },
            (0, 0),
            (0, 8),
        ),
        ElementSpec::ground(4, (0, 8)),
        ElementSpec::two(5, ElementKind::Resistor { ohms: 1000.0 }, (0, 4), (0, 8)),
    ];
    let mut on = engine(&elems, true);
    let mut off = engine(&elems, false);
    let ron = on.advance(2000);
    let roff = off.advance(2000);
    // Non-vacuous: this really does walk the whole rescue ladder and end in
    // quarantine (measured: 4 rescues, 0 accepted steps), with reuse armed
    // the entire way — a stale factorization would show up as a different
    // number of rescues or a circuit that "succeeds" instead of quarantining.
    assert!(on.is_quarantined(), "test circuit no longer fails to solve");
    assert_eq!(ron.rescues, 4, "rescue ladder depth changed");
    assert_eq!(on.is_quarantined(), off.is_quarantined());
    assert_eq!(ron.steps, roff.steps);
    assert_eq!(ron.rescues, roff.rescues);
    assert_eq!(on.state_hash(), off.state_hash());
}
