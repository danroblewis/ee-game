// THE FREIGHT HOIST, as a chip.
//
// This file is everything that is SPECIFIC to the hoist: its pin table, its
// nameplate, and the picture of the mechanism that goes inside the package.
// chip.ts owns the package itself (geometry, legs, body, bands, labels, LOD,
// dots, damage) and knows nothing about crates.
//
// THE POINT OF THE INTERIOR. It is not decoration and it is not an animation:
// every shape in here is placed from a number the server measured. The crate
// sits at `y`, the wiper tap sits at `wiper` (the value written into the
// solver), the limit blocks are bolted at `lim` (their real trip heights),
// the cable bows when motor torque falls below the crate's weight, and the
// dust is seeded from the tick's landing speed. The interior's job is to make
// the causal chain visible end to end:
//
//     current in at M+  ->  drum turns  ->  crate rises  ->  tap slides
//                                                     ->  SNS W changes
//
// and the internal bond leads are how that chain reaches the pins. Each lead
// is drawn in the SOLVER's colour for the pin it lands on, so the colour runs
// continuously from the player's wire, through the leg, through the package
// wall, to the device it is actually attached to. Mechanical linkages (the
// motor/drum coupling, the crate/tap tie) are DASHED BRASS, and are the only
// things allowed to cross a lead — the dash is how you tell a rod from a wire.
//
// ROUTING. One lane per pin group, groups ordered outward from the mechanism:
// the limit switches on 9.2 / 9.6, the sensor track on 10.4, the wiper tap on
// 12.4. The limit lanes' vertical runs sit left of the pot track and their
// horizontal runs are outside the track's rows, and the tap lane only ever
// runs inside them. Zero electrical crossings, as a property of the lane
// assignment rather than of tuning. A second machine gets the same for free by
// following the same rule.

import type { ChipFrame, ChipMeas, ChipSpec, PinoutRow } from '../chip';
import { atLeast } from '../chip';
import type { MachineAnim, MachineDef, MachineFrame, MachineMsg } from './seam';

export type { MachineMsg } from './seam';

/** One landing dust particle, in shaft-normalised space (u across the shaft
 * in shaft widths, w up from the floor), so a zoom or a pan never moves it
 * relative to the machine. */
export interface Dust {
  u: number;
  w: number;
  du: number;
  dw: number;
  age: number;
  life: number;
}

/** Everything the package needs to draw itself this frame: the server's
 * message, plus the handful of quantities the client INTEGRATES from that
 * message (drum angle from `vel`, dust from `impact`, flash ages). Nothing
 * here is invented. */
export interface HoistState {
  m: MachineMsg;
  /** performance.now(), for the win flash. */
  now: number;
  /** Cosmetic drum angle: the integral of the server's own `vel`. */
  spin: number;
  /** Seconds since the last landing, and how hard it was. */
  landAge: number;
  landV: number;
  /** Seconds since `win` flipped true. */
  winAge: number;
  dust: Dust[];
  /** No machine message recently: the picture is frozen and says so. */
  stale: boolean;
}

const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

// ---------------------------------------------------------------- palette

const SHAFT = '#0c0f13';
const RAIL = '#2a3240';
const RAIL_LIT = '#4d5b6d';
const STEEL = '#5b6a7a';
const CABLE = '#b9c6d2';
const CABLE_SLACK = '#6d757f';
const BRASS = '#7a6f52';
const DIM = '#6d7d89';
const GREEN = '#7dffb0';
const BAND_FILL = 'rgba(96, 255, 168, 0.16)';

// ------------------------------------------------------------ mechanism box
//
// The mechanism is drawn in ONE band-agnostic function against a canonical
// box, and mapped onto whatever box the caller supplies. Today the only
// caller is the chip's interior. When the world band lands (plan.md M6) it
// calls the same function with the full footprint box wrapped in a faceplate,
// and the crossfade mixes two alphas over the SAME shapes at the SAME world
// position — which is the only way "world <-> schematic zoom feels like one
// object" can actually be true.

export interface MechBox {
  u0: number;
  u1: number;
  v0: number;
  v1: number;
}

/** The box `drawMechanism`'s numbers are written in: the chip's die area. */
export const MECH_CANON: MechBox = { u0: 1.9, u1: 14.1, v0: 2, v1: 13 };

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

/** Shaft opening and travel, in canonical units. */
const SHAFT_U0 = 5.2;
const SHAFT_U1 = 8.8;
const TRAVEL_TOP = 5;
const TRAVEL_BOT = 11;
const CRATE_U0 = 5.8;
const CRATE_U1 = 8.2;
const CRATE_H = 0.8;
const SLAB_H = 0.2;
/** Drum radius, metres (machine constant): only used to turn the server's
 * `vel` into a rotation angle. Never displayed. */
const DRUM_R = 0.02;
/** Crate weight, newtons (m·g, m = 1.2 kg): the cable goes slack below it. */
const CRATE_WEIGHT = 1.2 * 9.81;
/** Cable tension from motor torque, newtons: K·i / r. A cue, not a number. */
const tension = (i: number) => (0.25 * i) / DRUM_R;
/** Landing flash/shake duration, seconds, and the impact that saturates it. */
const LAND_S = 0.45;
const LAND_FULL = 2;
/** Speed that saturates the motion streaks, m/s. */
const VEL_FULL = 0.35;

/** Platform surface row for a crate height in metres. */
const travelV = (y: number, h: number) =>
  TRAVEL_BOT - (clamp(y, 0, h) / Math.max(1e-9, h)) * (TRAVEL_BOT - TRAVEL_TOP);

/** The wiper as the SOLVER has it, not as `y` implies. They differ at the
 * stops, and the dashed tie between crate and tap is what shows that. */
const wiperOf = (m: MachineMsg) =>
  m.wiper !== undefined ? clamp(m.wiper, 0, 1) : clamp(1 - m.y / Math.max(1e-9, m.h), 0, 1);

/**
 * The shaft, the crate, the drum, the sensor and the two limit blocks —
 * drawn into `box`, in the canonical coordinates above.
 *
 * BAND-AGNOSTIC ON PURPOSE. This is the only place these shapes exist, so
 * the schematic band's chip interior and (later) the world band's faceplate
 * are guaranteed to be the same machine seen at two zooms rather than two
 * drawings that drift apart.
 */
export function drawMechanism(fr: ChipFrame, box: MechBox, st: HoistState): void {
  const f = mapFrame(fr, MECH_CANON, box);
  const { m } = st;
  const detail = atLeast(f.tier, 'full');
  const withText = atLeast(f.tier, 'text');
  const vy = (y: number) => travelV(y, m.h);
  const crateV = vy(m.y);
  const crateTop = crateV - CRATE_H;
  const sw = SHAFT_U1 - SHAFT_U0;

  // ---- shaft recess + guide rails
  f.box(SHAFT_U0, 3.9, sw, 7.7, SHAFT);
  for (const u of [SHAFT_U0 + 0.2, SHAFT_U1 - 0.28]) {
    f.box(u, 4.0, 0.08, 7.5, RAIL);
    f.box(u, 4.0, 0.03, 7.5, RAIL_LIT);
  }

  // ---- the goal band, from the message's own [lo, hi]
  const bTop = vy(m.band[1]);
  const bBot = vy(m.band[0]);
  const flash =
    st.winAge === Infinity ? 0 : (0.55 + 0.45 * Math.sin(st.now / 90)) * (st.winAge < 2 ? 1 : 0.45);
  f.box(SHAFT_U0, bTop, sw, bBot - bTop, BAND_FILL);
  f.line(
    [
      [SHAFT_U0, bTop],
      [SHAFT_U1, bTop],
    ],
    `rgba(125,255,176,${(0.55 + 0.45 * flash).toFixed(3)})`,
    0.045,
  );
  f.line(
    [
      [SHAFT_U0, bBot],
      [SHAFT_U1, bBot],
    ],
    `rgba(125,255,176,${(0.55 + 0.45 * flash).toFixed(3)})`,
    0.045,
  );

  // ---- travel scale, 0..h in mm, straight off the message
  if (withText) {
    for (let k = 0; k <= 4; k++) {
      const yv = (m.h * k) / 4;
      const tv = vy(yv);
      f.line(
        [
          [4.6, tv],
          [5.2, tv],
        ],
        '#3a4552',
        0.03,
      );
      f.text(`${Math.round(yv * 1000)}`, 4.5, tv, 0.18, '#637a86', 'right');
    }
  }

  // ---- motor and drum, coupled by a dashed shaft (a rod, not a wire)
  f.disc(3.4, 3.0, 0.85, '#14141c', '#8f96a6', 0.055);
  if (withText) f.text('M', 3.4, 3.05, 0.5, '#c2cad6', 'center');
  f.dash(
    [
      [4.25, 3.0],
      [6.4, 3.0],
    ],
    BRASS,
    0.045,
  );

  // ---- cable: taut and bright when motor torque carries the crate's
  // weight, bowed and dull when it does not. Tension is K·i/r from the
  // message's own current.
  const T = tension(m.i);
  const slack = clamp((CRATE_WEIGHT - T) / CRATE_WEIGHT, 0, 1);
  const cableCol = slack > 0.5 ? CABLE_SLACK : CABLE;
  if (slack > 0.05) {
    const bow = slack * 0.3;
    const midV = (3.0 + crateTop) / 2;
    f.line(
      [
        [7.0, 3.0],
        [7.0 + bow * 0.7, midV],
        [7.0, crateTop],
      ],
      cableCol,
      0.035,
    );
  } else {
    f.line(
      [
        [7.0, 3.0],
        [7.0, crateTop],
      ],
      cableCol,
      0.035,
    );
  }
  f.disc(7.0, 3.0, 0.6, '#2b333d', '#59677a', 0.085);
  for (let k = 0; k < 4; k++) {
    const a = st.spin + (k * Math.PI) / 4;
    f.line(
      [
        [7.0 - Math.cos(a) * 0.48, 3.0 - Math.sin(a) * 0.48],
        [7.0 + Math.cos(a) * 0.48, 3.0 + Math.sin(a) * 0.48],
      ],
      '#7f8fa2',
      0.06,
    );
  }

  // ---- motion streaks, speed straight off the message's `vel`
  const vk = clamp(Math.abs(m.vel) / VEL_FULL, 0, 1);
  if (detail && vk > 0.04) {
    const dir = m.vel > 0 ? 1 : -1; // +vel = rising = up the screen
    const len = vk * 1.0;
    for (let k = 0; k < 4; k++) {
      const u = SHAFT_U0 + sw * (0.2 + 0.2 * k);
      const v0 = dir > 0 ? crateV + 0.1 + k * 0.04 : crateTop - 0.1 - k * 0.04;
      f.line(
        [
          [u, v0],
          [u, v0 + dir * len],
        ],
        '#9fd8ff',
        0.025,
        0.1 + 0.35 * vk,
      );
    }
  }

  // ---- landing shake. Guard the PHASE, not the amplitude: landAge is
  // Infinity until the first landing and Math.sin(Infinity) is NaN, which
  // `* landK` does not clear (0 * NaN is NaN). An NaN here once reached a
  // gradient and killed whole frames.
  const landK =
    st.landAge < LAND_S ? (1 - st.landAge / LAND_S) * clamp(st.landV / LAND_FULL, 0.2, 1) : 0;
  const shake = landK > 0 ? landK * 0.1 * Math.sin(st.landAge * 90) : 0;

  // ---- platform slab + crate
  f.box(5.45 + shake, crateV, 3.1, SLAB_H, landK > 0 ? '#ffd9a0' : STEEL);
  f.box(CRATE_U0 + shake, crateTop, CRATE_U1 - CRATE_U0, CRATE_H, landK > 0 ? '#e8b877' : '#c8a05a', '#5d4720', 0.05);
  f.line(
    [
      [CRATE_U0 + shake, crateTop],
      [CRATE_U1 + shake, crateV],
    ],
    '#6f5527',
    0.03,
  );
  f.line(
    [
      [CRATE_U1 + shake, crateTop],
      [CRATE_U0 + shake, crateV],
    ],
    '#6f5527',
    0.03,
  );
  if (detail) f.text('1.2 kg', (CRATE_U0 + CRATE_U1) / 2 + shake, crateTop + CRATE_H / 2, 0.24, '#3a2c12', 'center');

  // ---- dust: puffs stay inside the shaft
  if (detail && st.dust.length > 0) {
    const floorV = vy(0);
    for (const d of st.dust) {
      const a = (1 - d.age / d.life) * 0.5;
      if (a <= 0) continue;
      const r = 0.08 + 0.14 * (d.age / d.life);
      f.disc(
        clamp(SHAFT_U0 + d.u * sw, SHAFT_U0 + r, SHAFT_U1 - r),
        clamp(floorV - d.w * sw, TRAVEL_TOP + r, floorV - r),
        r,
        `rgba(201,183,154,${a.toFixed(3)})`,
      );
    }
  }

  // ---- floor
  f.box(SHAFT_U0, vy(0) + SLAB_H + 0.15, sw, 0.09, '#2a323c');

  // ---- the position sensor: a pot track drawn along the crate's ACTUAL
  // travel, so the A end is the top of travel and the B end is the floor —
  // which is the pot's real polarity, not a picture of one.
  const zig: [number, number][] = [[10.4, TRAVEL_TOP]];
  for (let k = 0; k < 10; k++) {
    zig.push([10.4 + (k % 2 === 0 ? 0.22 : -0.22), TRAVEL_TOP + ((k + 0.5) / 10) * 6]);
  }
  zig.push([10.4, TRAVEL_BOT]);
  f.line(zig, '#9aa6b4', 0.05);

  // ---- the wiper tap, at the value the SOLVER was given, tied to the crate
  // by a dashed rod. That one moving line is the machine's whole causal
  // story: current in -> drum turns -> crate rises -> tap slides -> the
  // voltage on SNS W changes.
  const vTap = TRAVEL_TOP + 6 * wiperOf(m);
  f.dash(
    [
      [CRATE_U1 + shake, vTap],
      [10.4, vTap],
    ],
    BRASS,
    0.04,
  );
  f.box(9.9, vTap - 0.14, 0.28, 0.28, '#8a7f5e');
  f.line(
    [
      [10.4, vTap],
      [11.0, vTap],
    ],
    '#c9c9d4',
    0.05,
  );
  f.line(
    [
      [10.62, vTap - 0.16],
      [10.4, vTap],
      [10.62, vTap + 0.16],
    ],
    '#c9c9d4',
    0.05,
  );

  // ---- the two limit blocks, bolted to the shaft rail at their TRUE trip
  // heights. No actuator arm is needed or wanted: the platform physically
  // reaches them, which teaches the trip height for free.
  const lim = m.lim ?? [0, m.h];
  drawLimit(f, vy(lim[1]), m.limt ?? false);
  drawLimit(f, vy(lim[0]), m.limb ?? false);

  // ---- goal legend + hold bar: the chip explains itself without the card,
  // which matters because the card is collapsible and remembered off.
  if (detail) {
    f.box(2.0, 8.75, 0.4, 0.36, BAND_FILL, GREEN, 0.03);
    f.text('BAND', 2.52, 8.93, 0.22, GREEN);
    f.text(
      `${(m.band[0] * 1000).toFixed(0)}–${(m.band[1] * 1000).toFixed(0)} mm`,
      2.0,
      9.5,
      0.2,
      DIM,
    );
    const frac = m.need > 0 ? clamp(m.hold / m.need, 0, 1) : 0;
    f.box(2.0, 10.1, 2.0, 0.34, '#1b222b', '#39424f', 0.03);
    if (frac > 0) f.box(2.04, 10.14, 1.92 * frac, 0.26, GREEN);
    f.text(`HOLD ${m.hold.toFixed(1)}/${m.need.toFixed(1)} s`, 2.0, 10.85, 0.2, DIM);
  }
}

/** One limit block, straddling the shaft wall at its trip height. Closed is
 * a lit block with its contact bridged; open is a dark block with a gap. */
function drawLimit(f: ChipFrame, v: number, closed: boolean) {
  f.box(8.175, v - 0.3, 0.8, 0.6, closed ? '#1e3a2c' : '#20262f', closed ? GREEN : '#5b6473', 0.04);
  f.line(
    closed
      ? [
          [8.3, v],
          [8.85, v],
        ]
      : [
          [8.3, v],
          [8.55, v - 0.18],
        ],
    closed ? GREEN : '#8b9caa',
    0.055,
  );
}

// ------------------------------------------------------------- the ChipSpec

const MOTOR = 900;
const SENSOR = 901;
const LIM_TOP = 902;
const LIM_BOT = 903;

/** The motor's nameplate current as text, or a placeholder when the server
 * has not told us (a build from before parts could break). */
export function ratedA(m: MachineMsg | null): string {
  return m && m.imax !== undefined && m.imax > 0 ? `${m.imax.toFixed(1)} A` : '—';
}

export const HOIST_CHIP: ChipSpec<HoistState> = {
  kind: 'hoist',
  title: 'FREIGHT HOIST',
  // Declaration order only: WHERE each leg stands is read off the fixture
  // element it names, so this table cannot disagree with the server about the
  // footprint or the rows (chip.ts, "geometry is measured, not declared").
  //
  // The server lays them out left-is-drive, right-is-information, with the
  // right column a vertical MAP OF THE SHAFT: top stop at the top, floor stop
  // at the bottom, the sensor spanning between them with its wiper tap at
  // mid-travel. The interior below draws the top of travel and the floor at
  // the rows SNS A and SNS B actually land on, so the sensor's two ends are
  // literally at the two ends of the crate's travel however the server lays
  // the package out.
  pins: [
    { ref: [MOTOR, 0], label: 'M+' },
    { ref: [MOTOR, 1], label: 'M−' },
    { ref: [LIM_TOP, 0], label: 'TOP A' },
    { ref: [LIM_TOP, 1], label: 'TOP B' },
    { ref: [SENSOR, 0], label: 'SNS A' },
    { ref: [SENSOR, 1], label: 'SNS W' },
    { ref: [SENSOR, 2], label: 'SNS B' },
    { ref: [LIM_BOT, 0], label: 'BOT A' },
    { ref: [LIM_BOT, 1], label: 'BOT B' },
  ],

  status(st) {
    if (st.stale) return ['#8b9caa', 'NO LINK'];
    if (st.m.win) return [GREEN, 'HELD'];
    const inBand = st.m.y >= st.m.band[0] && st.m.y <= st.m.band[1];
    return inBand ? [GREEN, 'IN BAND'] : ['#8b9caa', 'OUT'];
  },

  plate(st) {
    return [
      // Deliberately loud: it is the difference between a working hoist and
      // a dead motor, and `imax` is the server's own damage-table number, so
      // the plate can never promise a limit the model does not enforce.
      ['#e8a04a', `${ratedA(st.m)} MAX · STALL = V/R`],
      ['#6d7d89', 'R 2Ω  L 1.5 mH  K 0.25  ·  SENSE 12.5 mV/mm'],
    ];
  },

  interior(f, st) {
    // The mechanism is written in MECH_CANON coordinates and painted into
    // whatever die area the package ACTUALLY has this frame. On the shipped
    // footprint those are the same box; if the server ever re-lays the
    // package out, the shaft follows the walls instead of hanging off them.
    // The bond leads go through the same map, so a lead written at the top of
    // travel still leaves from the top of travel.
    const fm = mapFrame(f, MECH_CANON, f.inner);
    drawMechanism(fm, MECH_CANON, st);
    // Bond leads last, over the mechanism: they are what the player is
    // actually wiring to, so they must never be hidden by the crate.
    if (!atLeast(f.tier, 'text')) return; // 0.045 grid is sub-pixel below this
    const { m } = st;
    const vy = (y: number) => travelV(y, m.h);
    const lim = m.lim ?? [0, m.h];
    const vT = vy(lim[1]);
    const vB = vy(lim[0]);
    const vTap = TRAVEL_TOP + 6 * wiperOf(m);
    fm.lead(0, [[2.55, 3.0]]); // M+  : armature, straight out
    fm.lead(1, [
      [3.4, 3.85],
      [3.4, 5.0],
    ]); // M−
    fm.lead(2, [
      [8.95, vT - 0.12],
      [9.2, vT - 0.12],
      [9.2, 2.0],
    ]); // TOP A
    fm.lead(3, [
      [8.95, vT + 0.12],
      [9.6, vT + 0.12],
      [9.6, 3.0],
    ]); // TOP B
    fm.lead(4, [[10.4, TRAVEL_TOP]]); // SNS A — the TOP of travel
    fm.lead(5, [
      [11.0, vTap],
      [12.4, vTap],
      [12.4, 8.0],
    ]); // SNS W — pivots as the tap slides
    fm.lead(6, [[10.4, TRAVEL_BOT]]); // SNS B — the floor
    fm.lead(7, [
      [8.95, vB - 0.12],
      [9.2, vB - 0.12],
      [9.2, 12.0],
    ]); // BOT A
    fm.lead(8, [
      [8.95, vB + 0.12],
      [9.6, vB + 0.12],
      [9.6, 13.0],
    ]); // BOT B
  },

  lod(f, st) {
    // Below LOD_FULL the package is a smudge, but "is the crate in the band"
    // is still a cross-the-room legible fact — so it survives, on shape.
    const { m } = st;
    const vy = (y: number) => travelV(y, m.h);
    const sw = SHAFT_U1 - SHAFT_U0;
    f.box(SHAFT_U0, vy(m.band[1]), sw, vy(m.band[0]) - vy(m.band[1]), BAND_FILL);
    f.box(CRATE_U0, vy(m.y) - CRATE_H, CRATE_U1 - CRATE_U0, CRATE_H, '#c8a05a');
  },

  // Where each device actually is inside the die, so "this one is cooking"
  // lands on the thing that is cooking. Burning the motor out is the
  // machine's core lesson, and the mark has to be unmistakably ON the motor.
  deviceAt(id) {
    if (id === MOTOR) return [3.4, 3.0];
    if (id === SENSOR) return [10.4, 8.0];
    if (id === LIM_TOP) return [8.575, TRAVEL_TOP + 0.6];
    if (id === LIM_BOT) return [8.575, TRAVEL_BOT - 0.6];
    return null;
  },

  /**
   * THE DATASHEET, and where its numbers come from.
   *
   * The three sensor rows used to print `wiper × 5 V` captioned "at 5 V
   * excitation" — a spec-sheet nominal in the column a player reads as a
   * reading. With a 1 kΩ load on the wiper it said 2.40 V while a probe on
   * that very node said 1.00 V, because a 10 kΩ pot loaded at 1 kΩ is not an
   * ideal divider and the nominal cannot know that. The pot's loading IS the
   * lesson there, and it is exactly what the solver already computes.
   *
   * So every row that states what a terminal is doing now states a MEASURED
   * quantity: `meas` is this frame's solver frame for that leg. Rows that
   * state a constant (the nameplate current, a trip height) stay constant —
   * a datasheet may print a rating; it may not print a nominal and let it
   * pass for an instrument.
   */
  pinout(st, meas) {
    const { m } = st;
    const volts = (k: number) => {
      const v = meas.v(k);
      return v === null ? '—' : `${v.toFixed(2)} V`;
    };
    const rows: PinoutRow[] = [
      ['M+', 'armature +, drives the drum', `${ratedA(m)} max · stall = V/R`],
      ['M−', 'armature −', `now ${(m.i * 1000).toFixed(0)} mA`],
      ['TOP A', 'head-stop switch', `closes at ${((m.lim?.[1] ?? m.h) * 1000).toFixed(0)} mm`],
      ['TOP B', 'head-stop switch', m.limt ? 'CLOSED' : 'open'],
      ['SNS A', 'track top — wire to the supply', `now ${volts(4)}`],
      ['SNS W', 'wiper — reads height', `now ${volts(5)} · tap ${(100 * (1 - wiperOf(m))).toFixed(0)}% up`],
      ['SNS B', 'track floor — wire to ground', `now ${volts(6)}`],
      ['BOT A', 'floor-stop switch', `closes at ${((m.lim?.[0] ?? 0) * 1000).toFixed(0)} mm`],
      ['BOT B', 'floor-stop switch', m.limb ? 'CLOSED' : 'open'],
    ];
    return rows;
  },
};

// ------------------------------------------------------------- the machine

/** The hoist's client-side animator.
 *
 * Everything it holds is an INTEGRAL of something the server sent: the drum
 * angle from `vel`, the dust from `impact`, the two flash clocks from the
 * ticks those events arrived on. It simulates nothing. */
function createHoistAnim(): MachineAnim<HoistState> {
  let m: MachineMsg | null = null;
  let spin = 0;
  let landAge = Infinity;
  let landV = 0;
  let winAge = Infinity;
  let dust: Dust[] = [];

  function spawnDust(impact: number) {
    const n = Math.round(clamp(impact / LAND_FULL, 0.15, 1) * 16);
    for (let k = 0; k < n; k++) {
      const side = k % 2 === 0 ? -1 : 1;
      const r = (k * 0.618) % 1; // cheap deterministic spread, no RNG needed
      dust.push({
        u: 0.5 + side * (0.12 + 0.3 * r),
        w: 0.01 + 0.03 * r,
        du: side * (0.25 + 0.55 * r) * clamp(impact / LAND_FULL, 0.3, 1.4),
        dw: (0.35 + 0.5 * r) * clamp(impact / LAND_FULL, 0.3, 1.4),
        age: 0,
        life: 0.5 + 0.5 * r,
      });
    }
    if (dust.length > 80) dust = dust.slice(dust.length - 80);
  }

  return {
    onMessage(next) {
      if (m && !m.win && next.win) winAge = 0;
      if (!m) winAge = next.win ? 0 : Infinity;
      if (next.impact > 0) {
        landAge = 0;
        landV = next.impact;
        spawnDust(next.impact);
      }
      m = next;
    },
    advance(dtSec) {
      if (!m) return;
      // Drum spin: omega = vel / r. Visual only — see DRUM_R.
      spin = (spin + (m.vel / DRUM_R) * dtSec) % (Math.PI * 2);
      if (landAge !== Infinity) landAge += dtSec;
      if (winAge !== Infinity) winAge += dtSec;
      for (const d of dust) {
        d.age += dtSec;
        d.u += d.du * dtSec;
        d.w += d.dw * dtSec;
        d.dw -= 1.2 * dtSec; // settle back down
        d.du *= 1 - Math.min(1, 2.5 * dtSec);
      }
      if (dust.length > 0) dust = dust.filter((d) => d.age < d.life);
    },
    frame(at: MachineFrame): HoistState {
      return { m: at.m, now: at.now, spin, landAge, landV, winAge, dust, stale: at.stale };
    },
  };
}

/** The hoist, as the registry sees it. */
export const HOIST: MachineDef<HoistState> = {
  spec: HOIST_CHIP,
  create: createHoistAnim,
};
