// The parts palette: every placeable device, its default parameters, and
// its placement geometry. Cost gates quantity/ratings later — never access
// (design pillar): the full palette is available from minute one.

import { DEFAULT_OPAMP_ISC, logicPins } from './circuit';
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
  // The logic family gets its own category rather than crowding into
  // 'Chips': thirteen entries would have doubled that submenu.
  'Logic',
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
    make: () => ({ t: 'VoltageSource', dc: 0, amp: 5, hz: 2, phase: 0, wave: 'Sine' }),
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
  {
    name: 'Bucket Brigade (BBD)',
    // Everything a player might call it while hunting for an echo.
    keys: 'delay echo bbd bucket brigade chorus flanger reverb tape',
    cat: 'Chips',
    key: '⇧E',
    // 1024 buckets: at a 10 kHz clock that is 51 ms, a slapback, and the
    // clock range a 555 comfortably covers puts the useful span either side
    // of it. Long enough to hear as an echo, short enough that the whole
    // chain fits in 8 kB.
    make: () => ({ t: 'Bbd', stages: 1024 }),
  },
  {
    name: 'Echo Chip (PT2399)',
    keys: 'delay echo pt2399 reverb slapback digital',
    cat: 'Chips',
    key: '⇧P',
    // No stages field: a real chip's RAM is fixed and only its clock moves.
    // The delay is whatever resistor you hang on RT.
    make: () => ({ t: 'Pt2399' }),
  },

  // ------------------------------------------------------- the logic family
  //
  // Every one of these is a CMOS chip with real VCC and GND pins: its levels
  // are fractions of whatever supply you put on it, its outputs source
  // current out of that supply through 50 Ω, and it burns what it burns.
  // Power one from the 9 V rail and it latches up and dies, which is what a
  // 74HC part does.
  {
    name: 'NAND Gate',
    keys: 'logic gate nand digital 7400 cmos',
    cat: 'Logic',
    key: '⇧D',
    // NAND first, and it is the default `GateOp`, because it is the
    // universal gate: two of them cross-coupled are an SR latch, and every
    // other function is reachable from it.
    make: () => ({ t: 'Gate', op: 'Nand', ins: 2 }),
  },
  {
    name: 'AND Gate',
    keys: 'logic gate and digital 7408 cmos',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'And', ins: 2 }),
  },
  {
    name: 'OR Gate',
    keys: 'logic gate or digital 7432 cmos',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'Or', ins: 2 }),
  },
  {
    name: 'NOR Gate',
    keys: 'logic gate nor digital 7402 cmos ring',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'Nor', ins: 2 }),
  },
  {
    name: 'XOR Gate',
    keys: 'logic gate xor exclusive digital 7486 cmos',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'Xor', ins: 2 }),
  },
  {
    name: 'XNOR Gate',
    keys: 'logic gate xnor digital cmos',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'Xnor', ins: 2 }),
  },
  {
    name: 'Inverter',
    keys: 'logic gate not inverter digital 7404 cmos schmitt',
    cat: 'Logic',
    key: '⇧I',
    make: () => ({ t: 'Gate', op: 'Not', ins: 1 }),
  },
  {
    name: 'Buffer',
    keys: 'logic gate buffer driver digital 7407 cmos schmitt',
    cat: 'Logic',
    make: () => ({ t: 'Gate', op: 'Buf', ins: 1 }),
  },
  {
    name: 'D Flip-Flop',
    keys: 'logic flipflop dff register edge clock divide 7474 cmos',
    cat: 'Logic',
    key: '⇧F',
    // Rising-edge triggered. Wire /Q back to D and it divides its clock by
    // two — the first thing worth building out of one.
    make: () => ({ t: 'FlipFlop', edge: true }),
  },
  {
    name: 'D Latch',
    keys: 'logic latch transparent level dff 7475 cmos',
    cat: 'Logic',
    key: '⇧L',
    // The SAME part with `edge` off: Q follows D while the clock is high
    // instead of sampling it once. Toggle the field on a live circuit and
    // watch the difference, which is the point of it being one kind.
    make: () => ({ t: 'FlipFlop', edge: false }),
  },
  {
    name: 'Shift Register',
    keys: 'logic shift register serial sequencer 595 cmos step',
    cat: 'Logic',
    key: '⇧S',
    // 4 bits. Chain two (Q3 -> SER) for 8, exactly as a 74HC595 chain is
    // built. Feed SER from NOR(Q0,Q1,Q2) and it is a self-starting one-hot
    // ring — a step sequencer, in two chips.
    make: () => ({ t: 'ShiftReg', bits: 4 }),
  },
  {
    name: 'Counter',
    keys: 'logic counter binary divide octave 4040 163 cmos',
    cat: 'Logic',
    key: '⇧C',
    // Synchronous, so all four bits move on one edge. Binary WEIGHT is what
    // it has that a shift register does not: divide a clock by 2/4/8/16, or
    // address a mux.
    make: () => ({ t: 'Counter', bits: 4, modulus: 16 }),
  },
  {
    name: 'Multiplexer',
    keys: 'logic mux multiplexer switch analog 4051 selector cmos',
    cat: 'Logic',
    key: '⇧U',
    // A 4051, not a '153: the selected channel is CONNECTED to Y through
    // 50 Ω, so it passes analog in both directions. Drive the select lines
    // from a counter and it is a step sequencer's output stage.
    make: () => ({ t: 'Mux', sel: 2 }),
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

/** Pin count per kind. MUST mirror `sim_core::ElementKind::pin_count` — the
 *  server drops any spec whose pin list is the wrong length, so a
 *  disagreement here is a part that silently refuses to be placed. */
export function pinCount(kind: ElementKind): number {
  switch (kind.t) {
    case 'Ground':
    case 'Rail':
      return 1;
    case 'Timer555':
      return 6;
    case 'Bbd':
    case 'Pt2399':
      return 4;
    case 'Ota':
      return 4;
    // The logic family: two supply pins plus its own signals.
    case 'Gate':
    case 'FlipFlop':
    case 'ShiftReg':
    case 'Counter':
    case 'Mux': {
      const lp = logicPins(kind)!;
      // Mux: VCC, GND, 2^sel channels, sel selects, Y.
      if (kind.t === 'Mux') return 3 + (1 << lp.nIn) + lp.nIn;
      return 2 + lp.nIn + lp.nOut;
    }
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
  // The logic family is the ONE layout Rust cannot generate yet. The whole
  // shape API keys off `Shape::for_tag(t)` — a tag string — but a chip's pin
  // count depends on its kind FIELDS (`Gate.ins`, `ShiftReg.bits`,
  // `Mux.sel`), so a tag alone cannot say whether a `Gate` has 5 pins or 7.
  // Until the boundary carries the pin count, these keep their own DIP
  // layout here, and `Shape::for_tag` classifies them `Free` — meaning a
  // logic chip is NOT yet covered by the rigidity rule. See the merge notes.
  if (logicPins(kind)) {
    // A DIP footprint sized to the part: supplies on the left column at top
    // and bottom (VCC top, GND bottom, matching the 555 and the model's pin
    // order), the remaining inputs down the left edge, and the outputs up
    // the right edge so signal flows left to right.
    const n = pinCount(kind);
    const lp = logicPins(kind)!;
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const horiz = Math.abs(dx) >= Math.abs(dy);
    const ux: Point = horiz ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1];
    const uy: Point = [-ux[1], ux[0]];
    const at = (x: number, y: number): Point => [
      a[0] + ux[0] * x + uy[0] * y,
      a[1] + ux[1] * x + uy[1] * y,
    ];
    // Which edge each pin lands on, by ROLE rather than by index — the
    // mux's pin order is [VCC, GND, I0..I3, S0, S1, Y], so its selects come
    // AFTER its channels and a positional split would put them on the wrong
    // side.
    const right = (p: number) =>
      lp.nOut > 0 ? p >= lp.out0 && p < lp.out0 + lp.nOut : p === n - 1;
    const lefts: number[] = [];
    const rights: number[] = [];
    for (let p = 2; p < n; p++) (right(p) ? rights : lefts).push(p);
    const h = Math.max(4, lefts.length + 1, rights.length + 1);
    const w = 6;
    const pins: Point[] = new Array<Point>(n);
    pins[0] = at(0, 0); // VCC
    pins[1] = at(0, h); // GND
    lefts.forEach((p, k) => (pins[p] = at(0, 1 + k)));
    // The right column runs bottom-up so Q0 sits nearest GND, matching the
    // way a real register's outputs are numbered up the package.
    rights.forEach((p, k) => (pins[p] = at(w, h - 1 - k)));
    return pins;
  }
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
