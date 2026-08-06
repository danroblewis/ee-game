//! END-TO-END ROOM TESTS: does this room actually work?
//!
//! Everything else in the suite tests a PART. This tests a ROOM — the thing a
//! player opens — and it exists because the two are not the same claim. Every
//! device in the TR-808 had passing tests on the day the room was silent,
//! because the fault was in the wiring, and nothing in the suite ever built
//! the wiring and listened to it.
//!
//! What it can do:
//!
//! * load a room from a TEMPLATE (what a new room is made of) or from a SAVE
//!   FILE on disk (what a player actually has), so a regression in either is
//!   caught by the same harness;
//! * assert the placement gate accepts it, that it never quarantines, and
//!   that its speakers make sound;
//! * FLIP SWITCHES AND TURN KNOBS mid-run and assert on what changed, which
//!   is the half that matters — a room that is only ever observed at rest is
//!   a room whose controls are untested.
//!
//! The blanket test at the bottom sweeps every built-in template. That is the
//! net: any new template gets covered the moment it is registered, without
//! anybody remembering to write a test for it.

#![cfg(test)]

use crate::templates::BUILTINS;
use sim_core::{ElementKind as K, ElementSpec, Engine, InteractOp};

/// The room dt the server itself runs at. Rooms are tested on the grid they
/// actually run on — a room that only behaves at a finer dt is not a room
/// that works.
const DT: f64 = crate::DT;

/// A room under test.
pub struct Room {
    pub elements: Vec<ElementSpec>,
    eng: Engine,
    /// Where the room came from, for assertion messages that name the thing
    /// that broke rather than "the room".
    what: String,
}

impl Room {
    /// Build from a registered template id — `"tr-808"`, `"hoist"`, and so on.
    pub fn template(id: &str) -> Room {
        let b = BUILTINS
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("no template {id:?}"));
        Room::new((b.build)().elements, format!("template {id:?}"))
    }

    /// Build from a saved room file — the JSON under `rooms/`.
    ///
    /// This is the case the per-part tests can never cover: a room a PLAYER
    /// built, with whatever they wired, loaded exactly as the server loads it.
    pub fn file(path: &str) -> Room {
        let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let elements: Vec<ElementSpec> = serde_json::from_value(v["elements"].clone())
            .unwrap_or_else(|e| panic!("{path}: elements did not parse: {e}"));
        Room::new(elements, format!("room file {path:?}"))
    }

    fn new(elements: Vec<ElementSpec>, what: String) -> Room {
        let mut eng = Engine::new(DT);
        eng.set_elements(&elements);
        Room {
            elements,
            eng,
            what,
        }
    }

    /// The placement gate must accept it. A room that cannot be gated is a
    /// room a player cannot edit without being refused.
    pub fn gate_ok(&self) -> &Self {
        assert_eq!(
            sim_core::check_document(&self.elements, DT),
            Ok(()),
            "{}: the placement gate refuses this room",
            self.what
        );
        self
    }

    /// Run for `secs` of SIM time.
    pub fn run(&mut self, secs: f64) -> &mut Self {
        let steps = (secs / DT).round() as u32;
        // In chunks, so a room that quarantines is caught at the moment it
        // does rather than after the whole run.
        let chunk = 2_000;
        let mut done = 0;
        while done < steps {
            self.eng.advance(chunk.min(steps - done));
            done += chunk;
            assert!(
                !self.eng.is_quarantined(),
                "{}: quarantined after {:.3} s",
                self.what,
                self.eng.time()
            );
        }
        self
    }

    /// Peak-to-peak swing across every speaker, over `secs`, largest first.
    pub fn speaker_swing(&mut self, secs: f64) -> Vec<(u32, f64)> {
        let ids: Vec<u32> = self
            .elements
            .iter()
            .filter(|e| matches!(e.kind, K::Speaker { .. }))
            .map(|e| e.id)
            .collect();
        let taps: Vec<(u32, _)> = ids
            .iter()
            .filter_map(|id| self.eng.tap(*id).map(|t| (*id, t)))
            .collect();
        let mut hi = vec![f64::MIN; taps.len()];
        let mut lo = vec![f64::MAX; taps.len()];
        for _ in 0..(secs / DT) as u32 {
            self.eng.advance(1);
            for (k, (_, t)) in taps.iter().enumerate() {
                let v = self.eng.tap_delta(*t, 0, 1);
                hi[k] = hi[k].max(v);
                lo[k] = lo[k].min(v);
            }
        }
        let mut out: Vec<(u32, f64)> = taps
            .iter()
            .enumerate()
            .map(|(k, (id, _))| (*id, hi[k] - lo[k]))
            .collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        out
    }

    /// A time-invariant summary of what the loudest speaker is doing:
    /// (rms, zero crossings per second).
    ///
    /// Peak-to-peak is NOT enough to tell whether a control does anything.
    /// The 555-VCO's output sits on its op-amp's 25 mA current limit, so its
    /// swing is pinned at 0.4 V whatever the knobs do — while the knobs are
    /// changing the PITCH, which peak-to-peak cannot see. Crossing rate can.
    /// And comparing raw samples cannot work either: the waveform is
    /// periodic and the two captures are at different times.
    pub fn speaker_character(&mut self, secs: f64) -> (f64, f64) {
        let id = match self
            .elements
            .iter()
            .find(|e| matches!(e.kind, K::Speaker { .. }))
            .map(|e| e.id)
        {
            Some(id) => id,
            None => return (0.0, 0.0),
        };
        let Some(tap) = self.eng.tap(id) else {
            return (0.0, 0.0);
        };
        let n = (secs / DT) as u32;
        let (mut sum, mut cross, mut prev) = (0.0f64, 0u32, 0.0f64);
        for k in 0..n {
            self.eng.advance(1);
            let v = self.eng.tap_delta(tap, 0, 1);
            sum += v * v;
            if k > 0 && (v > 0.0) != (prev > 0.0) {
                cross += 1;
            }
            prev = v;
        }
        ((sum / f64::from(n)).sqrt(), f64::from(cross) / secs)
    }

    /// Voltage at a grid point.
    pub fn volts(&self, at: (i32, i32)) -> f64 {
        self.eng
            .voltage_at(at)
            .unwrap_or_else(|| panic!("{}: nothing at {at:?}", self.what))
    }

    /// Flip a switch or press a button, exactly as a player's click does —
    /// through `InteractOp`, so what is tested is the path the client uses.
    pub fn set_switch(&mut self, id: u32, closed: bool) -> &mut Self {
        self.apply(id, InteractOp::SetSwitch { closed });
        self
    }

    /// Turn a knob. `frac` is the wiper position, 0..1.
    pub fn set_pot(&mut self, id: u32, frac: f64) -> &mut Self {
        self.apply(id, InteractOp::SetValue { value: frac });
        self
    }

    /// Mirror an interaction into the document AND the engine, the way the
    /// tick does — including the gate, so a test cannot set up a state the
    /// server would have refused.
    fn apply(&mut self, id: u32, op: InteractOp) {
        let mut next = self.elements.clone();
        crate::apply_interact_to(&mut next, id, op);
        assert_eq!(
            sim_core::check_document(&next, DT),
            Ok(()),
            "{}: the gate refuses {op:?} on #{id}",
            self.what
        );
        self.eng.interact(id, op);
        self.elements = next;
    }

    /// Every switch/button id in the room, for tests that want to exercise
    /// all of them without hard-coding a list that will rot.
    pub fn switches(&self) -> Vec<u32> {
        self.elements
            .iter()
            .filter(|e| matches!(e.kind, K::Switch { .. } | K::Button { .. }))
            .map(|e| e.id)
            .collect()
    }

    /// The switch (or button) with a terminal ON a given grid point.
    ///
    /// A test that says "the SER switch" wants to name it the way the sheet
    /// does — by where it is — not by an id that shifts the moment somebody
    /// inserts a resistor earlier in the room. Returns None rather than
    /// panicking, so a test can assert its absence too.
    pub fn switch_at(&self, at: (i32, i32)) -> Option<u32> {
        self.elements
            .iter()
            .find(|e| {
                matches!(e.kind, K::Switch { .. } | K::Button { .. }) && e.pins.contains(&at)
            })
            .map(|e| e.id)
    }

    /// Delete every WIRE with a terminal on a point — "what if this had never
    /// been connected?".
    ///
    /// This is how a room proves that one of its wires is load-bearing.
    /// Grounding the pin instead would test something else entirely: a pin a
    /// room deliberately ties to the supply is SHORTED by a ground symbol,
    /// and the placement gate refuses that before the solver ever sees it —
    /// correctly, and uselessly for the question being asked. Only wires are
    /// removed, so the part on the other end stays in the room.
    pub fn cut(&mut self, at: (i32, i32)) -> &mut Self {
        let before = self.elements.len();
        self.elements
            .retain(|e| !(matches!(e.kind, K::Wire) && e.pins.contains(&at)));
        assert!(
            self.elements.len() < before,
            "{}: no wire to cut at {at:?}",
            self.what
        );
        self.eng.set_elements(&self.elements);
        self
    }

    pub fn pots(&self) -> Vec<u32> {
        self.elements
            .iter()
            .filter(|e| matches!(e.kind, K::Potentiometer { .. }))
            .map(|e| e.id)
            .collect()
    }
}

// ---------------------------------------------------------------- the sweep

/// EVERY BUILT-IN TEMPLATE, gated and run.
///
/// This is the net. A template registered tomorrow is covered tomorrow, with
/// nobody remembering to write anything — which is the only kind of coverage
/// that survives a busy week.
#[test]
fn every_template_is_solvable_and_stays_solvable() {
    for b in BUILTINS {
        let mut r = Room::template(b.id);
        r.gate_ok();
        // Two seconds: long enough for RC settling, for a 555 to cycle, and
        // for the slowest shipped sequencer to reach its second step.
        r.run(2.0);
        println!("  ok  {:<22} {} parts", b.id, r.elements.len());
    }
}

/// Every template that HAS a speaker must actually drive it. A synth room
/// that solves cleanly and makes no sound is broken in the way that matters
/// most, and is exactly the failure that shipped in D4SWHQ.
#[test]
fn every_template_with_a_speaker_makes_sound() {
    for b in BUILTINS {
        let mut r = Room::template(b.id);
        let has_speaker = r.elements.iter().any(|e| matches!(e.kind, K::Speaker { .. }));
        if !has_speaker {
            continue;
        }
        r.run(1.0);
        let mut best = r.speaker_swing(0.5).first().map(|x| x.1).unwrap_or(0.0);
        // A room may legitimately be quiet AT REST — a showcase bench whose
        // speaker sits behind an open switch is not broken, it is waiting.
        // So if nothing is playing, close everything and ask again. That is
        // also a better test than the one it replaces: it exercises the
        // switches instead of only observing the room standing still.
        let mut how = "at rest";
        if best <= 1e-3 {
            for id in r.switches() {
                r.set_switch(id, true);
            }
            r.run(1.0);
            best = r.speaker_swing(0.5).first().map(|x| x.1).unwrap_or(0.0);
            how = "with every switch closed";
        }
        println!("  {:<22} loudest speaker {:.4} V p-p ({how})", b.id, best);
        assert!(
            best > 1e-3,
            "{}: has a speaker but drives it with only {best:.6} V {how} — silent room",
            b.id
        );
    }
}

/// THE CONTROLS MOVE SOMETHING. A knob that is wired to nothing, or a switch
/// whose branch never reaches the circuit, passes every other test in this
/// repo: the room still solves, still makes sound, still looks right. The
/// only way to catch it is to turn the thing and watch the output change.
#[test]
fn the_synth_knobs_and_switches_do_something() {
    for id in ["tr-808", "the-ladder", "vco-555", "bass-plus-plus", "synth"] {
        let mut r = Room::template(id);
        r.run(0.5);
        let (r0, c0) = r.speaker_character(0.3);

        // Every pot to one end, then re-measure. At least one of them has to
        // matter, or the room's controls are decoration.
        let pots = r.pots();
        for p in &pots {
            r.set_pot(*p, 0.05);
        }
        r.run(0.5);
        let (r1, c1) = r.speaker_character(0.3);
        println!(
            "  {id:<16} {} pots: rms {r0:.4}->{r1:.4}  crossings/s {c0:.0}->{c1:.0}",
            pots.len()
        );
        if pots.is_empty() {
            continue;
        }
        let loud = (r1 - r0).abs() > r0.max(1e-9) * 0.02;
        let pitch = (c1 - c0).abs() > c0.max(1.0) * 0.02;
        assert!(
            loud || pitch,
            "{id}: driving all {} pots to 0.05 moved neither the level \
             ({r0:.5} -> {r1:.5}) nor the pitch ({c0:.0} -> {c1:.0} crossings/s) \
             — are they connected to anything?",
            pots.len()
        );
    }
}

/// A room that a PLAYER saved must load and run. Skipped when the directory
/// is absent, because a checkout has no rooms until a server has run.
#[test]
fn saved_rooms_on_disk_still_load_and_run() {
    let dir = std::path::Path::new("../../rooms");
    if !dir.is_dir() {
        println!("  no rooms/ directory — skipping");
        return;
    }
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let path = p.to_string_lossy().to_string();
        let mut r = Room::file(&path);
        if r.elements.is_empty() {
            continue; // an empty sandbox is a legal room with nothing to test
        }
        r.gate_ok();
        r.run(0.5);
        n += 1;
        println!("  ok  {:<28} {} parts", entry.file_name().to_string_lossy(), r.elements.len());
    }
    println!("  {n} saved rooms checked");
}
