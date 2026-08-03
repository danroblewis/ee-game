# sim-layout — deterministic schematic layout from netlist + part geometry

Prototype of the approved `sim-layout` design (research task: "come up with a
good wire layout based on a netlist and known part dimensions/sizes/pin
layouts"). Takes a room document, keeps its netlist EXACTLY, and re-emits it
as a readable schematic: rigid canonical parts (catalog.ts `makePins`
footprints, rotation/mirror only), grounds and rails dissolved into per-pin
flags, wires re-synthesized by a tiered orthogonal router.

## Pipeline

extract (union-find, `compile()` semantics) → classify nets
(ground / rail / power / signal) → cluster (connected components over signal
nets = functional blocks) → per-block Sugiyama-lite placement (BFS layering
from sources, semantic out→in relaxation, barycenter ordering, 8-pose
orientation search, channel-sized legalization) → serpentine block tiling →
routing (power nets to horizontal bus lanes above the sheet; signal nets:
pattern tier → A* → staircase fallback) → ground/rail flags (planted last so
they cannot seal pin escapes) → emission (runs split at every same-net
endpoint: the engine connects coincident ENDPOINTS only) → verification.

Hard gates, checked before anything is returned:
1. part-pin net partition of the output == input (up to node renumbering);
2. `sim_core::check_document` accepts the output;
3. byte-determinism (pure integer function of the input; BTree containers,
   id-ordered iteration, no floats/RNG/clocks anywhere in the crate).

Reserved ids 900–999 (machine fixtures) are frozen: never moved, routed to
in place.

## Run it

```sh
cargo run -p sim-layout --bin relayout -- rooms/SPM5N6.json out.json out-room.json
cargo test -p sim-layout        # golden rooms: synth, showcase, hoist
```

`out.json` is the element array; `out-room.json` is the full room save with
elements replaced (loadable by the server). stdout is a JSON report:
quality, element/wire deltas, per-tier connection counts, crossings.

## Eyeballing / metrics

```sh
node crates/sim-layout/tools/render.js out.json out.svg    # SVG rendering
node crates/sim-layout/tools/shot.js out.svg               # PNG via chrome
node crates/sim-layout/tools/metrics.js <basename ...>     # insanity metrics
python3 crates/sim-layout/tools/checkpart.py               # independent
                                    # partition-equality check (run in a dir
                                    # with *_before/_after.json pairs)
```

## Measured on the saved rooms (2026-08)

| room | elements | wires | crossings | insanity/10el |
|---|---|---|---|---|
| Analog Synthesizer (generated, "insane") | 71 → 368 | 1 → 275 | 89 → ~90 | 206.9 → ~7 |
| Showcase (hand-drawn control) | 143 → 251 | 67 → 161 | 2 → 5 | 1.82 → 0.6 |
| Main Room (hoist fixtures frozen) | 42 → 82 | 15 → 55 | 0 → ~2 | — |

Known gaps (see the research/spec): pin abutment ("two-terminal parts as
edges") is not implemented — it is the main remaining wire-count lever; the
synth stays ~2× over the ~140-added-element realtime budget; crossings on
dense generated rooms are ~15× a careful hand layout.
