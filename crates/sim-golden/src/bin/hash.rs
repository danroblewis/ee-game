//! S1 determinism harness, native side: run each golden circuit for 10k
//! steps and print its state hash. The wasm side (sim-wasm `golden` feature
//! via Node) must print byte-identical lines.

use sim_core::Engine;

fn main() {
    for (name, elems) in sim_golden::all_golden() {
        let mut eng = Engine::new(1e-6);
        eng.set_elements(&elems);
        let report = eng.advance(10_000);
        println!(
            "{name} {:016x} steps={} quarantined={}",
            eng.state_hash(),
            report.steps,
            eng.is_quarantined()
        );
    }
}
