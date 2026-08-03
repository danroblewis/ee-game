//! gpu-bench: standalone benchmark for the GPU throughput-tier spike.
//!
//! Answers the design's Phase-1 gate questions:
//!  1. At what batch size does batched GPU dense LU beat the sim-math CPU
//!     path (single-thread and rayon-across-batch)?
//!  2. Is the precision survivable (f32 raw, f32 + CPU-f64 iterative
//!     refinement, df64) on well-conditioned and ill-conditioned MNA-like
//!     systems?
//!
//! Run: cargo run --release -p sim-gpu-spike [-- --quick]
//! No determinism claim; no server integration; dev-only.

mod cpu;
mod gen;
mod gpu;

use gpu::{GpuBatch, Precision};
use std::time::Instant;

const SEED: u64 = 0x5eed_c0ffee_1234;
const PIPE_DEPTH: usize = 16; // back-to-back submits for steady-state timing
const IR_ITERS: usize = 3;

fn time_it(budget_s: f64, max_reps: usize, mut f: impl FnMut()) -> f64 {
    f(); // warmup (also first-use pipeline warm)
    let start = Instant::now();
    let mut reps = 0usize;
    loop {
        f();
        reps += 1;
        let e = start.elapsed().as_secs_f64();
        if e >= budget_s || reps >= max_reps {
            return e / reps as f64;
        }
    }
}

fn fmt_us(t: f64) -> String {
    let us = t * 1e6;
    if us >= 100_000.0 {
        format!("{:>9.0}", us)
    } else if us >= 100.0 {
        format!("{:>9.1}", us)
    } else {
        format!("{:>9.2}", us)
    }
}

#[allow(dead_code)] // gpu/df64_pipe retained so rows carry the full record
struct Row {
    n: usize,
    batch: usize,
    cpu1: f64,
    ray: f64,
    gpu: f64,
    gpu_pipe: f64,
    df64_pipe: Option<f64>,
}

fn main() {
    let quick = std::env::args().any(|a| a == "--quick");
    let sizes: &[usize] = &[8, 16, 32, 64];
    let batches: &[usize] = if quick { &[1, 256] } else { &[1, 16, 256, 4096] };
    let budget = if quick { 0.1 } else { 0.35 };

    let ctx = gpu::Ctx::new();
    println!("== sim-gpu-spike: batched dense LU benchmark ==");
    println!(
        "adapter: {} ({:?}, {:?}); max workgroup storage {} B",
        ctx.info.name, ctx.info.device_type, ctx.info.backend, ctx.max_wg_storage
    );
    println!(
        "cpu: {} threads (rayon); sim-math DenseLu f64 is the reference/truth",
        std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1)
    );
    println!();

    // ---------- timing grid (well-conditioned) ----------
    println!("-- timing, well-conditioned diagonally-dominant systems --");
    println!("   (per-batch wall time; GPU 'pipe' = {PIPE_DEPTH} submits in flight, steady state)");
    println!(
        "{:>4} {:>6} | {:>9} {:>9} | {:>9} {:>9} {:>9} | {:>8} {:>8}",
        "n", "batch", "cpu1 us", "rayon us", "gpu us", "gpu-pipe", "df64pipe", "gpu/cpu1", "gpu/ray"
    );
    let mut rows: Vec<Row> = Vec::new();
    for &n in sizes {
        let max_batch = *batches.last().unwrap();
        let wb = gen::well_conditioned(n, max_batch, SEED + n as u64);
        for &batch in batches {
            let a = &wb.a[..batch * n * n];
            let b = &wb.b[..batch * n];
            let mut x = vec![0.0; batch * n];

            let cpu1 = time_it(budget, 2000, || cpu::solve_batch_single(n, a, b, &mut x));
            let ray = time_it(budget, 2000, || cpu::solve_batch_rayon(n, a, b, &mut x));

            let g = GpuBatch::new(&ctx, n, batch, Precision::F32).expect("f32 fits");
            g.upload(a, b);
            let gpu_t = time_it(budget, 2000, || {
                g.factor_solve();
                let _ = g.read_x();
            });
            let gpu_pipe = time_it(budget, 2000, || {
                for _ in 0..PIPE_DEPTH {
                    g.factor_solve();
                }
                g.wait();
            }) / PIPE_DEPTH as f64;

            let df64_pipe = GpuBatch::new(&ctx, n, batch, Precision::Df64).map(|gd| {
                gd.upload(a, b);
                time_it(budget, 2000, || {
                    for _ in 0..PIPE_DEPTH {
                        gd.factor_solve();
                    }
                    gd.wait();
                }) / PIPE_DEPTH as f64
            });

            println!(
                "{:>4} {:>6} | {} {} | {} {} {} | {:>8.2} {:>8.2}",
                n,
                batch,
                fmt_us(cpu1),
                fmt_us(ray),
                fmt_us(gpu_t),
                fmt_us(gpu_pipe),
                df64_pipe.map(fmt_us).unwrap_or_else(|| "      n/a".into()),
                cpu1 / gpu_pipe,
                ray / gpu_pipe,
            );
            rows.push(Row { n, batch, cpu1, ray, gpu: gpu_t, gpu_pipe, df64_pipe });
        }
    }
    println!();

    // crossover summary
    println!("-- crossover (steady-state GPU pipe vs CPU) --");
    for &n in sizes {
        let vs_cpu1 = rows.iter().find(|r| r.n == n && r.gpu_pipe < r.cpu1).map(|r| r.batch);
        let vs_ray = rows.iter().find(|r| r.n == n && r.gpu_pipe < r.ray).map(|r| r.batch);
        println!(
            "  n={:>2}: beats cpu1 at batch {}  |  beats rayon at batch {}",
            n,
            vs_cpu1.map(|b| b.to_string()).unwrap_or("never".into()),
            vs_ray.map(|b| b.to_string()).unwrap_or("never".into()),
        );
    }
    println!();

    // ---------- accuracy ----------
    let acc_batch = if quick { 64 } else { 256 };
    let sets: [(&str, Option<(f64, f64)>); 3] = [
        ("well-conditioned", None),
        ("moderate MNA-like (g in 1e-4..1e4 S, gmin diag)", Some((-4.0, 4.0))),
        ("harsh MNA-like (g in 1e-12..1e6 S, gmin diag)", Some((-12.0, 6.0))),
    ];
    for (label, span) in sets {
        println!("-- accuracy, {label}, batch {acc_batch} --");
        println!(
            "{:>4} | {:>12} {:>10} | {:>10} {:>10} {:>5} | {:>10} {:>10} {:>5} | {:>10} {:>10} {:>5}",
            "n", "cpu f64 res", "", "f32 err", "f32 res", ">1e-6", "f32+IR err", "IR res", ">1e-6", "df64 err", "df64 res", ">1e-6"
        );
        for &n in sizes {
            let batch = gen_batch(n, acc_batch, span);
            let (a, b) = (&batch.a, &batch.b);
            let mut truth = vec![0.0; b.len()];
            cpu::solve_batch_single(n, a, b, &mut truth);
            let cpu_acc = cpu::residuals(n, a, b, &truth, None);

            // raw f32
            let g = GpuBatch::new(&ctx, n, acc_batch, Precision::F32).unwrap();
            g.upload(a, b);
            g.factor_solve();
            let x32 = g.read_x();
            let acc32 = cpu::residuals(n, a, b, &x32, Some(&truth));

            // f32 + CPU-f64 iterative refinement (the design's gate pattern)
            let (xir, _iters) = ir_solve(&g, n, a, b, IR_ITERS);
            let accir = cpu::residuals(n, a, b, &xir, Some(&truth));

            // df64
            let accdf = GpuBatch::new(&ctx, n, acc_batch, Precision::Df64).map(|gd| {
                gd.upload(a, b);
                gd.factor_solve();
                let xdf = gd.read_x();
                cpu::residuals(n, a, b, &xdf, Some(&truth))
            });

            let (dfe, dfr, dfo) = match &accdf {
                Some(acc) => (
                    format!("{:>10.2e}", acc.max_rel_err),
                    format!("{:>10.2e}", acc.max_residual),
                    format!("{:>5}", acc.over_gate),
                ),
                None => ("       n/a".into(), "       n/a".into(), "  n/a".into()),
            };
            println!(
                "{:>4} | {:>12.2e} {:>10} | {:>10.2e} {:>10.2e} {:>5} | {:>10.2e} {:>10.2e} {:>5} | {} {} {}",
                n,
                cpu_acc.max_residual,
                "",
                acc32.max_rel_err,
                acc32.max_residual,
                acc32.over_gate,
                accir.max_rel_err,
                accir.max_residual,
                accir.over_gate,
                dfe,
                dfr,
                dfo,
            );
        }
        println!("   (err = max normwise rel. error vs sim-math f64 truth; res = max normwise backward error, f64 on CPU; >1e-6 = matrices failing the gate)");
        println!();
    }

    // df64 sanity: strict-IEEE CPU reference of the same double-single
    // algorithm, to separate algorithm bugs from Metal fast-math damage.
    {
        let n = 32;
        let wb = gen::well_conditioned(n, 8, SEED + 7 + n as u64);
        let mut xref = vec![0.0; wb.b.len()];
        for m in 0..8 {
            let x = cpu::df64_ref::lu_solve(n, &wb.a[m * n * n..(m + 1) * n * n], &wb.b[m * n..(m + 1) * n]);
            xref[m * n..(m + 1) * n].copy_from_slice(&x);
        }
        let mut truth = vec![0.0; wb.b.len()];
        cpu::solve_batch_single(n, &wb.a, &wb.b, &mut truth);
        let acc = cpu::residuals(n, &wb.a, &wb.b, &xref, Some(&truth));
        println!(
            "df64 CPU reference (strict IEEE, n=32): rel err {:.2e}, backward err {:.2e} — what the GPU df64 kernel would achieve without Metal fast-math",
            acc.max_rel_err, acc.max_residual
        );
    }

    // IR cost, one representative config
    if !quick {
        let n = 64;
        let batch = 256;
        let wb = gen::well_conditioned(n, batch, SEED + 99);
        let g = GpuBatch::new(&ctx, n, batch, Precision::F32).unwrap();
        g.upload(&wb.a, &wb.b);
        g.factor_solve();
        let x0 = g.read_x();
        let mut r = vec![0.0; wb.b.len()];
        let t_ir = time_it(0.3, 500, || {
            cpu::residual_vec(n, &wb.a, &wb.b, &x0, &mut r);
            g.upload_rhs(&r);
            g.solve_only();
            let _ = g.read_x();
        });
        println!(
            "IR iteration cost (n=64, batch=256): {:.1} us per refinement pass (CPU f64 residual + upload + solve dispatch + readback)",
            t_ir * 1e6
        );
    }
}

fn gen_batch(n: usize, count: usize, span: Option<(f64, f64)>) -> gen::Batch {
    match span {
        Some((lo, hi)) => gen::mna_network(n, count, SEED ^ 0xabcdef, lo, hi),
        None => gen::well_conditioned(n, count, SEED + 7 + n as u64),
    }
}

/// GPU f32 solve + CPU-f64 residual refinement (Haidar/Dongarra/Higham
/// mixed-precision pattern). x accumulates in f64 on the CPU; each pass
/// solves A·d = r with the stored f32 factors on the GPU.
fn ir_solve(g: &GpuBatch, n: usize, a: &[f64], b: &[f64], max_iters: usize) -> (Vec<f64>, usize) {
    g.upload(a, b);
    g.factor_solve();
    let mut x = g.read_x();
    let mut r = vec![0.0; b.len()];
    let mut used = 0;
    for _ in 0..max_iters {
        let worst = cpu::residual_vec(n, a, b, &x, &mut r);
        if worst < 1e-12 {
            break;
        }
        g.upload_rhs(&r);
        g.solve_only();
        let d = g.read_x();
        for (xi, di) in x.iter_mut().zip(d.iter()) {
            *xi += di;
        }
        used += 1;
    }
    (x, used)
}
