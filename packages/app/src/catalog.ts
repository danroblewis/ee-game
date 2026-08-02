// The parts palette: every placeable device, its default parameters, and
// its placement geometry. Cost gates quantity/ratings later — never access
// (design pillar): the full palette is available from minute one.

import { DEFAULT_OPAMP_ISC, logicPins } from './circuit';
import type { ElementKind, Point } from './circuit';

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

/** Pin layout for a part dragged from grid point A to B. */
export function makePins(kind: ElementKind, a: Point, b: Point): Point[] {
  if (pinCount(kind) === 1) return [a];
  if (a[0] === b[0] && a[1] === b[1]) b = [a[0] + 3, a[1]];
  if (pinCount(kind) === 2) return [a, b];
  if (kind.t === 'Timer555') {
    // A 4×4 DIP footprint anchored at A, oriented along the drag: VCC at
    // the anchor (top-left), GND bottom-left, TRG/THR down the left edge,
    // DIS/OUT on the right edge. [vcc, gnd, trig, thr, out, dis]
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const horiz = Math.abs(dx) >= Math.abs(dy);
    const ux: Point = horiz ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1];
    const uy: Point = [-ux[1], ux[0]];
    const at = (x: number, y: number): Point => [
      a[0] + ux[0] * x + uy[0] * y,
      a[1] + ux[1] * x + uy[1] * y,
    ];
    return [at(0, 0), at(0, 4), at(0, 1), at(0, 3), at(4, 3), at(4, 1)];
  }
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
  if (kind.t === 'Ota') {
    // [in+, in-, out, bias]: inputs split at A, out at B. The bias pin sits
    // square to the body one step back from the output — that is where the
    // transconductance balls are drawn, so its lead is a straight run out of
    // them (up for a left-to-right part) instead of a diagonal.
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const horiz = Math.abs(dx) >= Math.abs(dy);
    const p: Point = horiz ? [0, 1] : [Math.sign(dy) >= 0 ? -1 : 1, 0];
    const ux: Point = horiz ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1];
    const tip: Point = [b[0] - ux[0], b[1] - ux[1]];
    return [
      [a[0] - p[0], a[1] - p[1]],
      [a[0] + p[0], a[1] + p[1]],
      b,
      [tip[0] - p[0] * 2, tip[1] - p[1] * 2],
    ];
  }
  // 3-pin: split the far end (or inputs) perpendicular to the drag axis.
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const horiz = Math.abs(dx) >= Math.abs(dy);
  const p: Point = horiz ? [0, 1] : [Math.sign(dy) >= 0 ? -1 : 1, 0];
  const off = (pt: Point, k: number): Point => [pt[0] + p[0] * k, pt[1] + p[1] * k];
  switch (kind.t) {
    case 'Npn':
    case 'Pnp':
    case 'Nmos':
    case 'Pmos':
      // [base/gate at A], [collector/drain], [emitter/source]
      return [a, off(b, -2), off(b, 2)];
    case 'OpAmp':
      // [in+], [in-] on the near side, [out] at B
      return [off(a, -1), off(a, 1), b];
    case 'Potentiometer': {
      const mid: Point = [Math.round((a[0] + b[0]) / 2), Math.round((a[1] + b[1]) / 2)];
      return [a, off(mid, -2), b];
    }
    default:
      return [a, b];
  }
}
