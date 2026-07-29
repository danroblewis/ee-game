// TS mirror of sim-core's document model (serde internally-tagged with "t").

export type Point = [number, number];

export type ElementKind =
  | { t: 'Wire' }
  | { t: 'Ground' }
  | { t: 'Resistor'; ohms: number }
  | { t: 'Lamp'; ohms: number; rated_watts: number }
  | { t: 'Capacitor'; farads: number }
  | { t: 'Inductor'; henries: number }
  | { t: 'VoltageSource'; dc: number; amp: number; hz: number; phase: number }
  | { t: 'CurrentSource'; amps: number }
  | { t: 'Switch'; closed: boolean }
  | { t: 'Button'; closed: boolean }
  | { t: 'Diode' }
  | { t: 'Zener'; vz: number }
  | { t: 'Led'; color: number }
  | { t: 'Npn'; beta: number }
  | { t: 'Pnp'; beta: number }
  | { t: 'Nmos'; vt: number; k: number }
  | { t: 'Pmos'; vt: number; k: number }
  | { t: 'OpAmp'; rail: number }
  | { t: 'Ota' }
  | { t: 'Timer555' }
  | { t: 'Potentiometer'; ohms: number; wiper: number };

export interface ElementSpec {
  id: number;
  kind: ElementKind;
  pins: Point[];
}

export type InteractOp =
  | { t: 'SetSwitch'; closed: boolean }
  | { t: 'SetValue'; value: number };

export type DocOp =
  | { t: 'Add'; spec: ElementSpec }
  | { t: 'Remove'; id: number }
  | { t: 'Move'; id: number; pins: Point[] }
  | { t: 'SetKind'; id: number; kind: ElementKind };

/** Pin labels for tooltips, by kind. */
export function pinLabels(kind: ElementKind): string[] {
  switch (kind.t) {
    case 'Npn':
    case 'Pnp':
      return ['B', 'C', 'E'];
    case 'Nmos':
    case 'Pmos':
      return ['G', 'D', 'S'];
    case 'OpAmp':
      return ['+', '−', 'out'];
    case 'Ota':
      return ['+', '−', 'out', 'Iabc'];
    case 'Timer555':
      return ['VCC', 'GND', 'TRG', 'THR', 'OUT', 'DIS'];
    case 'Potentiometer':
      return ['A', 'W', 'B'];
    case 'Ground':
      return ['⏚'];
    default:
      return ['a', 'b'];
  }
}

/** The offline-fallback circuit: battery -> switch -> lamp. */
export function demoCircuit(): ElementSpec[] {
  return [
    { id: 1, kind: { t: 'VoltageSource', dc: 9, amp: 0, hz: 0, phase: 0 }, pins: [[2, 2], [2, 8]] },
    { id: 2, kind: { t: 'Wire' }, pins: [[2, 2], [7, 2]] },
    { id: 3, kind: { t: 'Switch', closed: false }, pins: [[7, 2], [11, 2]] },
    { id: 4, kind: { t: 'Wire' }, pins: [[11, 2], [16, 2]] },
    { id: 5, kind: { t: 'Lamp', ohms: 90, rated_watts: 1 }, pins: [[16, 2], [16, 8]] },
    { id: 6, kind: { t: 'Wire' }, pins: [[16, 8], [9, 8]] },
    { id: 7, kind: { t: 'Ground' }, pins: [[9, 8]] },
    { id: 8, kind: { t: 'Wire' }, pins: [[9, 8], [2, 8]] },
  ];
}

/** Per-element live values unpacked from the flat frame. */
export interface ElemLive {
  id: number;
  npins: number;
  /** Voltage at each pin. */
  v: number[];
  /** Current INTO the element at each pin. */
  i: number[];
  /** Dissipated power (negative = delivering). */
  power: number;
}

/** [id, npins, v0..v5, i0..i5, power] — MAX_PINS is 6 (the 555 timer). */
export const FRAME_STRIDE = 15;
export const MAX_PINS = 6;

export function unpackFrame(flat: ArrayLike<number>): Map<number, ElemLive> {
  const out = new Map<number, ElemLive>();
  for (let o = 0; o + FRAME_STRIDE <= flat.length; o += FRAME_STRIDE) {
    const id = flat[o]!;
    const v: number[] = [];
    const i: number[] = [];
    for (let p = 0; p < MAX_PINS; p++) {
      v.push(flat[o + 2 + p]!);
      i.push(flat[o + 2 + MAX_PINS + p]!);
    }
    out.set(id, { id, npins: flat[o + 1]!, v, i, power: flat[o + 2 + 2 * MAX_PINS]! });
  }
  return out;
}
