//! Scaling / parallelism measurements for the EE-game solver.
//!
//! Nothing here is wired into sim-core. Run from the repo root:
//!   cargo run --release --manifest-path tools/scale-bench/Cargo.toml -- <bench>
//! where <bench> is one of: lu | world | islands | sparse | all

mod demo;
mod mna;
mod sparse;
mod world;

use sim_core::Engine;
use std::time::Instant;

/// Server settings, read from crates/server/src/main.rs.
const DT: f64 = 20e-6;
const TICK_HZ: f64 = 30.0;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    println!("# machine: {}", machine());
    println!(
        "# profile: release (opt-level=3, lto=thin), rustc {}",
        rustc_version()
    );
    println!("# dt={DT}s tick={TICK_HZ}Hz substeps/tick={}", substeps());
    match arg.as_str() {
        "lu" => {
            bench_dense_lu();
            bench_state_copy();
        }
        "multirate" => bench_multirate(),
        "demo" => bench_demo_repeat(),
        "world" => bench_world(),
        "islands" => bench_islands(),
        "sparse" => bench_sparse(),
        "all" => {
            bench_dense_lu();
            bench_state_copy();
            bench_world();
            bench_islands();
            bench_multirate();
            bench_sparse();
        }
        other => eprintln!("unknown bench {other}"),
    }
}

fn substeps() -> u32 {
    ((1.0 / TICK_HZ) / DT).round() as u32
}

fn machine() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Run `f` until `min_secs` have elapsed (at least once, at most
/// `max_reps`); return seconds per rep and the rep count actually used.
fn bench<F: FnMut()>(mut f: F, min_secs: f64, max_reps: usize) -> (f64, usize) {
    // Warm-up rep, not timed.
    f();
    let t0 = Instant::now();
    let mut reps = 0usize;
    loop {
        f();
        reps += 1;
        if t0.elapsed().as_secs_f64() >= min_secs || reps >= max_reps {
            break;
        }
    }
    (t0.elapsed().as_secs_f64() / reps as f64, reps)
}

// ---------------------------------------------------------------- dense LU

fn circuit_shaped_dense(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    let mut a = vec![0.0; n * n];
    for r in 0..n {
        let mut acc = 0.0;
        for c in 0..n {
            if r != c {
                let v = rnd();
                a[r * n + c] = v;
                acc += v.abs();
            }
        }
        a[r * n + r] = acc + 0.5;
    }
    a
}

fn bench_dense_lu() {
    println!("\n## dense LU (sim-math::DenseLu), one thread");
    println!("n      factor_us   solve_us   factor_GFLOPs   reps");
    for n in [3usize, 4, 5, 6, 7, 8, 16, 32, 64, 128, 256, 512, 1024, 2048] {
        let a = circuit_shaped_dense(n, 0x1234 + n as u64);
        let mut lu = sim_math::DenseLu::new(n);
        let (tf, rf) = bench(
            || {
                lu.factor(&a);
            },
            0.4,
            100_000,
        );
        let mut x = vec![1.0; n];
        let (ts, _) = bench(
            || {
                lu.solve(&mut x);
                x.iter_mut().for_each(|v| *v = 1.0);
            },
            0.2,
            200_000,
        );
        // Dense LU: 2/3 n^3 flops (one multiply + one subtract per inner op).
        let gf = (2.0 / 3.0) * (n as f64).powi(3) / tf / 1e9;
        println!(
            "{n:<6} {:<11.3} {:<10.3} {:<15.2} {rf}",
            tf * 1e6,
            ts * 1e6,
            gf
        );
    }
}

/// The per-substep state snapshot `Engine::step` takes for the rescue
/// ladder (`let saved: Vec<ElemState> = ...`) is a fresh heap allocation
/// plus a copy of every element's state, every substep. ElemState is 13
/// f64 + padding; this measures what that costs at world scale.
fn bench_state_copy() {
    println!("\n## cost of one Vec<ElemState>-shaped snapshot per substep");
    println!("elems   snapshot_us  us_per_element");
    for e in [113usize, 565, 1130, 2260, 4520] {
        let src: Vec<[f64; 13]> = vec![[1.0; 13]; e];
        let (t, _) = bench(
            || {
                // Deliberately `iter().copied().collect()`, matching
                // engine.rs's `self.elems.iter().map(|e| e.state).collect()`.
                // `to_vec()` is faster, which is exactly why it would not
                // measure what the engine actually pays.
                #[allow(clippy::iter_cloned_collect)]
                let v: Vec<[f64; 13]> = src.iter().copied().collect();
                std::hint::black_box(&v);
            },
            0.3,
            200_000,
        );
        println!("{e:<7} {:<12.3} {:<.4}", t * 1e6, t * 1e6 / e as f64);
    }
}

// ------------------------------------------------------------------- world

struct WorldStats {
    elems: usize,
    islands: usize,
    n_mono: usize,
    n_max: usize,
    n_mean: f64,
}

fn stats(w: &world::World) -> WorldStats {
    let flat = w.flat();
    let (nn, nb) = world::mna_size(&flat);
    let sizes: Vec<usize> = w
        .islands
        .iter()
        .map(|i| {
            let (a, b) = world::mna_size(&i.elems);
            a + b
        })
        .collect();
    WorldStats {
        elems: flat.len(),
        islands: w.islands.len(),
        n_mono: nn + nb,
        n_max: *sizes.iter().max().unwrap(),
        n_mean: sizes.iter().sum::<usize>() as f64 / sizes.len() as f64,
    }
}

/// Time one substep of a monolithic engine, adaptively (a single substep of
/// a 2000-unknown nonlinear world costs seconds).
fn time_mono(flat: &[sim_core::ElementSpec], settle: u32) -> (f64, f64, u32) {
    let mut eng = Engine::new(DT);
    eng.set_elements(flat);
    let warm = eng.advance(settle);
    let t0 = Instant::now();
    let mut steps = 0u32;
    let mut iters = 0u32;
    loop {
        let r = eng.advance(1);
        steps += r.steps;
        iters += r.nr_iters;
        if t0.elapsed().as_secs_f64() > 1.5 || steps >= 400 {
            break;
        }
    }
    let per = t0.elapsed().as_secs_f64() / steps.max(1) as f64;
    let _ = warm;
    (per, iters as f64 / steps.max(1) as f64, steps)
}

fn bench_world() {
    println!("\n## today's demo room, verbatim (crates/server/src/main.rs)");
    let d = demo::demo_room_circuit();
    let (nn, nb) = world::mna_size(&d);
    let (per, iters, steps) = time_mono(&d, 500);
    println!("elems={} n={} (nodes={nn} branches={nb})", d.len(), nn + nb);
    println!(
        "us/substep={:.2}  NRiter/substep={:.2}  realtime_x={:.3}  ms/tick(1667 substeps)={:.1}  steps_measured={steps}",
        per * 1e6,
        iters,
        DT / per,
        per * substeps() as f64 * 1e3
    );

    println!("\n## ONE connected circuit (RC feeder ladder, cannot be partitioned)");
    println!("elems  n     linear?  us/substep  NRiter/substep  realtime_x  steps");
    for (stages, nl) in [
        (50usize, false),
        (250, false),
        (750, false),
        (1500, false),
        (50, true),
        (250, true),
        (750, true),
        (1500, true),
    ] {
        let f = world::feeder(stages, nl);
        let (nn, nb) = world::mna_size(&f);
        let (per, iters, steps) = time_mono(&f, 50);
        println!(
            "{:<6} {:<5} {:<8} {:<11.1} {:<15.2} {:<11.5} {steps}",
            f.len(),
            nn + nb,
            if nl { "no" } else { "yes" },
            per * 1e6,
            iters,
            DT / per
        );
    }

    println!("\n## monolithic engine (today's architecture), mixed linear+nonlinear world");
    println!(
        "copies elems islands n_mono n_max n_mean  us/substep  NRiter/substep  realtime_x  steps"
    );
    for copies in [1usize, 2, 5, 10, 20, 40] {
        let w = world::replicate(copies);
        let s = stats(&w);
        let flat = w.flat();
        let (per, iters, steps) = time_mono(&flat, 200);
        println!(
            "{copies:<6} {:<5} {:<7} {:<6} {:<5} {:<7.1} {:<11.1} {:<15.2} {:<11.4} {steps}",
            s.elems,
            s.islands,
            s.n_mono,
            s.n_max,
            s.n_mean,
            per * 1e6,
            iters,
            DT / per
        );
    }

    println!("\n## monolithic engine, LINEAR-ONLY world (factorization reused across substeps)");
    println!("copies elems islands n_mono  us/substep  realtime_x  steps");
    for copies in [1usize, 5, 20, 40, 80] {
        let w = world::replicate_linear(copies);
        let s = stats(&w);
        let flat = w.flat();
        let (per, _, steps) = time_mono(&flat, 200);
        println!(
            "{copies:<6} {:<5} {:<7} {:<6}  {:<11.2} {:<11.3} {steps}",
            s.elems,
            s.islands,
            s.n_mono,
            per * 1e6,
            DT / per
        );
    }

    println!("\n## per-element housekeeping (called once per tick / per edit)");
    println!("copies elems  frame_us  set_elements_ms");
    for copies in [1usize, 5, 10, 20, 40] {
        let w = world::replicate(copies);
        let flat = w.flat();
        let mut eng = Engine::new(DT);
        eng.set_elements(&flat);
        eng.advance(50);
        let (tf, _) = bench(
            || {
                let f = eng.frame();
                std::hint::black_box(&f);
            },
            0.5,
            2000,
        );
        let mut eng2 = Engine::new(DT);
        let (tc, _) = bench(
            || {
                eng2.set_elements(&flat);
            },
            0.5,
            200,
        );
        println!(
            "{copies:<6} {:<6} {:<9.1} {:<10.2}",
            flat.len(),
            tf * 1e6,
            tc * 1e3
        );
    }
}

// ----------------------------------------------------------------- islands

/// One engine per island: what per-island partitioning would buy, and how
/// it parallelises.
fn bench_islands() {
    use rayon::prelude::*;

    println!("\n## island partitioning: one Engine per island (serial), mixed world");
    println!("copies elems islands  us/substep(sum)  realtime_x  vs_monolithic");
    for copies in [1usize, 5, 10, 20, 40] {
        let w = world::replicate(copies);
        let s = stats(&w);
        let mut engines: Vec<Engine> = w
            .islands
            .iter()
            .map(|i| {
                let mut e = Engine::new(DT);
                e.set_elements(&i.elems);
                e.advance(200);
                e
            })
            .collect();
        let (per, _) = bench(
            || {
                for e in engines.iter_mut() {
                    e.advance(1);
                }
            },
            1.0,
            20_000,
        );
        // Monolithic comparison at the same world size.
        let (mono, _, _) = time_mono(&w.flat(), 200);
        println!(
            "{copies:<6} {:<5} {:<8} {:<16.2} {:<11.3} {:<10.1}x",
            s.elems,
            s.islands,
            per * 1e6,
            DT / per,
            mono / per
        );
    }

    let threads = rayon::current_num_threads();
    println!("\n## rayon across islands ({threads} rayon threads), mixed world");
    println!("copies islands serial_us/substep  par_us/substep(join per substep)  speedup  par_us/substep(join per tick)  speedup");
    for copies in [1usize, 5, 10, 20, 40] {
        let w = world::replicate(copies);
        let mut engines: Vec<Engine> = w
            .islands
            .iter()
            .map(|i| {
                let mut e = Engine::new(DT);
                e.set_elements(&i.elems);
                e.advance(200);
                e
            })
            .collect();
        let (serial, _) = bench(
            || {
                for e in engines.iter_mut() {
                    e.advance(1);
                }
            },
            0.7,
            20_000,
        );
        let (par1, _) = bench(
            || {
                engines.par_iter_mut().for_each(|e| {
                    e.advance(1);
                });
            },
            0.7,
            20_000,
        );
        // Join once per 100 substeps (what loose island coupling allows).
        let chunk = 100u32;
        let (par100, _) = bench(
            || {
                engines.par_iter_mut().for_each(|e| {
                    e.advance(chunk);
                });
            },
            0.7,
            2000,
        );
        let par100 = par100 / chunk as f64;
        println!(
            "{copies:<6} {:<7} {:<18.2} {:<32.2} {:<8.2} {:<29.3} {:<7.2}",
            w.islands.len(),
            serial * 1e6,
            par1 * 1e6,
            serial / par1,
            par100 * 1e6,
            serial / par100
        );
    }

    // Determinism of per-island parallelism: identical state hashes whether
    // the islands are advanced serially or by rayon.
    let w = world::replicate(4);
    let mk = || -> Vec<Engine> {
        w.islands
            .iter()
            .map(|i| {
                let mut e = Engine::new(DT);
                e.set_elements(&i.elems);
                e
            })
            .collect()
    };
    let mut a = mk();
    for e in a.iter_mut() {
        e.advance(3000);
    }
    let mut b = mk();
    b.par_iter_mut().for_each(|e| {
        e.advance(3000);
    });
    let ha: Vec<u64> = a.iter().map(|e| e.state_hash()).collect();
    let hb: Vec<u64> = b.iter().map(|e| e.state_hash()).collect();
    println!(
        "\n## determinism: {} islands x 3000 substeps, serial vs rayon state hashes: {}",
        ha.len(),
        if ha == hb { "IDENTICAL" } else { "DIFFER" }
    );

    equivalence_check();

    // How many islands are quiescent (nothing measurably moving)?
    let w = world::replicate(1);
    println!("\n## per-island cost and activity after 0.2 s of sim time");
    println!("island                  n     us/substep  max|dv|/substep");
    let mut quiescent = 0;
    for i in &w.islands {
        let (nn, nb) = world::mna_size(&i.elems);
        let mut e = Engine::new(DT);
        e.set_elements(&i.elems);
        e.advance((0.2 / DT) as u32);
        let f0 = e.frame();
        e.advance(1);
        let f1 = e.frame();
        let dv = f0
            .iter()
            .zip(f1.iter())
            .flat_map(|(a, b)| (0..a.npins).map(move |p| (a.v[p] - b.v[p]).abs()))
            .fold(0.0f64, f64::max);
        let (per, _) = bench(
            || {
                e.advance(1);
            },
            0.3,
            50_000,
        );
        if dv < 1e-9 {
            quiescent += 1;
        }
        println!(
            "{:<23} {:<5} {:<11.3} {:<.3e}",
            i.name,
            nn + nb,
            per * 1e6,
            dv
        );
    }
    println!(
        "quiescent islands (max|dv| < 1e-9 V per substep): {quiescent}/{}",
        w.islands.len()
    );
}

/// Run-to-run variance check: macOS moves threads between P- and E-cores,
/// so every headline number in the study is quoted with this spread in mind.
fn bench_demo_repeat() {
    println!("\n## today's demo room, 5 independent runs (variance check)");
    let d = demo::demo_room_circuit();
    for run in 0..5 {
        let (per, iters, steps) = time_mono(&d, 500);
        println!(
            "run {run}: us/substep={:.2}  NRiter/substep={:.2}  ms/tick={:.1}  realtime_x={:.3}  steps={steps}",
            per * 1e6,
            iters,
            per * substeps() as f64 * 1e3,
            DT / per
        );
    }
}

// --------------------------------------------------------------- multirate

/// How much timestep does each island actually need? Advance every golden
/// circuit to t = 0.2 s at dt = k x 20 us and compare pin voltages with the
/// k = 1 trajectory. `max|dv|` conflates amplitude error with phase error,
/// so for the three self-oscillating islands a large number means "the
/// waveform has drifted in phase", not "the answer is wrong" — read those
/// rows with that in mind.
fn bench_multirate() {
    println!("\n## per-island timestep tolerance (trajectory error at t = 0.2 s vs dt = 20 us)");
    println!("island                  |  k=2      k=5      k=25     k=100    | cost at k=1 (us per simulated second)");
    let w = world::replicate(1);
    let t_end = 0.2f64;
    for i in &w.islands {
        let refv = {
            let mut e = Engine::new(DT);
            e.set_elements(&i.elems);
            e.advance((t_end / DT) as u32);
            e.frame()
        };
        let mut cells = String::new();
        for k in [2u32, 5, 25, 100] {
            let dt = DT * k as f64;
            let mut e = Engine::new(dt);
            e.set_elements(&i.elems);
            e.advance((t_end / dt) as u32);
            let f = e.frame();
            let err = refv
                .iter()
                .zip(f.iter())
                .flat_map(|(a, b)| (0..a.npins).map(move |p| (a.v[p] - b.v[p]).abs()))
                .fold(0.0f64, f64::max);
            cells += &format!("{err:<9.2e}");
        }
        let mut e = Engine::new(DT);
        e.set_elements(&i.elems);
        e.advance(1000);
        let (per, _) = bench(
            || {
                e.advance(1);
            },
            0.3,
            50_000,
        );
        println!("{:<23} |  {cells} | {:.0}", i.name, per * (1.0 / DT) * 1e6);
    }
}

// ------------------------------------------------------------------ sparse

fn bench_sparse() {
    println!("\n## fixed-pattern sparse LU prototype vs dense LU, circuit-shaped matrices");
    println!("topology          n     nnz(A)  nnz(L+U)  fill_x  analyze_ms  refactor_us  solve_us  dense_factor_us  dense/sparse  max_rel_diff  resid");
    let cases: Vec<(String, mna::Coo)> = vec![
        ("ladder-100".into(), mna::ladder(100, DT)),
        ("ladder-500".into(), mna::ladder(500, DT)),
        ("ladder-2000".into(), mna::ladder(2000, DT)),
        ("mesh-10x10".into(), mna::mesh(10, 10)),
        ("mesh-20x20".into(), mna::mesh(20, 20)),
        ("mesh-45x45".into(), mna::mesh(45, 45)),
        ("islands-20x9".into(), mna::islands(20, 9)),
        ("islands-100x9".into(), mna::islands(100, 9)),
        ("islands-220x9".into(), mna::islands(220, 9)),
    ];
    for (name, coo) in cases {
        let n = coo.n;
        let vals: Vec<f64> = coo.entries.iter().map(|(_, _, v)| *v).collect();
        let t0 = Instant::now();
        let an = sparse::SparseLu::analyze(&coo);
        let analyze = t0.elapsed().as_secs_f64();
        let mut slu = an.lu;
        assert!(slu.refactor(&vals), "{name}: sparse refactor failed");

        let b = mna::rhs(n, 42);
        let mut xs = b.clone();
        slu.solve(&mut xs);
        let dense = coo.dense();
        let mut dlu = sim_math::DenseLu::new(n);
        assert!(dlu.factor(&dense), "{name}: dense factor failed");
        let mut xd = b.clone();
        dlu.solve(&mut xd);
        let maxrel = xs
            .iter()
            .zip(xd.iter())
            .map(|(a, b)| (a - b).abs() / b.abs().max(1e-9))
            .fold(0.0f64, f64::max);
        // Residual of the sparse solve against the original matrix.
        let ax = coo.mul(&xs);
        let bnorm = b.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let resid = ax
            .iter()
            .zip(b.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max)
            / bnorm;

        let (tr, _) = bench(
            || {
                slu.refactor(&vals);
            },
            0.4,
            200_000,
        );
        let mut x = b.clone();
        let (tsv, _) = bench(
            || {
                x.copy_from_slice(&b);
                slu.solve(&mut x);
            },
            0.3,
            400_000,
        );
        let (td, _) = bench(
            || {
                dlu.factor(&dense);
            },
            0.4,
            100_000,
        );
        println!(
            "{name:<17} {n:<5} {:<7} {:<9} {:<7.2} {:<11.2} {:<12.2} {:<9.2} {:<16.1} {:<13.1} {:<.2e}",
            an.nnz_a,
            an.nnz_lu,
            an.nnz_lu as f64 / an.nnz_a as f64,
            analyze * 1e3,
            tr * 1e6,
            tsv * 1e6,
            td * 1e6,
            td / (tr + tsv),
            maxrel,
        );
        println!(
            "#   residual ||Ax-b||inf/||b||inf = {resid:.2e}, refactor flops(mul-sub) = {}",
            an.flops
        );
        let _ = an.flops;
    }
}
