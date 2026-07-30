//! Scale baseline: where does the time actually go as circuits grow?
//!
//! Measurement only — nothing here optimizes anything. It generates
//! game-shaped worlds (`sim_golden::scale`), then measures, per size and per
//! island structure: unknowns after wire closure, matrix nnz/density,
//! compile cost, dense LU factor and solve cost separately, NR iterations
//! and refactorizations per simulated second, and the real-time ratio the
//! player actually feels. It then measures the two structural wins the plan
//! contemplates but has not built: island partitioning and quiescence.
//!
//! One-line invocation (from the repo root):
//!
//! ```text
//! cargo run --release -p sim-golden --bin scale
//! ```
//!
//! The full sweep behind `docs/scale-baseline.md` (tens of minutes, needs
//! ~5 GB of RAM for the 50k row's one dense matrix):
//!
//! ```text
//! cargo run --release -p sim-golden --bin scale -- \
//!     --sizes 500,1000,5000,20000,50000 --max-n 14000
//! ```
//!
//! Flags:
//!   --sizes 500,1000,5000     element counts to sweep
//!   --structures all          subset of districts,one,linear to sweep
//!   --district 100            elements per district in districts mode
//!   --nonlinear 30            percent of blocks from the nonlinear pool
//!   --active 20               percent of districts containing an oscillator
//!   --max-n 6000              refuse to build a dense matrix larger than this
//!   --step-budget 0.35        wall seconds of substeps per configuration
//!   --min-steps 5             substeps to time even if that blows the budget
//!   --step-cap 40             hard wall-second cap on one config's substeps
//!   --frame-max 8000          elements above which frame() is skipped
//!   --skip islands|quiescence|sizes|crossover|kernel
//!
//! Everything printed is measured on the machine that ran it; the harness
//! prints the machine, profile and every input so a number can never be
//! quoted without its provenance.

use sim_core::Engine;
use sim_golden::scale::{self, GenParams, LuOps, Structure, World};
use sim_math::DenseLu;
use std::time::Instant;

/// Matches the server (`crates/server/src/main.rs`): dt = 20 us, 30 Hz tick.
const DT: f64 = 20e-6;
const TICK_HZ: f64 = 30.0;

fn steps_per_tick() -> f64 {
    (1.0 / TICK_HZ) / DT
}

// ------------------------------------------------------------------ timing

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Time `f` enough times to get a stable median without burning more than
/// ~`budget_s` seconds.
fn time_median<F: FnMut()>(budget_s: f64, max_reps: usize, mut f: F) -> f64 {
    let t = Instant::now();
    f();
    let first = t.elapsed().as_secs_f64();
    let reps = ((budget_s / first.max(1e-9)) as usize).clamp(1, max_reps);
    let mut samples = Vec::with_capacity(reps);
    for _ in 0..reps {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64());
    }
    median(samples)
}

// -------------------------------------------------------------- one config

struct Row {
    label: String,
    elements: usize,
    islands: usize,
    n: usize,
    nnz: usize,
    nnz_per_row: f64,
    density: f64,
    lu_nnz: usize,
    fill: f64,
    ops: LuOps,
    linear: bool,
    compile_ms: f64,
    factor_ms: f64,
    solve_us: f64,
    step_us: f64,
    nr_mean: f64,
    nr_max: u32,
    refac_per_substep: f64,
    refac_per_simsec: f64,
    ratio: f64,
    tick_pct: f64,
    steps_measured: u32,
    rescues: u32,
    quarantined: bool,
    frame_ms: f64,
}

impl Row {
    /// Share of one substep explained by its refactorizations.
    ///
    /// NOT an independent measurement of the engine's internals: it is
    /// (isolated factor cost, measured) x (refactorizations per substep,
    /// counted by the engine) / (substep wall time, measured). The isolated
    /// kernel timing re-factors the same matrix in a tight loop, so its cache
    /// state is a best case; the shares can therefore sum to slightly more or
    /// less than 100%, and the printout says so rather than normalizing.
    fn factor_share(&self) -> f64 {
        self.refac_per_substep * self.factor_ms * 1e3 / self.step_us
    }

    /// The engine solves once per NR iteration, factored or not.
    fn solve_share(&self) -> f64 {
        self.nr_mean * self.solve_us / self.step_us
    }
}

struct Cfg {
    max_n: usize,
    step_budget_s: f64,
    min_steps: u32,
    step_cap_s: f64,
    /// Element count above which `frame()` is skipped instead of measured.
    frame_max: usize,
}

/// Advance one substep at a time, collecting per-step NR and factorization
/// statistics along with the wall time.
struct StepStats {
    steps: u32,
    wall: f64,
    nr_total: u64,
    nr_max: u32,
    factorizations: u64,
    rescues: u32,
    quarantined: bool,
}

fn measure_steps(eng: &mut Engine, cfg: &Cfg) -> StepStats {
    // One step first, to size the loop from a real measurement.
    let f0 = eng.factorizations();
    let t = Instant::now();
    let r0 = eng.advance(1);
    let first = t.elapsed().as_secs_f64();
    let mut st = StepStats {
        steps: 1,
        wall: first,
        nr_total: r0.nr_iters as u64,
        nr_max: r0.nr_iters,
        factorizations: eng.factorizations() - f0,
        rescues: r0.rescues,
        quarantined: r0.quarantined,
    };
    // Always time `min_steps` substeps (a 2-sample mean of a 1 s substep is
    // not a measurement), but never run past the hard cap.
    let target = ((cfg.step_budget_s / first.max(1e-9)) as u64)
        .clamp(cfg.min_steps as u64, 50_000)
        .saturating_sub(1);
    for _ in 0..target {
        if st.quarantined || st.wall > cfg.step_cap_s {
            break;
        }
        let f = eng.factorizations();
        let t = Instant::now();
        let r = eng.advance(1);
        st.wall += t.elapsed().as_secs_f64();
        st.steps += 1;
        st.nr_total += r.nr_iters as u64;
        st.nr_max = st.nr_max.max(r.nr_iters);
        st.factorizations += eng.factorizations() - f;
        st.rescues += r.rescues;
        st.quarantined = r.quarantined;
    }
    st
}

fn measure(params: GenParams, cfg: &Cfg) -> Option<Row> {
    let world = scale::generate(params);
    let specs = world.flat();
    let topo = scale::topology(&specs);
    let label = world.label();
    if topo.unknowns > cfg.max_n {
        let bytes = (topo.unknowns * topo.unknowns * 8) as f64;
        println!(
            "SKIPPED  {label}\n         junctions={} nodes={} branches={} islands={} \
             n={} unknowns; today's ONE dense matrix needs {:.2} GB for A plus the \
             same again for its LU copy, over the --max-n {} cap. Not measured.",
            topo.junctions,
            topo.nodes,
            topo.branches,
            topo.islands,
            topo.unknowns,
            2.0 * bytes / 1e9,
            cfg.max_n
        );
        return None;
    }

    let mut eng = Engine::new(DT);
    let t = Instant::now();
    eng.set_elements(&specs);
    let compile_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(
        eng.unknowns(),
        topo.unknowns,
        "generator topology disagrees with compile()"
    );

    // Settle a few steps so the matrix is stamped and the linear path has
    // taken its one-off factorization out of the per-step measurement.
    eng.advance(4);
    let n = eng.unknowns();
    let nnz = eng.matrix_nnz();

    // Dense LU factor and solve, timed separately on the live matrix.
    let a = eng.matrix().to_vec();
    let mut lu = DenseLu::new(n);
    let factor_ms = time_median(0.5, 25, || {
        lu.factor(&a);
    }) * 1e3;
    let lu_nnz = lu.factor_nnz();
    let mut rhs = vec![1.0f64; n];
    let solve_us = time_median(0.2, 500, || {
        rhs.iter_mut().for_each(|v| *v = 1.0);
        lu.solve(&mut rhs);
    }) * 1e6;
    drop(lu);
    // What that factor call actually executed, counted (see `scale::lu_ops`).
    let (ops, _) = scale::lu_ops(&a, n);
    drop(a);

    let st = measure_steps(&mut eng, cfg);
    let step_us = st.wall / st.steps as f64 * 1e6;
    let sim_s = st.steps as f64 * DT;
    let ratio = sim_s / st.wall;

    // The server calls frame() once per tick; its wire-current KCL pass is
    // O(elements^2) today, so it belongs in the budget picture. Above
    // `frame_max` elements it is skipped rather than measured (it grows
    // fast enough to dominate the harness itself) and reported as 0.
    let frame_ms = if specs.len() <= cfg.frame_max {
        time_median(0.3, 20, || {
            let f = eng.frame();
            std::hint::black_box(&f);
        }) * 1e3
    } else {
        0.0
    };

    Some(Row {
        label,
        elements: specs.len(),
        islands: topo.islands,
        n,
        nnz,
        nnz_per_row: nnz as f64 / n as f64,
        density: nnz as f64 / (n * n) as f64,
        lu_nnz,
        fill: lu_nnz as f64 / nnz as f64,
        ops,
        linear: eng.is_linear(),
        compile_ms,
        factor_ms,
        solve_us,
        step_us,
        nr_mean: st.nr_total as f64 / st.steps as f64,
        nr_max: st.nr_max,
        refac_per_substep: st.factorizations as f64 / st.steps as f64,
        refac_per_simsec: st.factorizations as f64 / sim_s,
        ratio,
        tick_pct: step_us * steps_per_tick() / (1e6 / TICK_HZ) * 100.0,
        steps_measured: st.steps,
        rescues: st.rescues,
        quarantined: st.quarantined,
        frame_ms,
    })
}

fn print_row(r: &Row) {
    println!("--- {}", r.label);
    println!(
        "    elements={} islands={} n={} (nodes+branches) nnz={} \
         nnz/row={:.2} density={:.4}% linear={}",
        r.elements,
        r.islands,
        r.n,
        r.nnz,
        r.nnz_per_row,
        r.density * 100.0,
        r.linear
    );
    println!(
        "    LU factor nnz={} = {:.1}x fill over the matrix, {:.1}% of the n^2 \
         dense footprint",
        r.lu_nnz,
        r.fill,
        100.0 * r.lu_nnz as f64 / (r.n * r.n) as f64
    );
    println!(
        "    LU work counted: {:.3} M row-updates per factor = {:.1}% of a \
         structure-blind n^3/3 ({:.3} M); zero-multiplier row skips {}",
        r.ops.updates as f64 / 1e6,
        100.0 * (1.0 - r.ops.structure_saving()),
        r.ops.dense_updates() as f64 / 1e6,
        r.ops.skipped_rows
    );
    println!(
        "    compile(set_elements)={:.2} ms   dense factor={:.3} ms   \
         solve={:.1} us   frame()={}",
        r.compile_ms,
        r.factor_ms,
        r.solve_us,
        if r.frame_ms > 0.0 {
            format!("{:.2} ms", r.frame_ms)
        } else {
            "not measured (over --frame-max)".to_string()
        }
    );
    println!(
        "    per substep={:.1} us   NR iters/substep mean={:.2} max={}   \
         refactors/substep={:.2}   refactors/sim-second={:.0}   rescues={}{}",
        r.step_us,
        r.nr_mean,
        r.nr_max,
        r.refac_per_substep,
        r.refac_per_simsec,
        r.rescues,
        if r.quarantined { "   QUARANTINED" } else { "" }
    );
    let accounted = r.factor_share() + r.solve_share();
    println!(
        "    substep attribution: refactorization {:.0}% + triangular solve {:.0}% \
         = {:.0}% of the measured substep{}",
        100.0 * r.factor_share(),
        100.0 * r.solve_share(),
        100.0 * accounted,
        if accounted > 1.02 {
            " (over 100%: the isolated kernel timings are warm-cache best cases)"
        } else {
            ", the rest is stamping + NR bookkeeping + accept"
        }
    );
    println!(
        "    REAL-TIME RATIO = {:.4}x sim-s per wall-s   \
         (one 33.3 ms tick needs {:.0}% of its budget; {} substeps timed)",
        r.ratio, r.tick_pct, r.steps_measured
    );
}

fn md_row(r: &Row) -> String {
    format!(
        "| {} | {} | {} | {} | {} | {:.2} | {:.1}x | {:.2} | {:.3} | {:.1} | {:.1} | {:.2} | {} | {:.0} | {:.0}% | {:.0}% | **{:.4}x** | {:.0}% |",
        r.elements,
        r.label.split(", ").nth(1).unwrap_or("?"),
        r.islands,
        r.n,
        r.nnz,
        r.nnz_per_row,
        r.fill,
        r.compile_ms,
        r.factor_ms,
        r.solve_us,
        r.step_us,
        r.nr_mean,
        r.nr_max,
        r.refac_per_simsec,
        100.0 * r.factor_share(),
        100.0 * r.solve_share(),
        r.ratio,
        r.tick_pct
    )
}

const MD_HEAD: &str = "| elements | structure | islands | n | nnz | nnz/row | LU fill | compile ms | factor ms | solve us | us/substep | NR mean | NR max | refactor/sim-s | refactor share | solve share | real-time | tick budget |\n|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|";

// ------------------------------------------------------------------ kernel

/// Raw LU kernel throughput, separated from circuit structure.
///
/// A stamped MNA matrix and a full dense matrix of the same `n` execute wildly
/// different amounts of inner-loop work, but they pay the same `n^2` costs:
/// `factor` memcpy's the whole `n x n` array and then walks `n(n-1)/2` pivot
/// candidates down columns (stride `n`, one cache line each). Timing both at
/// the same `n` says which of the two is actually paying for the factor — and
/// therefore whether a sparse solver would win by avoiding flops or by
/// avoiding memory traffic.
fn kernel_experiment(dsize: usize, nonlinear: u32) {
    println!("\n=== LU KERNEL: dense matrix vs stamped MNA matrix at the same n ===");
    println!(
        "  | n | dense factor ms | dense row-updates | ns/update | MNA factor ms | \
         MNA row-updates | MNA ns/'update' | dense/MNA time |"
    );
    println!("  |---:|---:|---:|---:|---:|---:|---:|---:|");
    for n in [64usize, 128, 256, 512, 1024, 2048] {
        // Diagonally dominant so no pivoting is needed and the elimination is
        // completely fill-free-free: every multiplier is nonzero, so this is
        // the honest n^3/3 upper bound for this kernel.
        let mut a = vec![0.0f64; n * n];
        for r in 0..n {
            let mut off = 0.0;
            for c in 0..n {
                if r != c {
                    let v = 1.0 / (1.0 + ((r * 31 + c * 17) % 97) as f64);
                    a[r * n + c] = v;
                    off += v;
                }
            }
            a[r * n + r] = off + 1.0;
        }
        let mut lu = DenseLu::new(n);
        let d_ms = time_median(0.4, 20, || {
            lu.factor(&a);
        }) * 1e3;
        let (d_ops, _) = scale::lu_ops(&a, n);

        // The closest stamped MNA matrix: grow a districts world until its
        // unknown count reaches n.
        let mut target = n * 4;
        let (mut m_n, mut specs) = (0usize, Vec::new());
        while m_n < n && target < n * 40 {
            let w = scale::generate(
                GenParams::new(target, Structure::Districts { size: dsize }).nonlinear(nonlinear),
            );
            specs = w.flat();
            m_n = scale::topology(&specs).unknowns;
            target += n / 2;
        }
        let mut eng = Engine::new(DT);
        eng.set_elements(&specs);
        eng.advance(4);
        let m_n = eng.unknowns();
        let ma = eng.matrix().to_vec();
        let mut mlu = DenseLu::new(m_n);
        let m_ms = time_median(0.4, 20, || {
            mlu.factor(&ma);
        }) * 1e3;
        let (m_ops, _) = scale::lu_ops(&ma, m_n);
        println!(
            "  | {} | {:.3} | {} | {:.2} | {:.3} (n={}) | {} | {:.2} | {:.1}x |",
            n,
            d_ms,
            d_ops.updates,
            d_ms * 1e6 / d_ops.updates as f64,
            m_ms,
            m_n,
            m_ops.updates,
            m_ms * 1e6 / m_ops.updates.max(1) as f64,
            d_ms / m_ms
        );
    }
    println!(
        "  ns/update far above the dense column's value means the factor is NOT paying \
         for arithmetic: it is paying the O(n^2) matrix copy and strided pivot search \
         that `factor` performs regardless of sparsity."
    );
}

// --------------------------------------------------------------- crossover

/// The number the brief asks for: the largest element count that still holds
/// real time. Bisects on target element count, measuring each candidate the
/// same way the sweep does.
fn crossover(structure: Structure, nonlinear: u32, active: u32, cfg: &Cfg) {
    let ratio_at = |target: usize| -> Option<(usize, usize, f64)> {
        let p = GenParams::new(target, structure)
            .nonlinear(nonlinear)
            .active(active);
        let world = scale::generate(p);
        let specs = world.flat();
        let mut eng = Engine::new(DT);
        eng.set_elements(&specs);
        eng.advance(4);
        let st = measure_steps(&mut eng, cfg);
        if st.quarantined {
            return None;
        }
        Some((specs.len(), eng.unknowns(), st.steps as f64 * DT / st.wall))
    };
    let (mut lo, mut hi) = (12usize, 4_000usize);
    // Established bracket: the smallest world must be faster than real time
    // and the largest slower, or a bisection means nothing.
    let Some((lo_e, lo_n, lo_r)) = ratio_at(lo) else {
        println!("  {} : smallest world quarantined", structure.label());
        return;
    };
    let Some((hi_e, hi_n, hi_r)) = ratio_at(hi) else {
        println!("  {} : largest world quarantined", structure.label());
        return;
    };
    print!(
        "  {}, {nonlinear}% nonlinear: bracket {lo_e} elems (n={lo_n}) {lo_r:.2}x .. \
         {hi_e} elems (n={hi_n}) {hi_r:.4}x -> ",
        structure.label()
    );
    if lo_r < 1.0 {
        println!("ALREADY below real time at {lo_e} elements ({lo_r:.2}x). No crossover in range.");
        return;
    }
    if hi_r > 1.0 {
        println!("still above real time at {hi_e} elements. No crossover in range.");
        return;
    }
    let mut best = (lo_e, lo_n, lo_r);
    let mut worst = (hi_e, hi_n, hi_r);
    while hi - lo > 12 {
        let mid = (lo + hi) / 2;
        match ratio_at(mid) {
            Some((e, n, r)) if r >= 1.0 => {
                best = (e, n, r);
                lo = mid;
            }
            Some((e, n, r)) => {
                worst = (e, n, r);
                hi = mid;
            }
            None => hi = mid,
        }
    }
    println!(
        "holds real time up to {} elements (n={}, {:.3}x); the next world up, \
         {} elements (n={}), runs at {:.3}x",
        best.0, best.1, best.2, worst.0, worst.1, worst.2
    );
}

// --------------------------------------------------------------- islands

/// One global matrix (today) vs one matrix per district (the unbuilt win).
fn islands_experiment(total: usize, dsize: usize, nonlinear: u32, cfg: &Cfg) {
    println!("\n=== ISLANDS: one world matrix vs one matrix per district ===");
    let params = GenParams::new(total, Structure::Districts { size: dsize }).nonlinear(nonlinear);
    let world = scale::generate(params);
    let specs = world.flat();
    let topo = scale::topology(&specs);
    println!(
        "world: {} elements in {} districts -> compile() builds ONE {}x{} matrix \
         ({} islands are all in it)",
        specs.len(),
        world.districts.len(),
        topo.unknowns,
        topo.unknowns,
        topo.islands
    );
    if topo.unknowns > cfg.max_n {
        println!("  global side skipped: n over --max-n");
        return;
    }

    let mut global = Engine::new(DT);
    let t = Instant::now();
    global.set_elements(&specs);
    let g_compile = t.elapsed().as_secs_f64() * 1e3;
    global.advance(4);
    let a = global.matrix().to_vec();
    let mut lu = DenseLu::new(global.unknowns());
    let g_factor = time_median(0.5, 25, || {
        lu.factor(&a);
    }) * 1e3;
    let (g_ops, _) = scale::lu_ops(&a, global.unknowns());
    drop((lu, a));
    let gs = measure_steps(&mut global, cfg);
    let g_step_us = gs.wall / gs.steps as f64 * 1e6;
    let g_nr = gs.nr_total as f64 / gs.steps as f64;

    // Per-district engines: exactly the same elements, partitioned.
    let mut engines: Vec<Engine> = Vec::with_capacity(world.districts.len());
    let t = Instant::now();
    for d in &world.districts {
        let mut e = Engine::new(DT);
        e.set_elements(d);
        engines.push(e);
    }
    let i_compile = t.elapsed().as_secs_f64() * 1e3;
    for e in engines.iter_mut() {
        e.advance(4);
    }
    let mut i_factor = 0.0;
    let mut i_updates = 0u64;
    let mut n_sum = 0usize;
    let mut n_max = 0usize;
    for e in engines.iter() {
        let a = e.matrix().to_vec();
        let mut lu = DenseLu::new(e.unknowns());
        i_factor += time_median(0.02, 200, || {
            lu.factor(&a);
        }) * 1e3;
        i_updates += scale::lu_ops(&a, e.unknowns()).0.updates;
        n_sum += e.unknowns();
        n_max = n_max.max(e.unknowns());
    }
    // Per-substep cost of the partitioned world = every island stepped once.
    let t = Instant::now();
    let mut sweeps = 0u32;
    let mut nr_max = 0u32;
    let mut nr_total = 0u64;
    while t.elapsed().as_secs_f64() < cfg.step_budget_s {
        for e in engines.iter_mut() {
            let r = e.advance(1);
            nr_max = nr_max.max(r.nr_iters);
            nr_total += r.nr_iters as u64;
        }
        sweeps += 1;
    }
    let i_step_us = t.elapsed().as_secs_f64() / sweeps as f64 * 1e6;
    let i_nr = nr_total as f64 / (sweeps as f64 * engines.len() as f64);

    println!(
        "  GLOBAL   n={} compile={:.2} ms  one factor={:.3} ms ({:.2} M row-updates)  \
         NR iters/substep mean={:.2} max={}  per substep={:.1} us  real-time={:.4}x",
        global.unknowns(),
        g_compile,
        g_factor,
        g_ops.updates as f64 / 1e6,
        g_nr,
        gs.nr_max,
        g_step_us,
        DT / (g_step_us / 1e6)
    );
    println!(
        "  ISLANDS  {} engines, sum n={} (max {})  compile={:.2} ms  \
         sum of all factors={:.3} ms ({:.2} M row-updates)  NR iters/substep/island \
         mean={:.2} max={}  per substep (all islands)={:.1} us  real-time={:.4}x",
        engines.len(),
        n_sum,
        n_max,
        i_compile,
        i_factor,
        i_updates as f64 / 1e6,
        i_nr,
        nr_max,
        i_step_us,
        DT / (i_step_us / 1e6)
    );
    println!(
        "  => factor {:.1}x cheaper, {:.1}x fewer row-updates, compile {:.1}x cheaper, \
         whole substep {:.1}x cheaper for electrically identical worlds \
         ({} global substeps, {} island sweeps timed)",
        g_factor / i_factor,
        g_ops.updates as f64 / i_updates.max(1) as f64,
        g_compile / i_compile,
        g_step_us / i_step_us,
        gs.steps,
        sweeps
    );
    println!(
        "  note: NR convergence is global too — one unconverged device makes the whole \
         world iterate again ({:.2} vs {:.2} iterations per substep above).",
        g_nr, i_nr
    );
}

// ------------------------------------------------------------- quiescence

/// How much of a realistic world is electrically static at any moment?
///
/// Measured per island (one `Engine` per district). The districts are
/// disconnected, so their voltages do not depend on being solved together;
/// per-island engines simply make a 1 s settle affordable, which the
/// global matrix at this size is not.
///
/// Read the result carefully. What is MEASURED is that a district containing
/// nothing that switches reaches a fully static DC state (no unknown moving
/// more than 1 uV per substep) and stays there, while a district containing an
/// oscillator or an AC source never does. What is ASSUMED is the mix: how many
/// of a real room's builds contain something that switches. The sweep over
/// `active_percent` below makes the dependence explicit — the static fraction
/// tracks `1 - active_percent` exactly, so the quiescence win is worth
/// whatever that fraction turns out to be in a real room, and the engine's own
/// contribution is that idle districts really do go completely still.
fn quiescence_sweep(total: usize, dsize: usize, nonlinear: u32, active: u32) {
    println!("\n=== QUIESCENCE: what fraction of the world is electrically static? ===");
    let mut actives = vec![0u32, 20, 50, 100];
    if !actives.contains(&active) {
        actives.push(active);
        actives.sort_unstable();
    }
    for a in actives {
        quiescence_experiment(total, dsize, nonlinear, a);
    }
}

fn quiescence_experiment(total: usize, dsize: usize, nonlinear: u32, active: u32) {
    let params = GenParams::new(total, Structure::Districts { size: dsize })
        .nonlinear(nonlinear)
        .active(active);
    let world: World = scale::generate(params);
    let mut engines: Vec<Engine> = world
        .districts
        .iter()
        .map(|d| {
            let mut e = Engine::new(DT);
            e.set_elements(d);
            e
        })
        .collect();
    let ns: Vec<usize> = engines.iter().map(|e| e.unknowns()).collect();
    let total_n: usize = ns.iter().sum();
    let cube: f64 = ns.iter().map(|n| (*n as f64).powi(3)).sum();
    // A district counts as static in a substep if no unknown moved by more
    // than STATIC_V: 1 uV per 20 us substep is 0.05 V/s, invisible to any
    // player and to every probe.
    const STATIC_V: f64 = 1e-6;
    const WINDOW: u32 = 500; // 10 ms observation window
    println!(
        "\n  --- active_percent={active} (ASSUMPTION): {} districts, {} built with a \
         switching block (relaxation oscillator or 1 kHz AC source), {} unknowns total, \
         {nonlinear}% nonlinear blocks",
        engines.len(),
        world.active.len(),
        total_n,
    );
    println!(
        "  static = no unknown moves more than {:.0e} V in a {} us substep, \
         for {} consecutive substeps (10 ms of sim time)",
        STATIC_V,
        DT * 1e6,
        WINDOW
    );
    println!("  | settled sim time | static districts | static unknowns | static share of sum(n_i^3) | mean per-substep static |");
    println!("  |---|---|---|---|---|");
    let mut last_moved = vec![true; engines.len()];
    let mut settled_ms = 0.0f64;
    for target_ms in [10.0f64, 50.0, 250.0, 1000.0] {
        let extra = ((target_ms - settled_ms) * 1e-3 / DT) as u32;
        for e in engines.iter_mut() {
            e.advance(extra);
        }
        settled_ms = target_ms;
        let mut moved = vec![false; engines.len()];
        let mut static_steps = 0u64;
        for _ in 0..WINDOW {
            for (i, e) in engines.iter_mut().enumerate() {
                let before = e.solution().to_vec();
                e.advance(1);
                let m = before
                    .iter()
                    .zip(e.solution().iter())
                    .any(|(a, b)| (a - b).abs() > STATIC_V);
                if m {
                    moved[i] = true;
                } else {
                    static_steps += 1;
                }
            }
        }
        settled_ms += WINDOW as f64 * DT * 1e3;
        let stat = |sel: &dyn Fn(usize) -> f64| -> f64 {
            ns.iter()
                .enumerate()
                .filter(|(i, _)| !moved[*i])
                .map(|(i, _)| sel(i))
                .sum()
        };
        let s_count = stat(&|_| 1.0);
        let s_n = stat(&|i| ns[i] as f64);
        let s_cube = stat(&|i| (ns[i] as f64).powi(3));
        // `+ 0.0` normalizes a -0.0 sum (empty filter) so an all-active world
        // prints "0%" rather than "-0%".
        let pct = |num: f64, den: f64| 100.0 * num / den + 0.0;
        println!(
            "  | {:.0} ms | {}/{} = {:.0}% | {:.0}% | {:.0}% | {:.0}% |",
            target_ms,
            s_count as usize,
            engines.len(),
            pct(s_count, engines.len() as f64),
            pct(s_n, total_n as f64),
            pct(s_cube, cube),
            pct(static_steps as f64, WINDOW as f64 * engines.len() as f64)
        );
        last_moved = moved;
    }
    // Instrument self-check: every district built with a switching block must
    // read as moving, or the measurement is not measuring what it claims.
    let active_moving = world.active.iter().filter(|i| last_moved[**i]).count();
    println!(
        "  self-check: {}/{} districts built with a switching block read as moving",
        active_moving,
        world.active.len()
    );
    let quiet_but_not_built_quiet = (0..engines.len())
        .filter(|i| last_moved[*i] && !world.active.contains(i))
        .count();
    println!(
        "  {} DC-only districts still moving after 1 s of sim time \
         (long reservoir-cap time constants, MOSFET NR damping)",
        quiet_but_not_built_quiet
    );
}

// --------------------------------------------------------------------- main

fn arg_val(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sizes: Vec<usize> = arg_val(&args, "--sizes")
        .unwrap_or_else(|| "500,1000,5000".to_string())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let dsize: usize = arg_val(&args, "--district")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let nonlinear: u32 = arg_val(&args, "--nonlinear")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let active: u32 = arg_val(&args, "--active")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let structures: Vec<String> = arg_val(&args, "--structures")
        .unwrap_or_else(|| "all".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let max_n: usize = arg_val(&args, "--max-n")
        .and_then(|s| s.parse().ok())
        .unwrap_or(6_000);
    // --skip may be repeated: --skip sizes --skip islands
    let skip: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "--skip")
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect();
    let cfg = Cfg {
        max_n,
        step_budget_s: arg_val(&args, "--step-budget")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.35),
        min_steps: arg_val(&args, "--min-steps")
            .and_then(|s| s.parse().ok())
            .unwrap_or(5),
        step_cap_s: arg_val(&args, "--step-cap")
            .and_then(|s| s.parse().ok())
            .unwrap_or(40.0),
        frame_max: arg_val(&args, "--frame-max")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8_000),
    };

    println!("# sim-core scale baseline");
    println!(
        "target {} / {}, profile: {} (release: opt-level 3, lto thin, no FMA)",
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "DEBUG - numbers are meaningless, rerun with --release"
        } else {
            "release"
        }
    );
    println!(
        "dt = {} us, tick = {} Hz -> {} substeps per tick (server MAX_STEPS_PER_TICK = 8000)",
        DT * 1e6,
        TICK_HZ,
        steps_per_tick()
    );
    println!("dense-matrix cap: n <= {max_n}\n");

    // Device mix of a representative world, so the shape is on the record.
    let sample = scale::generate(
        GenParams::new(
            *sizes.last().unwrap_or(&1000),
            Structure::Districts { size: dsize },
        )
        .nonlinear(nonlinear),
    );
    let flat = sample.flat();
    let (m, nl) = scale::mix(&flat);
    println!("device mix of the {}-element districts world:", flat.len());
    let parts: Vec<String> = m
        .iter()
        .map(|(k, c)| format!("{k} {:.1}%", 100.0 * *c as f64 / flat.len() as f64))
        .collect();
    println!("  {}", parts.join(", "));
    println!(
        "  nonlinear elements: {} = {:.1}% of the world\n",
        nl,
        100.0 * nl as f64 / flat.len() as f64
    );

    let mut md = Vec::new();
    if !skip.iter().any(|s| s == "sizes") {
        // The three variants each answer a different question, so they can be
        // selected individually: `districts` is what a real room looks like,
        // `one` is the worst case (everything electrically connected), and
        // `linear` isolates factor-once reuse from refactor-every-iteration.
        let want = |name: &str| structures.iter().any(|s| s == name || s == "all");
        for size in &sizes {
            if want("districts") {
                let p = GenParams::new(*size, Structure::Districts { size: dsize })
                    .nonlinear(nonlinear)
                    .active(active);
                if let Some(r) = measure(p, &cfg) {
                    print_row(&r);
                    md.push(md_row(&r));
                }
            }
            if want("one") {
                let p = GenParams::new(*size, Structure::One)
                    .nonlinear(nonlinear)
                    .active(active);
                if let Some(r) = measure(p, &cfg) {
                    print_row(&r);
                    md.push(md_row(&r));
                }
            }
            if want("linear") {
                let p = GenParams::new(*size, Structure::Districts { size: dsize })
                    .nonlinear(0)
                    .active(active);
                if let Some(r) = measure(p, &cfg) {
                    print_row(&r);
                    md.push(md_row(&r));
                }
            }
        }
    }
    if !skip.iter().any(|s| s == "kernel") {
        kernel_experiment(dsize, nonlinear);
    }
    if !skip.iter().any(|s| s == "crossover") {
        println!("\n=== REAL-TIME CROSSOVER: largest world that holds 1.0x today ===");
        for structure in [Structure::Districts { size: dsize }, Structure::One] {
            for nl in [nonlinear, 0] {
                crossover(structure, nl, active, &cfg);
            }
        }
    }
    if !skip.iter().any(|s| s == "islands") {
        islands_experiment(5_000, dsize, nonlinear, &cfg);
    }
    if !skip.iter().any(|s| s == "quiescence") {
        quiescence_sweep(2_000, dsize, nonlinear, active);
    }

    println!("\n=== markdown table (docs/scale-baseline.md) ===");
    println!("{MD_HEAD}");
    for l in md {
        println!("{l}");
    }
}
