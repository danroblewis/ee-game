// TS mirror of sim-core's document model (serde internally-tagged with "t").

export type Point = [number, number];

/** What a `Gate` computes. A plain string in the document, mirroring
 *  `sim_core::GateOp` — inversion is folded into the op rather than carried
 *  as a separate flag, so the properties panel has one field. */
export type GateOp = 'And' | 'Nand' | 'Or' | 'Nor' | 'Xor' | 'Xnor' | 'Buf' | 'Not';

/** The shape a source's AC component traces. Mirrors `sim_core::Wave`.
 *
 *  Absent on documents written before waveforms existed, and serde defaults
 *  those to `Sine` — so the field is optional here for exactly the same
 *  reason, and `undefined` MEANS sine rather than meaning "unset". */
export type Wave = 'Sine' | 'Square' | 'Triangle' | 'Saw';

/** Buffers and inverters take exactly one input whatever `ins` says — the
 *  mirror of `GateOp::fixed_ins`, and what keeps `pinCount` agreeing with
 *  the solver about how wide the part is. */
export const gateFixedIns = (op: GateOp): number | null =>
  op === 'Buf' || op === 'Not' ? 1 : null;

/** Pin roles for a logic chip, mirroring `sim_core::LogicPins`. Used by the
 *  symbol so the legs are labelled and grouped the way the model sees them. */
export interface LogicPins {
  nIn: number;
  in0: number;
  nOut: number;
  out0: number;
  clk: number | null;
}

export function logicPins(kind: ElementKind): LogicPins | null {
  switch (kind.t) {
    case 'Gate': {
      const n = gateFixedIns(kind.op) ?? clampW(kind.ins, 1, 4);
      return { nIn: n, in0: 2, nOut: 1, out0: 2 + n, clk: null };
    }
    case 'FlipFlop':
      return { nIn: 3, in0: 2, nOut: 2, out0: 5, clk: 0 };
    case 'ShiftReg':
      return { nIn: 3, in0: 2, nOut: clampW(kind.bits, 2, 4), out0: 5, clk: 0 };
    case 'Counter':
      return { nIn: 2, in0: 2, nOut: clampW(kind.bits, 2, 4), out0: 4, clk: 0 };
    case 'Mux': {
      const s = clampW(kind.sel, 1, 2);
      // The I pins are a pass gate: analog, bidirectional, neither input
      // nor output. Only the select lines are thresholded.
      return { nIn: s, in0: 2 + (1 << s), nOut: 0, out0: 0, clk: null };
    }
    default:
      return null;
  }
}

const clampW = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Math.round(n)));

export const isLogic = (kind: ElementKind): boolean => logicPins(kind) !== null;

export type ElementKind =
  | { t: 'Wire' }
  | { t: 'Ground' }
  | { t: 'Resistor'; ohms: number }
  | { t: 'Lamp'; ohms: number; rated_watts: number }
  | { t: 'Speaker'; ohms: number }
  | { t: 'Capacitor'; farads: number }
  | { t: 'Inductor'; henries: number }
  | { t: 'VoltageSource'; dc: number; amp: number; hz: number; phase: number; wave?: Wave }
  | { t: 'CurrentSource'; amps: number }
  | { t: 'Rail'; dc: number; amp: number; hz: number; phase: number; wave?: Wave }
  | { t: 'Switch'; closed: boolean }
  | { t: 'Button'; closed: boolean }
  | { t: 'Diode' }
  | { t: 'Zener'; vz: number }
  | { t: 'Led'; color: number }
  | { t: 'Npn'; beta: number }
  | { t: 'Pnp'; beta: number }
  | { t: 'Nmos'; vt: number; k: number }
  | { t: 'Pmos'; vt: number; k: number }
  | { t: 'OpAmp'; rail: number; isc: number }
  | { t: 'Ota' }
  | { t: 'Timer555' }
  | { t: 'Bbd'; stages: number }
  | { t: 'Pt2399' }
  | { t: 'Potentiometer'; ohms: number; wiper: number }
  /** Light-dependent resistor. `r_dark`/`r_lit` are the CALIBRATION and are
   *  document state; `light` is the READING and is not — the server never
   *  sends it inside an element and never saves it, so it arrives (if at all)
   *  on the separate `sensors` message and a fresh document reads dark.
   *  Optional here for exactly that reason. */
  | { t: 'Photocell'; r_dark: number; r_lit: number; light?: number }
  // `seed` picks WHICH noise, `volts` how loud. Two sources with the same
  // seed are the same signal, so each placement gets its own (catalog.ts).
  | { t: 'Noise'; volts: number; ohms: number; seed: number }
  // The CMOS logic family. A chip here is a passive conductance network
  // whose values are picked by discrete state, so its outputs source real
  // current out of its VCC pin and its levels are fractions of whatever
  // supply it is actually on.
  | { t: 'Gate'; op: GateOp; ins: number }
  | { t: 'FlipFlop'; edge: boolean }
  | { t: 'ShiftReg'; bits: number }
  | { t: 'Counter'; bits: number; modulus: number }
  | { t: 'Mux'; sel: number }
  // Machine fixtures. Not in the parts catalogue — a player cannot place one —
  // but they are ordinary elements in the document, so the client's model of
  // the document has to know them or their pins are untyped and unnamed.
  | { t: 'Motor'; ohms: number; henries: number; bemf: number };

/** A jellybean op-amp's short-circuit output current (741/LM358 class).
 *  Mirrors `sim_core::DEFAULT_OPAMP_ISC`; an op-amp cannot source more than
 *  this, which is why it cannot drive a motor. */
export const DEFAULT_OPAMP_ISC = 0.025;

export interface ElementSpec {
  id: number;
  kind: ElementKind;
  pins: Point[];
  /** Rating tier: 0 is the starting kit, higher rungs are the same device
   *  in a bigger package. Damage-only — the solver ignores it entirely.
   *  Absent on parts placed by older clients; treat as 0. */
  tier?: number;
  /** Quarter-turn symbol rotation, 0..3 clockwise. Only the one-pin parts
   *  (Ground, Rail) use it: everything else takes its orientation from its
   *  pin geometry. Absent on older parts; treat as 0. */
  rot?: number;
  /** What a player calls this part — "TEMPO", "BEAT 1". A label only: it
   *  never reaches the solver. Absent means unnamed, and the UI falls back
   *  to the kind and id. Mirrors `sim_core::ElementSpec::name`. */
  name?: string;
}

export type InteractOp =
  | { t: 'SetSwitch'; closed: boolean }
  | { t: 'SetValue'; value: number };

export type DocOp =
  | { t: 'Add'; spec: ElementSpec }
  | { t: 'Remove'; id: number }
  /** Reposition, and optionally turn the symbol. `rot` is omitted for the
   *  ordinary drag/reshape case ("leave the symbol alone") and carries the
   *  whole of the turn for one-pin parts, whose pins a rotation cannot
   *  move. */
  | { t: 'Move'; id: number; pins: Point[]; rot?: number }
  | { t: 'SetKind'; id: number; kind: ElementKind }
  /** Rename a part. Its own op, not a field on Move or SetKind: renaming is
   *  neither geometric nor electrical, so it must not be able to nudge a pin
   *  or a value by accident, and it reads as one undo entry saying
   *  "renamed". */
  | { t: 'SetName'; id: number; name: string };

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
    case 'Bbd':
      return ['IN', 'OUT', 'CLK', 'GND'];
    case 'Pt2399':
      return ['IN', 'OUT', 'RT', 'GND'];
    case 'Potentiometer':
      return ['A', 'W', 'B'];
    case 'Motor':
      return ['M+', 'M−'];
    case 'Ground':
      return ['⏚'];
    case 'Rail':
      return ['+'];
    case 'Noise':
      return ['out', 'ref'];
    case 'Photocell':
      return ['a', 'b'];
    // The logic family. Pin 0/1 are always VCC/GND, matching the 555.
    case 'Gate': {
      const n = gateFixedIns(kind.op) ?? clampW(kind.ins, 1, 4);
      const ins = ['A', 'B', 'C', 'D'].slice(0, n);
      return ['VCC', 'GND', ...ins, 'Y'];
    }
    case 'FlipFlop':
      return ['VCC', 'GND', 'CLK', 'D', '/RST', 'Q', '/Q'];
    case 'ShiftReg': {
      const b = clampW(kind.bits, 2, 4);
      return ['VCC', 'GND', 'CLK', 'SER', '/RST', ...qNames(b)];
    }
    case 'Counter': {
      const b = clampW(kind.bits, 2, 4);
      return ['VCC', 'GND', 'CLK', '/RST', ...qNames(b)];
    }
    case 'Mux': {
      const s = clampW(kind.sel, 1, 2);
      const chans = Array.from({ length: 1 << s }, (_, j) => `I${j}`);
      const sels = Array.from({ length: s }, (_, j) => `S${j}`);
      return ['VCC', 'GND', ...chans, ...sels, 'Y'];
    }
    default:
      return ['a', 'b'];
  }
}

const qNames = (b: number) => Array.from({ length: b }, (_, j) => `Q${j}`);

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

/** `[id, npins, v0..vN, i0..iN, power]` — the TS mirror of
 *  `sim_core::FRAME_STRIDE` and `sim_core::MAX_PINS`. 10 is set by the
 *  widest logic parts (4-bit shift register, 4:1 mux, both 9 pins). */
export const MAX_PINS = 10;

/** Longest part name the server accepts. Mirrors `sim_core::MAX_NAME`; the
 *  gate refuses anything longer, so the input caps at the same number rather
 *  than letting a player type a name that will bounce. */
export const MAX_NAME = 24;
export const FRAME_STRIDE = 3 + 2 * MAX_PINS;

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
