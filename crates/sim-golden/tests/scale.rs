//! Tests for the scale generator. The benchmark is only as trustworthy as
//! the worlds it builds, so the properties the report relies on are asserted
//! here: determinism, island structure, agreement with `Engine::compile`,
//! and that the worlds actually simulate.

use sim_core::Engine;
use sim_golden::scale::{self, GenParams, Structure};
use sim_math::DenseLu;

/// The bench's op counter is a hand copy of the `sim-math` kernel (counters
/// must not enter the shipping solver's hot loop). This test is what keeps the
/// copy honest: same pivots, bit-identical factor, same nonzero count on real
/// stamped MNA matrices. If `dense.rs` changes, this fails rather than quietly
/// invalidating every op count in `docs/scale-baseline.md`.
#[test]
fn lu_ops_matches_dense_lu() {
    for structure in [Structure::One, Structure::Districts { size: 90 }] {
        for nonlinear in [0, 30] {
            let w = scale::generate(GenParams::new(600, structure).nonlinear(nonlinear));
            let mut eng = Engine::new(20e-6);
            eng.set_elements(&w.flat());
            eng.advance(20);
            // One matrix per island since partitioning landed: check the
            // mirror against the real kernel on every one of them.
            let mut biggest = 0usize;
            let mut fill_seen = false;
            for island in eng.islands() {
                let n = island.unknowns();
                if n == 0 {
                    continue;
                }
                biggest = biggest.max(n);
                let a = island.matrix().to_vec();

                let mut lu = DenseLu::new(n);
                assert!(lu.factor(&a), "reference factor went singular");
                let (ops, mirror) = scale::lu_ops(&a, n);

                assert!(!ops.singular);
                assert_eq!(ops.n, n);
                assert_eq!(
                    mirror,
                    lu.factor_slice(),
                    "mirror LU differs from sim-math (bit level)"
                );
                assert_eq!(ops.factor_nnz, lu.factor_nnz());
                // Every subdiagonal entry gets exactly one division, and every
                // one either updates its row or is counted as skipped.
                let sub = (n * (n - 1) / 2) as u64;
                assert_eq!(ops.divisions, sub);
                assert_eq!(ops.pivot_cmps, sub);
                // Some row updates are always skipped (MNA matrices are
                // sparse), so dense-LU cost is structure dependent rather
                // than a clean n^3/3.
                assert!(ops.skipped_rows > 0, "{ops:?}");
                assert!(ops.updates < ops.dense_updates(), "{ops:?}");
                assert!(ops.structure_saving() > 0.0 && ops.structure_saving() < 1.0);
                fill_seen |= ops.factor_nnz > island.matrix_nnz();
            }
            // Fill-in is a topology property: a single connected world fills
            // heavily, a world of small builds barely at all — which is the
            // whole reason partitioning is worth so much.
            assert!(biggest > 0);
            if matches!(structure, Structure::One) {
                assert!(fill_seen, "a connected world must fill in");
            }
        }
    }
}

#[test]
fn generation_is_deterministic() {
    let p = GenParams::new(800, Structure::Districts { size: 100 });
    let a = scale::generate(p);
    let b = scale::generate(p);
    assert_eq!(a.element_count(), b.element_count());
    assert_eq!(a.active, b.active);
    let (fa, fb) = (a.flat(), b.flat());
    for (x, y) in fa.iter().zip(fb.iter()) {
        assert_eq!(x.id, y.id);
        assert_eq!(x.pins, y.pins);
        assert_eq!(x.kind, y.kind, "element {} differs between runs", x.id);
    }
    assert_eq!(scale::topology(&fa), scale::topology(&fb));
}

#[test]
fn districts_are_electrically_disconnected() {
    for nonlinear in [0, 30, 100] {
        let p = GenParams::new(1200, Structure::Districts { size: 100 }).nonlinear(nonlinear);
        let w = scale::generate(p);
        let t = scale::topology(&w.flat());
        assert_eq!(
            t.islands,
            w.districts.len(),
            "{nonlinear}% nonlinear: {} districts produced {} islands",
            w.districts.len(),
            t.islands
        );
        // Each district on its own must be a single island.
        for (i, d) in w.districts.iter().enumerate() {
            assert_eq!(
                scale::topology(d).islands,
                1,
                "district {i} is not connected"
            );
        }
    }
}

#[test]
fn one_circuit_mode_is_one_island() {
    for nonlinear in [0, 30, 100] {
        let w = scale::generate(GenParams::new(900, Structure::One).nonlinear(nonlinear));
        assert_eq!(w.districts.len(), 1);
        assert_eq!(scale::topology(&w.flat()).islands, 1);
    }
}

#[test]
fn topology_matches_engine_compile() {
    for structure in [Structure::One, Structure::Districts { size: 120 }] {
        let w = scale::generate(GenParams::new(700, structure));
        let specs = w.flat();
        let t = scale::topology(&specs);
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&specs);
        assert_eq!(eng.node_count(), t.nodes);
        assert_eq!(eng.branch_count(), t.branches);
        assert_eq!(eng.unknowns(), t.unknowns);
    }
}

#[test]
fn device_mix_is_game_shaped() {
    let w = scale::generate(GenParams::new(2000, Structure::Districts { size: 150 }));
    let specs = w.flat();
    let (mix, nl) = scale::mix(&specs);
    let frac = |name: &str| {
        mix.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, c)| *c as f64 / specs.len() as f64)
            .unwrap_or(0.0)
    };
    // Mostly passive wiring...
    assert!(frac("wire") > 0.3, "wires: {}", frac("wire"));
    assert!(frac("resistor") > 0.15);
    assert!(frac("cap") > 0.03);
    // ...with a real nonlinear population, and every device class that
    // forces refactorization present.
    let nl_frac = nl as f64 / specs.len() as f64;
    assert!(
        (0.02..0.20).contains(&nl_frac),
        "nonlinear fraction {nl_frac}"
    );
    for k in ["diode", "led", "npn", "nmos", "opamp"] {
        assert!(frac(k) > 0.0, "no {k} in the mix");
    }
}

#[test]
fn worlds_simulate_without_quarantine() {
    for structure in [Structure::One, Structure::Districts { size: 100 }] {
        let w = scale::generate(GenParams::new(400, structure));
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&w.flat());
        let r = eng.advance(200);
        assert_eq!(r.steps, 200, "steps short: {r:?}");
        assert!(!eng.is_quarantined(), "quarantined: {r:?}");
        // Sanity: the supply rail is powered and every island's solve is
        // finite.
        let x = || eng.islands().iter().flat_map(|i| i.solution().iter());
        assert!(x().all(|v| v.is_finite()));
        assert!(x().any(|v| *v > 1.0));
    }
}

#[test]
fn nonlinear_worlds_refactor_every_iteration_linear_ones_do_not() {
    // The premise of the whole report: `linear` is a global flag.
    let mut linear = Engine::new(20e-6);
    linear.set_elements(&scale::generate(GenParams::new(400, Structure::One).nonlinear(0)).flat());
    linear.advance(100);
    assert!(linear.is_linear());
    let f_lin = linear.factorizations();

    let mut nl = Engine::new(20e-6);
    nl.set_elements(&scale::generate(GenParams::new(400, Structure::One).nonlinear(30)).flat());
    nl.advance(100);
    assert!(!nl.is_linear());
    assert!(
        nl.factorizations() >= 100,
        "nonlinear world factored only {} times in 100 substeps",
        nl.factorizations()
    );
    assert!(
        f_lin < 10,
        "linear world refactored {f_lin} times in 100 substeps"
    );
}

/// The property the quiescence lever is sold on, asserted rather than
/// eyeballed in a report: a world of DC-only builds really does go
/// completely still, the engine really does stop visiting it, and the
/// numbers it keeps reporting are the ones the solver last produced —
/// identical, to the volt, to a world simulated with no skipping at all.
#[test]
fn an_idle_world_goes_static_and_still_reads_the_same_as_a_fully_solved_one() {
    let specs = scale::generate(
        GenParams::new(1_000, Structure::Districts { size: 100 })
            .nonlinear(30)
            .active(0),
    )
    .flat();

    let mut skipping = Engine::new(20e-6);
    skipping.set_elements(&specs);
    let mut plain = Engine::new(20e-6);
    plain.set_tuning(sim_core::Tuning::off());
    plain.set_elements(&specs);
    // 300 ms of sim time: past the ~250 ms a DC district needs to settle.
    for _ in 0..15 {
        skipping.advance(1_000);
        plain.advance(1_000);
    }
    assert_eq!(
        skipping.time(),
        plain.time(),
        "the world clock is untouched"
    );

    let solving = skipping
        .islands()
        .iter()
        .filter(|i| i.unknowns() > 0)
        .count();
    assert!(
        skipping.static_islands() * 4 >= solving * 3,
        "only {}/{solving} idle islands went static",
        skipping.static_islands()
    );

    // A skipped island must not invent anything: every pin agrees with the
    // engine that solved every substep.
    let plain_frame = plain.frame();
    for f in skipping.frame() {
        let p = plain_frame.iter().find(|p| p.id == f.id).unwrap();
        for k in 0..f.npins {
            assert!(
                (f.v[k] - p.v[k]).abs() < 1e-3,
                "elem {} pin {k}: skipped {} vs solved {}",
                f.id,
                f.v[k],
                p.v[k]
            );
        }
    }

    // Nothing further is spent on the islands that have gone static. Per
    // island, deliberately: at 300 ms a couple of districts are still
    // finishing a long tail, and they SHOULD still be solving — sleeping is
    // now gated on a demonstrated decay, not merely on having gone quiet
    // (see `Tuning::quiescence_decay`). What must hold is that a sleeping
    // island costs nothing at all, and that it stays asleep.
    let before: Vec<(bool, u64)> = skipping
        .islands()
        .iter()
        .map(|i| (i.is_static(), i.factorizations()))
        .collect();
    let asleep = before.iter().filter(|(s, _)| *s).count();
    let r = skipping.advance(1_000);
    assert_eq!(r.steps, 1_000, "the clock keeps its promises");
    assert_eq!(
        r.static_islands as usize, asleep,
        "an island that was asleep did work anyway"
    );
    for (island, (was_static, facs)) in skipping.islands().iter().zip(before) {
        if was_static {
            assert!(island.is_static(), "a sleeping island woke on its own");
            assert_eq!(
                island.factorizations(),
                facs,
                "a sleeping island factored"
            );
        }
    }
}

/// ...and the other side of it: a world where every district contains
/// something that switches must not be skipped at all. A lever that is
/// wrong here is a lever that lies about the game.
#[test]
fn a_fully_active_world_is_never_declared_static() {
    let specs = scale::generate(
        GenParams::new(1_000, Structure::Districts { size: 100 })
            .nonlinear(30)
            .active(100),
    )
    .flat();
    let mut eng = Engine::new(20e-6);
    eng.set_elements(&specs);
    let solving = eng.islands().iter().filter(|i| i.unknowns() > 0).count();
    for _ in 0..15 {
        eng.advance(1_000);
        // A district built around an oscillator or an AC source is never
        // still. Allow a couple of stragglers only for districts whose
        // switching block happens to sit latched.
        assert!(
            eng.static_islands() * 10 <= solving,
            "{}/{solving} active islands were declared static",
            eng.static_islands()
        );
    }
}
