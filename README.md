# Common Ground (EE Game)

A web-based multiplayer game where players draw electrical schematics on an
infinite shared canvas — and a **real circuit simulation** (Modified Nodal
Analysis, Newton–Raphson, trapezoidal integration) runs authoritatively on the
server. The world *is* the canvas. All interaction, co-op and PvP, happens through
simulated electricity: shared power grids, machines you drive by wiring them, and
parts that release their magic smoke when you overload them.

The design pillar everything else serves: **every number a player sees comes from
the solver** — no faked electrical behavior, ever. What you hear through a speaker
is literally samples of the matrix.

## Status

Playable multiplayer prototype: rooms and templates, a live schematic editor with
placement validation (invalid circuits are *refused with a named reason*, not
tolerated), the Freight Hoist machine (a real motor in the matrix, co-simulated
mechanics, a measurable goal), damage and repair, scopes and control panels, and
solver-sample audio. Bit-identical native/WASM determinism is CI-enforced. Not yet
built: island partitioning of the solver (measured, in flight), WebGL renderer,
semantic-zoom crossfade, sessions/permissions. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) for the honest list.

## Running it

Prereqs: the pinned Rust toolchain (`rust-toolchain.toml` — rustup will pick it
up), `wasm-pack`, Node + `pnpm`.

```sh
pnpm install
pnpm wasm                        # build the WASM sim into packages/app/src/wasm
cargo run -p server              # game server on :8080 (override with EE_ADDR)
pnpm dev                         # vite client on :5173, proxies /ws and /api to :8080
```

The client also runs without a server (offline fallback: the same solver compiled
to WASM, single-player, silent speakers).

Tests and checks:

```sh
cargo test --workspace           # solver goldens, gate, machine, damage, server
./tools/determinism.sh           # native vs wasm32 state hashes — must be bit-identical;
                                 # run after ANY change to sim-math/sim-core numerics
pnpm --filter @ee/app exec tsc --noEmit
```

Rooms persist as JSON under `$EE_ROOMS` (default `rooms/`); extra templates load
from `$EE_TEMPLATES/*.json`.

## Where to read next

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — **start here**: the whole system in one
  read; pillars, the life of an edit, co-simulation, what's real and what isn't.
- `docs/asbuilt_*.md` — detailed as-built docs: the solver, co-sim & machines,
  authority & the placement gate, determinism & scale, the client.
- `docs/plan.md` — the approved milestone plan (M0–M8) and binding resolutions.
- `docs/arch_*.md` — the July 2026 *planned* architecture; read
  `ARCHITECTURE.md`'s divergence table before trusting details.
- `docs/game_design.md`, `docs/design_rationale.md` — why the game is shaped this
  way.

## Layout

| path | what |
|---|---|
| `crates/sim-math` | deterministic dense LU — no FMA, no SIMD, bit-identical across targets |
| `crates/sim-core` | the MNA engine: netlist → matrix → NR/TR-BE → quarantine; placement gate |
| `crates/sim-golden` | golden circuits, analytic tests, determinism-harness hashes |
| `crates/sim-wasm` | wasm-bindgen facade (browser gate + offline sim) |
| `crates/machine` | the hoist mechanism (co-simulated, outside the solver) |
| `crates/damage` | stress ODE, per-instance ratings, break/repair decisions |
| `crates/server` | rooms, templates, the tick loop, the op pipeline |
| `packages/app` | the client: Canvas2D renderer, panels, scopes, AudioWorklet |
