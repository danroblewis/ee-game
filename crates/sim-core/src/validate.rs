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
//! 4. **Convergence** — trial RUNS on the scratch engine. Factorability is
//!    not solvability: the shipped 9 V battery wired straight across the
//!    shipped LED factors perfectly and then freezes the room, because an
//!    ideal source pins the junction voltage somewhere its exponential can
//!    never meet. Newton burns all 100 iterations, the rescue ladder halves
//!    dt four times, and the engine quarantines — self-concealing, because
//!    the damage model is skipped while quarantined so the LED never even
//!    burns out. This is reachable with two catalog parts in the first
//!    minute of play, and it is far more likely than every structural
//!    failure combined.
//!
//!    A trial is run in each of up to four STATES of the document, because
//!    the freeze is not in general a t = 0 property (see [`trial_depth`] for
//!    how deep each one runs and what that costs):
//!
//!    * **as placed**, for `trial_depth` steps — the state the room is in;
//!    * **every switch closed**, for `trial_depth` steps — the same clone
//!      layer 3 factors, because a switch closed by a player OR BY THE
//!      MACHINE must not be able to reach a state nothing validated;
//!    * **every source at the top of its swing**, one step;
//!    * **every source at the bottom of its swing**, one step.
//!
//!    The last two are the cheap answer to time-varying drive. A sine across
//!    an LED does not fail at t = 0 — it fails when the waveform climbs past
//!    what the junction can hold, 107 steps later for a 9 V 50 Hz sine, and
//!    2 500 000 steps later for the 0.3 Hz sine the showcase ships. Chasing
//!    that in the time domain is hopeless at any affordable depth; pinning
//!    each source at `dc ± |amp|` reaches the same operating point in ONE
//!    step, at a cost independent of frequency. Measured over accepted-
//!    then-frozen fuzz documents, the two extreme states alone account for
//!    79% of the class — more than a 400-step trial (60%) and 400× cheaper.
//!
//!    Measured end to end over 12 000 fuzzed documents (this tree, release):
//!    of the documents this gate ACCEPTS, the share that then quarantines
//!    inside 5 000 steps falls from **0.67% to 0.07%**. The survivors are
//!    deep transients — median step 1474 — which no affordable trial reaches
//!    and which the quarantine machinery still handles honestly.
//!
//!    Every trial is run **one MNA block at a time** (see [`Blocks`]). The
//!    world's matrix is block diagonal — two circuits that share only a
//!    ground symbol share no unknown — and a dense LU is O(n³), so judging
//!    the room whole costs `(Σ nᵢ)³` where judging its circuits costs
//!    `Σ nᵢ³`. That is not a different answer, it is the same answer at 1/15
//!    the price on a 400-element room, and it is what pays for the four
//!    states. It is also what lets depth be BOUGHT per circuit against a
//!    budget ([`TRIAL_BUDGET`]) instead of read off a table of element
//!    counts: the old table switched layer 4 off entirely above 400 elements,
//!    so a 401-element room accepted an AC source across an LED that a
//!    400-element room refused, and then quarantined at step 107. Nothing
//!    switches off now.
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
use crate::engine::{Engine, Tuning};
use crate::netlist::{ElementKind, ElementSpec, Point};

/// The tuning every engine this module builds runs with: both work-skipping
/// levers off.
///
/// The gate is an observational experiment. Quiescence freezes an island
/// that has stopped moving and local dt integrates a slow one at `k·dt`;
/// both are correct for a live room and both are wrong here, because the
/// question the gate asks is "does this document's solver survive the next
/// few hundred substeps", and an island that skips them has not answered it.
#[inline]
fn sim_tuning_off() -> Tuning {
    Tuning::off()
}

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

/// A single MNA block wider than this is not trialled — the rest of the
/// document still is.
///
/// This is the last remaining cap, and it is deliberately shaped as a taper
/// rather than the cliff it replaces. The old cap keyed on ELEMENT COUNT and
/// switched the whole of layer 4 off above 400 elements, which had the worst
/// possible shape: it admitted exactly the band where cost peaked (measured
/// on this tree, before blocks: 19.2 ms at 300 elements, 42.9 ms at 400) and
/// then refused nothing at all above it — a 401-element room accepted an AC
/// source across an LED that a 400-element room correctly refused, and then
/// quarantined at step 107. Keying on the widest BLOCK instead means a big
/// room loses the trial only for the one enormous circuit inside it that is
/// genuinely unaffordable; every other circuit in the same room is still
/// judged, and adding parts elsewhere can never switch the trial off.
///
/// The most a document will ever spend on layer 4, in the step units
/// [`block_step_cost`] counts. One unit is about 0.6 us of a cold block step,
/// so this is ~2.6 ms — and it is a CEILING, not a budget: [`TRIAL_BUDGET`]
/// buys depth and can always be spent more thinly, but one step of every block
/// is the floor that depth cannot shrink, and this is what bounds it.
///
/// It is spent cheapest-first (see [`affordable_cost`]), so what a document
/// gives up when it cannot afford everything is its WIDEST circuits — the ones
/// a trial step costs the most on — while every ordinary circuit beside them
/// is still judged. That is the whole shape difference from the cap this
/// replaces, which keyed on the document's element count and switched layer 4
/// off for all of it at once, cheap circuits included.
///
/// Measured (release, this tree) on a diode ladder — one block, one step from
/// cold, which is what one trial state costs:
///
/// ```text
///   unknowns   elements   one step   all four states
///          4          5      1.3 us            5 us
///         18         26     13.2 us           53 us
///         50         74     92.4 us          370 us
///         98        146    378   us         1512 us
///        130        194    648   us         2592 us
///        194        290   1222   us         4890 us
///        386        578   7182   us        28729 us
/// ```
///
/// So one four-state circuit is affordable up to ~128 unknowns — about 190
/// parts drawn as a single connected mesh — and a DC one with no switch up to
/// twice that. Rooms do not reach it by growing: the shipped 147-element room's
/// widest block is 9 unknowns, and a 405-element room grown from it is still 9.
pub const TRIAL_CEILING: u64 = 4096;

/// Deepest any trial state ever runs. Beyond this the marginal catch rate is
/// not worth the constant: measured over the four documents the gate accepts
/// and the engine still freezes, going 256 -> 4096 steps catches ONE more and
/// costs 18x.
pub const MAX_TRIAL_DEPTH: u32 = 256;

/// The whole of layer 4, for one document, is held to this many step units —
/// where one unit is about 0.6 us of a cold block step (see
/// [`block_step_cost`]), so the budget is ~250 us of trial per gate call at
/// EVERY document size.
///
/// Calibrated against the corpus, not taste. Replayed over the same 20 000
/// fuzzed documents the four-state trial was measured on, 1024 units gives
/// back one document the trial used to catch (it needed 126 steps and got
/// 102); 2048 reproduces the four-state gate's verdict on all 20 000, document
/// for document, code for code. Measured end to end, on the shipped room grown
/// the way a room really grows — `check_document` in full, all four states,
/// every block (release, this tree):
///
/// ```text
///   elements     4    147    250    400    600    800   1200
///   before     1.5   1304   11443  42909   1372   1996   5583   us
///   after       46    485     625    906   1511   2372   5829   us
/// ```
///
/// Layer 4 is the flat part of that curve — it is this budget, and the budget
/// does not grow with the room (at 400 elements it is 483 us of the 906).
/// What grows is layers 1-3: the two whole-document factorizations are O(n³)
/// in the room's unknowns and nothing here caps them. That is the remaining
/// wall, it is older than this budget, and it is why the numbers converge
/// again past 800. The client's `GATE_MAX_ELEMENTS` is the guard in front of
/// it.
/// Calibrated at 2560, not 2048: an adversarial 20,000-document corpus found
/// one 8-unknown document that 2048 buys 204 trial steps for and which needs
/// 218. At 2560 the accepted set is IDENTICAL to the whole-document gate this
/// replaced — 0 newly accepted, 0 newly refused — for 1.02-1.16x the cost.
/// 4096 also works and buys nothing further. If this constant is ever lowered,
/// re-run that corpus: the failure mode is silent, a document that validates
/// and then quarantines a few hundred steps later.
pub const TRIAL_BUDGET: u64 = 2560;

/// What one step of one block costs, in units of "one step of a small block".
///
/// A step of a nonlinear block re-stamps and re-FACTORS once per Newton
/// iteration, so it is somewhere between quadratic and cubic in the block's
/// unknowns — the dense LU is O(u³), but the stamping and the iteration count
/// dominate until the matrix gets wide. `1 + u²/16` tracks the measured curve
/// in [`TRIAL_CEILING`] to within 1.6x from 4 unknowns to 128, which is
/// the only range this is ever asked about; wider blocks are not trialled at
/// all. Getting this shape wrong is what let the old element-count ladder
/// mis-price documents by 30x in both directions.
///
/// Integer arithmetic on an integer input, so the depth a document gets is
/// bit-identical on every target.
fn block_step_cost(unknowns: usize) -> u64 {
    let u = unknowns as u64;
    1 + u.saturating_mul(u) / 16
}

/// The largest per-block cost this document can afford one step of, given
/// [`TRIAL_CEILING`]. Blocks costing more are not trialled; blocks costing
/// this or less all are. `costs` is every candidate block's cost for one step
/// of every state it runs, and is left sorted.
///
/// Cheapest first, so the circuits a document gives up are its widest ones.
/// The cut is a VALUE, not a position, so blocks that cost the same are always
/// treated the same and the answer cannot depend on the order the document
/// happens to be in — the one property a "spend until the budget runs out"
/// loop would not have had.
fn affordable_cost(costs: &mut Vec<u64>) -> u64 {
    costs.sort_unstable();
    let (mut spent, mut cut, mut i) = (0u64, 0u64, 0usize);
    while i < costs.len() {
        let v = costs[i];
        let mut group = 0u64;
        while i < costs.len() && costs[i] == v {
            group = group.saturating_add(v);
            i += 1;
        }
        match spent.checked_add(group) {
            Some(t) if t <= TRIAL_CEILING => (spent, cut) = (t, v),
            _ => break,
        }
    }
    cut
}

/// How many steps one block's trial states run, given that block's cost for
/// one step of every state it will run (`states x block_step_cost`) and how
/// many trialled blocks the document has to share the budget between.
///
/// Depth is bought, not assumed. The old gate ran exactly ONE step and
/// justified it by calling the freeze "a DC operating-point failure, visible
/// on the very first step". That premise is measurably false: over fuzzed
/// documents this gate ACCEPTED and the engine then quarantined, **0.00%
/// died at step 0** — median step 161, p90 step 2601. A single step catches
/// essentially none of the class on its own.
///
/// Depth is not free either, so it is spent against a budget instead of read
/// off a table of element counts. Each trialled block gets an equal SHARE of
/// [`TRIAL_BUDGET`] and buys as many steps as its share affords. That has
/// four properties the element-count table did not:
///
/// * it never reaches zero, so there is no size at which the gate stops
///   seeing the non-convergence class;
/// * it prices a block by its width, which is what actually costs — a
///   400-element room of small circuits is cheaper to trial deeply than a
///   21-element one drawn as a single dense mesh, and the old table had that
///   exactly backwards;
/// * it is per BLOCK, so one big circuit already in the room cannot blind the
///   gate to the three-part one a player just drew next to it;
/// * it degrades smoothly, so no edit can move a document across a cliff.
///
/// Order-independent by construction: `share` depends on how many blocks the
/// document has, never on which one is being priced, so shuffling the
/// document cannot change a single depth.
fn trial_depth(share: u64, block_cost: u64) -> u32 {
    (share / block_cost.max(1)).clamp(1, MAX_TRIAL_DEPTH as u64) as u32
}

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

    /// Public so `Reject::SourceLoop` is constructible outside this module —
    /// every other variant's fields are plain, and one that could only be
    /// built here would make the enum awkward to match against in tests.
    pub fn of(all: &[u32]) -> Self {
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
    // `seen` doubles as the BFS queue: nodes are appended in visit order and
    // read back with a head index, so one vector serves both. `came[i]` is
    // the (previous node, edge id) that reached `seen[i]`.
    let mut seen: Vec<usize> = vec![from];
    let mut came: Vec<Option<(usize, u32)>> = vec![None];
    let mut head = 0;
    while head < seen.len() {
        let cur = seen[head];
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
        }
    }
    // Walk back from `to`. Each hop lands on a node discovered strictly
    // earlier, so this terminates; a `to` that was never reached (impossible
    // once union-find says the endpoints are connected) simply yields the
    // closing edge alone.
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
/// not converge: a diode/LED/zener whose two nodes are joined by a PATH of
/// ideal constraints. That is not a guess about the numerics — a chain of
/// zero-impedance sources pins the junction voltage to the sum along the
/// path, a value its exponential can never satisfy, which is precisely the
/// shipped-battery-across-the-shipped-LED case.
///
/// A path rather than a single constraint, because the interesting version
/// is not the one drawn as a single part: 9 V into a closed switch into an
/// LED is a source, a 0 V constraint and a junction, and naming the LED
/// there is the difference between a part the client can flash and an
/// anonymous refusal. Union-find in document order, integer comparisons
/// only, first offender wins — deterministic on every target.
fn blame_for_divergence(eng: &Engine, specs: &[ElementSpec]) -> Option<u32> {
    let cs = eng.ideal_constraints();
    if cs.is_empty() {
        return None;
    }
    let mut parent: Vec<usize> = (0..eng.node_count() + 1).collect();
    for (_, c) in &cs {
        let (ra, rb) = (find(&mut parent, c.a), find(&mut parent, c.b));
        parent[ra] = rb;
    }
    let nodes = eng.element_nodes();
    for s in specs {
        if !matches!(
            s.kind,
            ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. }
        ) {
            continue;
        }
        // A malformed element is dropped by `set_elements`, so it may not be
        // in the compiled list at all: skip it, do not abandon the search.
        let Some((_, _, n)) = nodes.iter().find(|(id, _, _)| *id == s.id) else {
            continue;
        };
        if n[0] != n[1] && find(&mut parent, n[0]) == find(&mut parent, n[1]) {
            return Some(s.id);
        }
    }
    None
}

/// One trial run: compile `specs` on a scratch engine and step it. `Some`
/// when the engine gave up — that is the room freezing, in advance.
///
/// A linear document takes the solver's single-pass path and cannot fail to
/// converge, so the common big-document case (resistor ladders, wire nets)
/// pays only the compile. `advance` stops at the first failed step, so the
/// depth is paid by documents that are FINE, never by the ones being caught.
fn trial(specs: &[ElementSpec], dt: f64, steps: u32) -> Option<Reject> {
    let mut eng = Engine::new(dt);
    // Levers off: see `check_document`. A trial that skips the island it is
    // trialling proves nothing about it.
    eng.set_tuning(sim_tuning_off());
    eng.set_elements(specs);
    if eng.is_linear() {
        return None;
    }
    if eng.advance(steps).quarantined {
        return Some(Reject::WillNotConverge {
            id: blame_for_divergence(&eng, specs),
        });
    }
    None
}

// ------------------------------------------------------------- MNA blocks

/// The document cut into the independent blocks of its MNA matrix, so a
/// trial solves each one on its own.
///
/// **Why this is exact, not an approximation.** There is exactly one matrix
/// for the whole world, but it is BLOCK DIAGONAL. Every stamp an element
/// makes lands on rows and columns belonging to its own pins' nodes (and its
/// own branch row); nothing in `Engine` couples two elements that share no
/// node. Node 0 is not an unknown — it has no row and no column — so two
/// circuits whose only common node is ground are two blocks, not one. And
/// two ideal constraints share a branch row only when their canonical keys
/// match, which needs the same node pair; the only pair two blocks can share
/// is `(0, 0)`, and every `(0, 0)` constraint is a `ShortedSource` refused by
/// layer 2 before layer 4 ever runs.
///
/// Newton on a block-diagonal system is Newton on each block: the linear
/// solve inside every iteration decouples, the guess update is per device, and
/// the rescue ladder only ever subjects a block to a SMALLER step than it
/// would have taken alone. So a block that quarantines alone quarantines in
/// the room, and one that survives alone survives — the verdict is the same,
/// which is the whole licence for doing this.
///
/// **Why it is worth doing.** A dense LU is O(n³) and Newton pays it once per
/// iteration, so trialling the world whole costs `(Σ nᵢ)³` where trialling the
/// blocks costs `Σ nᵢ³`. Rooms are not one circuit; they are dozens of small
/// ones sharing a ground symbol. The shipped 147-element room is 61 unknowns
/// in 14 blocks whose biggest is 7 nodes, and a 400-element room grown from it
/// is 214 unknowns in 65 blocks — still 9 unknowns at the widest. Measured on
/// this tree, that is what takes the whole gate at 400 elements from 42.9 ms
/// to 0.91 ms, and what lets depth stop being guessed from an element count
/// (see [`TRIAL_BUDGET`]).
struct Blocks {
    /// Indices into the document, per block, in document order.
    members: Vec<Vec<usize>>,
    /// Points the FULL document holds at node 0 which this block would
    /// otherwise float: it is grounded through `Ground` parts and wires that
    /// live outside the block, and the sub-document has to say so itself.
    grounds: Vec<Vec<Point>>,
    /// Unknowns the full matrix devotes to this block — nodes plus branch
    /// rows. The cost model the depth ladder is written against.
    unknowns: Vec<usize>,
    /// Does this block contain an OPEN switch or button? If not, the
    /// all-closed state is the same sub-document as the as-placed one.
    opens: Vec<bool>,
    /// Does this block contain a source with a non-zero amplitude? If not, the
    /// two peak states are the same sub-document as the all-closed one.
    swings: Vec<bool>,
    /// Anything nonlinear? A linear block cannot fail to converge — the solver
    /// takes its single-pass path — so it is not worth compiling to find out.
    nonlinear: Vec<bool>,
}

/// Cut a COMPILED document into blocks. `None` when the compiled element list
/// does not line up with `specs` one-to-one (a malformed element was dropped),
/// in which case the caller trials the document whole rather than guessing.
///
/// Switch state deliberately does not enter: an element joins its pins'
/// nodes whether or not it stamps, so an open switch is treated as coupling
/// its two sides. That is the conservative direction (bigger blocks, same
/// verdict) and it buys one partition that is valid for all four trial
/// states, since neither closing a switch nor pinning a source at its peak
/// moves a pin.
fn split_blocks(eng: &Engine, specs: &[ElementSpec]) -> Option<Blocks> {
    let nodes = eng.element_nodes();
    if nodes.len() != specs.len() {
        return None;
    }
    let mut parent: Vec<usize> = (0..specs.len()).collect();
    // First element seen at each node; usize::MAX = none yet.
    let mut first: Vec<usize> = vec![usize::MAX; eng.node_count() + 1];
    for (i, s) in specs.iter().enumerate() {
        for &nd in &nodes[i].2[..s.pins.len()] {
            if nd == 0 {
                continue; // ground couples nothing: it has no row
            }
            if first[nd] == usize::MAX {
                first[nd] = i;
            } else {
                let (a, b) = (find(&mut parent, first[nd]), find(&mut parent, i));
                parent[a] = b;
            }
        }
    }

    let mut block_of_root: Vec<usize> = vec![usize::MAX; specs.len()];
    let mut b = Blocks {
        members: Vec::new(),
        grounds: Vec::new(),
        unknowns: Vec::new(),
        opens: Vec::new(),
        swings: Vec::new(),
        nonlinear: Vec::new(),
    };
    // Distinct non-ground nodes per block, counted as they are first seen.
    let mut node_seen: Vec<usize> = vec![usize::MAX; eng.node_count() + 1];
    for (i, s) in specs.iter().enumerate() {
        let ns = &nodes[i].2[..s.pins.len()];
        if ns.iter().all(|n| *n == 0) {
            // Every pin on ground: no row, no column, nothing to solve. These
            // are the `Ground` parts and the wires along a ground rail — the
            // blocks that need them re-ground themselves below.
            continue;
        }
        let r = find(&mut parent, i);
        let k = if block_of_root[r] == usize::MAX {
            block_of_root[r] = b.members.len();
            b.members.push(Vec::new());
            b.grounds.push(Vec::new());
            // Every branch-carrying part is one unknown, except the ones that
            // alias onto an earlier constraint's row — which `share_n` would
            // tell us, but which only ever SHRINKS the count, and this is a
            // cost model, not a contract.
            b.unknowns.push(0);
            b.opens.push(false);
            b.swings.push(false);
            b.nonlinear.push(false);
            b.members.len() - 1
        } else {
            block_of_root[r]
        };
        b.members[k].push(i);
        if s.kind.is_branch() {
            b.unknowns[k] += 1;
        }
        match s.kind {
            ElementKind::Switch { closed: false } | ElementKind::Button { closed: false } => {
                b.opens[k] = true
            }
            ElementKind::VoltageSource { amp, .. } | ElementKind::Rail { amp, .. }
                if amp != 0.0 =>
            {
                b.swings[k] = true
            }
            _ => {}
        }
        b.nonlinear[k] |= s.kind.is_nonlinear();
        for (p, &nd) in s.pins.iter().zip(ns.iter()) {
            if nd == 0 {
                if !b.grounds[k].contains(p) {
                    b.grounds[k].push(*p);
                }
            } else if node_seen[nd] == usize::MAX {
                node_seen[nd] = k;
                b.unknowns[k] += 1;
            }
        }
    }
    Some(b)
}

/// Ids for the `Ground` parts a sub-document has to invent. Counting down
/// from the top of the range keeps them clear of anything a room hands out
/// (ids are allocated upward from 1, and the hoist fixture reserves 900-999).
/// They are never stored, broadcast or blamed: `blame_for_divergence` only
/// ever names a diode, an LED or a zener.
const SYNTHETIC_GROUND_ID: u32 = u32::MAX;

/// Block `k` of `specs` as a document in its own right. `specs` must be the
/// document [`split_blocks`] was run on, or a clone of it with the same
/// elements in the same order and the same pins — every trial state qualifies.
fn block_doc(specs: &[ElementSpec], b: &Blocks, k: usize, out: &mut Vec<ElementSpec>) {
    out.clear();
    out.extend(b.members[k].iter().map(|i| specs[*i].clone()));
    for (j, p) in b.grounds[k].iter().enumerate() {
        out.push(ElementSpec::ground(SYNTHETIC_GROUND_ID - j as u32, *p));
    }
}

/// Does this part drive a waveform, rather than a constant? The two peak
/// states are the same document as the all-closed one without one of these,
/// which is how a block skips them.
fn swings(kind: &ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::VoltageSource { amp, .. } | ElementKind::Rail { amp, .. } if *amp != 0.0
    )
}

/// Freeze every source in place at one end of its swing: `dc + sign·|amp|`,
/// no frequency, no phase.
///
/// This is the whole answer to time-varying drive, and it is worth being
/// precise about why it is legitimate rather than a heuristic. The freeze
/// this layer exists to catch is an ideal constraint pinning a junction
/// somewhere its exponential cannot reach. Whether that happens depends on
/// the constraint's VALUE, not on how it got there — and the set of values a
/// `dc + amp·sin(...)` source takes is exactly `[dc − |amp|, dc + |amp|]`,
/// both ends of which it really does visit, every cycle. Testing the two
/// ends is testing the operating points the circuit will actually be in.
///
/// One-sided conservatism, named rather than hidden: sources with different
/// phases are pinned at their extremes SIMULTANEOUSLY here, and two sources
/// in one loop that never peak together would be judged on a sum they never
/// really see. Every shipped and fixtured circuit is checked against that in
/// the tests; a room that hit it would be refused a placement that was in
/// fact safe, which is the safe direction to be wrong in.
///
/// `dc + |amp|` can exceed the [`MAX_SOURCE_VOLTS`] the value layer enforces
/// (1 MV + 1 MV). That is fine and deliberate: this document is a scratch
/// operating point, it is never stored, broadcast or shown, and 2e6 is nine
/// orders of magnitude inside f64.
fn pin_at_peak(specs: &mut [ElementSpec], sign: f64) {
    for s in specs.iter_mut() {
        if let ElementKind::VoltageSource { dc, amp, hz, phase }
        | ElementKind::Rail { dc, amp, hz, phase } = &mut s.kind
        {
            if *amp != 0.0 {
                *dc += sign * amp.abs();
                *amp = 0.0;
                *hz = 0.0;
                *phase = 0.0;
            }
        }
    }
}

// ---------------------------------------------------------------- the gate

/// Would the engine accept this document? `Ok(())` = every parameter is in
/// range, no ideal constraint is shorted, conflicting or looped, the MNA
/// matrix factors both as placed and with every switch closed, and Newton
/// finds an operating point as placed, with every switch closed, and at both
/// ends of every source's swing. Pure and deterministic. `dt` should be the
/// timestep the live engine runs at (companion conductances depend on it;
/// structural singularity does not).
///
/// Two properties worth stating because they are what make the answer mean
/// anything:
///
/// * the trial engine compiles the SAME `specs` slice the live engine will,
///   so it interns the same junctions, numbers the same nodes and factors
///   the same permutation of the same matrix. The numeric layers are
///   therefore a prediction of what this document does in the room, not of
///   what some equivalent document would do;
/// * the trial starts COLD (t = 0, every device at rest) while a live edit
///   keeps continuous state by id. A document that only fails from an
///   initial transient it has already lived through is refused — the safe
///   direction, and the only one available to a pure function.
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
    //
    // The gate is an OBSERVATIONAL experiment, so it runs with the
    // work-skipping levers off (`Tuning::off`). A trial that lets an island
    // sleep, or integrates it at `k·dt`, is measuring its own skipping
    // instead of the world — and the divergence this gate exists to catch is
    // exactly the kind of event a sleeping island would never reach.
    let mut eng = Engine::new(dt);
    eng.set_tuning(sim_tuning_off());
    eng.set_elements(specs);
    diagnose(&eng)?;
    if !eng.probe_solvable() {
        return Err(Reject::Unsolvable);
    }
    // The matrix layer 4 will solve, cut into its independent blocks. Taken
    // HERE, off the as-placed compile, because node numbering depends only on
    // geometry, wires and grounds — not on switch positions or source values —
    // so this one partition serves every trial state below.
    let blocks = split_blocks(&eng, specs);

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
        // `diagnose` first purely as a shortcut: everything it names is also
        // singular, so it is O(E + G²) standing in front of an O(n³)
        // factorization on the reject path. The VERDICT is the same either
        // way, which is why both arms return the one generic code.
        if diagnose(&eng).is_err() || !eng.probe_solvable() {
            return Err(Reject::UnsolvableWhenSwitched);
        }
    }

    // Convergence, in up to four states of the document — and in each state,
    // one MNA block at a time (see [`Blocks`]).
    //
    // The four states, in the order a refusal claims them:
    //
    // 1. **As placed** — the operating point the room is in right now.
    // 2. **Every switch closed** — the SAME clone layer 3 factors, held to
    //    the same policy. This is not symmetry for its own sake: the hoist's
    //    limit switches are closed by the MACHINE through `write_param`, on
    //    its own schedule, with no gate anywhere in front of them, and
    //    `write_param` deliberately never clears quarantine. Without this
    //    trial a player could wire an LED in series with LIM-TOP, be told the
    //    placement was fine, and have the crate itself freeze the room on the
    //    way up — the one failure the game inflicts on a document it blessed.
    //
    //    Singularity is monotone in the closed set (layer 3's argument);
    //    convergence is not, so this is a two-point sample of a space with
    //    2^n corners, not a proof. It covers the direction that matters:
    //    closing switches only ever ADDS ideal constraints and merges nodes,
    //    which is what puts a source across a junction.
    // 3-4. **Both ends of every source's swing**, one step each. A DC
    //    document's operating-point failure IS visible on the first step —
    //    that claim was only ever true here, of a document with nothing left
    //    moving in it. Built from the all-closed clone so the two
    //    conservatisms compose instead of needing four more trials.
    //
    // A block only runs a state that is a DIFFERENT sub-document for it. A
    // circuit with no open switch has the same all-closed state it is already
    // in; one with no swinging source has the same peaks. Trialling those
    // again is not extra safety, it is the same computation three more times —
    // and in a room of many small circuits, it is most of the bill.
    // `None` = no usable partition, which means a malformed element was
    // dropped by `set_elements` — layer 1 refuses those, so this is a
    // belt-and-braces path: judge the document whole, as one block.
    let whole = Blocks {
        members: vec![(0..specs.len()).collect()],
        grounds: vec![Vec::new()],
        unknowns: vec![eng.unknowns()],
        opens: vec![any_open],
        swings: vec![specs.iter().any(|s| swings(&s.kind))],
        nonlinear: vec![!eng.is_linear()],
    };
    let b = blocks.as_ref().unwrap_or(&whole);

    // What layer 4 costs has to be PRICED before it is spent, in two parts.
    // Integer arithmetic on the document alone, so a document gets the same
    // trial on native and on wasm32.
    //
    // First the floor: one step of block k in every state it runs, which no
    // depth decision can reduce. `TRIAL_CEILING` is what the document may
    // spend on that, cheapest circuits first.
    let floor = |k: usize| {
        let states = 1 + u64::from(b.opens[k]) + 2 * u64::from(b.swings[k]);
        states.saturating_mul(block_step_cost(b.unknowns[k]))
    };
    let mut costs: Vec<u64> = (0..b.members.len())
        .filter(|k| b.nonlinear[*k])
        .map(&floor)
        .collect();
    let cut = affordable_cost(&mut costs);
    let trialled = |k: usize| b.nonlinear[k] && floor(k) <= cut;
    // Then the depth: `TRIAL_BUDGET`, split evenly between the blocks that
    // survived the ceiling.
    let share = TRIAL_BUDGET / (0..b.members.len()).filter(|k| trialled(*k)).count().max(1) as u64;
    let depth = |k: usize| {
        // Only the as-placed and all-closed states are `depth` steps deep; the
        // two peak states are one step each however deep the others run, so
        // they are not what depth is being traded against.
        let deep_states = 1 + u64::from(b.opens[k]);
        trial_depth(
            share,
            deep_states.saturating_mul(block_step_cost(b.unknowns[k])),
        )
    };

    // States 3 and 4 are one step by construction: pinning every source at
    // `dc ± |amp|` reaches the operating point it will visit immediately, so
    // depth buys nothing there. States outermost, so a refusal still names the
    // earliest state that fails — the one the player is closest to.
    let mut sub: Vec<ElementSpec> = Vec::new();
    for which in [State::AsPlaced, State::Closed, State::PeakHi, State::PeakLo] {
        if matches!(which, State::Closed) && !any_open {
            continue;
        }
        for k in 0..b.members.len() {
            let runs = match which {
                State::AsPlaced => true,
                State::Closed => b.opens[k],
                State::PeakHi | State::PeakLo => b.swings[k],
            };
            if !runs || !trialled(k) {
                continue;
            }
            let (source, steps) = match which {
                State::AsPlaced => (specs, depth(k)),
                State::Closed => (&closed[..], depth(k)),
                _ => (&closed[..], 1),
            };
            block_doc(source, b, k, &mut sub);
            match which {
                State::PeakHi => pin_at_peak(&mut sub, 1.0),
                State::PeakLo => pin_at_peak(&mut sub, -1.0),
                _ => {}
            }
            if let Some(r) = trial(&sub, dt, steps) {
                return Err(r);
            }
        }
    }
    Ok(())
}

/// Which of the four trial states a sweep is running, so a block can skip the
/// ones that are the same sub-document it has already been judged in.
#[derive(Clone, Copy)]
enum State {
    AsPlaced,
    Closed,
    PeakHi,
    PeakLo,
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

    /// REPRO A: three catalogue parts, a beginner's first minute — a 9 V
    /// 50 Hz sine straight across the shipped LED. The gate used to ACCEPT
    /// this and the engine quarantined 107 steps (2.14 ms) later, before the
    /// damage model ever ran, so the LED did not even burn out: the room
    /// simply stopped, self-concealing, until somebody deleted the part.
    ///
    /// The DC twin was always refused. The gate was blind SPECIFICALLY to
    /// time-varying drive, because it ran one step and called the class a
    /// t = 0 property.
    #[test]
    fn an_ac_source_straight_across_a_junction_is_refused() {
        let repro = |kind: ElementKind| {
            vec![
                ElementSpec::two(1, ac(0.0, 9.0, 50.0, 0.0), (0, 0), (0, 6)),
                ElementSpec::two(2, kind, (0, 0), (0, 6)),
                ElementSpec::ground(3, (0, 6)),
            ]
        };
        for (kind, why) in [
            (ElementKind::Led { color: 0 }, "LED"),
            (ElementKind::Diode, "diode"),
            (ElementKind::Zener { vz: 5.1 }, "zener"),
        ] {
            let d = repro(kind);
            let got = reject_of(&d);
            assert!(
                matches!(got, Reject::WillNotConverge { id: Some(2) }),
                "{why}: {got:?}"
            );
        }

        // The freeze this prevents is real, and it is NOT at t = 0. Committed
        // to a live engine, the LED version runs 107 clean steps and then
        // quarantines for good — which is exactly why one trial step could
        // never have seen it.
        let d = repro(ElementKind::Led { color: 0 });
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        let mut good = 0;
        while good < 5000 && !e.advance(1).quarantined {
            good += 1;
        }
        assert_eq!(good, 107, "the measured failure step");
        assert!(e.is_quarantined());

        // And the one-step trial the old gate ran does not see it at any
        // frequency the catalogue offers — the point of the extreme states.
        for hz in [0.3, 1.0, 50.0, 60.0, 440.0] {
            let d = vec![
                ElementSpec::two(1, ac(0.0, 9.0, hz, 0.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Led { color: 0 }, (0, 0), (0, 6)),
                ElementSpec::ground(3, (0, 6)),
            ];
            let mut one = Engine::new(DT);
            one.set_elements(&d);
            assert!(
                !one.advance(1).quarantined,
                "{hz} Hz: one step must NOT be what catches this"
            );
            assert!(check_document(&d, DT).is_err(), "{hz} Hz must be refused");
        }
    }

    /// The extreme-state trials must judge circuits by operating points they
    /// really reach, and nothing else. Every one of these is a circuit a
    /// player is supposed to be able to build.
    #[test]
    fn time_varying_drive_that_is_actually_fine_stays_placeable() {
        // The same sine WITH a series resistor: the whole point of the hint.
        for ohms in [1.0, 330.0] {
            ok(&[
                ElementSpec::two(1, ac(0.0, 9.0, 50.0, 0.0), (0, 0), (0, 6)),
                ElementSpec::two(2, r(ohms), (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 6)),
                ElementSpec::ground(4, (0, 6)),
            ]);
        }
        // A sine offset so it never crosses zero, and one that swings far
        // negative into a diode's reverse region.
        for (dc_v, amp) in [(6.0, 1.0), (0.0, 24.0), (-5.0, 12.0)] {
            ok(&[
                ElementSpec::two(1, ac(dc_v, amp, 50.0, 0.7), (0, 0), (0, 6)),
                ElementSpec::two(2, r(1000.0), (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Diode, (4, 0), (0, 6)),
                ElementSpec::ground(4, (0, 6)),
            ]);
        }
        // The showcase's shape: a sub-Hz gate driver on an NMOS, where the
        // period is 166 667 steps and no affordable time trial could ever
        // reach the peak.
        ok(&[
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 12)),
            ElementSpec::two(
                2,
                ElementKind::Lamp {
                    ohms: 60.0,
                    rated_watts: 1.2,
                },
                (0, 0),
                (4, 0),
            ),
            ElementSpec::three(
                3,
                ElementKind::Nmos { vt: 1.5, k: 0.05 },
                (8, 6),
                (4, 0),
                (4, 12),
            ),
            ElementSpec::two(4, ac(3.0, 3.0, 0.3, 0.0), (8, 6), (0, 12)),
            ElementSpec::ground(5, (0, 12)),
        ]);
        // Two sines of different phase and frequency in one loop: the case
        // the simultaneous-peak conservatism could in principle refuse.
        ok(&[
            ElementSpec::two(1, ac(0.0, 6.0, 50.0, 0.0), (0, 0), (4, 0)),
            ElementSpec::two(2, ac(0.0, 6.0, 60.0, 1.5), (4, 0), (8, 0)),
            ElementSpec::two(3, r(470.0), (8, 0), (8, 6)),
            ElementSpec::two(4, ElementKind::Led { color: 0 }, (8, 6), (0, 6)),
            ElementSpec::two(5, ElementKind::Wire, (0, 6), (0, 0)),
            ElementSpec::ground(6, (0, 6)),
        ]);
    }

    /// F2: the one failure the GAME inflicts on a document the gate blessed.
    ///
    /// `machine_step` writes LIM-TOP/LIM-BOT straight into the live engine
    /// every 640 µs with no gate in front of it, and `write_param` — quite
    /// correctly — never clears quarantine. So a circuit that only diverges
    /// once a limit switch closes is a room-wide freeze that the crate
    /// itself triggers, that no player action can undo, and that the gate
    /// used to sign off on because it only ever trialled the state the
    /// document was IN.
    #[test]
    fn a_switch_closing_into_a_junction_is_refused_before_it_can_close() {
        // 9 V -> switch -> LED -> ground. Open: the LED dangles. Closed:
        // an ideal source straight across it.
        let repro = |source: ElementKind, closed: bool| {
            vec![
                ElementSpec::two(1, source, (0, 0), (0, 6)),
                ElementSpec::two(902, ElementKind::Switch { closed }, (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 6)),
                ElementSpec::ground(4, (0, 6)),
            ]
        };
        for source in [dc(9.0), ac(0.0, 9.0, 50.0, 0.0)] {
            let d = repro(source, false);
            // It factors as placed and with the switch closed: layer 3 has
            // nothing to say about it, which is why layer 4 has to.
            let mut e = Engine::new(DT);
            e.set_elements(&d);
            assert!(e.probe_solvable(), "structurally fine as placed");
            let got = reject_of(&d);
            assert!(
                matches!(got, Reject::WillNotConverge { id: Some(3) }),
                "the LED must be named: {got:?}"
            );
            // Closed as placed, it is refused too — same circuit, no gap.
            assert!(check_document(&repro(source, true), DT).is_err());
        }

        // What was actually happening: the machine closes the switch and the
        // room never comes back, not even when the crate leaves the limit.
        let d = repro(dc(9.0), false);
        let mut e = Engine::new(DT);
        e.set_elements(&d);
        e.advance(50);
        assert!(!e.is_quarantined(), "healthy while the switch is open");
        e.write_param(902, crate::netlist::ParamWrite::Switch { closed: true });
        e.advance(1);
        assert!(e.is_quarantined(), "the machine froze the room");
        e.write_param(902, crate::netlist::ParamWrite::Switch { closed: false });
        e.advance(100);
        assert!(e.is_quarantined(), "and it is self-locking: no way back");

        // A switch closing onto something that is FINE closed stays legal —
        // the fix must not make the hoist's own drive unplaceable.
        for closed in [false, true] {
            ok(&[
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(902, ElementKind::Switch { closed }, (0, 0), (4, 0)),
                ElementSpec::two(3, r(330.0), (4, 0), (8, 0)),
                ElementSpec::two(4, ElementKind::Led { color: 0 }, (8, 0), (0, 6)),
                ElementSpec::ground(5, (0, 6)),
            ]);
        }
    }

    /// F4: the depth budget. Monotone, bounded, and — the whole point of
    /// replacing the element-count table — never zero.
    #[test]
    fn the_trial_depth_budget_is_bounded_and_never_switches_off() {
        let mut last = u32::MAX;
        for cost in 0..=100_000u64 {
            let d = trial_depth(TRIAL_BUDGET, cost);
            assert!(d <= last, "depth must not grow at cost {cost}");
            assert!(
                (1..=MAX_TRIAL_DEPTH).contains(&d),
                "depth {d} out of range at cost {cost}"
            );
            last = d;
        }
        assert_eq!(
            trial_depth(TRIAL_BUDGET, 1),
            MAX_TRIAL_DEPTH,
            "a beginner's circuit gets the deep run"
        );
        assert_eq!(
            trial_depth(TRIAL_BUDGET, u64::MAX),
            1,
            "no block is ever priced out of the trial entirely"
        );
        assert_eq!(
            trial_depth(0, 1),
            1,
            "not even a room of thousands of circuits: every one still gets a step"
        );
        // The cost model has to be monotone too, or a bigger block could buy
        // a deeper trial than a smaller one.
        let mut last = 0;
        for u in 0..=512 {
            let c = block_step_cost(u);
            assert!(c >= last, "block cost must not shrink at {u} unknowns");
            last = c;
        }
        assert_eq!(block_step_cost(usize::MAX), u64::MAX / 16 + 1, "saturates");
    }

    /// Depth follows the width of a circuit, not the size of the room. The
    /// element-count table this replaced had that backwards, which is how a
    /// 160-element room of four-part circuits came to get a 2-step trial.
    #[test]
    fn depth_follows_the_widest_block_not_the_element_count() {
        // Same share, different widths: wider buys fewer steps. This is the
        // whole content of the cost model.
        let share = TRIAL_BUDGET;
        let narrow = trial_depth(share, block_step_cost(4));
        let wide = trial_depth(share, block_step_cost(64));
        assert!(
            narrow > wide,
            "a 4-unknown block ({narrow}) must out-run a 64-unknown one ({wide})"
        );
        assert_eq!(narrow, MAX_TRIAL_DEPTH, "small circuits still run deep");

        // 40 independent 4-element circuits: 40 blocks, and every one of them
        // still gets a real trial. The old table gave this document 2 steps.
        let mut room: Vec<ElementSpec> = Vec::new();
        for k in 0..40i32 {
            let (x, id) = (k * 4, 1 + 10 * k as u32);
            room.push(ElementSpec::two(id, dc(9.0), (x, 0), (x, 6)));
            room.push(ElementSpec::two(id + 1, r(330.0), (x, 0), (x + 1, 0)));
            room.push(ElementSpec::two(
                id + 2,
                ElementKind::Led { color: 0 },
                (x + 1, 0),
                (x, 6),
            ));
            room.push(ElementSpec::ground(id + 3, (x, 6)));
        }
        assert_eq!(check_document(&room, DT), Ok(()));
        let (depth, blocks) = depth_for(&room);
        assert_eq!(blocks, 40, "one block per circuit");
        assert!(
            depth >= 16,
            "160 elements in small blocks must still run deep, got {depth}"
        );
    }

    /// What a document gives up when it cannot afford to judge everything is
    /// its widest circuits — never its cheap ones, and never because of how
    /// many parts are on the canvas.
    #[test]
    fn the_ceiling_gives_up_the_widest_circuits_first() {
        // A room the ceiling comfortably covers: nothing is given up.
        let mut cheap = vec![1u64; 200];
        assert_eq!(affordable_cost(&mut cheap), 1);
        // One expensive circuit among cheap ones: the cheap ones survive.
        let mut mixed = vec![1, 1, 1, 1, TRIAL_CEILING * 4];
        assert_eq!(affordable_cost(&mut mixed), 1, "the wide one is given up");
        // Equal-cost circuits are all treated the same, however many there
        // are — so the answer cannot depend on document order.
        let big = TRIAL_CEILING / 2 + 1;
        assert_eq!(affordable_cost(&mut vec![big]), big, "one fits");
        assert_eq!(
            affordable_cost(&mut vec![big, big]),
            0,
            "two do not, so neither runs"
        );
        assert_eq!(
            affordable_cost(&mut vec![1, big, big]),
            1,
            "and the cheap circuit beside them still runs"
        );
        // Reordering the same multiset cannot change the cut.
        let m = [7u64, 3, 900, 3, 4096, 1];
        let mut a: Vec<u64> = m.to_vec();
        let mut b: Vec<u64> = m.iter().rev().copied().collect();
        assert_eq!(affordable_cost(&mut a), affordable_cost(&mut b));
        // Nothing to trial: no cut, and no panic on the empty case.
        assert_eq!(affordable_cost(&mut Vec::new()), 0);
        // Saturating: a block whose cost overflows the accumulator is simply
        // unaffordable, not a wrapped-around bargain.
        assert_eq!(affordable_cost(&mut vec![u64::MAX, 1]), 1);
    }

    /// The measured regression the old element-count cap left behind: an AC
    /// source straight across an LED was refused in a 400-element room and
    /// ACCEPTED in a 401-element one, where it then quarantined at step 107.
    /// Room size must not decide whether the gate can see a three-part fault.
    #[test]
    fn a_bad_circuit_stays_refused_however_big_the_room_around_it_is() {
        // A healthy room, grown one four-part circuit at a time.
        let cell = |k: i32| {
            let (x, id) = (k * 4, 1000 + 10 * k as u32);
            vec![
                ElementSpec::two(id, dc(9.0), (x, 0), (x, 6)),
                ElementSpec::two(id + 1, r(330.0), (x, 0), (x + 1, 0)),
                ElementSpec::two(id + 2, ElementKind::Led { color: 0 }, (x + 1, 0), (x, 6)),
                ElementSpec::ground(id + 3, (x, 6)),
            ]
        };
        // The fault: 9 V peak straight across a junction, far from everything.
        let fault = vec![
            ElementSpec::two(1, ac(0.0, 9.0, 50.0, 0.0), (-100, 0), (-100, 6)),
            ElementSpec::two(2, ElementKind::Led { color: 0 }, (-100, 0), (-100, 6)),
            ElementSpec::ground(3, (-100, 6)),
        ];
        for cells in [0usize, 25, 50, 100, 200, 400] {
            let mut room: Vec<ElementSpec> = Vec::new();
            for k in 0..cells {
                room.extend(cell(k as i32));
            }
            assert_eq!(
                check_document(&room, DT),
                Ok(()),
                "{} healthy elements must stay placeable",
                room.len()
            );
            let mut bad = room.clone();
            bad.extend(fault.iter().cloned());
            let got = check_document(&bad, DT);
            assert!(
                matches!(got, Err(Reject::WillNotConverge { .. })),
                "the same three parts must be refused in a {}-element room, got {got:?}",
                bad.len()
            );
        }
    }

    /// The deepest trial `check_document` would run on `specs`, and how many
    /// blocks it cut the document into.
    fn depth_for(specs: &[ElementSpec]) -> (u32, usize) {
        let mut eng = Engine::new(DT);
        eng.set_elements(specs);
        let b = split_blocks(&eng, specs).expect("well-formed document");
        let floor = |k: usize| {
            (1 + u64::from(b.opens[k]) + 2 * u64::from(b.swings[k]))
                .saturating_mul(block_step_cost(b.unknowns[k]))
        };
        let mut costs: Vec<u64> = (0..b.members.len())
            .filter(|k| b.nonlinear[*k])
            .map(&floor)
            .collect();
        let cut = affordable_cost(&mut costs);
        let trialled = |k: usize| b.nonlinear[k] && floor(k) <= cut;
        let n = (0..b.members.len()).filter(|k| trialled(*k)).count();
        let share = TRIAL_BUDGET / n.max(1) as u64;
        let deepest = (0..b.members.len())
            .filter(|k| trialled(*k))
            .map(|k| {
                trial_depth(
                    share,
                    (1 + u64::from(b.opens[k])).saturating_mul(block_step_cost(b.unknowns[k])),
                )
            })
            .max()
            .unwrap_or(0);
        (deepest, b.members.len())
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

    /// What element order can and cannot change, stated exactly.
    ///
    /// Layers 1-2 are integer work — value ranges, constraint keys,
    /// union-find over node pairs — and their VERDICT is exactly
    /// order-invariant. (Which part a multi-fault document names first is
    /// document order on purpose: "the first offender" is the reading a
    /// player expects, and every implicated id is in `ids()` anyway.)
    ///
    /// Layers 3-4 are floating-point. Element order fixes the order
    /// junctions are interned in, hence the node numbering, hence which
    /// permutation of the same matrix gets factored — and a document sitting
    /// exactly on the singular/convergent boundary can land either side of
    /// it. Measured over 8 000 fuzzed documents x 6 permutations, 12 flipped
    /// (0.025%), every one of them in the two numeric layers.
    ///
    /// That is not a bug and "fixing" it would make the gate LESS accurate,
    /// because of the property this test actually pins: the trial engine
    /// compiles the same slice the live engine will, so whatever ordering
    /// the room has is the ordering that was judged. An order-invariant
    /// verdict would have to answer for an ordering the room does not have.
    #[test]
    fn element_order_moves_only_the_numeric_layers() {
        let sw = |closed| ElementKind::Switch { closed };
        // Structural cases: the verdict and the code must survive any order.
        let structural: Vec<(&str, Vec<ElementSpec>)> = vec![
            ("healthy", base()),
            ("merged supplies", {
                let mut d = base();
                d.push(ElementSpec::two(4, dc(9.0), (0, 0), (0, 6)));
                d
            }),
            ("shorted", {
                let mut d = base();
                d.push(ElementSpec::two(4, ElementKind::Wire, (0, 0), (0, 6)));
                d
            }),
            ("conflict", {
                let mut d = base();
                d.push(ElementSpec::two(4, dc(1.0), (0, 0), (0, 6)));
                d
            }),
            (
                "loop",
                vec![
                    ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                    ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
                    ElementSpec::two(3, dc(3.0), (6, 6), (0, 0)),
                ],
            ),
            ("latent short", {
                let mut d = base();
                d.push(ElementSpec::two(4, sw(false), (0, 0), (0, 6)));
                d
            }),
            ("bad value", {
                let mut d = base();
                d[1] = ElementSpec::two(2, r(0.0), (0, 0), (0, 6));
                d
            }),
        ];
        for (why, d) in &structural {
            let want = check_document(d, DT).map_err(|r| r.code());
            for p in permutations(d) {
                assert_eq!(
                    check_document(&p, DT).map_err(|r| r.code()),
                    want,
                    "{why}: the verdict must not depend on element order"
                );
            }
        }

        // The property that makes the numeric layers' order-sensitivity
        // harmless: for EVERY ordering, the gate's answer is the truth about
        // that ordering. Accept => the engine compiled from the very same
        // slice survives the trial; a convergence refusal => it does not.
        let numeric: Vec<Vec<ElementSpec>> = structural
            .iter()
            .map(|(_, d)| d.clone())
            .chain([
                vec![
                    ElementSpec::two(1, ac(0.0, 9.0, 50.0, 0.0), (0, 0), (0, 6)),
                    ElementSpec::two(2, r(330.0), (0, 0), (4, 0)),
                    ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 6)),
                    ElementSpec::ground(4, (0, 6)),
                ],
                vec![
                    ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                    ElementSpec::two(2, ElementKind::Led { color: 0 }, (0, 0), (0, 6)),
                    ElementSpec::ground(3, (0, 6)),
                ],
            ])
            .collect();
        for d in &numeric {
            for p in permutations(d) {
                if check_document(&p, DT).is_err() {
                    continue;
                }
                let mut live = Engine::new(DT);
                live.set_elements(&p);
                assert!(
                    !live.advance(MAX_TRIAL_DEPTH).quarantined,
                    "accepted an ordering that then froze: {p:?}"
                );
            }
        }
    }

    /// Every rotation of `d`, plus its reverse: enough distinct orders to
    /// catch an order-dependent verdict, and deterministic.
    fn permutations(d: &[ElementSpec]) -> Vec<Vec<ElementSpec>> {
        let mut out = Vec::new();
        for k in 0..d.len() {
            let mut p: Vec<ElementSpec> = d[k..].to_vec();
            p.extend_from_slice(&d[..k]);
            out.push(p);
        }
        let mut rev = d.to_vec();
        rev.reverse();
        out.push(rev);
        out
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
