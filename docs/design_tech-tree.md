# Tech tree: parts as progression

*Status: recorded idea, not scheduled. Captured 2026-08-02 from the owner.
The tree itself is NOT built. **The seam it needs is** — see "What this
requires of the damage model", which is now a description of shipped code
rather than a requirement.*

## The idea

Parts are not a flat catalogue you get all of at once. They are the
progression. A player starts with low-voltage, low-current, low-power
components and works outward through a tech tree, revealing better parts as
they go.

- **Start small.** The opening kit is genuinely feeble: half-watt resistors,
  a 25 V electrolytic, a 40 mA LED, small-signal transistors. Enough to
  light a lamp and burn a few things learning why.
- **Choose a starting set.** The player picks an opening hand rather than
  receiving a fixed one, so two players' early rooms look different and the
  first hour has a decision in it.
- **Navigate to reveal.** Progress is spatial/graph-like rather than a
  linear unlock list — you move through the tree and new parts appear.
- **Unique parts as nodes.** Some parts are not merely "the bigger one";
  they are specific finds that sit at particular nodes of the tree.

## Why this fits the game

Headroom is already the natural reward, because damage is already the
teacher. `crates/damage/src/lib.rs` is built so the classic mistakes teach
the classic lessons — an LED with no series resistor dies in 0.35 s, an
over-volted electrolytic vents, a stalled motor cooks its armature. If those
limits are the early game's walls, then a **higher-rated part is a wall
coming down**, which is exactly what a progression reward should feel like.
It also gives the low starting tier a purpose beyond difficulty: it is what
makes the unlock legible.

Natural tiers, all of which are the same *kind* at a different rating:

| kind | starting tier | later tier | state |
|---|---|---|---|
| Resistor | ¼ W film | 5 W wirewound | **shipped** |
| Wire | 22 AWG, 3 A | 14 AWG, 15 A | **shipped** |
| Nmos / Pmos | 0.35 W TO-92 | 20 W TO-220 on a heatsink | **shipped** |
| Capacitor | 16 V electrolytic | 100 V film | **shipped** |
| Diode | 1N4001, 1 A | 3 A Schottky | **shipped** |
| Npn / Pnp | 0.35 W TO-92 | 15 W TO-220 | **shipped** |
| Motor, Speaker, Inductor | small | rated for real load | one rung so far |

## What this requires of the damage model — BUILT

A rating used to be hardcoded **per kind** (one match arm per
`ElementKind`), a shape that cannot express "a 0.25 W resistor and a 5 W
resistor are the same kind at different tiers". That is fixed, and the fix
is the only part of this document that exists in code:

- **`ElementSpec::tier: u8`** (`crates/sim-core/src/netlist.rs`) — a
  per-instance document property, `#[serde(default)]` so every part in every
  existing saved room is tier 0, the starting kit. It rides `Add` through
  the ordinary op pipeline and is range-checked against
  `netlist::MAX_TIER` by `check_document`, so client and server can never
  disagree about what is placeable.
- **It never reaches the solver.** `tier` is not in `ElementKind`, so it
  cannot be stamped, cannot change a node count and cannot move a state
  hash. Two resistors at different tiers are the same circuit — which is
  exactly what makes headroom a *reward* rather than a rebalance.
- **`damage::tiers(kind) -> &[Tier]`** is the ladder: one row per rung,
  lowest first, each with a name a player sees. `damage::rating(kind, tier)`
  picks the instance's rung and clamps a tier from a newer build down to the
  best rung this one knows, so a room from the future still loads and still
  cooks its parts.
- **`PartDef::tier`** in `packages/app/src/catalog.ts` is where a rung
  becomes a placeable part.

**Shipping a new tier is therefore: one row in `tiers()`, one entry in the
catalogue.** No solver change, no wire-format change, no migration. Three
worked examples ship already — the 5 W wirewound resistor, the 14 AWG heavy
wire and (the one the Freight Hoist needs) the TO-220 power MOSFET.

What is deliberately NOT built: any gating. Every tier is placeable today.
The tree decides *who may place what*, and that is the tree's job.

## Open questions

- Is the tree per player, per room, or account-wide? Rooms are shared, so
  two players in one room may have different unlocks — what does the
  catalogue show then, and can a player use a part someone else placed?
- How does this interact with the joule-credit shop in `plan.md` M7, which
  already gates *ratings and quantity* by price? The shop and the tree are
  two answers to the same question and should probably become one.
- Blueprint publishing (M7) lets players share circuits. What happens when a
  blueprint contains a part the recipient has not unlocked?
- Does the tree gate *instruments* too (a second scope channel, the FFT), or
  only parts?

## Related

- `docs/plan.md` — M6 device roster, M7 joule-credit shop and blueprints.
- `crates/damage/src/lib.rs` — the rating table this depends on, and its
  comment block explaining why each limit is what it is.
