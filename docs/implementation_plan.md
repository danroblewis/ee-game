# EE Game — Implementation Plan

Greenfield repo at `/Users/danroblewis/ee-game`. Rust authoritative sim (native server + WASM client preview), TypeScript frontend, one sim per room, 2–16 players. This plan merges the sim-engine, backend-netcode, and frontend architecture docs; contradictions between them are resolved in §2 and those resolutions are binding.

## 1. Monorepo layout

```
/ee-game
  Cargo.toml                 # workspace
  crates/
    sim-math/       # deterministic kernels: dense LU (partial pivot), fixed-pattern
                    #   sparse LU (AMD + Gilbert–Peierls), libm transcendentals,
                    #   NaN canonicalization. No SIMD, no FMA. faer = dev-dep oracle only.
    sim-core/       # netlist, wire closure, stamping, devices (enum dispatch),
                    #   islands + Bergeron corridors, TR/BE integrator, NR + fallback
                    #   ladder, dirty tiers, snapshot/restore/state_hash, measure/
                    #   contract verifiers. No tokio, no I/O — compiles native + wasm32.
    sim-probe/      # probe registry, sample rings, min-max pyramid, measurement
                    #   chips, rustfft FFT, protocol decoders (UART first).
    sim-digital/    # post-MVP: stamped gates already in sim-core; this = event layer,
                    #   RV32 MCU, pin bridges.
    protocol/       # single source of truth for wire + doc types: CircuitOp,
                    #   Mutation, InteractOp, Tier-B delta codec, probe-chunk codec,
                    #   StateSnapshot, part catalog data. serde + postcard. Compiled
                    #   into sim-wasm so TS never hand-maintains a schema.
    server/         # axum: lobby.rs, registry.rs, room/{task,doc,interest,probes,
                    #   sim_thread}.rs, persist.rs (SQLite/rusqlite), transport.rs
                    #   (3-class WS queues).
    sim-wasm/       # wasm-bindgen facade: init/applyOps/interact/advance/reseed/
                    #   drainSamples/compile + protocol encode/decode. Zero-copy
                    #   Float32Array export.
    sim-golden/     # golden circuits, ngspice envelopes, cross-target determinism
                    #   harness (wasmtime), criterion benches, netlist fuzzer.
  packages/                  # pnpm workspaces, Vite, strict TS
    protocol/       # TS type mirrors + part catalog JSON; hot-path codecs delegate
                    #   to sim-wasm; low-rate messages via serde-wasm-bindgen.
    canvas/         # WebGL2 engine: camera (f64 + floating origin), chunked spatial
                    #   hash, band passes + crossfade, instanced SDF wires, dot pass,
                    #   voltage texture, MSDF text, perf HUD.
    editor/         # input router, tool state machines, hit-testing, DocStore +
                    #   optimistic rebase, undo (inverse ops), picker, copy/paste blobs.
    scope/          # scope WebGL renderer, panel manager, ScopeKernel client.
    net/            # net-worker, transport abstraction (WS now, WebTransport later),
                    #   FrameBus (SAB + postMessage fallback), snapshot decoder,
                    #   probe SAB rings, reconciliation.
    app/            # React 19 + Zustand chrome, worker bootstrap, routes.
  tools/            # chaos proxy, headless bot client, 4-client op interleaver.
```

## 2. Contradiction resolutions (binding)

1. **Timestep policy.** sim-engine says fixed dt=10 µs with per-island dilation; backend-netcode says 1 kHz base with adaptive halving. **Resolution: sim-engine wins on numerics** — room-nominal fixed dt = 10 µs (configurable 5–20 µs), never adaptively raised for accuracy reasons; overload response is island `local_dt` k-step multiples + sync-group sim-time dilation. The backend doc's "adaptive halving on NR trouble" is subsumed by the engine's fallback ladder (BE step, dt-halving to 50 ns floor, regrow ×1.05).
2. **Tick rate.** sim-engine says 50 Hz, backend says 60 Hz. **Resolution: 60 Hz server tick** (aligns with client frame pacing and 20 Hz = every 3rd tick snapshots). Ops/`NetlistPatch` apply at tick boundaries only.
3. **Island coupling.** sim-engine specifies Bergeron lossy transmission lines (exact one-dt decoupling); backend says "solve against previous-substep boundary values" (explicit coupling). **Resolution: Bergeron model** — it is the physically honest version of the same decoupling and defines island boundaries. Fallback for laggy-feeling short corridors: merge small adjacent islands into one matrix (engine supports both).
4. **Determinism vs prediction.** sim-engine targets bit-identical native/WASM; netcode says no determinism requirement. **Resolution: both.** Determinism is a CI-enforced property of sim-core (hash-match gates from M5 on) because it makes prediction perfect and replay debugging possible; netcode never *depends* on it — the client is always a preview that hard-reseeds from authoritative snapshots.
5. **Reconciliation model.** Frontend doc mentions "re-simulate pending local inputs from that tick" (rollback-flavored); backend says hard reseed, never physics rollback. **Resolution: hard reseed.** On each Tier-B snapshot: reseed continuous state (v_C, i_L, source phase), snap discrete state, then re-apply *pending unacked doc ops* (a document rebase, not input replay). 2–3-frame display-value blend masks pops.
6. **Solver library.** Research brief suggested faer; sim-engine hand-rolls. **Resolution: hand-rolled dense + fixed-pattern sparse LU** (faer's SIMD/FMA kernels break bit-determinism); faer is the differential-test oracle. Dense path (n < ~150) is the always-correct fallback if sparse slips.
7. **Wire types ownership.** Backend puts codecs in Rust `protocol` via WASM; frontend has `packages/protocol` with TS codecs. **Resolution:** binary codecs live in Rust `protocol`, exposed through sim-wasm; `packages/protocol` holds TS types + the data-driven part catalog only.
8. **Crate naming.** sim-engine's `sim-api` = backend's `protocol`. **Resolution: one crate, `protocol`.** sim-engine's `sim-native` folds into `server/room/sim_thread.rs` (rayon-over-islands lives there).

## 3. Critical risks and de-risking spikes (M0 — do these first)

Ordered by (impact × uncertainty). Each spike is timeboxed 3–5 days, produces a go/no-go note and keeper code where possible.

- **S1 — Cross-target bit-determinism.** Dense LU + diode NR loop on 3 circuits; run x86-64, aarch64, wasmtime; xxhash3 of state at 10k steps must match. Verifies no-FMA discipline, libm parity, codegen surprises. *Fallback if broken:* prediction still works (resolution 4); drop the CI gate, keep reseed cadence tighter.
- **S2 — Renderer throughput.** WebGL2 instanced SDF wires + animated dot pass + R32F voltage texture: 10k segments + 5k dots at 60 fps on integrated GPU. *Fallback:* LOD governors, dot→glow threshold raised.
- **S3 — Fixed-pattern sparse LU.** AMD + Gilbert–Peierls + refactor-without-repivot vs faer oracle on 200 random circuit-shaped matrices; perf target n=500 refactor+solve < 50 µs native. *Fallback:* raise dense threshold; MVP islands mostly fit dense.
- **S4 — Three-class WS transport + chaos proxy.** Bounded queues, drop policies, `gap` flags; property tests under injected latency/loss/reorder. Backpressure bugs are silent-corruption-shaped — build the harness before the features.
- **S5 — COOP/COEP + SAB pipeline.** Verify cross-origin-isolated hosting (Vite dev + prod), SAB ring between net-worker/main, and the postMessage FrameBus fallback. Decides hosting config day one.
- **S6 — Zoom crossfade feel.** Throwaway prototype: schematic↔world band crossfade with hysteresis on one lamp circuit. This is a *design* risk (signature moment); validate with hands on wheel, not argument.
- **S7 — Bergeron corridor latency feel.** Two islands joined by a 1-dt line; confirm short corridors don't feel laggy; tune merge threshold.

Remaining top risks tracked per-milestone: adversarial NR convergence (fuzzing from M1, quarantine as terminal state), netlist-patch refactor spikes (p99 patch-apply metric from M4), world-band art cost (few faceplates + procedural styling), rebase edge cases (delete-vs-edit fuzz, M4).

## 4. Milestones

Each milestone ends with a demoable vertical slice. Rust and TS tracks run in parallel where noted.

### M0 — Spikes + skeleton (the 7 spikes above)
Also: repo scaffolding, CI (cargo test/clippy/fmt, wasm-pack build, vitest, wasmtime harness), pnpm+Cargo workspaces, perf HUD stub.
**Demo:** spike report; 10k-wire canvas at 60 fps; hash-matching sim step across three targets.

### M1 — Falstad parity, single-player, local (sim in browser)
*Rust:* sim-math dense LU; sim-core nodes/wire-closure/stamp-pattern; devices R, V/I sources, switch (two-state ValueDirty), C, L (TR companions + BE-after-switch), diode + pnjlim; NR loop + full fallback ladder + quarantine; fixed-dt advance with wall budget; `apply_ops`/`interact`/`query`; sim-wasm facade. Golden tests: divider (exact), RC/RL step vs closed form, RLC ring, rectifier ripple. KCL + no-NaN property tests. Netlist fuzzer starts here and never stops.
*TS:* canvas package (camera, chunks, wires, components, MSDF text); sim-worker hosting WASM; SimFrame SAB (double-buffered, generation-stamped); voltage texture + color ramp; current-dot pass; hover live-value tooltips; switch/pot click interaction.
**Demo: battery → switch → lamp(resistor) on the canvas; flip the switch, dots flow, wire colors change, tooltip shows live volts. Feels alive, no run button.**

### M2 — Editor
Tool state machine (Select with Falstad drag semantics, Place, transient Pan/Zoom); grid-snapped wiring with waypoints, auto-junction dots; net-highlight-on-hover + open-pin markers (from Rust `compile()` via WASM — connectivity has exactly one implementation); type-to-search radial picker with recents (data-driven catalog); vertical value-drag with unit-aware log sweep + dialog with expressions; marquee multi-select; local undo/redo as inverse ops; copy/paste human-readable text blobs (blueprint format); diegetic DRC hint panel fed by engine diagnostics.
**Demo: build a bridge rectifier from a blank canvas in under two minutes, mouse-only, live the whole time; break it and the DRC names the problem.**

### M3 — Instrumentation v1 (still local)
*Rust:* sim-probe — probe registry, `drain_samples` batches, min-max pyramid, measurement chips (Vpp/Vrms/mean/freq/duty/rise), rustfft FFT (Hann/4096/Welch); analysis-worker WASM build.
*TS:* click-wire=V-probe, click-component=current-clamp, pinned color-coded flags; scope package — GPU ring-buffer trace renderer, auto-trigger (hysteretic mid-level edge + autocorrelation period → timebase 2–4 cycles), 1-2-5 auto-scale, bottom-docked auto-tiling panels, drag-to-overlay/split, math channel A−B, measurement chips at 10 Hz direct-DOM; one FFT view; in-canvas mini-waveforms.
**Demo: probe the rectifier: scope auto-locks the ripple, chips read Vrms/ripple/freq, knob-drag the cap and watch ripple shrink live in scope + FFT.**

### M4 — Multiplayer document (the netcode risk core)
*Server:* axum lobby (`POST /session` auth-lite token, create/join by 6-char code), registry, room task; Document (property-LWW objects, ObjId=(clientNo,counter)), op pipeline (permission → validation → seq → apply → broadcast), atomic multi-mutation ops; op-log append (SQLite WAL, dedicated writer); transport.rs class-1 path from S4.
*Client:* net-worker, DocStore confirmed+pending with optimistic rebase, OpRejected rollback + toast, presence cursors (class 2), per-player undo through the log, late-join Welcome flow.
*Then* sim thread integration: `NetlistPatch` at tick boundaries, dirty tiers (Rhs/Value/Topology, state-preserving recompile keyed by canonical terminal id), Tier-B snapshots (full frames, no delta yet), client renders server truth.
Verification: randomized 4-client op interleaver vs single-threaded reference (doc convergence); delete-vs-edit fuzz.
**Demo: two browsers co-edit one circuit through the server; player A flips the switch, player B's lamp lights within a frame budget. "Two friends see the same lamp light."**

### M5 — Prediction, interest, probe streaming
Snapshot delta encoding (32-deep per-client ring, ack piggyback, 16-bit quantized fields, keyframe on stale baseline); `rstar` R-tree interest management from ViewportUpdate, interest-table indices, out-of-view aggregates, server-side visibility masks (anti-wallhack); WASM preview: sim-worker runs interest-set netlist with Thevenin boundary drives, hard-reseed per snapshot + display blend, FrameComposer (predicted island / server elsewhere / boundaries always server); ParamPreview knob-drag streaming (class 2) + commit op on release; probe subscriptions (server-side min-max decimation keyed to timebase×pixelWidth, class-3 chunks with gap flags, shared decimation buffers), client probe SAB rings, tick-aligned cross-player probe overlay; determinism CI gate goes blocking (native↔wasmtime hash match on goldens).
Reseed-delta debug overlay ships here (risk 2 of backend doc).
**Demo: 4 players, ~1k components; local edits feel zero-latency, remote circuits animate smoothly between 20 Hz snapshots; two players overlay probes of the same node on one scope, traces align by tick. Chaos proxy on: no corruption, scopes show gaps not lies.**

### M6 — Two worlds: world band, real devices, damage
*Rust devices:* pot, relay, fuse/breaker (i²t interrupt), Zener, LED, lamp (P=I²R glow state), DC motor (back-EMF + inertia companion), speaker, op-amp, NPN, battery/finite generator, AC source; world-scale wire runs (gauge R/m, R(T) thermal state, lumped L/C over thresholds); Bergeron corridor segments; thermal-overstress → drift → trip-open/short state machine (trip = self-issued ValueDirty/TopologyDirty op); Meter device (∫V·I); per-island rayon on server + budgets + sim-time dilation with `AdvanceReport` ratio.
*TS:* world-band faceplate pass (lamp glow, motor rotor, speaker WebAudio from node voltage, smoke on overstress), corridor glow ∝ power, band crossfade with hysteresis (from S6), LOD rules, "plot at 0.3×" badge, "why is this off?" inspector.
Map scaffold: handcrafted map — 2 source taps, 3 NPC load sites, plots, one corridor with 3 thermal grid segments + Party Line trunk, per-plot service entrance (unbypassable auto-reset main fuse + meter).
**Demo: wheel-zoom from a glowing village down into a lamp's filament schematic in one gesture; overload a feeder, watch the district brown out and the wire run heat, trip, gray out, then repair it.**

### M7 — Game shell: contracts, economy, conflict, persistence
Contracts: server-side verifiers = sim-core measurement chips + assertion bands at NPC-load terminals (payouts derive only from server measurement — anti-cheat); 5 DC contracts (light lamp → regulate brightness → motor-RPM uptime through a fuse → ripple spec → survive scheduled line fault via server-injected real shorts); failed specs show the violating waveform; Zachtronics efficiency histograms (part count / energy efficiency / copper) vs room population. Economy: joule-credits, component shop from catalog (price gates ratings/quantity, never access), repair costs with caps, copper per meter. Conflict verbs: splice-tap on neutral ground (tap impedance is real), cutter with charge-up inrush signature, TDR flow (pulse source + scope = locate splice); plot claims/permissions enforced per-op; islanding relay prefab. Basic subcircuits: Create Block with auto-port detection, edit-in-context (breadcrumb, dimmed-but-live surroundings, Figma def semantics), blueprint publish via lobby HTTP. Persistence: checkpoints (500 ops / 5 min), device-state snapshot (thermal, i²t, meters, credits, contract progress), park/resume (sim pauses when empty), reconnect grace window, offline Sandbox mode (same sim-wasm, full doc).
**Demo — the MVP story: two friends join by room code. One completes the lamp and ripple contracts, buys copper, ties into the grid. The other splice-taps their corridor run and leeches; victim sees the sag, probes the service entrance, sees outbound current, fires a TDR pulse, reads the reflection distance, rides out and cuts the splice. Both see histograms. Room parks and resumes intact.**

### M8 — Hardening + ship MVP
Headless bot load test (4 rooms × 4 bots, edit+probe churn); chaos proxy soak; adversarial netlist fuzz campaign (V-loops, L-loops, 0 Ω, 1e12 ratios — quarantine only, never panic, never stall the tick); p99 patch-apply + per-island budget telemetry; rate limits; WebGL context-loss restore; colorblind ramps, reduced-motion, keyboard/touch pass, ARIA on chrome; onboarding: first-60-seconds battery→lamp in world view, starter blueprints with annotations, contracts-as-tutorial sequencing; musl single-binary deploy on one VM/Fly, TLS at edge, COOP/COEP headers.
**Demo: public playtest build; the three slice-validation questions answered with real users: (1) does world↔schematic zoom feel like one object? (2) does contract+scope hold a non-EE 30 min? (3) does one tap/TDR duel generate a story?**

### Post-MVP order (from design doc, unchanged)
Decoders + logic blocks (stamped gates) → block band + full hierarchy/instancing → Commons Co-op events → MCU (sim-digital RV32) + rolling codes → optical/RF links → AC era (transformers, resonance) → Blackout mode → 16-player scaling → WebTransport swap.

## 5. Testing & verification per layer

**sim-math:** unit tests vs faer oracle (dense + sparse, random circuit-shaped matrices, pivot-growth cases); proptest on solve residuals; CI grep + disasm check for `fma`/`mul_add`.
**sim-core:** golden circuits with analytic closed forms (tolerance = TR LTE bound at dt); ngspice reference envelopes (published physics only, never GPL code); per-step debug-build invariants: |Ax−b| < tol, passive energy non-increase, no NaN/Inf outside quarantine, snapshot→restore→advance ≡ advance; continuous netlist fuzzing (panic = failure, quarantine = pass); criterion perf gates (linear n=200 step < 2 µs, nonlinear n=500 < 50 µs native), regression-tracked.
**Determinism:** every golden runs x86-64 + aarch64 + wasmtime; xxhash3 state hashes must match at checkpoints; op-log replay from checkpoint reproduces hashes. Blocking from M5.
**Protocol/doc:** randomized multi-client interleaver vs sequential reference (convergence); LWW/delete-wins property tests; codec round-trip fuzz; snapshot-delta encode/decode against ring-aging scenarios.
**Server:** chaos proxy (latency/loss/reorder/burst) on all three classes — asserts: class-1 total order, class-2 staleness bound, class-3 gap-flag correctness; park/resume round-trip equivalence; SQLite writer stall injection.
**Frontend:** perf HUD budget assertions in CI (playwright trace: 5 ms render / 4 ms sim / 0 per-frame allocs sampled); hit-testing unit tests (tolerance, priority order); DocStore rebase unit tests; visual snapshot tests for band crossfade and scope rendering; lint rule forbidding sim-plane data in React components.
**End-to-end:** playwright two-browser scenarios (co-edit, lamp-lights, probe overlay); headless bot soak; the M8 fuzz-under-load campaign.

## 6. Explicit MVP definition

Ship = M8 exit. Contents, exhaustively:
- **One room template** (Free-Market-lite, PvP toggle doubling as co-op), 2–4 players, one handcrafted map (2 source taps, 3 NPC loads, plots, 1 corridor: 3 thermal grid segments + 1 Party Line trunk, per-plot service entrance with main fuse + meter). Room-code join, auth-lite sessions, park/resume persistence, offline sandbox.
- **Sim:** MNA + TR/BE + NR + fallback ladder + quarantine, dense+sparse LU, islands with Bergeron corridors, per-island budgets/dilation, dt=10 µs @ 60 Hz tick. Devices: wire runs (2 gauges, R(T)), R, C, L, pot, switch, relay, fuse/breaker, battery/finite generator, DC/AC source, diode, Zener, LED, lamp, DC motor, speaker, op-amp, NPN. Trip-and-repair damage, meters.
- **Views:** schematic band (dots, voltage colors, knob-drag, tooltips, mini-waveforms) + world band (lamp/motor/speaker faceplates, glowing runs) with continuous crossfade; edit-in-context; basic Create Block subcircuits; blueprint copy/paste text. Block band explicitly deferred.
- **Instrumentation:** 4 probe channels (V/I one-click), docked WebGL scope with auto-trigger/auto-scale, math A−B, Vrms/Vpp/ripple/freq chips, one FFT view, min-max streamed, cross-player tick-aligned overlay.
- **Multiplayer:** WS three-tier sync (op-log / 20 Hz delta snapshots with viewport interest / lossy probe streams), WASM preview with snapshot reseed, plots/permissions, presence cursors.
- **Game:** 5 DC contracts with server-side verification + efficiency histograms, joule-credit shop, one attack verb (splice-tap → brownout/leech), one detection verb (TDR locate), cut + repair.
- **Not in MVP:** block band, logic gates/decoders beyond UART plumbing, MCU, AC-era transformers, optical/RF, Blackout mode, >4 players, accounts, WebTransport, maze auto-router, mobile polish.

Ship gate = the three slice-validation questions all "yes" with external playtesters; otherwise iterate inside M6–M8 scope — add nothing new until they are.