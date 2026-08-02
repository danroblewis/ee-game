# Scale baseline — where the solver time actually goes

Measurement only. Nothing was optimised to produce this document; it is the
yardstick later solver decisions get judged against.

## Provenance

| | |
|---|---|
| machine | Apple M4, 10 cores, 24 GB RAM, macOS 26.5.2 |
| target | `aarch64-apple-darwin` |
| build | `--release` (opt-level 3, lto thin, no FMA, no SIMD) |
| toolchain | rustc 1.95.0, pinned by `rust-toolchain.toml` |
| branch | `wf/scale-bench` |
| sim config | dt = 20 µs, tick = 30 Hz → 1,666.7 substeps/tick, **50,000 substeps per simulated second** (matches `crates/server/src/main.rs`) |

Every number below is a wall-clock measurement on that machine unless it is
explicitly labelled an estimate. Two caveats, stated up front:

- The **20,000-element one-circuit row** and nothing else in the tables was
  measured while another single-threaded benchmark intermittently shared the
  machine. Treat its wall times as an upper bound. (It is 0.0000x real time
  either way; a 2x error changes no conclusion.)
- The **50,000-element one-circuit row was not measured.** Estimate, with its
  basis: the measured op count for the connected worlds is 62% of a full
  `n^3/3`, and the measured rate at n=4,873 was 16.3 s per factor; at
  n=12,173 that is ~15 minutes per factor and ~2 hours for a single row.
  Its topology (n=12,173, one island, 2.37 GB for one dense `A`) *was*
  measured, by compiling the world without factoring it.

## How to re-run

```
cargo run --release -p sim-golden --bin scale
```

67 s measured on this machine: 500/1000/5000 elements, the LU kernel
comparison, the real-time crossover search, and the islands and quiescence
experiments. The exact invocations behind the tables below:

```
cargo run --release -p sim-golden --bin scale -- --sizes 500,1000,5000,20000 --max-n 6000
cargo run --release -p sim-golden --bin scale -- --sizes 2000 --structures districts,linear \
    --skip crossover --skip islands --skip quiescence
cargo run --release -p sim-golden --bin scale -- --sizes 50000 --structures districts,linear \
    --max-n 14000 --min-steps 2 --step-cap 30 --frame-max 0 \
    --skip kernel --skip crossover --skip islands --skip quiescence
cargo run --release -p sim-golden --bin scale -- --skip sizes --skip kernel
```

Code: generator `crates/sim-golden/src/scale.rs`, harness
`crates/sim-golden/src/bin/scale.rs`, generator properties asserted in
`crates/sim-golden/tests/scale.rs` (determinism, island structure, agreement
with `Engine::compile`, LU op-counter equals `sim-math` bit-for-bit, and that
the worlds simulate without quarantining).

## What is being simulated

The generator builds *game-shaped* worlds, not academic matrices: a world is a
set of player builds ("districts"), each a small schematic hung between a
supply rail and a ground return, drawn with the wire-heavy sloppiness of a real
player. Two knobs matter:

- **island structure** — `districts~100` = electrically disconnected builds of
  ~100 elements each (what a real room looks like); `one-circuit` = a single
  connected mega-circuit (worst case).
- **nonlinear population** — 30% of blocks come from the nonlinear pool (LED
  indicator, rectifier, BJT low-side switch, NMOS switch, op-amp buffer,
  op-amp relaxation oscillator), which works out to **4.5–4.7% of elements
  being nonlinear devices**. A `0%` variant gives a genuinely linear world,
  which isolates factor-once reuse from refactor-every-NR-iteration.

Measured device mix of the 20,495-element districts world:

```
wire 47.8%, resistor 29.0%, cap 9.1%, inductor 4.0%, switch 3.6%, opamp 1.1%,
vsource 1.1%, ground 1.0%, npn 0.9%, nmos 0.9%, diode 0.8%, led 0.8%
```

Construction is deterministic (fixed LCG in the generator only; no RNG ever
enters `sim-core`). Two substitutions, because this branch's engine lacks the
devices: the 555 astable is built as an op-amp Schmitt trigger + RC integrator
(which is what a 555 astable *is*, electrically), and the DC motor is an R+L
series load with a snubber. Everything else is a real engine device.

## The table

`refactor share` = (measured isolated factor cost) × (refactorizations per
substep, counted in the engine) ÷ (measured substep). `solve share` likewise.
The isolated kernel timings re-factor one matrix in a tight loop, so their
cache state is a best case and the two shares can sum slightly over 100%.

| elements | structure | islands | n | nnz | nnz/row | LU fill | compile ms | factor ms | solve µs | µs/substep | NR mean | NR max | refactor/sim-s | refactor share | solve share | real-time | tick budget |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 516 | districts~100 | 5 | 136 | 375 | 2.76 | 4.3x | 0.14 | 0.050 | 11.5 | 71.9 | 1.05 | 3 | 52576 | 73% | 17% | **0.2782x** | 359% |
| 500 | one-circuit | 1 | 124 | 353 | 2.85 | 28.3x | 0.11 | 0.091 | 10.0 | 115.0 | 1.04 | 3 | 51758 | 82% | 9% | **0.1738x** | 575% |
| 515 | districts, linear | 5 | 132 | 348 | 2.64 | 1.0x | 0.13 | 0.049 | 11.2 | 18.4 | 1.00 | 1 | 0 | 0% | 61% | **1.0868x** | 92% |
| 1023 | districts~100 | 10 | 265 | 733 | 2.77 | 2.8x | 0.48 | 0.245 | 53.7 | 374.0 | 1.19 | 3 | 59730 | 78% | 17% | **0.0535x** | 1870% |
| 1003 | one-circuit | 1 | 248 | 706 | 2.85 | 51.4x | 0.43 | 0.870 | 47.6 | 1478.5 | 1.48 | 3 | 74153 | 87% | 5% | **0.0135x** | 7392% |
| 1028 | districts, linear | 10 | 246 | 653 | 2.65 | 1.0x | 0.48 | 0.214 | 47.8 | 66.3 | 1.00 | 1 | 0 | 0% | 72% | **0.3016x** | 332% |
| 2040 | districts~100 | 20 | 535 | 1488 | 2.78 | 2.5x | 2.02 | 1.400 | 290.5 | 3508.2 | 2.02 | 3 | 100758 | 80% | 17% | **0.0057x** | 17541% |
| 2055 | districts, linear | 20 | 491 | 1306 | 2.66 | 1.0x | 1.99 | 1.083 | 232.6 | 308.3 | 1.00 | 1 | 0 | 0% | 75% | **0.0649x** | 1542% |
| 5122 | districts~100 | 50 | 1348 | 3755 | 2.79 | 2.4x | 10.17 | 7.011 | 1489.4 | 17026.6 | 2.08 | 3 | 103846 | 86% | 18% | **0.0012x** | 85133% |
| 5004 | one-circuit | 1 | 1219 | 3492 | 2.86 | 276.0x | 8.68 | 120.933 | 1420.3 | 281050.3 | 2.20 | 3 | 110000 | 95% | 1% | **0.0001x** | 1405252% |
| 5132 | districts, linear | 50 | 1215 | 3235 | 2.66 | 1.0x | 9.69 | 6.113 | 1299.0 | 1650.0 | 1.00 | 1 | 0 | 0% | 79% | **0.0121x** | 8250% |
| 20495 | districts~100 | 200 | 5333 | 14777 | 2.77 | 2.2x | 168.81 | 133.025 | 42283.7 | 451420.7 | 2.40 | 3 | 120000 | 71% | 22% | **0.0000x** | 2257103% |
| 20001 | one-circuit † | 1 | 4873 | 13971 | 2.87 | 1108.2x | 115.54 | 16345.648 | 71578.2 | 52525651.3 | 3.00 | 3 | 150000 | 93% | 0% | **0.0000x** | 262628257% |
| 20525 | districts, linear | 200 | 4897 | 13010 | 2.66 | 1.0x | 210.29 | 188.543 | 49798.0 | 85694.4 | 1.00 | 1 | 0 | 0% | 58% | **0.0002x** | 428472% |
| 51252 | districts~100 | 500 | 13336 | 37036 | 2.78 | 2.3x | 1261.03 | 859.694 | 286390.0 | 3555807.1 | 2.50 | 3 | 125000 | 60% | 20% | **0.0000x** | 17779036% |
| 50005 | one-circuit | 1 | 12173 | — | — | — | — | not measured (see Provenance) | | | | | | | | | |
| 51355 | districts, linear | 500 | 12324 | 32706 | 2.65 | 1.0x | 822.12 | 700.964 | 242179.2 | 405338.4 | 1.00 | 1 | 0 | 0% | 60% | **0.0000x** | 2026692% |

† measured under intermittent CPU contention; upper bound.

`frame()` (the per-tick render pass the server calls when probes exist),
measured on the same worlds: 0.13 ms at 516 elements, 0.44 ms at 1,023,
2.16 ms at 2,040, 8.22 ms at 5,122, and **42.36 ms at 5,004 one-circuit**
elements — already over a whole 33 ms tick. Above 8,000 elements it was
skipped rather than measured.

## The largest world that holds real time today

Bisected on element count, each candidate measured exactly like the table
rows. Machine quiet for this run.

| structure | nonlinear | holds real time up to | next world up |
|---|---|---|---|
| districts~100 | 4.7% of elements | **205 elements** (n=52, 1.087x) | 309 elements (n=78) → 0.457x |
| districts~100 | none (linear) | **414 elements** (n=105, 1.053x) | 515 elements (n=132) → 0.699x |
| one-circuit | 4.7% of elements | **245 elements** (n=56, 1.035x) | 256 elements (n=59) → 0.936x |
| one-circuit | none (linear) | **495 elements** (n=113, 1.027x) | 507 elements (n=115) → 0.991x |

(One run, machine otherwise idle: `--skip sizes --skip kernel`.)

The bisection is on element count, so its resolution is one bracket step and
its result moves with machine noise. Across three runs the nonlinear boundary
landed at 205, 205 and 226 elements, and the linear one at 311, 414 and 495 —
so read these as **~200 and ~400**, not to three digits.

**Plainly: on an Apple M4 in release, today's engine holds real time up to
about 200 elements with a realistic nonlinear mix, and about 400 elements
if the world is perfectly linear.** The existing demo world is ~150 elements,
i.e. the shipping configuration is already within ~30% of its ceiling. The
owner's "simple game" target of ~3,000 elements is roughly **15x** past it, and
the sweep shows the gap is worse than linear: 2,040 elements runs at 0.0057x,
so ~175x too slow, not 15x.

Two adjustments worth keeping in mind when quoting these numbers:

- `docs/plan.md` resolution 1 puts the **nominal** dt at 10 µs (20 µs is the
  cheap end of the configurable band) and resolution 2 puts the tick at 60 Hz.
  At dt = 10 µs there are 100,000 substeps per simulated second, so every
  real-time ratio above **halves**.
- Resolution 6 reserves the dense path for `n < ~150` as the "always-correct
  fallback". Measured, real time is lost at n≈52 (nonlinear) and n≈105
  (linear), so the dense range is *already* wider than the real-time budget.

## Where the time actually goes

### 1. Refactorization, for any world with a single nonlinear device

`Engine::compile` sets one global `linear` flag (`crates/sim-core/src/engine.rs`),
and `build()` recomputes `need_factor` as `!(linear && factor_valid && …)`.
So one diode anywhere makes the **entire world** re-stamp and re-factor on
every Newton-Raphson iteration. Measured: **52,576 → 150,000 refactorizations
per simulated second**, consuming **60–95% of every substep**.

The linear rows isolate this exactly. At 5,000 elements, 50 districts:
1,650 µs/substep linear vs 17,027 µs/substep with 4.7% nonlinear devices —
a **10.3x** penalty for the same topology and the same n.

> **Partly fixed since this baseline was measured** — see "Structural win 3"
> below. The gate is no longer `linear` but `linear || !smooth_nonlinear`:
> an op-amp or a 555 is piecewise-linear, so it no longer forces a refactor
> on substeps where nothing flipped. A **diode/zener/LED/BJT/MOSFET/OTA**
> anywhere still does exactly what this section describes, so every row of
> the table above (30% nonlinear blocks, drawn from a pool that is 4/5
> smooth devices) is unchanged and still valid.

### 2. Newton-Raphson iteration count grows with world size

NR mean per substep: 1.05 (516 elems) → 1.19 (1,023) → 2.02 (2,040) → 2.08
(5,122) → 2.40 (20,495) → 2.50 (51,252). The devices did not get harder;
convergence is **global**, so one unconverged device anywhere makes the whole
world iterate again. The islands experiment measures the same 5,122-element
world both ways: **2.09 iterations/substep as one matrix vs 1.03 per island**.

### 3. Fill-in, and it is a topology property, not an n property

`nnz/row` is flat at **2.6–2.9 at every size** (density 2.03% at n=136 down to
0.021% at n=13,336) — these matrices are as sparse as MNA theory says. What
changes is what the factor does to them:

| world | fill (LU nnz ÷ matrix nnz) | row-updates as % of a blind n³/3 |
|---|---|---|
| districts, linear | 1.0x at every size | 0.6% (n=491) → 0.0% (n=12,324) |
| districts, 4.7% nonlinear | 2.2–4.3x | 6.8% (n=136) → 0.0% (n=13,336) |
| one-circuit, 4.7% nonlinear | 28x (n=124) → 51x (n=248) → **276x** (n=1,219) → **1,108x** (n=4,873) | 62–68% |

A single connected world is, after pivoting, effectively a **dense** solve:
65% of `n^2` occupied and 62% of `n^3/3` arithmetic executed. A district world
of the same n is almost fill-free. The current dense kernel already exploits
this by accident — the `if m != 0.0` guard in `sim_math::DenseLu::factor` skips
whole row updates — which is why the districts rows are not catastrophic.

*Hypothesis, not measured:* the drivers of the one-circuit blow-up are (a) the
star topology, where every block couples only through two shared rail nodes,
and (b) partial pivoting preferring the ±1 rows of voltage sources and the
1e5-gain rows of op-amps, which sit at the far end of the unknown vector
(`[all nodes | all branches]`) and spread fill across the full width. Worth a
follow-up experiment before an ordering is chosen.

### 4. The triangular solve is not free — it is O(n²) memory traffic

The "linear circuits only change the RHS" fast path still pays a full
`n x n` forward/back substitution every substep. Measured solve cost:
11.5 µs (n=136) → 290 µs (n=535) → 1.5 ms (n=1,348) → **42.3 ms (n=5,333)** →
286 ms (n=13,336). At n=5,333 **one triangular solve exceeds a whole 33 ms
tick.** For linear worlds the solve is **58–79% of the entire substep**.

### 5. The dense LU is memory-bound, not arithmetic-bound, on real matrices

Same kernel, dense matrix vs stamped MNA matrix at matched n:

| n | dense factor ms | dense row-updates | ns/update | MNA factor ms | MNA row-updates | MNA ns per 'update' | dense/MNA time |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 64 | 0.034 | 85,344 | 0.40 | 0.027 (n=78) | 16,940 | 1.61 | 1.2x |
| 128 | 0.207 | 690,880 | 0.30 | 0.120 (n=161) | 75,006 | 1.60 | 1.7x |
| 256 | 1.927 | 5,559,680 | 0.35 | 0.415 (n=295) | 196,996 | 2.11 | 4.6x |
| 512 | 15.488 | 44,608,256 | 0.35 | 1.487 (n=557) | 673,067 | 2.21 | 10.4x |
| 1024 | 138.444 | 357,389,824 | 0.39 | 5.932 (n=1,089) | 2,363,132 | 2.51 | 23.3x |
| 2048 | 1477.844 | 2,861,214,720 | 0.52 | 27.671 (n=2,203) | 8,626,178 | 3.21 | 53.4x |

The kernel does a row-update in a steady **0.30–0.52 ns** on a dense matrix.
On a sparse MNA matrix the *same* kernel bills **1.6–3.2 ns per update, rising
with n** — because `factor` memcpy's the whole `n x n` array and walks
`n(n-1)/2` pivot candidates down columns at stride `n` regardless of sparsity.
At n=2,203 the arithmetic accounts for ~4.3 ms of a 27.7 ms factor; the other
~85% is the `O(n^2)` copy and strided pivot search. **A sparse solver would win
here mostly by not touching `n^2` memory, not by skipping flops.**

### 6. `compile()` and `frame()` are O(elements²)

`compile()` finds junctions with a linear scan (`points.iter().position`), and
`solve_wire_currents` does the same per pin. Measured `set_elements`:
0.14 ms (516 elements) → 2.02 ms (2,040) → 10.17 ms (5,122) → 168.81 ms
(20,495) → **1,261 ms (51,252)**. A 99x increase in elements cost 9,000x more
compile time. At 20k elements a single edit already costs 5 ticks; at 50k, 38.

### 7. Outside the solver, per-element bookkeeping alone eats the budget

Subtracting the measured solve from the measured substep on the linear rows
(no factorization at all) leaves stamping + `saved`-state clone +
`update_guesses` + `accept`:

| elements | substep µs | solve µs | remainder µs | ns per element per substep |
|---:|---:|---:|---:|---:|
| 515 | 18.4 | 11.2 | 7.2 | 14 |
| 1,028 | 66.3 | 47.8 | 18.5 | 18 |
| 2,055 | 308.3 | 232.6 | 75.7 | 37 |
| 5,132 | 1,650.0 | 1,299.0 | 351.0 | 68 |

Real time at dt = 20 µs allows **20 µs of wall per substep, total**. At 14–68 ns
per element visited, that budget is exhausted by **300–1,400 elements** even
with a hypothetically free solver. This is the ceiling that no solver change
can lift.

## Structural win 1: islands

Confirmed in code: `Engine::compile` numbers every node in the world into one
unknown vector and does `self.a.resize(self.n * self.n, 0.0)` — **one dense
matrix for everything**, with disconnected districts sharing it. The generator
tests assert the districts really are separate islands
(`districts_are_electrically_disconnected`).

Measured on one 5,122-element world, 50 districts, 4.7% nonlinear, solved both
ways (identical elements):

| | unknowns | compile | one factor | LU row-updates | NR iters/substep | per substep | real time |
|---|---:|---:|---:|---:|---:|---:|---:|
| today: one matrix | n=1,348 | 12.23 ms | 7.813 ms | 3.48 M | 2.09 | 20,536 µs | 0.0010x |
| 50 independent engines | Σn=1,348 (max 32) | 0.93 ms | Σ 0.133 ms | 0.08 M | 1.03 | 300 µs | 0.0666x |
| **ratio** | | **13.2x** | **58.6x** | **45.5x** | **2.0x** | **68.4x** | |

So the answer to "one 5,000-unknown dense factor vs the sum of 50 independent
100-unknown factors" is **58.6x on the factor and 68.4x on the whole substep**,
for an electrically identical world. Partitioning also halves the NR iteration
count for free, because convergence stops being all-or-nothing.

Stability across four runs of this experiment: factor 56.7x / 58.6x / 61.5x /
62.6x, whole substep 66.6x / 68.4x / 68.7x / 76.7x. The **45.5x** row-update
ratio is identical every run because it is a count, not a timing — that one is
exact.

**But islands alone do not reach real time at 5,000 elements**: 0.0666x is
still 15x short. See the recommendation.

## Structural win 2: quiescence

A district counts as static when no unknown moves more than 1 µV in a 20 µs
substep (0.05 V/s — invisible to any player and any probe) for 500 consecutive
substeps. Measured per island on a 2,040-element / 20-district world:

| active_percent (assumption) | static districts @250 ms | @1,000 ms | static share of Σn³ | DC-only districts still moving @1 s |
|---:|---:|---:|---:|---:|
| 0 | 20/20 = 100% | 100% | 100% | 0 |
| 20 | 15/20 = 75% | 75% | 73% | 0 |
| 50 | 9/20 = 45% | 45% | 40% | 0 |
| 100 | 0/20 = 0% | 0% | 0% | 0 |

Read this carefully — the halves must not be conflated:

- **Measured:** a district containing nothing that switches reaches a fully
  static DC state and *stays* there, in every case, within 250 ms of sim time
  (0 exceptions across all four sweeps). A district containing an oscillator or
  an AC source never does (self-check: every such district reads as moving).
  Settling is not instant: at 10 ms of sim time only 10% are static; the
  reservoir caps and RC time constants need ~250 ms.
- **Assumed:** how many builds in a real room contain something that switches.
  The generator's `active_percent` is a modelling knob, and the static fraction
  tracks `1 - active_percent` exactly. The quiescence win is therefore worth
  whatever that fraction turns out to be in a real room — the engine's
  contribution to the answer is that the idle part really does go completely
  still, so skipping it is safe.

Quiescence is worth nothing without islands: you cannot skip part of one
matrix. It is islands' multiplier, not an alternative to it.

## Recommendation, in order

1. **Per-island partitioning.** Largest measured win (68.4x per substep at
   5,000 elements), cheapest to build — `compile()` already computes the
   wire-closure union-find that the partition needs. It simultaneously fixes
   the global `linear` flag (a diode in district 7 stops costing district 3
   anything), the global NR convergence (measured 2.09 → 1.03 iterations), the
   fill blow-up (island n stays small, so fill stays near 1.0x), and the
   `O(elements^2)` compile.
2. **Skip quiescent islands.** Measured: DC-only islands go completely static
   and stay static. Multiplies win 1 by `1/(1-idle fraction)`; at the modelled
   20% active that is 4x. Only possible after 1.
3. **Per-island local dt** (`docs/plan.md` resolution 1). Islands and
   quiescence reduce the *work per substep*; only local dt reduces the 50,000
   *substeps per simulated second*, which is the other factor in every number
   here. An idle-but-not-static district on a 10x dt is 10x cheaper.
4. **Then the sparse LU (spike S3)** — and size it against the right target.
   Measured at n=535 (nonlinear districts), refactor + solve = 1.400 ms +
   0.291 ms = **1.69 ms, i.e. 34x over the S3 gate of "n=500 refactor+solve <
   50 µs native"**. The kernel table says the win there is mostly in *not
   touching `n^2` memory* (85% of a sparse factor at n=2,203 is the copy and
   pivot search), so a fixed-pattern sparse factor with a pre-computed
   elimination order should get most of the 34x. But note the ordering: after
   islands cap island size near n≈32, the dense factor is already only ~2.7 µs
   per island and 45% of the per-substep cost — so sparse LU matters for *large
   single islands* (a big shared power grid, or the `one-circuit` case where
   fill reached 1,108x), not for a world of small builds.
5. **The triangular solve deserves the same treatment as the factor.** It is
   58–79% of a linear substep and 42 ms at n=5,333. A sparse factor with a
   dense solve would leave most of the linear-world cost in place.
6. **Fix the `O(elements^2)` `compile()` and `frame()`** before 20k elements is
   attempted at all: 1.26 s per edit and >33 ms per render frame are already
   over budget at sizes the solver has no hope of running anyway, but they will
   also throttle editing in a large *quiescent* world, which islands +
   quiescence are supposed to make cheap.

What none of these do is lift the per-element bookkeeping floor measured in
§7 (14–68 ns per element per substep, i.e. 300–1,400 element-visits per
20 µs substep). Reaching 3,000 elements at real time therefore requires that
most of the world is **not visited at all** on most substeps — which is
exactly what islands + quiescence + local dt buy, and what a faster solver
cannot.

## Structural win 3 (BUILT): event-driven piecewise-linear reuse

Measured after the fact, on this same machine, and reported here because it
changes the premise of §1 for one specific — and, for the game, central —
circuit class: op-amps and 555s.

An op-amp's and a 555's contribution to **A** is a function of a *discrete*
state (rail region, RS latch), not of the operating point. Between two flips
the matrix is constant, so the previous substep's LU is the factorization of
the same matrix, bit for bit. `ElementKind::needs_newton()` now separates
those two devices from the genuinely smooth ones (diode, zener, LED, BJT,
MOSFET, OTA), and `Engine::reusable()` keeps the factorization until
`update_guesses` sees a region or latch actually move.

A 555 astable at 480 Hz flips 957 times per simulated second against 50,000
substeps: **98.1% of substeps reuse**. Op-amp Schmitt relaxation: 909 flips,
98.2%. An op-amp in negative feedback never leaves region 0 at all.

Medians of 5 runs, each 50,000 substeps at dt = 20 µs = 1.000 s simulated
(`cargo run --release -p sim-golden --bin pwlroom`). "room" = the districts
generator in `sim_golden::scale` at 0% nonlinear, so the padding is what a
room of other people's passive builds looks like.

| world | elements | n | before | after |
|---|---:|---:|---:|---:|
| 555 astable alone | 14 | 6 | 61.2x | **91.7x** |
| op-amp Schmitt alone | 12 | 4 | 94.5x | **122.6x** |
| 555 + room | 172 | 51 | 1.89x | **5.08x** |
| Schmitt + room | 170 | 49 | 2.03x | **5.35x** |
| both + room | 184 | 55 | 1.62x | **4.43x** |
| 555 + room (one connected circuit) | 154 | 32 | 3.80x | **7.63x** |
| 555 + room + ONE led (control) | 176 | 54 | 1.70x | 1.71x |
| linear room only (control) | 158 | 45 | 6.63x | 6.75x |

Largest 555 room that holds real time (log-log interpolation between the
239- and 342-element rows before, the 342- and 518-element rows after):
**243 elements (n≈70) → 486 elements (n≈136)**. Exactly 2x the room, and it
multiplies with islands rather than overlapping it: islands shrinks n per
matrix, this removes ~98% of the factorizations.

Output is **bit-identical**, not merely close: the reused L/U is the
factorization of a matrix that a refactor would have rebuilt to the same
bits. `crates/sim-golden/tests/pwl_reuse.rs` asserts that directly — it
compares `Engine::matrix()` word for word between a reusing engine and a
refactor-every-substep engine at every substep of every golden, plus the
state hash, the NR pass count, the rescue count and the quarantine flag,
including under live edits and under damage that reclassifies a world
mid-run. `tools/determinism.sh` still reports native == wasm32 with the same
hashes as before the change.

What it does **not** do: help any world containing a diode, zener, LED, BJT,
MOSFET or OTA (measured: 1.00x, correctly — freezing a diode's conductance
would be approximating the physics), or move the per-element bookkeeping
floor of §7. After the change the O(n²) triangular solve is the ceiling for
these circuits.
