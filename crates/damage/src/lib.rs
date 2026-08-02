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
// PER-KIND DEFAULTS — the single source of truth for what parts can take.
// One line per kind, chosen to match EE intuition rather than any particular
// datasheet, and to make the classic mistakes teach the classic lessons.
//
// | kind                | metric  | limit          | tau   | why
// |---------------------|---------|----------------|-------|--------------------------------
// | Resistor            | power   | 0.5 W          | 6 s   | a half-watt film resistor; 2x
// |                     |         |                |       | rated cooks it in 4.2 s
// | Lamp                | power   | 2 × rated_watts| 4 s   | rated_watts is the DESIGN point
// |                     |         |                |       | of a bulb, not its limit; a
// |                     |         |                |       | filament at 2× rated dies fast
// | Speaker             | power   | 0.5 W          | 4 s   | small 8 Ω driver, voice coil
// | Potentiometer       | power   | 0.5 W          | 8 s   | panel pot; the wiper track cooks
// | Capacitor           | voltage | 25 V           | 2 s   | electrolytic — over-volt it and
// |                     |         |                |       | it vents, fast and loudly
// | Inductor            | current | 1 A            | 5 s   | small choke: winding fuses
// | Motor               | current | 3 A            | 6 s   | 3 A continuous. A 12 V supply
// |                     |         |                |       | straight across the hoist motor
// |                     |         |                |       | (R = 2 Ω) stalls it at 6 A and
// |                     |         |                |       | cooks the armature in ~2 s;
// |                     |         |                |       | its ~0.94 A running current and
// |                     |         |                |       | a controlled drive's chatter
// |                     |         |                |       | both sit far below the limit
// | Switch / Button     | current | 5 A            | 3 s   | contacts weld and burn
// | Diode               | current | 1 A            | 1 s   | 1N400x class
// | Zener               | current | 0.2 A          | 1 s   | half-watt zener at ~5 V
// | Led                 | current | 40 mA          | 0.35 s| 20 mA nominal, 40 mA absolute
// |                     |         |                |       | max, and THE first lesson: no
// |                     |         |                |       | series resistor = instant death
// | Npn / Pnp           | power   | 0.625 W        | 2 s   | TO-92 small-signal BJT
// | Nmos / Pmos         | power   | 1 W            | 2 s   | small power FET, no heatsink
// | Timer555            | power   | 0.6 W          | 3 s   | bipolar 555 in a DIP: VCC and
// |                     |         |                |       | GND are modelled, so the
// |                     |         |                |       | frame's power really is the
// |                     |         |                |       | chip's own dissipation
// | OpAmp, Ota          | —       | —              | —     | NOT breakable yet: no supply
// |                     |         |                |       | pins in the model, so their
// |                     |         |                |       | frame power is what they
// |                     |         |                |       | deliver, not what they burn
// | VoltageSource       | current | 10 A           | 5 s   | bench supply / battery pack:
// | CurrentSource       | current | 10 A           | 5 s   | generous, so the LOAD dies first
// | Rail                | current | 10 A           | 5 s   | same class as VoltageSource
// | Wire, Ground        | —       | —              | —     | not breakable in this pass
//
// The metric column also picks the heat law (see `heat`): power-rated parts
// heat in proportion to the load, current- and voltage-rated ones as its
// square.
//
// Resistors, pots and speakers carry no rating field in the document yet, so
// they use the per-kind default; giving them a nameplate (like Lamp's
// `rated_watts`) is a later pass, and this table is the one place to change.
/// The safety limit for a part kind, or `None` for parts that cannot break.
pub fn rating(kind: &ElementKind) -> Option<Rating> {
    use ElementKind as K;
    use Metric::{Current, Power, Voltage};
    Some(match kind {
        K::Wire | K::Ground => return None,
        K::Resistor { .. } => Rating::new(Power, 0.5, 6.0),
        // A bulb is MEANT to run at rated_watts, so its failure limit sits
        // above it. Guard the multiply: a zero/absurd nameplate must not make
        // an unbreakable (or instantly broken) lamp.
        K::Lamp { rated_watts, .. } => {
            Rating::new(Power, 2.0 * rated_watts.clamp(0.01, 1.0e6), 4.0)
        }
        K::Speaker { .. } => Rating::new(Power, 0.5, 4.0),
        K::Potentiometer { .. } => Rating::new(Power, 0.5, 8.0),
        K::Capacitor { .. } => Rating::new(Voltage, 25.0, 2.0),
        K::Inductor { .. } => Rating::new(Current, 1.0, 5.0),
        K::Motor { .. } => Rating::new(Current, 3.0, 6.0),
        K::Switch { .. } | K::Button { .. } => Rating::new(Current, 5.0, 3.0),
        K::Diode => Rating::new(Current, 1.0, 1.0),
        K::Zener { .. } => Rating::new(Current, 0.2, 1.0),
        K::Led { .. } => Rating::new(Current, 0.04, 0.35),
        K::Npn { .. } | K::Pnp { .. } => Rating::new(Power, 0.625, 2.0),
        K::Nmos { .. } | K::Pmos { .. } => Rating::new(Power, 1.0, 2.0),
        // Op-amps and OTAs are deliberately NOT breakable yet: their models
        // have no supply terminals, so `ElemFrame::power` is what they
        // DELIVER, not what they dissipate. Breaking a part on a number that
        // is not its own dissipation would be inventing physics, which is
        // the one thing this codebase does not do. They become breakable in
        // the pass that gives them rails. The 555 is different — its VCC and
        // GND pins are modelled, so the frame's power really is the chip's.
        K::OpAmp { .. } | K::Ota => return None,
        K::Timer555 => Rating::new(Power, 0.6, 3.0),
        K::VoltageSource { .. } | K::CurrentSource { .. } | K::Rail { .. } | K::Noise { .. } => {
            Rating::new(Current, 10.0, 5.0)
        }
    })
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
            if let Some(rating) = rating(&e.kind) {
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
    /// quarantined or budget-limited tick advances less, and must therefore
    /// cook less). Callers pass 0 for a tick that produced no new numbers.
    pub fn tick(&mut self, frames: &[ElemFrame], h: f64) -> Vec<Broke> {
        let mut broke = Vec::new();
        if !(h.is_finite() && h > 0.0) || self.rated.is_empty() {
            return broke;
        }
        for f in frames {
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
        ];
        for k in kinds {
            let r = rating(&k).unwrap_or_else(|| panic!("{} has no rating", kind_name(&k)));
            assert!(r.limit > 0.0 && r.limit.is_finite(), "{:?}", r);
            assert!(r.tau > 0.0 && r.tau.is_finite(), "{:?}", r);
            assert_ne!(kind_name(&k), "");
        }
        // Conductors do not break in this pass, and neither do the two
        // chips whose dissipation the model cannot see (no supply pins).
        assert!(rating(&K::Wire).is_none());
        assert!(rating(&K::Ground).is_none());
        assert!(rating(&K::OpAmp { rail: 12.0 }).is_none());
        assert!(rating(&K::Ota).is_none());
        // A nonsense lamp nameplate cannot produce a nonsense limit.
        assert!(
            rating(&K::Lamp {
                ohms: 90.0,
                rated_watts: 0.0
            })
            .unwrap()
            .limit
                > 0.0
        );
    }

    #[test]
    fn at_or_under_the_limit_a_part_lives_forever() {
        let r = rating(&ElementKind::Resistor { ohms: 100.0 }).unwrap();
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
        let led = rating(&ElementKind::Led { color: 0 }).unwrap();
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
        let r = rating(&ElementKind::Resistor { ohms: 100.0 }).unwrap();
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
        let r = rating(&ElementKind::Led { color: 0 }).unwrap();
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
        let cap = rating(&ElementKind::Capacitor { farads: 1e-6 }).unwrap();
        assert_eq!(cap.metric, Metric::Voltage);
        assert!((load(&cap, &frame(1, 0.0, 0.0, (30.0, -20.0))) - 2.0).abs() < 1e-12);
        // Current-rated: the largest terminal current, whatever its sign.
        let mot = rating(&ElementKind::Motor {
            ohms: 2.0,
            henries: 1.5e-3,
            bemf: 0.0,
        })
        .unwrap();
        assert_eq!(mot.metric, Metric::Current);
        assert!((load(&mot, &frame(1, 0.0, -6.0, (12.0, 0.0))) - 2.0).abs() < 1e-12);
        // Power-rated: sign is irrelevant, a part delivering power heats too.
        let res = rating(&ElementKind::Resistor { ohms: 1.0 }).unwrap();
        assert_eq!(res.metric, Metric::Power);
        assert!((load(&res, &frame(1, -1.0, 0.0, (0.0, 0.0))) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_model_tracks_a_document_and_prunes_cold_and_deleted_parts() {
        let elems = vec![
            ElementSpec::two(1, ElementKind::Resistor { ohms: 100.0 }, (0, 0), (4, 0)),
            ElementSpec::two(2, ElementKind::Wire, (4, 0), (8, 0)),
        ];
        let mut d = DamageModel::new();
        d.set_document(&elems);
        assert!(d.rating_of(1).is_some());
        assert!(d.rating_of(2).is_none(), "wires do not break");

        // Warm it up without breaking it, then let it cool: the entry appears
        // and then disappears again, so a quiet room costs nothing.
        let warm = frame(1, 0.45, 0.0, (0.0, 0.0));
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
