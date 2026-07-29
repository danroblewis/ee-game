# EE Game Simulation Engine — Architecture

## 1. Workspace / crate layout

Cargo workspace, all `no_std`-friendly where practical (not required), zero C dependencies (WASM parity):

```
sim-math/      Deterministic numeric kernels: fixed-order dense LU (partial pivoting),
               fixed-pattern sparse LU (Gilbert–Peierls + AMD), libm-backed exp/ln/tanh,
               NaN canonicalization. No SIMD intrinsics, no FMA. faer as dev-dependency
               (reference oracle in tests only).
sim-core/      Netlist, node mapping, stamping, devices, islands, integrator, NR loop,
               engine state machine, op-log application, snapshot/restore.
sim-digital/   Event layer: MCU (RV32 interpreter), big-chip execute() blocks,
               D/A–A/D pin bridges. Depends on sim-core traits.
sim-probe/     Probe registry, sample rings, min-max decimation pyramid, measurement
               chips (Vrms/Vpp/freq/duty/THD), FFT (Hann/Welch), protocol decoders.
sim-api/       serde-schema'd public types: CircuitOp, InteractOp, Query, SampleBatch,
               StateSnapshot, Diagnostics. The wire format between engine and everything.
sim-native/    Server shell: rayon-over-islands threading, room integration.
sim-wasm/      wasm-bindgen shell: single-threaded, typed-array sample export.
sim-golden/    Golden circuits, cross-target determinism harness, criterion benches.
```

Client and server compile the *same* `sim-core`; only the shells differ. Threading lives entirely in `sim-native` (islands are the parallel unit), so WASM needs no threads/COOP-COEP.

## 2. Netlist & schematic mapping

**Document → netlist.** The schematic document is an op-log of placements: devices (position, rotation, params), board-scale wires, junctions, world-scale wire runs, subcircuit instances. Compilation per island:

1. **Wire closure** (Falstad): union-find merges every board-scale-wire-connected terminal into one electrical `NodeId`. Board wires are never stamped; their currents are reconstructed post-solve by dependency-ordered traversal (`current_into_node` per device). Canonical node key = smallest persistent `TerminalId` in the closure — this key survives re-analysis so node voltage state carries across edits.
2. **World-scale wire runs** are *devices*: gauge-dependent series R/m (temperature-coefficient thermal state), plus lumped series L and shunt C above length thresholds.
3. **Corridor trunk segments** are **Bergeron lossy transmission lines**: characteristic impedance Z₀ = √(L/C), delay τ = max(dt, ℓ/v), series R lumped at both ends. Each end stamps as resistor Z₀ + history current source read from the far end's state one step ago — **exact decoupling**, defining island boundaries.
4. **Subcircuit instances** flatten at compile time (Figma-component semantics live in the document layer; the netlist sees flat copies). Instance-local `TerminalId`s are namespaced by instance path so editing a definition recompiles all instances but preserves per-instance state where terminals still exist.
5. **Ground**: no ground required. Every node gets gmin (1e-12 S) to the reference; each island elects a reference node (lowest canonical id, or an explicit ground symbol if present). DRC pre-checks (floating node, V-source loop, inductor loop) emit named diagnostics, not failures.

**Key structures:**

```rust
struct Island {
  nodes: Vec<NodeSlot>,            // canonical key, voltage state
  devices: Vec<DeviceInstance>,    // enum-dispatched (no dyn in hot path)
  matrix: MnaMatrix,               // dense or sparse variant
  pattern: StampPattern,           // (device, entry) -> nnz slot index
  x: Vec<f64>, b: Vec<f64>,        // solution: node V then branch currents
  linear_base: Vec<f64>,           // static+reactive numeric values (memcpy source)
  flags: IslandFlags,              // linear-only, dirty tier, quarantined
  local_dt: f64, cost_ema: f64,    // budget accounting
}
```

`DeviceInstance` is a Rust enum (`Resistor(..) | Diode(..) | ...`) — enum dispatch keeps stamping monomorphic, fast, and deterministic in iteration order (devices iterated by stable `DeviceId` order always).

**Device model contract** (implemented per enum arm, mirrored as a trait for the MCU/chip layer):

```rust
fn topo(&self) -> Topo;                 // terminals, extra branch vars, Linear|Reactive|Nonlinear|Switching
fn stamp_static(&self, s: &mut Stamper);           // constant conductances/sources → linear_base
fn stamp_reactive(&self, dt: f64, s: &mut Stamper);// companion G_eq (changes only with dt/method)
fn load_rhs(&self, state, s: &mut RhsStamper);      // history I_eq each step
fn stamp_nl(&mut self, x: &[f64], s: &mut Stamper); // per NR iteration: G=dI/dv, I_eq at iterate
fn commit(&mut self, x: &[f64], ctx: StepCtx);      // update state: v_C, i_L, thermal J, i²t
```

`commit` is also where **damage physics** lives: thermal integrators, parameter drift, trip-to-open/short (a trip sets `TopologyDirty` — it is just a value/topology edit issued by the device itself).

## 3. MNA solver

**Stamping API.** At (re)analysis, a pattern pass records every (row, col) each device will touch → `StampPattern` maps to flat nnz indices. Runtime stamping is pure indexed `+=` into a values array — no hashing, no allocation, fixed order (device order → deterministic float accumulation order).

**Three-layer matrix decomposition:**
- `A = A_static + A_reactive(dt) + A_nl(x)`; the first two are baked into `linear_base` whenever values or dt change.
- **Fully linear island** (no `Nonlinear` devices): numeric factorization happens once per value/dt change; each timestep is `load_rhs` + one triangular solve — O(nnz). This is Falstad's core speed trick and covers most beginner circuits.
- **Nonlinear island**: per NR iteration, `values ← memcpy(linear_base)`, then `stamp_nl` deltas, then numeric refactorization, then solve.

**Solver selection per island:** dense LU (own kernel, partial pivoting) for n < ~150 unknowns; above that, own **fixed-pattern sparse LU**: AMD ordering + Gilbert–Peierls left-looking factorization, symbolic analysis computed once per topology change, numeric refactor reusing pattern and pivot order (KLU's `klu_refactor` pattern). Pivot-growth monitored each refactor; growth beyond 1e8 triggers a full repivoting factorization (rare — after big value swings). Trivial-row elimination (Falstad's `simplifyMatrix`) applied at pattern build to shrink n. Rationale for hand-rolling (~1–2 kLOC): KLU is C (WASM pain), faer's SIMD kernels may emit FMA and target-dependent reduction orders — fatal to bit-determinism. Circuit matrices (3–5 nnz/row, ≤ low-thousands unknowns) don't need state-of-the-art kernels.

**Newton–Raphson loop** (per nonlinear island per substep):
1. Predictor: previous solution (no extrapolation — determinism-simple).
2. Iterate: restamp → refactor → solve → **per-device limiting** (diode pnjlim capping updates near v_crit = nV_T·ln(nV_T/(√2·I_s)); MOSFET vgs/vds step caps) → convergence test `|Δv| < reltol·|v| + vntol` (1e-3 / 1e-6), branch currents likewise.
3. Cap 40 iterations. On failure, escalate the fallback ladder.

**Fallback ladder** (per island, in order):
1. NR damping (halve Δ) for remaining iterations.
2. Switch integrator to backward Euler for this step (L-stable, forgiving).
3. Halve `local_dt` (floor 50 ns), retry; regrow dt by ×1.05/step after 30 clean steps (circuitjs #853 pattern).
4. Temporary gmin brace: gmin ×1e6, resolve, step gmin back down in decades over subsequent steps.
5. **Quarantine**: freeze the island, zero non-finite state, raise a named diagnostic ("solver fault in <region>", highlighted). Neighbors see the island through Bergeron ports as its last valid state decaying to open. NaN/Inf scan on every solution vector; any hit jumps straight to quarantine. No DC operating-point solve ever — simulation starts from zero state; the startup transient is the OP (avoids the entire gmin/source-stepping continuation apparatus).

## 4. Integration & timestep / real-time budget

- **Method:** trapezoidal by default; **1–2 forced backward-Euler steps per island after any switching event** (switch/relay/breaker/trip, gate output transition, diode region change) to kill TR ringing. Companion models per §1 research (TR: G=2C/h etc.).
- **Timestep:** room-nominal fixed dt = 10 µs (configurable 5–20 µs). All islands in a *sync group* (connected via Bergeron lines) advance the same global t in lockstep; Bergeron delay ≥ dt means each island's step is independent → islands solved in parallel (rayon on server, sequential in WASM).
- **Budget:** the engine's `advance(wall_budget, max_sim_time)` runs substeps in batches. Per-island EMA of step cost feeds a budget allocator. Overload response, in order: (a) raise the offending island's `local_dt` (island substeps every k-th global step, ZOH on its Bergeron ports — accuracy degrades locally); (b) if still over budget, **dilate the whole sync group's sim clock** (fewer substeps per wall frame — Falstad's slow-sim-never-UI rule); truly disconnected islands (islanded plots) dilate independently. `AdvanceReport` returns achieved sim-time ratio per sync group for UI display ("plot running at 0.3×").
- **Capacity math:** 2–16 players, low-thousands of unknowns per room. Linear islands: n=200 sparse solve ≈ a few k flops/step → 100k steps/s trivial. Nonlinear islands: 2–4 refactors/step at n=500 ≈ 50–200k flops/step — one server core sustains several such islands at dt=10 µs real time; island threading adds headroom. Client WASM predicts only the local player's island + immediate Bergeron neighbors at reduced substep rates.

## 5. Live editing / incremental re-stamping

All mutations arrive as `CircuitOp`s (the same op-log the netcode replicates and undo replays). Applied only at substep boundaries. Three dirty tiers per island:

- **RhsDirty** (interactive source waveform changes, MCU pin drives): nothing to do — RHS is rebuilt every step anyway.
- **ValueDirty** (knob-drag R/C/L/params, thermal drift, trip-to-value): restamp `linear_base`, numeric refactor. Sub-millisecond → knob-drags update within one frame.
- **TopologyDirty** (place/delete/rewire, subcircuit edit, trip-to-open, cut/splice): recompile *that island only* — wire closure, pattern build, symbolic analysis. State preservation: node voltages rekeyed by canonical terminal key; reactive/thermal state lives inside devices and survives untouched; genuinely new nodes start at 0 V (gmin makes that safe). A splice on a corridor splits/merges islands — union-find recompute is scoped to affected sync group.

Switch/pot/button interaction is an `InteractOp` fast path: switches are pre-stamped as two-state conductance pairs (closed 1e-3 Ω / open gmin) so toggling is ValueDirty, not TopologyDirty.

## 6. Digital / mixed-signal layer

**Two tiers, per research recommendation:**

- **Stamped-analog gates** (glue logic, flip-flops, 555, comparators): outputs are voltage-source stamps whose value is computed in `stamp_nl` from input node voltages with Schmitt hysteresis and slew-limited transitions; flagged `Nonlinear` so combinational chains settle inside one substep's NR iterations. Because outputs are real sources with finite output impedance, they can be loaded, shorted, back-driven, and jammed — required by the warfare design.
- **Event layer** (`sim-digital`): MCU block = cycle-budgeted **RV32 interpreter** (integer-only → trivially deterministic), executing `clock_hz × dt` cycles per analog substep; big chips implement `setup_pins()/execute()`. **All events quantize to analog dt boundaries** — no event queue interleaving, no analog step rejection/backtracking (dt = 10 µs makes sub-dt timing gameplay-irrelevant). Pin bridges: output pins stamp slew-limited Thevenin sources (D/A); input pins sample node voltage through a Schmitt comparator at `commit` time (A/D) — hysteresis prevents chatter at thresholds. Execution order fixed: analog solve → commit → digital execute (reads this step's voltages, drives next step's stamps).

## 7. Probes & measurement streams

- A probe is a registered extractor: **voltage probe** = index into `x`; **current probe** = branch variable index (V-source/inductor branches) or a per-device current formula evaluated at `commit`; **wire current** = Falstad-style post-solve reconstruction (computed only for probed wires).
- Per substep, each probe appends one f64 (stored f32) to a ring buffer. `drain_samples()` returns `SampleBatch { probe_id, t0, dt_eff, samples }` per engine tick; dt_eff reflects island dilation so time axes stay honest.
- `sim-probe` consumes batches identically on client (scope rendering) and server (contract verification): **min-max decimation pyramid** (levels of 2×) for artifact-free zoomed-out scope traces and cheap lossy streaming; incremental **measurement chips** (Vrms/Vpp/ripple/freq/duty/rise/THD/phase) as streaming folds; FFT (Hann, 4096, Welch averaging); stackable decoders (UART first). Contract verifiers are just measurement chips + assertion bands attached to NPC-load terminals — one measurement stack, per pillar 3.
- Probes are room-scoped entities on the authoritative tick (shared clock), created/destroyed via `CircuitOp` like everything else.

## 8. Determinism (native ↔ WASM)

Target: **bit-identical** state evolution for identical op sequences.

1. All hot-path float code is our own scalar Rust: fixed operation order, no data-dependent reduction reordering, no SIMD, no `mul_add`/FMA anywhere (CI greps + inspects for `fma` in generated code).
2. Transcendentals (diode exp/ln, sources' sin) via the pure-Rust **`libm`** crate on *all* targets — never platform libm.
3. Device and island iteration in stable id order; rayon parallelism is per-island with no cross-island float accumulation, so threading cannot reorder math.
4. NaN canonicalization at quarantine boundaries; NaNs never enter persistent state.
5. RNG (fault events, noise sources): explicit seeded PCG64 in engine state, advanced only by ops/steps.
6. **Enforced by CI**: every golden circuit runs native (x86-64 + aarch64) and under wasmtime; xxhash3 of the canonical `StateSnapshot` must match at checkpoints. Netcode still assumes drift *can* happen: client prediction resyncs from periodic authoritative snapshots regardless, so determinism is an optimization (perfect prediction), not a correctness dependency.

## 9. Public API (`sim-api`, consumed by server and client shells)

```rust
impl Engine {
  fn new(cfg: EngineConfig) -> Engine;                       // dt, budgets, tolerances, seed
  fn apply_ops(&mut self, ops: &[CircuitOp]) -> Vec<OpAck>;  // authoritative op-log; validation + DRC diagnostics
  fn interact(&mut self, op: InteractOp);                    // switch/knob/button fast path (still logged)
  fn advance(&mut self, wall_budget_us: u32, max_sim_us: u32) -> AdvanceReport; // substeps; report: sim-time ratio,
                                                             //   per-island health, trips, NR stats, diagnostics
  fn add_probe(&mut self, spec: ProbeSpec) -> ProbeId;
  fn drain_samples(&mut self) -> Vec<SampleBatch>;
  fn query(&self, q: Query) -> QueryResult;                  // instantaneous V/I/P, device state, thermal, DRC, meters
  fn snapshot(&self) -> StateSnapshot;                       // full deterministic state (netlist + x + device state + rng)
  fn restore(&mut self, s: &StateSnapshot);
  fn state_hash(&self) -> u64;                               // xxhash3, drift detection
}
```

Server: owns the room `Engine`, applies validated ops, calls `advance` at 50 Hz tick, streams snapshots/deltas and probe batches. Client WASM: same `Engine` behind wasm-bindgen; applies predicted local ops + server ops, `restore`s on authoritative snapshots, exports samples as zero-copy `Float32Array`. Energy metering (`∫V·I dt` at service entrances) is a device (`Meter`) queried via `query`, not a special engine feature.

## 10. Test strategy

- **Golden circuits (analytic):** resistor ladders/dividers (exact), RC/RL step response vs closed-form exponential (assert within LTE bound for TR at given dt), series RLC ring (frequency + decay envelope), bridge rectifier ripple, op-amp inverting/non-inverting gain, Zener clamp levels, 555 astable frequency, boost-converter steady-state ripple.
- **Reference envelopes:** same netlists run in ngspice offline; stored traces with tolerance envelopes (never comparing to GPL code, only to published physics).
- **Property tests (every step, debug builds):** KCL residual |A·x−b| < tol; passive-network energy non-increase; no NaN/Inf outside quarantine; snapshot→restore→advance ≡ advance (state completeness).
- **Determinism CI:** cross-target hash matching (§8); op-log replay from checkpoint reproduces hashes.
- **Adversarial fuzzing:** random and mutation-based netlists (V-source loops, L loops, 0-Ω shorts, 1e12-ratio values) — engine must never panic; quarantine + diagnostic is the only acceptable failure.
- **Perf gates:** criterion benches per phase (linear n=200 step < 2 µs; nonlinear n=500 step < 50 µs native), regression-tracked.

## 11. Build order

1. **Linear core:** `sim-math` dense LU; nodes/stamping/pattern; R, V/I sources; resistor-divider goldens; KCL property test.
2. **Dynamics:** C/L companion models, TR + BE, fixed-dt loop; RC/RLC goldens; LTE validation.
3. **Nonlinear:** NR loop, diode + pnjlim, convergence ladder, switch (two-state), BE-after-switch; rectifier golden; fuzz harness starts here and never stops.
4. **Scale:** trivial-row elimination, fixed-pattern sparse LU + refactor reuse, island partitioning + Bergeron corridors, per-island budgets; perf gates.
5. **Liveness:** op-log application, dirty tiers, state-preserving recompile, InteractOp knob-drag path; snapshot/restore + hashing.
6. **Probes:** registry, sample batches, decimation, measurement chips; contract-verifier assembly.
7. **MVP devices:** pot, relay, fuse/breaker (i²t), battery/finite generator, AC source, Zener, LED, lamp, DC motor (back-EMF companion), speaker, op-amp, NPN, wire runs with thermal R(T); trip-and-repair state machine.
8. **WASM:** `sim-wasm` shell, determinism CI cross-target, prediction-mode config (reduced substeps/neighborhood).
9. **Digital:** stamped gates/comparator/555 hysteresis models → event layer + RV32 MCU + pin bridges.
10. **Post-MVP:** FFT/decoders in `sim-probe`, transformer + AC-era elements, optical/RF coupling stamps, adaptive-dt polish, 16-player scaling passes.

## 12. Key risks & mitigations

- **Hand-rolled sparse LU correctness/effort** (~2 kLOC): highest-risk component. Mitigate: faer-as-oracle differential testing on random SPD-ish and real circuit matrices; dense path is the always-correct fallback (raise the dense threshold if sparse slips schedule — MVP island sizes mostly fit dense).
- **Determinism erosion:** any future dependency or SIMD "optimization" can silently break it. Mitigate: hash-matching CI is mandatory and blocking from phase 8 onward; netcode never *requires* determinism.
- **Adversarial convergence:** players will build ill-conditioned monsters deliberately. Mitigate: fuzzing from phase 3, quarantine as guaranteed terminal state, per-island budgets so one monster never stalls the room.
- **Bergeron latency semantics:** one-dt corridor delay is physically honest but must be tuned so short corridors don't feel laggy; fallback is merging small adjacent islands into one matrix (engine supports both).