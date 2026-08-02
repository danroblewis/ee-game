//! Canonical form for IDEAL, zero-impedance branch constraints — the shared
//! definition of "these two parts are the same net".
//!
//! An ideal source pins a node pair to a voltage with no internal impedance,
//! so its MNA row is pure ±1 incidence and depends on nothing but its node
//! pair. Two such elements on the SAME ordered node pair therefore emit
//! IDENTICAL rows and (by the transpose stamp) identical columns: rank
//! deficiency, structurally, before any value is looked at. The LU sees only
//! "singular" and every one of those cases used to collapse into one
//! anonymous `Unsolvable`.
//!
//! But two of those cases are not errors:
//!
//! * **the same constraint twice** — two 5 V supplies on one node, a supply
//!   and a rail of the same voltage, a supply drawn one way and its negative
//!   drawn the other. All 5 V sources are assumed to come from the same
//!   supply; they are one net, one row, one current.
//! * **two closed switches in parallel** — a switch is a 0 V source, so
//!   two-way lighting, an OR contact and a manual override across a relay
//!   are the *identical* singularity. This is why the rule is written over
//!   ideal constraints and not over "sources".
//!
//! And two of them are:
//!
//! * different voltages on one node pair (1 V against 5 V) — inconsistent;
//! * the same magnitude in opposite directions (+5 V against −5 V across one
//!   pair) — also inconsistent, and shorted it would put 10 V across zero
//!   ohms.
//!
//! This module reduces every participant to one oriented constraint
//!
//! ```text
//!     v(a) − v(b) = dc + amp·sin(2π·hz·t + phase),   a < b
//! ```
//!
//! and gives it an integer [`ConstraintKey`]. Equal keys ⇒ one net (merged
//! by `Engine::compile`); a differing key on the same pair ⇒ a conflict the
//! validator can name, with both element ids and both voltages.
//!
//! **Motor, OpAmp and Timer555 do not participate.** A motor stamps
//! `−(R + L/h)` on its own diagonal, so parallel motors are well-posed and
//! must NOT be merged. An op-amp's and a 555's row/column structure depends
//! on their discrete region, and two op-amps driving one node is a real
//! design error, not a net.
//!
//! Everything here is total, integer-compared and free of libm: the grouping
//! it induces is a genuine equivalence relation and is identical on native
//! and wasm32.

use crate::netlist::{ElementKind, MAX_PINS};

/// One ideal constraint in canonical form: `v(a) − v(b) = dc + amp·sin(...)`
/// with `a < b` (except in the degenerate `a == b` short, which the
/// validator names rather than merges).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub a: usize,
    pub b: usize,
    pub dc: f64,
    pub amp: f64,
    pub hz: f64,
    pub phase: f64,
    /// True when the element was DRAWN with its pins in the opposite order
    /// to the canonical `(a, b)`. Two members of one group whose `flipped`
    /// differs read the shared branch current with opposite signs.
    pub flipped: bool,
}

/// Order-independent identity of a [`Constraint`]. Integer fields only, so
/// `==` is exact and grouping by it is a true equivalence relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstraintKey {
    pub a: usize,
    pub b: usize,
    w: [u64; 4],
}

/// Relative quantization of a waveform parameter, by mantissa truncation.
///
/// **Never compare these floats with `==`, and never with an epsilon.** `==`
/// makes a hand-edited save or a log-scale value drag (`10^(log10 v + δ)`)
/// fail to merge for a difference no player can see. An epsilon comparison
/// is worse: it is not transitive, so "same net" would not be an equivalence
/// relation — you would get A~B, B~C, A≁C, and *which* pairs merged would
/// shift when an unrelated element was deleted and iteration order changed.
/// That is a correctness bug, not a purity concern.
///
/// Quantization is a total function, so grouping by the result is an
/// equivalence relation and is order-independent by construction.
///
/// `DROP = 12` keeps 40 of the 52 mantissa bits: a relative tolerance of
/// 2⁻⁴⁰ ≈ 9.1e-13. Coarse enough to absorb representation noise (~4000 ulps
/// of headroom); nine orders of magnitude finer than the finest distinction
/// the value UI can express (3–4 significant figures), so it can never merge
/// two values a player meant to differ.
///
/// Pure bit manipulation — no division, no rounding mode, no libm — so it is
/// bit-identical on native and wasm32. The carry out of the mantissa into the
/// exponent is CORRECT: binary64 bit patterns are monotone in magnitude for a
/// fixed sign. Carry into the sign bit would need `|v|` at the top of the
/// exponent range; `wrapping_add` keeps even a NaN payload total rather than
/// panicking in debug.
///
/// Values astride a quantization boundary key apart and are reported as a
/// conflict. That is the safe direction and it is not even wrong: an ideal
/// 5.000000000001 V source across an ideal 5 V source really is a short. The
/// tolerance forgives representation noise; it does not paper over
/// disagreement.
pub fn qkey(v: f64) -> u64 {
    if v == 0.0 {
        return 0; // folds −0.0 and +0.0 together
    }
    const DROP: u32 = 12;
    let half = 1u64 << (DROP - 1);
    (v.to_bits().wrapping_add(half)) & !((1u64 << DROP) - 1)
}

impl Constraint {
    /// Canonicalize a raw `v(p0) − v(p1) = dc + amp·sin(2π·hz·t + phase)`.
    ///
    /// Orientation is normalized BEFORE the amplitude sign, deliberately.
    /// The reverse order would push `phase` by π on the swap and again on the
    /// re-normalization, so a source and its exact mirror image (drawn the
    /// other way with a negated amplitude) would land on `phase = 2π` versus
    /// `phase = 0` and refuse to merge despite being the same waveform.
    ///
    /// `phase` is NOT reduced modulo 2π. Doing so would mean reducing by an
    /// f64 approximation of an irrational constant on a path the determinism
    /// harness polices, to buy a convenience case. The consequence is that a
    /// player who writes `phase = 0` on one source and `phase = 2π` on
    /// another gets a conflict instead of a merge: rare, deterministic, and
    /// fail-safe (it refuses rather than silently simulating something else).
    fn canonical(mut a: usize, mut b: usize, mut dc: f64, mut amp: f64, hz: f64, phase: f64) -> Self {
        let mut hz = hz;
        let mut phase = phase;
        let flipped = a > b;
        if flipped {
            core::mem::swap(&mut a, &mut b);
            dc = -dc;
            amp = -amp;
        }
        if amp < 0.0 {
            amp = -amp;
            phase += core::f64::consts::PI; // −A·sin(x) = A·sin(x + π)
        }
        if amp == 0.0 {
            // A DC constraint has no frequency and no phase. Without this,
            // `dc(5)` and `{dc: 5, amp: 0, hz: 50}` would key apart despite
            // being the same waveform.
            amp = 0.0;
            hz = 0.0;
            phase = 0.0;
        }
        if dc == 0.0 {
            dc = 0.0; // fold −0.0
        }
        Constraint {
            a,
            b,
            dc,
            amp,
            hz,
            phase,
            flipped,
        }
    }

    pub fn key(&self) -> ConstraintKey {
        ConstraintKey {
            a: self.a,
            b: self.b,
            w: [
                qkey(self.dc),
                qkey(self.amp),
                qkey(self.hz),
                qkey(self.phase),
            ],
        }
    }

    /// Both pins resolve to one electrical node: the constraint row cancels
    /// to all zeros. This is a shorted source, not a merge candidate.
    pub fn is_shorted(&self) -> bool {
        self.a == self.b
    }

    /// The DC level this constraint puts on the net, for the conflict
    /// message: the voltage of the HIGHER-numbered node relative to the
    /// lower, which is `-dc` because canonicalization sorts the pair.
    ///
    /// The direction matters for readability, not just for sign hygiene.
    /// Node 0 is always the lowest, so anything referenced to ground reads
    /// as the number on the part: a 5 V battery to ground reports `5 V`, not
    /// `-5 V`. Two constraints on one pair are still measured the SAME way,
    /// so they stay directly comparable — an anti-parallel pair reports
    /// `5 V` and `-5 V`, which is exactly the physical situation.
    pub fn nominal(&self) -> f64 {
        if self.dc == 0.0 {
            0.0 // never render "-0 V"
        } else {
            -self.dc
        }
    }
}

/// The canonical constraint an element imposes, or `None` when it is not an
/// ideal zero-impedance constraint (Motor / OpAmp / Timer555 / everything
/// that is not a branch device, plus open switches, which own no branch).
///
/// A `Rail` folds to the pair `(node[0], 0)` because its return path IS node
/// 0 — that single line is what makes rail-vs-rail and rail-vs-grounded-
/// source fall out of the SAME rule as source-vs-source, instead of needing
/// three special cases.
pub fn constraint_of(kind: &ElementKind, node: &[usize; MAX_PINS]) -> Option<Constraint> {
    let (a, b, dc, amp, hz, phase) = match *kind {
        ElementKind::VoltageSource { dc, amp, hz, phase } => (node[0], node[1], dc, amp, hz, phase),
        ElementKind::Rail { dc, amp, hz, phase } => (node[0], 0, dc, amp, hz, phase),
        // A closed switch is a 0 V source. Including it here is what makes
        // two-way lighting and an OR contact placeable.
        ElementKind::Switch { closed: true } | ElementKind::Button { closed: true } => {
            (node[0], node[1], 0.0, 0.0, 0.0, 0.0)
        }
        _ => return None,
    };
    Some(Constraint::canonical(a, b, dc, amp, hz, phase))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(a: usize, b: usize) -> [usize; MAX_PINS] {
        let mut n = [0; MAX_PINS];
        n[0] = a;
        n[1] = b;
        n
    }

    fn vs(dc: f64) -> ElementKind {
        ElementKind::VoltageSource {
            dc,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        }
    }

    fn ac(dc: f64, amp: f64, hz: f64, phase: f64) -> ElementKind {
        ElementKind::VoltageSource { dc, amp, hz, phase }
    }

    fn rail(dc: f64) -> ElementKind {
        ElementKind::Rail {
            dc,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        }
    }

    fn key(k: &ElementKind, a: usize, b: usize) -> ConstraintKey {
        constraint_of(k, &nodes(a, b)).unwrap().key()
    }

    #[test]
    fn quantization_is_a_total_equivalence_relation() {
        // Reflexive, symmetric, and — the property an epsilon compare would
        // break — transitive, because it is a function.
        assert_eq!(qkey(5.0), qkey(5.0));
        assert_eq!(qkey(0.0), qkey(-0.0));
        assert_ne!(qkey(5.0), qkey(-5.0));
        assert_ne!(qkey(5.0), qkey(5.001));
        // Representation noise merges; a real difference does not.
        assert_eq!(qkey(5.0), qkey(5.0 + 5.0 * 1e-14));
        assert_ne!(qkey(5.0), qkey(5.0 * (1.0 + 1e-6)));
        // Total over every f64 the value gate can pass, and over the ones it
        // cannot: no panic, no overflow.
        for v in [
            f64::MIN,
            f64::MAX,
            f64::NAN,
            -f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE,
            -1e300,
            1e-300,
        ] {
            let _ = qkey(v);
        }
    }

    #[test]
    fn identical_constraints_share_a_key() {
        assert_eq!(key(&vs(5.0), 1, 2), key(&vs(5.0), 1, 2));
        // Drawn the other way with the negated value: literally the same
        // constraint, two ways.
        assert_eq!(key(&vs(5.0), 1, 2), key(&vs(-5.0), 2, 1));
        // A rail folds onto (node, 0), so a rail and a grounded source of the
        // same voltage are one net.
        assert_eq!(key(&rail(5.0), 3, 0), key(&vs(5.0), 3, 0));
        // ... including when the source was drawn ground-to-rail.
        assert_eq!(key(&rail(5.0), 3, 0), key(&vs(-5.0), 0, 3));
        // Two closed switches, either way round: 0 V vs −0.0 V.
        let sw = ElementKind::Switch { closed: true };
        let bt = ElementKind::Button { closed: true };
        assert_eq!(key(&sw, 1, 2), key(&sw, 2, 1));
        assert_eq!(key(&sw, 1, 2), key(&bt, 1, 2));
        // Identical AC sources.
        assert_eq!(key(&ac(0.0, 3.0, 50.0, 0.5), 1, 2), key(&ac(0.0, 3.0, 50.0, 0.5), 1, 2));
        // A DC source and an amp-zero "AC" source of the same level.
        assert_eq!(key(&vs(5.0), 1, 2), key(&ac(5.0, 0.0, 50.0, 1.2), 1, 2));
    }

    #[test]
    fn different_constraints_key_apart() {
        assert_ne!(key(&vs(5.0), 1, 2), key(&vs(1.0), 1, 2));
        // Anti-parallel, same magnitude: +5 against −5 on one pair.
        assert_ne!(key(&vs(5.0), 1, 2), key(&vs(5.0), 2, 1));
        // Same DC level, different waveform.
        assert_ne!(key(&vs(5.0), 1, 2), key(&ac(5.0, 1.0, 50.0, 0.0), 1, 2));
        assert_ne!(
            key(&ac(0.0, 3.0, 50.0, 0.0), 1, 2),
            key(&ac(0.0, 3.0, 60.0, 0.0), 1, 2)
        );
        assert_ne!(
            key(&ac(0.0, 3.0, 50.0, 0.0), 1, 2),
            key(&ac(0.0, 3.0, 50.0, 1.0), 1, 2)
        );
        // A closed switch is 0 V: it conflicts with any live source on the
        // same pair (that is a short across the source).
        let sw = ElementKind::Switch { closed: true };
        assert_ne!(key(&sw, 1, 2), key(&vs(9.0), 1, 2));
        // Different node pairs never group, whatever the value.
        assert_ne!(key(&vs(5.0), 1, 2), key(&vs(5.0), 1, 3));
    }

    #[test]
    fn orientation_is_recorded_not_lost() {
        let fwd = constraint_of(&vs(5.0), &nodes(1, 2)).unwrap();
        let rev = constraint_of(&vs(-5.0), &nodes(2, 1)).unwrap();
        assert_eq!(fwd.key(), rev.key());
        assert!(!fwd.flipped);
        assert!(rev.flipped);
    }

    #[test]
    fn non_ideal_branches_never_participate() {
        for k in [
            ElementKind::Motor {
                ohms: 2.0,
                henries: 1e-3,
                bemf: 0.0,
            },
            ElementKind::OpAmp { rail: 9.0, isc: crate::DEFAULT_OPAMP_ISC },
            ElementKind::Timer555,
            ElementKind::Switch { closed: false },
            ElementKind::Button { closed: false },
            ElementKind::Resistor { ohms: 100.0 },
        ] {
            assert!(constraint_of(&k, &nodes(1, 2)).is_none(), "{k:?}");
        }
    }

    #[test]
    fn a_source_with_both_pins_on_one_node_is_shorted_not_merged() {
        assert!(constraint_of(&vs(5.0), &nodes(2, 2)).unwrap().is_shorted());
        // A rail sitting on ground is the same degeneracy: (0, 0).
        assert!(constraint_of(&rail(5.0), &nodes(0, 0)).unwrap().is_shorted());
    }
}
