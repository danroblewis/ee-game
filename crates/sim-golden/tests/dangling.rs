//! WHAT A LOOSE WIRE-END COSTS, and why.
//!
//! Daniel found that a few resistors with one end connected to nothing made
//! his room noticeably slower and could not see why. They should be free:
//! a resistor going nowhere carries no current and changes no voltage.
//!
//! They are not free, and the reason is that EVERY LOOSE END IS A NODE. MNA
//! gives every node an unknown and a row, so a dangling end is solved for on
//! every substep like any other — even though KCL determines it in one line
//! (nothing else touches it, so the element's current must be zero, so its
//! voltage equals the far end's).
//!
//! The discriminator below is what makes that certain rather than plausible:
//! ten resistors connected at BOTH ends add ten elements and NO nodes, while
//! ten dangling ones add ten elements and ten nodes. Measured, 50k steps:
//!
//!     none                  4.3 ms    2 nodes
//!     10 connected          9.1 ms    2 nodes    (stamping only)
//!     10 dangling          16.7 ms   12 nodes    (stamping + 10 unknowns)
//!
//! Newton iterations and factorization counts are identical across all
//! three, so this is not extra solving work per step — it is a bigger
//! matrix, every step, forever.
//!
//! FIXED, by merging rather than pruning. A loose end of a pure conductance
//! is at EXACTLY the far end's potential — zero current through a resistor
//! is zero volts across it — so it can share that node's unknown instead of
//! getting one of its own. After:
//!
//!     none                  4.3 ms    2 nodes
//!     10 connected          9.2 ms    2 nodes
//!     10 dangling           8.0 ms    2 nodes   <- was 16.7 ms and 12 nodes
//!
//! A loose end now costs exactly what a connected one costs: its stamping,
//! and nothing else.
//!
//! ONLY PURE CONDUCTANCES, which is the whole safety argument. Zero current
//! means zero drop for a resistor and does not for anything else: a
//! capacitor holds its charge, an inductor its history, and an ideal source
//! with a loose end would become `0 = dc` — a singular row — the moment its
//! terminals were made one. Those keep their unknown.
use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;
use std::time::Instant;
const DT: f64 = 20e-6;

fn room(dangle: usize) -> Vec<sim_core::ElementSpec> {
    let mut d = vec![
        spec(1, K::VoltageSource{dc:0.0,amp:5.0,hz:300.0,phase:0.0,wave:Wave::Sine}, (0,0),(0,8)),
        gnd(2,(0,8)),
        spec(3, r(100.0), (0,0),(8,0)),
        spec(4, K::Speaker{ohms:8.0}, (8,0),(8,8)),
        gnd(5,(8,8)),
    ];
    // Resistors hanging off the speaker node with their far end connected to
    // nothing at all — exactly what Daniel had.
    for k in 0..dangle {
        d.push(spec(100 + k as u32, r(1000.0), (8, 0), (40 + k as i32 * 3, 40)));
    }
    d
}

/// The discriminator: ten resistors that ARE connected at both ends add no
/// new nodes, so if the cost is really "one extra unknown per loose end"
/// these should be nearly free while the dangling ones are not.
fn room_connected(n: usize) -> Vec<sim_core::ElementSpec> {
    let mut d = room(0);
    for k in 0..n {
        // Both ends on nodes that already exist.
        d.push(spec(200 + k as u32, r(1_000_000.0), (8, 0), (0, 0)));
    }
    d
}

#[test]
fn a_loose_end_costs_no_more_than_a_connected_one() {
    let bench = |d: Vec<sim_core::ElementSpec>| {
        let mut eng = Engine::new(DT);
        eng.set_elements(&d);
        eng.advance(2_000);
        let t = Instant::now();
        eng.advance(50_000);
        (t.elapsed().as_secs_f64() * 1000.0, eng.node_count())
    };
    let (base, nb) = bench(room(0));
    let (conn, nc) = bench(room_connected(10));
    let (dang, nd) = bench(room(10));
    println!("  none        : {base:6.1} ms   {nb} nodes");
    println!("  10 connected: {conn:6.1} ms   {nc} nodes");
    println!("  10 dangling : {dang:6.1} ms   {nd} nodes");
    // THE GUARD: loose ends must add no unknowns. Without the merge this is
    // 12 against 2, and the room pays for it on every step forever.
    assert_eq!(nd, nb, "loose ends must not add nodes");
    assert_eq!(nc, nb, "connected resistors never did");
}

#[test]
fn dangling_resistors_cost_what() {
    for n in [0usize, 1, 3, 10] {
        let d = room(n);
        let mut eng = Engine::new(DT);
        eng.set_elements(&d);
        eng.advance(2_000); // settle
        let t = Instant::now();
        let rep = eng.advance(50_000);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{n:>3} dangling: {ms:8.1} ms for 50k steps   nr={} rescues={} factorizations={}",
            rep.nr_iters, rep.rescues, eng.factorizations()
        );
    }
}
