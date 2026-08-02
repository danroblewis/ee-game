# As built: determinism, time, and the road to scale (islands)

*Status: commit `0475bbf` (main, August 2026). The islands sections describe
measured benchmark results and an integration **in flight** — they are explicitly
NOT the shipping solver, and this doc says which is which. All scale numbers are
Apple M4 release builds from `docs/scale-baseline.md` and
`docs/scale-parallelism.md` (July 2026), some taken under measured CPU contention —
the ratios are the trustworthy part; treat absolute times with a grain of salt, as
those docs themselves do.*

---

## 1. Determinism as an architectural constraint

**Problem.** Two independent systems must agree *exactly*: the server's native
solver and the browser's WASM copy of it. Not "close" — bit-for-bit. Floating point
makes that hard: FMA fuses roundings, SIMD reorders reductions, platform `libm`s
differ in the last ulp, hash maps iterate in seeded orders, and NaN payloads are
the one place IEEE-754 lets targets differ.

**Mechanism — what is enforced, all verified in this tree:**

- `sim-math` bans FMA/`mul_add`, SIMD, fast-math, and platform libm by policy
  (crate header) and uses only plain scalar f64 arithmetic. Even `abs` is a
  hand-written bit mask — one implementation to audit.
- Transcendentals in `sim-core` come exclusively from the `libm` crate (software
  implementations, identical on every target).
- The toolchain is pinned (`rust-toolchain.toml`, 1.95.0, wasm32 target): "bump
  deliberately and re-run the harness."
- `sim-core` has no I/O, threads, or clocks — its dependency list is `sim-math`,
  `libm`, a hasher, optional serde.
- `state_hash()` is xxh3 over NaN-*canonicalized* little-endian f64 bits of time,
  the solution vector, and every element's continuous and discrete state. New
  state classes (broken ids, noise counters) hash only when present, precisely so
  historical golden hashes never moved.
- The harness (`tools/determinism.sh`, CI-enforced): run the golden circuits
  10 000 steps natively and under wasm32 in Node, diff the hash lists, fail on any
  bit difference.
- The constraint reaches surprising places, and knowing this saves future
  contributors from "harmless" changes: `BTreeMap`s instead of hash maps anywhere
  iteration order matters; the source-merge grouping is an integer-keyed linear
  scan in document order; the gate's trial-depth arithmetic is integer so every
  target buys the same depth; the validator formats SI prefixes via a threshold
  table specifically to avoid a transcendental; the noise source is counter-based
  integer SplitMix64 *because* of this invariant; a source's phase is not reduced
  mod 2π because π is not exactly representable.

**What it buys.** The client-side placement gate that can never disagree with the
server (see [`asbuilt_authority.md`](asbuilt_authority.md)) — that whole design is
only sound because both sides run the same bits. Replay and state-hash debugging.
Golden-circuit regression at the precision of "one bit moved." And a future
client-side preview that cannot drift from the server.

**What it forbids.** GPU compute — ruled out on determinism grounds *and*
independently on latency: 1 667 dependent substeps per tick × ~30 µs dispatch
overhead is a ~50 ms floor against a 33 ms tick (measured in
`docs/scale-parallelism.md`). Fast-math anywhere. Per-target codegen cleverness.
What it does **not** forbid: per-island CPU parallelism — islands share no memory,
and `scale-parallelism.md` reports a 68-engine serial-vs-rayon hash comparison
coming back identical — with the corollary that *partitioning itself* re-baselines
hashes once (different matrix, different pivot order).

## 2. Time: dt, the budget, and dilation

**Problem.** A fixed-timestep simulation must coexist with a fixed-rate game loop
on hardware of unknown speed, without ever stalling the UI (pillar 2) and without
faking results when it can't keep up.

**Mechanism.** The engine's substep is fixed at construction (dt = 20 µs as
shipped). The server tick is 30 Hz with `MAX_STEPS_PER_TICK = 8000` (~5× the
nominal ~1 667 substeps — headroom for catch-up, not sustained overload). The
enforcement is `advance(max_steps)`'s very signature: the engine takes a step
*count*, never a deadline, so when a room is too heavy, the server simply runs
fewer substeps than nominal and **sim time falls behind wall time**. Frame rate and
tick rate are untouched; the room's clock dilates.

Dilation is then *surfaced*: the realtime ratio rides every audio message, the
audio worklet slaves pitch to it (a sim at 0.9× plays flat — see
[`asbuilt_client.md`](asbuilt_client.md)), and the dock displays "sim 0.9×".

**Configuration honesty.** `docs/plan.md` resolution 1 puts *nominal* dt at 10 µs
and the tick at 60 Hz. Everything measured, and the shipped server, runs 20 µs at
30 Hz (the client's offline fallback runs 10 µs). Every real-time ratio in the
scale docs roughly halves at the plan's nominal settings — always state which
configuration a number belongs to.

## 3. Why one matrix hurts (measured)

**Status first:** in this tree the world solves as **one matrix**.
`Engine::unknowns`' doc comment says so in as many words; disconnected islands
share it. Everything in §4–5 is measured in benchmark harnesses
(`sim-golden/tests/scale.rs` and the scale docs) but **not merged into the live
solver**. An integration branch (`wf/islands-integrate`) exists with no commits
beyond main at the time of writing.

The measured pain (Apple M4, release, dt = 20 µs, `docs/scale-baseline.md`):

- **One smooth-nonlinear device anywhere makes the whole world re-stamp and
  re-factor every NR iteration**: 52 000–150 000 refactorizations per simulated
  second, 60–95 % of substep cost. (Since fixed for op-amp/555-only worlds by PWL
  factor reuse — see [`asbuilt_solver.md`](asbuilt_solver.md) — unchanged for
  diodes and transistors.)
- **Newton convergence is global**: mean iterations per substep grow 1.05 → 2.50
  from 516 to 51 000 elements. The same 5 122-element world measured 2.09
  iterations as one matrix vs 1.03 per island — everyone pays for the hardest
  circuit's bad night.
- **The real-time ceiling today** is roughly 200 elements for a nonlinear mix,
  ~400 linear; the shipped 143-element demo room measured 0.47–0.66× real time
  (i.e. already dilated) in `scale-parallelism.md` §1.2.

## 4. Islands: an exact decomposition, not an approximation

**The idea.** Most player builds are electrically disconnected from each other.
Partition the world into per-circuit matrices and solve each alone.

**Why it is *exact* — the node-0 argument.** Ground has no row in the matrix
(see [`asbuilt_solver.md`](asbuilt_solver.md) §2). Two circuits sharing only a
ground symbol therefore share **no unknown**; the world matrix is block-diagonal;
Newton on a block-diagonal system is Newton per block; and the rescue ladder only
ever gives a block a smaller step than it would take alone. Same verdict, same
physics, different bookkeeping. This is not a hoped-for property: the placement
gate already ships the same mathematics, judging documents one MNA block at a time
(`validate.rs`, `split_blocks`), and states the licence argument in full.

**Measured wins** (benchmark harnesses, Apple M4, release, July 2026):

| world | monolithic | per-island | win |
|---|---|---|---|
| 5 122 elements / 50 districts | 0.0010× real time | 0.0666× | **68.4× per substep** (58.6× on the factor; four-run spread 66.6–76.7×) |
| golden tiles, 113 elements | — | — | 8.6× |
| golden tiles, 4 520 elements | ~E^2.7 scaling | linear, 0.062–0.075 µs/element/substep | **467×** |

**The multipliers islands unlock** (both measured in harnesses, both changing
semantics deliberately, neither shipping):

- **Quiescence** — skip circuits that have gone static. You cannot skip *part of*
  one matrix, which is why this is islands' multiplier rather than an alternative.
  Measured: DC-only districts go fully static within ~250 ms of sim time with zero
  exceptions; 13 of 17 goldens quiesce, covering 62 % of solve cost on that mix.
  It is a new, deliberate tolerance semantics (a frozen island stops accumulating
  GMIN drift), not a free lunch.
- **Per-island local dt** — 12 of 17 goldens are bit-quiet or sub-nanovolt at
  100× dt; oscillators need k = 1 (their large "errors" are phase, not amplitude).
  The k decision must derive from deterministic state, never from load, or
  determinism dies.

**Why islands are necessary and not merely nice:** the measured per-element
bookkeeping floor (14–68 ns per element per substep, against a 20 µs budget) means
a 3 000-element world cannot be reached by *any* faster solver alone — most of the
world must not be **visited** at all, which is exactly what islands + quiescence +
local dt buy. (`scale-baseline.md`'s closing argument.)

**Also in the measured-not-fixed column:** `frame()`'s wire-current KCL recovery
is O(elements²) via a linear id scan and measured 8.8 ms/frame at 4 520 elements;
compile-time junction interning had the same shape and *was* fixed (BTreeMap
interning); `frame()` has not been.

## 5. What re-baselines when islands land

Written down so nobody is surprised: partitioning changes matrix layout and pivot
order, so **golden state hashes move once**, deliberately, harness re-run and
re-blessed (the scale doc says exactly this). Quiescence and local dt are
*semantic* knobs that must ship as explicit, documented tolerances — not silent
optimizations — because pillar 1 ("every number is the solver's") is the thing
they trade against at the margin.
