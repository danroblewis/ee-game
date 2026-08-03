//! `damage`: what a part can take, how hot it is, and when it lets go.
//!
//! Parts in this game have real safety limits. Overload one and it heats,
//! discolours, smokes and finally releases its magic smoke — after which it
//! is an open circuit until somebody finds it and repairs it. The point is
//! pedagogical: a player should SEE what overloading looks like before the
//! part dies, and should learn that a battery straight across a motor is not
//! a plan.
//!
//! ## Where this lives, and why it is not in sim-core
//!
//! sim-core owns exactly one damage mechanism — `Engine::set_broken`, which
//! makes a part stamp nothing — and nothing else. Ratings, accumulators and
//! the decision to break live here, outside the solve path, because:
//!   * they are bookkeeping, not numerics: the solver must stay a solver;
//!   * the S1 determinism harness pins sim-core's golden state hashes, and a
//!     per-element thermal field would change every one of them.
//!
//! The input is the tick's `ElemFrame` list — the same frame the room
//! broadcasts, so nothing here re-sweeps the document — and every number it
//! reads is a solver output. Stress is never guessed and never faked.
//!
//! ## The thermal model
//!
//! Each breakable part has a `Rating`: a metric (power, current or voltage),
//! a `limit` in that metric's unit, and a thermal time constant `tau`. The
//! part's `load` is `metric / limit`, its heat input is `load` for
//! power-rated parts and `load²` for current- and voltage-rated ones (see
//! `heat`), and its stress `s` in 0..=1 is a normalised TEMPERATURE chasing
//! that heat:
//!
//! ```text
//!   ds/dt = (heat - s) / tau        break when s >= 1
//! ```
//!
//! That is deliberately i²t-shaped rather than an instantaneous trip:
//!   * a brief spike barely moves `s` (heat is an integral, not a level);
//!   * a sustained overload cooks the part in `tau·ln(heat/(heat-1))`
//!     seconds — 2× rated power in a 6 s resistor is 4.2 s;
//!   * anything at or under its limit settles at `s = heat <= 1` and lives
//!     forever, while still reading visibly hot at 80 % of rated;
//!   * `s` decays with the same `tau` once the overload stops — parts cool.
//!
//! `s` is integrated with the ODE's exact solution rather than an Euler
//! step, because the caller's step (a 33 ms room tick) is not small next to
//! the fastest `tau` here (an LED's 0.35 s) and because a 1000× overload
//! would make an Euler step explode instead of simply breaking the part.
//!
//! ## Failure levels
//!
//! `s` maps to the four levels the client renders: ok → stressed → smoking →
//! BROKEN. The thresholds are the client's business (see `render.ts`); what
//! crosses the wire is the number, so the ramp stays continuous and honest.

use sim_core::{ElemFrame, ElementKind, ElementSpec};

/// Which solver output decides whether this part is in trouble.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Dissipated power, W (`|ElemFrame::power|`).
    Power,
    /// Largest terminal current, A.
    Current,
    /// Largest terminal-to-terminal voltage, V.
    Voltage,
}

/// One part kind's safety limit.
#[derive(Clone, Copy, Debug)]
pub struct Rating {
    pub metric: Metric,
    /// Failure limit in the metric's unit (W, A or V). Steady operation at
    /// or below it never breaks the part.
    pub limit: f64,
    /// Thermal time constant, s: how long the part takes to heat or cool.
    pub tau: f64,
}

impl Rating {
    const fn new(metric: Metric, limit: f64, tau: f64) -> Self {
        Rating { metric, limit, tau }
    }
}

/// Largest `load` ratio the model will consider. A dead short across an
/// ideal source can hand the solver an astronomically large (or non-finite)
/// current; clamping keeps `load²` finite so the part simply breaks on the
/// next tick instead of poisoning the accumulator with NaN. A part 1000×
/// over its limit is destroyed either way — this is about arithmetic, not
/// about physics.
pub const LOAD_MAX: f64 = 1000.0;

/// Stress at or above this means the part has let go.
pub const BREAK_AT: f64 = 1.0;

/// Below this, a part is cold enough to forget about (it stops being
/// reported and its bookkeeping entry is dropped).
pub const COLD: f64 = 0.02;

/// Stress worth telling the client about. A quiet world sends nothing.
pub const REPORT_AT: f64 = 0.05;

// ---------------------------------------------------------------- the table
//
// WHAT A PART CAN TAKE — the single source of truth, and the thing the tech
// tree will grow into.
//
// A rating is a property of the PART INSTANCE, not of its kind: a 0.25 W
// film resistor and a 5 W wirewound are the same `ElementKind::Resistor` at
// two different `ElementSpec::tier`s, electrically identical and thermally
// nothing alike. `tiers()` returns one row per rung for a kind, lowest
// first, and `rating()` picks the instance's rung. Shipping a new rung is
// adding a row here plus a catalogue entry in the client — it is content,
// not surgery, and it touches neither the solver nor the wire format
// (`tier` already rides every `ElementSpec` and defaults to 0).
//
// TIER 0 IS THE OPENING KIT, and it is deliberately feeble: this is a game
// about earning headroom, so the starting hand has to be the thing that
// makes the first unlock legible. Every tier-0 number below is a real
// still-air rating for the cheapest part of its class, and where a number
// moved in this pass it moved DOWN.
//
// | kind             | tier 0                  | tau   | why
// |------------------|-------------------------|-------|--------------------------
// | Wire             | current  3 A            | 8 s   | 22 AWG PVC hookup wire in
// |                  |                         |       | free air. Real conductors
// |                  |                         |       | have a current rating and
// |                  |                         |       | this one is measurable
// |                  |                         |       | today: see `K::Wire`. The
// |                  |                         |       | hoist's controlled drive
// |                  |                         |       | runs it at 1.7 A RMS; the
// |                  |                         |       | naive 12 V lead is 6 A and
// |                  |                         |       | would cook it in 2.3 s if
// |                  |                         |       | the motor (1.7 s) did not
// |                  |                         |       | go first
// | Resistor         | power    0.25 W         | 6 s   | quarter-watt carbon film,
// |                  |                         |       | THE default hobby part
// | Lamp             | power    2×rated_watts  | 4 s   | rated_watts is a bulb's
// |                  |                         |       | DESIGN point, not its
// |                  |                         |       | limit; the nameplate is
// |                  |                         |       | already per-instance, and
// |                  |                         |       | is the precedent this
// |                  |                         |       | whole tier idea follows
// | Speaker          | power    0.25 W         | 4 s   | 8 Ω 28 mm mylar cone
// | Potentiometer    | power    0.2 W          | 8 s   | 9 mm panel pot; the wiper
// |                  |                         |       | track is what cooks
// | Capacitor        | voltage  16 V           | 2 s   | the cheapest electrolytic
// |                  |                         |       | there is. A 12 V rail is
// |                  |                         |       | fine, 24 V vents it —
// |                  |                         |       | which is the lesson
// | Inductor         | current  0.3 A          | 5 s   | small radial choke
// | Motor            | current  3 A            | 6 s   | the hoist armature. 3 A
// |                  |                         |       | continuous: its 0.94 A
// |                  |                         |       | hold current is a third
// |                  |                         |       | of it, and 12 V across
// |                  |                         |       | R = 2 Ω stalls it at 6 A
// |                  |                         |       | and cooks it in 1.7 s
// | Switch / Button  | current  1 A            | 3 s   | a small toggle / tactile
// |                  |                         |       | switch. Contacts weld,
// |                  |                         |       | and "switch the motor
// |                  |                         |       | directly" must not work
// | Diode            | current  1 A            | 1 s   | 1N4001. Held at tier 0
// |                  |                         |       | on purpose: the flyback
// |                  |                         |       | lesson has to be
// |                  |                         |       | reachable on day one
// | Zener            | current  0.1 A          | 1 s   | half-watt zener at ~5 V
// | Led              | current  40 mA          | 0.35 s| 20 mA nominal, 40 mA
// |                  |                         |       | absolute max, and THE
// |                  |                         |       | first lesson: no series
// |                  |                         |       | resistor = instant death
// | Npn / Pnp        | power    0.35 W         | 2 s   | 2N3904 in still air (the
// |                  |                         |       | 625 mW on the datasheet
// |                  |                         |       | is at a 25 C case)
// | Nmos / Pmos      | power    0.35 W         | 2 s   | 2N7000, TO-92, no tab
// | OpAmp            | power    0.35 W         | 4 s   | DIP-8 in still air. Now
// |                  |                         |       | breakable: see below
// | Ota              | current  5 mA           | 2 s   | LM13700 — Iabc and the
// |                  |                         |       | output it mirrors are
// |                  |                         |       | both a few mA
// | Timer555         | power    0.35 W         | 3 s   | bipolar 555 in a DIP
// | VoltageSource    | current  2 A            | 20 s  | a battery pack / small
// | CurrentSource    | current  2 A            | 20 s  | bench supply: see the
// | Rail             | current  2 A            | 20 s  | note on tau below
// | Ground           | —                       | —     | a reference, not a part
//
// HIGHER TIERS SHIPPED SO FAR (the worked examples that prove the seam):
//
// | kind             | tier 1                  | tau   | what it is
// |------------------|-------------------------|-------|--------------------------
// | Resistor         | power    5 W            | 25 s  | ceramic wirewound; the
// |                  |                         |       | body is a heat sink, so
// |                  |                         |       | tau goes up with the watts
// | Nmos / Pmos      | power    20 W           | 10 s  | logic-level TO-220 power
// |                  |                         |       | MOSFET on a small sink —
// |                  |                         |       | the part that makes the
// |                  |                         |       | hoist solvable at all
// | Wire             | current  15 A           | 30 s  | 14 AWG; the thing you run
// |                  |                         |       | a motor on
//
// The metric column also picks the heat law (see `heat`): power-rated parts
// heat in proportion to the load, current- and voltage-rated ones as its
// square.

/// One rung of one kind's tier ladder: a rating plus the name a player sees.
#[derive(Clone, Copy, Debug)]
pub struct Tier {
    /// Short human name for this rung ("¼ W film", "power MOSFET").
    pub name: &'static str,
    pub metric: Metric,
    /// Failure limit in the metric's unit. For `Lamp` ONLY this is a
    /// multiple of the bulb's own `rated_watts` nameplate rather than an
    /// absolute figure — a bulb is the one part that already carries its
    /// design point in the document.
    pub limit: f64,
    pub tau: f64,
}

impl Tier {
    const fn new(name: &'static str, metric: Metric, limit: f64, tau: f64) -> Self {
        Tier {
            name,
            metric,
            limit,
            tau,
        }
    }
}

// The ladders themselves, one per family. `Metric` and the numbers are
// justified in the table above.
use Metric::{Current, Power, Voltage};

const NO_TIERS: &[Tier] = &[];
const WIRE: &[Tier] = &[
    Tier::new("22 AWG hookup", Current, 3.0, 8.0),
    Tier::new("14 AWG", Current, 15.0, 30.0),
];
const RESISTOR: &[Tier] = &[
    Tier::new("1/4 W film", Power, 0.25, 6.0),
    Tier::new("5 W wirewound", Power, 5.0, 25.0),
];
const LAMP: &[Tier] = &[Tier::new("bulb", Power, 2.0, 4.0)];
const SPEAKER: &[Tier] = &[Tier::new("8 ohm cone", Power, 0.25, 4.0)];
const POT: &[Tier] = &[Tier::new("panel pot", Power, 0.2, 8.0)];
/// A 5 mm CdS cell in an epoxy blob. 100 mW is generous for the package and
/// it is the same order as a quarter-watt resistor, which is what it is:
/// a resistor you are not allowed to choose the value of.
const PHOTOCELL: &[Tier] = &[Tier::new("5 mm CdS cell", Power, 0.1, 6.0)];
const CAPACITOR: &[Tier] = &[
    Tier::new("16 V electrolytic", Voltage, 16.0, 2.0),
    Tier::new("100 V film", Voltage, 100.0, 3.0),
];
const INDUCTOR: &[Tier] = &[Tier::new("radial choke", Current, 0.3, 5.0)];
const MOTOR: &[Tier] = &[Tier::new("hoist armature", Current, 3.0, 6.0)];
const CONTACTS: &[Tier] = &[Tier::new("toggle", Current, 1.0, 3.0)];
const DIODE: &[Tier] = &[
    Tier::new("1N4001", Current, 1.0, 1.0),
    Tier::new("3 A Schottky", Current, 3.0, 2.0),
];
const ZENER: &[Tier] = &[Tier::new("1/2 W zener", Current, 0.1, 1.0)];
const LED: &[Tier] = &[Tier::new("5 mm LED", Current, 0.04, 0.35)];
const BJT: &[Tier] = &[
    Tier::new("TO-92 small-signal", Power, 0.35, 2.0),
    Tier::new("TO-220 power BJT", Power, 15.0, 10.0),
];
const MOSFET: &[Tier] = &[
    Tier::new("TO-92 small-signal", Power, 0.35, 2.0),
    Tier::new("TO-220 power MOSFET", Power, 20.0, 10.0),
];
const OPAMP: &[Tier] = &[Tier::new("DIP-8", Power, 0.35, 4.0)];
const OTA: &[Tier] = &[Tier::new("LM13700", Current, 0.005, 2.0)];
const TIMER555: &[Tier] = &[Tier::new("DIP-8 bipolar", Power, 0.35, 3.0)];
const SUPPLY: &[Tier] = &[Tier::new("battery pack", Current, 2.0, 20.0)];

/// The tier ladder for a kind, lowest rung first. Empty means the kind has
/// no failure mode at all — see `K::Ground`.
pub fn tiers(kind: &ElementKind) -> &'static [Tier] {
    use ElementKind as K;
    match kind {
        // Ground is a REFERENCE, not a component: it is the statement "this
        // node is 0 V", it has no conductor, no package and no dissipation,
        // and `ElemFrame` reports it no current to be judged on. There is
        // nothing here to rate, so it is the one honest empty row in the
        // table. (Contrast Wire, which is a real conductor and does have
        // one.)
        K::Ground => NO_TIERS,
        // Wire IS a conductor and it gets a rating, because "you cannot run
        // a motor through signal wire" is exactly the class of lesson this
        // model exists to teach, and because M6's world wire runs (gauge
        // R/m, R(T)) will need the ladder to already be here.
        //
        // Caveat, stated rather than hidden: a wire has no MNA unknown of
        // its own, so its current comes from the KCL recovery pass in
        // `Engine::solve_wire_currents`, which resolves a wire only when its
        // junction has exactly one unknown incident current. A wire inside a
        // pure-wire loop, or the leg that lands on a ground symbol, reads
        // 0 A and therefore never breaks. That fails SAFE — it under-reports,
        // never over-reports — and it is the same number already drawn as
        // current dots, so no display and no rating can disagree. When M6
        // gives wire a real conductance the recovery goes away and the
        // rating starts biting everywhere.
        K::Wire => WIRE,
        K::Resistor { .. } => RESISTOR,
        // A bulb is MEANT to run at rated_watts, so its failure limit sits
        // above it — the limit column is a MULTIPLE here, applied against the
        // instance's own nameplate in `rating`.
        K::Lamp { .. } => LAMP,
        K::Speaker { .. } => SPEAKER,
        K::Potentiometer { .. } => POT,
        K::Photocell { .. } => PHOTOCELL,
        K::Capacitor { .. } => CAPACITOR,
        K::Inductor { .. } => INDUCTOR,
        K::Motor { .. } => MOTOR,
        K::Switch { .. } | K::Button { .. } => CONTACTS,
        K::Diode => DIODE,
        K::Zener { .. } => ZENER,
        K::Led { .. } => LED,
        K::Npn { .. } | K::Pnp { .. } => BJT,
        K::Nmos { .. } | K::Pmos { .. } => MOSFET,
        // Op-amps ARE breakable now, and the thing that made them
        // unbreakable was never the rating — it was the model. Their frame
        // power used to be what they DELIVERED (no supply pins, so the
        // return current vanishes into node 0) and a railed output sat at
        // exactly ±rail, so the output stage's own drop was identically
        // zero however much current it passed. Both are fixed upstream:
        // `ElementKind::OpAmp` now carries a short-circuit limit `isc` that
        // the solver actually enforces, and `sim_core`'s `elem_power`
        // reports an op-amp's dissipation as |i_out|·(rail - sign·vout) —
        // solved current against solved voltage, no invented terms. So the
        // number this rating judges really is the chip's own heat.
        //
        // 0.35 W is a DIP-8 in still air. A 25 mA part shorted on ±5 V
        // burns 0.125 W: hot, and immortal, which is the truth about a
        // jellybean op-amp. The same short on a ±100 V part is 2.5 W and
        // kills it in about 1.6 s.
        K::OpAmp { .. } => OPAMP,
        // The OTA is rated by CURRENT rather than power for the same
        // reason, arrived at from the other side: it too has no supply
        // pins, but its real datasheet limit is a current one anyway. An
        // LM13700's amplifier bias current tops out around 2 mA and the
        // output mirrors it, so `Metric::Current` over its pins (which
        // takes the largest of Iabc and Iout) is measuring exactly the
        // quantity the datasheet limits. 5 mA is the absolute-max end of
        // that range.
        K::Ota => OTA,
        // The 555 was always honest: its VCC and GND pins ARE modelled, so
        // the frame's power really is the chip's own dissipation.
        K::Timer555 => TIMER555,
        // Sources: 2 A, down from 10 A. The old 10 A was policy ("generous,
        // so the LOAD dies first") dressed as a rating, and 10 A is not a
        // battery a beginner owns. The policy survives anyway, and now it
        // survives for a physical reason instead of a fudge: tau is 20 s.
        // A battery pack has orders of magnitude more thermal mass than the
        // part it is cooking, so on the classic mistakes — an LED straight
        // across a cell, a stalled motor — the load reaches its own limit in
        // a tick or two while the supply has barely warmed, and the player
        // loses the part that was wrong rather than the part that was fine.
        // Hold a real 3 A short for half a minute and the supply does die,
        // which is also true.
        // `Noise` is a source too — it arrived with the synth world after this
        // ladder was written, and it is a Norton source like the rest.
        K::VoltageSource { .. } | K::CurrentSource { .. } | K::Rail { .. } | K::Noise { .. } => SUPPLY,
    }
}

/// The safety limit for one placed part: its kind's ladder, at its own
/// tier. `None` only for kinds with no failure mode (`Ground`).
///
/// A tier past the end of the ladder clamps to the top rung rather than
/// failing: `check_document` already refuses anything above `MAX_TIER`, so
/// the only way to get here is a document written by a NEWER build that
/// knew about a rung this one does not. Clamping means such a room still
/// loads and still cooks its parts; it just judges them against the best
/// rung this build understands.
pub fn rating(kind: &ElementKind, tier: u8) -> Option<Rating> {
    let ladder = tiers(kind);
    let t = *ladder.get(tier as usize).or_else(|| ladder.last())?;
    let limit = match kind {
        // The one nameplate-relative row: a bulb's limit is a multiple of
        // its own design point. Guard the multiply — a zero or absurd
        // nameplate must not make an unbreakable (or instantly broken) lamp.
        ElementKind::Lamp { rated_watts, .. } => t.limit * rated_watts.clamp(0.01, 1.0e6),
        _ => t.limit,
    };
    Some(Rating::new(t.metric, limit, t.tau))
}

/// The name of the rung a placed part sits on ("1/4 W film"), for the
/// properties panel and the catalogue. `None` for unrated kinds.
pub fn tier_name(kind: &ElementKind, tier: u8) -> Option<&'static str> {
    let ladder = tiers(kind);
    ladder.get(tier as usize).or_else(|| ladder.last()).map(|t| t.name)
}

/// Short human name for a kind — for the server log line and the client's
/// "released its magic smoke" toast.
pub fn kind_name(kind: &ElementKind) -> &'static str {
    use ElementKind as K;
    match kind {
        K::Wire => "Wire",
        K::Ground => "Ground",
        K::Resistor { .. } => "Resistor",
        K::Lamp { .. } => "Lamp",
        K::Speaker { .. } => "Speaker",
        K::Capacitor { .. } => "Capacitor",
        K::Inductor { .. } => "Inductor",
        K::VoltageSource { .. } => "Source",
        K::CurrentSource { .. } => "Current source",
        K::Rail { .. } => "Rail",
        K::Switch { .. } => "Switch",
        K::Button { .. } => "Button",
        K::Diode => "Diode",
        K::Zener { .. } => "Zener",
        K::Led { .. } => "LED",
        K::Npn { .. } => "NPN",
        K::Pnp { .. } => "PNP",
        K::Nmos { .. } => "NMOS",
        K::Pmos { .. } => "PMOS",
        K::OpAmp { .. } => "Op-amp",
        K::Ota => "OTA",
        K::Timer555 => "555",
        K::Potentiometer { .. } => "Potentiometer",
        K::Photocell { .. } => "Photocell",
        K::Motor { .. } => "Motor",
        K::Noise { .. } => "Noise source",
    }
}

/// The metric's magnitude for one element, straight out of the tick frame.
/// Non-finite solver output reads as an extreme overload rather than
/// propagating NaN into the accumulator.
pub fn measure(metric: Metric, f: &ElemFrame) -> f64 {
    // NOTE: f64::max/min swallow NaN, so every candidate is checked for
    // finiteness explicitly. A NaN current must read as "destroyed", never as
    // the zero that `0.0f64.max(NAN)` would quietly hand back.
    let npins = f.npins.min(f.v.len());
    let m = match metric {
        Metric::Power => f.power.abs(),
        Metric::Current => {
            let mut m = 0.0f64;
            for p in 0..npins {
                let a = f.i[p].abs();
                if !a.is_finite() {
                    return f64::INFINITY;
                }
                if a > m {
                    m = a;
                }
            }
            m
        }
        // Terminal-to-terminal, NOT pin-to-ground: a capacitor floating at
        // 30 V with 1 V across it is fine, and must read as 1 V.
        Metric::Voltage if npins > 0 => {
            let mut lo = f.v[0];
            let mut hi = f.v[0];
            for p in 0..npins {
                let v = f.v[p];
                if !v.is_finite() {
                    return f64::INFINITY;
                }
                if v < lo {
                    lo = v;
                }
                if v > hi {
                    hi = v;
                }
            }
            hi - lo
        }
        Metric::Voltage => 0.0,
    };
    if m.is_finite() {
        m
    } else {
        f64::INFINITY // clamped by `load`
    }
}

/// `metric / limit`, clamped to `LOAD_MAX` and never non-finite.
pub fn load(rating: &Rating, f: &ElemFrame) -> f64 {
    let m = measure(rating.metric, f);
    if rating.limit <= 0.0 || !m.is_finite() {
        return LOAD_MAX;
    }
    (m / rating.limit).clamp(0.0, LOAD_MAX)
}

/// Normalised heat input for a load — the stress the part settles at if that
/// load is held forever, and the `target` the accumulator chases.
///
/// The exponent is the physics, not a tuning knob: a part rated by POWER
/// heats in proportion to the load itself (temperature rise follows
/// dissipation), while one rated by CURRENT or VOLTAGE heats as its square,
/// because that is what i²R is. It matters for anything periodic — an 8 Ω
/// series resistor passing a 440 Hz tone settles at its MEAN power, exactly
/// as a real one does, instead of being punished for the waveform's crest
/// factor.
pub fn heat(metric: Metric, load: f64) -> f64 {
    match metric {
        Metric::Power => load,
        Metric::Current | Metric::Voltage => load * load,
    }
}

/// Advance one part's stress by `h` seconds at a given heat input, using the
/// exact solution of `ds/dt = (heat - s)/tau`. Pure; the whole failure model
/// is this one line of arithmetic.
pub fn advance_stress(stress: f64, heat: f64, tau: f64, h: f64) -> f64 {
    if !(h.is_finite() && h > 0.0 && tau.is_finite() && tau > 0.0) {
        return stress.clamp(0.0, BREAK_AT);
    }
    let target = heat.clamp(0.0, LOAD_MAX * LOAD_MAX);
    let decay = libm::exp(-h / tau);
    (target + (stress - target) * decay).clamp(0.0, BREAK_AT)
}

/// Seconds of steady overload at a given heat input before a part with this
/// `tau` breaks from cold. `None` when it never will (`heat <= 1`).
/// Documentation and tests use it; the model itself never needs it.
pub fn time_to_break(heat: f64, tau: f64) -> Option<f64> {
    // NaN-safe on purpose: an unknown heat never promises a failure time.
    if !(heat.is_finite() && heat > BREAK_AT) {
        return None;
    }
    Some(tau * libm::log(heat / (heat - BREAK_AT)))
}

/// Live thermal state for one part. Only warm or dead parts get an entry.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Part {
    pub id: u32,
    /// Normalised temperature, 0..=1. 1 means it has broken.
    pub stress: f64,
    pub broken: bool,
}

/// A part that broke on this tick — the magic-smoke event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Broke {
    pub id: u32,
    pub kind: &'static str,
    /// The load that killed it (multiples of its limit), for the log line.
    pub load: f64,
}

/// One document element's rating, resolved once per document edit.
#[derive(Clone, Copy, Debug)]
struct Rated {
    id: u32,
    rating: Rating,
    name: &'static str,
}

/// The room's damage bookkeeping: every part's stress, and which ones are
/// dead. Persisted in the room checkpoint, so a repair job survives a
/// restart.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct DamageModel {
    /// Ratings for the current document, sorted by id — derived state,
    /// rebuilt by `set_document`, never persisted.
    #[cfg_attr(feature = "serde", serde(skip))]
    rated: Vec<Rated>,
    /// Thermal state, sorted by id. Cold parts are pruned, so a quiet room
    /// keeps an empty vector.
    parts: Vec<Part>,
}

impl DamageModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-derive the ratings from the document. Call on every document edit
    /// (an `InteractOp` cannot change a rating, so knob drags need not).
    /// Parts that no longer exist lose their thermal state — an id is never
    /// reused, so a new part always starts cold.
    pub fn set_document(&mut self, elems: &[ElementSpec]) {
        self.rated.clear();
        self.rated.reserve(elems.len());
        for e in elems {
            if let Some(rating) = rating(&e.kind, e.tier) {
                self.rated.push(Rated {
                    id: e.id,
                    rating,
                    name: kind_name(&e.kind),
                });
            }
        }
        self.rated.sort_by_key(|r| r.id);
        self.rated.dedup_by_key(|r| r.id);
        let rated = &self.rated;
        self.parts
            .retain(|p| rated.binary_search_by_key(&p.id, |r| r.id).is_ok());
    }

    /// Integrate every part's stress over `h` seconds of SIM time from this
    /// tick's frame, and return the parts that just broke.
    ///
    /// `h` must be the simulated time the tick actually advanced (a
    /// budget-limited tick advances less, and must therefore cook less).
    /// Callers pass 0 for a tick that produced no new numbers.
    ///
    /// Quarantine is handled here rather than by the caller, and per part:
    /// a part whose island stopped solving carries no new numbers, so there
    /// is nothing to integrate for it — while every part on every OTHER
    /// island is still dissipating real power and must still cook. Skipping
    /// the whole sweep because something, somewhere, diverged is an exploit:
    /// it lets a player make an overloaded part immortal by quarantining an
    /// unrelated board.
    pub fn tick(&mut self, frames: &[ElemFrame], h: f64) -> Vec<Broke> {
        let mut broke = Vec::new();
        if !(h.is_finite() && h > 0.0) || self.rated.is_empty() {
            return broke;
        }
        for f in frames {
            if f.quarantined {
                continue; // no new numbers: a frozen circuit cooks nothing
            }
            let Ok(k) = self.rated.binary_search_by_key(&f.id, |r| r.id) else {
                continue; // wire, ground, or a part the document dropped
            };
            let Rated {
                rating: r, name, ..
            } = self.rated[k];
            let slot = self.parts.binary_search_by_key(&f.id, |p| p.id);
            // A dead part is frozen: it conducts nothing, so there is nothing
            // to integrate, and it must not cool its way back to health.
            if let Ok(s) = slot {
                if self.parts[s].broken {
                    continue;
                }
            }
            let was = slot.map(|s| self.parts[s].stress).unwrap_or(0.0);
            let l = load(&r, f);
            let hot = heat(r.metric, l);
            let now = advance_stress(was, hot, r.tau, h);
            // Bookkeeping is kept for anything that is warm OR is being
            // warmed (equilibrium above COLD). Dropping merely-cool parts is
            // what keeps a quiet room free, but a part climbing THROUGH the
            // cold band must keep its entry or it can never heat up at all.
            let cold = now < COLD && hot < COLD;
            if now >= BREAK_AT {
                let part = Part {
                    id: f.id,
                    stress: BREAK_AT,
                    broken: true,
                };
                match slot {
                    Ok(s) => self.parts[s] = part,
                    Err(s) => self.parts.insert(s, part),
                }
                broke.push(Broke {
                    id: f.id,
                    kind: name,
                    load: l,
                });
            } else if cold {
                if let Ok(s) = slot {
                    self.parts.remove(s); // cooled off: forget it entirely
                }
            } else {
                match slot {
                    Ok(s) => self.parts[s].stress = now,
                    Err(s) => self.parts.insert(
                        s,
                        Part {
                            id: f.id,
                            stress: now,
                            broken: false,
                        },
                    ),
                }
            }
        }
        broke
    }

    /// Repair a part: it conducts again and starts cold. Returns false when
    /// the id is not broken (so callers can ignore a stale click).
    ///
    /// A repair is a WORLD event, not a document edit: it is not undoable and
    /// it is allowed on server-owned fixtures.
    pub fn repair(&mut self, id: u32) -> bool {
        let Ok(s) = self.parts.binary_search_by_key(&id, |p| p.id) else {
            return false;
        };
        if !self.parts[s].broken {
            return false;
        }
        self.parts.remove(s);
        true
    }

    /// Break a part on purpose (test setups, and a future sabotage verb).
    pub fn force_break(&mut self, id: u32) {
        let part = Part {
            id,
            stress: BREAK_AT,
            broken: true,
        };
        match self.parts.binary_search_by_key(&id, |p| p.id) {
            Ok(s) => self.parts[s] = part,
            Err(s) => self.parts.insert(s, part),
        }
    }

    pub fn stress(&self, id: u32) -> f64 {
        self.part(id).map(|p| p.stress).unwrap_or(0.0)
    }

    pub fn is_broken(&self, id: u32) -> bool {
        self.part(id).map(|p| p.broken).unwrap_or(false)
    }

    pub fn part(&self, id: u32) -> Option<&Part> {
        self.parts
            .binary_search_by_key(&id, |p| p.id)
            .ok()
            .map(|s| &self.parts[s])
    }

    /// Every part with thermal state, sorted by id.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn broken_ids(&self) -> Vec<u32> {
        self.parts
            .iter()
            .filter(|p| p.broken)
            .map(|p| p.id)
            .collect()
    }

    /// The rating a part is being judged against, if it has one.
    pub fn rating_of(&self, id: u32) -> Option<Rating> {
        self.rated
            .binary_search_by_key(&id, |r| r.id)
            .ok()
            .map(|k| self.rated[k].rating)
    }

    /// The lossy per-tick report: `[id, stress, broken]` for everything worth
    /// drawing, dead parts first, capped at `cap` entries. Stress is rounded
    /// to 1/1000 — it drives a colour ramp, and three digits keep the JSON
    /// short.
    pub fn report(&self, cap: usize) -> Vec<[f64; 3]> {
        let mut out: Vec<[f64; 3]> = Vec::new();
        let row = |p: &Part| {
            [
                p.id as f64,
                (p.stress * 1000.0).round() / 1000.0,
                if p.broken { 1.0 } else { 0.0 },
            ]
        };
        // Broken parts are what a player has to FIND, so they never lose
        // their slot to a merely warm one.
        for p in self.parts.iter().filter(|p| p.broken) {
            if out.len() >= cap {
                return out;
            }
            out.push(row(p));
        }
        for p in self
            .parts
            .iter()
            .filter(|p| !p.broken && p.stress >= REPORT_AT)
        {
            if out.len() >= cap {
                return out;
            }
            out.push(row(p));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::MAX_PINS;

    fn frame(id: u32, power: f64, i0: f64, v: (f64, f64)) -> ElemFrame {
        let mut f = ElemFrame {
            id,
            npins: 2,
            v: [0.0; MAX_PINS],
            i: [0.0; MAX_PINS],
            power,
            quarantined: false,
        };
        f.i[0] = i0;
        f.i[1] = -i0;
        f.v[0] = v.0;
        f.v[1] = v.1;
        f
    }

    #[test]
    fn every_breakable_kind_has_a_sane_rating() {
        use ElementKind as K;
        let kinds = [
            K::Resistor { ohms: 1000.0 },
            K::Lamp {
                ohms: 90.0,
                rated_watts: 1.0,
            },
            K::Speaker { ohms: 8.0 },
            K::Potentiometer {
                ohms: 10_000.0,
                wiper: 0.5,
            },
            K::Capacitor { farads: 1e-6 },
            K::Inductor { henries: 1e-3 },
            K::Motor {
                ohms: 2.0,
                henries: 1.5e-3,
                bemf: 0.0,
            },
            K::Switch { closed: true },
            K::Button { closed: false },
            K::Diode,
            K::Zener { vz: 5.6 },
            K::Led { color: 0 },
            K::Npn { beta: 100.0 },
            K::Pnp { beta: 100.0 },
            K::Nmos { vt: 1.5, k: 0.05 },
            K::Pmos { vt: 1.5, k: 0.05 },
            K::Timer555,
            K::VoltageSource {
                dc: 9.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
            },
            K::CurrentSource { amps: 0.01 },
            K::Wire,
            K::OpAmp {
                rail: 12.0,
                isc: sim_core::DEFAULT_OPAMP_ISC,
            },
            K::Ota,
        ];
        for k in kinds {
            let r = rating(&k, 0).unwrap_or_else(|| panic!("{} has no rating", kind_name(&k)));
            assert!(r.limit > 0.0 && r.limit.is_finite(), "{:?}", r);
            assert!(r.tau > 0.0 && r.tau.is_finite(), "{:?}", r);
            assert_ne!(kind_name(&k), "");
            // Every rung of every ladder has to be sane too, or a tech-tree
            // unlock could ship a part that is unbreakable or born dead.
            for (t, tier) in tiers(&k).iter().enumerate() {
                let r = rating(&k, t as u8).expect("a listed tier must resolve");
                assert!(r.limit > 0.0 && r.limit.is_finite(), "{}: {:?}", tier.name, r);
                assert!(r.tau > 0.0 && r.tau.is_finite(), "{}: {:?}", tier.name, r);
                assert!(!tier.name.is_empty());
            }
        }
        // Ground is the ONE exemption, and it is not an oversight: it is a
        // reference, not a component — no conductor, no package, no
        // dissipation, and no current reported against it to judge.
        assert!(rating(&K::Ground, 0).is_none());
        assert!(tiers(&K::Ground).is_empty());
        assert!(tier_name(&K::Ground, 0).is_none());
        // A nonsense lamp nameplate cannot produce a nonsense limit.
        assert!(
            rating(
                &K::Lamp {
                    ohms: 90.0,
                    rated_watts: 0.0
                },
                0
            )
            .unwrap()
            .limit
                > 0.0
        );
    }

    #[test]
    fn a_tier_is_the_same_part_with_more_headroom() {
        use ElementKind as K;
        // The worked example the tech tree needs: one kind, two rungs, the
        // higher one strictly tougher and named for a player.
        let r = K::Resistor { ohms: 100.0 };
        let film = rating(&r, 0).unwrap();
        let ww = rating(&r, 1).unwrap();
        assert_eq!(film.metric, ww.metric, "a tier changes the limit, not the law");
        assert_eq!(film.limit, 0.25);
        assert_eq!(ww.limit, 5.0);
        assert!(ww.tau > film.tau, "a bigger body is also a slower one");
        assert_eq!(tier_name(&r, 0), Some("1/4 W film"));
        assert_eq!(tier_name(&r, 1), Some("5 W wirewound"));
        // 1 W: cooks the film part, and is a quiet fifth of the wirewound.
        let f = frame(1, 1.0, 0.0, (0.0, 0.0));
        assert!(time_to_break(heat(film.metric, load(&film, &f)), film.tau).is_some());
        assert!(time_to_break(heat(ww.metric, load(&ww, &f)), ww.tau).is_none());
        // A tier this build has never heard of clamps to the best rung it
        // knows rather than making the part immortal.
        assert_eq!(rating(&r, 200).unwrap().limit, 5.0);
        assert_eq!(tier_name(&r, 200), Some("5 W wirewound"));
    }

    #[test]
    fn the_starting_kit_teaches_the_classic_lessons() {
        use ElementKind as K;
        let h = 1.0 / 30.0;
        let cook = |k: &ElementKind, tier: u8, f: &ElemFrame, secs: f64| -> Option<f64> {
            let r = rating(k, tier).unwrap();
            let hot = heat(r.metric, load(&r, f));
            let mut s = 0.0;
            let mut t = 0.0;
            while t < secs {
                s = advance_stress(s, hot, r.tau, h);
                t += h;
                if s >= BREAK_AT {
                    return Some(t);
                }
            }
            None
        };
        // An LED with no series resistor, straight across a 9 V cell.
        let led = K::Led { color: 0 };
        assert!(cook(&led, 0, &frame(1, 0.0, 3.0, (9.0, 0.0)), 1.0).unwrap() <= h);
        // ...and the 9 V cell behind it does NOT die for the LED's mistake:
        // 3 A is 1.5x its 2 A rating, but the LED is gone within one tick
        // and the pack has barely warmed by then.
        let cell = K::VoltageSource {
            dc: 9.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        };
        let cr = rating(&cell, 0).unwrap();
        let warm = advance_stress(
            0.0,
            heat(cr.metric, load(&cr, &frame(2, 0.0, 3.0, (9.0, 0.0)))),
            cr.tau,
            h,
        );
        assert!(warm < 0.02, "the supply must survive the load's mistake: {warm}");
        // Hold that same short for half a minute and the supply DOES die,
        // so the generosity is thermal mass, not immunity.
        assert!(cook(&cell, 0, &frame(2, 0.0, 3.0, (9.0, 0.0)), 60.0).is_some());
        // An electrolytic on a 24 V rail vents; on 12 V it is fine forever.
        let cap = K::Capacitor { farads: 100e-6 };
        assert!(cook(&cap, 0, &frame(3, 0.0, 0.0, (24.0, 0.0)), 30.0).is_some());
        assert!(cook(&cap, 0, &frame(3, 0.0, 0.0, (12.0, 0.0)), 600.0).is_none());
        // A stalled hoist motor: 12 V across R = 2 ohm is 6 A, twice rated.
        let motor = K::Motor {
            ohms: 2.0,
            henries: 1.5e-3,
            bemf: 0.0,
        };
        let stall = cook(&motor, 0, &frame(4, 0.0, 6.0, (12.0, 0.0)), 30.0).unwrap();
        assert!(stall < 3.0, "a stalled motor must cook fast: {stall} s");
        // ...but its 0.94 A hold current never does.
        assert!(cook(&motor, 0, &frame(4, 0.0, 0.942, (12.0, 0.0)), 600.0).is_none());
        // A toggle switch is not a motor controller: 1 A rated, and the
        // hoist's 6 A inrush welds it.
        let sw = K::Switch { closed: true };
        assert!(cook(&sw, 0, &frame(5, 0.0, 6.0, (12.0, 0.0)), 30.0).is_some());
        // An op-amp shorted on +/-5 V is hot and immortal (0.125 W of a
        // 0.35 W part); the same short on a +/-100 V part kills it.
        let small = K::OpAmp {
            rail: 5.0,
            isc: 0.025,
        };
        let big = K::OpAmp {
            rail: 100.0,
            isc: 0.025,
        };
        assert!(cook(&small, 0, &frame(6, 0.125, 0.025, (0.0, 0.0)), 600.0).is_none());
        assert!(cook(&big, 0, &frame(6, 2.5, 0.025, (0.0, 0.0)), 30.0).is_some());
    }

    #[test]
    fn at_or_under_the_limit_a_part_lives_forever() {
        let r = rating(&ElementKind::Resistor { ohms: 100.0 }, 0).unwrap();
        // 80 % of a half-watt part: hot, and immortal. 600 s of it.
        let f = frame(1, 0.8 * r.limit, 0.0, (0.0, 0.0));
        let mut s = 0.0;
        for _ in 0..18_000 {
            s = advance_stress(s, heat(r.metric, load(&r, &f)), r.tau, 1.0 / 30.0);
            assert!(s < BREAK_AT, "80 % of rating must never break: {s}");
        }
        // It settles at 0.8 of the failure temperature: plainly hot, which
        // is the truth about a part run at 80 % of rated dissipation.
        assert!((s - 0.8).abs() < 1e-3, "settled at {s}");
        assert_eq!(time_to_break(0.8, r.tau), None);
        // A current-rated part heats as the square, so 80 % of ITS rating is
        // a milder 0.64.
        let led = rating(&ElementKind::Led { color: 0 }, 0).unwrap();
        assert!((heat(led.metric, 0.8) - 0.64).abs() < 1e-12);
        // Exactly at the limit is the knee: it converges on 1 and never
        // crosses it.
        let mut s = 0.0;
        for _ in 0..18_000 {
            s = advance_stress(s, 1.0, r.tau, 1.0 / 30.0);
        }
        assert!(s < BREAK_AT);
    }

    #[test]
    fn sustained_overload_cooks_but_a_spike_does_not() {
        let r = rating(&ElementKind::Resistor { ohms: 100.0 }, 0).unwrap();
        // 2× rated power: the closed form says tau·ln(2) = 4.159 s, and the
        // integrator has to agree with it.
        let want = time_to_break(2.0, r.tau).unwrap();
        assert!((want - 4.159).abs() < 0.01, "closed form {want}");
        let h = 1.0 / 30.0;
        let mut s = 0.0;
        let mut t = 0.0;
        while s < BREAK_AT && t < 60.0 {
            s = advance_stress(s, 2.0, r.tau, h);
            t += h;
        }
        assert!(s >= BREAK_AT, "2× rated must break");
        assert!((t - want).abs() < h * 2.0, "broke at {t}, expected {want}");

        // A 100× spike lasting 20 ms is survivable: heat is an integral.
        let mut s = 0.0;
        s = advance_stress(s, 100.0, r.tau, 0.02);
        assert!(s < BREAK_AT, "a 20 ms spike must not break it: {s}");
        // ...and it cools back down within a few tau.
        for _ in 0..1800 {
            s = advance_stress(s, 0.0, r.tau, h);
        }
        assert!(s < COLD, "must cool off: {s}");
    }

    #[test]
    fn an_led_with_no_resistor_dies_at_once_and_nan_cannot_poison_it() {
        let r = rating(&ElementKind::Led { color: 0 }, 0).unwrap();
        // The solver's answer for an ideal source straight across a diode is
        // enormous (or non-finite). Either way: one tick.
        for i in [50.0, 1e30, f64::INFINITY, f64::NAN] {
            let f = frame(1, 0.0, i, (9.0, 0.0));
            let l = load(&r, &f);
            assert!(l.is_finite() && l > 1.0, "load {l} for {i} A");
            let s = advance_stress(0.0, heat(r.metric, l), r.tau, 1.0 / 30.0);
            assert!(s.is_finite());
            assert!(s >= BREAK_AT, "bare LED must break in one tick, got {s}");
        }
        // 21 mA behind a 330 Ω resistor is its whole working life.
        let f = frame(1, 0.0, 0.021, (2.1, 0.0));
        let mut s = 0.0;
        for _ in 0..3000 {
            s = advance_stress(s, heat(r.metric, load(&r, &f)), r.tau, 1.0 / 30.0);
        }
        assert!(s < 0.3, "a properly driven LED stays cool: {s}");
    }

    #[test]
    fn the_metric_is_the_one_the_kind_is_rated_by() {
        // Voltage-rated: an electrolytic sees its terminal difference.
        let cap = rating(&ElementKind::Capacitor { farads: 1e-6 }, 0).unwrap();
        assert_eq!(cap.metric, Metric::Voltage);
        // 32 V across a 16 V part: floating at 30 V with only 32 V across it
        // is still exactly 2x, because the metric is terminal-to-terminal.
        assert!((load(&cap, &frame(1, 0.0, 0.0, (30.0, -2.0))) - 2.0).abs() < 1e-12);
        // Current-rated: the largest terminal current, whatever its sign.
        let mot = rating(&ElementKind::Motor {
            ohms: 2.0,
            henries: 1.5e-3,
            bemf: 0.0,
        }, 0)
        .unwrap();
        assert_eq!(mot.metric, Metric::Current);
        assert!((load(&mot, &frame(1, 0.0, -6.0, (12.0, 0.0))) - 2.0).abs() < 1e-12);
        // Power-rated: sign is irrelevant, a part delivering power heats too.
        let res = rating(&ElementKind::Resistor { ohms: 1.0 }, 0).unwrap();
        assert_eq!(res.metric, Metric::Power);
        assert!((load(&res, &frame(1, -0.5, 0.0, (0.0, 0.0))) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_model_tracks_a_document_and_prunes_cold_and_deleted_parts() {
        let elems = vec![
            ElementSpec::two(1, ElementKind::Resistor { ohms: 100.0 }, (0, 0), (4, 0)),
            ElementSpec::ground(2, (8, 0)),
        ];
        let mut d = DamageModel::new();
        d.set_document(&elems);
        assert!(d.rating_of(1).is_some());
        assert!(d.rating_of(2).is_none(), "a ground reference does not break");

        // Warm it up without breaking it, then let it cool: the entry appears
        // and then disappears again, so a quiet room costs nothing.
        let warm = frame(1, 0.225, 0.0, (0.0, 0.0));
        for _ in 0..300 {
            assert!(d.tick(&[warm], 1.0 / 30.0).is_empty());
        }
        assert!(d.stress(1) > REPORT_AT);
        assert_eq!(d.report(64).len(), 1);
        // Switched off: it cools with the same 10 s time constant, so a
        // minute later there is nothing left to remember.
        let cold = frame(1, 0.0, 0.0, (0.0, 0.0));
        for _ in 0..1800 {
            d.tick(&[cold], 1.0 / 30.0);
        }
        assert_eq!(d.parts().len(), 0, "cold parts are forgotten");
        assert!(d.report(64).is_empty());

        // Break it, then delete it from the document: the state goes with it.
        let hot = frame(1, 5.0, 0.0, (0.0, 0.0));
        let broke = d.tick(&[hot], 1.0);
        assert_eq!(broke.len(), 1);
        assert_eq!(broke[0].id, 1);
        assert!(d.is_broken(1));
        // A broken part conducts nothing, so its frame goes quiet — and it
        // must NOT cool its way back to health.
        for _ in 0..600 {
            d.tick(&[cold], 1.0 / 30.0);
        }
        assert!(d.is_broken(1), "a dead part stays dead until repaired");
        assert_eq!(d.report(64), vec![[1.0, 1.0, 1.0]]);
        d.set_document(&elems[1..]);
        assert!(!d.is_broken(1));
        assert!(d.parts().is_empty());

        // A tick that advanced no sim time cooks nothing.
        d.set_document(&elems);
        assert!(d.tick(&[hot], 0.0).is_empty());
        assert!(d.parts().is_empty());
    }

    #[test]
    fn repair_clears_broken_and_zeroes_stress() {
        let elems = vec![ElementSpec::two(
            7,
            ElementKind::Resistor { ohms: 100.0 },
            (0, 0),
            (4, 0),
        )];
        let mut d = DamageModel::new();
        d.set_document(&elems);
        assert!(!d.repair(7), "nothing to repair yet");
        d.tick(&[frame(7, 5.0, 0.0, (0.0, 0.0))], 1.0);
        assert!(d.is_broken(7));
        assert_eq!(d.stress(7), 1.0);
        assert!(d.repair(7));
        assert!(!d.is_broken(7));
        assert_eq!(d.stress(7), 0.0, "a repaired part starts cold");
        assert!(!d.repair(7), "and repairing it twice is a no-op");
        assert!(!d.repair(12_345), "unknown ids are ignored");
    }

    #[test]
    fn the_report_puts_dead_parts_first_and_respects_the_cap() {
        let mut d = DamageModel::default();
        for id in 1..=8 {
            d.force_break(id * 2);
        }
        for id in 1..=8 {
            d.parts.push(Part {
                id: 100 + id,
                stress: 0.5,
                broken: false,
            });
        }
        d.parts.sort_by_key(|p| p.id);
        let r = d.report(10);
        assert_eq!(r.len(), 10);
        assert!(r[..8].iter().all(|row| row[2] == 1.0), "dead first");
        assert!(r[8..].iter().all(|row| row[2] == 0.0));
        // Stress is rounded to three decimals on the wire.
        let mut d = DamageModel::default();
        d.parts.push(Part {
            id: 1,
            stress: 0.123_456_7,
            broken: false,
        });
        assert_eq!(d.report(4)[0][1], 0.123);
    }
}
