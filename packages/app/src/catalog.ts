// The parts palette: every placeable device, its default parameters, and
// its placement geometry. Cost gates quantity/ratings later — never access
// (design pillar): the full palette is available from minute one.

import { DEFAULT_OPAMP_ISC } from './circuit';
import type { ElementKind, Point } from './circuit';
import {
  partHandle,
  partIsRigid,
  partPins,
  partPivot,
  partReshape,
  partRigidHint,
  partStraighten,
} from './wasm/sim_wasm';

/** Menu categories, in the order the right-click submenu lists them. */
export const CATEGORIES = [
  'Passive',
  'Sources',
  'Switches',
  'Outputs',
  'Diodes',
  'Transistors',
  'Chips',
] as const;
export type Category = (typeof CATEGORIES)[number];

export interface PartDef {
  name: string;
  /** Extra search keywords. */
  keys: string;
  cat: Category;
  /** Single-key shortcut shown in the menu (see PART_HOTKEYS in main.ts). */
  key?: string;
  /** Rating tier this entry places at. 0 (the starting kit) when omitted.
   *
   *  This is where the tech tree will attach: a higher tier is the SAME
   *  `ElementKind` — same solver, same matrix, same numbers — with a bigger
   *  package behind it, so "unlock the 5 W resistor" is one more row in this
   *  list and one more row in `crates/damage`'s ladder, and nothing else in
   *  the game has to move. Everything here is placeable today; gating comes
   *  with the tree itself. */
  tier?: number;
  make(): ElementKind;
}

/** A u32 nobody else in the room is likely to be holding. */
const nextSeed = () => Math.floor(Math.random() * 0x1_0000_0000) >>> 0;

export const CATALOG: PartDef[] = [
  { name: 'Wire', keys: 'w line', cat: 'Passive', key: 'W', make: () => ({ t: 'Wire' }) },
  {
    // 14 AWG: 15 A instead of 3, for runs that carry real load current.
    name: 'Heavy Wire',
    keys: 'w line thick gauge awg power',
    cat: 'Passive',
    tier: 1,
    make: () => ({ t: 'Wire' }),
  },
  { name: 'Ground', keys: 'gnd earth', cat: 'Passive', key: 'G', make: () => ({ t: 'Ground' }) },
  {
    name: 'Resistor',
    keys: 'r ohm',
    cat: 'Passive',
    key: 'R',
    make: () => ({ t: 'Resistor', ohms: 1000 }),
  },
  {
    // The worked example of a tier: electrically identical to the ¼ W part
    // above (same kind, same ohms), thermally a different animal — 5 W and a
    // 25 s body instead of 0.25 W and 6 s.
    name: 'Resistor 5 W',
    keys: 'r ohm power wirewound high',
    cat: 'Passive',
    tier: 1,
    make: () => ({ t: 'Resistor', ohms: 1000 }),
  },
  {
    name: 'Capacitor',
    keys: 'c cap farad',
    cat: 'Passive',
    key: 'C',
    make: () => ({ t: 'Capacitor', farads: 10e-6 }),
  },
  {
    name: 'Inductor',
    keys: 'l coil henry',
    cat: 'Passive',
    key: 'L',
    make: () => ({ t: 'Inductor', henries: 10e-3 }),
  },
  {
    name: 'Potentiometer',
    keys: 'pot knob wiper variable',
    cat: 'Passive',
    key: 'T',
    make: () => ({ t: 'Potentiometer', ohms: 10000, wiper: 0.5 }),
  },
  {
    // The spine of the external-input feature, and deliberately a PART
    // rather than a setting: you place it on the world and it reads what is
    // under it. 1 MΩ dark / 1 kΩ lit is a garden-variety 5 mm CdS cell.
    name: 'Photocell',
    keys: 'ldr light photoresistor cds sensor camera cell',
    cat: 'Passive',
    key: 'Y',
    make: () => ({ t: 'Photocell', r_dark: 1e6, r_lit: 1e3 }),
  },
  {
    name: 'Battery',
    keys: 'v dc source volt',
    cat: 'Sources',
    key: 'V',
    make: () => ({ t: 'VoltageSource', dc: 9, amp: 0, hz: 0, phase: 0 }),
  },
  {
    name: 'AC Source',
    keys: 'sine ac oscillator signal',
    cat: 'Sources',
    key: 'F',
    make: () => ({ t: 'VoltageSource', dc: 0, amp: 5, hz: 2, phase: 0 }),
  },
  {
    name: 'Current Source',
    keys: 'i amp',
    cat: 'Sources',
    key: 'I',
    make: () => ({ t: 'CurrentSource', amps: 0.01 }),
  },
  {
    name: 'V Rail',
    keys: 'rail vcc vdd supply single pin',
    cat: 'Sources',
    key: '⇧V',
    make: () => ({ t: 'Rail', dc: 5, amp: 0, hz: 0, phase: 0 }),
  },
  {
    name: 'Noise',
    keys: 'n white hiss random drum snare noise',
    cat: 'Sources',
    key: '⇧N',
    // A fresh seed per placement: two noise sources with the same seed are
    // literally the same signal, so defaulting them all to one number would
    // make every "independent" hiss in a patch move in lockstep. The seed is
    // a document parameter once placed — editable, saved, and reproducible.
    make: () => ({ t: 'Noise', volts: 1, ohms: 1000, seed: nextSeed() }),
  },
  {
    name: 'Switch',
    keys: 'sw toggle',
    cat: 'Switches',
    key: 'S',
    make: () => ({ t: 'Switch', closed: false }),
  },
  {
    name: 'Button',
    keys: 'push momentary',
    cat: 'Switches',
    make: () => ({ t: 'Button', closed: false }),
  },
  {
    name: 'Lamp',
    keys: 'light bulb',
    cat: 'Outputs',
    key: 'B',
    make: () => ({ t: 'Lamp', ohms: 90, rated_watts: 1 }),
  },
  { name: 'LED', keys: 'light emitting', cat: 'Outputs', key: 'E', make: () => ({ t: 'Led', color: 0 }) },
  {
    name: 'Speaker',
    keys: 'audio sound listen loudspeaker buzzer',
    cat: 'Outputs',
    make: () => ({ t: 'Speaker', ohms: 8 }),
  },
  { name: 'Diode', keys: 'd rectifier flyback freewheel', cat: 'Diodes', key: 'D', make: () => ({ t: 'Diode' }) },
  {
    name: 'Schottky 3 A',
    keys: 'd rectifier power schottky fast',
    cat: 'Diodes',
    tier: 1,
    make: () => ({ t: 'Diode' }),
  },
  {
    name: 'Zener',
    keys: 'z regulator breakdown',
    cat: 'Diodes',
    key: 'Z',
    make: () => ({ t: 'Zener', vz: 5.6 }),
  },
  {
    name: 'NPN',
    keys: 'q bjt transistor',
    cat: 'Transistors',
    key: 'N',
    make: () => ({ t: 'Npn', beta: 100 }),
  },
  {
    name: 'PNP',
    keys: 'q bjt transistor',
    cat: 'Transistors',
    key: 'P',
    make: () => ({ t: 'Pnp', beta: 100 }),
  },
  {
    name: 'NMOS',
    keys: 'fet mosfet transistor',
    cat: 'Transistors',
    key: 'M',
    make: () => ({ t: 'Nmos', vt: 1.5, k: 0.05 }),
  },
  {
    name: 'PMOS',
    keys: 'fet mosfet transistor',
    cat: 'Transistors',
    key: '⇧M',
    make: () => ({ t: 'Pmos', vt: 1.5, k: 0.05 }),
  },
  {
    // The part that can actually switch a motor. A logic-level TO-220:
    // k = 5 A/V² at a 2 V threshold is 67 mΩ from a 5 V gate (20 mΩ from
    // 12 V), so an amp through it burns 67 mW instead of the 1.9 W the
    // small-signal part above would. Tier 1: 20 W on a small heatsink.
    // Its gate draws no current at all, which is why a 25 mA op-amp can
    // drive it.
    name: 'Power NMOS',
    keys: 'fet mosfet power motor switch driver irlz44 to220',
    cat: 'Transistors',
    tier: 1,
    make: () => ({ t: 'Nmos', vt: 2, k: 5 }),
  },
  {
    name: 'Power PMOS',
    keys: 'fet mosfet power high side switch to220',
    cat: 'Transistors',
    tier: 1,
    make: () => ({ t: 'Pmos', vt: 2, k: 5 }),
  },
  {
    name: 'Op-Amp',
    keys: 'amplifier comparator',
    cat: 'Chips',
    key: 'A',
    // 25 mA of output current, like every jellybean op-amp ever made. It is
    // a BRAIN, not a muscle: past this the output folds back and the voltage
    // sags, so an op-amp cannot drive a motor, a relay coil or a filament —
    // it drives the gate of something that can.
    make: () => ({ t: 'OpAmp', rail: 12, isc: DEFAULT_OPAMP_ISC }),
  },
  {
    name: 'OTA',
    keys: 'transconductance current amplifier gm vco',
    cat: 'Chips',
    key: 'U',
    make: () => ({ t: 'Ota' }),
  },
  {
    name: '555 Timer',
    keys: 'timer astable monostable',
    cat: 'Chips',
    key: '5',
    make: () => ({ t: 'Timer555' }),
  },
];

export const partsInCategory = (cat: Category): PartDef[] => CATALOG.filter((p) => p.cat === cat);

export function searchParts(q: string): PartDef[] {
  const s = q.trim().toLowerCase();
  if (!s) return CATALOG;
  return CATALOG.filter(
    (p) => p.name.toLowerCase().includes(s) || p.keys.split(' ').some((k) => k.startsWith(s)),
  );
}

export function pinCount(kind: ElementKind): number {
  switch (kind.t) {
    case 'Ground':
    case 'Rail':
      return 1;
    case 'Timer555':
      return 6;
    case 'Ota':
      return 4;
    case 'Npn':
    case 'Pnp':
    case 'Nmos':
    case 'Pmos':
    case 'OpAmp':
    case 'Potentiometer':
      return 3;
    default:
      return 2;
  }
}

/** Pin layout for a part dragged from grid point A to B.
 *
 * This used to be a hand-written table of offsets, and it was one of the two
 * places skew came from: it snapped the part's PERPENDICULAR offsets to an
 * axis but left the far pin wherever the cursor was, so a diagonal drag put
 * an op-amp down as a skewed triangle before anyone touched a terminal.
 *
 * It is now `sim_core::shape::canonical_pins`, reached through wasm. Same
 * layouts, with the far end projected onto the snapped axis — and, more to
 * the point, the SAME code the placement gate judges the result with. A
 * second copy in TypeScript would be a second set of rounding rules, and the
 * first thing the two disagreed about was where an odd-length
 * potentiometer's wiper goes: the old client rounded the midpoint in world
 * coordinates, so it landed on a different grid unit depending on which way
 * the drag went. */
export function makePins(kind: ElementKind, a: Point, b: Point): Point[] {
  return unflatten(partPins(kind.t, a[0], a[1], b[0], b[1]));
}

/** Is this pin list a legal placement — the part's canonical layout under a
 *  rotation and an optional mirror? Always true for one- and two-pin parts,
 *  whose geometry is not a symbol. The same predicate the server enforces. */
export function isRigidPlacement(kind: ElementKind, pins: Point[]): boolean {
  return partIsRigid(kind.t, flatten(pins));
}

/** Snap a part that predates the shape rule back into formation, keeping its
 *  orientation, handedness and rough size. The identity on a part that is
 *  already in formation. */
export function straightenPins(kind: ElementKind, pins: Point[]): Point[] {
  return unflatten(partStraighten(kind.t, flatten(pins)));
}

/** Drag terminal `k` to `cursor` and get back the WHOLE part: the reshape
 *  gesture for rigid parts. `null` when nothing would change.
 *
 *  This is the only way this client can author a multi-pin placement, which
 *  is what makes "it cannot draw a skewed part" structural rather than a
 *  convention someone has to remember. */
export function reshapePins(
  kind: ElementKind,
  pins: Point[],
  k: number,
  cursor: Point,
): Point[] | null {
  const out = partReshape(kind.t, flatten(pins), k, cursor[0], cursor[1]);
  return out.length ? unflatten(out) : null;
}

/** The grid point a reshape of terminal `k` turns about — the far end of the
 *  part, which stays exactly where it is while the rest swings. `null` for a
 *  terminal that carries the part rather than reorienting it.
 *
 *  Worth having as its own answer: a rigid swing moves EVERY pin, because the
 *  perpendicular offsets turn with the axis. "Which pins did not move" is
 *  therefore not the pivot; this is. */
export function pinPivot(kind: ElementKind, pins: Point[], k: number): Point | null {
  const p = partPivot(kind.t, flatten(pins), k);
  return p.length === 2 ? [p[0]!, p[1]!] : null;
}

/** The shape rule in one sentence, as the placement gate itself would say it
 *  — so the editor uses the same words when it straightens a part on its own
 *  initiative as the server uses when it refuses one. */
export function rigidHint(kind: ElementKind): string {
  return partRigidHint(kind.t);
}

/** What dragging terminal `k` does: 'free' (an endpoint of a two-pin part —
 *  drag it where you like, that is how a resistor gets drawn), 'swing'
 *  (reorient and resize the whole part about its far end) or 'carry' (a leg
 *  with no say in the axis — a DIP pin, an OTA's bias, a wiper — which
 *  carries the whole part). */
export type PinGesture = 'free' | 'swing' | 'carry';
export function pinGesture(kind: ElementKind, k: number): PinGesture {
  return (['free', 'swing', 'carry'] as const)[partHandle(kind.t, k)] ?? 'free';
}

const flatten = (pins: Point[]): Int32Array => {
  const a = new Int32Array(pins.length * 2);
  for (let i = 0; i < pins.length; i++) {
    a[i * 2] = pins[i]![0];
    a[i * 2 + 1] = pins[i]![1];
  }
  return a;
};

const unflatten = (flat: Int32Array): Point[] => {
  const pins: Point[] = [];
  for (let i = 0; i < flat.length; i += 2) pins.push([flat[i]!, flat[i + 1]!]);
  return pins;
};
