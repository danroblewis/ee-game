#!/usr/bin/env python3
"""Regenerate the ten intro-lesson room templates.

    ./tools/gen-lesson-templates.py [outdir]     # default crates/server/templates

The JSON files in crates/server/templates/ are the shipped truth (embedded
into the server by lessons.rs); this script is their provenance and the
fastest way to re-lay a room out. Lessons can also be edited live: run a
room, fix it, "save as template", and copy the file back over the JSON.

Each output file is a server SaveFile (kind "template") — the same format a
room checkpoint uses. Element ids here are a CONTRACT with the client's
lesson.ts check functions: change one side, change the other.
"""

import json, os, sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "crates/server/templates"


def el(id, kind, pins, tier=0, **kw):
    e = {"id": id, "kind": kind, "pins": [list(p) for p in pins]}
    if tier:
        e["tier"] = tier
    e.update(kw)
    return e


def wire(id, a, b):
    return el(id, {"t": "Wire"}, [a, b])


def battery(id, volts, a, b):
    return el(id, {"t": "VoltageSource", "dc": volts, "amp": 0.0, "hz": 0.0, "phase": 0.0}, [a, b])


def ac(id, amp, hz, a, b):
    return el(id, {"t": "VoltageSource", "dc": 0.0, "amp": amp, "hz": hz, "phase": 0.0}, [a, b])


def rail(id, volts, at):
    return el(id, {"t": "Rail", "dc": volts, "amp": 0.0, "hz": 0.0, "phase": 0.0}, [at])


def gnd(id, at):
    return el(id, {"t": "Ground"}, [at])


def res(id, ohms, a, b, tier=0):
    return el(id, {"t": "Resistor", "ohms": ohms}, [a, b], tier=tier)


def lamp(id, ohms, watts, a, b):
    return el(id, {"t": "Lamp", "ohms": ohms, "rated_watts": watts}, [a, b])


def led(id, color, a, b):
    return el(id, {"t": "Led", "color": color}, [a, b])


def diode(id, anode, cathode):
    return el(id, {"t": "Diode"}, [anode, cathode])


def cap(id, farads, a, b):
    return el(id, {"t": "Capacitor", "farads": farads}, [a, b])


def switch(id, closed, a, b):
    return el(id, {"t": "Switch", "closed": closed}, [a, b])


def button(id, a, b):
    return el(id, {"t": "Button", "closed": False}, [a, b])


def pot(id, ohms, wiper, a, w, b):
    return el(id, {"t": "Potentiometer", "ohms": ohms, "wiper": wiper}, [a, w, b])


def opamp(id, rail_v, inp, inm, out):
    return el(id, {"t": "OpAmp", "rail": rail_v, "isc": 0.025}, [inp, inm, out])


def nmos(id, vt, k, gate, drain, source, tier=0):
    return el(id, {"t": "Nmos", "vt": vt, "k": k}, [gate, drain, source], tier=tier)


def probe(pid, elem, pin, kind, r=None):
    return {"pid": pid, "elem": elem, "pin": pin, "kind": kind, "r": r}


def scope(x, y, w, h, pids, timebase):
    return {"x": x, "y": y, "w": w, "h": h, "pids": pids, "timebase": timebase}


def panel(plid, x0, y0, x1, y1, name):
    return {"plid": plid, "x0": x0, "y0": y0, "x1": x1, "y1": y1, "name": name}


def save(tid, name, blurb, elements, probes, panels_, home, scopes, machine=None):
    ids = [e["id"] for e in elements]
    assert len(ids) == len(set(ids)), f"{tid}: duplicate element id"
    doc = {
        "v": 1,
        "kind": "template",
        "id": tid,
        "name": name,
        "blurb": blurb,
        "elements": elements,
        "probes": probes,
        "next_pid": max([p["pid"] for p in probes], default=0) + 1,
        "panels": panels_,
        "next_plid": max([p["plid"] for p in panels_], default=0) + 1,
        "machine": machine if machine else {"kind": "none"},
        "view": {"home": home, "scopes": scopes},
    }
    path = os.path.join(OUT, f"{tid}.json")
    with open(path, "w") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    print(f"wrote {path}: {len(elements)} elements, {len(probes)} probes")


os.makedirs(OUT, exist_ok=True)

# ---------------------------------------------------------------- lesson 1
# A circuit is a loop. Battery, lamp, one gap. Close it, cut it, close it.
save(
    "intro-01-loop",
    "1 · The Loop",
    "A battery, a lamp, and a gap. Nothing flows until the loop closes — start here.",
    [
        battery(1, 9.0, (4, 3), (4, 9)),
        wire(2, (4, 3), (14, 3)),
        lamp(3, 90.0, 1.0, (14, 3), (14, 9)),
        wire(4, (14, 9), (9, 9)),
        # the gap: (9,9) .. (4,9)
    ],
    [probe(1, 3, 0, "i"), probe(2, 1, 1, "i")],
    [],
    [-1.0, 0.0, 27.0, 21.5],
    [scope(2.0, 11.5, 14.0, 7.0, [1, 2], 2.0)],
)

# ---------------------------------------------------------------- lesson 2
# Ohm's law on a slider; the divider as a dialable voltage.
save(
    "intro-02-divider",
    "2 · Volts, Ohms, the Divider",
    "Ohm's law with a slider, and the voltage divider you will use forever.",
    [
        # station A: V across 1 k, dial the current
        battery(1, 3.0, (4, 3), (4, 9)),
        wire(2, (4, 3), (10, 3)),
        res(3, 1000.0, (10, 3), (10, 9)),
        wire(4, (10, 9), (4, 9)),
        # station B: 4 V across a pot, the wiper picks a voltage
        battery(5, 4.0, (18, 3), (18, 9)),
        wire(6, (18, 3), (24, 3)),
        pot(7, 10000.0, 0.5, (24, 3), (26, 6), (24, 9)),
        wire(8, (24, 9), (18, 9)),
        gnd(9, (18, 9)),
    ],
    [probe(1, 3, 0, "i"), probe(2, 7, 1, "v")],
    [
        panel(1, 2.0, 1.5, 12.0, 10.5, "OHM BENCH — DIAL 5.0 mA"),
        panel(2, 16.0, 1.5, 28.0, 10.5, "DIVIDER — DIAL 1.00 V"),
    ],
    [-23.0, 0.0, 31.0, 21.0],
    [
        scope(2.0, 12.0, 12.0, 6.5, [1], 2.0),
        scope(16.0, 12.0, 12.0, 6.5, [2], 2.0),
    ],
)

# ---------------------------------------------------------------- lesson 3
# Kirchhoff: currents split and re-add; drops share the push.
save(
    "intro-03-kirchhoff",
    "3 · Kirchhoff",
    "Current in equals current out, and the drops around a loop share the battery's push.",
    [
        # station A: one source, two lamp branches (100 mA + 50 mA)
        battery(1, 6.0, (4, 3), (4, 11)),
        wire(2, (4, 3), (9, 3)),
        lamp(3, 60.0, 1.0, (9, 3), (9, 11)),
        wire(4, (9, 3), (14, 3)),
        switch(5, True, (14, 3), (14, 6)),
        lamp(6, 120.0, 0.5, (14, 6), (14, 11)),
        wire(7, (14, 11), (9, 11)),
        wire(8, (9, 11), (4, 11)),
        # station B: series loop, drops sum to the source
        battery(9, 9.0, (22, 3), (22, 11)),
        wire(10, (22, 3), (27, 3)),
        res(11, 2000.0, (27, 3), (27, 7)),
        res(12, 1000.0, (27, 7), (27, 11)),
        wire(13, (27, 11), (22, 11)),
        gnd(14, (22, 11)),
    ],
    [
        probe(1, 1, 1, "i"),
        probe(2, 3, 0, "i"),
        probe(3, 6, 0, "i"),
        probe(4, 11, 0, "v", r=[11, 1]),
        probe(5, 12, 0, "v", r=[12, 1]),
        probe(6, 9, 0, "v", r=[9, 1]),
    ],
    [panel(1, 20.0, 1.5, 31.0, 12.5, "ONE LOOP, ONE PUSH")],
    [-24.0, 0.0, 33.0, 23.0],
    [
        scope(2.0, 13.5, 13.0, 7.0, [1, 2, 3], 2.0),
        scope(17.0, 13.5, 13.0, 7.0, [4, 5, 6], 2.0),
    ],
)

# ---------------------------------------------------------------- lesson 4
# Power and heat. Burn an LED; save its twin with a resistor; repair.
save(
    "intro-04-smoke",
    "4 · Smoke",
    "Watts are volts times amps. Parts have ratings. The damage is real — burn one and see.",
    [
        # loop A: 9 V straight into an LED, one gap. Closing it is REFUSED by
        # the placement gate (no operating point) — the refusal is the lesson.
        battery(1, 9.0, (4, 3), (4, 9)),
        wire(2, (4, 3), (10, 3)),
        led(3, 0, (10, 3), (10, 9)),
        # the gap: (10,9) .. (4,9)
        # loop B: the top gap wants a RESISTOR; then the LED lives
        battery(4, 9.0, (16, 3), (16, 9)),
        wire(5, (16, 3), (20, 3)),
        # the resistor gap: (20,3) .. (24,3)
        led(6, 1, (24, 3), (24, 9)),
        wire(7, (24, 9), (16, 9)),
        # loop C: "10 ohms is close enough", armed by a switch — the burn
        battery(8, 9.0, (4, 13), (4, 19)),
        wire(9, (4, 13), (7, 13)),
        switch(10, False, (7, 13), (10, 13)),
        res(11, 10.0, (10, 13), (13, 13), tier=1),
        led(12, 0, (13, 13), (13, 19)),
        wire(13, (13, 19), (4, 19)),
    ],
    [probe(1, 12, 0, "i"), probe(2, 6, 0, "i")],
    [],
    [-24.0, 0.0, 31.0, 22.0],
    [scope(16.0, 12.0, 12.0, 7.0, [1, 2], 2.0)],
)

# ---------------------------------------------------------------- lesson 5
# RC time: charge through R, hold, dump through the lamp.
save(
    "intro-05-time",
    "5 · Time",
    "A capacitor charges through a resistor on a curve, holds what it stored, and dumps it in a flash.",
    [
        battery(1, 5.0, (4, 3), (4, 11)),
        wire(2, (4, 3), (7, 3)),
        button(3, (7, 3), (11, 3)),
        res(4, 1000.0, (11, 3), (15, 3)),
        wire(5, (15, 3), (16, 3)),
        cap(6, 1000e-6, (16, 3), (16, 11)),
        wire(7, (16, 11), (4, 11)),
        gnd(8, (4, 11)),
        button(9, (16, 3), (20, 3)),
        lamp(10, 20.0, 0.5, (20, 3), (20, 11)),
        wire(11, (20, 11), (16, 11)),
    ],
    [probe(1, 6, 0, "v"), probe(2, 10, 0, "i")],
    [],
    [-17.0, 0.0, 27.0, 23.5],
    [scope(2.0, 13.5, 16.0, 8.0, [1, 2], 4.0)],
)

# ---------------------------------------------------------------- lesson 6
# The diode: one-way current with a fixed toll. AC bench shows the valve.
save(
    "intro-06-diode",
    "6 · One Way",
    "Current has a one-way valve, and the valve charges a fixed toll — this is why the LED died.",
    [
        # station A: slider both ways through a diode + lamp
        battery(1, -5.0, (4, 3), (4, 9)),
        wire(2, (4, 3), (9, 3)),
        diode(3, (9, 3), (9, 6)),
        lamp(4, 60.0, 0.5, (9, 6), (9, 9)),
        wire(5, (9, 9), (4, 9)),
        gnd(6, (4, 9)),
        # station B: the same valve fed a wave — half of it gets through
        ac(7, 4.0, 1.0, (20, 3), (20, 9)),
        wire(8, (20, 3), (25, 3)),
        diode(9, (25, 3), (25, 6)),
        res(10, 300.0, (25, 6), (25, 9)),
        wire(11, (25, 9), (20, 9)),
        gnd(12, (20, 9)),
    ],
    [
        probe(1, 4, 0, "i"),
        probe(2, 3, 0, "v", r=[3, 1]),
        probe(3, 7, 0, "v"),
        probe(4, 10, 0, "v", r=[10, 1]),
    ],
    [panel(1, 2.0, 1.5, 12.0, 10.5, "PUSH IT BOTH WAYS")],
    [-22.0, 0.0, 30.0, 21.5],
    [
        scope(2.0, 12.0, 12.0, 7.0, [1, 2], 2.0),
        scope(16.0, 12.0, 12.0, 7.0, [3, 4], 2.0),
    ],
)

# ---------------------------------------------------------------- lesson 7
# The op-amp: comparator first (it decides), then the 25 mA truth.
save(
    "intro-07-opamp",
    "7 · The Decider",
    "The op-amp compares two voltages and slams to a rail — and 25 mA is all it can push.",
    [
        # decider bench: pot wiper vs a 2 V reference, LED on the verdict.
        # WIPER 0.6, NOT 0.4. The wiper is measured from the grounded end, so
        # 0.4 puts 2.40 V on in+ -- already past the 2.25 V the step checks
        # for. The room opened with the badge at 1/3 and the LED lit, so a
        # player never caused the snap-ON the lesson exists to show them.
        # 0.6 is 1.60 V: below the reference, LED dark, and the turn is the
        # lesson.
        rail(1, 4.0, (4, 3)),
        pot(2, 10000.0, 0.6, (4, 3), (6, 6), (4, 9)),
        gnd(3, (4, 9)),
        wire(4, (6, 6), (12, 6)),
        battery(5, 2.0, (14, 2), (17, 2)),
        gnd(6, (17, 2)),
        wire(7, (12, 4), (12, 2)),
        wire(8, (12, 2), (14, 2)),
        # pins [in+, in-, out]: in+ is the wiper, in- the 2 V reference
        opamp(9, 5.0, (12, 6), (12, 4), (16, 5)),
        res(10, 220.0, (16, 5), (20, 5)),
        led(11, 1, (20, 5), (20, 9)),
        gnd(12, (20, 9)),
        # muscle bench: an op-amp alone against a lamp
        rail(13, 4.0, (24, 6)),
        wire(14, (24, 6), (26, 6)),
        gnd(15, (26, 4)),
        opamp(16, 5.0, (26, 6), (26, 4), (30, 5)),
        switch(17, False, (30, 5), (33, 5)),
        lamp(18, 30.0, 0.5, (33, 5), (33, 9)),
        gnd(19, (33, 9)),
    ],
    [probe(1, 2, 1, "v"), probe(2, 9, 2, "v"), probe(3, 16, 2, "i")],
    [
        panel(1, 2.0, 1.0, 8.0, 10.5, "SETPOINT KNOB"),
        panel(2, 28.5, 3.0, 35.0, 10.5, "MUSCLE TEST"),
    ],
    [-30.0, -1.0, 37.0, 21.0],
    [
        scope(2.0, 12.0, 13.0, 7.0, [1, 2], 4.0),
        scope(17.0, 12.0, 11.0, 7.0, [3], 2.0),
    ],
)

# ---------------------------------------------------------------- lesson 8
# The MOSFET as a switch — and why it must switch the LOW side.
save(
    "intro-08-mosfet",
    "8 · The Gate",
    "A gate that draws nothing commands a current that matters — if you switch the low side.",
    [
        # station A: low-side switch, done right
        rail(1, 5.0, (8, 1)),
        wire(2, (8, 1), (8, 3)),
        lamp(3, 25.0, 1.0, (8, 3), (8, 7)),
        nmos(4, 2.0, 0.05, (4, 9), (8, 7), (8, 11)),
        gnd(5, (8, 11)),
        rail(6, 5.0, (2, 7)),
        wire(7, (2, 7), (2, 9)),
        switch(8, False, (2, 9), (4, 9)),
        res(9, 100000.0, (4, 9), (4, 13)),
        gnd(10, (4, 13)),
        # station B: the same parts with the FET on TOP of the lamp
        rail(11, 5.0, (26, 1)),
        wire(12, (26, 1), (26, 3)),
        nmos(13, 2.0, 0.05, (22, 5), (26, 3), (26, 7)),
        lamp(14, 25.0, 1.0, (26, 7), (26, 11)),
        gnd(15, (26, 11)),
        rail(16, 5.0, (20, 3)),
        wire(17, (20, 3), (20, 5)),
        switch(18, False, (20, 5), (22, 5)),
        res(19, 100000.0, (22, 5), (22, 9), ),
        gnd(20, (22, 9)),
    ],
    [probe(1, 3, 0, "i"), probe(2, 4, 0, "i"), probe(3, 14, 0, "i")],
    [],
    [-16.0, -1.0, 31.0, 23.0],
    [scope(2.0, 14.5, 16.0, 7.5, [1, 2, 3], 2.0)],
)

# ---------------------------------------------------------------- lesson 9
# Power parts: the same command into a TO-92 and a TO-220.
save(
    "intro-09-muscle",
    "9 · Muscle",
    "Why the brain cannot be the muscle: watts land in the package, and packages have tiers.",
    [
        # station A: a small-signal FET asked to carry half an amp
        rail(1, 6.0, (8, 1)),
        wire(2, (8, 1), (8, 3)),
        lamp(3, 12.0, 3.0, (8, 3), (8, 7)),
        nmos(4, 2.0, 0.05, (4, 9), (8, 7), (8, 11)),
        gnd(5, (8, 11)),
        rail(6, 5.0, (2, 7)),
        wire(7, (2, 7), (2, 9)),
        switch(8, False, (2, 9), (4, 9)),
        res(9, 100000.0, (4, 9), (4, 13)),
        gnd(10, (4, 13)),
        # station B: the same command into a power FET
        rail(11, 6.0, (26, 1)),
        wire(12, (26, 1), (26, 3)),
        lamp(13, 12.0, 3.0, (26, 3), (26, 7)),
        nmos(14, 2.0, 5.0, (22, 9), (26, 7), (26, 11), tier=1),
        gnd(15, (26, 11)),
        rail(16, 5.0, (20, 7)),
        wire(17, (20, 7), (20, 9)),
        switch(18, False, (20, 9), (22, 9)),
        res(19, 100000.0, (22, 9), (22, 13)),
        gnd(20, (22, 13)),
    ],
    [
        probe(1, 3, 0, "i"),
        probe(2, 4, 1, "v", r=[4, 2]),
        probe(3, 13, 0, "i"),
        probe(4, 14, 1, "v", r=[14, 2]),
    ],
    [],
    [-16.0, -1.0, 31.0, 23.5],
    [scope(2.0, 14.5, 16.0, 8.0, [1, 2, 3, 4], 2.0)],
)

# --------------------------------------------------------------- lesson 10
# Close the loop: the hoist with sense + compare + drive benches standing,
# one wire missing between the op-amp's verdict and the FET's gate.
HX, HY = 30, 2  # hoist rect origin; rect is 16 x 15
hrect = [HX, HY, HX + 16, HY + 15]
l = HX + 1          # left pin column: M+ (l, HY+3), M- (l, HY+5)
r = HX + 15         # right column: SNS A (r,HY+5) W (r,HY+8) B (r,HY+11)
MP, MM = (l, HY + 3), (l, HY + 5)
SA, SW, SB = (r, HY + 5), (r, HY + 8), (r, HY + 11)
save(
    "intro-10-close-the-loop",
    "10 · Close the Loop",
    "Sense, compare, drive: the hoist with its benches standing and one wire missing.",
    [
        # SENSE: 4 V across the machine's own position pot
        rail(1, 4.0, (r + 3, SA[1])),
        wire(2, (r + 3, SA[1]), SA),
        gnd(3, (r + 3, SB[1])),
        wire(4, (r + 3, SB[1]), SB),
        # the wiper's journey to the comparator, routed south of the machine
        wire(5, SW, (r + 2, SW[1])),
        wire(6, (r + 2, SW[1]), (r + 2, HY + 17)),
        wire(7, (r + 2, HY + 17), (12, HY + 17)),
        wire(8, (12, HY + 17), (12, 14)),
        # COMPARE: wiper vs a 3.2 V setpoint (= 0.32 m through 4 V / 0.40 m)
        battery(9, 3.2, (8, 12), (8, 16)),
        gnd(10, (8, 16)),
        wire(11, (8, 12), (12, 12)),
        # pins [in+, in-, out]: in+ = setpoint, in- = wiper
        opamp(12, 5.0, (12, 12), (12, 14), (16, 13)),
        # DRIVE: 6 V through the motor, low-side power FET, freewheel diode
        rail(13, 6.0, (26, 3)),
        wire(14, (26, 3), (26, 5)),
        wire(15, (26, 5), (28, 5)),
        wire(16, (28, 5), MP),
        diode(17, (28, 7), (28, 5)),
        wire(18, MM, (28, 7)),
        wire(19, (28, 7), (24, 7)),
        nmos(20, 2.0, 5.0, (20, 9), (24, 7), (24, 11), tier=1),
        gnd(21, (24, 11)),
        res(22, 100000.0, (20, 9), (20, 12)),
        gnd(23, (20, 12)),
        # the missing link is OUT (16,13) -> GATE (20,9): the player's wire
        #
        # The machine's own fixture (ids 900-903). The server re-derives the
        # pins from `rect` on load (`ensure_fixture`); they are listed here so
        # the armature/wiper probes above survive `normalize`.
        el(900, {"t": "Motor", "ohms": 2.0, "henries": 1.5e-3, "bemf": 0.0}, [MP, MM]),
        pot(901, 10000.0, 0.98, SA, SW, SB),
        switch(902, False, (r, HY + 2), (r, HY + 3)),
        switch(903, True, (r, HY + 12), (r, HY + 13)),
    ],
    [probe(1, 900, 0, "i"), probe(2, 901, 1, "v"), probe(3, 20, 0, "v")],
    [],
    [-10.0, -2.0, 65.0, 30.0],
    [scope(8.0, 21.0, 13.0, 7.5, [1, 2], 1.0)],
    machine={"kind": "hoist", "rect": hrect},
)

print("done")
