# Preferred values: the E-series

*Status: PROTOTYPE BEHIND A FLAG, off by default. The machinery
(`packages/app/src/eseries.ts`) is built and tested; one non-blocking
affordance is wired into the property editor; nothing is enforced anywhere
and no existing room changes. Constraining values to a series is a gameplay
decision that has not been made — the options are at the bottom of this file
for the owner to pick from.*

## Why this exists

From the owner:

> electrical engineering typically doesn't have "whatever you want" parts.
> There are very rarely 3 ohm resistors, you would put three 1 ohm resistors
> together... there is a standard progression of values for each type, and
> for some reason it typically includes 47 or 4.7 or 470.

"For some reason" has a good answer, and the game exists to teach exactly
this sort of thing.

## The engineering, correctly

**The series are geometric.** E-*n* is the *n*-th roots of ten:
`value(k) = round(10^(k/n))`. Because the rungs are a constant RATIO rather
than a constant amount, the same twelve numbers work in every decade and in
every unit — ohms, farads, henries. Scale-free by construction.

**The ratio is chosen so tolerance bands tile the number line.** Consecutive
E12 values differ by `10^(1/12)` = 1.2115, i.e. 21.2%. Half a step is 10.1%,
which is a shade more than the ±10% a 10%-tolerance part carries — so the
band around one stock value ends where the band around the next begins.
Every value you could want is within tolerance of something on the shelf,
using the fewest possible distinct parts. Same argument for E6/±20% and
E24/±5%.

| series | step | half-step | designed for |
|---|---|---|---|
| E6 | ×1.4678 | ±21.2% | ±20% |
| E12 | ×1.2115 | ±10.1% | ±10% |
| E24 | ×1.1007 | ±4.91% | ±5% |
| E96 | ×1.0243 | ±1.21% | ±1% |

**4.7 is `10^(8/12)` = 4.6416**, rounded so it fits on a resistor body.
(Not `10^(7/12)` — that is 3.831, which is where **3.9** comes from. The
mistake is common enough to be worth stating explicitly, and `unitcheck.ts`
asserts against it.)

Two footnotes this project should not tidy away:

1. **The rounding breaks the perfect tiling.** 3.162 became 3.3 (+4.4%) and
   2.610 became 2.7 (+3.4%) because those values were already in warehouses
   when IEC 60063 was written in 1950 and the warehouses won. Printed E12 at
   ±10% therefore leaves a real gap around 1.32–1.35. The ideal is
   geometric; the shipped series is the ideal rounded to printable numbers.
2. **E48/E96 are marginally coarser than their tolerance covers.** The
   "no gaps" property holds for E3–E24 and is slightly false above.

Provenance: not electronics at all. Col. Charles Renard, 1877, replaced 425
balloon mooring-cable sizes with a geometric series of 17 (ISO 3 today). IEC
applied the idea to passives in 1950. Its sibling standard IEC 60062 is the
RKM code — `4k7`, `6R8`, `1M5` — where the prefix stands in for the decimal
point because a printed dot goes missing in a photocopy and a letter does
not. **The value parser accepts RKM form**, which is the cheapest possible
nod to this whole area and costs no gameplay decision at all.

## What is already true of this repo

Measured over the shipped rooms: **24 of 31 distinct passive values (77%)
are already on E24**, and the room authors wrote them that way by hand —
`4.7e-6`, `0.47e-6`, `6.8e-6`, `47e-9`, `100e-9`, `470e3`, plus 5.1 V and
5.6 V zeners (which are E24 precisely because zeners are). Preferred values
would *ratify existing practice*, not impose a new one.

The residue is instructive about where a rule must NOT apply:

- `160.03 Ω`, `3360.4 Ω`, `8329.8 Ω`, `47860 Ω` — machine and device
  internals. A motor winding is a physical fact, not a part off a reel.
- `10860.000000000004 Ω`, `41279.99999999999 Ω` — float artifacts from an
  old knob drag. (The new formatter already displays these as `10.86 kΩ`;
  a series-snapped drag would stop producing them at all.)
- `8 Ω`, `60 Ω`, `90 Ω` — round-number placeholders.

## What is built (and off)

`packages/app/src/eseries.ts`:

- `E_SERIES` — E3/E6/E12/E24/E48/E96 mantissas, and `SERIES_TOLERANCE`.
- `nearestPreferred(v, series)` — nearest stock value, measured in LOG
  space, which is the only measure that makes sense on a geometric ladder.
- `preferredNeighbours(v, series)` — the two rungs a free value sits between.
- `isPreferred`, `preferredError`, `stepPreferred` — membership, error, and
  one detent up or down.
- `seriesExplainer(series)` — the player-facing explanation, written for
  someone who has never heard of E12.
- `stdValuesMode()` / `setStdValuesMode()` — the opt-in. **Default `'off'`.**

Which fields carry a series is declared once, in `units.ts`'s
`SERIES_BY_FIELD`, and covers only the passives a player buys:
`Resistor.ohms` (E24), `Potentiometer.ohms` (E6), `Capacitor.farads` (E12),
`Inductor.henries` (E12), `Zener.vz` (E24). Deliberately absent: every
source, every wiper, and every machine internal.

**Turning it on:** `?stdvalues=hint` in the URL (sticky — it writes
`localStorage['ee.stdvalues']`, because main.ts rewrites the query string
once it has joined a room), or `localStorage.setItem('ee.stdvalues','hint')`.
`'off'` restores the default.

**What it does when on**, and only then:

- The property editor grows a dim line under a value: `✓ E24 standard value`
  when the value is on the ladder, or `not stocked · nearest E24: 10 kΩ ·
  11 kΩ` when it is not. The two neighbours are clickable and snap. Nothing
  snaps on its own.
- A `ⓘ` opens the explainer inline.
- The arrow keys in that field walk the series instead of the 1-2-5 ladder,
  so the detents are felt rather than read.

**What it never does:** change a value without a click, enforce anything,
travel on the wire, or touch a room that has not opted in.

## Options for the owner, ranked

**1 — Ship the hint as a default-on setting.** What is behind the flag
today, with the flag flipped. Zero disruption: no room changes, no server
change, off-series values stay legal forever. Teaches by osmosis, and it
lets you *watch* whether players click the snap before deciding anything
harder. This is the cheap one.

**2 — Series-aware knob-drag.** The canvas value-drag does not exist yet
(`docs/plan.md` M2: "vertical value-drag with unit-aware log sweep"). This
is the cheapest moment in the project's life to build it series-aware: E24
detents by default, a modifier for the continuous sweep. Log-spaced detents
are also better knob *feel* — every step is a fixed ratio, so the knob
behaves identically at 100 Ω and at 100 kΩ — and they kill the
`10860.000000000004` artifact class outright. `stepPreferred` is written and
waiting, unwired.

**3 — A per-part datasheet card.** `chip.ts`'s ⓘ badge and the hoist's
two-tab card are a shipped, good "datasheet" pattern in exactly the right
voice. A VALUES tab on an ordinary part — the ladder with the current value
marked, the colour bands, the RKM spelling, and `seriesExplainer` — would
cost almost nothing and would reach players who click ⓘ. Pairs with option 1,
which is what makes them click. **This is where the explanation should live,
not the tutorial** — the tutorial system does not exist yet, and the card
does.

**4 — `stock_values` as a room-template flag.** `"off" | "E24" | "E12"`, per
room, default off. On, the editor accepts only series values for R/C/L/Vz
while sources and machine internals stay free. This is NI Multisim's
virtual-vs-real split, which is the strongest precedent in the survey and
the only place any tool constrains values. Real upside: scarcity is what
forces series/parallel combination, which is the actual lesson. Needs a
template field and a client gate; existing rooms default off and are
untouched. **This is the gameplay decision, and it is yours to make.**

**5 — The combination solver.** "3.0 kΩ from stock: 2.7 k + 330 (+0.99%) ·
2× 1.5 k in series · 6.8 k ∥ 5.6 k", with an offer to place them. Direct
payoff for the "three 1 Ω resistors" instinct, and it makes option 4 fun
instead of annoying. KiCad ships this, but buried in a separate calculator
app rather than in the schematic editor — doing it inline would be a genuine
differentiator. Pointless before option 4 is decided.

**6 — Tolerance as a simulated property.** A "5% resistor" is 4.7 k ±5% and
your divider is off, and *that* is why the series exists. Deeply on-pillar.
High disruption: changes every existing circuit's numbers and the per-part
offsets must be derived from the room seed, never `Math.random`, or
determinism dies. Future direction, not scheduled.

**Explicitly not recommended: global snapping on by default.** It would
rewrite device internals, silently alter ~19% of existing saved values, and
no simulator in the survey does it.

## The teaching angle already latent in the sim

The owner's own line — *"you would put three 1 ohm resistors together"* —
contains three real skills, two of which this sim already enforces and
nobody has named:

1. **Hit a value the series does not stock.** 3.0 Ω is in E24 but not in
   E12 — exactly the case where combination is the answer.
2. **Split the heat.** Three 1 Ω in series each dissipate a third of the
   power. `crates/damage` already burns an over-stressed ¼ W resistor, so
   "two parts instead of one" is a lesson the sim *already punishes you for
   not knowing*.
3. **Beat the tolerance.** Parallel combination averages error down. Needs
   option 6.

## Related

- `packages/app/src/units.ts` — the shared formatter/parser; `Quantity.series`
  is where a field's series is declared.
- `packages/app/src/unitcheck.ts` — asserts the series are geometric, nest
  correctly, and that the explainer does not repeat the 7/12 mistake.
- `docs/design_tech-tree.md` — the other "parts are the progression" idea;
  options 4 and 6 belong in the same conversation.
