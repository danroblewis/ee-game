//! ROOM COST BENCH — the offline half of every room's real-time budget.
//!
//! `cargo test --release -p server room_realtime_budgets -- --ignored --nocapture`
//!
//! Holding 1.0x real time is a CORRECTNESS requirement for an instrument
//! room: the client rate-slaves its audio to the server's sim ratio, so a
//! room at 0.9x is a synth a semitone flat, not a laggy one. This bench
//! steps each room's circuit the way the server steps it (chunks of
//! `AUDIO_EVERY` with a probe tap on each) and reports µs/substep against
//! the 20 µs budget. The LIVE number — the server's own `rt` read off its
//! audio frames over a websocket — is the one that ships in each room's
//! scope notes; this offline one is the regression guard.

use sim_core::{ElementKind as K, ElementSpec, Engine};
use std::time::Instant;

const DT: f64 = 20e-6;

pub struct RoomCost {
    pub us_per_substep: f64,
    pub x_realtime: f64,
    pub nr_per_substep: f64,
    pub fac_per_substep: f64,
    pub devices: usize,
    pub elements: usize,
    pub unknowns: usize,
}

/// Best-of-3 timing pass over `steps` substeps, server-shaped stepping.
/// Panics if the room quarantines — a quarantined instrument is not slow,
/// it is broken.
pub fn measure(name: &str, elems: &[ElementSpec], steps: u32) -> RoomCost {
    let devices = elems
        .iter()
        .filter(|e| !matches!(e.kind, K::Wire | K::Ground))
        .count();
    let mut best: Option<RoomCost> = None;
    for _ in 0..3 {
        let mut e = Engine::new(DT);
        e.set_elements(elems);
        e.advance(5000);
        assert!(!e.is_quarantined(), "{name}: quarantined in warmup");
        let unknowns = e.unknowns();
        let f0 = e.factorizations();
        let mut nr = 0u64;
        let mut sink = 0.0f64;
        let probe = elems[0].pins[0];
        let t = Instant::now();
        for _ in 0..steps / 4 {
            let rep = e.advance(4);
            nr += u64::from(rep.nr_iters);
            sink += e.voltage_at(probe).unwrap_or(0.0);
        }
        let el = t.elapsed().as_secs_f64();
        assert!(!e.is_quarantined(), "{name}: quarantined during run");
        std::hint::black_box(sink);
        let m = RoomCost {
            us_per_substep: el / f64::from(steps) * 1e6,
            x_realtime: f64::from(steps) * DT / el,
            nr_per_substep: nr as f64 / f64::from(steps),
            fac_per_substep: (e.factorizations() - f0) as f64 / f64::from(steps),
            devices,
            elements: elems.len(),
            unknowns,
        };
        if best
            .as_ref()
            .map_or(true, |b| m.us_per_substep < b.us_per_substep)
        {
            best = Some(m);
        }
    }
    let m = best.unwrap();
    println!(
        "  {name:<28} {:>3} el {:>3} dev {:>3} unk | {:>6.2} us/substep | {:>5.2}x realtime | nr {:>4.2} | fac {:>5.3}",
        m.elements, m.devices, m.unknowns, m.us_per_substep, m.x_realtime, m.nr_per_substep, m.fac_per_substep
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five instrument rooms against the 20 µs substep budget. Run in
    /// RELEASE — a debug number is meaningless — and read the µs, not just
    /// the ratio: the live server also carries websockets, damage sweeps
    /// and the audio tap.
    #[test]
    #[ignore = "timing: cargo test --release -p server room_realtime_budgets -- --ignored --nocapture"]
    fn room_realtime_budgets() {
        println!("\n== room costs at dt = 20 µs (budget: hold >1.0x SUSTAINED, live bar is rt 0.999) ==");
        let steps = 150_000;
        measure("synth (shipped reference)", &crate::synth::synth_room_circuit(), steps);
        measure("vco-555", &crate::vco555::vco555_room_circuit(), steps);
        measure("tr-808", &crate::tr808::tr808_room_circuit(), steps);
        measure("the-ladder", &crate::moog::moog_room_circuit(), steps);
        measure("bass-plus-plus", &crate::bass::bass_room_circuit(), steps);
        measure("the-scream", &crate::ms20::ms20_room_circuit(), steps);
        // Rooms land here one at a time, each measured before the next
        // begins; a missing line means the room does not exist yet.
    }

    /// EVERY instrument room reads as a schematic, and two headings on the
    /// same square are a smear rather than a diagram. This is a real defect
    /// class, not a style rule: BASS++ first laid its honesty plaque straight
    /// across the IMPACT block and the DECAY heading, and the same check then
    /// found a second overlap in its block headings that nobody had seen.
    /// Cheap to run, so it runs for all of them.
    #[test]
    fn no_instrument_room_overlaps_its_own_label_boxes() {
        let rooms: [(&str, Vec<crate::synth::PanelDef>); 5] = [
            ("vco-555", crate::vco555::vco555_label_boxes()),
            ("tr-808", crate::tr808::tr808_label_boxes()),
            ("the-ladder", crate::moog::moog_label_boxes()),
            ("bass-plus-plus", crate::bass::bass_label_boxes()),
            ("the-scream", crate::ms20::ms20_label_boxes()),
        ];
        for (room, boxes) in rooms {
            for (i, a) in boxes.iter().enumerate() {
                for b in &boxes[i + 1..] {
                    let hit = a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;
                    assert!(
                        !hit,
                        "{room}: label boxes overlap: {:?} ({}, {}, {}, {}) vs {:?} ({}, {}, {}, {})",
                        a.name, a.x0, a.y0, a.x1, a.y1, b.name, b.x0, b.y0, b.x1, b.y1
                    );
                }
            }
        }
    }
}
