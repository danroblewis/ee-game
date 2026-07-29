# EE Game ("Common Ground") — Full Implementation Plan

## Context

Greenfield project in `/Users/danroblewis/ee-game` (empty repo). Goal: a web-based multiplayer game where players play by **drawing electrical schematics** while a **real electrical simulation** runs on an authoritative backend. Players control real outputs (lights, motors), can hide the schematic to see the "real world," and interact/fight/cooperate **only through electricity** — shared power and data transmission lines. Real-time oscilloscopes (V and I anywhere), FFTs, and analyses are core gameplay with a maximally intuitive UX. UI feel: Falstad. Non-electrical physics out of scope.

A 13-agent research/design/architecture workflow (`wf_f78f8f17-4b8`) produced the research, three competing game designs, a judged synthesis, three architecture docs, and this plan. Full source docs (to be committed into `docs/` at M0):

- `/private/tmp/claude-501/-Users-danroblewis-ee-game/374390d1-1ce8-4ace-b9a9-b063d06c7840/scratchpad/{research_briefs,design_rationale,game_design,arch_sim-engine,arch_backend-netcode,arch_frontend,implementation_plan}.md`

## Locked decisions (confirmed with user)

- **Scale:** room/session-based, 2–16 players, one authoritative sim per room.
- **Sim engine:** from scratch, Falstad-style — MNA, companion models, trapezoidal+BE integration, Newton-Raphson. No SPICE; no GPL CircuitJS1 code (algorithms only).
- **Stack:** Rust sim core (native server + WASM client preview); TypeScript frontend/net.
- **World view (decided by design synthesis):** **the world IS the canvas** — one infinite plane, three crossfaded semantic-zoom bands (world / block / schematic) rendered by one renderer. Diving from glowing village to transistor is a single wheel-zoom gesture.

## Game design summary (from unified design doc)

**Fantasy:** engineer-settler on a shared frontier grid. Pillars: everything is electricity (every number comes from the solver); one sim, two worlds; the scope is the game; **degrade, never destroy**; trivial first circuit, bottomless ceiling.

- **Zoom bands:** world (faceplates: lamp glow ∝ P, motors spin at back-EMF speed, smoke on overstress; corridor runs glow ∝ power) → block (ICs with live aggregate meters, power-flow arrows) → schematic (full Falstad: voltage-colored wires, animated current dots, hover tooltips, in-canvas mini-waveforms). Edit-in-context subcircuits (double-click to enter, breadcrumbs, Figma-component instancing) are the abstraction ladder AND progression system.
- **Geometry is electrical at world scale only:** wire runs stamped as per-meter R (+ lumped L/C on long runs, R(T) thermal state); corridors are lumped transmission-line sections → honest droop/reflection and natural per-island solver boundaries. Inside boards, layout is lumped/ideal.
- **Territory:** owned plots (edit permission boundary) + neutral-ground corridors carrying the shared Grid bus and "Party Line" data trunks. Anyone may run wires across neutral ground; anyone may splice-tap or cut wires crossing it — this one rule generates the metagame.
- **Service entrance:** each plot's only tie to shared infra has an unbypassable auto-resetting main fuse + real energy meter (∫V·I). Structural anti-grief: damage capped at trip-and-repair; attacker pays continuous real energy, defenses are cheap series parts; no offline automation (sim pauses in empty rooms).
- **Contracts** (goal engine): NPC loads post executable specs verified server-side by the same measurement chips players use ("48 V ±5%, ripple <100 mVpp", motor-RPM band, FFT-verified carrier, UART byte sequence, survive injected line faults). Payouts scale with measured efficiency; Zachtronics-style histograms per completion.
- **Conflict is emergent circuits, never attack buttons:** brownout/overdraw, back-feed, thermal line burn, leeching, ripple injection; splice-tap → decoders → replay/forge frames; jamming costs real watts. Defense is real engineering (fuses, crowbars, filters, isolation, islanding relays, rolling codes). Detection is instrumentation: every attack has a physics signature (clamp probes, FFT fingerprints, TDR locates taps to the meter).
- **Instrumentation:** click wire = V-probe, click component = current clamp; auto-trigger + autocorrelation timebase + 1-2-5 auto-scale (never fight the scope); math channels, measurement chips, FFT, XY mode, logic view, stackable protocol decoders; probes are room-scoped on the authoritative tick, so cross-player overlays are trivial.
- **Progression:** never gate primitives — joule-credits buy capacity/ratings/quantity/copper/repairs, never access. Modes: Sandbox, Commons Co-op, Free Market, Blackout (post-MVP).
- **Onboarding:** first 60 s = drag wire from battery to lamp in world view, it lights, contract pays. Floating-tolerant solver (gmin), prefabs-first, contracts-as-tutorial.

## Monorepo layout

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
    sim-digital/    # post-MVP: event layer, RV32 MCU, pin bridges.
    protocol/       # single source of truth for wire + doc types: CircuitOp, Mutation,
                    #   InteractOp, delta codec, probe-chunk codec, StateSnapshot,
                    #   part catalog. serde + postcard. Compiled into sim-wasm.
    server/         # axum: lobby, registry, room/{task,doc,interest,probes,sim_thread},
                    #   persist (SQLite/rusqlite), transport (3-class WS queues).
    sim-wasm/       # wasm-bindgen facade: init/applyOps/interact/advance/reseed/
                    #   drainSamples/compile + codecs. Zero-copy Float32Array export.
    sim-golden/     # golden circuits, ngspice envelopes, cross-target determinism
                    #   harness (wasmtime), criterion benches, netlist fuzzer.
  packages/                  # pnpm workspaces, Vite, strict TS
    protocol/       # TS type mirrors + part catalog JSON; hot-path codecs via sim-wasm.
    canvas/         # WebGL2 engine: f64 camera + floating origin, chunked spatial hash,
                    #   band passes + crossfade, instanced SDF wires, dot pass,
                    #   voltage texture, MSDF text, perf HUD.
    editor/         # input router, tool state machines, hit-testing, DocStore +
                    #   optimistic rebase, undo (inverse ops), picker, copy/paste blobs.
    scope/          # scope WebGL renderer, panel manager, ScopeKernel client.
    net/            # net-worker, transport abstraction (WS now, WebTransport later),
                    #   FrameBus (SAB + postMessage fallback), snapshot decoder,
                    #   probe SAB rings, reconciliation.
    app/            # React 19 + Zustand chrome, worker bootstrap, routes.
  tools/            # chaos proxy, headless bot client, 4-client op interleaver.
  docs/             # the workflow's research/design/architecture docs, committed at M0
```

## Binding architecture resolutions

1. **Timestep:** room-nominal fixed dt = 10 µs (5–20 µs configurable); overload response is per-island `local_dt` k-step multiples + sim-time dilation (never UI slowdown). NR trouble handled by the engine fallback ladder (BE step, dt-halving to 50 ns floor, regrow ×1.05).
2. **Server tick: 60 Hz**; ops/`NetlistPatch` apply at tick boundaries; snapshots every 3rd tick (20 Hz).
3. **Island coupling: Bergeron lossy transmission lines** (exact one-dt decoupling = physically honest); small adjacent islands may merge into one matrix if short corridors feel laggy.
4. **Determinism AND reseed:** bit-identical native/WASM is a CI-enforced property of sim-core (no FMA, no fast-math, pure-Rust `libm`, no relaxed-SIMD) — but netcode never depends on it; client preview hard-reseeds from authoritative snapshots.
5. **Reconciliation: hard reseed, never physics rollback.** Per Tier-B snapshot: reseed continuous state, snap discrete state, re-apply pending unacked doc ops (document rebase), 2–3-frame display blend.
6. **Solver: hand-rolled dense + fixed-pattern sparse LU** (faer's SIMD/FMA breaks determinism); faer is the differential-test oracle; dense path (n < ~150) is the always-correct fallback.
7. **Schematic sync: Figma-style server-ordered ops** with per-property LWW, optimistic client apply + rebase. NOT CRDTs (server must validate every op electrically anyway).
8. **Three-tier state:** (a) schematic doc — reliable op-log; (b) sim display state — 20 Hz delta snapshots + viewport interest management (R-tree); (c) waveform probes — lossy, server-side min-max decimation keyed to timebase×pixelWidth, gap flags.

## M0 de-risking spikes (do first; 3–5 days each, go/no-go notes)

- **S1** Cross-target bit-determinism: dense LU + diode NR on 3 circuits; x86-64/aarch64/wasmtime state hashes must match at 10k steps.
- **S2** Renderer throughput: WebGL2 instanced SDF wires + dot pass + R32F voltage texture; 10k segments + 5k dots @ 60 fps on integrated GPU.
- **S3** Fixed-pattern sparse LU vs faer oracle; n=500 refactor+solve < 50 µs native.
- **S4** Three-class WS transport + chaos proxy (latency/loss/reorder property tests) — build the harness before the features.
- **S5** COOP/COEP + SharedArrayBuffer pipeline (Vite dev + prod) + postMessage fallback.
- **S6** Zoom crossfade feel prototype (schematic↔world, hysteresis, one lamp circuit) — validate the signature moment by hand.
- **S7** Bergeron corridor latency feel; tune island-merge threshold.

## Milestones (each ends with a demoable vertical slice)

- **M0 — Spikes + skeleton.** The 7 spikes; repo scaffolding; CI (cargo test/clippy/fmt, wasm-pack, vitest, wasmtime harness); pnpm+Cargo workspaces; perf HUD stub. *Demo: spike report; 10k-wire canvas @ 60 fps; hash-matching sim step across three targets.*
- **M1 — Falstad parity, single-player, local (sim in browser).** Rust: dense LU; wire-closure; devices R, V/I sources, switch, C, L (TR + BE-after-switch), diode + pnjlim; NR + fallback ladder + quarantine; fixed-dt advance; sim-wasm facade. Golden tests (divider exact, RC/RL vs closed form, RLC ring, rectifier ripple); netlist fuzzer starts and never stops. TS: canvas, sim-worker, SAB SimFrame, voltage colors, current dots, tooltips, click interaction. *Demo: battery→switch→lamp; flip switch, dots flow, colors change. Feels alive, no run button.*
- **M2 — Editor.** Tool state machine (Falstad drag semantics), snapped wiring + junction dots, net-highlight + open-pin markers (connectivity from Rust `compile()` — one implementation), type-to-search radial picker, vertical value-drag with unit-aware log sweep, marquee select, undo/redo as inverse ops, copy/paste text blueprints, diegetic DRC hints. *Demo: build a bridge rectifier mouse-only in <2 min, live the whole time.*
- **M3 — Instrumentation v1 (local).** sim-probe (rings, min-max pyramid, measurement chips, rustfft). Click-to-probe V/I, GPU trace renderer, auto-trigger + autocorrelation timebase + 1-2-5 autoscale, docked auto-tiling panels, math A−B, chips @ 10 Hz, FFT view, in-canvas mini-waveforms. *Demo: probe rectifier, scope auto-locks ripple, knob-drag the cap, watch ripple shrink live in scope + FFT.*
- **M4 — Multiplayer document (netcode risk core).** Axum lobby (room codes, auth-lite), Document (property-LWW, ObjId=(clientNo,counter)), op pipeline (permission→validation→seq→apply→broadcast), SQLite op-log; client DocStore optimistic rebase, per-player undo, late-join. Then sim-thread integration: `NetlistPatch` at tick boundaries, dirty tiers, full-frame snapshots. Verify: randomized 4-client interleaver vs reference; delete-vs-edit fuzz. *Demo: two browsers co-edit; A flips switch, B's lamp lights within frame budget.*
- **M5 — Prediction, interest, probe streaming.** Delta snapshots (32-deep ack ring, 16-bit quantization), R-tree viewport interest + visibility masks (anti-wallhack), WASM preview with Thevenin boundary drives + hard reseed + display blend, ParamPreview knob streaming, probe subscriptions with server decimation + tick-aligned cross-player overlay; determinism CI gate goes blocking. *Demo: 4 players, ~1k components, zero-latency local edits, aligned cross-player probe overlay, chaos proxy on: gaps not lies.*
- **M6 — Two worlds: world band, real devices, damage.** Devices: pot, relay, fuse/breaker (i²t), Zener, LED, lamp, DC motor (back-EMF + inertia), speaker, op-amp, NPN, batteries/generators, AC source; world wire runs (gauge R/m, R(T)), Bergeron corridors; overstress→drift→trip state machine; per-island rayon + budgets + dilation. World-band faceplates (glow, rotor, WebAudio speaker, smoke), corridor glow ∝ power, band crossfade. Handcrafted map (2 source taps, 3 NPC loads, plots, 1 corridor, service entrances). *Demo: wheel-zoom from glowing village into a lamp's filament schematic in one gesture; overload a feeder, district browns out, wire heats and trips, repair it.*
- **M7 — Game shell: contracts, economy, conflict, persistence.** 5 DC contracts verified by server-side measurement chips (payouts only from server measurement — anti-cheat), efficiency histograms; joule-credit shop (price gates ratings/quantity, never access); splice-tap, cutter with inrush signature, TDR locate flow; plot permissions; Create Block subcircuits with edit-in-context; blueprint publish; checkpoints, park/resume, offline sandbox. *Demo (the MVP story): friend A completes contracts and ties into the grid; friend B splice-taps and leeches; A sees the sag, probes outbound current, TDR-locates the splice, rides out and cuts it. Room parks and resumes intact.*
- **M8 — Hardening + ship MVP.** Bot load tests, chaos soak, adversarial netlist fuzz campaign (quarantine, never panic/stall), rate limits, WebGL context-loss restore, accessibility (colorblind ramps, reduced motion), onboarding (first-60-s lamp, starter blueprints, contracts-as-tutorial), musl single-binary deploy + TLS + COOP/COEP. *Demo: public playtest answering the three validation questions.*

**Post-MVP order:** decoders + logic blocks → block band + full hierarchy → Commons Co-op events → MCU (RV32) + rolling codes → optical/RF links → AC era (transformers, resonance) → Blackout mode → 16-player scaling → WebTransport.

## Testing & verification

- **sim-math:** unit tests vs faer oracle; proptest solve residuals; CI grep + disasm check forbidding `fma`/`mul_add`.
- **sim-core:** golden circuits vs analytic closed forms (tolerance = TR LTE bound); ngspice reference envelopes (published physics, never GPL code); debug-build invariants (|Ax−b| < tol, passive energy non-increase, no NaN outside quarantine, snapshot→restore→advance ≡ advance); continuous netlist fuzzing (panic = failure, quarantine = pass); criterion perf gates (linear n=200 step < 2 µs; nonlinear n=500 < 50 µs).
- **Determinism:** every golden on x86-64 + aarch64 + wasmtime, xxhash3 state hashes match; op-log replay reproduces hashes. Blocking from M5.
- **Protocol/doc:** multi-client interleaver vs sequential reference; LWW property tests; codec round-trip fuzz.
- **Server:** chaos proxy asserting class-1 total order / class-2 staleness bound / class-3 gap correctness; park/resume equivalence.
- **Frontend:** playwright perf budgets (5 ms render / 4 ms sim / 0 per-frame allocs), hit-testing + rebase unit tests, visual snapshots for crossfade and scope.
- **End-to-end:** playwright two-browser scenarios (co-edit, lamp-lights, probe overlay); M8 fuzz-under-load.

## MVP definition (ship = M8 exit)

One Free-Market-lite room template (PvP toggle doubling as co-op), 2–4 players, one handcrafted map. Sim: MNA + TR/BE + NR ladder + quarantine, dense+sparse LU, Bergeron islands, dt=10 µs @ 60 Hz tick, ~20 device types. Views: schematic + world bands with continuous crossfade, edit-in-context, basic subcircuits, text blueprints (block band deferred). Instrumentation: 4 probe channels, auto-everything scope, math A−B, measurement chips, one FFT, cross-player overlay. Multiplayer: WS three-tier sync, WASM preview + reseed, plots/permissions. Game: 5 DC contracts + histograms, joule-credit shop, splice-tap + TDR + cut/repair. **Ship gate:** external playtesters answer yes to all three: (1) world↔schematic zoom feels like one object; (2) contract+scope loop holds a non-EE for 30 min; (3) one tap/TDR duel generates a story.
