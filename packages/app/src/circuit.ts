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
  | { t: 'Diode' }
  | { t: 'Zener'; vz: number }
  | { t: 'Led'; color: number }
  | { t: 'Npn'; beta: number }
  | { t: 'Pnp'; beta: number }
  | { t: 'Nmos'; vt: number; k: number }
  | { t: 'Pmos'; vt: number; k: number }
  | { t: 'OpAmp'; rail: number }
  | { t: 'Potentiometer'; ohms: number; wiper: number };

export interface ElementSpec {
  id: number;
  kind: ElementKind;
  pins: Point[];
}

export type InteractOp =
  | { t: 'SetSwitch'; closed: boolean }
  | { t: 'SetValue'; value: number };

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

export const FRAME_STRIDE = 9;

export function unpackFrame(flat: ArrayLike<number>): Map<number, ElemLive> {
  const out = new Map<number, ElemLive>();
  for (let o = 0; o + FRAME_STRIDE <= flat.length; o += FRAME_STRIDE) {
    const id = flat[o]!;
    out.set(id, {
      id,
      npins: flat[o + 1]!,
      v: [flat[o + 2]!, flat[o + 3]!, flat[o + 4]!],
      i: [flat[o + 5]!, flat[o + 6]!, flat[o + 7]!],
      power: flat[o + 8]!,
    });
  }
  return out;
}
