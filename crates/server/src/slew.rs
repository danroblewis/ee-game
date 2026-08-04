//! A knob that arrives instead of jumping.
//!
//! ## The fault
//!
//! A player dragging a pot sends one `SetValue` per pointer sample. The tick
//! coalesces those to ONE (`supersede`), and the tick is 30 Hz — so however
//! smoothly a hand moves, the wiper the solver sees is a staircase with a
//! 33 ms tread. Everything downstream inherits that. In a DC circuit it is
//! invisible; in an AUDIO circuit each tread is a step discontinuity in the
//! speaker waveform 30 times a second, which is the click-per-tread the
//! owner heard as "jagged". It is the classic zipper: the artefact is not
//! the pot, it is the sample-and-hold in front of it.
//!
//! The staircase is also just WRONG. A real wiper is a physical object
//! sliding along a track; it passes through every value between where it
//! was and where it is going, and it takes real time to do it. Nothing in
//! the world jumps 8% of a track in zero seconds. This is the same standard
//! the rest of the project holds itself to — the solver is honest, so the
//! thing feeding it should be too.
//!
//! ## The fix
//!
//! The tick already subdivides itself: the machine co-simulates every
//! `MACHINE_SUBSTEPS` (640 µs), and probes and speakers sample finer still.
//! Those sub-instants are already being visited. A wiper walking toward its
//! target on the machine cadence gets **52 intermediate positions inside a
//! tick that previously had one**, at the cost of one refactorisation each
//! — the identical cost the machine co-simulation already pays, and pays
//! only while a knob is actually moving.
//!
//! The walk is a one-pole approach with a 20 ms time constant, which is
//! what every synthesiser does to a control voltage and for exactly this
//! reason. It is not merely an anti-click: it is closer to the truth. A
//! finger on a knob has mass and the wiper has friction, so the mechanical
//! step response of the real thing is a lag of about this order.
//!
//! ## Why the document still holds the target
//!
//! The document stores where the player asked the knob to BE; the engine
//! holds where it currently IS. They differ for ~60 ms after a drag stops
//! and are identical at every other moment. This split is deliberate:
//!
//! * the placement gate judges the TARGET, so what is validated is the
//!   value the player actually asked for;
//! * a save file records the target, so reloading a room puts the knob
//!   exactly where it was left rather than mid-flight;
//! * other players' panels echo the target, so a knob reads the same on
//!   every screen while it travels.
//!
//! ## Why this cannot be less safe than the staircase it replaces
//!
//! Every intermediate value lies strictly between the old value and the new
//! one, and the player was ALREADY sweeping through that interval — just in
//! one 33 ms leap instead of 52 steps. The slew only makes the path finer,
//! never wider. It cannot reach a wiper position that the jump did not
//! already pass through, so it cannot reach a state the jump could not.

use sim_core::{Engine, ParamWrite};

/// How long the wiper takes to cover ~63% of the distance to its target.
///
/// 20 ms is chosen against the 33 ms tick: over one tick the wiper covers
/// 81% of the remaining gap, so it TRACKS a continuous drag with a lag far
/// below the ~100 ms at which a control starts to feel disconnected, while
/// still being long enough that the residual steps land well under any
/// audible click. Shorter and the treads come back; much longer and the
/// knob feels like it is dragging behind the hand.
pub const WIPER_TAU: f64 = 0.020;

/// Stop when the remaining gap is under this. The wiper is clamped to
/// 0.01..0.99, so this is one part in ten thousand of the track — below
/// what a panel prints (whole percent) and far below what the ear resolves.
/// Without a floor, a one-pole approach never formally arrives and every
/// touched knob would refactor its island forever.
const EPS: f64 = 1e-4;

/// One knob in flight.
#[derive(Clone, Copy, Debug)]
struct InFlight {
    id: u32,
    /// Where the wiper actually is — the value the engine is holding.
    now: f64,
    /// Where the player asked it to go. Matches the document.
    target: f64,
}

/// The knobs currently travelling.
///
/// A `Vec`, not a map: this is empty in a still room, and holds one entry
/// per knob a hand is on — which is one, or a few on a gamepad. Linear scan
/// of a 1-element vector beats hashing, and it keeps the iteration order
/// fixed, which keeps a tick's writes in a deterministic sequence.
#[derive(Default, Debug)]
pub struct Slews {
    live: Vec<InFlight>,
}

impl Slews {
    /// Nothing in flight — the caller can skip the whole cadence.
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Point a knob at a new target.
    ///
    /// `from` is the wiper's value in the document BEFORE this write, used
    /// only when the knob is not already travelling; a knob that IS
    /// travelling keeps its current position and merely re-aims, which is
    /// what makes a continuous drag one smooth movement rather than 30
    /// little ones a second.
    pub fn aim(&mut self, id: u32, from: f64, target: f64) {
        if let Some(f) = self.live.iter_mut().find(|f| f.id == id) {
            f.target = target;
            return;
        }
        // Already there: a repeated write of the same value (a player
        // holding a knob still, a gamepad axis at rest) starts nothing.
        if (from - target).abs() < EPS {
            return;
        }
        self.live.push(InFlight {
            id,
            now: from,
            target,
        });
    }

    /// Hold a travelling knob at its CURRENT position after something
    /// recompiled the engine from the document.
    ///
    /// `Engine::interact` rebuilds from the document, and the document holds
    /// the target — so any compile snaps a travelling wiper to its
    /// destination, which is the exact jump this module exists to remove.
    /// Calling this straight after one puts it back where it was.
    pub fn reassert(&mut self, eng: &mut Engine, id: u32) {
        if let Some(f) = self.live.iter().find(|f| f.id == id) {
            eng.write_param(f.id, ParamWrite::Wiper { frac: f.now });
        }
    }

    /// Advance every travelling knob by `h` seconds of SIM time and write
    /// the new positions into the engine.
    ///
    /// Sim time, not wall time, is the right clock: a heavy room dilates
    /// both the audio and the tick, and a knob that swept in wall time
    /// would travel at a different rate than the sound it is shaping.
    ///
    /// A knob whose part has gone (deleted mid-drag) fails its write and is
    /// dropped rather than retried forever.
    pub fn advance(&mut self, eng: &mut Engine, h: f64) {
        if self.live.is_empty() {
            return;
        }
        // 1 - e^(-h/tau): the fraction of the remaining gap closed in h.
        let a = 1.0 - (-h / WIPER_TAU).exp();
        self.live.retain_mut(|f| {
            let gap = f.target - f.now;
            let arrived = gap.abs() <= EPS;
            f.now = if arrived { f.target } else { f.now + gap * a };
            // The write is what makes it real; a failed write means the
            // part is gone. Note the ARRIVED knob is still written once,
            // so a knob always finishes exactly on its target rather than
            // an epsilon short of it.
            let alive = eng.write_param(f.id, ParamWrite::Wiper { frac: f.now });
            alive && !arrived
        });
    }

    /// Abandon every journey, leaving each wiper wherever the engine now
    /// has it.
    ///
    /// Called after a DOCUMENT EDIT, which rebuilds the engine from a
    /// document that holds targets — so every travelling knob has just been
    /// snapped to its destination. Continuing to walk from `now` after that
    /// would write the wiper BACKWARDS, turning a harmless edit into an
    /// audible lurch in the wrong direction. Forgetting instead costs one
    /// 33 ms jump on the rare tick where someone edits the circuit with a
    /// hand still on a knob, which is exactly what happened before this
    /// module existed. The next `SetValue` of the drag aims afresh from
    /// there and the walk resumes.
    pub fn clear(&mut self) {
        self.live.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DT, MACHINE_H};
    use sim_core::{ElementKind as K, ElementSpec, Point};

    fn spec(id: u32, kind: K, pins: Vec<Point>) -> ElementSpec {
        ElementSpec {
            id,
            kind,
            pins,
            ..Default::default()
        }
    }

    /// A 5 V rail across a pot, the wiper feeding a load: the load's voltage
    /// is a direct, MONOTONE function of the wiper and nothing else, so every
    /// wiggle in the tap is the wiper's doing and only the wiper's. That is
    /// why this is DC — put an oscillator in the loop and the measurement
    /// would be dominated by the waveform rather than the knob.
    ///
    /// Pot pins are [end, WIPER, end]: `r1 = ohms·wiper` spans 0-1 and
    /// `r2 = ohms·(1-wiper)` spans 1-2.
    fn divider() -> Vec<ElementSpec> {
        vec![
            spec(
                1,
                K::Rail {
                    dc: 5.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                    wave: sim_core::Wave::Sine,
                },
                vec![(0, 0)],
            ),
            spec(
                2,
                K::Potentiometer {
                    ohms: 10_000.0,
                    wiper: 0.1,
                },
                vec![(0, 0), (8, 0), (16, 0)],
            ),
            spec(3, K::Resistor { ohms: 100_000.0 }, vec![(8, 0), (8, 8)]),
            spec(4, K::Ground, vec![(8, 8)]),
            spec(5, K::Ground, vec![(16, 0)]),
        ]
    }

    /// Sweep the wiper 0.1 -> 0.9 across 15 ticks and return the speaker
    /// drive at the audio cadence. `smooth` picks the mechanism under test.
    ///
    /// Both arms see the IDENTICAL sequence of 30 Hz targets — the only
    /// difference is whether the wiper teleports to each one or walks.
    fn sweep(smooth: bool) -> Vec<f64> {
        const TICKS: usize = 15;
        const AUDIO_EVERY: u32 = 4;
        // One tick of sim time, in whole machine periods.
        let per_tick = ((1.0 / 30.0) / DT) as u32;
        let machine_periods = per_tick / 32;
        let chunks_per_machine = 32 / AUDIO_EVERY;

        let mut eng = Engine::new(DT);
        let mut elems = divider();
        eng.set_elements(&elems);
        let tap = eng.tap(3).expect("speaker tap");
        let mut slews = Slews::default();
        let mut out = Vec::new();

        // The sweep, then SETTLE ticks with the hand off the knob. Those
        // matter: a slewed wiper is still ~19% short of the last target at
        // the instant the drag stops, and the honest claim is that it
        // arrives shortly after — not that it was never behind.
        const SETTLE: usize = 4;
        for t in 0..TICKS + SETTLE {
            let target = 0.1 + 0.8 * ((t + 1).min(TICKS) as f64) / TICKS as f64;
            // Exactly what the tick does: mirror into the document, then
            // recompile the engine from it.
            let from = match elems[1].kind {
                K::Potentiometer { wiper, .. } => wiper,
                _ => unreachable!(),
            };
            if let K::Potentiometer { ref mut wiper, .. } = elems[1].kind {
                *wiper = target;
            }
            eng.set_elements(&elems);
            if smooth {
                slews.aim(2, from, target);
                slews.reassert(&mut eng, 2);
            }
            for _ in 0..machine_periods {
                for _ in 0..chunks_per_machine {
                    eng.advance(AUDIO_EVERY);
                    out.push(eng.tap_delta(tap, 0, 1));
                }
                if smooth {
                    slews.advance(&mut eng, MACHINE_H);
                }
            }
        }
        out
    }

    /// The largest jump between consecutive audio samples. This IS the
    /// click: a step in the speaker's drive is a step in the cone, and the
    /// ear hears the discontinuity, not the value.
    fn worst_step(v: &[f64]) -> f64 {
        v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max)
    }

    /// THE MEASUREMENT. Not "it looks smoother" — a number, from the tap the
    /// speaker actually plays, on both mechanisms driven by the same targets.
    #[test]
    fn slewing_a_wiper_removes_the_staircase() {
        let jumpy = sweep(false);
        let smooth = sweep(true);

        let a = worst_step(&jumpy);
        let b = worst_step(&smooth);
        println!("worst step per audio sample: staircase {a:.6} V, slewed {b:.6} V ({:.1}x)", a / b);
        assert!(
            b * 8.0 < a,
            "slewed sweep should be at least 8x smoother: staircase {a:.6} V/sample, \
             slewed {b:.6} V/sample"
        );

        // Both must actually GET there — a smoother sweep that does not
        // arrive is not a fix, it is a broken knob. The last sample is the
        // wiper at 0.9 either way.
        let end_a = *jumpy.last().unwrap();
        let end_b = *smooth.last().unwrap();
        assert!(
            (end_a - end_b).abs() < 1e-3,
            "slewed sweep must end where the jumpy one does: {end_a:.6} vs {end_b:.6}"
        );

        // And it must be MONOTONE, in whichever direction this divider runs.
        // A one-pole cannot overshoot; if this ever fires, the coefficient
        // has gone past 1 and the "smoothing" is ringing — which would be
        // audibly worse than the staircase it replaced.
        let dir = (smooth.last().unwrap() - smooth.first().unwrap()).signum();
        let back = smooth
            .windows(2)
            .filter(|w| (w[1] - w[0]) * dir < -1e-9)
            .count();
        assert_eq!(back, 0, "a slewed wiper must never reverse: {back} reversals");
    }

    /// A knob that is re-aimed mid-flight keeps travelling from where it
    /// is. This is the whole of a continuous drag: 30 re-aims a second,
    /// each landing on a wiper that has not yet reached the last one.
    #[test]
    fn re_aiming_mid_flight_does_not_restart_the_journey() {
        let mut eng = Engine::new(DT);
        let mut elems = divider();
        eng.set_elements(&elems);
        let mut s = Slews::default();

        s.aim(2, 0.1, 0.9);
        s.advance(&mut eng, MACHINE_H);
        let after_one = s.live[0].now;
        assert!(after_one > 0.1 && after_one < 0.2, "moved a little: {after_one}");

        // Re-aim somewhere else; the wiper must continue from `after_one`,
        // not snap back to 0.1 or jump to the new target.
        s.aim(2, 0.1, 0.5);
        assert_eq!(s.live[0].now, after_one, "re-aim must not move the wiper");
        assert_eq!(s.live[0].target, 0.5, "re-aim must change the target");

        // Let it run and it arrives, then retires itself.
        for _ in 0..4000 {
            s.advance(&mut eng, MACHINE_H);
        }
        assert!(s.is_empty(), "an arrived knob must stop costing anything");
        if let K::Potentiometer { ref mut wiper, .. } = elems[1].kind {
            *wiper = 0.5;
        }
        let _ = elems;
    }

    /// A still room must cost exactly nothing. `advance` on an empty set
    /// touches no engine state, so a room full of pots nobody is holding
    /// refactors nothing.
    #[test]
    fn a_knob_nobody_is_touching_costs_nothing() {
        let mut s = Slews::default();
        assert!(s.is_empty());
        // A write that does not move the knob starts no journey.
        s.aim(7, 0.42, 0.42);
        assert!(s.is_empty(), "a no-op write must not start a slew");
    }

    /// A part deleted mid-drag must drop out rather than be written forever.
    #[test]
    fn a_knob_that_stops_existing_is_forgotten() {
        let mut eng = Engine::new(DT);
        eng.set_elements(&divider());
        let mut s = Slews::default();
        s.aim(2, 0.1, 0.9);
        // id 999 is not in the document at all: its write fails.
        s.aim(999, 0.1, 0.9);
        s.advance(&mut eng, MACHINE_H);
        assert_eq!(s.live.len(), 1, "the missing part should have been dropped");
        assert_eq!(s.live[0].id, 2);
    }
}
