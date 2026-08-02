//! The owner's circuit class, measured: a 555 astable and an op-amp Schmitt
//! relaxation oscillator sitting in a room of ~150 elements.
//!
//! This bin deliberately uses ONLY the public `Engine` API that exists on
//! both sides of the piecewise-linear factorization-reuse change, so the
//! same source file can be run against a stashed (pristine) tree to produce
//! the "before" column.
//!
//! ```text
//! cargo run --release -p sim-golden --bin pwlroom
//! ```
//!
//! Everything is 50 000 substeps at dt = 20 us — exactly 1.000 s of
//! simulated time, so `realtime` is simply 1 / wall.

use sim_core::{ElementKind, ElementSpec, Engine};
use sim_golden::scale::{self, GenParams, Structure};
use std::time::Instant;

const DT: f64 = 20e-6;
const SUBSTEPS: u32 = 50_000;

/// Move a golden circuit out of the padding generator's coordinate strips
/// (districts live at x = k * 1_000_000, y in 0..=240) and out of its id
/// space, so padding can never accidentally share a junction with it.
fn shifted(specs: &[ElementSpec], slot: u32) -> Vec<ElementSpec> {
    let dx = -2_000_000 - 1000 * slot as i32;
    specs
        .iter()
        .map(|s| ElementSpec {
            id: 900_000_000 + slot * 1000 + s.id,
            kind: s.kind,
            pins: s.pins.iter().map(|(x, y)| (x + dx, y - 4000)).collect(),
            ..Default::default()
        })
        .collect()
}

/// Linear padding shaped like a room full of other people's builds.
fn padding(elements: usize, structure: Structure) -> Vec<ElementSpec> {
    scale::generate(
        GenParams::new(elements, structure)
            .nonlinear(0)
            .active(0),
    )
    .flat()
}

/// Padding with the generator's nonlinear pool switched on: this is the
/// control that must NOT speed up, because a diode/BJT/MOSFET in the same
/// matrix makes the matrix move on every Newton pass.
fn smooth_padding(elements: usize, structure: Structure) -> Vec<ElementSpec> {
    scale::generate(
        GenParams::new(elements, structure)
            .nonlinear(100)
            .active(0),
    )
    .flat()
}

/// Median of `REPS` independent 1.0 s-simulated runs. Each rep is a fresh
/// engine, so nothing carries over; run-to-run spread on this machine is
/// ~8%, which is why a median (and not a single sample) is reported.
const REPS: usize = 5;

fn run(label: &str, specs: Vec<ElementSpec>) {
    let mut walls = Vec::with_capacity(REPS);
    let mut last = None;
    for _ in 0..REPS {
        let mut eng = Engine::new(DT);
        eng.set_elements(&specs);
        // Warm: one tick's worth, so the first-compile factorization and
        // any page faults stay out of the timed window.
        eng.advance(600);
        let f0 = eng.factorizations();
        let t = Instant::now();
        let report = eng.advance(SUBSTEPS);
        walls.push(t.elapsed().as_secs_f64());
        let cur = (
            eng.element_count(),
            eng.unknowns(),
            eng.factorizations() - f0,
            report.nr_iters,
            report.rescues,
            eng.state_hash(),
            eng.is_quarantined(),
        );
        if let Some(prev) = last {
            assert_eq!(prev, cur, "{label}: run-to-run nondeterminism");
        }
        last = Some(cur);
    }
    walls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let wall = walls[REPS / 2];
    let (elems, n, facts, nr, resc, state, q) = last.unwrap();
    let sim = SUBSTEPS as f64 * DT;
    println!(
        "{label:<28} elems={elems:<4} n={n:<4} facts={facts:<6} nr={nr:<6} resc={resc} wall={wall:.4}s realtime={:>7.3}x state=0x{state:016x} q={q}",
        sim / wall,
    );
}

fn concat(parts: Vec<Vec<ElementSpec>>) -> Vec<ElementSpec> {
    parts.into_iter().flatten().collect()
}

fn main() {
    // --- correctness column: every golden's state hash, printed exactly
    // like the determinism harness so a pristine tree and a patched tree can
    // be diffed line for line.
    for (name, elems) in sim_golden::all_golden() {
        let mut eng = Engine::new(1e-6);
        eng.set_elements(&elems);
        let report = eng.advance(10_000);
        println!(
            "GOLDEN {name:<22} 0x{:016x} steps={} q={}",
            eng.state_hash(),
            report.steps,
            eng.is_quarantined()
        );
    }

    let t555 = sim_golden::timer555_astable();
    let schmitt = sim_golden::opamp_relaxation();

    println!("\n-- bare oscillators");
    run("t555 alone", shifted(&t555, 1));
    run("schmitt alone", shifted(&schmitt, 2));

    for (sname, structure) in [
        ("districts~20", Structure::Districts { size: 20 }),
        ("one-circuit", Structure::One),
    ] {
        println!("\n-- room of ~150 elements, {sname}");
        let pad = padding(136, structure);
        run(
            &format!("t555 + pad [{sname}]"),
            concat(vec![shifted(&t555, 1), pad.clone()]),
        );
        run(
            &format!("schmitt + pad [{sname}]"),
            concat(vec![shifted(&schmitt, 2), pad.clone()]),
        );
        run(
            &format!("both + pad [{sname}]"),
            concat(vec![shifted(&t555, 1), shifted(&schmitt, 2), pad.clone()]),
        );
        run(&format!("linear pad only [{sname}]"), pad);
    }

    println!("\n-- controls (must not change)");
    // One LED in the same room: a smooth nonlinearity, so reuse is disarmed.
    let led = vec![ElementSpec::two(
        800_000_001,
        ElementKind::Led { color: 0 },
        (-3_000_000, -4000),
        (-3_000_000, -3990),
    )];
    let src = vec![
        ElementSpec::two(
            800_000_002,
            ElementKind::VoltageSource {
                dc: 9.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
            },
            (-3_000_000, -3990),
            (-3_000_010, -3990),
        ),
        ElementSpec::two(
            800_000_003,
            ElementKind::Resistor { ohms: 330.0 },
            (-3_000_010, -3990),
            (-3_000_000, -4000),
        ),
        ElementSpec::ground(800_000_004, (-3_000_010, -3990)),
    ];
    run(
        "t555 + pad + ONE led",
        concat(vec![
            shifted(&t555, 1),
            padding(136, Structure::Districts { size: 20 }),
            led,
            src,
        ]),
    );
    run(
        "t555 + smooth-nl pad",
        concat(vec![
            shifted(&t555, 1),
            smooth_padding(136, Structure::Districts { size: 20 }),
        ]),
    );

    println!("\n-- scaling: t555 in rooms of growing size (districts~20)");
    for elems in [40usize, 136, 200, 300, 450, 600] {
        run(
            &format!("t555 + {elems} el"),
            concat(vec![
                shifted(&t555, 1),
                padding(elems, Structure::Districts { size: 20 }),
            ]),
        );
    }
}
