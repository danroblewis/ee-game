//! TEMPORARY audit benchmark (delete after use).
use sim_core::{ElementKind as K, ElementSpec, Engine};
use std::time::Instant;

fn ladder(stages: usize, nonlinear: bool) -> Vec<ElementSpec> {
    let mut v = Vec::new();
    let mut id = 1u32;
    let mut next = |k: K, a: (i32, i32), b: (i32, i32)| {
        let s = ElementSpec {
            id,
            kind: k,
            pins: vec![a, b],
        };
        id += 1;
        s
    };
    v.push(next(
        K::VoltageSource {
            dc: 9.0,
            amp: 1.0,
            hz: 60.0,
            phase: 0.0,
        },
        (0, 0),
        (0, 4),
    ));
    v.push(ElementSpec {
        id: 9999,
        kind: K::Ground,
        pins: vec![(0, 4)],
    });
    for s in 0..stages {
        let x = s as i32 * 2;
        v.push(next(K::Resistor { ohms: 100.0 }, (x, 0), (x + 2, 0)));
        v.push(next(K::Capacitor { farads: 1e-6 }, (x + 2, 0), (0, 4)));
        if nonlinear {
            v.push(next(K::Diode, (x + 2, 0), (x + 2, 4)));
            v.push(next(K::Resistor { ohms: 1000.0 }, (x + 2, 4), (0, 4)));
        }
    }
    v
}

fn main() {
    for nonlinear in [false, true] {
        for stages in [10usize, 25, 50, 100, 200, 400] {
            let elems = ladder(stages, nonlinear);
            let mut eng = Engine::new(20e-6);
            let t_compile = Instant::now();
            eng.set_elements(&elems);
            let compile_ms = t_compile.elapsed().as_secs_f64() * 1e3;
            eng.advance(10);
            let steps = 2000u32;
            let t = Instant::now();
            let rep = eng.advance(steps);
            let el = t.elapsed().as_secs_f64();
            let t_frame = Instant::now();
            let n_frames = 30;
            for _ in 0..n_frames {
                let _ = eng.frame();
            }
            let frame_ms = t_frame.elapsed().as_secs_f64() * 1e3 / n_frames as f64;
            // unknowns: nodes = stages+1, branches = 1
            println!(
                "nonlinear={nonlinear} stages={stages:4} elems={:5} unknowns~{:4} \
                 {:9.1} steps/s  (real-time factor at dt=20us: {:6.2}x)  \
                 nr_iters/step={:.1} quarantined={} set_elements={:.2}ms frame()={:.3}ms",
                elems.len(),
                stages + 2,
                rep.steps as f64 / el,
                (rep.steps as f64 * 20e-6) / el,
                rep.nr_iters as f64 / rep.steps.max(1) as f64,
                rep.quarantined,
                compile_ms,
                frame_ms,
            );
        }
    }
}
