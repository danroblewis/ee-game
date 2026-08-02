//! Placement-time validation: decide whether a document is one the engine
//! can actually solve BEFORE it becomes the live netlist.
//!
//! This is the single implementation of the placement guard. The server
//! calls it on a candidate copy of the room document before committing any
//! mutation (doc edit, interact, repair, machine move); the client reaches
//! the same code through `sim-wasm` for pre-send hints. One implementation,
//! two callers — the two sides can never disagree about what is placeable.
//!
//! Two layers:
//!
//! 1. **Value sanity** — every numeric parameter must be finite and inside
//!    the range the solver is conditioned for. These are the same bounds
//!    `InteractOp::SetValue` clamps to (extended with upper bounds), applied
//!    to the `Add`/`SetKind`/`SetValue` payloads that previously bypassed
//!    them. A `1/ohms` of a zero-ohm resistor is `inf` straight into the
//!    matrix; a 1e300 V source overflows every derived power figure.
//!
//! 2. **Structural solvability** — stamp and LU-factor the candidate
//!    document on a scratch [`Engine`] (one dense factorization, the same
//!    cost an accepted edit pays on its next step). A singular matrix here
//!    is exactly what would quarantine the whole room one tick later:
//!    voltage-source loops, paralleled/shorted ideal sources, a Rail whose
//!    pin lands in the ground set, a source with its pins merged onto one
//!    node. The factor runs twice:
//!
//!    * **as placed** — the document with switch states as they stand;
//!    * **worst case** — every `Switch`/`Button` forced closed.
//!
//!    The worst-case pass is deliberate policy: a circuit that only becomes
//!    singular when a switch closes is a landmine, not a valid placement.
//!    Switches are the game's interaction primitive (any player can flip
//!    one at any time) and the hoist's limit switches are flipped by the
//!    MACHINE through `write_param`, which — correctly — never clears
//!    quarantine, so a machine-closed short is a self-locking, room-wide
//!    freeze (measured: a wire across LIM-TOP deadlocks until deleted).
//!    Closing a switch only ever ADDS a 0 V branch row and merges nothing,
//!    so singularity is monotone in the closed set: all-closed is the true
//!    worst case and one extra factorization covers every combination.
//!    The cost is conservatism about documents that are singular with all
//!    switches closed but would be fine in some mixed state — those already
//!    freeze the whole room today the moment the switches DO close, so
//!    nothing playable is lost by refusing them.
//!
//! Broken parts are validated as if healthy: a broken part stamps nothing,
//! but `Repair` can put any of them back at any time, so a document is only
//! accepted if it solves with every part in service.
//!
//! What is deliberately ACCEPTED: floating subgraphs, dangling current
//! sources (GMIN keeps both solvable — normal mid-build states), capacitor
//! loops and inductor cutsets (finite companion conductances), coincident
//! pins on non-source parts. Never reject a circuit the engine can solve.

use crate::engine::Engine;
use crate::netlist::{ElementKind, ElementSpec};

// ---- value ranges. Lower bounds match the `InteractOp::SetValue` clamps in
// `Engine::interact`; upper bounds keep every derived quantity (1/ohms, C/h,
// h/L, amps/GMIN, v*i) comfortably finite in f64.

pub const MIN_OHMS: f64 = 1e-6;
pub const MAX_OHMS: f64 = 1e12;
pub const MIN_FARADS: f64 = 1e-15;
pub const MAX_FARADS: f64 = 1e3;
pub const MIN_HENRIES: f64 = 1e-12;
pub const MAX_HENRIES: f64 = 1e6;
/// Sources: 1 MV / 1 MA. A stranded 1 MA current source across GMIN reads
/// 1e18 V — absurd, but finite everywhere it propagates (v*i = 1e24 W),
/// so no NaN/inf can reach a broadcast, a save file or the energy meter.
pub const MAX_SOURCE_VOLTS: f64 = 1e6;
pub const MAX_SOURCE_AMPS: f64 = 1e6;
pub const MAX_HZ: f64 = 1e9;
pub const MIN_BETA: f64 = 1e-3;
pub const MAX_BETA: f64 = 1e9;
pub const MAX_MOS_K: f64 = 1e9;

/// Why a document was refused. `code()` is the machine-readable reason for
/// the wire protocol; `hint()` is a human sentence the client can surface
/// as a diegetic DRC hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// A parameter is NaN/inf or outside the solver's safe range.
    BadValue { id: u32, hint: &'static str },
    /// A source-like part with both terminals on the same grid point: its
    /// branch row would be all zeros.
    CollapsedPins { id: u32 },
    /// The document as it stands has no solution (singular MNA matrix).
    Unsolvable,
    /// Solvable as placed, but singular once some switch/button closes —
    /// and any switch can close at any time (player or machine).
    UnsolvableWhenSwitched,
}

impl Reject {
    pub fn code(&self) -> &'static str {
        match self {
            Reject::BadValue { .. } => "bad_value",
            Reject::CollapsedPins { .. } => "collapsed_pins",
            Reject::Unsolvable => "unsolvable",
            Reject::UnsolvableWhenSwitched => "unsolvable_switched",
        }
    }

    /// The element the refusal is pinned to, when it is one element's fault.
    pub fn id(&self) -> Option<u32> {
        match self {
            Reject::BadValue { id, .. } | Reject::CollapsedPins { id } => Some(*id),
            _ => None,
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Reject::BadValue { hint, .. } => hint,
            Reject::CollapsedPins { .. } => {
                "both terminals sit on the same point - stretch the part out"
            }
            Reject::Unsolvable => {
                "no solution exists for that circuit - look for shorted, looped or conflicting sources"
            }
            Reject::UnsolvableWhenSwitched => {
                "that would short a source the moment a switch closes - reroute it"
            }
        }
    }
}

fn in_range(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

fn mag_ok(v: f64, max: f64) -> bool {
    v.is_finite() && v.abs() <= max
}

/// Per-kind parameter sanity. Err carries the hint text.
fn check_kind(kind: &ElementKind) -> Result<(), &'static str> {
    use ElementKind as K;
    match *kind {
        K::Wire
        | K::Ground
        | K::Switch { .. }
        | K::Button { .. }
        | K::Diode
        | K::Led { .. }
        | K::Ota
        | K::Timer555 => Ok(()),
        K::Resistor { ohms } | K::Speaker { ohms } => {
            if !in_range(ohms, MIN_OHMS, MAX_OHMS) {
                return Err("resistance must be a finite value between 1 uOhm and 1 TOhm");
            }
            Ok(())
        }
        K::Lamp { ohms, rated_watts } => {
            if !in_range(ohms, MIN_OHMS, MAX_OHMS) {
                return Err("resistance must be a finite value between 1 uOhm and 1 TOhm");
            }
            if !in_range(rated_watts, 1e-9, 1e12) {
                return Err("power rating must be a positive finite value");
            }
            Ok(())
        }
        // Noise is a Norton source: an amplitude and its own internal
        // impedance. `seed` needs no range — every u32 is a valid stream, and
        // that is the point of seeding it rather than sampling a clock.
        K::Noise { volts, ohms, seed: _ } => {
            // Zero is legal — a silent noise source is a valid part, not a
            // broken one — so this is a magnitude bound, not a range.
            if !mag_ok(volts, MAX_SOURCE_VOLTS) {
                return Err("noise amplitude must be a finite voltage");
            }
            if !in_range(ohms, MIN_OHMS, MAX_OHMS) {
                return Err("source impedance must be a finite value between 1 uOhm and 1 TOhm");
            }
            Ok(())
        }
        K::Capacitor { farads } => {
            if !in_range(farads, MIN_FARADS, MAX_FARADS) {
                return Err("capacitance must be a finite value between 1 fF and 1 kF");
            }
            Ok(())
        }
        K::Inductor { henries } => {
            if !in_range(henries, MIN_HENRIES, MAX_HENRIES) {
                return Err("inductance must be a finite value between 1 pH and 1 MH");
            }
            Ok(())
        }
        K::VoltageSource { dc, amp, hz, phase } | K::Rail { dc, amp, hz, phase } => {
            if !mag_ok(dc, MAX_SOURCE_VOLTS) || !mag_ok(amp, MAX_SOURCE_VOLTS) {
                return Err("source voltage is limited to 1 MV");
            }
            if !mag_ok(hz, MAX_HZ) {
                return Err("source frequency is limited to 1 GHz");
            }
            if !phase.is_finite() {
                return Err("source phase must be finite");
            }
            Ok(())
        }
        K::CurrentSource { amps } => {
            if !mag_ok(amps, MAX_SOURCE_AMPS) {
                return Err("source current is limited to 1 MA");
            }
            Ok(())
        }
        K::Zener { vz } => {
            if !in_range(vz, 0.0, MAX_SOURCE_VOLTS) {
                return Err("zener voltage must be a finite value between 0 and 1 MV");
            }
            Ok(())
        }
        K::Npn { beta } | K::Pnp { beta } => {
            if !in_range(beta, MIN_BETA, MAX_BETA) {
                return Err("transistor beta must be a positive finite value");
            }
            Ok(())
        }
        K::Nmos { vt, k } | K::Pmos { vt, k } => {
            if !mag_ok(vt, MAX_SOURCE_VOLTS) {
                return Err("threshold voltage is limited to 1 MV");
            }
            if !in_range(k, 0.0, MAX_MOS_K) {
                return Err("transconductance must be a non-negative finite value");
            }
            Ok(())
        }
        K::OpAmp { rail } => {
            if !in_range(rail, 0.0, MAX_SOURCE_VOLTS) {
                return Err("op-amp rail must be a finite value between 0 and 1 MV");
            }
            Ok(())
        }
        K::Potentiometer { ohms, wiper } => {
            if !in_range(ohms, MIN_OHMS, MAX_OHMS) {
                return Err("resistance must be a finite value between 1 uOhm and 1 TOhm");
            }
            if !in_range(wiper, 0.0, 1.0) {
                return Err("wiper position must be between 0 and 1");
            }
            Ok(())
        }
        K::Motor {
            ohms,
            henries,
            bemf,
        } => {
            if !in_range(ohms, MIN_OHMS, MAX_OHMS) {
                return Err("winding resistance must be a finite value between 1 uOhm and 1 TOhm");
            }
            if !in_range(henries, 0.0, MAX_HENRIES) {
                return Err("winding inductance must be a non-negative finite value");
            }
            if !mag_ok(bemf, MAX_SOURCE_VOLTS) {
                return Err("back-EMF is limited to 1 MV");
            }
            Ok(())
        }
    }
}

/// Source-like two-pin parts whose branch row cancels to all zeros when both
/// pins land on one point. (A switch counts even while open: closing it
/// later would do the same thing, and the collapsed geometry is never what
/// the player meant.) Resistive parts and wires with coincident pins are
/// electrical no-ops and stay legal.
fn collapses_when_coincident(kind: &ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::VoltageSource { .. }
            | ElementKind::Motor { .. }
            | ElementKind::Switch { .. }
            | ElementKind::Button { .. }
    )
}

/// Would the engine accept this document? `Ok(())` = every parameter is in
/// range and the MNA matrix factors both as placed and with every switch
/// closed. Pure and deterministic; costs at most two dense factorizations
/// on a scratch engine. `dt` should be the timestep the live engine runs at
/// (companion conductances depend on it; structural singularity does not).
pub fn check_document(specs: &[ElementSpec], dt: f64) -> Result<(), Reject> {
    for s in specs {
        if s.pins.len() != s.kind.pin_count() {
            return Err(Reject::BadValue {
                id: s.id,
                hint: "wrong pin count for this part",
            });
        }
        if let Err(hint) = check_kind(&s.kind) {
            return Err(Reject::BadValue { id: s.id, hint });
        }
        if collapses_when_coincident(&s.kind) && s.pins[0] == s.pins[1] {
            return Err(Reject::CollapsedPins { id: s.id });
        }
    }

    // As placed.
    let mut eng = Engine::new(dt);
    eng.set_elements(specs);
    if !eng.probe_solvable() {
        return Err(Reject::Unsolvable);
    }

    // Worst case: every switch and button closed. Closing only ever adds
    // 0 V branch rows (it merges no nodes), so any singular mixed state is
    // singular here too — one factorization covers all 2^n combinations.
    let mut any_open = false;
    let closed: Vec<ElementSpec> = specs
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if let ElementKind::Switch { closed } | ElementKind::Button { closed } = &mut s.kind {
                if !*closed {
                    any_open = true;
                    *closed = true;
                }
            }
            s
        })
        .collect();
    if any_open {
        eng.set_elements(&closed);
        if !eng.probe_solvable() {
            return Err(Reject::UnsolvableWhenSwitched);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::ElementSpec;

    const DT: f64 = 20e-6;

    fn dc(volts: f64) -> ElementKind {
        ElementKind::VoltageSource {
            dc: volts,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        }
    }

    fn rail(volts: f64) -> ElementKind {
        ElementKind::Rail {
            dc: volts,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        }
    }

    fn r(ohms: f64) -> ElementKind {
        ElementKind::Resistor { ohms }
    }

    /// Battery + resistor + ground: the healthy base every breaker repro
    /// started from.
    fn base() -> Vec<ElementSpec> {
        vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, r(100.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ]
    }

    fn ok(specs: &[ElementSpec]) {
        assert_eq!(check_document(specs, DT), Ok(()), "must accept: {specs:?}");
    }

    fn rejected(specs: &[ElementSpec], want: Reject) {
        assert_eq!(
            check_document(specs, DT),
            Err(want),
            "must reject: {specs:?}"
        );
    }

    // ---- repro class: zero-resistance loop / shorted source

    #[test]
    fn wire_across_a_source_is_rejected() {
        let mut d = base();
        d.push(ElementSpec::two(4, ElementKind::Wire, (0, 0), (0, 6)));
        rejected(&d, Reject::Unsolvable);
    }

    // ---- repro class: paralleled / stacked ideal sources

    #[test]
    fn parallel_sources_are_rejected_agreeing_or_not() {
        for volts in [9.0, 5.0] {
            let mut d = base();
            d.push(ElementSpec::two(4, dc(volts), (0, 0), (0, 6)));
            rejected(&d, Reject::Unsolvable);
        }
    }

    // ---- repro class: voltage-source loop

    #[test]
    fn v_loop_is_rejected() {
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
            ElementSpec::two(3, dc(3.0), (6, 6), (0, 0)),
        ];
        rejected(&d, Reject::Unsolvable);
    }

    // ---- repro class: Move collapsing a source onto one point

    #[test]
    fn coincident_source_pins_are_rejected() {
        let d = vec![ElementSpec::two(1, dc(9.0), (0, 0), (0, 0))];
        rejected(&d, Reject::CollapsedPins { id: 1 });
        // Even via wires merging the two pins into one node.
        let mut d = base();
        d.push(ElementSpec::two(4, ElementKind::Wire, (0, 0), (4, 0)));
        d.push(ElementSpec::two(5, ElementKind::Wire, (4, 0), (0, 6)));
        rejected(&d, Reject::Unsolvable);
    }

    // ---- repro class: the single-pin V Rail and the implicit ground

    #[test]
    fn rail_grounded_or_stacked_is_rejected() {
        // Rail + Ground on the same point: branch row all zeros.
        let d = vec![
            ElementSpec {
                id: 1,
                kind: rail(12.0),
                pins: vec![(0, 0)],
            },
            ElementSpec::ground(2, (0, 0)),
        ];
        rejected(&d, Reject::Unsolvable);
        // Two rails on one point: dependent rows whatever their values.
        let d = vec![
            ElementSpec {
                id: 1,
                kind: rail(12.0),
                pins: vec![(0, 0)],
            },
            ElementSpec {
                id: 2,
                kind: rail(5.0),
                pins: vec![(0, 0)],
            },
        ];
        rejected(&d, Reject::Unsolvable);
        // A source from a rail node to a grounded node closes a V-loop
        // through the rail's implicit ground return — invisible on screen.
        let d = vec![
            ElementSpec {
                id: 1,
                kind: rail(12.0),
                pins: vec![(0, 0)],
            },
            ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        rejected(&d, Reject::Unsolvable);
        // A rail powering a grounded load is the intended use and stays legal.
        let d = vec![
            ElementSpec {
                id: 1,
                kind: rail(5.0),
                pins: vec![(0, 0)],
            },
            ElementSpec::two(2, r(1000.0), (0, 0), (8, 0)),
            ElementSpec::ground(3, (8, 0)),
        ];
        ok(&d);
    }

    // ---- repro class: switch closure completing a short (player OR machine)

    #[test]
    fn switch_across_a_source_is_rejected_in_both_states() {
        // Closed: singular right now.
        let mut d = base();
        d.push(ElementSpec::two(
            4,
            ElementKind::Switch { closed: true },
            (0, 0),
            (0, 6),
        ));
        rejected(&d, Reject::Unsolvable);
        // Open: solvable as placed, a landmine when flipped — the LIM-TOP
        // deadlock class. Buttons are the same device.
        let mut d = base();
        d.push(ElementSpec::two(
            4,
            ElementKind::Switch { closed: false },
            (0, 0),
            (0, 6),
        ));
        rejected(&d, Reject::UnsolvableWhenSwitched);
        let mut d = base();
        d.push(ElementSpec::two(
            4,
            ElementKind::Button { closed: false },
            (0, 0),
            (0, 6),
        ));
        rejected(&d, Reject::UnsolvableWhenSwitched);
    }

    #[test]
    fn wire_across_a_closed_switch_is_rejected() {
        // The LIM-BOT repro: a wire across a closed switch zeroes the
        // switch's branch row.
        let d = vec![
            ElementSpec::two(1, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(2, ElementKind::Wire, (0, 0), (4, 0)),
        ];
        rejected(&d, Reject::Unsolvable);
        // Same wire across an OPEN switch: fine now, singular on close.
        let d = vec![
            ElementSpec::two(1, ElementKind::Switch { closed: false }, (0, 0), (4, 0)),
            ElementSpec::two(2, ElementKind::Wire, (0, 0), (4, 0)),
        ];
        rejected(&d, Reject::UnsolvableWhenSwitched);
    }

    // ---- repro class: degenerate values through Add/SetKind

    #[test]
    fn degenerate_values_are_rejected() {
        let cases: Vec<(ElementKind, &str)> = vec![
            (r(0.0), "zero ohms"),
            (r(-100.0), "negative ohms"),
            (ElementKind::Capacitor { farads: -1e-6 }, "negative farads"),
            (ElementKind::Inductor { henries: 0.0 }, "zero henries"),
            (ElementKind::Inductor { henries: -1.0 }, "negative henries"),
            (dc(1e300), "absurd voltage"),
            (dc(f64::NAN), "NaN voltage"),
            (dc(f64::INFINITY), "inf voltage"),
            (ElementKind::CurrentSource { amps: 1e150 }, "absurd current"),
            (
                ElementKind::Speaker { ohms: 0.0 },
                "zero-ohm speaker (properties panel)",
            ),
            (ElementKind::Npn { beta: 0.0 }, "zero beta"),
        ];
        for (kind, why) in cases {
            let d = vec![ElementSpec::two(1, kind, (0, 0), (0, 6))];
            match check_document(&d, DT) {
                Err(Reject::BadValue { id: 1, .. }) => {}
                other => panic!("{why}: expected BadValue, got {other:?}"),
            }
        }
    }

    // ---- never reject valid circuits (negative controls, incl. the
    // breaker agents' measured-solvable cases)

    #[test]
    fn valid_and_mid_build_circuits_are_accepted() {
        ok(&base());
        // Dangling current source: normal mid-build state (GMIN-solvable).
        ok(&[ElementSpec::two(
            1,
            ElementKind::CurrentSource { amps: 1.0 },
            (0, 0),
            (0, 6),
        )]);
        // Floating battery + resistor island, no ground anywhere.
        ok(&[
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, r(100.0), (0, 0), (0, 6)),
        ]);
        // Capacitor straight across a source (companion conductance).
        ok(&[
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, ElementKind::Capacitor { farads: 1e-6 }, (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ]);
        // Zero-length wire self-loop.
        ok(&[ElementSpec::two(1, ElementKind::Wire, (2, 2), (2, 2))]);
        // A switch in SERIES with a source (the demo's lamp loop shape) is
        // fine in both states.
        for closed in [false, true] {
            ok(&[
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Switch { closed }, (0, 0), (4, 0)),
                ElementSpec::two(3, r(90.0), (4, 0), (0, 6)),
                ElementSpec::ground(4, (0, 6)),
            ]);
        }
    }

    #[test]
    fn hoist_shaped_fixture_is_accepted() {
        // Motor + sensor pot + both limit switches (bottom one closed, like
        // the crate at rest), a rail drive and a ground — the machine drive
        // every repro started from must stay placeable, including with both
        // limit switches closed (the worst-case pass).
        let d = vec![
            ElementSpec::two(
                900,
                ElementKind::Motor {
                    ohms: 2.0,
                    henries: 1.5e-3,
                    bemf: 0.0,
                },
                (57, 5),
                (57, 9),
            ),
            ElementSpec::three(
                901,
                ElementKind::Potentiometer {
                    ohms: 10_000.0,
                    wiper: 0.95,
                },
                (57, 12),
                (61, 14),
                (57, 16),
            ),
            ElementSpec::two(902, ElementKind::Switch { closed: false }, (57, 19), (61, 19)),
            ElementSpec::two(903, ElementKind::Switch { closed: true }, (57, 22), (61, 22)),
            ElementSpec {
                id: 1,
                kind: rail(5.0),
                pins: vec![(57, 5)],
            },
            ElementSpec::ground(2, (57, 9)),
        ];
        ok(&d);
        // But a battery across the closed LIM-BOT pair is refused, and one
        // across the open LIM-TOP pair is refused as a latent short.
        let mut with_bot = d.clone();
        with_bot.push(ElementSpec::two(10, dc(9.0), (57, 22), (61, 22)));
        rejected(&with_bot, Reject::Unsolvable);
        let mut with_top = d.clone();
        with_top.push(ElementSpec::two(10, dc(9.0), (57, 19), (61, 19)));
        rejected(&with_top, Reject::UnsolvableWhenSwitched);
    }

    #[test]
    fn reject_carries_code_id_and_hint() {
        let r = Reject::BadValue {
            id: 7,
            hint: "resistance must be a finite value between 1 uOhm and 1 TOhm",
        };
        assert_eq!(r.code(), "bad_value");
        assert_eq!(r.id(), Some(7));
        assert!(!r.hint().is_empty());
        assert_eq!(Reject::Unsolvable.id(), None);
        assert_eq!(
            Reject::UnsolvableWhenSwitched.code(),
            "unsolvable_switched"
        );
    }
}
