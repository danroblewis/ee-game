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
  | { t: 'Diode' };

export interface ElementSpec {
  id: number;
  kind: ElementKind;
  a: Point;
  b: Point;
}

export type InteractOp =
  | { t: 'SetSwitch'; closed: boolean }
  | { t: 'SetValue'; value: number };

/** The M1 demo: battery -> switch -> lamp, one honest loop. */
export function demoCircuit(): ElementSpec[] {
  return [
    { id: 1, kind: { t: 'VoltageSource', dc: 9, amp: 0, hz: 0, phase: 0 }, a: [2, 2], b: [2, 8] },
    { id: 2, kind: { t: 'Wire' }, a: [2, 2], b: [7, 2] },
    { id: 3, kind: { t: 'Switch', closed: false }, a: [7, 2], b: [11, 2] },
    { id: 4, kind: { t: 'Wire' }, a: [11, 2], b: [16, 2] },
    { id: 5, kind: { t: 'Lamp', ohms: 90, rated_watts: 1 }, a: [16, 2], b: [16, 8] },
    { id: 6, kind: { t: 'Wire' }, a: [16, 8], b: [9, 8] },
    { id: 7, kind: { t: 'Ground' }, a: [9, 8], b: [9, 8] },
    { id: 8, kind: { t: 'Wire' }, a: [9, 8], b: [2, 8] },
  ];
}

/** Per-element live values unpacked from the flat WASM frame. */
export interface ElemLive {
  id: number;
  va: number;
  vb: number;
  current: number;
  power: number;
}

export const FRAME_STRIDE = 5;

export function unpackFrame(flat: Float32Array): Map<number, ElemLive> {
  const out = new Map<number, ElemLive>();
  for (let i = 0; i + FRAME_STRIDE <= flat.length; i += FRAME_STRIDE) {
    const id = flat[i]!;
    out.set(id, {
      id,
      va: flat[i + 1]!,
      vb: flat[i + 2]!,
      current: flat[i + 3]!,
      power: flat[i + 4]!,
    });
  }
  return out;
}
