# sim-gpu-spike

Phase-1 prototype from the GPU-accelerated-server design: a standalone,
dev-only benchmark that answers *"at what batch size does batched GPU dense
LU beat the sim-math CPU path, and is the precision survivable?"* It is NOT
in the server dependency graph and makes **no determinism claim** — the CPU
f64 path (`sim-math`/`sim-core`) remains the sole authoritative solver.

## Run

```sh
PATH="$HOME/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  cargo run --release -p sim-gpu-spike            # full grid, ~1 min
  cargo run --release -p sim-gpu-spike -- --quick # smoke run
```

## What it does

- Generates seeded, MNA-realistic batches: well-conditioned diagonally
  dominant systems (with forced pivoting) and node-conductance networks
  with log-uniform branch conductances + gmin diagonal, at two spans
  (moderate 1e-4..1e4 S, harsh 1e-12..1e6 S), n ∈ {8, 16, 32, 64}.
- Solves them on the GPU with a WGSL kernel: one workgroup per matrix,
  full LU with partial pivoting + triangular solve in workgroup shared
  memory (bank-conflict-padded tile), factors persisted for solve-only
  refinement passes. Precisions: f32 and df64 (double-single).
- Solves the same batches with `sim_math::DenseLu` (f64), single-thread
  and rayon-across-batch, as the CPU baselines and accuracy truth.
- Reports per-batch wall times across batch sizes {1, 16, 256, 4096}
  (dispatch+readback and 16-deep pipelined steady state), the crossover
  batch size per n, and f64 accuracy metrics (forward error vs truth,
  normwise backward error, 1e-6 gate count) for f32, f32 + CPU-f64
  iterative refinement, and df64.

## Headline findings (Apple M3 Ultra, 2026-08, wgpu 24 / Metal)

- Steady-state GPU f32 beats 28-thread rayon by 1.3–6.6x for n ≤ 32 at
  batch ≥ 256–4096, and beats single-thread CPU by 4–17x. At n = 64 the
  simple kernel **loses** to rayon by ~4x (wins 4x vs one core).
- Raw f32 forward error is ~1e-7 on well-conditioned systems, but up to
  26% on moderately conditioned MNA networks and unbounded on the harsh
  set — raw f32 is unusable for player-visible values, as predicted.
- f32 + CPU-f64 iterative refinement recovers ~1e-13 backward error on
  well-conditioned systems and rescues the moderate set partially; it
  diverges on the harsh set. A backward-error gate alone does NOT catch
  ill-conditioned garbage (backward error stays ~1e-8 while the solution
  is wrong by orders of magnitude) — any real gate needs a condition
  estimate or a CPU re-solve.
- **df64 is broken on wgpu/Metal**: wgpu (checked 24 and 26) compiles MSL
  with Metal's default `fastMathEnabled = true`, which reassociates away
  the error-free transformations. The identical algorithm in strict-IEEE
  Rust f32 reaches 1.5e-14 relative error; the GPU kernel returns plain
  f32 accuracy. Until wgpu exposes precise-math compilation, there is no
  usable df64 tier on Metal.
