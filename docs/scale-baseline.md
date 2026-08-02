# Scale baseline — where the solver time actually goes

Measurement only. Nothing was optimised to produce this document; it is the
yardstick later solver decisions get judged against.

> **Status: recommendations 1, 2 and 3 have landed.** `sim-core` now
> partitions every document into independent per-island engines (one small
> dense matrix, one `linear` flag, one NR loop, one quarantine flag each),
> **skips islands that have gone electrically static**, and gives every
> island **its own timestep** inside a configurable band. **Everything in
> this document up to and including "Structural win 1" is the BEFORE
> measurement and is kept exactly as it was measured** — it is the yardstick.
> The AFTER measurements, taken with the same harness on the same worlds,
> are in [Structural win 1](#structural-win-1-islands--landed),
> [Structural win 2](#structural-win-2-quiescence--landed) and
> [Structural win 3](#structural-win-3-per-island-local-dt--landed). Two
> consequences for reading the tables below: the `n` column is now the *sum*
> over islands (no such matrix is ever built), and `Engine::compile` no
> longer puts the whole world in one matrix, so §6's "one dense matrix for
> everything" is history rather than description.

## Provenance

| | |
|---|---|
| machine | Apple M4, 10 cores, 24 GB RAM, macOS 26.5.2 |
| target | `aarch64-apple-darwin` |
| build | `--release` (opt-level 3, lto thin, no FMA, no SIMD) |
| toolchain | rustc 1.95.0, pinned by `rust-toolchain.toml` |
| branch | `wf/islands-integrate` (originally `wf/scale-bench`) |
| sim config | dt = 20 µs, tick = 30 Hz → 1,666.7 substeps/tick, **50,000 substeps per simulated second** (matches `crates/server/src/main.rs`) |

Every number below is a wall-clock measurement on that machine unless it is
explicitly labelled an estimate.

> **Two vintages of measurement live in this document.** The tables in
> "Where the time actually goes" and in Structural wins 1–3 were taken on the
> original `wf/scale-bench` tree, BEFORE piecewise-linear factorization reuse
> existed. Reuse changes the refactor-rate columns for op-amp and 555 worlds
> specifically, and nothing else — a world whose nonlinearity is a diode, a
> BJT or a MOSFET (which is what the generator's 30%-nonlinear pool is, 4/5
> of the time) refactors exactly as those tables say. The rows re-measured on
> THIS tree, with everything landed, are in "Structural win 4" and "The whole
> branch, end to end"; quote those when quoting a shipping number.

Two caveats, stated up front:

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
comparison, the real-time crossover search, and the islands, quiescence and
lever experiments. Two flags decide what regime is being measured, and every
number below states which it was taken in:

```
--tuning off        quiescence and local dt disabled: the islands-only
                    yardstick everything after "Structural win 1" is
                    measured against.
--settle-ms 300     run 300 ms of SIM time before the stopwatch starts.
                    A DC-only district needs ~250 ms to reach its static
                    state (measured below), so a measurement of a settled
                    room has to pay for it; the default 0 measures a world
                    four substeps after it was placed.
```

The exact invocations behind the tables below:

```
cargo run --release -p sim-golden --bin scale -- --sizes 500,1000,5000,20000 --max-n 6000
cargo run --release -p sim-golden --bin scale -- --sizes 2000 --structures districts,linear \
    --skip crossover --skip islands --skip quiescence
cargo run --release -p sim-golden --bin scale -- --sizes 50000 --structures districts,linear \
    --max-n 14000 --min-steps 2 --step-cap 30 --frame-max 0 \
    --skip kernel --skip crossover --skip islands --skip quiescence
cargo run --release -p sim-golden --bin scale -- --skip sizes --skip kernel

# the two levers, before/after on the same worlds (Structural wins 2 and 3)
cargo run --release -p sim-golden --bin scale -- \
    --tuning off --skip kernel --skip islands --skip quiescence --skip levers
cargo run --release -p sim-golden --bin scale -- \
    --settle-ms 300 --structures districts,linear \
    --skip kernel --skip islands --skip quiescence --skip levers
cargo run --release -p sim-golden --bin scale -- \
    --skip sizes --skip kernel --skip crossover --skip islands

# Structural win 4 and "the whole branch, end to end", re-measured on this
# tree. The first uses only the public Engine API that exists on both sides
# of this branch, so the same source runs against a pristine checkout to
# produce the "before" column.
cargo run --release -p sim-golden --bin pwlroom
cargo run --release -p sim-golden --bin scale -- --sizes 150,500,1000 \
    --structures districts --skip kernel --skip islands --skip crossover \
    --skip quiescence --skip levers
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

> **Fixed twice since this baseline was measured** — see "Structural win 4"
> below and "Structural win 1" above.
>
> First, the gate is no longer `linear` but `linear || !smooth_nonlinear`:
> an op-amp or a 555 is piecewise-linear, so it no longer forces a refactor
> on substeps where nothing flipped. A **diode/zener/LED/BJT/MOSFET/OTA**
> still does, so every row of the table above (30% nonlinear blocks, drawn
> from a pool that is 4/5 smooth devices) is unchanged and still valid as a
> description of a world containing one.
>
> Second, and this is what islands added on top: the flag is no longer
> global. It is a property of the ISLAND, so "one diode anywhere makes the
> entire world re-stamp and re-factor" is now "one diode makes ITS OWN
> DISTRICT re-stamp and re-factor". Measured on the owner's circuit class,
> the 555-plus-one-LED room went from **1.72x to 123.7x** real time on the
> strength of that single change of scope.

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

## Structural win 1: islands — LANDED

Confirmed in code *at the time of the measurement below*: `Engine::compile`
numbered every node in the world into one unknown vector and did
`self.a.resize(self.n * self.n, 0.0)` — **one dense matrix for everything**,
with disconnected districts sharing it. The generator tests assert the
districts really are separate islands
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

### After: the same harness, with partitioning in the engine

`sim-core` now builds that partition itself (`Engine` owns a `Vec<Island>`;
`crates/sim-core/src/engine.rs`). Same machine, same `cargo run --release -p
sim-golden --bin scale`, same generated worlds, two runs a minute apart:

| world | before µs/substep | after (run 1 / run 2) | before real-time | after | largest matrix, before → after |
|---|---:|---:|---:|---:|---|
| 516 elems, districts~100, 4.7% nonlinear | 71.9 | **17.3 / 17.6** | 0.2782x | **1.157x / 1.135x** | n=136 → 32 |
| 515 elems, districts, linear | 18.4 | **5.4 / 5.4** | 1.0868x | **3.73x / 3.68x** | n=132 → 29 |
| 1,023 elems, districts~100 | 374.0 | **33.3 / 34.1** | 0.0535x | **0.601x / 0.586x** | n=265 → 32 |
| 1,028 elems, districts, linear | 66.3 | **10.4 / 11.1** | 0.3016x | **1.919x / 1.799x** | n=246 → 29 |
| 5,122 elems, districts~100 | 17,026.6 | **174.2 / 179.4** | 0.0012x | **0.1148x / 0.1115x** | n=1,348 → 32 |
| 5,132 elems, districts, linear | 1,650.0 | **55.4 / 58.5** | 0.0121x | **0.361x / 0.342x** | n=1,215 → 29 |
| 500 elems, **one-circuit** (control) | 115.0 | 109.3 / 113.3 | 0.1738x | 0.183x / 0.177x | n=124 → 124 |
| 5,004 elems, **one-circuit** (control) | 281,050 | 162,710 / 168,765 | 0.0001x | 0.0001x | n=1,219 → 1,219 |

Read the one-circuit rows as the control they are: a single connected world
has exactly one island, so partitioning cannot help it, and it does not
(1.05x at 500 elements, which is the wire/ground elements no longer being
visited per substep — see below). **Every districts row moved because the
partition moved, not because the machine did.** Beware comparing absolute
times across the two runs of this document: the unchanged dense-LU kernel
measured 138.4 ms at n=1024 for the before table and 65.1 ms for the after
one, so the before run was partly contended. The within-run comparisons are
the trustworthy ones, and they are stark: at ~500 elements, districts vs
one-circuit was 1.6x apart before and is **6.3x** apart now; at ~5,000
elements it was 16.5x apart and is **934x** apart.

The islands experiment now compares the engine's own partition against a
hand-built `Engine` per district, and finds them the same to within the
noise — **factor 1.02x / 0.99x, row-updates 1.00x, whole substep 1.00x /
1.01x** over two runs. The engine gives away nothing versus splitting by
hand, which is exactly the 300 µs/substep hand-split number measured above.

What changed in the numbers, and why:

- **Fill-in collapsed** because island `n` is small: the 5,122-element world
  went from a factor with 3.48 M row-updates to **0.077 M** to factor every
  one of the 50 islands (45.5x, and it is a count, not a timing).
- **NR is per island**: 2.09 iterations/substep → **1.03**, exactly as the
  before-experiment predicted.
- **The `linear` flag is per island.** A world of linear districts refactors
  0 times per substep even with a diode next door; the districts-linear rows
  hold real time up to 1,752 elements.
- **`compile()` stopped being O(elements²)** (hashed junction interning,
  hashed state carry-over, root-indexed node numbering): 10.17 ms → **0.80
  ms** at 5,122 elements, 12.7x. `frame()`'s KCL pass got the same treatment:
  8.22 ms → **0.86 ms**.
- **Wires and grounds are no longer visited by the solve loops.** They stamp
  nothing, accept nothing and converge nothing, so they are parked outside
  the per-substep prefix — 48% of a real document, removed from §7's
  per-element floor. Broken parts and parts with every pin on ground are
  parked with them.

The largest world that holds real time, bisected exactly as above (two runs):

| structure | nonlinear | before | after |
|---|---|---|---|
| districts~100 | 4.7% of elements | 205 elements (n=52) | **516 elements** (n=136, sum; largest island 32) |
| districts~100 | none | 414 elements | **1,752 elements** (n=420 sum) |
| one-circuit | 4.7% of elements | 245 elements | 310 elements |
| one-circuit | none | 495 elements | 723 elements |

The remaining recommendations are unchanged and are now *unblocked*: skipping
quiescent islands (measured again here: 100% of DC-only districts static
within 250 ms), per-island local dt, and — for the one-circuit case, which is
the only place a big matrix survives — the sparse LU.

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

Re-measured after the lever landed, the same sweep reproduces the same table
(100% / 75% / 45% / 0% static at 250 ms, self-check 5/5, 11/11 and 20/20
switching districts reading as moving), and the engine's own verdict agrees
with it: on the 5,122-element world it puts **49.8 / 50** solving islands to
sleep at 0% active, **38.5 / 50** at 20%, **24.5 / 50** at 50% and
**1.1 / 50** at 100%.

### What landed

`Island` sleeps when **three** conditions hold continuously for
`Tuning::quiescence_hold` (default 10 ms of sim time, the 500-substep window
above). Every one of them is applied **per unknown, in that unknown's own
dimension**: the unknown vector is `[node voltages | branch currents]`, and a
volt-dimensioned threshold applied to an amp is not a conservative
approximation, it is a category error. Each threshold therefore comes in a
pair, the `_i` half being the same criterion over a 1 kΩ reference impedance.

1. **Slew** — no unknown moved faster than `quiescence_slew` = 0.05 V/s
   (`quiescence_slew_i` = 50 µA/s). This is exactly the criterion the sweep
   above validated.
2. **Window drift** — no unknown moved more than `quiescence_drift` = 1 µV
   (`quiescence_drift_i` = 1 nA) *across the whole window*, not per step.
   Without this the slew test is a trap: a monotone crawl of 0.9 µV per
   substep passes it forever while travelling half a volt.
3. **Remaining travel** — the travel each unknown has LEFT is inside
   `quiescence_drift`. This is the condition the first draft of the lever did
   not have, and (1) and (2) do not imply it. They bound the travel an island
   has recently DONE; sleeping makes the travel it has left PERMANENT, and
   for a first-order tail those two quantities differ by `tau / hold` — a
   factor of 1,000 at tau = 10 s and 10,000 at tau = 100 s.

   Measured on the criterion without (3), walking into the trap naturally:
   10 MΩ + 10 µF (tau = 100 s) slept at t = 693.5 s holding **9.990136 V**
   against a closed form of 9.999997 V, and never moved again. 1 MΩ + 10 µF
   (tau = 10 s) held 9.999014 V. The claim that the residual was "sub-µV for
   every RC the shipped catalogue can build" was true only up to tau = 10 ms.

The travel left is measurable, so the engine measures it. Across two
consecutive hold windows a relaxing unknown travels `m0` then `m1 = rho*m0`,
so everything still to come sums to `m1*rho/(1-rho) = m1^2/(m0-m1)`. Nothing
is fitted and nothing is assumed about the circuit — the island's own decay
says how far it has left to go. Two whole windows is what makes that
resolvable (a 10 ms baseline against a 20 µs step is 500x the signal), and
`m0-m1` is checked against the f64 noise floor of the differences it came
from before it is believed. An unknown already moving by less than a few ulps
of its own magnitude is exempt, because a BE/TR increment under half an ulp
returns the same bits forever: continuing to solve holds that number exactly
as firmly as sleeping does.

**The worst-case residual, as a number: 1 µV per node and 1 nA per branch, at
any time constant.** Measured on 1 kΩ + 10 mF (tau = 10 s, so that GMIN's
leak shifts the DC point by 1e-9 rather than 1e-6): the old criterion froze
it at t = 93 s, 975 µV short; it now sleeps at t = 161 s holding 9.99999895 V
— 1.05 µV short of 10 V, the 5% over budget being the estimator's own
resolution slack. 10 MΩ + 10 µF now tracks the closed form to 9.2 nV out to
3,000 s of sim time.

An island that cannot demonstrate the decay does not sleep. That is the right
answer — it has not finished moving — and local dt is what makes it cheap.
A ramp has `rho = 1` and never qualifies at all.

Three tests in `crates/sim-core/src/lib.rs` hold this down:

- `a_slow_ramp_is_not_mistaken_for_static`: 10 MΩ + 10 µF, 2 nV per substep,
  asserted never to sleep.
- `a_long_tail_never_freezes_short_of_the_truth`: 15 million substeps, 300 s
  of sim time, asserting the reported voltage tracks the closed form the
  whole way and that the sleep, when it comes, is AT the answer. The trap is
  at 93 s; **no shorter test can see it**, which is why the original suite
  (0.2 s of sim time) did not.
- `a_ramping_branch_current_is_never_declared_static`: the amps half. 50 µV
  across 1 H ramps the source's branch current at 5e-5 A/s forever; read as
  volts that is a 50 nV/s crawl, and the island slept in the first 60 ms
  holding 2.5 µA while the truth walked to 1 mA.

The last two also keep their reproduction live: each re-runs its circuit
with `quiescence_decay = 1` (which disables the settle test, and is exactly
the criterion that shipped) and asserts the freeze comes straight back. A
regression test that stops reproducing the defect has stopped measuring
anything, and should say so out loud.

Two more rules:

- An island holding a **time-varying source** (`amp != 0 && hz != 0`) is
  *structurally* barred from sleeping. Its equations depend on `t`, so
  freezing it would be a lie the moment the clock moves. This is a property
  of the netlist, computed once per edit.
- A sleeping island **reports the DC state its last real solve produced** —
  `x`, every device's history, every pin current, untouched. Skipping
  arithmetic is not the same as inventing a number, and the test
  `an_idle_world_goes_static_and_still_reads_the_same_as_a_fully_solved_one`
  asserts every pin of a slept 1,000-element world agrees to 1 mV with the
  same world solved every substep.

**Waking** is immediate and total: back to the room dt, no history, next
substep solved in full. Every path that can perturb an island routes through
it — `set_elements`/`interact`/`set_broken` (which rebuild the partition, so
every island is born awake), `write_param` (the co-simulation path, which
does *not* recompile and so wakes explicitly), and the public
`wake_all`/`wake_island`/`wake_element` for couplings sim-core does not model
yet. `a_static_board_wakes_immediately_on_a_switch_flip` asserts the lamp
lights one substep after the flip on a board that had been asleep for 100 ms.

`write_param` wakes only when the write actually moved a number. That is the
same promise it already made about factorizations, and it matters here: a
machine mirroring an unchanged back-EMF at 1.5 kHz must cost what silence
costs, or a stalled motor's island could never go still.

## Structural win 3: per-island local dt — LANDED

Islands and quiescence cut the *work per substep*. Local dt is the only lever
that cuts the other factor in every number in this document: the **50,000
substeps per simulated second**.

Each island integrates at `h = k * dt`, `k` a power of two. Powers of two, so
every island's step boundary is also a world substep boundary — no island can
ever land *between* two of them. `k` is raised only from an
integration-error estimate computed from the island's own state, and
collapses to 1 on any perturbation.

### The correctness argument

**Against the integrator.** TR and BE are one-step methods: their companion
models are derived for an arbitrary `h`, `build()` already stamps
`geq = 2C/h` (TR) or `C/h` (BE) from whatever `h` it is handed, and
`accept()` recovers the device current with the same `h`. A capacitor's
history is `(v_prev, i_prev)` — the physical voltage and current at the last
accepted time, not a fixed-step difference — so the trapezoid rule over a
longer interval is still the trapezoid rule. Changing `h` mid-stream is
therefore *exact*, not an approximation; what changes is the local truncation
error, which is O(h^3 * y''') for TR and O(h^2 * y'') for BE. The
factorization is the one thing that is not h-independent, and `build()`
already refuses to reuse a factorization stamped at a different `h`
(`factored_h == h`), so nothing had to be invalidated by hand.

**Choosing `k`: two budgets, for the two ways a bigger step costs accuracy.**

*Curvature*, which bounds the error made INSIDE the step. The controller
estimates it from the second difference of the unknown vector,
`|x_n - 2 x_{n-1} + x_{n-2}| ~= h^2 * y''` — the leading BE error term, and
an upper bound on the TR one — against `Tuning::local_dt_err` = 0.1 mV per
step (`local_dt_err_i` = 0.1 µA for the branch unknowns).

*Motion*, which bounds the error at the END of it. An island's step lands on
a world substep boundary, but the caller can stop the world anywhere, so the
island sits up to `(k-1)*dt` of world time behind the number it reports. That
lag is a FIRST-order error, `slew * lag`, and it dwarfs anything the
curvature budget bounds. So `k` may only rise while the island's travel
across one local step stays under `Tuning::local_dt_slew * dt` = 5 V/s * dt
(`local_dt_slew_i` = 5 mA/s), i.e. while `slew * k <= local_dt_slew`. **The
read-out lag is then under `local_dt_slew * dt` volts whatever `k` is: 100 µV
at the shipped 20 µs, 5 µV at 1 µs.** (The test is on the step just taken, so
an island that accelerates can overshoot that once by whatever the
acceleration was worth — bounded in turn by `local_dt_err`, because curvature
is what acceleration is, and `k` is back at 1 one step later. The honest bound
is `local_dt_slew * dt + local_dt_err`.)

The second budget is not redundant, and leaving it out inverted the meaning
of `dt`. Worst error of `rc_step` against the closed form over its first
10 ms, with a curvature budget alone:

| dt | levers off | levers on, curvature only | levers on, both budgets |
|---:|---:|---:|---:|
| 40 µs | 1.44e-2 | 1.44e-2 | 1.44e-2 |
| 20 µs | 3.79e-3 | 3.79e-3 | 3.79e-3 |
| 10 µs | 9.53e-4 | 1.29e-3 | 9.53e-4 |
| 5 µs | 2.39e-4 | 3.59e-3 | 2.39e-4 |
| 2 µs | 3.83e-5 | 4.81e-3 | 3.83e-5 |
| 1 µs | 9.57e-6 | 4.81e-3 | 9.57e-6 |

Because the curvature budget is an ABSOLUTE volt figure, halving `dt` simply
let `k` double to spend the same budget: `h` stayed where it was and the lag
with it, so refining the room dt made the answer 500x worse than the same
engine with the levers off — and worse than the same engine at twenty times
the step size. Asking for a more accurate simulation must never buy a less
accurate one. With the motion budget the two right-hand columns are
identical: a capacitor moving at kilovolts per second is not dilated at all,
and dilation starts where it always should have — 0.3 s into a 100 ms tail,
not 40 ms in while it is still moving at 34 V/s.
`refining_the_room_dt_never_makes_the_answer_worse`
(`crates/sim-golden/tests/golden.rs`) walks that sweep and asserts the worst
error is non-increasing, with the pre-fix controller kept live beside it as
the reproduction.

`k` doubles only after `local_dt_hold` = 64 consecutive steps whose estimates
*at twice the step* (`4 * cmax` for curvature, because that term is `h^2`;
`2 * dmax` for motion, because that one is first order) still fit, and drops
straight back to 1 the moment either does not. It is slow to rise and instant
to fall, which is the right asymmetry: being one step late to dilate costs
nothing, being one step late to contract costs accuracy.

Three things collapse `k` to 1 that are not numeric estimates at all,
because they are *facts* about the devices rather than extrapolations from
two old samples: a **discrete transition** (op-amp rail region, 555 latch),
an **NR rescue** (the dt-halving ladder engaging is the loudest possible
"that step was too big"), and any **perturbation** from outside.

**The history rule, which is where naive multirate controllers break.** The
second difference is only an error estimate when the three samples are
equally spaced *at the current `h`*. The first step after a change of `k`
compares against samples taken at the old one, which reads as a huge false
curvature and slams `k` back to 1 — forever, one step after every raise. The
implementation tracks `hist` and simply refuses to have an opinion until
three evenly spaced samples exist. (This was a real bug in the first draft,
caught by `local_dt_dilates_a_slow_island_and_collapses_on_a_transient`.)

**Determinism.** Every input to the controller is deterministic f64 state
compared against a constant; the counters are integers. Nothing reads a wall
clock, a load average or a thread count — `docs/scale-parallelism.md` §5.3
flagged that instinct as the one that would destroy determinism and it is
forbidden here. `./tools/determinism.sh` passes: native aarch64 == wasm32 on
all 17 goldens. The hashes themselves **moved**, because the trajectory
legitimately changed; this is the documented, tolerance-defined semantics
change §5.2 said it would have to be.

**Cross-island coupling.** There is none to get wrong yet: islands are
electrically disjoint by construction, and ground is a shared reference, not
a coupling. When Bergeron corridors land (plan resolution 3, exact one-dt
decoupling), the rule is already fixed by the design: a corridor is a delay
line sampled at the room dt, so an island that terminates one must be clamped
to `k * dt <=` the corridor's travel time, and a corridor whose boundary
state moved calls `Engine::wake_island` on both ends. `Island::advance` takes
the room `dt` from the caller every call and re-derives its cap from it, so
that clamp is a one-line change, not a redesign.

**What a player can observe.** Three separate guards, because this is the
lever that could quietly lie:

1. **Instruments pin their island to `k = 1`.** `Engine::set_sampled` takes
   the element ids a scope probe, measurement chip or audio tap is reading,
   and the islands owning them never dilate. Speakers and co-simulated
   motors are pinned *structurally*, from the netlist, because they are
   sampled faster than the tick by definition. The server re-declares the
   set every tick (`crates/server/src/main.rs`), after the edit drain, so it
   survives the partition being rebuilt.
2. **A time-varying source caps `k` by Nyquist-with-margin** —
   `local_dt_min_samples` = 64 samples per cycle — on top of whatever the
   error controller decides. The controller would catch aliasing anyway
   (a 60 Hz sine's second difference blows the budget long before 64
   samples/cycle); this is a structural belt to its braces.
3. **Staleness is bounded twice: in time by `local_dt_max` = 500 µs, and in
   volts by `local_dt_slew * dt` = 100 µV.** An island can owe at most one
   local step of world time, so at 60 Hz it is at most 3% of a frame behind
   — and the motion budget above means that whatever it owes, the number it
   is reporting is within 100 µV of the one it would report if it were
   exactly on world time. The volts bound is the one that matters, because
   it shrinks with the room dt and the time bound does not.
   `dt_dilation_never_desynchronises_the_world` advances in ragged chunks
   that divide no `k` and asserts the world clock lands exactly where the
   substep count says.

And the interaction of the two levers is guarded directly:
`both_levers_together_do_not_change_what_a_player_sees` runs a three-island
world (static divider, dilating RC, nonlinear LED board) for 10,000 substeps
with both levers on and with both off, and asserts every pin voltage agrees
to 1 mV and every pin current to 10 µA, at identical world times.

### Measured

> **Re-measured after the correctness fixes.** The table below is the
> shipping criterion: sleeping now requires an island to have *finished*
> moving (remaining travel under 1 µV), not merely to have gone quiet, and
> `k` is governed by read-out lag as well as curvature. The first draft's
> numbers at 0% active (215x / 194x for quiescence, 576x / 582x for both)
> were measuring islands frozen 50 µV to 10 mV short of their answers, and
> are not comparable. The tables further down this document that carry a
> "+ both levers" column were taken under that first draft and are marked
> where they appear.

Same machine, same harness, same generated worlds. Four configurations of the
*same* 5,122-element world before the stopwatch starts. Two runs, minutes
apart, machine shared with several other agent workflows.

**Settled 300 ms** (past the ~250 ms a DC district needs to stop moving
visibly — but NOT past the point where the engine can certify that the travel
left is under a microvolt, which is the thing sleeping now turns on):

| active% | config | µs/substep (run 1 / run 2) | real-time | **vs islands-only** | static islands | mean k |
|---:|---|---:|---:|---:|---:|---:|
| 0 | islands only | 171.4 / 171.5 | 0.117x | 1.00x | 0/51 | 1.00 |
| 0 | + quiescence | 80.9 / 81.3 | 0.247x / 0.246x | **2.12x / 2.11x** | 27.3/51 | 1.00 |
| 0 | + local dt | 11.1 / 11.1 | 1.80x | **15.5x / 15.4x** | 0/51 | 16.0 |
| 0 | + both | 1.8 / 1.8 | 11.4x | **97.8x / 97.5x** | 43.2/51 | 15.9 |
| 20 | islands only | 180.6 / 181.4 | 0.111x | 1.00x | 0/51 | 1.00 |
| 20 | + quiescence | 100.7 / 100.0 | 0.199x / 0.200x | **1.79x / 1.81x** | 24.0/51 | 1.00 |
| 20 | + local dt | 53.7 / 53.4 | 0.372x / 0.375x | **3.36x / 3.40x** | 0/51 | 12.7 |
| 20 | + both | 47.4 / 47.4 | 0.422x | **3.81x / 3.83x** | 31.8/51 | 6.90 |
| 50 | islands only | 187.3 / 190.8 | 0.107x / 0.105x | 1.00x | 0/51 | 1.00 |
| 50 | + quiescence | 137.8 / 139.3 | 0.145x / 0.144x | **1.36x / 1.37x** | 15.0/51 | 1.00 |
| 50 | + local dt | 106.1 / 107.6 | 0.189x / 0.186x | **1.77x** | 0/51 | 8.50 |
| 50 | + both | 103.7 / 100.7 | 0.193x / 0.199x | **1.81x / 1.90x** | 20.2/51 | 3.43 |
| 100 | islands only | 200.8 / 200.0 | 0.100x | 1.00x | 0/51 | 1.00 |
| 100 | + quiescence | 197.8 / 194.8 | 0.101x / 0.103x | **1.02x / 1.03x** | 1.0/51 | 1.00 |
| 100 | + local dt | 193.3 | 0.104x | **1.04x** | 0/51 | 1.60 |
| 100 | + both | 192.6 | 0.104x | **1.04x** | 1.0/51 | 1.31 |

**Settled 5 s**, the same worlds. This is the honest shape of the quiescence
lever: it pays when the room really has stopped, not when it merely looks
stopped, and the difference between these two tables is the price of not
freezing an island short of its answer.

| active% | config | µs/substep | real-time | **vs islands-only** | static islands | mean k |
|---:|---|---:|---:|---:|---:|---:|
| 0 | islands only | 172.4 | 0.116x | 1.00x | 0/51 | 1.00 |
| 0 | + quiescence | 38.2 | 0.524x | **4.51x** | 39.0/51 | 1.00 |
| 0 | + local dt | 10.9 | 1.83x | **15.8x** | 0/51 | 16.0 |
| 0 | + both | 1.4 | 14.7x | **127x** | 45.0/51 | 16.0 |
| 20 | islands only | 177.7 | 0.113x | 1.00x | 0/51 | 1.00 |
| 20 | + quiescence | 74.0 | 0.270x | **2.40x** | 31.0/51 | 1.00 |
| 20 | + local dt | 53.2 | 0.376x | **3.34x** | 0/51 | 12.7 |
| 20 | + both | 45.7 | 0.438x | **3.89x** | 36.0/51 | 4.21 |
| 50 | islands only | 183.5 | 0.109x | 1.00x | 0/51 | 1.00 |
| 50 | + quiescence | 115.0 | 0.174x | **1.60x** | 20.0/51 | 1.00 |
| 50 | + local dt | 108.3 | 0.185x | **1.69x** | 0/51 | 8.50 |
| 50 | + both | 103.7 | 0.193x | **1.77x** | 23.0/51 | 2.11 |
| 100 | islands only | 201.7 | 0.099x | 1.00x | 0/51 | 1.00 |
| 100 | + quiescence | 194.6 | 0.103x | **1.04x** | 2.0/51 | 1.00 |
| 100 | + local dt | 193.2 | 0.104x | **1.04x** | 0/51 | 1.60 |
| 100 | + both | 191.8 | 0.104x | **1.05x** | 2.0/51 | 1.00 |

The 100% block is the one row a longer settle cannot move, and does not: a
room where every district holds an oscillator has nothing that finishes
moving, so 2 islands of 51 go static at either settle and both levers are
worth 1.04x. That is the lever telling the truth about a room that is
genuinely busy.

Between the two tables: 27 -> 39 of 51 islands certified still at 0% active,
quiescence 2.1x -> 4.5x, both levers 98x -> 127x. The 12 that are still awake
at 5 s have tails of order a second; at 5 s such a tail is genuinely tens of
millivolts from its DC point, and freezing it would be exactly the defect
this criterion exists to prevent. They are not free, but at `k = 16` they are
a sixteenth of what they were.

Read it as three findings:

- **The quiescence multiplier is bounded by how much of the room has
  actually finished**, not by how much of it is idle. At a 300 ms settle
  that is about half the idle islands (27/51 at 0% active), because the
  rest are still finishing tails whose remaining travel the engine cannot
  yet certify at a microvolt — and it is right not to freeze them. Given
  5 s it is 39/51 and the lever is worth 4.5x. `active_percent`
  remains an *assumption* about a real room; the engine's contribution is
  that the part which really has stopped costs nothing to keep.
- **Local dt is worth 3.4x on its own at 20% active** and, unlike
  quiescence, it is worth something in a world where *nothing* is idle
  (1.04x at 100% active — the oscillator districts genuinely cannot be
  stepped coarsely, and the lever correctly refuses to try). It is also
  what makes the slower settle affordable: an island that has not finished
  is still integrated, but at `k = 16`.
- **The two overlap heavily and must not be multiplied.** An island that has
  gone completely still is also an island whose curvature is zero, so the
  `+ both` row at 20% active is 3.8x, not 1.8 x 3.4 = 6.1x.
  `docs/scale-parallelism.md` §5.3 warned about exactly this and the
  measurement confirms it. Note the `mean k` column collapses from 12.7 to
  6.9 when quiescence is on — it is a mean over *awake* islands, and once
  the still ones are asleep the islands left are the ones that are moving,
  which correctly sit nearer `k = 1`.

### The size sweep, and the crossover

> **Taken under the first draft of the levers** (sleeping on "has gone
> quiet" rather than "has finished moving", `k` governed by curvature
> alone) and NOT re-measured. Every "+ both levers" figure below is
> therefore optimistic by whatever the un-certified sleeps were worth on
> that world; at 20% active the re-measured levers table above moved by
> about 10%, so these are indicative rather than exact. The islands-only
> yardstick column is unaffected — `Tuning::off()` is byte-identical to the
> pre-lever engine, and a test now asserts that against the digests commit
> 5bc5fe3 produced.

Same worlds as the after-islands table, measured `--tuning off` (yardstick)
and then `--settle-ms 300` with both levers on, at 20% active:

| world | islands only µs/substep | + both levers | islands only real-time | + both levers |
|---|---:|---:|---:|---:|
| 516 elems, districts~100, 4.7% nonlinear | 16.8 | **7.3** | 1.19x | **2.73x** |
| 515 elems, districts, linear | 5.3 | **2.3** | 3.79x | **8.84x** |
| 1,023 elems, districts~100 | 31.7 | **7.4** | 0.630x | **2.70x** |
| 1,028 elems, districts, linear | 10.6 | **2.2** | 1.89x | **9.22x** |
| 5,122 elems, districts~100 | 165.7 | **43.5** | 0.121x | **0.459x** |
| 5,132 elems, districts, linear | 55.1 | **13.5** | 0.363x | **1.486x** |

Both columns are from the same session on the same machine, so they are
directly comparable — unlike the before/after pair in Structural win 1,
where the "before" run was partly contended.

The largest world that holds real time, bisected exactly as before (same
caveat: the "+ both levers" column predates the correctness fixes):

| structure | nonlinear | before islands | islands only | **+ both levers** |
|---|---|---|---|---|
| districts~100 | 4.7% of elements | 205 elements | 617 elements | **2,241 elements** (n=585) |
| districts~100 | none | 414 elements | 1,955 elements | **> 4,107 elements** (still 2.10x at the top of the bracket) |
| one-circuit | 4.7% of elements | 245 elements | 315 elements | 310 elements |
| one-circuit | none | 495 elements | 746 elements | 702 elements |

The one-circuit rows are the control and they say the right thing: a single
connected world is one island, the generator fills it with switching blocks,
so there is nothing to sleep and nothing to dilate — and the levers cost it
nothing measurable (310 vs 315 and 702 vs 746 are inside the bisection's
one-bracket resolution and the machine's noise). **Every districts row moved
because the levers moved, not because the machine did.**

Against the owner's targets: the "simple game" figure of ~3,000 elements is
now within reach for a linear room and 1.3x away for a 4.7%-nonlinear one,
from 15x away before islands and 4.9x away after them.

## Structural win 4: event-driven piecewise-linear reuse — LANDED, and now per island

This one landed on `main` before islands did, was measured there, and is
re-measured here because partitioning changed what it is worth by an order
of magnitude.

An op-amp's and a 555's contribution to **A** is a function of a *discrete*
state (rail region, RS latch), not of the operating point. Between two flips
the matrix is constant, so the previous substep's LU is the factorization of
the same matrix, bit for bit. `ElementKind::needs_newton()` separates those
two devices from the genuinely smooth ones (diode, zener, LED, BJT, MOSFET,
OTA), and `Island::reusable()` keeps the factorization until
`update_guesses` sees a region or a latch actually move.

A 555 astable at 480 Hz flips 957 times per simulated second against 50,000
substeps: **98.1% of substeps reuse**. Op-amp Schmitt relaxation: 909 flips,
98.2%. An op-amp in negative feedback never leaves region 0 at all.

**What islands changed.** `smooth_nonlinear` used to be one flag for the
whole world, so a single diode anywhere disarmed reuse for every op-amp and
every 555 in the room. It is a property of the island now. The control row
below is the whole story: a 555 room with ONE LED dropped in it used to cost
50,957 factorizations per simulated second (one per Newton pass, forever)
and now costs 1,018 — the LED disarms its own district and nothing else.

### Measured, before and after this branch

Medians of 5 runs, each 50,000 substeps at dt = 20 µs = 1.000 s simulated
(`cargo run --release -p sim-golden --bin pwlroom`). "room" = the districts
generator in `sim_golden::scale` at 0% nonlinear, so the padding is what a
room of other people's passive builds looks like. Same machine, same run,
back to back: "before" is `main` at 0475bbf (piecewise-linear reuse, one
matrix for the world); "after" is this branch (islands + quiescence +
local dt on top of it).

| world | elements | n | islands | static | before | after | x |
|---|---:|---:|---:|---:|---:|---:|---:|
| 555 astable alone | 14 | 6 | 2 | 0 | 90.0x | **139.7x** | 1.6x |
| op-amp Schmitt alone | 12 | 4 | 2 | 0 | 125.8x | **203.5x** | 1.6x |
| 555 + room | 172 | 51 | 9 | 7 | 5.28x | **125.5x** | 23.8x |
| Schmitt + room | 170 | 49 | 9 | 7 | 5.61x | **191.2x** | 34.1x |
| both + room | 184 | 55 | 10 | 7 | 4.59x | **81.0x** | 17.6x |
| 555 + room (one connected pad) | 154 | 32 | 3 | 1 | 7.95x | **88.0x** | 11.1x |
| 555 + room + ONE led | 176 | 54 | 10 | 8 | 1.72x | **123.7x** | 71.9x |
| 555 + smooth-nonlinear room | 167 | 61 | 9 | 5 | 1.33x | **93.9x** | 70.6x |
| linear room only | 158 | 45 | 8 | 7 | 6.82x | **2159.8x** | 316x |

The last three rows are the ones that were *controls* before islands landed
— worlds reuse was supposed to be unable to help, because they contain a
smooth nonlinearity. They are no longer controls, and that is the point: the
diode is still refactoring every Newton pass, in its own district, while the
other eight districts do not.

The 555 room's scaling curve, same harness:

| room size | elements | n | islands | before | after |
|---:|---:|---:|---:|---:|---:|
| 40 | 64 | 21 | 4 | 18.4x | **130.2x** |
| 136 | 172 | 51 | 9 | 5.13x | **123.9x** |
| 200 | 239 | 69 | 12 | 3.17x | **95.4x** |
| 300 | 342 | 96 | 17 | 1.71x | **84.3x** |
| 450 | 518 | 145 | 25 | 0.887x | **76.4x** |
| 600 | 668 | 187 | 32 | 0.580x | **68.0x** |

Before, a 555 in a room stopped holding real time somewhere around 450
elements. After, a 600-element room still runs at 68x — the curve is nearly
flat, because the cost is now the ONE district that is oscillating plus a
per-element floor, not the whole world's matrix.

### Exactness

Reuse is bit-identical, not merely close: the reused L/U is the
factorization of a matrix a refactor would have rebuilt to the same bits.
`crates/sim-golden/tests/pwl_reuse.rs` asserts that directly and PER ISLAND
— it compares `Island::matrix()` word for word between a reusing engine and
a refactor-every-substep engine at every substep of every golden, plus the
solution vector, the state hash, the NR pass count, the rescue count and the
quarantine flag, including under live edits and under damage that
reclassifies a world mid-run. It runs with `Tuning::off()` so the two engines
are comparing solves rather than skips.

`a_diode_district_does_not_disarm_reuse_next_door` in the same file is the
per-island statement: one room, a diode district and a 555 district, and the
555 district must still reuse.

## The whole branch, end to end

`cargo run --release -p sim-golden --bin scale --  --sizes 150,500,1000
--structures districts --skip kernel --skip islands --skip crossover
--skip quiescence`, same machine, back to back, shipping tuning:

| elements | islands | sum n | max island n | before (one matrix) | after | x |
|---:|---:|---:|---:|---:|---:|---:|
| 205 | 2 | 52 | 29 | 1.6726x | **7.2061x** | 4.3x |
| 516 | 5 | 136 | 32 | 0.2627x | **2.2403x** | 8.5x |
| 1023 | 10 | 265 | 32 | 0.0671x | **0.9398x** | 14.0x |

The multiplier grows with the room because the thing being removed grows
with the room: `max island n` is flat at 32 while `sum n` is not.

### Exactness, for the whole branch

With both levers off (`Tuning::off()`) this engine is the engine at
`main` 0475bbf, **bit for bit**, on every one of the 18 golden circuits —
same state hashes at 1 µs × 10,000 steps. That is asserted, with the digests
pinned, by `with_both_levers_off_the_engine_is_the_pre_lever_engine_bit_for_bit`
in `crates/sim-golden/tests/golden.rs`, and it holds with factorization reuse
on OR off (`levers_off_and_reuse_off_is_the_same_engine_again`).

It holds because a one-island document gets exactly the node numbering it had
before partitioning: islands are numbered in ascending global-node order and
number their own nodes the same way, so with one island `local == global`.
`every_golden_circuit_is_a_single_island` guards that premise rather than
assuming it.

With the levers ON, 4 of the 18 golden hashes move (`rc_step`, `rl_step`,
`npn_switch`, `opamp_relaxation`) — those are the circuits that legitimately
sleep or dilate, and the trajectory change is the documented,
tolerance-defined semantics of `docs/scale-parallelism.md` §5.2. The
cross-target harness is unaffected in kind: `tools/determinism.sh` reports
native arm64 == wasm32 on all 18.

## Recommendation, in order

1. **Per-island partitioning — DONE**, see "Structural win 1" above for the
   after-measurement (97.7x per substep at 5,122 elements as measured, on a
   machine that was also ~1.7x quieter than the before run). Largest measured
   win, cheapest to build — `compile()` already computed the
   wire-closure union-find that the partition needed. It simultaneously fixes
   the global `linear` flag (a diode in district 7 stops costing district 3
   anything), the global NR convergence (measured 2.09 → 1.03 iterations), the
   fill blow-up (island n stays small, so fill stays near 1.0x), and the
   `O(elements^2)` compile.
2. **Skip quiescent islands — DONE**, see "Structural win 2". Measured
   **3.9x** at the modelled 20% active, **1.9x** at 50%, 194–215x in a
   fully idle room, and 1.02–1.04x (i.e. harmless) in a room where nothing
   is idle. Tracks `1/(1-idle fraction)` exactly. Only possible after 1.
3. **Per-island local dt — DONE** (`docs/plan.md` resolution 1), see
   "Structural win 3". Measured **3.5x** on its own at 20% active and
   15.6x in an idle room; unlike quiescence it still pays 1.08x when every
   district is oscillating. Together with 2 the pair is worth **4.2–4.3x**
   at 20% active — not the product, because the two levers overlap.
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

That is now the measured mechanism rather than the plan: at 20% active on the
5,122-element world, **38.5 of 50 islands are visited zero times per substep**
and the remainder are visited at 1.75x fewer substeps than the clock. The
arithmetic the owner needs for 100k circuits (~2,000,000 elements against a
floor of 300–1,400 element-visits per substep) is the same arithmetic, taken
further: it needs the *visited* fraction to fall to ~0.05%, which is three
more orders of magnitude than the 77% these two levers remove. The remaining
orders have to come from somewhere the solver cannot reach — region
separation, and the fact that a room with nobody in it does not need to be
stepped at all (`docs/plan.md`: "no offline automation, sim pauses in empty
rooms"). What islands + quiescence + local dt establish is that the *engine*
imposes no floor of its own: an idle island already costs literally nothing
per substep, so the ceiling is set by how much of the world is genuinely
moving, which is a game-design question.
