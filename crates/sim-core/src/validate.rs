//! Placement-time validation: decide whether a document is one the engine
//! can actually solve BEFORE it becomes the live netlist.
//!
//! This is the single implementation of the placement guard. The server
//! calls it on a candidate copy of the room document before committing any
//! mutation (doc edit, interact, repair, machine move); the client reaches
//! the same code through `sim-wasm` for pre-send hints. One implementation,
//! two callers — the two sides can never disagree about what is placeable.
//! Pure and deterministic: no I/O, no clocks, no hashing, no float ordering.
//!
//! Four layers:
//!
//! 1. **Value sanity** — every numeric parameter must be finite and inside
//!    the range the solver is conditioned for. These are the same bounds
//!    `InteractOp::SetValue` clamps to (extended with upper bounds), applied
//!    to the `Add`/`SetKind`/`SetValue` payloads that previously bypassed
//!    them. A `1/ohms` of a zero-ohm resistor is `inf` straight into the
//!    matrix; a 1e300 V source overflows every derived power figure.
//!
//! 2. **Structural diagnosis** — compile the candidate and look at the ideal
//!    zero-impedance constraints it imposes (see [`crate::constraint`]).
//!    Every degenerate arrangement of them is detectable here, by NAME and
//!    with the ids of the parts responsible:
//!
//!    * a source with both pins on one node — shorted;
//!    * two constraints on one node pair that do not agree — conflicting;
//!    * a cycle of constraints with nothing between them — a source loop.
//!
//!    Constraints that DO agree are merged by `Engine::compile` into one net
//!    and are not a rejection at all: two 5 V supplies, a 5 V supply and a
//!    5 V rail, two closed switches in parallel (two-way lighting) are all
//!    legal placements.
//!
//! 3. **Structural solvability** — stamp and LU-factor the candidate on a
//!    scratch [`Engine`]. The factor runs twice:
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
//!    Layer 3 is not redundant with layer 2. **Layer 2 exists to give a
//!    rejection a NAME; layer 3 exists to make sure a rejection HAPPENS.**
//!    The LU is the soundness guarantee and must never be removed: it is
//!    what catches any residual dependency the structural pass does not
//!    model.
//!
//! 4. **Convergence** — one trial step on the scratch engine. Factorability
//!    is not solvability: the shipped 9 V battery wired straight across the
//!    shipped LED factors perfectly and then freezes the room at t = 0,
//!    because an ideal source pins the junction voltage somewhere its
//!    exponential can never meet. Newton burns all 100 iterations, the
//!    rescue ladder halves dt four times, and the engine quarantines with
//!    ZERO steps completed — self-concealing, because the damage model is
//!    skipped while quarantined so the LED never even burns out. This is
//!    reachable with two catalog parts in the first minute of play, and it
//!    is far more likely than every structural failure combined.
//!
//! Broken parts are validated as if healthy: a broken part stamps nothing,
//! but `Repair` can put any of them back at any time, so a document is only
//! accepted if it solves with every part in service.
//!
//! What is deliberately ACCEPTED: floating subgraphs, dangling current
//! sources (GMIN keeps both solvable — normal mid-build states), capacitor
//! loops and inductor cutsets (finite companion conductances), coincident
//! pins on non-source parts, parallel motors (a motor is not ideal), and
//! every agreeing arrangement of ideal sources. Never reject a circuit the
//! engine can solve.

use crate::constraint::ConstraintKey;
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
/// Smallest op-amp output-current limit a document may carry.
pub const MIN_OPAMP_ISC: f64 = 1e-6;

/// Above this element count the convergence trial (layer 4) is skipped and
/// the gate falls back to structural checks only.
///
/// This is a STOPGAP and should be named as one. A trial step costs about
/// what the next real step costs, and the gate runs on the sim thread inside
/// the command drain, so at a few thousand elements one edit would consume a
/// whole 33 ms tick — which would violate "the sim never stalls the UI", the
/// invariant that decides the trade. The residual gap is honest and worth
/// stating: **above this cap the non-convergence class is still reachable.**
/// It is a first-minute-of-play, small-circuit failure, so the cap buys
/// nearly all of the value.
///
/// The real fix is not a bigger cap, it is the two quadratics in the compile
/// path (`Engine::compile` interns junctions with a linear scan;
/// `set_elements` restores per-element state with another), which dominate
/// long before the LU does. With those replaced the cap rises or disappears.
pub const TRIAL_MAX_ELEMENTS: usize = 400;

/// Up to four element ids carried by a [`Reject`], so the client can flash
/// every implicated part rather than just one. Fixed-size to keep `Reject`
/// `Copy`; `len` is the TRUE count, which may exceed what is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SmallIds {
    ids: [u32; 4],
    stored: u8,
    len: u8,
}

impl SmallIds {
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.ids[..self.stored as usize].iter().copied()
    }

    /// How many parts are implicated in total (may be more than `iter` yields).
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn from_slice(all: &[u32]) -> Self {
        let mut s = SmallIds {
            len: all.len().min(u8::MAX as usize) as u8,
            ..Default::default()
        };
        for (dst, src) in s.ids.iter_mut().zip(all.iter()) {
            *dst = *src;
            s.stored += 1;
        }
        s
    }

    fn of(items: &[u32]) -> Self {
        Self::from_slice(items)
    }
}

/// Why a document was refused. `code()` is the machine-readable reason for
/// the wire protocol; `hint()` is a human sentence the client can surface
/// as a diegetic DRC hint; `ids()` is every part to flash.
///
/// `Eq` is deliberately absent: `ConflictingSources` carries two f64 volt
/// readings, and they are finite by the time they are stored (layer 1 ran
/// first), so `PartialEq` is well-behaved. Nothing needs `Eq`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reject {
    /// A parameter is NaN/inf or outside the solver's safe range.
    BadValue { id: u32, hint: &'static str },
    /// A source-like part with both terminals on the same grid point: its
    /// branch row would be all zeros.
    CollapsedPins { id: u32 },
    /// Both ends of an ideal source resolve to ONE electrical node — a wire
    /// across it, or a rail dropped on ground. Same all-zero row as
    /// `CollapsedPins`, but reached through wire closure rather than
    /// geometry, so the pins can be far apart on screen.
    ShortedSource { id: u32 },
    /// Two ideal constraints on one node pair that do not agree. `va`/`vb`
    /// are the two DC levels measured across that pair IN THE SAME
    /// DIRECTION, so they are directly comparable even when the two parts
    /// were drawn facing opposite ways.
    ConflictingSources { a: u32, b: u32, va: f64, vb: f64 },
    /// A cycle of ideal sources with nothing between them. Dependent
    /// incidence columns whether or not the voltages happen to sum to zero:
    /// the current around the loop is undetermined either way.
    SourceLoop { ids: SmallIds },
    /// Factors, but Newton-Raphson finds no operating point — the room would
    /// quarantine on its first step. An ideal source straight across a diode
    /// or LED is the usual cause; `id` names it when it can be identified.
    WillNotConverge { id: Option<u32> },
    /// The document as it stands has no solution (singular MNA matrix) for a
    /// reason the structural pass does not model. Backstop, not a diagnosis.
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
            Reject::ShortedSource { .. } => "shorted_source",
            Reject::ConflictingSources { .. } => "conflicting_sources",
            Reject::SourceLoop { .. } => "source_loop",
            Reject::WillNotConverge { .. } => "will_not_converge",
            Reject::Unsolvable => "unsolvable",
            Reject::UnsolvableWhenSwitched => "unsolvable_switched",
        }
    }

    /// The element the refusal is pinned to, when it is one element's fault.
    /// For multi-part refusals this is the primary one; use [`Reject::ids`]
    /// for the rest.
    pub fn id(&self) -> Option<u32> {
        match self {
            Reject::BadValue { id, .. }
            | Reject::CollapsedPins { id }
            | Reject::ShortedSource { id } => Some(*id),
            Reject::ConflictingSources { a, .. } => Some(*a),
            Reject::SourceLoop { ids } => ids.iter().next(),
            Reject::WillNotConverge { id } => *id,
            Reject::Unsolvable | Reject::UnsolvableWhenSwitched => None,
        }
    }

    /// Every part implicated, for the client to flash. Empty only for the
    /// two structural backstops — no NAMED refusal is ever anonymous.
    pub fn ids(&self) -> SmallIds {
        match self {
            Reject::BadValue { id, .. }
            | Reject::CollapsedPins { id }
            | Reject::ShortedSource { id } => SmallIds::of(&[*id]),
            Reject::ConflictingSources { a, b, .. } => SmallIds::of(&[*a, *b]),
            Reject::SourceLoop { ids } => *ids,
            Reject::WillNotConverge { id } => match id {
                Some(id) => SmallIds::of(&[*id]),
                None => SmallIds::default(),
            },
            Reject::Unsolvable | Reject::UnsolvableWhenSwitched => SmallIds::default(),
        }
    }

    /// A sentence for the player. Reads like a DRC callout from a bench
    /// instrument: lowercase, diagnosis then remedy, EE-literate but never
    /// jargon for its own sake.
    pub fn hint(&self) -> String {
        match self {
            Reject::BadValue { hint, .. } => (*hint).to_string(),
            Reject::CollapsedPins { .. } => {
                "both terminals sit on the same point - stretch the part out".to_string()
            }
            Reject::ShortedSource { .. } => {
                "both ends of this source land on the same net - it is shorted out. \
                 move a terminal, or remove the wire across it"
                    .to_string()
            }
            Reject::ConflictingSources { va, vb, .. } => conflict_hint(*va, *vb),
            Reject::SourceLoop { .. } => {
                "these sources form a loop with nothing between them - the current \
                 around it is undefined. break the loop, or put a load in it"
                    .to_string()
            }
            Reject::WillNotConverge { .. } => {
                "no operating point exists for this - an ideal source straight across \
                 a diode or LED is the usual cause. give it a series resistor"
                    .to_string()
            }
            Reject::Unsolvable => {
                "no solution exists for that circuit - look for shorted, looped or \
                 conflicting sources"
                    .to_string()
            }
            Reject::UnsolvableWhenSwitched => {
                "that would short a source the moment a switch closes - reroute it".to_string()
            }
        }
    }
}

// ------------------------------------------------------------ value display

/// SI-prefixed, 3 significant figures, trailing zeros trimmed. Threshold
/// table rather than a log: no transcendental on any path, and the output is
/// identical on native and wasm32.
fn si_volts(v: f64) -> String {
    if !v.is_finite() {
        return format!("{v} V");
    }
    let a = v.abs();
    let (scale, prefix) = if a == 0.0 {
        (1.0, "")
    } else if a >= 1e9 {
        (1e9, "G")
    } else if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else if a >= 1.0 {
        (1.0, "")
    } else if a >= 1e-3 {
        (1e-3, "m")
    } else if a >= 1e-6 {
        (1e-6, "u")
    } else {
        (1e-9, "n")
    };
    let s = v / scale;
    let m = s.abs();
    let mut t = if m < 10.0 {
        format!("{s:.2}")
    } else if m < 100.0 {
        format!("{s:.1}")
    } else {
        format!("{s:.0}")
    };
    if t.contains('.') {
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
    }
    format!("{t} {prefix}V")
}

/// The conflict sentence, in three tiers.
///
/// Two constraints can key apart while DISPLAYING the same — the merge
/// tolerance is 2⁻⁴⁰ relative and the display is 3 significant figures — and
/// "5 V and 5 V wired to the same net" would be the single most infuriating
/// string in the game. So: show the difference at full precision when the
/// short form hides it, and say what is really different when even that does
/// not (equal DC, different waveform or polarity).
fn conflict_hint(va: f64, vb: f64) -> String {
    let (sa, sb) = (si_volts(va), si_volts(vb));
    if sa != sb {
        return format!(
            "{sa} and {sb} wired to the same net - two supplies on one net have to \
             agree, or stay apart"
        );
    }
    let (ea, eb) = (format!("{va:.12e}"), format!("{vb:.12e}"));
    if ea != eb {
        return format!(
            "{ea} V and {eb} V wired to the same net - two supplies on one net have \
             to agree, or stay apart"
        );
    }
    "two supplies wired to the same net do not match - same DC level, but a \
     different waveform or polarity. one of them has to go"
        .to_string()
}

// ------------------------------------------------------------- value sanity

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
        K::OpAmp { rail, isc } => {
            if !in_range(rail, 0.0, MAX_SOURCE_VOLTS) {
                return Err("op-amp rail must be a finite value between 0 and 1 MV");
            }
            // A zero-isc op-amp would stamp a 0 A current source in the
            // limited region — legal, but it would be a part that cannot do
            // anything, and the branch row would be indistinguishable from a
            // typo. The floor is 1 uA (below any real part) and the ceiling
            // is the same 1 MA every other source gets.
            if !in_range(isc, MIN_OPAMP_ISC, MAX_SOURCE_AMPS) {
                return Err("op-amp output current limit must be between 1 uA and 1 MA");
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

// ------------------------------------------------------ structural diagnosis

/// One distinct ideal constraint in the compiled document, in first-seen
/// (document) order.
struct Group {
    key: ConstraintKey,
    a: usize,
    b: usize,
    /// The first element that imposed it — the one named in a conflict.
    id: u32,
    /// The DC level across `(a, b)`, in the canonical direction.
    nominal: f64,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Name the degeneracy, if the document has one. Runs on a COMPILED engine,
/// so it sees real node numbers after wire closure and ground merging — a
/// wire across a battery and a battery with coincident pins are the same
/// thing here, which is exactly right.
///
/// Every rejection this returns is one the LU would also refuse; it never
/// refuses anything the LU would accept. Each case is a provable rank
/// deficiency:
///
/// * **shorted** — the row is identically zero;
/// * **conflict** — two branch columns with the same incidence pattern, so
///   one is ± the other;
/// * **loop** — the oriented incidence columns around a cycle telescope to
///   zero (dropping the node-0 row does not change that).
///
/// Deterministic throughout: linear scans in document order, integer key
/// comparison, no hashing.
fn diagnose(eng: &Engine) -> Result<(), Reject> {
    let cs = eng.ideal_constraints();
    if cs.is_empty() {
        return Ok(());
    }

    // 1. Shorted out: both ends on one node.
    for (id, c) in &cs {
        if c.is_shorted() {
            return Err(Reject::ShortedSource { id: *id });
        }
    }

    // 2. Distinct constraints. Equal key = the same net, already accounted
    //    for and merged by `compile`. A DIFFERENT key on a node pair we
    //    already constrain is a genuine contradiction.
    let mut groups: Vec<Group> = Vec::new();
    for (id, c) in &cs {
        let key = c.key();
        if groups.iter().any(|g| g.key == key) {
            continue; // same net as something already seen
        }
        if let Some(g) = groups.iter().find(|g| (g.a, g.b) == (c.a, c.b)) {
            return Err(Reject::ConflictingSources {
                a: g.id,
                b: *id,
                va: g.nominal,
                vb: c.nominal(),
            });
        }
        groups.push(Group {
            key,
            a: c.a,
            b: c.b,
            id: *id,
            nominal: c.nominal(),
        });
    }

    // 3. Source loops. One edge per distinct constraint (a merged group is
    //    ONE edge — that is the whole point), over nodes 0..=num_nodes.
    //    A cycle of ideal sources is always a rejection, whether or not the
    //    voltages sum to zero: the currents are undetermined either way.
    //    Conveniently, that means no float comparison is needed to decide it.
    let nn = eng.node_count() + 1;
    let mut parent: Vec<usize> = (0..nn).collect();
    let mut accepted: Vec<(usize, usize, u32)> = Vec::new();
    for g in &groups {
        let (ra, rb) = (find(&mut parent, g.a), find(&mut parent, g.b));
        if ra == rb {
            let mut ids = trace_loop(&accepted, g.a, g.b);
            ids.push(g.id);
            return Err(Reject::SourceLoop {
                ids: SmallIds::of(&ids),
            });
        }
        parent[ra] = rb;
        accepted.push((g.a, g.b, g.id));
    }
    Ok(())
}

/// The already-accepted constraints on the path from `from` to `to` — the
/// rest of the loop the closing edge completes, so the message can name the
/// whole cycle instead of one arbitrary member. Breadth-first over a forest,
/// so the path is unique and the traversal order is the document order the
/// edges were accepted in.
fn trace_loop(accepted: &[(usize, usize, u32)], from: usize, to: usize) -> Vec<u32> {
    // (previous node, edge id) per visited node.
    let mut came: Vec<Option<(usize, u32)>> = Vec::new();
    let mut seen: Vec<usize> = vec![from];
    let mut queue: Vec<usize> = vec![from];
    came.push(None);
    let mut head = 0;
    while head < queue.len() {
        let cur = queue[head];
        head += 1;
        if cur == to {
            break;
        }
        for (a, b, id) in accepted {
            let next = if *a == cur {
                *b
            } else if *b == cur {
                *a
            } else {
                continue;
            };
            if seen.contains(&next) {
                continue;
            }
            seen.push(next);
            came.push(Some((cur, *id)));
            queue.push(next);
        }
    }
    let mut path = Vec::new();
    let mut cur = to;
    while cur != from {
        let Some(i) = seen.iter().position(|s| *s == cur) else {
            break;
        };
        let Some((prev, id)) = came[i] else { break };
        path.push(id);
        cur = prev;
    }
    path.reverse();
    path
}

/// The nonlinear junction device most likely to be the reason Newton could
/// not converge: a diode/LED/zener sitting directly across an ideal
/// constraint. That is not a guess about the numerics — an ideal source pins
/// the junction voltage to a value its exponential can never satisfy, which
/// is precisely the shipped-battery-across-the-shipped-LED case.
fn blame_for_divergence(eng: &Engine, specs: &[ElementSpec]) -> Option<u32> {
    let cs = eng.ideal_constraints();
    let nodes = eng.element_nodes();
    for s in specs {
        if !matches!(
            s.kind,
            ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. }
        ) {
            continue;
        }
        let (_, n) = nodes.iter().find(|(id, _)| *id == s.id)?;
        let pair = if n[0] <= n[1] {
            (n[0], n[1])
        } else {
            (n[1], n[0])
        };
        if cs.iter().any(|(_, c)| (c.a, c.b) == pair) {
            return Some(s.id);
        }
    }
    None
}

// ---------------------------------------------------------------- the gate

/// Would the engine accept this document? `Ok(())` = every parameter is in
/// range, no ideal constraint is shorted, conflicting or looped, the MNA
/// matrix factors both as placed and with every switch closed, and Newton
/// finds an operating point. Pure and deterministic. `dt` should be the
/// timestep the live engine runs at (companion conductances depend on it;
/// structural singularity does not).
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
        // Tier and rotation never reach a stamp, so neither can make a
        // document unsolvable — but both are indices into small tables
        // (the damage crate's rating rows, the renderer's four quarter
        // turns) and an out-of-range one is a stale or hostile client, not
        // a placement. Refusing here means neither table ever needs a
        // defensive clamp for a value that should not exist.
        if s.tier > crate::netlist::MAX_TIER {
            return Err(Reject::BadValue {
                id: s.id,
                hint: "that part tier does not exist yet",
            });
        }
        if s.rot > 3 {
            return Err(Reject::BadValue {
                id: s.id,
                hint: "rotation must be 0, 1, 2 or 3 quarter turns",
            });
        }
        if collapses_when_coincident(&s.kind) && s.pins[0] == s.pins[1] {
            return Err(Reject::CollapsedPins { id: s.id });
        }
    }

    // As placed: name the degeneracy first, then let the LU catch whatever
    // the structural pass does not model.
    let mut eng = Engine::new(dt);
    eng.set_elements(specs);
    diagnose(&eng)?;
    if !eng.probe_solvable() {
        return Err(Reject::Unsolvable);
    }

    // Worst case: every switch and button closed. Closing only ever adds
    // 0 V branch rows (it merges no nodes), so any singular mixed state is
    // singular here too — one factorization covers all 2^n combinations.
    //
    // The generic code is kept on purpose. "That would short a source the
    // moment a switch closes" is the right register for a LATENT fault; a
    // structural name here would describe a state the player is not in yet.
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
        if diagnose(&eng).is_err() || !eng.probe_solvable() {
            return Err(Reject::UnsolvableWhenSwitched);
        }
    }

    // Convergence. A linear document takes the single-pass path and cannot
    // fail to converge, so the common big-document case (resistor ladders,
    // wire nets) pays nothing at all.
    //
    // ONE step is enough and is not a compromise: this class is a DC
    // operating-point failure, visible on the very first step with zero
    // steps completed. Deeper trials cost linearly and catch nothing more.
    //
    // As-placed only, not the all-switches-closed clone: convergence is a
    // property of the operating point the player is actually in, and the
    // structural pass already covers the closed states. Deliberate one-sided
    // conservatism gap, recorded rather than hidden.
    if specs.len() <= TRIAL_MAX_ELEMENTS {
        let mut trial = Engine::new(dt);
        trial.set_elements(specs);
        if !trial.is_linear() && trial.advance(1).quarantined {
            return Err(Reject::WillNotConverge {
                id: blame_for_divergence(&trial, specs),
            });
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

    fn ac(dc: f64, amp: f64, hz: f64, phase: f64) -> ElementKind {
        ElementKind::VoltageSource { dc, amp, hz, phase }
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

    fn rail_at(id: u32, volts: f64, at: (i32, i32)) -> ElementSpec {
        ElementSpec {
            id,
            kind: rail(volts),
            pins: vec![at],
            ..Default::default()
        }
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

    fn reject_of(specs: &[ElementSpec]) -> Reject {
        check_document(specs, DT).expect_err("expected a rejection")
    }

    // ====================================================================
    // THE ACCEPT SIDE: same-source merging.
    //
    // "It's possible to connect two 5 V sources together not because they
    // are the same voltage but because we make the assumption that all 5 V
    // sources are from the same source."
    // ====================================================================

    #[test]
    fn identical_parallel_sources_are_one_net() {
        // Two 9 V batteries across the same pair: one net, one row.
        let mut d = base();
        d.push(ElementSpec::two(4, dc(9.0), (0, 0), (0, 6)));
        ok(&d);
        // Three of them.
        d.push(ElementSpec::two(5, dc(9.0), (0, 0), (0, 6)));
        ok(&d);
        // And the same source drawn the other way with the negated value is
        // literally the same constraint.
        let mut d = base();
        d.push(ElementSpec::two(4, dc(-9.0), (0, 6), (0, 0)));
        ok(&d);
    }

    #[test]
    fn identical_parallel_rails_are_one_net() {
        let d = vec![
            rail_at(1, 5.0, (0, 0)),
            rail_at(2, 5.0, (0, 0)),
            ElementSpec::two(3, r(1000.0), (0, 0), (8, 0)),
            ElementSpec::ground(4, (8, 0)),
        ];
        ok(&d);
    }

    #[test]
    fn a_rail_and_a_grounded_source_of_the_same_voltage_are_one_net() {
        // A Rail folds to (node, 0) because its return path IS ground, so
        // this is the same rule as source-vs-source and needs no special
        // case. Both directions of the source.
        for (a, b, volts) in [((0, 0), (0, 6), 5.0), ((0, 6), (0, 0), -5.0)] {
            let d = vec![
                rail_at(1, 5.0, (0, 0)),
                ElementSpec::two(2, dc(volts), a, b),
                ElementSpec::ground(3, (0, 6)),
                ElementSpec::two(4, r(1000.0), (0, 0), (0, 6)),
            ];
            ok(&d);
        }
    }

    #[test]
    fn parallel_closed_switches_are_one_net() {
        // Two-way lighting, an OR contact, a manual override across a relay.
        // A closed switch is a 0 V source, so this is the identical
        // singularity as parallel batteries and used to be refused.
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(4, r(90.0), (4, 0), (0, 6)),
            ElementSpec::ground(5, (0, 6)),
        ];
        ok(&d);
        // Drawn in opposite directions, and mixed with a Button.
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Button { closed: true }, (4, 0), (0, 0)),
            ElementSpec::two(4, r(90.0), (4, 0), (0, 6)),
            ElementSpec::ground(5, (0, 6)),
        ];
        ok(&d);
    }

    #[test]
    fn parallel_open_switches_are_accepted_and_stay_accepted_when_closed() {
        // The worst-case pass forces both closed; they merge there too.
        for (x, y) in [(false, false), (true, false), (false, true)] {
            let d = vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Switch { closed: x }, (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Switch { closed: y }, (0, 0), (4, 0)),
                ElementSpec::two(4, r(90.0), (4, 0), (0, 6)),
                ElementSpec::ground(5, (0, 6)),
            ];
            ok(&d);
        }
    }

    #[test]
    fn identical_ac_sources_in_parallel_are_one_net() {
        let d = vec![
            ElementSpec::two(1, ac(0.0, 5.0, 50.0, 0.25), (0, 0), (0, 6)),
            ElementSpec::two(2, ac(0.0, 5.0, 50.0, 0.25), (0, 0), (0, 6)),
            ElementSpec::two(3, r(1000.0), (0, 0), (0, 6)),
            ElementSpec::ground(4, (0, 6)),
        ];
        ok(&d);
    }

    #[test]
    fn merging_keeps_the_solver_honest() {
        // The merged net carries the same TOTAL current a single source
        // would, and each member reports its share. Nothing is invented:
        // the total is the solver's, the split is the only permutation-
        // invariant division of it.
        let one = vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(9, r(10.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        let mut two = one.clone();
        two.push(ElementSpec::two(2, dc(10.0), (0, 0), (0, 6)));
        ok(&one);
        ok(&two);

        let load_current = |specs: &[ElementSpec]| -> f64 {
            let mut e = Engine::new(DT);
            e.set_elements(specs);
            e.advance(4);
            e.frame()
                .into_iter()
                .find(|f| f.id == 9)
                .expect("load")
                .i[0]
        };
        let source_currents = |specs: &[ElementSpec]| -> Vec<f64> {
            let mut e = Engine::new(DT);
            e.set_elements(specs);
            e.advance(4);
            e.frame()
                .into_iter()
                .filter(|f| f.id == 1 || f.id == 2)
                .map(|f| f.i[0])
                .collect()
        };

        // 10 V across 10 ohm: 1 A, whether one supply or two.
        let i1 = load_current(&one);
        let i2 = load_current(&two);
        assert!((i1 - 1.0).abs() < 1e-6, "one supply: {i1}");
        assert!((i2 - 1.0).abs() < 1e-6, "two supplies: {i2}");
        // The single supply delivers all of it; the pair splits it evenly
        // and still sums to the same total.
        let s1 = source_currents(&one);
        let s2 = source_currents(&two);
        assert_eq!(s1.len(), 1);
        assert_eq!(s2.len(), 2);
        assert!((s1[0].abs() - 1.0).abs() < 1e-6, "{s1:?}");
        assert!((s2[0].abs() - 0.5).abs() < 1e-6, "{s2:?}");
        assert!((s2[1].abs() - 0.5).abs() < 1e-6, "{s2:?}");
        assert!(
            (s2[0] + s2[1] - s1[0]).abs() < 1e-9,
            "the total must be the solver's: {s2:?} vs {s1:?}"
        );
    }

    #[test]
    fn merged_members_drawn_opposite_ways_report_the_same_current() {
        // Orientation is recorded, not lost: a member drawn backwards reads
        // its share of the shared branch current with the opposite sign, so
        // both parts report current flowing the same physical way.
        let d = vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(-10.0), (0, 6), (0, 0)),
            ElementSpec::two(9, r(10.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        ok(&d);
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        e.advance(4);
        let f = e.frame();
        let a = f.iter().find(|f| f.id == 1).unwrap();
        let b = f.iter().find(|f| f.id == 2).unwrap();
        // Element 2's pin 0 sits where element 1's pin 1 sits, so their
        // pin-0 currents are opposite and equal in magnitude.
        assert!((a.i[0] + b.i[0]).abs() < 1e-9, "{} vs {}", a.i[0], b.i[0]);
        assert!((a.i[0].abs() - 0.5).abs() < 1e-6, "{}", a.i[0]);
    }

    #[test]
    fn breaking_one_of_a_merged_pair_regroups_the_survivors() {
        // Group membership is recomputed inside `compile()` every time and
        // never cached across it, so a break drops the member out and the
        // next one becomes leader — automatically, with no bookkeeping.
        let d = vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(9, r(10.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        e.advance(4);
        let half = e.frame().into_iter().find(|f| f.id == 1).unwrap().i[0];
        assert!((half.abs() - 0.5).abs() < 1e-6, "{half}");
        // Break the LEADER: the survivor must carry the whole current.
        assert!(e.set_broken(1, true));
        e.advance(4);
        let f = e.frame();
        assert_eq!(f.iter().find(|f| f.id == 1).unwrap().i[0], 0.0);
        let all = f.iter().find(|f| f.id == 2).unwrap().i[0];
        assert!((all.abs() - 1.0).abs() < 1e-6, "{all}");
        // The load never noticed.
        let load = f.iter().find(|f| f.id == 9).unwrap().i[0];
        assert!((load.abs() - 1.0).abs() < 1e-6, "{load}");
    }

    #[test]
    fn changing_one_members_value_splits_the_group() {
        // `interact` routes through `compile()`, so re-keying is automatic.
        let d = vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(10.0), (0, 0), (0, 6)),
            ElementSpec::two(9, r(10.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        assert_eq!(e.branch_count(), 1, "merged");
        e.interact(2, crate::netlist::InteractOp::SetValue { value: 12.0 });
        assert_eq!(e.branch_count(), 2, "split once the values differ");
        // And the gate now refuses that document, by name.
        let mut d2 = d.clone();
        d2[1] = ElementSpec::two(2, dc(12.0), (0, 0), (0, 6));
        assert!(matches!(
            reject_of(&d2),
            Reject::ConflictingSources { a: 1, b: 2, .. }
        ));
    }

    // ====================================================================
    // THE REJECT SIDE: still refused, now by name.
    // ====================================================================

    #[test]
    fn a_wire_across_a_source_is_a_named_short() {
        let mut d = base();
        d.push(ElementSpec::two(4, ElementKind::Wire, (0, 0), (0, 6)));
        rejected(&d, Reject::ShortedSource { id: 1 });
        // Reached through a chain of wires, pins far apart on screen.
        let mut d = base();
        d.push(ElementSpec::two(4, ElementKind::Wire, (0, 0), (4, 0)));
        d.push(ElementSpec::two(5, ElementKind::Wire, (4, 0), (0, 6)));
        rejected(&d, Reject::ShortedSource { id: 1 });
    }

    #[test]
    fn disagreeing_parallel_sources_name_both_parts() {
        let mut d = base();
        d.push(ElementSpec::two(4, dc(1.0), (0, 0), (0, 6)));
        let got = reject_of(&d);
        assert!(
            matches!(got, Reject::ConflictingSources { a: 1, b: 4, .. }),
            "{got:?}"
        );
        assert_eq!(got.ids().iter().collect::<Vec<_>>(), vec![1, 4]);
        assert!(got.hint().contains("9 V"), "{}", got.hint());
        assert!(got.hint().contains("1 V"), "{}", got.hint());
    }

    #[test]
    fn anti_parallel_equal_sources_are_a_conflict_not_a_net() {
        // +5 against -5 across one pair: shorted, that is 10 V across zero
        // ohms. Same magnitude is NOT the same constraint.
        let d = vec![
            ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (0, 0)),
            ElementSpec::ground(3, (0, 6)),
        ];
        assert!(matches!(
            reject_of(&d),
            Reject::ConflictingSources { a: 1, b: 2, .. }
        ));
    }

    #[test]
    fn a_dc_source_and_an_ac_source_of_the_same_level_conflict() {
        // They agree only where sin = 0; at every other instant they demand
        // different voltages on one node pair.
        let d = vec![
            ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
            ElementSpec::two(2, ac(5.0, 1.0, 50.0, 0.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        let got = reject_of(&d);
        assert!(matches!(got, Reject::ConflictingSources { .. }), "{got:?}");
        // The DC levels render identically, so the message must NOT read
        // "5 V and 5 V" — it says what is really different.
        let h = got.hint();
        assert!(h.contains("waveform"), "{h}");
    }

    #[test]
    fn disagreeing_rails_on_one_node_name_both_parts() {
        let d = vec![rail_at(1, 5.0, (0, 0)), rail_at(2, 12.0, (0, 0))];
        let got = reject_of(&d);
        assert!(
            matches!(got, Reject::ConflictingSources { a: 1, b: 2, .. }),
            "{got:?}"
        );
    }

    #[test]
    fn a_rail_sitting_on_ground_is_a_named_short() {
        let d = vec![rail_at(1, 12.0, (0, 0)), ElementSpec::ground(2, (0, 0))];
        rejected(&d, Reject::ShortedSource { id: 1 });
        // A 0 V rail on ground is the same degeneracy (`0·i = 0`).
        let d = vec![rail_at(1, 0.0, (0, 0)), ElementSpec::ground(2, (0, 0))];
        rejected(&d, Reject::ShortedSource { id: 1 });
    }

    #[test]
    fn a_source_from_a_rail_node_to_ground_conflicts_with_the_rail() {
        // The loop through the rail's implicit ground return, invisible on
        // screen. Both are constraints on (0, rail node), so it surfaces as
        // the more specific conflict rather than as a loop.
        let d = vec![
            rail_at(1, 12.0, (0, 0)),
            ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
        ];
        assert!(matches!(
            reject_of(&d),
            Reject::ConflictingSources { a: 1, b: 2, .. }
        ));
    }

    #[test]
    fn a_source_loop_names_every_member() {
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
            ElementSpec::two(3, dc(3.0), (6, 6), (0, 0)),
        ];
        let got = reject_of(&d);
        let Reject::SourceLoop { ids } = got else {
            panic!("{got:?}")
        };
        let mut v: Vec<u32> = ids.iter().collect();
        v.sort_unstable();
        assert_eq!(v, vec![1, 2, 3]);
        assert_eq!(ids.len(), 3);
        // A consistent loop (voltages summing to zero) is refused too: the
        // currents around it are undetermined either way.
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
            ElementSpec::two(3, dc(-14.0), (6, 6), (0, 0)),
        ];
        assert!(matches!(reject_of(&d), Reject::SourceLoop { .. }));
        // A closed switch closing a loop of sources counts as a member.
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
            ElementSpec::two(3, ElementKind::Switch { closed: true }, (6, 6), (0, 0)),
        ];
        assert!(matches!(reject_of(&d), Reject::SourceLoop { .. }));
    }

    #[test]
    fn coincident_source_pins_are_still_the_geometric_reject() {
        let d = vec![ElementSpec::two(1, dc(9.0), (0, 0), (0, 0))];
        rejected(&d, Reject::CollapsedPins { id: 1 });
    }

    #[test]
    fn switch_across_a_source_is_rejected_in_both_states() {
        // Closed: a 0 V constraint against a 9 V constraint on one pair.
        let mut d = base();
        d.push(ElementSpec::two(
            4,
            ElementKind::Switch { closed: true },
            (0, 0),
            (0, 6),
        ));
        assert!(matches!(
            reject_of(&d),
            Reject::ConflictingSources { a: 1, b: 4, .. }
        ));
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
        // The LIM-BOT repro: a wire across a closed switch collapses its two
        // pins onto one node.
        let d = vec![
            ElementSpec::two(1, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(2, ElementKind::Wire, (0, 0), (4, 0)),
        ];
        rejected(&d, Reject::ShortedSource { id: 1 });
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

    // ====================================================================
    // CONVERGENCE: the class that froze rooms at t = 0.
    // ====================================================================

    #[test]
    fn an_ideal_source_straight_across_a_junction_is_refused() {
        // The shipped 9 V battery and the shipped LED, two catalog parts and
        // one wire. Used to pass the gate and quarantine the room at
        // t = 0.000000 with zero steps completed — and conceal itself,
        // because damage is skipped while quarantined so the LED never even
        // burnt out.
        for (kind, why) in [
            (ElementKind::Led { color: 0 }, "LED"),
            (ElementKind::Diode, "diode"),
            (ElementKind::Zener { vz: 5.1 }, "zener"),
        ] {
            let d = vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, kind, (0, 0), (0, 6)),
                ElementSpec::ground(3, (0, 6)),
            ];
            let got = reject_of(&d);
            assert!(
                matches!(got, Reject::WillNotConverge { id: Some(2) }),
                "{why}: {got:?}"
            );
            assert!(got.hint().contains("series resistor"), "{}", got.hint());
        }
    }

    #[test]
    fn the_same_led_with_a_series_resistor_is_fine() {
        for ohms in [1.0, 330.0] {
            ok(&[
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, r(ohms), (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 6)),
                ElementSpec::ground(4, (0, 6)),
            ]);
        }
    }

    // ====================================================================
    // NEVER REJECT A VALID CIRCUIT.
    // ====================================================================

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
    fn series_sources_and_separate_rails_are_accepted() {
        // Two 5 V sources in series: a chain, not a loop.
        ok(&[
            ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
            ElementSpec::two(2, dc(5.0), (0, 6), (0, 12)),
            ElementSpec::two(3, r(1000.0), (0, 0), (0, 12)),
            ElementSpec::ground(4, (0, 12)),
        ]);
        // Two 5 V rails at DIFFERENT nodes: different pairs, never singular.
        // (Labeled nets would make these one net; that is a separate,
        // deliberate decision and is NOT taken here.)
        ok(&[
            rail_at(1, 5.0, (0, 0)),
            rail_at(2, 5.0, (8, 0)),
            ElementSpec::two(3, r(1000.0), (0, 0), (0, 6)),
            ElementSpec::two(4, r(1000.0), (8, 0), (0, 6)),
            ElementSpec::ground(5, (0, 6)),
        ]);
        // Two grounds at different points (both are node 0).
        ok(&[
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, r(100.0), (0, 0), (0, 6)),
            ElementSpec::ground(3, (0, 6)),
            ElementSpec::ground(4, (20, 20)),
        ]);
    }

    #[test]
    fn parallel_motors_are_accepted_and_not_merged() {
        // A motor stamps -(R + L/h) on its own diagonal, so parallel motors
        // are well-posed. Merging them would be wrong: they are two separate
        // loads, each with its own current.
        let motor = ElementKind::Motor {
            ohms: 2.0,
            henries: 1.5e-3,
            bemf: 0.0,
        };
        let d = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
            ElementSpec::two(2, motor, (0, 0), (0, 6)),
            ElementSpec::two(3, motor, (0, 0), (0, 6)),
            ElementSpec::ground(4, (0, 6)),
        ];
        ok(&d);
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        assert_eq!(e.branch_count(), 3, "one source + two independent motors");
    }

    #[test]
    fn a_bridge_rectifier_is_accepted() {
        // Four diodes, an AC source and a smoothing cap: the classic circuit
        // that must never be refused.
        let (ac_hi, ac_lo) = ((0, 0), (0, 12));
        let (plus, minus) = ((8, 0), (8, 12));
        ok(&[
            ElementSpec::two(1, ac(0.0, 12.0, 50.0, 0.0), ac_hi, ac_lo),
            ElementSpec::two(2, ElementKind::Diode, ac_hi, plus),
            ElementSpec::two(3, ElementKind::Diode, ac_lo, plus),
            ElementSpec::two(4, ElementKind::Diode, minus, ac_hi),
            ElementSpec::two(5, ElementKind::Diode, minus, ac_lo),
            ElementSpec::two(6, ElementKind::Capacitor { farads: 100e-6 }, plus, minus),
            ElementSpec::two(7, r(1000.0), plus, minus),
            ElementSpec::ground(8, minus),
        ]);
    }

    #[test]
    fn a_motor_driven_through_a_switch_is_accepted() {
        for closed in [false, true] {
            ok(&[
                ElementSpec::two(1, dc(12.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Switch { closed }, (0, 0), (4, 0)),
                ElementSpec::two(
                    3,
                    ElementKind::Motor {
                        ohms: 2.0,
                        henries: 1.5e-3,
                        bemf: 3.0,
                    },
                    (4, 0),
                    (0, 6),
                ),
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
            rail_at(1, 5.0, (57, 5)),
            ElementSpec::ground(2, (57, 9)),
        ];
        ok(&d);
        // A battery across the closed LIM-BOT pair is a 9 V constraint
        // against the switch's 0 V one, named with both ids.
        let mut with_bot = d.clone();
        with_bot.push(ElementSpec::two(10, dc(9.0), (57, 22), (61, 22)));
        assert!(matches!(
            reject_of(&with_bot),
            Reject::ConflictingSources { a: 903, b: 10, .. }
        ));
        // And one across the open LIM-TOP pair is refused as a latent short.
        let mut with_top = d.clone();
        with_top.push(ElementSpec::two(10, dc(9.0), (57, 19), (61, 19)));
        rejected(&with_top, Reject::UnsolvableWhenSwitched);
    }

    // ====================================================================
    // DETERMINISM AND PROTOCOL
    // ====================================================================

    #[test]
    fn grouping_does_not_depend_on_element_order() {
        // The merge is a linear scan over an integer key in document order:
        // reordering changes WHICH member leads a group, never whether the
        // group forms, and never the verdict.
        let a = ElementSpec::two(1, dc(9.0), (0, 0), (0, 6));
        let b = ElementSpec::two(2, dc(9.0), (0, 0), (0, 6));
        let c = ElementSpec::two(3, dc(9.0), (0, 6), (0, 0)); // conflicting
        let load = ElementSpec::two(9, r(10.0), (0, 0), (0, 6));
        let g = ElementSpec::ground(4, (0, 6));
        for order in [
            vec![a.clone(), b.clone(), load.clone(), g.clone()],
            vec![load.clone(), b.clone(), a.clone(), g.clone()],
            vec![g.clone(), load.clone(), a.clone(), b.clone()],
        ] {
            ok(&order);
            let mut e = Engine::new(DT);
            e.set_elements(&order);
            assert_eq!(e.branch_count(), 1, "one merged net in every order");
        }
        for order in [
            vec![a.clone(), c.clone(), load.clone(), g.clone()],
            vec![c.clone(), a.clone(), load.clone(), g.clone()],
        ] {
            assert!(
                matches!(reject_of(&order), Reject::ConflictingSources { .. }),
                "a conflict is a conflict in any order"
            );
        }
    }

    #[test]
    fn near_equal_values_merge_but_a_real_difference_does_not() {
        // Representation noise (a hand-edited save, a log-scale value drag)
        // must not split a net...
        let mut d = base();
        d.push(ElementSpec::two(4, dc(9.0 + 9.0 * 1e-14), (0, 0), (0, 6)));
        ok(&d);
        // ...but anything a player could have meant must.
        let mut d = base();
        d.push(ElementSpec::two(4, dc(9.0 * (1.0 + 1e-6)), (0, 0), (0, 6)));
        assert!(matches!(
            reject_of(&d),
            Reject::ConflictingSources { a: 1, b: 4, .. }
        ));
    }

    #[test]
    fn reject_carries_code_id_ids_and_hint() {
        let r = Reject::BadValue {
            id: 7,
            hint: "resistance must be a finite value between 1 uOhm and 1 TOhm",
        };
        assert_eq!(r.code(), "bad_value");
        assert_eq!(r.id(), Some(7));
        assert!(!r.hint().is_empty());
        assert_eq!(Reject::Unsolvable.id(), None);
        assert_eq!(Reject::UnsolvableWhenSwitched.code(), "unsolvable_switched");
        // Every NAMED refusal carries at least one part to flash. Only the
        // two structural backstops are anonymous.
        for r in [
            Reject::BadValue { id: 1, hint: "x" },
            Reject::CollapsedPins { id: 1 },
            Reject::ShortedSource { id: 1 },
            Reject::ConflictingSources {
                a: 1,
                b: 2,
                va: 5.0,
                vb: 1.0,
            },
            Reject::SourceLoop {
                ids: SmallIds::of(&[1, 2, 3]),
            },
            Reject::WillNotConverge { id: Some(4) },
        ] {
            assert!(!r.ids().is_empty(), "{r:?}");
            assert!(r.id().is_some(), "{r:?}");
            assert!(!r.hint().is_empty(), "{r:?}");
            assert!(!r.code().is_empty(), "{r:?}");
        }
        assert!(Reject::Unsolvable.ids().is_empty());
        assert!(Reject::UnsolvableWhenSwitched.ids().is_empty());
    }

    #[test]
    fn conflict_wording_never_says_five_volts_and_five_volts() {
        // Values that key apart can still render the same at 3 significant
        // figures. The message has to stay actionable.
        let close = 5.0 * (1.0 + 1e-9);
        let h = conflict_hint(5.0, close);
        assert!(!h.contains("5 V and 5 V"), "{h}");
        assert!(h.contains('e'), "must fall back to full precision: {h}");
        // Identical DC, different waveform: say so.
        let h = conflict_hint(5.0, 5.0);
        assert!(h.contains("waveform"), "{h}");
        // The ordinary case stays readable.
        assert_eq!(
            conflict_hint(9.0, 1.0),
            "9 V and 1 V wired to the same net - two supplies on one net have to \
             agree, or stay apart"
        );
        assert!(si_volts(0.0).starts_with("0 "));
        assert_eq!(si_volts(3.3), "3.3 V");
        assert_eq!(si_volts(-12.0), "-12 V");
        assert_eq!(si_volts(0.005), "5 mV");
    }

    #[test]
    fn the_ids_of_a_big_loop_are_truncated_but_the_count_is_not() {
        let ids = SmallIds::of(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(ids.len(), 6);
        assert_eq!(ids.iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }
}
