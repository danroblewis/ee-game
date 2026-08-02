# Tech tree: parts as progression

*Status: recorded idea, not scheduled. Captured 2026-08-02 from the owner.
Nothing here is built. The one thing being built now is the SEAM it needs —
see "What this requires of the damage model" below.*

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

| kind | starting tier | later tier |
|---|---|---|
| Resistor | 0.5 W film | high-power wirewound |
| Capacitor | 25 V electrolytic | supercapacitor, high-voltage film |
| Nmos / Pmos | 1 W small-signal | power MOSFET on a heatsink |
| Npn / Pnp | 0.625 W TO-92 | power BJT |
| Motor, Speaker, Inductor | small | rated for real load |

## What this requires of the damage model

Today a rating is hardcoded **per kind** (one match arm per `ElementKind`).
That shape cannot express "a 0.5 W resistor and a 5 W resistor are the same
kind at different tiers", so a tech tree would mean rewriting the damage
crate later.

The requirement, which is being built now as part of the device work: a
rating must become a property of the **part instance** — a tier or variant
carried in the document and validated like any other parameter — defaulting
to the low tier so existing saved rooms still load. Then shipping a new tier
is content, not surgery.

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
