# EE Game Frontend Architecture (TypeScript)

## 0. Principles

1. **The canvas is the product; the framework is trim.** All game rendering is a custom WebGL2 engine. React renders only chrome (panels, dialogs, HUD). Nothing that changes per frame ever enters framework state.
2. **Three state planes, three update rates.** Document plane (netlist/entities, changes on user ops), sim plane (node voltages/currents, changes every sim tick), UI plane (tools/selection/panels). Each has its own store, transport, and subscription model. Mixing them is the canonical failure mode of this genre of app.
3. **One clock.** Every sample, snapshot, and predicted frame is stamped with the authoritative sim tick. All rendering, scopes, and reconciliation key off tick index, never wall clock.
4. **Zero per-frame allocation** in the render/ingest paths; typed arrays and pooled objects throughout.

## 1. Process & thread topology

```
Main thread        : input router, editor, WebGL2 schematic+world renderer,
                     scope renderer, React chrome, layout
net-worker         : WebSocket; decode op-log / 20Hz snapshots / probe frames;
                     write probe SAB rings; forward ops+snapshots (postMessage)
sim-worker         : Rust WASM sim; local prediction of player's island(s);
                     fixed-dt stepping vs real-time budget; writes SimFrame SAB
analysis-worker    : Rust WASM (rustfft + measurements + protocol decoders);
                     reads probe SABs; trigger detection, min/max pyramids,
                     FFT, measurement chips, decoder annotations
```

SharedArrayBuffer rings connect workers (requires COOP/COEP headers; the app must be cross-origin isolated — plan hosting for it from day one). Fallback path: transferable `postMessage` of frame batches (works, adds one frame of latency; keep the abstraction `FrameBus` so both are behind one interface).

Rendering stays on the **main thread for MVP**: hit-testing, input, and DOM overlay all want the same coordinate state, and OffscreenCanvas-in-worker complicates text overlay and picking for little gain at MVP scale. The renderer is written against an explicit `RenderHost` interface so it can move to a render worker with OffscreenCanvas later without API change.

## 2. Rendering stack: custom WebGL2

**Decision: raw WebGL2, no pixi.js/three.js/Canvas2D.** Rationale:

- Every visual is a function of per-frame sim output: wire color = node voltage, dot speed = branch current, lamp glow = dissipated power, crossfade = camera z. Scene-graph libraries assume mostly-static display objects with occasional property changes; we would bypass their entire retained model and use them as a glorified context wrapper.
- Required shader work — instanced SDF thick lines, current-dot animation, node-voltage texture lookup, phosphor persistence, band crossfade — is custom regardless of library.
- Canvas2D cannot hold 60 fps with thousands of animated wires + dots (Falstad's own dot pass is its bottleneck).
- WebGPU is a later optimization behind the same `Renderer` interface; WebGL2 is ~universal today.

**Draw architecture (per frame):**

1. `SimFrameReader` grabs the latest generation-stamped frame from SAB (predicted frame for the player's island, server frames elsewhere; see §6).
2. Node voltages uploaded as a **float texture** (`R32F`, node-id → texel). Wire and pin shaders sample it and map through a color ramp (green/red/gray, brightness ∝ |V|; alternate ramps for power view and colorblind modes).
3. **Wires**: instanced quad-per-segment with fragment-shader capsule SDF (thick antialiased lines; native lineWidth is capped at 1px). Instance attributes: endpoints, node indices, wire flags. One draw call per LOD bucket.
4. **Current dots**: CPU advances a per-wire phase accumulator (`phase += k·I·dt`, from branch currents in the sim frame) into a Float32Array instance buffer; dots are instanced points positioned along segments by phase. Below ~2 px/grid-unit LOD, the dot pass is skipped and the wire shader animates a flow-glow/dash instead (block/world band power arrows use the same mechanism on corridor runs).
5. **Components**: instanced sprites from a texture atlas (schematic glyphs) with per-instance state uniforms packed in an instance buffer (selection, trip/failed state, rotation). **World-band faceplates**: same instancing, world-band atlas, plus state-driven shader params (lamp glow ∝ P, motor rotor angle integrated from simulated speed, smoke particle emitter keyed to thermal state).
6. **Overlay pass**: selection outlines, net highlight, marquee, probe flags, in-canvas mini-waveforms (small GPU ring-buffer strips, same trace shader as the scope).
7. **Text**: MSDF glyph atlas for in-canvas labels/values (crisp at all zooms); DOM absolutely-positioned overlay for editable fields, tooltips, context menus (positioned via camera transform, updated on camera change only).

**Precision**: camera stored in float64; geometry uploaded relative to a floating anchor origin re-based when the camera drifts > ~1e5 units, so an infinite canvas never hits float32 jitter.

**Budget**: 5 ms render, 4 ms local sim step, 2 ms ingest/analysis on main-thread-visible work, per-frame allocations = 0. Enforced by a perf HUD from phase 1.

## 3. Infinite canvas & semantic zoom

`Camera { x, y, z }` with wheel zoom-to-cursor (point under cursor invariant), space/middle-drag pan, pinch on touch, Cmd+0/1/2 fit/100%/selection, soft rubber-band zoom limits.

**Bands are not modes.** A continuous `bandWeights(z) → {world, block, schematic}` function with hysteresis produces 0–1 weights; during a transition window both band pass-sets draw with crossfade alpha. Each band is a set of draw passes over the *same* entity data:

- **Schematic** (near): full wires, dots, junction dots, values, mini-waveforms.
- **Block** (mid, post-MVP): subcircuit instances and devices as IC boxes with live port badges; inter-block wires as flow arrows; per-plot aggregate meters.
- **World** (far): faceplate sprites at placed positions; schematic wires alpha→0; corridor runs render as glowing power lines (glow ∝ metered power from sim frame). Rival plots render world band only per room template (visibility mask supplied by server; the renderer simply has no schematic data for masked plots — enforcement is server-side interest management, not client courtesy).

Positions are shared across bands (landmark stability). LOD rules: text hidden < 4 px height, values fade below threshold, dot pass → glow below threshold, wires batch into merged buffers per chunk.

**Spatial index**: uniform-grid spatial hash over world coordinates, chunked (e.g., 256-unit cells). Serves frustum culling (which instance buffers to draw), hit-testing, and marquee. Chunks own their GPU instance buffers; edits dirty only their chunk (incremental re-upload via `bufferSubData`).

## 4. Document plane: state, sync, undo

### 4.1 Data model (`@ee/protocol`, shared TS types + binary codecs)

```ts
type Id = string;            // ULID, client-minted, prefixed by player for debuggability
interface CircuitDoc {
  comps:  Map<Id, Comp>;     // { type, pos, rot, mirror, params, plotId,
                             //   defId?, paramOverrides? }
  wires:  Map<Id, Wire>;     // { points: Vec2[], gauge?, plotId|corridor }
  defs:   Map<Id, SubcktDef>;// { name, ports: Port[], body: CircuitDoc }
  probes: Map<Id, Probe>;    // { kind: 'V'|'I'|'diff'|'math', target, color, owner }
  plots:  Map<Id, Plot>;     // bounds, owner, permissions
}
```

Ops are small, invertible, and player-attributed: `AddComp, RemoveComp, MoveComp, SetParam, AddWire, EditWirePoints, CreateDef, EditDef, InstanceDef, AddProbe, ...` each carrying `{opId, playerId, targets, before/after}`.

### 4.2 Replication

Server-authoritative **op-log** (reliable channel of the three-tier sync). `DocStore` holds `confirmedDoc` + an ordered list of **pending local ops** applied optimistically on top. On server ack, pending op is promoted; on remote op arrival, remote applies to confirmed and pending ops **rebase** (re-apply; ops are designed to be commutative-or-skip: an op targeting a deleted entity no-ops with a toast). Edit-permission checks run client-side for UX and server-side for truth.

`DocStore` is a plain TS class emitting **fine-grained change events** (`onEntityChanged(id)`, `onTopologyChanged(chunkIds)`) consumed by: the renderer (dirty chunks), the netlist compiler, and React (coarse selectors only — e.g., inspector panel for the selected entity).

### 4.3 Netlist identity

Node/branch ids must match the server exactly (probe streams and sim frames are keyed by them). **The netlist compiler lives in the Rust core** and is called through WASM: `compile(doc) → { nodes, branches, nodeOfPin, branchOfComp }`. One deterministic implementation, used natively on the server and via WASM on the client; the frontend never re-implements electrical connectivity. Connectivity queries for UX (net highlight on hover) use the compiler's output map, incrementally recomputed per dirty island (debounced ~1 frame).

### 4.4 Undo/redo

Per-player undo stack of **inverse ops**, executed as *new* ops through the same log (multiplayer-safe; never rewinds shared history). Value-drag sweeps coalesce: during drag, ephemeral `ParamPreview` messages go over the lossy channel at ~20 Hz (server applies to sim but not to the log); on release one `SetParam` op commits. This gives EveryCircuit-feel live sweeping without op-log spam.

## 5. Sim plane: frames, prediction, probe streams

### 5.1 SimFrame SAB layout

Double-buffered, atomically generation-stamped:

```
Header: { generation:u32, tick:u64, dt:f32, nodeCount, branchCount }
Body:   Float32Array nodeVoltages[nodeCount]
        Float32Array branchCurrents[branchCount]
        Float32Array deviceAux[]   // thermal state, motor speed, fuse i²t...
```

Writers: sim-worker (predicted, player's island) and net-worker (server 20 Hz deltas, interpolated/extrapolated to display tick for remote islands). A `FrameComposer` on the main thread selects per-island source: **local prediction inside the player's solver island(s); server truth everywhere else; boundary nodes always server** — matching the corridor-boundary island partitioning, so prediction seams sit exactly where the solver already cuts.

### 5.2 Reconciliation

Sim-worker steps the WASM sim at fixed dt against a real-time budget. On each server snapshot `{tick, islandState}`, if divergence exceeds epsilon, the worker loads the snapshot and **re-simulates pending local inputs** from that tick (inputs = param previews, switch toggles — cheap to replay for one island). UI never blocks; a subtle "resync" shimmer is the only artifact.

### 5.3 Probe subscription pipeline

Client sends `{probeId, nodeOrBranch, requestedRate, mode: full|minmax}`; net-worker writes arriving frames (tick-stamped, f32 or i16-fixed, or min/max pairs when zoomed out) into a per-probe **SAB ring** (~10 s full-rate) and increments an atomic head. Analysis-worker and scope renderer read via atomics. For probes inside the predicted island, the sim-worker writes the same ring format locally for zero-latency traces, reconciled by tick when server frames arrive.

## 6. Editor architecture

**Input router → tool state machine.** Pointer/keyboard events normalize (mouse/touch/pen via Pointer Events) into world coordinates, then dispatch to the active tool: `Select` (default; Falstad semantics: drag-empty = draw wire, drag-component = move), `Place`, `Probe`, `Cut/Splice`, transient `Pan/Zoom`. Tools are classes implementing `{onDown, onMove, onUp, onKey, renderOverlay}` — overlay drawing goes through the renderer's overlay pass, not DOM.

**Hit-testing**: CPU spatial-hash query with screen-space tolerance (≥ 8 px, ≥ 44 px touch targets by inflating tolerance at low zoom); priority: value labels > pins/endpoints > component bodies > wire segments > empty. No GPU picking (determinism, worker-independence).

**Wiring**: grid-snapped orthogonal segments with click-waypoints; auto-L-route for the simple case; **no maze auto-router at MVP** (manual waypoints, Wokwi-style, are predictable and cheap). Auto-inserted junction dots on T-connections; 4-way crossings never connect by default. **Net highlight on hover** (whole electrical node glows, from compiler output) plus open-pin markers — the two features that kill "looks connected but isn't."

**Values**: vertical drag on any rendered value sweeps log-scale within the part's sane range (unit-aware); right-click → dialog with expressions ("4.7k", "2*22u"). Any param bindable to a HUD slider.

**Copy/paste**: selection serializes to a human-readable text blob (Wokwi-style JSON) that is also the blueprint interchange format; paste re-mints ids and re-anchors at cursor.

**Damage/DRC surfacing is diegetic**: dots stop, wires gray, breaker glyphs flash; a non-modal DRC hint panel lists plain-language issues ("this wire connects + to − with nothing in between") each with a zoom-to-it button. The sim never stops.

## 7. Component picker & library

Double-tap/"+"/keypress opens a **type-to-search radial picker** at cursor: fuzzy search over name/aliases/tags ("res", "npn", "opamp"), recents ring (8 slots), curated category petals for browsing. Fully keyboard-driven: type → arrows → Enter places in wire-drag mode (placement gesture defines orientation). Catalog is **data-driven** from `@ee/protocol`: per part — ports, params (unit, range, default, drag-curve), schematic glyph, faceplate ref, price, ratings. Shop/economy chrome reads the same catalog. Never gated: search always sees everything; price is the only gate.

## 8. Hierarchy / subcircuits UI

- **Create Block**: marquee → auto-detect ports from wires crossing the selection boundary → user names/arranges pins on a chip body → `CreateDef` + `InstanceDef` ops.
- **Edit-in-context**: double-click an instance pushes a breadcrumb frame; the definition body renders *in place* at the instance transform, surroundings dimmed but still live (renderer treats the open instance as a temporary chunk overlay; the sim already simulates internals — this is purely presentational). Esc/breadcrumb pops.
- **Figma semantics**: edits target the definition (ops on `defs`), updating all instances; per-instance `paramOverrides`. Closed instances stay alive: port pins voltage-colored, activity via aggregate badges (block band).
- Definitions are the blueprint asset: publish = export def + datasheet card.

## 9. Instrumentation subsystem

**Probes** are replicated document entities (room-scoped, tick-aligned by construction — cross-player overlay is free). Click wire = V probe; click component = current clamp (ring glyph); A-then-B = differential; one-click V×I power probe. Pinned flags share color with traces and chips.

**ScopeKernel (analysis-worker, Rust WASM)** per trace:
- min/max pyramid (÷4 levels, built incrementally) for O(log n) zoomed-out rendering;
- auto-trigger: hysteretic edge at (min+max)/2, autocorrelation period fallback; periodic → timebase auto-set to 2–4 cycles, phase-locked; else rolling. The only trigger UI is a "stabilized/rolling" chip with manual override;
- auto-scale: decaying min/max envelope snapped to 1-2-5 steps, symmetric when bipolar;
- measurements over integer periods: Vpp, min/max, mean, true RMS, freq, duty, rise/fall, overshoot, phase(A,B), THD;
- FFT: rustfft, Hann default (flat-top/rect options), 4096-pt, dB, Welch averaging, parabolic peak interpolation, 10–20 Hz update with overlap;
- stackable protocol decoders (sigrok model): analog→bits→UART frames, emitting time-spanned annotation rows.

**Scope renderer** (own WebGL context/canvas, shared shader lib with mini-waveforms): GPU-resident ring VBO per trace (upload tail only; vertex shader derives X from instance index + head offset); thick traces via instanced-quad SDF, 1-px via GL_LINES; unit-grouped axes (V left, A right); XY/I–V mode with phosphor persistence (decaying accumulation FBO); logic view with bus-to-hex; annotation rows under traces. Headroom target: 64 traces @ 60 fps.

**Panels**: bottom-docked auto-tiling strip; drag probe-flag onto a panel to overlay, drag trace out to split; panels can float and **pin to a canvas region** (moves with camera). Measurement chips are DOM under each panel, updated at 10 Hz via direct-DOM subscription. In-canvas mini-waveforms beside probed wires use the same ring data at reduced resolution.

TDR, spectrum analyzer, injector: in-fiction devices whose UI is just… a probe + these panels with a pulse source, keeping "instruments are circuits" honest.

## 10. Chrome framework & UI plane

**React 19 + Zustand, Vite, strict TS.** Rationale: chrome is thin (menus, inspector, contracts, shop, DRC list, room/lobby); React maximizes ecosystem/contributor familiarity; fine-grained-reactivity frameworks' advantages evaporate given the hard rule: **no per-frame data in React**. High-rate readouts (hover values, chips, meters) subscribe to `SimFrameReader`/ScopeKernel and mutate `textContent` via refs at ≤ 20 Hz. Zustand stores: `uiStore` (tool, selection, panels, picker), `sessionStore` (room, players, permissions, economy), `docStore` bridged via coarse selectors. All canvas surfaces are `<canvas>` leaves React never re-renders.

## 11. Accessibility & onboarding affordances

- Colorblind-safe alternate voltage ramps (dual-encoded: hue + brightness; optional pattern overlay); reduced-motion mode (dots → static dashes with brightness ∝ I).
- Full keyboard operation of editor (Falstad shortcut set: w/r/c/l/g, space=select, R=rotate) and picker; DOM chrome fully ARIA'd; canvas exposes selected-entity state via live region ("Resistor R3, 4.7 kΩ, 12 mA").
- Touch first-class: pointer-event unified input, pinch zoom, long-press context menu, inflated hit targets.
- "Why is this off?" inspector on any dead device (no current path / V below Vf / reversed / tripped) — rule-based over compiler + sim frame.
- Starter blueprints open in edit-in-context with annotation callouts; contracts panel doubles as tutorial sequencer.

## 12. Package layout (pnpm workspaces + Cargo)

```
crates/sim-core        Rust: MNA solver, compiler, ScopeKernel math, decoders
crates/sim-wasm        wasm-bindgen bindings (sim + compile + analysis)
packages/protocol      shared types, op codecs, probe frame codecs, part catalog
packages/canvas        WebGL2 engine: camera, chunks, passes, MSDF text, LOD
packages/editor        tools, hit-testing, DocStore, undo, picker, hierarchy
packages/scope         scope renderer, panel manager, kernel client
packages/net           net-worker, FrameBus (SAB + postMessage fallback), reconcile
packages/app           React chrome, routes, workers bootstrap, Vite
```

## 13. Build order (tracks the MVP slice)

1. **Canvas core**: camera/pan/zoom, spatial hash chunks, wires+components static render, MSDF text, perf HUD. *Exit: 10k wires @ 60 fps.*
2. **Falstad parity, local**: WASM sim in worker, SimFrame SAB, voltage texture, current dots, hover readouts, switch/pot interaction. *Exit: battery-switch-lamp feels alive.*
3. **Editor**: tools, wiring + net highlight, picker, value drag, undo (local), copy/paste blobs.
4. **Instrumentation v1**: probes, rings, ScopeKernel (trigger/scale/measurements), docked scope, mini-waveforms, one FFT view.
5. **Netcode**: op-log sync + rebase, prediction/reconcile per island, probe subscriptions, plots/permissions, presence cursors.
6. **World band**: faceplates (lamp/motor/speaker), corridor glow, crossfade zoom, service-entrance meter UI. *Exit: zoom feels like one object.*
7. **Hierarchy**: Create Block, edit-in-context, instancing, blueprint publish.
8. **Game shell**: contracts panel + histograms, shop, TDR flow, splice/cut verbs, DRC hints, onboarding polish.

## 14. Risks & mitigations

- **COOP/COEP isolation** constrains embedding/third-party assets → decide hosting early; FrameBus fallback keeps dev unblocked.
- **Renderer scope creep** (three bands × instancing × text) → band passes share one instancing framework; block band explicitly deferred.
- **Prediction seams** at island boundaries → boundaries are corridor nodes (server-owned by design); visual smoothing only, never client authority.
- **React perf foot-guns** → lint rule + code review gate on sim-plane data in components; perf HUD from day one.
- **WebGL context loss / low-end GPUs** → context-restore path, dot-count and trace-count LOD governors tied to frame-time telemetry.
