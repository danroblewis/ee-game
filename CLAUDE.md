# EE Game ("Common Ground")

Web-based multiplayer game where players draw electrical schematics on an
infinite canvas while a real MNA circuit simulation runs authoritatively on
the server. The world IS the canvas: semantic zoom crossfades between a
"real world" device view and the full Falstad-style schematic. All player
interaction (co-op and PvP) happens through simulated electricity — shared
power grids and data lines.

**Read `docs/plan.md` first** — it is the approved implementation plan with
milestones M0–M8 and binding architecture resolutions. Full design and
research docs live in `docs/`.

## Layout

- `crates/sim-math` — deterministic numeric kernels (dense LU). NO FMA/`mul_add`,
  no SIMD, no platform libm: cross-target bit-determinism is CI-enforced.
- `crates/sim-core` — the MNA engine (netlist, wire closure, stamping,
  devices, TR/BE integration, Newton-Raphson + rescue ladder + quarantine).
  Pure computation; compiles for native and wasm32 identically.
- `crates/sim-golden` — golden circuits + analytic tests + `hash` bin for
  the determinism harness.
- `crates/sim-wasm` — wasm-bindgen facade (`--features golden` adds the
  determinism harness entry point; never ship that feature).
- `packages/app` — Vite/TS client (M1 demo: Canvas2D; WebGL canvas package
  comes with the S2 spike).

## Commands

- `cargo test --workspace` — all sim tests (golden circuits vs closed forms).
- `./tools/determinism.sh` — native vs wasm32 state-hash comparison; must
  stay bit-identical. Run after ANY change to sim-math/sim-core numerics.
- `pnpm wasm` — rebuild WASM into `packages/app/src/wasm` (gitignored).
- `pnpm dev` — run the client demo.

## Invariants (do not break)

- Determinism: no `mul_add`, no fast-math, transcendentals via `libm` crate
  only, NaN canonicalization before hashing. `rust-toolchain.toml` is
  pinned; bump deliberately and re-run the harness.
- Every number shown to players must come from the solver — no faked
  electrical behavior, ever (design pillar).
- The sim never stalls the UI: heavy circuits slow sim time (step budget),
  never the frame rate. NR failure ends in quarantine, never a panic.
- `sim-core` stays free of I/O, threads, clocks, and platform dependencies.
