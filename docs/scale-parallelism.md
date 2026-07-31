# Scaling the solver: parallelism, GPU, and what actually wins

Feasibility study for taking the authoritative simulation from today's
~140-element demo room to a 3,000–5,000-element world in real time.

**Verdict up front: GPU is a NO. Per-island partitioning is the whole
game, CPU threads are the cheap 4x on top, and the two biggest levers
after that are per-island multirate timesteps and skipping quiescent
islands. A sparse LU is worth building, but only for the case of one
large connected circuit — it does nothing for a world of small islands.**

Everything below is measured on the code at commit `976ec43`
(`wf/scale-gpu`, which merges `origin/main`). Benchmark sources:
`tools/scale-bench/` (CPU) and `tools/gpu-bench/` (wgpu). Neither is a
workspace member; neither is a dependency of any shipping crate.

---

## 0. Measurement conditions, and an honesty note

| | |
|---|---|
| Machine | Apple M4, 10 cores (4 performance + 6 efficiency), 24 GB, macOS 26.5.2 |
| GPU | Apple M4 integrated GPU, Metal backend via `wgpu` 30.0.0 |
| Toolchain | `rustc 1.95.0` (repo-pinned), profile `release` = `opt-level=3, lto="thin"` |
| Sim settings read from code | `DT = 20 µs`, `TICK_HZ = 30`, `substeps/tick = 1667`, `MAX_STEPS_PER_TICK = 8000` |

**The machine was shared.** Other agents in this workflow were running
servers, browsers and their own benchmarks throughout: load average was
8.3–9.0 on a 10-core box. Consequences, stated plainly:

* Absolute timings drift by up to **2.2x** between runs of identical code
  (dense LU at n=2048: 0.91 s in the quietest run, 2.12 s in the noisiest).
  Where a single number is quoted below it is the **minimum over all runs**
  — the closest available estimate of an uncontended core — and the
  observed range is given alongside.
* **The multicore scaling numbers are lower bounds.** Measuring rayon
  across 10 cores while 5–6 of them are executing someone else's work
  understates the speedup. Read "4.5x on 10 cores" as "at least 4.5x".
* Ratios measured inside a single process run (monolithic vs islands,
  serial vs rayon, sparse vs dense) are much more trustworthy than
  absolute times, and the argument below leans on ratios.

Reproduce:

```
cargo run --release --manifest-path tools/scale-bench/Cargo.toml -- all
cargo run --release --manifest-path tools/gpu-bench/Cargo.toml
```

---

## 1. The shape of the work

### 1.1 What the code actually does per substep

From `crates/sim-core/src/engine.rs`:

* One `Engine` owns **one dense `n x n` matrix** (`a: Vec<f64>`, row-major)
  and **one `DenseLu`**. There is no partitioning of any kind: `compile()`
  numbers every node in the document into a single unknown vector
  `[v_1..v_N, i_branch1..i_branchM]`, and `n = num_nodes + num_branches`.
* Wires add no unknowns (union-find closure), and ground is node 0, which
  is eliminated. **Every `Ground` element ties into the same node 0, so
  electrically separate boards are already block-diagonal blocks of one
  matrix** — the coupling is a fiction of the data layout, not physics.
* `solve_step` runs `1` iteration if `self.linear`, else up to
  `NR_MAX_ITERS = 100`. Each iteration calls `build()`, which for a
  nonlinear circuit zeroes all `n²` entries of `A`, re-stamps every
  element, and calls `lu.factor(&a)` — a full O(n³/3) factorization —
  then one O(n²) `solve`.
* `need_factor` is false only when `self.linear && factor_valid &&
  factored_h == h && factored_be == be`. `self.linear` is false if *any*
  element in the *whole world* is nonlinear (`is_nonlinear()`: diode,
  Zener, LED, BJT, MOSFET, op-amp, OTA, 555). **One diode anywhere forces
  a full re-stamp and re-factorization of the entire world matrix on every
  NR iteration of every substep.**
* Correction to the brief: `Motor` is **not** nonlinear at this commit. It
  is a *branch* device (`is_branch()`), and its back-EMF arrives as an
  RHS-only `ParamWrite::Bemf`, which "never refactors" (its own comment).
  `Timer555` *is* nonlinear.
* `step()` also allocates and copies a `Vec<ElemState>` snapshot of every
  element, every substep, for the rescue ladder.

Measured NR iterations per substep: **2.00** for the demo room and for
every tiled world tested; **1.00** for a diode-terminated RC feeder that
sits at its operating point. The rescue ladder (`RESCUE_DEPTH = 4`) can
multiply this by up to 32 on a bad step, and `NR_MAX_ITERS` bounds a bad
substep at 100 factorizations.

### 1.2 Today's demo room is already slower than real time

143 elements, n=50 (36 nodes + 14 branches), verbatim from
`crates/server/src/main.rs`:

| metric | measured |
|---|---|
| NR iterations / substep | 2.00 |
| time / substep | **30.5 µs** (range 30.5–42.3 over 15 timed runs) |
| CPU per 30 Hz tick (1667 substeps) | **50.8 ms** (range 50.8–70.5) |
| real-time factor | **0.66x** (range 0.47–0.66) |

The tick budget is 33.3 ms. The shipping showcase room needs 51–70 ms of
one core per tick, so **sim time is already dilated by 1.5–2.1x today**.
`MAX_STEPS_PER_TICK = 8000` never engages (the budget is
`min(1667, 8000)`); the dilation happens through `MissedTickBehavior::Delay`
letting the loop fall behind the wall clock. Any scaling plan starts from
a deficit, not from headroom.

### 1.3 The work per simulated second at 5,000 elements

Two completely different regimes. This distinction decides everything.

**Case A — one connected circuit.** Measured with an RC feeder ladder
(series R per span, shunt C per node; 2 elements per stage), which is the
*friendliest* possible large topology (near-tridiagonal):

| elements | n | nonlinear? | µs / substep | real-time factor |
|---|---|---|---|---|
| 103 | 52 | no | 5.4 | 3.7x |
| 503 | 252 | no | 95 | 0.21x |
| 1503 | 752 | no | 754 | 0.027x |
| 3003 | 1502 | no | **4,442** | 0.0045x |
| 104 | 53 | yes (one diode) | 16.2 | 1.24x |
| 504 | 253 | yes | 395 | 0.051x |
| 1504 | 753 | yes | 3,723 | 0.0054x |
| 3004 | 1503 | yes | **16,456** | 0.0012x |

A single connected 3,000-element circuit costs 4.4 ms/substep linear and
16.5 ms/substep with one diode in it — **220x and 820x over the 20 µs
budget**. Note the linear column still costs milliseconds despite reusing
the factorization: the O(n²) dense triangular solve alone is fatal
(measured `DenseLu::solve` at n=1024: 1.38 ms).

**Case B — many islands (the realistic world).** Tiling the 17 golden
circuits (~6.6 elements, n≈4 each):

| elements | islands | monolithic µs/substep | one-Engine-per-island µs/substep | island speedup |
|---|---|---|---|---|
| 113 | 17 | 63.9 | 7.35 | 8.6x |
| 565 | 85 | 1,723 | 40.6 | 41x |
| 1,130 | 170 | 7,091 | 82.3 | 95x |
| 2,260 | 340 | 32,261 | 170.0 | 154x |
| 4,520 | 680 | 169,692 | **280.8** | **467x** |

Monolithic scales as roughly E^2.7 (measured: 2,654x cost for 40x the
elements). Island-partitioned scales **linearly**: 0.062–0.075 µs per
element per substep across the whole range.

**Answer to the question.** At 5,000 elements in the island regime
(~760 islands of the golden mix, 10 of 17 kinds nonlinear), per simulated
second (50,000 substeps):

* ~**45 million refactorizations** of matrices of order **3–7**
  (447 nonlinear islands x 2 NR iterations x 50,000), plus ~16 M
  RHS-only solves for the linear islands.
* Arithmetic content: ~130 flops per island-substep → **~6 GFLOP/s**.
  That is *one* M4 core's measured scalar peak (6.3 GFLOP/s at n≥256), so
  the problem is not flops. It is per-element overhead and tiny-matrix
  inefficiency: the same `DenseLu` code runs at **0.24 GFLOP/s at n=3 and
  0.92 GFLOP/s at n=8** versus 6.3 GFLOP/s at n=256.
* Estimated total (from the measured 0.065 µs/element/substep, linear in
  elements): **~16 core-seconds of CPU per simulated second**, i.e. 16x
  over budget on one core. Labelled an estimate: it is a linear
  extrapolation from 4,520 measured elements to 5,000.

### 1.4 Latency-bound or throughput-bound?

Both, on different axes, and this is the crux:

* **Along time, it is a strictly serial latency chain.** Substep k+1 needs
  substep k's accepted state; NR iteration j+1 needs iteration j's
  solution. Per 30 Hz tick that is **1,667 dependent links** (3,334
  dependent solves at 2 NR iterations). Nothing can reorder them. The
  budget per link is 33.3 ms / 1667 = **20 µs**, and per solve **10 µs**.
* **Across islands, it is embarrassingly parallel throughput.** At 5,000
  elements there are ~760 arithmetically independent chains. With Bergeron
  corridors (plan resolution 3) the coupling is an exact one-dt delay, so
  islands need to exchange data at most once per substep — and for
  uncoupled boards, not at all.
* **For one connected circuit there is no width at all.** 1,667 dependent
  factorizations of one n=1,500 matrix per tick. The only parallelism
  available inside a single sparse LU of that size is fine-grained and
  wide-inefficient, and it collides head-on with bit-determinism (§4.2).

So: **a GPU can only ever help case B, and only if the per-link latency
fits in 20 µs.** That is exactly what §2 measures.

### 1.5 Two non-solver hotspots that will bite first

Measured on the tiled worlds:

| elements | `frame()` per call | `set_elements()` per call |
|---|---|---|
| 113 | 14 µs | 0.02 ms |
| 1,130 | 671 µs | 1.18 ms |
| 2,260 | 2.37 ms | 4.51 ms |
| 4,520 | **8.77 ms** | **20.7 ms** |

Both are ~O(E²) (linear `position()` searches inside per-element loops:
`compile()`'s junction interning, `solve_wire_currents()`'s `jix`).
`frame()` is called at least once per tick, and once per probe chunk when a
wire-current probe exists (`SAMPLE_EVERY = 16` → up to 104 calls/tick).
At 4,520 elements, `frame()` alone can consume 26x the tick budget. These
are pure data-structure problems (hash the junctions, cache the incidence
lists), cost about a day, and have zero determinism risk. They are not
"parallelism", but they will dominate before any solver work matters.

Also visible: the per-substep `Vec<ElemState>` snapshot in `step()` costs
**28.9 µs at 4,520 elements** (measured, 6.4 ns/element) — 1.4x the entire
substep budget by itself, and trivially removable by reusing one buffer.

---

## 2. GPU: batched small LU on wgpu

The only GPU-shaped workload here is "factor many independent island
matrices in one dispatch". Measured with a WGSL kernel doing one complete
n=8 LU + forward/back solve per invocation (`tools/gpu-bench/`), on the
M4's integrated GPU through Metal. The kernel reads its RHS from the same
buffer it writes, so chained dispatches are genuinely data-dependent —
the dependency the fixed timestep imposes.

### 2.1 Dispatch and synchronisation latency

| pattern | measured |
|---|---|
| empty kernel, 100 dispatches in one pass, one submit + wait | **3.11 µs / dispatch** |
| empty kernel, one submit + wait | 172 µs |
| LU kernel (batch 1024), 100 dependent dispatches in one pass | **32.1 µs / dispatch** |
| LU kernel, 100 separate submits, one wait | 42.3 µs / submit |
| LU kernel, single submit + wait | 350 µs |
| LU dispatch + copy + map-read 32 KiB back to the CPU | **268 µs round trip** |

The per-dispatch cost of a *dependent* LU dispatch is ~30 µs and is
almost independent of batch size below 4,096 (32.1 µs at batch 64, 32.1 µs
at batch 1,024, 36.0 µs at batch 4,096) — it is barrier/queue latency, not
work. The 3.11 µs empty-kernel figure is the floor for *independent*
dispatches.

### 2.2 Host↔device transfer

| payload | upload (write_buffer + submit + wait) | readback round trip |
|---|---|---|
| 256 f32 (1 KiB) | 190 µs | 217 µs |
| 1,000 f32 (4 KiB) | 226 µs | 221 µs |
| 5,000 f32 (20 KiB) | 233 µs | 222 µs |
| 20,000 f32 (78 KiB) | 256 µs | 226 µs |

Transfer is **pure latency at our sizes**: an RHS/solution vector for a
5,000-unknown world is 20 KiB and costs the same ~220 µs as 1 KiB. Even
one round trip per tick is 0.7% of the budget (fine); one per substep is
1,667 x 220 µs = **367 ms per tick, 11x over budget** (fatal).

### 2.3 Batched throughput, and where it beats the CPU

Same batch, n=8: GPU in f32, CPU in f64 via `sim_math::DenseLu` (so the
comparison already favours the GPU by a factor of ~2 in arithmetic width).

| batch | GPU µs/dispatch | GPU Msys/s | CPU 1 thread Msys/s | CPU 10 threads Msys/s | GPU / CPU-all-cores |
|---|---|---|---|---|---|
| 64 | 32.1 | 2.0 | 4.0 | 2.3 | 0.86x |
| 256 | 31.7 | 8.1 | 4.2 | 5.7 | 1.41x |
| 1,024 | 32.1 | 31.9 | 4.2 | 8.5 | 3.77x |
| 4,096 | 36.0 | 113.8 | 3.9 | 9.6 | 11.9x |
| 16,384 | 132.1 | 124.1 | 3.5 | 11.4 | 10.9x |
| 65,536 | 456.0 | 143.7 | 3.1 | 12.5 | 11.5x |

**Crossover: ~256 islands to beat all 10 CPU cores at all; ~4,000 islands
to reach the GPU's asymptotic ~11x.** For context, 256 islands is about
1,700 elements of the golden mix and 4,096 islands is ~27,000 elements —
5x past the target world.

### 2.4 The tick test, which is where it dies

One 30 Hz tick = 1,667 *dependent* substeps. Budget 33.3 ms.

| batch | all 1,667 dispatches in one pass, one wait | with a readback per substep | CPU serial (1 thread, same work) |
|---|---|---|---|
| 256 | **49.6 ms** | 434 ms | 105 ms |
| 1,024 | **50.0 ms** | 432 ms | 400 ms |
| 4,096 | **57.0 ms** | 443 ms | 1,593 ms |

Even in the most favourable possible configuration — every dispatch
pre-encoded in one pass, one submit, one synchronisation, no readback, no
stamping, no device models, f32 — **the GPU cannot finish a tick's
dependency chain in 33.3 ms.** 1,667 x 30 µs = 50 ms is a floor set by
dispatch latency, not by arithmetic. Add the readback that a CPU-side NR
convergence test or probe sampling needs and it is 13x over.

### 2.5 The only GPU architecture that could work, and its price

Amortising the dispatch latency requires **one dispatch per tick** with a
*persistent kernel* that loops all 1,667 substeps internally, keeping the
entire circuit state resident in GPU memory. That is arithmetically
plausible (measured 7 ns per island-substep at batch 65,536 vs ~65 ns/element
on a CPU core), but it requires:

* the stamping, all device models (`exp`, `tanh`, `pnjlim`), the
  integrator, the NR convergence test, the rescue ladder and the
  quarantine logic all rewritten in WGSL;
* **f32 only.** Verified at runtime: this adapter reports
  `SHADER_F64: false`, and `wgpu`'s `SHADER_F64` feature is documented as
  Vulkan/SPIR-V-only and native-only (`wgpu-types/src/features.rs`), so on
  Metal and on the web there is no f64 at all. Our engine uses
  `GMIN = 1e-12` on the diagonal and diode currents around `1.7e-7 A` with
  `exp(v/0.0517)`; f32 has ~7 decimal digits. A 1e-12 conductance next to
  a 1e-2 conductance is *below the f32 rounding of the diagonal it sits
  on*, so the floating-node conditioning the beginner-tolerant solver
  depends on simply disappears;
* emulating f64 in double-float (two-f32) arithmetic is possible but costs
  ~10-20x the operations *and* requires exactly-rounded add/mul with no
  FMA contraction — precisely the compiler behaviour GPU drivers do not
  guarantee (§3);
* Amdahl's ceiling anyway: at island scale the dense factor+solve is only
  **~40-75% of the per-substep cost** (a linear island with a reused
  factorization spends ~0.07 µs of its 0.23 µs total in `solve`; a
  nonlinear n=4 island spends ~0.33 µs of 0.44 µs in 2x factor+solve).
  Moving only the solver to the GPU caps the win at ~2-4x even with zero
  latency.

**Conclusion for §2: no.** Not "not yet" — the dispatch-latency floor is
1.5x the entire tick budget for a workload that is 50x too wide to exist
in our target world, in a precision we cannot use, for a component that is
at most 75% of the cost.

---

## 3. The determinism collision

### 3.1 What the repo enforces today, verified

`./tools/determinism.sh` was run on this branch: native arm64 and wasm32
state hashes are **bit-identical for all 15 golden circuits at 10,000
steps** (e.g. `demo_lamp 7fe9da43ef448d7b`). `cargo test --workspace`
(26 tests), `cargo clippy --workspace --all-targets` and
`cargo fmt --all --check` are clean.

The invariant survives because of a very specific discipline: no
`mul_add`, no fast-math, no SIMD, transcendentals only via the pure-Rust
`libm` crate, one hand-written scalar LU whose loop order is fixed by
source order, and NaN canonicalisation before hashing.

### 3.2 What GPU execution does to it

Every one of these breaks bit-identity, and they compound:

1. **No f64.** The GPU path is a different arithmetic to begin with; the
   results cannot match the native f64 path bit-for-bit, only
   approximately. This alone ends the discussion for an authoritative
   solver — it is not a subtle reduction-order issue, it is 24-bit vs
   53-bit mantissas.
2. **FMA contraction is the driver's choice.** WGSL has no contraction
   pragma. Metal, DXC/HLSL and the various Vulkan drivers each decide
   independently whether `a*b + c` becomes an FMA. Two vendors, or two
   driver versions, legitimately produce different last bits. Our `no
   mul_add` rule is unenforceable past the WGSL boundary.
3. **Fast-math defaults.** Shader compilers routinely apply
   reassociation, `x*(1/y)` for `x/y`, and denormal flushing. Metal
   defaults to fast-math for a lot of this; the WebGPU spec permits
   substantial numeric latitude and explicitly does not promise
   reproducibility across implementations.
4. **Reduction and scheduling order.** Any parallel reduction inside a
   factorization (a dot product across lanes, a subgroup sum, an atomic
   accumulation) has a nondeterministic summation order in general.
   Avoidable with care in a one-thread-per-matrix design; unavoidable
   for a parallel-within-one-matrix design.
5. **Backend and device variation.** wgpu targets Metal / Vulkan / DX12 /
   GL / WebGPU with *different shader compilers*. The same WGSL run under
   Metal on the server and WebGPU-on-Vulkan in a player's browser would
   have to agree bit-for-bit. Nothing in the stack promises that, and no
   CI we could build would keep it true across driver updates.
6. **Transcendental accuracy is unspecified.** WGSL `exp`, `tanh`, `log`
   have implementation-defined accuracy (a few ULP), and every diode in
   the game goes through `exp`. `libm`'s exact bit patterns cannot be
   reproduced without reimplementing `libm` in WGSL — the same fight we
   already won on the CPU by refusing platform libm.

### 3.3 The escape routes, with real costs

**(a) Drop bit-determinism; make the server the only truth.**
Cost: the plan's resolution 4 already says "netcode never depends on it;
client preview hard-reseeds from authoritative snapshots", so the *netcode*
survives. What dies is the engineering infrastructure:
`tools/determinism.sh` and its CI gate; op-log replay reproducing a bug
exactly; the ability to say "the client preview and the server agree";
cross-target golden hashes as a regression net (they currently catch any
accidental numeric change in 15 circuits at once, cheaply). You would
replace them with tolerance-based comparisons, which are much weaker —
a tolerance test passes a slow drift that a hash test catches instantly.
For a game whose pillar is "every number comes from the solver",
losing exact replay is a real loss of debuggability, and it buys nothing
by itself: the GPU still fails the latency test in §2.4. **Not
recommended, and note that it is not even sufficient.**

**(b) Integer / fixed-point solver.** Determinism becomes trivial and
target-independent, and integer ALUs are fast. But MNA matrices routinely
span 12+ decades in one system (`GMIN = 1e-12` S beside 1 S conductances;
1e-14 A saturation currents beside amps), Newton–Raphson on
`exp(v/0.0517)` needs enormous dynamic range near the knee, and the
pivot growth in LU has no a priori bound. You would be building your own
soft float (or block-scaled fixed point with per-island exponents) and
re-validating every device model and every golden circuit against it.
Realistic cost: **weeks to months**, with a real chance the accuracy is
worse than f64 and the speed is not better on modern hardware, which has
fast f64. **Not recommended.**

**(c) GPU for non-authoritative preview only; server stays scalar f64.**
This is the only version that is architecturally clean: the server's
authoritative sim stays exactly as it is (bit-deterministic, CI-gated),
and a GPU path could power *presentation* work — field visualisations,
thermal/glow shading, waveform rendering, an FFT for display. Note this is
where the GPU already belongs in the plan (S2 renderer spike), and that it
is explicitly *not* solver work. The design pillar "every number shown to
players comes from the solver" forbids using a GPU approximation to
produce displayed voltages, so a GPU *preview solver* would violate a
pillar even though it is allowed by the netcode. Restrict the GPU to
rendering.

**Recommendation: (c), narrowly — GPU for rendering only. Keep
bit-determinism. It costs us nothing that we could have spent on
performance, because the GPU cannot meet the latency requirement anyway.**

---

## 4. CPU parallelism — the boring answer that wins

### 4.1 rayon across islands

Measured on tiled worlds, one `Engine` per island, comparing serial and
parallel advance of the same set of engines in the same process run
(10 rayon threads on 10 contended cores):

| elements | islands | serial µs/substep | join per substep | speedup | join per 100 substeps | speedup |
|---|---|---|---|---|---|---|
| 113 | 17 | 6.5 | 27.0 | **0.24x** | 2.88 | 2.26x |
| 565 | 85 | 34.1 | 39.2 | 0.87x | 9.56 | 3.57x |
| 1,130 | 170 | 67.9 | 54.5 | 1.25x | 17.2 | 3.96x |
| 2,260 | 340 | 140.1 | 90.2 | 1.55x | 31.0 | **4.52x** |
| 4,520 | 680 | 293.9 | 171.0 | 1.72x | 67.4–92.5 | 3.2–4.9x |

With `RAYON_NUM_THREADS=4` (performance cores only) the coarse-join
speedup at 600 islands was **3.43x**; the 6 efficiency cores add only
~1.3x on top, which matches an M4's asymmetric core design.

Two conclusions:

* **Join granularity decides everything.** A rayon join costs ~20–27 µs
  here, and the entire substep budget is 20 µs. Parallelising *per
  substep* is a **4x slowdown** at demo scale and never exceeds 1.7x.
  Parallelising per *tick* (or per k=100 substeps) gives 3.5–4.9x. Since
  Bergeron corridors decouple islands with an exact one-dt delay, the
  natural design is: exchange boundary values once per substep but only
  *fork/join once per tick*, i.e. persistent per-island worker tasks with
  a barrier, not a `par_iter` per substep. If corridor coupling forces a
  real barrier per substep, use one barrier (~2–5 µs) rather than a
  fork/join, and expect much less than 4x.
* **Expected win: 3.5–5x**, and this is a lower bound given the contended
  measurement.

### 4.2 Does determinism survive? Yes, and it is measured

Per-island parallelism is bit-safe, and the argument is exact rather than
statistical:

1. Each island's matrix, RHS, factorization, NR loop and device state are
   disjoint memory. Nothing is shared, so no read-write race can change
   an operand.
2. Every floating-point operation therefore has the same operands, in the
   same order, as in the serial execution. IEEE-754 f64 with a fixed
   operation order is a pure function; thread scheduling cannot change a
   pure function's result.
3. Reductions across islands (which *would* be order-sensitive) do not
   exist in the sim: `state_hash()` iterates elements in a fixed order,
   and island coupling (Bergeron) exchanges values from the *previous*
   substep, i.e. a fixed dependency graph, not a live accumulation.
4. The one thing that must be pinned is **island identity and ordering**:
   the partition itself must be computed deterministically (sorted by a
   stable key, not by `HashMap` iteration order), or the same document
   would produce different island→engine assignments on different runs.

Measured check (`tools/scale-bench`, `islands` bench): 68 islands advanced
3,000 substeps each, once serially and once via `par_iter_mut`, then
`state_hash()` compared per island → **IDENTICAL** in every run.

Note the important corollary: **the hash of a partitioned world will not
match the hash of the same world solved monolithically.** Partitioning
changes the arithmetic (different matrix, different pivot order, no GMIN
coupling through a shared factorization). The determinism harness must be
re-baselined once, and from then on it is stable. That is a one-time cost,
not a lost invariant.

### 4.3 SIMD inside a factorization: blocked, and not worth fighting for

* The inner loop `lu[r*n+c] -= m * lu[k*n+c]` vectorises trivially, but a
  4-wide f64 SIMD version is only bit-identical to the scalar version if
  each lane performs exactly the multiply-then-subtract we do now, with no
  FMA and no reassociation. That is achievable in principle (NEON `fmul`
  + `fsub` are exactly-rounded), but not while staying portable: the
  wasm32 path would need `relaxed-simd`-free `simd128` with the same lane
  ordering, and the plan explicitly forbids relaxed SIMD. The CI grep for
  `fma`/`mul_add` would have to be replaced by a disassembly audit on
  every target.
* More importantly it is aimed at the wrong thing. Our matrices are
  **n=3–7 in the island regime**; there is no vector length there, and
  measured LU efficiency at that size is 0.24–0.92 GFLOP/s — the loss is
  loop and branch overhead, not lack of SIMD.
* The determinism-safe version of this optimisation is **specialised
  straight-line kernels for n ≤ 8** (fully unrolled LU+solve, fixed
  operation order, no pivot search where the diagonal is known dominant).
  Fixed order means bit-identical by construction, and closing even half
  of the 0.9 → 6.3 GFLOP/s gap is a *bigger* win than SIMD would give.
  Estimated 1.5–3x on the solver component of small islands (estimate:
  based on the measured 7–26x efficiency gap between n≤8 and n≥256 in the
  same code, of which unrolling can plausibly recover part).

### 4.4 Multiple sim threads per room / rooms per box

Each room already owns an independent `Engine`; rooms share nothing.
Scaling rooms across cores is free and exactly as deterministic as one
room (same argument as §4.2). The measured 68-engine parallel run *is*
this experiment in miniature. This does nothing for a single big room,
which is the actual problem, but it means server capacity planning is a
non-issue: ~4–5 concurrent heavy rooms per 10-core box at today's
per-room cost.

---

## 5. Algorithmic alternatives, ranked

Ranking metric: (expected speedup x confidence) / effort. Effort in
engineer-days, assuming this codebase's existing test and determinism
infrastructure.

### 5.1 Per-island partitioning — **measured 8.6x to 467x, 3–5 days**

Split the document into connected components (union-find over
non-`Ground` connectivity — the code that computes it already exists in
`compile()`), one `Engine`-like solver context each. Measured above:
linear-in-elements cost instead of E^2.7, 467x at 4,520 elements. Also
turns `self.linear` into a per-island property, so one diode stops
poisoning the whole world's factor-reuse.

Determinism: safe (§4.2), but re-baselines the golden hashes once.
Confidence: **very high** — measured end-to-end with the real engine, not
modelled. This is the single highest-value change in this document and it
is not parallelism at all; it is a data-structure change.

### 5.2 Skipping quiescent islands — **measured 62% of cost, 2–4 days**

Measured after 0.2 s of settling on the 17 golden circuits: **13 of 17
have `max|Δv| < 1e-9 V` per substep** (a DC operating point that is not
moving). Weighted by measured per-island cost, the four active islands
(rectifier, relaxation oscillator, OTA VCO, 555) account for only
**38% of the total cost** — so freezing quiescent islands removes ~62% of
the work, a **2.6x** speedup on this mix.

Mechanics: an island is frozen when its state change per substep is below
tolerance; it wakes on any edit, interact, `ParamWrite`, or corridor
boundary change beyond a threshold. Cost is one cheap norm per island per
substep (or per k substeps).

Determinism: this *changes the numbers* (a frozen island stops
accumulating GMIN-level drift), so it must be a deliberate, documented,
tolerance-defined semantics — deterministic (the same threshold test on
the same state gives the same decision on every target) but a new
baseline. Confidence: high on the cost fraction (measured), medium on the
game-mix generality (the golden set is DC-heavy; a world full of
oscillators saves less).

### 5.3 Per-island multirate timesteps — **measured 25–100x on 11 of 17 islands, 5–8 days**

Measured trajectory error at t = 0.2 s versus the dt = 20 µs reference,
for dt multiples k:

| island | k=2 | k=5 | k=25 | k=100 |
|---|---|---|---|---|
| demo_lamp, emitter_follower, nmos_switch, opamp_follower, opamp_comparator, zener_regulator, pot_divider, led_loop | 0 | 0 | 0 | 0 |
| rc_step | 1.6e-13 | 2.4e-13 | 2.4e-13 | 2.4e-13 |
| rl_step | 6.8e-16 | 1.3e-15 | 1.1e-15 | 3.3e-11 |
| npn_switch | 1.8e-16 | 5.7e-15 | 2.7e-15 | 0 |
| motor_step | 1.4e-14 | 2.8e-14 | 2.8e-14 | 2.8e-14 |
| rlc_ring | 1.0e-15 | 1.2e-15 | 8.1e-4 | 2.3e-4 |
| half_wave_rectifier | 7.5e-5 | 6.0e-4 | 2.9e-2 | 9.2e-1 |
| opamp_relaxation | 1.0e+1 | 1.2e-1 | 1.6e-1 | 7.3e-1 |
| ota_vco | 2.5e0 | 3.7e0 | 4.0e0 | 2.0e0 |
| timer555_astable | 1.2e0 | 7.7e0 | 7.7e0 | 1.6e0 |

**12 of 17 islands are bit-quiet or sub-nanovolt at dt = 2 ms (k=100)** —
they are being stepped 100x more often than their physics requires. The
rectifier tolerates k=5 (0.6 mV); the three self-oscillating islands show
large numbers that are *phase* error, not amplitude error (the metric
compares instantaneous voltages, so a 1 Hz square wave that has drifted a
few ms reads as a 10 V "error"). Those need either k=1 or a proper
local-error controller.

Because plan resolution 1 already specifies `local_dt` as k-step multiples
of the room dt, this is a sanctioned design. Combined with §5.2 the two
overlap heavily (the quiescent islands are mostly the dt-tolerant ones):
**do not multiply the two speedups.** Together they take the measured
cost-weighted work to ~40% of today's, i.e. ~2.5x, and they make the
*worst case* (a world of oscillators) no worse.

Determinism: safe *if* k is chosen by a deterministic rule from
deterministic state (e.g. a quantised local-error estimate), never from a
wall clock or a load measurement. This is the one place where a "make it
adaptive to CPU load" instinct would destroy determinism, and it must be
forbidden in review.

### 5.4 Fixed-pattern sparse LU (KLU-style) — **measured 39–309x, but only for big connected islands, 8–12 days**

I built a prototype (`tools/scale-bench/src/sparse.rs`): exact greedy
minimum-degree ordering on the pattern of A+Aᵀ with node rows eliminated
before voltage-source branch rows, symbolic fill once, then numeric
refactor over the frozen pattern with no pivot search — KLU's division of
labour (analyze once per edit, refactor per NR iteration).

| matrix | n | nnz(A) | nnz(L+U) | fill | analyze (once) | refactor | solve | dense factor | dense/sparse |
|---|---|---|---|---|---|---|---|---|---|
| RC ladder | 101 | 300 | 301 | 1.00x | 0.07 ms | 1.60 µs | 1.06 µs | 35.6 µs | **13x** |
| RC ladder | 501 | 1,500 | 1,501 | 1.00x | 0.50 ms | 9.2 µs | 5.6 µs | 954 µs | **65x** |
| RC ladder | 2,001 | 6,000 | 6,001 | 1.00x | 4.6 ms | 42.7 µs | 34.7 µs | 15.2 ms | **196x** |
| 2-D mesh 10x10 | 101 | 462 | 1,151 | 2.49x | 0.23 ms | 7.5 µs | 1.7 µs | 45.4 µs | 5.0x |
| 2-D mesh 20x20 | 401 | 1,922 | 7,133 | 3.71x | 1.7 ms | 69.7 µs | 9.2 µs | 876 µs | 11x |
| 2-D mesh 45x45 | 2,026 | 9,947 | 55,898 | 5.62x | 24 ms | 1,017 µs | 68 µs | 42.3 ms | **39x** |
| 220 islands x 9 | 2,200 | 5,940 | 6,160 | 1.04x | 5.7 ms | 37.7 µs | 31.0 µs | 18.2 ms | **266x** |

Accuracy versus the dense solve on the same matrix: max relative
difference 1.4e-14 … 1.9e-11; residual `‖Ax−b‖∞/‖b‖∞ ≤ 1.3e-13`.

Two things this table says loudly:

* **Circuit sparsity is special, and that is why KLU exists.** On
  ladder/feeder and block-diagonal structures the factorization has
  *zero fill* (nnz(L+U) = nnz(A)) and one refactor is 2,000
  multiply-subtracts for n=2,001 — versus 2/3·n³ ≈ 5.3e9 for dense. Even
  a 2-D copper plane, the worst realistic topology, only fills 5.6x. This
  is precisely the regime KLU (Davis & Palamadai Natarajan, *Algorithm
  907: KLU*, ACM TOMS 37(3), 2010) was designed for: circuit matrices stay
  so sparse under BTF + AMD ordering that supernodal/BLAS-3 solvers lose
  to a scalar left-looking Gilbert–Peierls factorization, and refactoring
  with a frozen pivot sequence is far cheaper than a fresh factorization
  with pivot search. Those are the published design claims; my numbers
  above are my own and are against *our* dense LU, not against
  SuperLU/UMFPACK.
* **It is the wrong tool for the world we are actually building.** In the
  island regime the matrices are n=3–7, where the dense path is already
  fine and sparse bookkeeping would be pure overhead. Sparse LU matters
  for the *one big connected island* case: a district-wide power grid, a
  long corridor run, a player's 500-part board. It converts §1.3 case A
  from 16.5 ms/substep to an estimated ~0.2–0.4 ms/substep (refactor 43 µs
  + solve 35 µs + measured stamping ~150 µs at 3,000 elements) — a
  **~40–80x** improvement that still leaves it ~10–20x over the real-time
  budget, so it must be combined with multirate.

Determinism: the prototype is *structurally* deterministic — the pattern
fixes the operation sequence, there is no pivot search, no reassociation,
no FMA, no transcendentals. Unverified across targets: the S3 spike must
add it to `tools/determinism.sh`. Two real risks: (i) the ordering
algorithm must be deterministic (mine is: exact min-degree with
lowest-index tie-breaking; AMD implementations must be checked for
hash/set iteration order); (ii) no-repivot numeric failure needs the
dense fallback the plan already specifies.

Caveat on the comparison: our shipped `DenseLu` skips the row update when
the multiplier is exactly zero, so it is already semi-sparse on structured
matrices — the "dense factor" column is that optimised behaviour, not a
strawman.

### 5.5 Partial refactorization (touch only rows whose devices moved) — **speculative, 5–10 days**

Only the rows of nonlinear devices change between NR iterations. In a
sparse LU you can in principle refactor only the parts of L/U that depend
on changed entries (the "reachable" columns in the elimination tree), or
use a Woodbury/Sherman–Morrison update for a handful of changed
conductances. Expected win: on an island with 1 diode among 500 elements,
the changed set is tiny, so the refactor could approach the cost of a
solve. **I did not measure this**, and I flag two reasons for caution: our
NR converges in 2 iterations, so the total addressable saving is one
refactor per substep (already only 43 µs at n=2,001 in §5.4); and updating
a factorization while preserving bit-determinism means the update path
must produce the same bits as a full refactor, or the state hash depends
on *which* path ran, which is a nasty class of bug. Low priority.

### 5.6 Ranked table

| # | change | measured/est. speedup | confidence | effort (days) | determinism |
|---|---|---|---|---|---|
| 1 | **Per-island partitioning** | 8.6x @113 el → **467x @4,520 el** (measured) | very high | 3–5 | safe, re-baseline hashes |
| 2 | **Fix the O(E²) hotspots** (`frame`, `compile`, state snapshot) | removes 8.8 ms/tick and 20.7 ms/edit at 4,520 el (measured) | very high | 1–2 | none |
| 3 | **Quiescent-island skip** | **2.6x** cost-weighted (measured) | high | 2–4 | new tolerance semantics |
| 4 | **Per-island multirate dt** | 25–100x on 12/17 islands; ~2.5x combined with #3 (measured) | high | 5–8 | safe if k is state-derived |
| 5 | **rayon over islands, per-tick joins** | **3.5–4.9x** (measured, lower bound) | high | 3–5 | proven bit-identical |
| 6 | **Fixed-pattern sparse LU** | 39–309x on n≥400 connected matrices (measured prototype) | high for big islands, irrelevant for small | 8–12 | needs a determinism gate |
| 7 | **Unrolled n≤8 solve kernels** | 1.5–3x on the solver part of small islands (estimate) | medium | 2–4 | safe (fixed order) |
| 8 | Partial refactorization | ≤2x of the refactor component (estimate) | low | 5–10 | risky |
| 9 | SIMD inside the factorization | ~2-3x on large dense factors only (literature-level expectation, not measured) | low | 5–10+ | hostile |
| 10 | **GPU batched LU** | 0.86–11.5x on the LU alone, but **1.5x over the tick budget in latency** (measured) | n/a — fails | 20+ | fatal |

---

## 6. Verdict and roadmap

**What makes a 3,000-element world real time:** per-island partitioning,
then the O(E²) housekeeping fixes, then quiescent-island skipping and
per-island multirate dt, then rayon with per-tick joins. All four are
CPU-side, all four preserve bit-determinism, and the measured chain gets
there:

Estimated budget for 5,000 elements (measured inputs, arithmetic mine):

| stage | core-seconds of CPU per simulated second | basis |
|---|---|---|
| today, monolithic | ~11,500 (extrapolated) | measured 169.7 ms/substep at 4,520 el |
| + per-island partitioning | **16.3** | measured 0.065 µs/element/substep |
| + quiescence & multirate (they overlap) | **6.5** | measured 38% active cost fraction |
| + rayon, per-tick joins | **1.4** | measured 4.5x, a lower bound on a contended box |
| + unrolled small-n kernels (estimate) | ~0.9 | estimate, §4.3 |

The last row crosses 1.0, i.e. real time, with little margin — and 3,000
elements (0.85 core-seconds/s before the last row) crosses it comfortably.
So: **3,000 elements is reachable; 5,000 is reachable but wants the
tiny-kernel work too.** Both without a GPU and without touching
determinism.

**What is premature:** the GPU (fails on latency, precision and Amdahl,
all measured), SIMD inside the factorization (wrong problem size, hostile
to the invariant), partial refactorization (saving a component that is
already small), and — this is the surprising one — **the sparse LU as a
scale lever for the game world**. It is a 39–309x win on connected
matrices of n≥400, but a world of small boards never builds such a matrix
once §5.1 lands. Build it when a player actually builds a 500-part board
or a district-wide grid, which is also when the plan's S3 gate applies.

### Ordered roadmap with gates

**Step 1 — Per-island partitioning (3–5 d).**
Split by connected component (excluding ground), one solver context per
island, `linear`/`factor_valid` per island, deterministic island ordering
by a stable key. Re-baseline `tools/determinism.sh`.
*Gate to step 2:* the tiled 20-copy world (2,260 elements) advances at
≤ 200 µs/substep single-threaded (measured today: 170 µs for the
equivalent per-island run vs 32.3 ms monolithic), and native/wasm32 hashes
match on the new baseline.

**Step 2 — Kill the O(E²) housekeeping (1–2 d).**
Hash-map junction interning in `compile()`, cached incidence lists for
`solve_wire_currents()`, one reused `ElemState` snapshot buffer in
`step()`.
*Gate to step 3:* `frame()` ≤ 200 µs and `set_elements()` ≤ 2 ms at 4,520
elements (measured today: 8.8 ms and 20.7 ms).

**Step 3 — Quiescent-island freeze + per-island `local_dt` (7–12 d).**
Deterministic activity metric and k-multiple local timesteps with a
state-derived rule; wake on edit/interact/corridor delta.
*Gate to step 4:* the 40-copy world (4,520 elements) advances at
≤ 120 µs/substep single-threaded (measured today: 281 µs), with the
oscillator islands still matching their k=1 waveforms to within the
documented tolerance, and the determinism harness green.

**Step 4 — Thread the islands (3–5 d).**
Persistent per-island workers with a **once-per-tick** fork/join (never
`par_iter` per substep — measured 0.24x at demo scale), Bergeron boundary
exchange between substeps inside the worker loop.
*Gate to step 5:* ≥ 3x speedup on 8 threads at 4,520 elements on an idle
box, and per-island `state_hash()` identical to the serial run (the check
already exists in `tools/scale-bench`).

**Step 5 — Only now, and only if a real circuit demands it: the fixed-pattern
sparse LU (8–12 d, the plan's S3).**
Trigger: any single island exceeds n ≈ 150–200 in a real playtest room.
Ship it behind the existing dense fallback and add it to the determinism
harness.
*Gate:* n=500 refactor + solve < 50 µs native (the plan's own S3 number;
my prototype measures 9.2 + 5.6 = 14.8 µs on a ladder and would need
re-measuring on a real island's pattern), residual < 1e-12, bit-identical
native/wasm32.

**Not on the roadmap:** GPU compute for the solver. Revisit only if the
architecture changes so that a tick becomes one dependent step instead of
1,667 — i.e. if dt rises to ~1 ms for everything, which would be a
different game.

---

## Appendix A — raw benchmark output

Regenerate with the two commands in §0. Full captured runs used in this
document:

* dense LU, `state_hash` snapshot cost: §1.5, §1.3, §4.3
* demo room, feeder ladder, tiled worlds, housekeeping: §1.2, §1.3, §1.5
* island partitioning, rayon, quiescence, multirate: §4.1, §4.2, §5.2, §5.3
* sparse LU prototype: §5.4
* wgpu dispatch/transfer/batch/tick: §2.1–§2.4

## Appendix B — what I did not measure

* **Bergeron corridor coupling cost.** Islands were measured fully
  decoupled. Corridor exchange adds a per-substep boundary update and
  (if implemented as a barrier) a synchronisation cost I estimate at
  2–5 µs per substep for a barrier across 8 threads — unmeasured, and it
  would eat 10–25% of the substep budget. This is the main risk to the
  step-4 gate.
* **The sparse prototype on real engine matrices.** It was fed
  synthetically stamped matrices with the same stamping rules as
  `engine.rs` (verified by reading `stamp_g`/branch stamping), not
  matrices extracted from a live `Engine` (which has no accessor for `A`).
  Structures are representative; values are not identical.
* **Cross-target determinism of the sparse prototype.** Structurally
  deterministic by construction (§5.4) but not run through
  `tools/determinism.sh`.
* **Discrete GPU behaviour.** All GPU numbers are Apple M4 integrated /
  Metal. A discrete PCIe GPU would have *worse* transfer latency and
  similar or worse dispatch latency, so the §2.4 conclusion does not
  soften; a Vulkan box could enable `SHADER_F64`, at 16–64x the f32 cost
  per wgpu's own documentation, which does not rescue it either.
* **Unrolled small-n kernels.** §4.3's 1.5–3x is an estimate from the
  measured efficiency gap between n≤8 and n≥256 in the same code, not a
  prototype.
