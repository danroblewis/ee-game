# EE Game — Backend & Networking Architecture

Rust authoritative server, TypeScript client networking layer, one simulation per room. This document specifies module structure, data structures, protocol, and build order for the MVP slice, with post-MVP paths noted but not built.

## 1. Crate / Module Breakdown

```
/crates
  sim-core/        # MNA solver, components, islands. No I/O, no async, no net deps.
                   # Compiled twice: native (server), wasm32 (client). cdylib+rlib.
  protocol/        # All wire types + doc/op types, serde-derived. Shared by server & sim-core;
                   # compiled to WASM alongside sim-core so client TS gets one ABI.
  server/          # axum app: lobby HTTP, WS upgrade, room registry, room tasks, persistence.
  sim-wasm/        # thin wasm-bindgen facade over sim-core + protocol (init, applyOp,
                   # step(dt), reseed(snapshot), readDisplayState, probe(nodeId))
/client
  src/net/         # transport abstraction, op queue + rebase, snapshot decoder, probe rings
  src/sim/         # WASM host: worker-thread sim loop, reseed logic, display-state bridge
```

**Server internal layout:**

```
server/src/
  lobby.rs         # HTTP: create/join/list rooms, auth-lite session issue, blueprint CRUD
  registry.rs      # RoomId -> RoomHandle map; lifecycle (spawn/park/evict)
  room/
    task.rs        # tokio task: membership, doc authority, op validation, fan-out
    doc.rs         # authoritative Document + op application + netlist projection
    interest.rs    # per-client viewport, R-tree culling, Tier-B delta encoder
    probes.rs      # probe subscription table, decimation buffers
    sim_thread.rs  # OS-thread sim loop; channels to/from task.rs
  persist.rs       # op-log append, checkpointing, device-state snapshots (SQLite)
  transport.rs     # WS framing, 3-class send queues, backpressure policy
```

**Rationale for the split:** `sim-core` must stay free of tokio/net so the identical code runs as WASM; `protocol` as a shared crate is the single source of truth for the wire format — the TS client consumes it through the WASM boundary (encode/decode in Rust, structured data out via serde-wasm-bindgen for low-rate messages; raw `ArrayBuffer` views for hot paths), eliminating a hand-maintained TS schema.

## 2. Room Service & Lifecycle

**Room = one tokio task + one dedicated OS thread.**

- **Room task** (async): owns `Document`, member table, per-client `ClientState` (viewport, acked snapshot seq, probe subs, send queues). Receives client messages, validates + sequences ops, broadcasts, encodes snapshots, runs decimation fan-out.
- **Sim thread** (sync, `std::thread` with a named thread per room): owns the `Simulation` (netlist, MNA matrices, LU factors, state vectors). The sim is CPU-bound and must never run on the async runtime.

**Channels** (all bounded):

```rust
struct RoomHandle {
  to_task:  mpsc::Sender<ClientEnvelope>,      // from WS reader tasks
  ctl_sim:  crossbeam::channel::Sender<SimCmd>, // NetlistPatch{ops}, SetViewportHints,
                                                //  SubscribeProbe, Pause, Checkpoint
  from_sim: crossbeam::channel::Receiver<SimOut>, // DisplayFrame, ProbeChunks, SimHealth
}
```

`SimOut` frames are latest-wins on the task side: if the task falls behind, older `DisplayFrame`s are discarded before encoding.

**Lifecycle:** `create` (lobby HTTP, returns 6-char room code + join token) → `active` → `parked` when last player leaves — sim thread exits, doc checkpoint + device-state snapshot written, `RoomHandle` dropped; registry keeps a stub row → `resumed` on rejoin: load checkpoint + op tail, rebuild netlist, restore device states, respawn thread → `evicted` from registry after TTL (persisted rooms survive on disk; match-mode rooms are deleted). Parking implements the locked "sim pauses in empty rooms" anti-Screeps rule and is also what makes hundreds of rooms per box cheap: only occupied rooms burn a thread.

**Late join / reconnect:** server sends `Welcome{selfId, members, docCheckpoint, opTail[], fullTierB, simTime}`; client instantiates WASM sim from it and is live. Reconnect within a grace window reuses the session and replays ops since the client's last acked seq.

## 3. Authoritative Sim Loop: Ticks vs. Substeps

Three distinct rates, deliberately decoupled:

| Clock | Rate | Purpose |
|---|---|---|
| **Sim substep `h`** | 1 kHz base (h=1 ms), adaptive halving to 62.5 µs on NR trouble/switching events | Trapezoidal integration accuracy; audio-band fidelity for speaker/AC-era content |
| **Server tick** | 60 Hz | Real-time pacing: apply queued netlist patches, sample probes, integrate meters, evaluate contracts/thermal/trip logic |
| **Snapshot rate** | 15–20 Hz (every 3rd–4th tick) | Tier-B display fan-out |

Sim thread loop per tick: (1) drain `ctl_sim` — apply `NetlistPatch` ops **at the tick boundary only**, restamping/refactorizing MNA as needed (topology-changing ops trigger island re-partition; value-only ops restamp in place); (2) run substeps to advance sim-time by the tick budget, per island; (3) sample probe taps at full substep resolution into per-probe SPSC rings; (4) every Nth tick, write a `DisplayFrame` (all display scalars, unculled — culling happens per-client in the task).

**Per-island budgets (the anti-DoS rule):** islands are discovered at corridor/transmission-line boundaries (the lumped line sections give a natural decoupling point — solve islands against each other's previous-substep boundary values, one-substep explicit coupling). Each island gets a wall-clock budget per tick proportional to its size. An island that blows its budget (adversarial stiff circuit, NR non-convergence storms) gets its **sim-time dilated** — it advances fewer substeps per tick and the client renders a "slow-motion" badge on that region. The tick never stalls; other islands and the UI are unaffected. `SimHealth` reports per-island load for telemetry and the badge.

## 4. Schematic Document & Collaborative Editing

**Chosen approach: Figma-style server-serialized ops with property-level last-writer-wins. Explicitly not CRDT, not OT.**

Justification: (a) every op must be validated against gameplay rules (plot ownership, neutral-ground splice rules, currency costs for placed parts) and transactionally applied to the live netlist — the server is unavoidably the arbiter, so CRDT merge-anywhere buys nothing and costs semantic control; (b) Figma and tldraw, the two closest production analogues (live multiplayer canvas), both independently rejected CRDT/OT for exactly this shape; (c) the op-log we need anyway *is* the persistence format and the undo substrate.

**Document model:**

```rust
struct Document {
  version: u64,                       // last applied seq
  objects: HashMap<ObjId, Object>,    // ObjId = (authorClientNo: u16, counter: u48) — no coordination
}
struct Object {                        // component | wire | junction | subcircuit-def |
  kind: ObjKind,                       //  subcircuit-instance | probe | plot | annotation
  parent: ObjId,                       // hierarchy: root plane -> plot -> block -> ...
  props: HashMap<PropKey, PropValue>,  // position, rotation, value, gauge, ratings,
}                                      //  endpoints: [PortRef], owner, ...
```

Each property is independently LWW: concurrent edits to *different* props of one object both survive; same-prop conflicts resolve by server arrival order. Wires store endpoint `PortRef`s (`ObjId` + port index) or plane coordinates; electrical node identity is *derived* server-side by connectivity analysis, never stored in the doc — this kills a whole class of merge hazards.

**Protocol:**

```
Client:  Op { clientOpId: u64, baseVersion: u64, mutations: [Mutation] }
Mutation: Create{id, kind, parent, props} | SetProps{id, patches} | Delete{id} | Reparent{id, parent}
Server:  OpApplied { seq: u64, authorId, clientOpId, mutations }   // broadcast, incl. author
         OpRejected { clientOpId, reason: DrcCode | PermissionDenied | InsufficientCredits }
```

Server pipeline per op: permission check (owner/ally of the target plot; neutral-ground rules for corridor objects) → semantic validation (ports exist, no self-loops-of-nothing; soft DRC issues are warnings, not rejections) → economy check → assign `seq`, apply to `Document` → enqueue derived `NetlistPatch` to the sim thread → append to op-log → broadcast. Multi-mutation ops are atomic (paste, block creation).

**Client optimistic loop:** apply op locally to doc + WASM sim immediately; hold in an unacked queue; on each `OpApplied` from *others*, rebase — undo unacked local mutations, apply remote, reapply local (tldraw's model; cheap because unacked queues are 0–3 ops at human edit speed). On `OpRejected`, roll back and surface the reason diegetically. Undo/redo is per-client inverse-op generation over its own history (Figma semantics: undo of "set R=5k" restores *your* prior value even if someone else edited between).

**Subcircuit instancing:** definitions are object subtrees under a `SubcircuitDef`; instances reference the def id. Edits inside a def are ordinary ops on def-children; the server's netlist projection expands instances at stamp time, so "edit once, all instances update" is free and no per-instance doc duplication exists.

## 5. Wire Protocol & Transport

**One WebSocket per client, binary frames, three app-level message classes:**

| Class | Contents | Queue policy |
|---|---|---|
| 1 — reliable-ordered | ops, op results, membership, contracts, economy, chat-adjacent control | unbounded-ish (256 cap; exceeding it disconnects the client — it's broken) |
| 2 — lossy latest-wins | Tier-B snapshot deltas, presence (cursors, viewports) | per-topic single slot; new frame replaces unsent old |
| 3 — lossy stream | probe waveform chunks | per-probe ring of 8 chunks; overflow drops oldest, sets a `gap` flag so the scope renders a break, not a lie |

All three multiplex over the socket via a 1-byte class tag; the server's per-connection writer task drains class 1 first, then 2, then round-robins 3. `bufferedAmount`-style pressure is handled server-side by the bounded queues (tungstenite send backpressure propagates to the writer task, which then exercises the drop policies). This abstraction maps 1:1 to WebTransport later (class 1 → bidi stream, class 2 → datagrams, class 3 → uni streams); the upgrade is a `transport.rs`/`net/transport.ts` swap, no protocol redesign. Do not build WebTransport or WebRTC for MVP.

**Serialization:** `postcard` (serde, `no_std`-friendly, compact varint encoding) for classes 1–2 message envelopes; hand-rolled fixed layouts for hot payloads:

- **Tier-B snapshot delta** (Quake-3 model): server keeps a ring of the last 32 encoded snapshots per client; each outgoing delta is encoded against the client's last-acked baseline (`ack` piggybacks on any client message), so loss never forces a resync — the baseline just ages; a baseline older than the ring forces one full keyframe. Per-entity payload: `entityIdx: u16` into a client-known interest table, changed-field bitmask, then values. Voltages/currents as 16-bit quantized against a per-snapshot per-field scale (display-grade precision; exact values come from hover-tooltips via class-1 request or from probes). Device display states (LED brightness, motor ω, switch pos, trip/thermal flags) as u8/u16. Typical steady-state delta: dozens of bytes.
- **Probe chunk:** `{probeId: u16, seq: u32, t0: f64, dt: f32, count: u16, scale: f32, offset: f32}` + `i16[count]`.

Numbers at MVP scale (4 players, ~2k components, ~500 visible each): class 2 ≈ 2–10 kB/s/client after deltas; class 3 ≈ 5–6 kB/s/probe-display. Trivial; headroom is 100×.

## 6. Interest Management (Viewport)

Client sends `ViewportUpdate{rect, zoomBand}` (class 2, throttled ~5 Hz). Server maintains an R-tree (`rstar` crate) over object AABBs, updated on doc ops. Per client per snapshot: query rect + 30% margin → interest set → entities entering the set get full state (and an interest-table index assignment); entities leaving are tombstoned after a linger. Out-of-view state collapses to **aggregates** computed once per snapshot for all clients: per-plot power in/out, per-grid-segment thermal load and flow direction, alarm flags — enough for world-band far zoom and the block-band strategic view without per-component data. Competitive-visibility rules (rival plots render silhouettes only) are enforced *here*, server-side: interest filtering is the anti-wallhack layer — clients never receive rival internal Tier-B state they shouldn't see; they do receive corridor/shared-bus state, which is exactly the "measure what reaches the shared lines" fiction. The viewport set also seeds the WASM preview scope: the client requests netlist detail (class 1) for what it views, which is the same set it can edit.

## 7. Waveform / Scope Pipeline

Server side, per probe subscription:

```
SubscribeProbe { probeId, target: NodeV(objId, port) | BranchI(objId), timebase_s: f32, pixelWidth: u16 }
```

Sim thread samples the target at full substep rate into an SPSC ring (sample = f32). The room task's decimator consumes the ring per subscription: bucket width = `timebase / pixelWidth`; emit **min-max pairs** per bucket (2 points/column, peak-preserving — correct for scopes; LTTB explicitly rejected as it destroys peaks). When zoom makes samples-per-bucket < 2, pass raw samples. Chunks of ~64 columns ship on class 3 at ~30 Hz. Multiple clients viewing the same probe at the same {timebase, pixelWidth} share one decimation buffer (subscription key includes the display params).

Client keeps a per-probe ring (~10 s at display resolution) in a SharedArrayBuffer; scope rendering (WebGL) and analysis — FFT (Hann/4096/Welch), math channels, measurement chips, decoders — all run **client-side** in the WASM/worker over the ring. Server cost per probe stays O(decimation). Measurement chips that are *contract verifiers* run server-side from the same sim-thread taps (shared code in `sim-core::measure`) — the anti-cheat property is that payouts derive only from server-side measurement. Probes are room-scoped doc objects on the shared authoritative timebase (`t0` is sim-time), so overlaying your trace against a teammate's is just subscribing to their probe id — permission-checked like any object read.

## 8. Client WASM Sim: Preview & Reconciliation

**No determinism requirement, no rollback.** The WASM sim is a *display extrapolator and optimism engine*:

1. Runs continuously in a dedicated Web Worker at the same nominal substep rate, over the client's interest-set netlist (out-of-view boundary nodes driven as Thevenin equivalents shipped in Tier-B aggregates).
2. Produces 60 fps display state (current-dot velocities, voltage colors, device states) between 15–20 Hz server snapshots — extrapolation, not scalar interpolation, so ripple and oscillation animate honestly.
3. Local edits and interactions (switch toggle, knob-drag value sweep) apply to the local doc + sim instantly; the same op goes to the server; reconciliation is the document rebase of §4 — a doc correction, never a physics rollback. Server rejections are rare (rule violations only).
4. Each Tier-B snapshot **hard-reseeds** local continuous state (cap voltages, inductor currents, source phases from a server-supplied phase word) and snaps all discrete state (logic levels, relay/trip states — never predicted across other players' actions). ~50–65 ms of analog drift between reseeds is visually invisible; oscillator phase snapping is masked by the reseed including source phase.
5. The same `sim-wasm` module, fed a full doc instead of an interest set, *is* the offline Sandbox mode and the tutorial — zero extra work.

Knob-drag streams value updates as coalesced class-2 presence-style messages during the drag and commits one class-1 `SetProps` op on release, so the op-log stays clean and other clients see the sweep live.

## 9. Persistence

- **Op-log:** append-only per room, `(seq, authorId, timestamp, postcard(mutations))`. This is the source of truth for the document.
- **Checkpoints:** full `Document` serialization every N=500 ops or 5 minutes; log truncatable before the last two checkpoints. Load = checkpoint + tail replay.
- **Device-state snapshot** on park/checkpoint: the gameplay-critical non-doc state — thermal integrators, trip states, fuse i²t budgets, meter totals (∫V·I), fuel remaining, contract progress, player credit balances. Sim *numeric* state (MNA vectors, companion history) is disposable: on resume, rebuild netlist from doc, restore device states, settle with a few DC-ish startup ticks.
- **Blueprints:** the copy/paste text-blob format (postcard→base64 of a mutation batch, with a human-readable JSON header) stored in a global table with author, datasheet card, and version — served by lobby HTTP, room-independent.
- **Store:** SQLite via `rusqlite`, one DB file per room plus one global DB (accounts, blueprints, room directory), all writes through a dedicated blocking-pool writer per room. Rationale: single-binary deploys, zero ops burden, per-room write isolation, and rooms never share hot state. Postgres is the flagged migration path when multi-node arrives; `persist.rs` hides the choice behind a trait.

## 10. Auth-Lite & Room Access

MVP: no accounts required. `POST /session {displayName}` → signed cookie/token (`PASETO` or HMAC'd id) carrying `playerId` (random stable id, persisted client-side) — this id is what plots, credits, and blueprints hang off. Rooms are joined by 6-char code; the room create response includes an invite URL embedding the code. Room creator gets a room-admin flag (kick, template settings, PvP toggle). Rate-limit session and room creation by IP. Optional email/OAuth accounts later merely *claim* an existing `playerId`; nothing else changes. WS upgrade requires a valid session token + room membership check; all subsequent authorization (plot edit rights, probe visibility) is per-op inside the room task.

## 11. Deployment Shape & Scaling

**One static Rust binary** (musl build) serving lobby HTTP, WS, static client assets, on a single VM or Fly.io app; SQLite on an attached volume; TLS at the edge (Fly proxy/Caddy). No Kubernetes, no external services at launch. Capacity: a 16-core box runs ~hundreds of *occupied* rooms (one sim thread each, oversubscription fine since most circuits are small) and thousands of parked ones.

**Scaling path (documented, not built):** the constraint is honest — one room never spans processes. Scale-out = many identical binaries + a thin **room directory** (Postgres row per room: code → host) + routing at the edge; rooms are sticky to a host, migrate via park/resume (checkpoint is the migration format for free). Agones/K8s only if fleet management ever demands it. No Nakama/Colyseus — they solve social layers we don't need and can't run our sim.

## 12. Risks & Mitigations

1. **Netlist patch → refactorization cost spikes** on large-room topology edits: mitigate with per-island scoping (only the touched island restamps) and the sim-time-dilation budget so a pathological restamp slows *that island*, not the tick. *Watch metric: p99 patch-apply time.*
2. **WASM preview divergence looks glitchy** (visible pops on reseed): mitigate by reseeding continuous states with a short (2–3 frame) blend on *display* values while snapping *sim* state; ship a debug overlay showing reseed deltas early.
3. **Interest-set boundary artifacts** (Thevenin-equivalent boundaries misextrapolate during neighbors' transients): acceptable — server truth arrives within 65 ms; degrade gracefully by widening interest margin at low zoom.
4. **Backpressure policy bugs** are silent-corruption-shaped: build the three-class queue with property tests + a chaos proxy (latency/loss injection) in week one of net work.
5. **SQLite writer stalls** under checkpoint + op-append contention: dedicated writer thread per room, WAL mode, checkpoints throttled.
6. **Op rebase edge cases** (delete-vs-edit races on the same object): property LWW + delete-wins is the rule; fuzz with a randomized 4-client op interleaver against a single-threaded reference.

## 13. Build Order

1. **`protocol` + `sim-core` skeleton** — doc types, op types, netlist projection, minimal MNA (R, source, switch), native tests. *(Gate: op-fuzz doc convergence test green.)*
2. **Room vertical slice** — axum lobby, one room, WS class-1 only: two browsers co-edit a doc with optimistic rebase; no sim streaming yet.
3. **Sim thread integration** — tick loop, `NetlistPatch` at tick boundaries, Tier-B snapshots (full, no delta), client renders live voltages. *First "two friends see the same lamp light."*
4. **WASM preview** — `sim-wasm`, worker loop, reseed; 60 fps animation between snapshots; offline sandbox falls out here.
5. **Delta encoding + interest management** — snapshot ring, ack piggyback, R-tree culling, aggregates.
6. **Probe pipeline** — subscriptions, min-max decimation, class-3 queues, client ring + WebGL scope feed.
7. **Persistence + lifecycle** — op-log, checkpoints, park/resume, reconnect, late join.
8. **Gameplay authority layer** — permissions, economy checks, contract verifiers (server-side measure), device-state snapshot, trip/repair.
9. **Hardening** — chaos proxy, per-island budgets, rate limits, load test with headless bot clients.

Steps 1–4 are the risk core and mirror the slice-validation questions; everything after is additive.