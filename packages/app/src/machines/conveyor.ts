// THE BELT CONVEYOR — the registry's second machine.
//
// WHY IT EXISTS. "A generic form, not a one-off" was a claim about a seam,
// and a seam nobody has ever put a second thing through is a comment. This
// module is the second thing: a machine whose live state type
// (`ConveyorState` — a belt phase and a roller angle) has NOTHING in common
// with the hoist's (a drum angle, a dust cloud and two flash clocks), which
// is precisely the case the old registry could not type. It composes here
// with one line in machines/index.ts and no change to chip.ts.
//
// WHAT IT IS. The same one-degree-of-freedom carriage the server integrates
// (`crates/machine`), turned on its side: a motor drives a belt, a parcel
// rides the belt, a painted zone on the belt is the goal band, a linear pot
// under the belt reads the parcel's position, and an end stop sits at each
// end of the run. Same nine terminals, same protocol message, same solver
// quantities — a different machine to look at and to think about. It is
// selected the way any machine is: the server's `machine.kind`. For review
// without a server change, `?chip=conveyor` forces the presentation (see
// hoist.ts's dev flags).
//
// HONEST LIMIT. The server stands up one mechanism per room, so this second
// machine is a second PRESENTATION over that mechanism rather than a second
// mechanism. The seam it proves is the client registry's; a genuinely second
// mechanism additionally needs the server's one-machine assumption lifted
// (`HOIST: MachineDef` and the singleton `Hoist` in crates/server).
//
// ROUTING, same rule as the hoist: one lane per pin group, groups ordered
// outward from the mechanism, and a group's horizontal runs stay outside the
// rows of every group inboard of it. Zero electrical crossings as a property
// of the lane assignment rather than of tuning.

import type { ChipFrame, ChipSpec, PinoutRow } from '../chip';
import { atLeast } from '../chip';
import type { MachineAnim, MachineDef, MachineFrame, MachineMsg } from './seam';

/** The conveyor's live state: the server's message plus the two phases the
 * client integrates from its `vel`. No dust, no landing clock, no win flash —
 * an unrelated type to `HoistState`, which is the point. */
export interface ConveyorState {
  m: MachineMsg;
  now: number;
  /** Belt tread phase, in canonical units: the integral of the server's own
   * `vel`, wrapped to one tread pitch. */
  phase: number;
  /** Roller angle, radians: the same integral over the roller radius. */
  roll: number;
  stale: boolean;
}

const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

// ---------------------------------------------------------------- palette

const FRAME_STEEL = '#4a5563';
const BELT = '#23272e';
const BELT_LIT = '#39414c';
const TREAD = '#5c6673';
const PARCEL = '#c8a05a';
const PARCEL_EDGE = '#5d4720';
const BRASS = '#7a6f52';
const DIM = '#6d7d89';
const GREEN = '#7dffb0';
const BAND_FILL = 'rgba(96, 255, 168, 0.16)';

// ---------------------------------------------------------- canonical box

export interface MechBox {
  u0: number;
  u1: number;
  v0: number;
  v1: number;
}

/** The box the numbers below are written in: the chip's die area. */
const CANON: MechBox = { u0: 1.9, u1: 14.1, v0: 2, v1: 13 };

/** A ChipFrame whose local coordinates are `from`, painting into `to`. */
function mapFrame(f: ChipFrame, from: MechBox, to: MechBox): ChipFrame {
  const kx = (to.u1 - to.u0) / (from.u1 - from.u0);
  const ky = (to.v1 - to.v0) / (from.v1 - from.v0);
  if (kx === 1 && ky === 1 && to.u0 === from.u0 && to.v0 === from.v0) return f;
  const k = Math.min(kx, ky);
  const mu = (u: number) => to.u0 + (u - from.u0) * kx;
  const mv = (v: number) => to.v0 + (v - from.v0) * ky;
  const mp = (p: [number, number]): [number, number] => [mu(p[0]), mv(p[1])];
  return {
    ...f,
    at: (u, v) => f.at(mu(u), mv(v)),
    line: (pts, c, w, a) => f.line(pts.map(mp), c, w * k, a),
    dash: (pts, c, w) => f.dash(pts.map(mp), c, w * k),
    box: (u, v, w, h, fill, stroke, lw) =>
      f.box(mu(u), mv(v), w * kx, h * ky, fill, stroke, lw === undefined ? lw : lw * k),
    disc: (u, v, r, fill, stroke, lw) =>
      f.disc(mu(u), mv(v), r * k, fill, stroke, lw === undefined ? lw : lw * k),
    text: (t, u, v, size, c, al, bl) => f.text(t, mu(u), mv(v), size * k, c, al, bl),
    lead: (idx, pts) => f.lead(idx, pts.map(mp)),
  };
}

// ------------------------------------------------------------ the mechanism

/** The belt run, in canonical units: the parcel travels RUN_U0 -> RUN_U1. */
const RUN_U0 = 5.0;
const RUN_U1 = 10.8;
/** Roller centre line and radius. */
const ROLL_V = 7.2;
const ROLL_R = 0.6;
/** Belt surfaces. */
const TOP_V = ROLL_V - ROLL_R;
const BOT_V = ROLL_V + ROLL_R;
/** The parcel that rides the belt. */
const BOX_W = 0.9;
const BOX_H = 0.75;
/** Tread pitch, canonical units. */
const TREAD_PITCH = 0.5;
/** The pot track under the belt. */
const TRACK_V = 9.4;
/** Motor, off the left end of the run. */
const MOTOR_U = 3.2;
const MOTOR_V = 3.4;
const MOTOR_R = 0.55;
/** End-stop blocks, just above the belt at each end of the run. */
const STOP_V = 5.9;
/** Speed that saturates the tread highlight, m/s. */
const VEL_FULL = 0.35;

/** Canonical u for a carriage position in metres. */
const runU = (y: number, h: number) =>
  RUN_U0 + (clamp(y, 0, h) / Math.max(1e-9, h)) * (RUN_U1 - RUN_U0);

/** The wiper as the SOLVER has it, not as `y` implies: they differ at the
 * stops, where the pot clamps off its own ends. */
const wiperOf = (m: MachineMsg) =>
  m.wiper !== undefined ? clamp(m.wiper, 0, 1) : clamp(1 - m.y / Math.max(1e-9, m.h), 0, 1);

function drawMechanism(fr: ChipFrame, st: ConveyorState): void {
  const f = fr;
  const { m } = st;
  const detail = atLeast(f.tier, 'full');
  const withText = atLeast(f.tier, 'text');
  const ux = (y: number) => runU(y, m.h);

  // ---- frame rails the belt runs between
  f.box(RUN_U0 - 1.0, TOP_V - 0.34, RUN_U1 - RUN_U0 + 2.0, 0.12, FRAME_STEEL);
  f.box(RUN_U0 - 1.0, BOT_V + 0.22, RUN_U1 - RUN_U0 + 2.0, 0.12, FRAME_STEEL);

  // ---- belt: two rollers and the band between them
  f.box(RUN_U0, TOP_V, RUN_U1 - RUN_U0, BOT_V - TOP_V, BELT);
  f.line(
    [
      [RUN_U0, TOP_V],
      [RUN_U1, TOP_V],
    ],
    BELT_LIT,
    0.05,
  );
  f.line(
    [
      [RUN_U0, BOT_V],
      [RUN_U1, BOT_V],
    ],
    BELT_LIT,
    0.05,
  );
  for (const cu of [RUN_U0, RUN_U1]) {
    f.disc(cu, ROLL_V, ROLL_R, BELT, '#6b7684', 0.07);
    // Roller spokes: the client's integral of the server's own `vel`.
    for (let k = 0; k < 3; k++) {
      const a = st.roll + (k * Math.PI) / 3;
      f.line(
        [
          [cu - Math.cos(a) * ROLL_R * 0.8, ROLL_V - Math.sin(a) * ROLL_R * 0.8],
          [cu + Math.cos(a) * ROLL_R * 0.8, ROLL_V + Math.sin(a) * ROLL_R * 0.8],
        ],
        '#7f8fa2',
        0.05,
      );
    }
  }
  // ---- treads, scrolling at the belt's own speed
  const vk = clamp(Math.abs(m.vel) / VEL_FULL, 0, 1);
  const off = ((st.phase % TREAD_PITCH) + TREAD_PITCH) % TREAD_PITCH;
  for (let u = RUN_U0 + off; u < RUN_U1; u += TREAD_PITCH) {
    f.line(
      [
        [u, TOP_V],
        [u, TOP_V + 0.22],
      ],
      TREAD,
      0.05,
      0.35 + 0.5 * vk,
    );
    f.line(
      [
        [u, BOT_V - 0.22],
        [u, BOT_V],
      ],
      TREAD,
      0.05,
      0.35 + 0.5 * vk,
    );
  }

  // ---- the goal zone, painted on the belt from the message's own [lo, hi]
  const b0 = ux(m.band[0]);
  const b1 = ux(m.band[1]);
  f.box(b0 - BOX_W / 2, TOP_V - BOX_H - 0.35, b1 - b0 + BOX_W, BOX_H + 0.35, BAND_FILL);
  for (const bu of [b0 - BOX_W / 2, b1 + BOX_W / 2]) {
    f.line(
      [
        [bu, TOP_V - BOX_H - 0.35],
        [bu, TOP_V],
      ],
      GREEN,
      0.045,
    );
  }

  // ---- travel scale in mm, straight off the message
  if (withText) {
    for (let k = 0; k <= 4; k++) {
      const yv = (m.h * k) / 4;
      const tu = ux(yv);
      f.line(
        [
          [tu, BOT_V + 0.4],
          [tu, BOT_V + 0.72],
        ],
        '#3a4552',
        0.03,
      );
      f.text(`${Math.round(yv * 1000)}`, tu, BOT_V + 1.05, 0.18, '#637a86', 'center');
    }
  }

  // ---- motor, coupled to the drive roller by a dashed rod (a rod, not a wire)
  f.disc(MOTOR_U, MOTOR_V, MOTOR_R, '#14141c', '#8f96a6', 0.055);
  if (withText) f.text('M', MOTOR_U, MOTOR_V + 0.03, 0.36, '#c2cad6', 'center');
  f.dash(
    [
      [MOTOR_U + MOTOR_R, MOTOR_V],
      [RUN_U0, MOTOR_V],
      [RUN_U0, ROLL_V - ROLL_R],
    ],
    BRASS,
    0.045,
  );

  // ---- the parcel
  const pu = ux(m.y);
  f.box(pu - BOX_W / 2, TOP_V - BOX_H, BOX_W, BOX_H, PARCEL, PARCEL_EDGE, 0.05);
  f.line(
    [
      [pu - BOX_W / 2, TOP_V - BOX_H],
      [pu + BOX_W / 2, TOP_V],
    ],
    '#6f5527',
    0.03,
  );
  if (detail) f.text('1.2 kg', pu, TOP_V - BOX_H / 2, 0.2, '#3a2c12', 'center');

  // ---- the position sensor: a linear pot track along the ACTUAL run, A at
  // the far end and B at the near one, which is the pot's real polarity.
  const zig: [number, number][] = [[RUN_U0, TRACK_V]];
  for (let k = 0; k < 10; k++) {
    zig.push([
      RUN_U0 + ((k + 0.5) / 10) * (RUN_U1 - RUN_U0),
      TRACK_V + (k % 2 === 0 ? 0.22 : -0.22),
    ]);
  }
  zig.push([RUN_U1, TRACK_V]);
  f.line(zig, '#9aa6b4', 0.05);

  // ---- the wiper tap, at the value the SOLVER was given, tied to the parcel
  // by a dashed rod. That one moving line is the causal story: current in at
  // M+ -> roller turns -> belt runs -> parcel moves -> tap slides -> SNS W.
  const tu = RUN_U0 + (RUN_U1 - RUN_U0) * (1 - wiperOf(m));
  f.dash(
    [
      [tu, TOP_V],
      [tu, TRACK_V - 0.5],
    ],
    BRASS,
    0.04,
  );
  f.box(tu - 0.14, TRACK_V - 0.64, 0.28, 0.28, '#8a7f5e');
  f.line(
    [
      [tu, TRACK_V - 0.5],
      [tu, TRACK_V + 0.5],
    ],
    '#c9c9d4',
    0.05,
  );
  f.line(
    [
      [tu - 0.16, TRACK_V + 0.28],
      [tu, TRACK_V + 0.5],
      [tu + 0.16, TRACK_V + 0.28],
    ],
    '#c9c9d4',
    0.05,
  );

  // ---- the two end stops, bolted at their TRUE trip positions
  const lim = m.lim ?? [0, m.h];
  drawStop(f, ux(lim[1]), m.limt ?? false);
  drawStop(f, ux(lim[0]), m.limb ?? false);

  // ---- goal legend + hold bar, so the package explains itself without the
  // card (which is collapsible, and remembered off).
  if (detail) {
    f.box(2.1, 10.9, 0.4, 0.36, BAND_FILL, GREEN, 0.03);
    f.text('ZONE', 2.62, 11.08, 0.22, GREEN);
    f.text(
      `${(m.band[0] * 1000).toFixed(0)}–${(m.band[1] * 1000).toFixed(0)} mm`,
      4.4,
      11.08,
      0.2,
      DIM,
    );
    const frac = m.need > 0 ? clamp(m.hold / m.need, 0, 1) : 0;
    f.box(2.1, 11.7, 2.4, 0.34, '#1b222b', '#39424f', 0.03);
    if (frac > 0) f.box(2.14, 11.74, 2.32 * frac, 0.26, GREEN);
    f.text(`HOLD ${m.hold.toFixed(1)}/${m.need.toFixed(1)} s`, 4.7, 11.88, 0.2, DIM);
  }
}

/** One end stop, straddling the belt line at its trip position. */
function drawStop(f: ChipFrame, u: number, closed: boolean) {
  f.box(u - 0.3, STOP_V - 0.4, 0.6, 0.8, closed ? '#1e3a2c' : '#20262f', closed ? GREEN : '#5b6473', 0.04);
  f.line(
    closed
      ? [
          [u, STOP_V - 0.25],
          [u, STOP_V + 0.25],
        ]
      : [
          [u, STOP_V - 0.25],
          [u - 0.18, STOP_V],
        ],
    closed ? GREEN : '#8b9caa',
    0.055,
  );
}

// ------------------------------------------------------------- the ChipSpec

const MOTOR = 900;
const SENSOR = 901;
const LIM_FAR = 902;
const LIM_NEAR = 903;

const ratedA = (m: MachineMsg | null): string =>
  m && m.imax !== undefined && m.imax > 0 ? `${m.imax.toFixed(1)} A` : '—';

export const CONVEYOR_CHIP: ChipSpec<ConveyorState> = {
  kind: 'conveyor',
  title: 'BELT CONVEYOR',
  pins: [
    { ref: [MOTOR, 0], label: 'M+' },
    { ref: [MOTOR, 1], label: 'M−' },
    { ref: [LIM_FAR, 0], label: 'FAR A' },
    { ref: [LIM_FAR, 1], label: 'FAR B' },
    { ref: [SENSOR, 0], label: 'SNS A' },
    { ref: [SENSOR, 1], label: 'SNS W' },
    { ref: [SENSOR, 2], label: 'SNS B' },
    { ref: [LIM_NEAR, 0], label: 'NR A' },
    { ref: [LIM_NEAR, 1], label: 'NR B' },
  ],

  status(st) {
    if (st.stale) return ['#8b9caa', 'NO LINK'];
    if (st.m.win) return [GREEN, 'HELD'];
    const inZone = st.m.y >= st.m.band[0] && st.m.y <= st.m.band[1];
    return inZone ? [GREEN, 'IN ZONE'] : ['#8b9caa', 'OUT'];
  },

  plate(st) {
    return [
      ['#e8a04a', `${ratedA(st.m)} MAX · STALL = V/R`],
      ['#6d7d89', 'R 2Ω  L 1.5 mH  K 0.25  ·  SENSE 12.5 mV/mm'],
    ];
  },

  interior(f, st) {
    const fm = mapFrame(f, CANON, f.inner);
    drawMechanism(fm, st);
    if (!atLeast(f.tier, 'text')) return; // hair-thin leads are sub-pixel below this
    const { m } = st;
    const ux = (y: number) => runU(y, m.h);
    const lim = m.lim ?? [0, m.h];
    const uF = ux(lim[1]);
    const uN = ux(lim[0]);
    const tu = RUN_U0 + (RUN_U1 - RUN_U0) * (1 - wiperOf(m));
    fm.lead(0, [[MOTOR_U, MOTOR_V - MOTOR_R], [MOTOR_U, 3.0]]); // M+
    fm.lead(1, [[MOTOR_U, MOTOR_V + MOTOR_R], [MOTOR_U, 5.0]]); // M−
    fm.lead(2, [[uF - 0.3, STOP_V - 0.2], [11.0, STOP_V - 0.2], [11.0, 2.0]]); // FAR A
    fm.lead(3, [[uF + 0.3, STOP_V + 0.2], [11.4, STOP_V + 0.2], [11.4, 3.0]]); // FAR B
    fm.lead(4, [[RUN_U1, TRACK_V], [11.8, TRACK_V], [11.8, 5.0]]); // SNS A — far end
    fm.lead(5, [[tu, TRACK_V + 0.5], [tu, 10.2], [12.2, 10.2], [12.2, 8.0]]); // SNS W
    fm.lead(6, [[RUN_U0, TRACK_V], [RUN_U0, 11.0]]); // SNS B — near end
    fm.lead(7, [[uN - 0.3, STOP_V - 0.2], [4.6, STOP_V - 0.2], [4.6, 12.0]]); // NR A
    fm.lead(8, [[uN - 0.3, STOP_V + 0.2], [4.2, STOP_V + 0.2], [4.2, 13.0]]); // NR B
  },

  lod(f, st) {
    const fm = mapFrame(f, CANON, f.inner);
    const { m } = st;
    const ux = (y: number) => runU(y, m.h);
    const b0 = ux(m.band[0]);
    const b1 = ux(m.band[1]);
    fm.box(b0 - BOX_W / 2, TOP_V - BOX_H, b1 - b0 + BOX_W, BOX_H, BAND_FILL);
    fm.box(ux(m.y) - BOX_W / 2, TOP_V - BOX_H, BOX_W, BOX_H, PARCEL);
  },

  deviceAt(id) {
    if (id === MOTOR) return [MOTOR_U, MOTOR_V];
    if (id === SENSOR) return [(RUN_U0 + RUN_U1) / 2, TRACK_V];
    if (id === LIM_FAR) return [RUN_U1, STOP_V];
    if (id === LIM_NEAR) return [RUN_U0, STOP_V];
    return null;
  },

  pinout(st, meas) {
    const { m } = st;
    const volts = (k: number) => {
      const v = meas.v(k);
      return v === null ? '—' : `${v.toFixed(2)} V`;
    };
    const rows: PinoutRow[] = [
      ['M+', 'drive +, turns the belt', `${ratedA(m)} max · stall = V/R`],
      ['M−', 'drive −', `now ${(m.i * 1000).toFixed(0)} mA`],
      ['FAR A', 'far end stop', `closes at ${((m.lim?.[1] ?? m.h) * 1000).toFixed(0)} mm`],
      ['FAR B', 'far end stop', m.limt ? 'CLOSED' : 'open'],
      ['SNS A', 'track far end — wire to the supply', `now ${volts(4)}`],
      [
        'SNS W',
        'wiper — reads position',
        `now ${volts(5)} · tap ${(100 * (1 - wiperOf(m))).toFixed(0)}% along`,
      ],
      ['SNS B', 'track near end — wire to ground', `now ${volts(6)}`],
      ['NR A', 'near end stop', `closes at ${((m.lim?.[0] ?? 0) * 1000).toFixed(0)} mm`],
      ['NR B', 'near end stop', m.limb ? 'CLOSED' : 'open'],
    ];
    return rows;
  },
};

// ------------------------------------------------------------- the machine

/** Belt phase and roller angle, both integrals of the server's own `vel`.
 * Canonical units per metre of travel, so the treads scroll at exactly the
 * speed the parcel moves. */
const U_PER_M = (RUN_U1 - RUN_U0) / 0.4;

function createConveyorAnim(): MachineAnim<ConveyorState> {
  let m: MachineMsg | null = null;
  let phase = 0;
  let roll = 0;
  return {
    onMessage(next) {
      m = next;
    },
    advance(dtSec) {
      if (!m) return;
      phase = (phase + m.vel * U_PER_M * dtSec) % TREAD_PITCH;
      roll = (roll + (m.vel / 0.02) * dtSec) % (Math.PI * 2);
    },
    frame(at: MachineFrame): ConveyorState {
      return { m: at.m, now: at.now, phase, roll, stale: at.stale };
    },
  };
}

/** The conveyor, as the registry sees it. */
export const CONVEYOR: MachineDef<ConveyorState> = {
  spec: CONVEYOR_CHIP,
  create: createConveyorAnim,
};
