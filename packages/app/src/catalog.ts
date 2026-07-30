// The parts palette: every placeable device, its default parameters, and
// its placement geometry. Cost gates quantity/ratings later — never access
// (design pillar): the full palette is available from minute one.

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
] as const;
export type Category = (typeof CATEGORIES)[number];

export interface PartDef {
  name: string;
  /** Extra search keywords. */
  keys: string;
  cat: Category;
  /** Single-key shortcut shown in the menu (see PART_HOTKEYS in main.ts). */
  key?: string;
  make(): ElementKind;
}

export const CATALOG: PartDef[] = [
  { name: 'Wire', keys: 'w line', cat: 'Passive', key: 'W', make: () => ({ t: 'Wire' }) },
  { name: 'Ground', keys: 'gnd earth', cat: 'Passive', key: 'G', make: () => ({ t: 'Ground' }) },
  {
    name: 'Resistor',
    keys: 'r ohm',
    cat: 'Passive',
    key: 'R',
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
  { name: 'Diode', keys: 'd rectifier', cat: 'Diodes', key: 'D', make: () => ({ t: 'Diode' }) },
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
    name: 'Op-Amp',
    keys: 'amplifier comparator',
    cat: 'Chips',
    key: 'A',
    make: () => ({ t: 'OpAmp', rail: 12 }),
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

function pinCount(kind: ElementKind): number {
  switch (kind.t) {
    case 'Ground':
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
  if (kind.t === 'Ota') {
    // [in+, in-, out, bias]: inputs split at A, out at B, bias below.
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const horiz = Math.abs(dx) >= Math.abs(dy);
    const p: Point = horiz ? [0, 1] : [Math.sign(dy) >= 0 ? -1 : 1, 0];
    const mid: Point = [Math.round((a[0] + b[0]) / 2), Math.round((a[1] + b[1]) / 2)];
    return [
      [a[0] - p[0], a[1] - p[1]],
      [a[0] + p[0], a[1] + p[1]],
      b,
      [mid[0] + p[0] * 2, mid[1] + p[1] * 2],
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
