# Solving the Freight Hoist

*A walkthrough, written because the goal was not solvable with the parts
that existed. It is now, and this is the circuit — plus the reasoning, so
the next machine can be worked out rather than looked up.*

Everything below is measured, not asserted: the circuit is
`comparator_feedback_circuit()` in `crates/server/src/main.rs`, and the
numbers come from `a_controlled_drive_holds_the_band_without_cooking_the_motor`.

---

## What the machine gives you

Five terminals on the right-hand column of the cabinet:

| terminal | what it is |
|---|---|
| **M+ / M−** | the drum motor's armature. 2 Ω, 1.5 mH, 3 A nameplate. Current INTO M+ lifts. |
| **SENSE-A / SENSE-W / SENSE-B** | a 10 kΩ potentiometer whose wiper tracks the crate. Excite A-to-B and the wiper reads height. |
| **LIM-TOP, LIM-BOT** | dry-contact end stops. Not needed for this solution. |

The goal: hold the crate between **300 mm and 340 mm** for **5 continuous
seconds**. The shaft is 400 mm tall.

## The two things that make it hard

**1. Voltage buys speed, not position.** A DC motor's steady speed is set by
its supply; its *height* is the integral of that. Wire a battery across
M+/M− and the crate runs to the top and parks against the head stop — where
the rotor is stalled, back-EMF is zero, and the armature draws the full
V/R. At 12 V that is **6 A into a 3 A motor**, and it cooks in under two
seconds. So you need feedback: measure the height, and switch the drive.

**2. The thing that decides is not the thing that drives.** This is the part
that was missing. An op-amp is a *brain*: 25 mA of output current, because
that is what a real 741/LM358 output stage folds back to. Holding the crate
against gravity needs **0.94 A** — forty times more. Wire an op-amp output
straight to M+ and it does not explode; it simply refuses, sags to a few
tens of millivolts, and the crate never leaves the floor. (Measured:
`an_op_amp_cannot_drive_a_motor`.)

So the shape of the answer is: **a comparator commands a power switch.**

## The circuit

```
                          6 V
                           │
                           ├──────────────┐
                           │              │
                         [M+]             │        ← the whole motor loop
                    ╔══════════╗          │          runs from here
                    ║  MOTOR   ║        ─┬─  D1     freewheel diode
                    ║ 2Ω 1.5mH ║         ▲          (anode at M−,
                    ╚══════════╝         │           cathode at M+)
                         [M−]            │
                           ├─────────────┘
                           │
    4 V ──[SENSE-A]        │  drain
     │                     │
   ╔═══════╗            ║──┘
   ║ SENSOR║   wiper    ║          Q1: POWER NMOS
   ║  POT  ╟────┐   ┌─╢ ║          (vt = 2 V, k = 5 A/V², tier 1)
   ╚═══════╝    │   │   ║──┐
     │          │  gate    │  source
  [SENSE-B]     │          │
     │          │          │
    GND         │         GND
                │
                └────────────────┐
                                 │ in−
                          ┌──────┴───────┐
       3.2 V ─── in+ ─────┤   OP-AMP     ├──── out ──→ gate of Q1
                          │  rails ±5 V  │
                          └──────────────┘
```

**Parts list (nine components plus wire):**

| # | part | value | why |
|---|---|---|---|
| 1 | Battery | **4 V** | excites the sensor pot: SENSE-A to +4 V, SENSE-B to ground |
| 2 | Battery | **3.2 V** | the setpoint. 4 V × (0.32 m ÷ 0.40 m) = 3.2 V |
| 3 | Op-Amp | rails **±5 V** | the comparator. in+ = setpoint, in− = wiper |
| 4 | Battery | **6 V** | the motor supply |
| 5 | **Power NMOS** | vt 2 V, k 5 A/V², tier 1 | the switch. Gate ← op-amp out, drain ← M−, source → ground |
| 6 | Diode | 1N4001 | freewheel, across the motor: **anode at M−, cathode at M+** |
| 7–9 | Ground | — | pot bottom, op-amp reference chain, FET source, battery negatives |

### Wiring, terminal by terminal

1. **Sensor excitation.** 4 V battery: + to SENSE-A, − to ground. SENSE-B to
   ground. Now SENSE-W sits at `4 V × height/400 mm` — 12.5 mV per mm,
   which is what the machine faceplate says.
2. **Setpoint.** 3.2 V battery, − to ground, + to the op-amp's **in+**.
3. **Comparator.** SENSE-W to the op-amp's **in−**. Below 320 mm the wiper
   is under 3.2 V, so in+ wins and the output goes to +5 V; above 320 mm it
   goes to −5 V. That is bang-bang control.
4. **Gate.** Op-amp **out** straight to the FET's **G**. This costs nothing:
   a MOSFET gate draws literally zero current in the model (and in the world,
   only a switching transient), which is exactly why a 25 mA part is allowed
   to command an amp of drum current.
5. **Power loop.** 6 V battery + to **M+**. **M−** to the FET's **D**. The
   FET's **S** to ground. 6 V battery − to ground.
6. **Freewheel diode** across the motor: anode on the M− (drain) side,
   cathode on the M+ (supply) side. Reverse-biased while the FET is on; it
   catches the winding when the FET turns off.

## Why each value is what it is

**Why 6 V and not 12 V.** Both win. 6 V is better, and the difference is
worth understanding: the loop is bang-bang, so the motor current alternates
between `(V − backEMF)/R` and zero. At 12 V that peak is about 6 A and the
RMS through the wiring is ~2.4 A; at 6 V the peak is ~3 A and the RMS is
~1.7 A. Since heating goes as *i²*, halving the supply quarters the
heating everywhere — motor, wire and supply. 12 V also overshoots the band
to 372 mm before settling. **More supply is not more margin.**

**Why a power MOSFET and not the small-signal NMOS.** The catalogue's plain
NMOS is `k = 0.05 A/V²`. Its saturation current at a 5 V gate is
`½·k·(5 − 1.5)² = 0.31 A` — a third of the 0.94 A needed to hold the crate,
so it cannot lift it *at any gate voltage the op-amp can produce*. And it
would be dissipating about 1.9 W in a part rated for 0.35 W while failing.
The power part is `k = 5 A/V²` at a 2 V threshold: `Rds(on) = 1/(k·(Vgs−Vt))`
= **67 mΩ from a 5 V gate**. At 0.94 A that is 60 mW in a 20 W package. The
FET stops being the fragile thing — which is the point.

**Why not a BJT.** A bipolar transistor's saturation voltage is a fixed
~1 V drop, so at 1 A it burns ~1 W *no matter how big the die*, where the
FET burns `i²·Rds` = 0.06 W. It also needs 10–40 mA of base drive, which an
honest 25 mA op-amp cannot supply and still have margin. This is precisely
why every real low-voltage motor drive is a MOSFET.

**Why not a plain switch.** The Switch/Button contacts are rated 1 A. The
inrush alone is three to six times that, and a switch cannot modulate — you
would be standing there flipping it.

**Why the freewheel diode.** Measured, with and without: with the diode the
FET's drain never leaves the supply rail (peaks at 6.9 V); without it, every
turn-off drives the drain to **56 V**, the MOSFET's avalanche knee, because
the winding's current has nowhere else to go. For *this* machine the FET
survives that — 1.5 mH at 3 A is about 7 mJ a pop and a TO-220 is
avalanche-rated for far more — but it is 56 V appearing in a 6 V circuit,
which will destroy anything else sharing that node, and a larger winding
would end the transistor. Fit the diode.

## What it does, measured

```
entered the band at 0.96 s, won at 5.96 s
peak height 328.5 mm (band is 300–340)
peak motor current 3.85 A, motor stress 0.37 of failure
op-amp 0.00 · supply 0.43 · wiring 0.32 · diode 0.00 · power NMOS 0.00
```

Nothing breaks, nothing is close to breaking, and it holds indefinitely: the
test runs 20 s, which is over three motor thermal time constants.

## What a player has to work out for themselves

1. **That position needs feedback at all.** Constant voltage is the obvious
   move and it visibly fails — the crate parks at the top and the motor
   starts smoking. That failure is the tutorial.
2. **That the sensor is a divider, and the setpoint is arithmetic.** 320 mm
   of 400 mm at 4 V of excitation is 3.2 V. Nothing tells you this; the
   faceplate gives you 12.5 mV/mm and the rest is a ratio.
3. **That the comparator cannot be the driver.** This is the new lesson, and
   the honest one: the op-amp's output current limit is a real number on a
   real datasheet, it is 25 mA, and 25 mA does not lift crates. The failure
   is *silent* — no smoke, no broken part, just a motor that does not turn —
   which is exactly how a real bench behaves and exactly why it takes a
   probe on the output to diagnose.
4. **That gate drive is free.** The bridge between the two halves. A MOSFET
   gate is a capacitor, not a load, so a milliamp-class part can command an
   amp-class part. Brain, then muscle.
5. **That a switched inductive load needs a return path.** Delete the diode
   and probe the drain: 56 V on a 6 V supply.
6. **That RMS is what heats things.** Halving the supply does not halve the
   heating, it quarters it. The bang-bang duty cycle is the reason 12 V
   works but 6 V works *better*.

---

## Related

- `crates/damage/src/lib.rs` — the rating ladder every number above is
  judged against, and why each limit is what it is.
- `docs/design_tech-tree.md` — where the tier the power MOSFET sits on is
  eventually earned rather than given.
- `crates/server/src/main.rs` — `comparator_feedback_circuit()` builds this
  exact netlist, and four tests keep it honest.
