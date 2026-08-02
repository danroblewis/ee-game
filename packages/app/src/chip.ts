// MACHINE-AS-CHIP — the generic package presentation for a machine.
//
// A machine is drawn the way every other multi-pin part is drawn: a
// rectangular package with its terminals on legs OUTSIDE the body, exactly
// the 555's grammar (render.ts's Timer555 case). What makes a machine
// different is only what goes INSIDE the body — the live physics, and the
// internal leads that tie each moving thing to the leg the player wires to.
//
// The one decision everything else follows from: the server's footprint rect
// is the machine's CELL on the grid, not its body. The pin columns sit one
// unit inside the rect and the body is inset a leg-length further, so the
// legs point INWARD from pin to body wall. Every pin therefore still lies
// inside the rect the server broadcasts — which is why the server's "every
// fixture pin is inside HOIST_RECT" invariant, the persistence, the move
// validation and the undo machinery all survived this change untouched.
//
//   ┌──────────────────────────────────────────┐  the footprint: the server's
//   │  ●──┤ FREIGHT HOIST                 ⓘ ├──●  │  rect, the machine's cell
//   │  ●──┤ ▓▓ the live physics, and the   ├──●  │
//   │     │ ▓▓ internal leads that carry   ├──●  │  ● a pin. A player's wire
//   │     │ ▓▓ each moving thing out to    ├──●  │    attaches HERE, one unit
//   │     │    the leg it belongs to       ├──●  │    inside the footprint,
//   │     │ 3.0 A MAX · STALL = V/R        ├──●  │    with its leg running IN
//   │     └───────────────────────────────┘     │    to the body wall.
//   └──────────────────────────────────────────┘
//
// SEAM. Nothing in this file knows what a hoist is. A machine supplies a
// `ChipSpec`: a pin table (which locked element+pin each leg belongs to, its
// side, its row and its ≤5-character name), a title/status/plate, and an
// `interior()` that draws the mechanism into a `ChipFrame` of body-local grid
// coordinates. `renderChip` owns everything else — geometry, legs in solver
// colour, the body, the two bands, the ⓘ badge, pin labels, the LOD form,
// current dots and the damage overlay. See machines/hoist.ts for the first
// one and machines/index.ts for the registry.

import {
  drawDots,
  LOD_FULL,
  STRESS_WARM,
  voltageColor,
  type Camera,
  type DamageState,
  type DotFlow,
} from './render';
import type { ElemLive, ElementSpec } from './circuit';

export type Px = [number, number];

/** How much of the package is worth drawing at this zoom.
 *
 * The tiers deliberately reuse the thresholds render.ts already uses for the
 * 555 (24 px/unit for text, 40 for the innards hint) plus the shared
 * `LOD_FULL`, so a machine and the schematic around it never straddle a
 * threshold differently.
 *
 *   'full'  everything: numbers, captions, particles, the goal legend
 *   'text'  body, legs, bond leads, labels, both bands, mechanism shapes
 *   'shape' body, legs, mechanism SHAPES only — no text, no hair-thin leads
 *   'lod'   one neutral package rect, one coloured stub per pin, plus
 *           whatever state cue the machine can carry on shape alone
 *
 * The 'shape' tier is the one that matters. A 555 is a mute rectangle for a
 * whole zoom octave because everything it says is text; a machine cannot
 * afford that, because its entire reason to exist is a state the player is
 * trying to hold. Carrying that state on SHAPE keeps the package readable
 * from LOD_FULL up and drops only the words. */
export type Tier = 'lod' | 'shape' | 'text' | 'full';

/** px per grid unit at which package text appears (render.ts:604's gate). */
export const TIER_TEXT = 24;
/** px per grid unit at which the fine detail appears (render.ts:577's gate). */
export const TIER_FULL = 40;

export function tierFor(scale: number): Tier {
  if (scale < LOD_FULL) return 'lod';
  if (scale < TIER_TEXT) return 'shape';
  if (scale < TIER_FULL) return 'text';
  return 'full';
}

const RANK: Record<Tier, number> = { lod: 0, shape: 1, text: 2, full: 3 };
export const atLeast = (t: Tier, min: Tier) => RANK[t] >= RANK[min];

// ------------------------------------------------------------------ layout
//
// All of these are grid units, and all of them are derived — a machine
// declares its footprint and its pin rows, never a body box.

/** Leg length: the 555's `min(0.9, w * 0.25)`, and the reason the pin handle
 * stands clear of the package instead of being a pad printed on its face. */
const STUB = 0.9;
/** How far the body overhangs the outermost pin row, top and bottom.
 *
 * The 555 uses 0.5 because an ordinary part's selection halo is derived from
 * its pin bounding box. A machine's halo is derived from the footprint, which
 * already carries a full unit of margin outside the body, so a taller
 * overhang is free — and the two 1.0-tall bands it buys are the package's
 * identity (title) and its datasheet (plate), without stealing interior. */
const OVERHANG = 1;
/** Distance from the body wall in to the bond rail: the lane the internal
 * leads land on. Leaves a clear 1.0-unit lane for the pin label, which makes
 * "a label never collides with an internal" structural rather than tuned. */
const RAIL_INSET = 1;
/** Pin label inset from the body wall. */
const LABEL_INSET = 0.16;

const BODY_FILL = '#181820';
const BODY_LINE = '#c9c9d4';
const BODY_LINE_HOT = '#e6e6f0';
const LABEL_COLOR = '#8a8a98';
const BAND_LINE = '#2b2b36';
/** Package bodies carry no single potential (render.ts:1126) — neutral. */
const LOD_BODY = '#70727f';
const BROKEN_MARK = '#ff6a3d';

/** One leg of the package. */
export interface ChipPin {
  /** Which locked element and pin this leg actually belongs to. The leg's
   * colour, its current dots and its damage all come from that element's own
   * solver frame — which is why a nine-pin machine needs no change to
   * MAX_PINS: the chip is a VIEW over four ordinary elements. */
  ref: [elemId: number, pin: number];
  side: 'L' | 'R';
  /** Row in footprint-local grid units (integer, so wires run on-grid). */
  row: number;
  /** ≤ 5 characters: the label lane between the bond rail and the wall is
   * exactly one grid unit wide. */
  label: string;
}

/** A row of the ⓘ card's pinout table. */
export type PinoutRow = [name: string, what: string, number: string];

/** Everything a machine has to say about itself as a package. `S` is the
 * machine's own live state (its server message plus whatever animation state
 * the client integrates from it). */
export interface ChipSpec<S> {
  /** Matches the `kind` field of the machine message. */
  kind: string;
  /** Engraved on the title band. */
  title: string;
  /** Footprint in grid units. MUST equal the server's own W/H. */
  size: [w: number, h: number];
  /** Footprint edge -> pin column (x) and -> first usable row (y). */
  margin: { x: number; y: number };
  pins: ChipPin[];
  /** Title band, right-hand side: [colour, text]. */
  status(s: S): [string, string];
  /** Plate band, one line each: [colour, text]. */
  plate(s: S): [string, string][];
  /** The live physics, in body-local grid coordinates. */
  interior(f: ChipFrame, s: S): void;
  /** State cues legible below LOD_FULL, in the same local frame. Optional:
   * a machine with nothing shape-shaped to say can leave it out. */
  lod?(f: ChipFrame, s: S): void;
  /** Where a child device actually sits inside the die, in local units.
   * Optional: it only moves the heat/broken mark onto the thing that is
   * cooking. Without it the mark lands on that device's bond-rail lane,
   * which is still ITS lane — so a second machine may skip this entirely. */
  deviceAt?(elemId: number): [number, number] | null;
  /** The ⓘ card's pinout table, in pin order. */
  pinout(s: S): PinoutRow[];
}

/** Body-local drawing surface handed to `interior()`.
 *
 * Coordinates are FOOTPRINT-local grid units (u across, v down) — the same
 * frame the pin rows are written in, so a lead drawn to row 8 lands on the
 * leg named at row 8 with no conversion. Widths and font sizes are in grid
 * units too, so the whole interior scales with the package exactly like the
 * 555's innards hint does.
 *
 * Nothing a machine draws touches a screen axis: every coordinate, width and
 * font size goes through this frame. `move_machine` only ever TRANSLATES an
 * axis-aligned rect, so `at` is a translation and a scale today — but because
 * the local->px map is the single chokepoint, a future `MachineRotate` op is
 * one edit here and none in any machine file. */
export interface ChipFrame {
  ctx: CanvasRenderingContext2D;
  cam: Camera;
  tier: Tier;
  /** px per grid unit. */
  s: number;
  /** Local (u, v) -> px. */
  at(u: number, v: number): Px;
  /** Polyline in local units; `w` is a grid-unit line width. */
  line(pts: [number, number][], color: string, w: number, alpha?: number): void;
  /** Same, dashed: reserved for MECHANICAL linkages (a shaft, a slider rod).
   * They are the only things allowed to cross an electrical lead, and the
   * dash is how a player tells "this is a rod" from "this is a wire". */
  dash(pts: [number, number][], color: string, w: number): void;
  box(u: number, v: number, w: number, h: number, fill?: string, stroke?: string, lw?: number): void;
  disc(u: number, v: number, r: number, fill?: string, stroke?: string, lw?: number): void;
  text(
    t: string,
    u: number,
    v: number,
    size: number,
    color: string,
    align?: CanvasTextAlign,
    baseline?: CanvasTextBaseline,
  ): void;
  /** An internal bond lead from a device out to pin `k`'s rail pad.
   *
   * This is the visible answer to "how does the physics relate to the pins":
   * the lead is drawn in the SOLVER's colour for that pin, dimmed because it
   * is under the epoxy, so the colour runs continuously from the player's
   * wire, through the leg, through the wall, to the thing it actually lands
   * on. `pts` is the Manhattan route from the device; the pad on the rail is
   * added here. */
  lead(k: number, pts: [number, number][]): void;
  /** Body box in local units. */
  body: { u0: number; u1: number; v0: number; v1: number };
  /** The two bond rails: where internal leads terminate. */
  rail: { l: number; r: number };
  /** The interior box (body minus the title and plate bands). */
  inner: { u0: number; u1: number; v0: number; v1: number };
}

/** Derived package geometry, in footprint-local grid units. */
export interface ChipGeom {
  w: number;
  h: number;
  colL: number;
  colR: number;
  body: { u0: number; u1: number; v0: number; v1: number };
  rail: { l: number; r: number };
  inner: { u0: number; u1: number; v0: number; v1: number };
  /** Centre of the ⓘ badge. */
  badge: [number, number];
}

/** Everything about the package's shape, from its declaration alone. */
export function chipGeom(spec: ChipSpec<unknown>): ChipGeom {
  const [w, h] = spec.size;
  const colL = spec.margin.x;
  const colR = w - spec.margin.x;
  const u0 = colL + STUB;
  const u1 = colR - STUB;
  let lo = Infinity;
  let hi = -Infinity;
  for (const p of spec.pins) {
    if (p.row < lo) lo = p.row;
    if (p.row > hi) hi = p.row;
  }
  const v0 = lo - OVERHANG;
  const v1 = hi + OVERHANG;
  return {
    w,
    h,
    colL,
    colR,
    body: { u0, u1, v0, v1 },
    rail: { l: u0 + RAIL_INSET, r: u1 - RAIL_INSET },
    // The title and plate bands are one unit each, and they are pin-free by
    // construction: the overhang is exactly what creates them.
    inner: { u0, u1, v0: v0 + 1, v1: v1 - 1 },
    badge: [u1 - 0.55, v0 + 0.5],
  };
}

/** Hit radius of the ⓘ badge, px. */
const BADGE_R = (scale: number) => Math.max(8, scale * 0.3);

/**
 * What a pointer is over:
 *   'info' — the ⓘ badge, and ONLY when it is actually painted. Same
 *            discipline as panel.ts's tab: never hit-test a glyph you did
 *            not draw, or an invisible button eats clicks when zoomed out.
 *   'body' — the package face. Note this is the BODY box, not the footprint:
 *            the leg corridors are not machine zones by construction, so the
 *            package can never swallow a click aimed at a terminal.
 * Callers must still hit-test pins and child elements first.
 */
export function chipZoneAt(
  cam: Camera,
  spec: ChipSpec<unknown>,
  rect: [number, number, number, number],
  x: number,
  y: number,
): 'body' | 'info' | null {
  const g = chipGeom(spec);
  const s = cam.scale;
  const ox = cam.ox + rect[0] * s;
  const oy = cam.oy + rect[1] * s;
  if (s >= TIER_TEXT) {
    const bx = ox + g.badge[0] * s;
    const by = oy + g.badge[1] * s;
    if (Math.hypot(x - bx, y - by) <= BADGE_R(s)) return 'info';
  }
  const { u0, u1, v0, v1 } = g.body;
  if (x < ox + u0 * s || x > ox + u1 * s) return null;
  if (y < oy + v0 * s || y > oy + v1 * s) return null;
  return 'body';
}

// ------------------------------------------------------------------ render

export interface ChipDraw<S> {
  ctx: CanvasRenderingContext2D;
  cam: Camera;
  spec: ChipSpec<S>;
  /** The footprint in grid units: the server's, or an optimistic drag's. */
  rect: [number, number, number, number];
  state: S;
  /** The machine's locked child elements, by id. A leg whose element is not
   * in the document is not drawn: the package must never show a terminal
   * that nothing is actually attached to. */
  children: Map<number, ElementSpec>;
  live: Map<number, ElemLive>;
  damage: Map<number, DamageState>;
  dots: DotFlow;
  dtSec: number;
  /** Pointer is over the body (or a drag is in progress). */
  hot: boolean;
}

/** Dot phase key for one chip leg.
 *
 * Provably disjoint from everything else that keys a DotFlow: fixture ids are
 * 900..999, so this lands in [14400, 15999]; player element ids start at 1e6
 * and the capacitor's second-lead convention is `id + 1_000_000` (>= 2e6). */
const dotKey = (id: number, pin: number) => id * 16 + pin;

/** The legs that actually have an element behind them. A machine's fixtures
 * are injected by the server and can never be removed, so this is normally
 * every pin — but a document that has not finished arriving (or a legacy
 * save) must not grow a terminal with nothing attached to it. */
const bonded = <S,>(d: ChipDraw<S>): ChipPin[] =>
  d.spec.pins.filter((p) => d.children.has(p.ref[0]));

/** Draw a machine as a package. Returns silently if the footprint is off
 * screen or degenerate — a machine at the far zoom is still cheap. */
export function renderChip<S>(d: ChipDraw<S>): void {
  const { ctx, cam, spec, rect, state } = d;
  const s = cam.scale;
  const g = chipGeom(spec);
  const ox = cam.ox + rect[0] * s;
  const oy = cam.oy + rect[1] * s;
  const W = (rect[2] - rect[0]) * s;
  const H = (rect[3] - rect[1]) * s;
  if (!Number.isFinite(W) || !Number.isFinite(H) || !(W > 2 && H > 2)) return;
  if (ox + W < 0 || oy + H < 0 || ox > window.innerWidth || oy > window.innerHeight) return;

  const tier = tierFor(s);
  const at = (u: number, v: number): Px => [ox + u * s, oy + v * s];
  const f = makeFrame(d, g, at, tier);
  const pins = bonded(d);

  if (tier === 'lod') {
    drawLod(d, g, at, f, pins);
    return;
  }

  const volts = (p: ChipPin) => d.live.get(p.ref[0])?.v[p.ref[1]] ?? 0;

  // ---- legs, one per pin, each in its OWN solver colour and pointing in
  // from the pin to the body wall. A part is not at one potential, so the
  // colour belongs to the lead and never to the package (render.ts:1126).
  ctx.lineWidth = Math.max(2, s * 0.07);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  for (const p of pins) {
    const wall = p.side === 'L' ? g.body.u0 : g.body.u1;
    const col = p.side === 'L' ? g.colL : g.colR;
    ctx.strokeStyle = voltageColor(volts(p));
    ctx.beginPath();
    ctx.moveTo(...at(col, p.row));
    ctx.lineTo(...at(wall, p.row));
    ctx.stroke();
  }

  // ---- body, painted over any leg overshoot.
  const anyBroken = pins.some((p) => d.damage.get(p.ref[0])?.broken);
  ctx.fillStyle = BODY_FILL;
  ctx.strokeStyle = anyBroken ? BROKEN_MARK : d.hot ? BODY_LINE_HOT : BODY_LINE;
  ctx.beginPath();
  ctx.moveTo(...at(g.body.u0, g.body.v0));
  ctx.lineTo(...at(g.body.u1, g.body.v0));
  ctx.lineTo(...at(g.body.u1, g.body.v1));
  ctx.lineTo(...at(g.body.u0, g.body.v1));
  ctx.closePath();
  ctx.fill();
  ctx.stroke();

  // ---- interior: the live physics, clipped to the die area so a machine
  // can never paint over its own labels or outside its package.
  ctx.save();
  ctx.beginPath();
  ctx.rect(
    ox + g.inner.u0 * s,
    oy + g.inner.v0 * s,
    (g.inner.u1 - g.inner.u0) * s,
    (g.inner.v1 - g.inner.v0) * s,
  );
  ctx.clip();
  spec.interior(f, state);
  ctx.restore();

  if (atLeast(tier, 'text')) {
    drawBands(d, g, f, anyBroken);
    drawLabels(d, g, at, pins);
  }

  // ---- current dots on every leg, from that leg's own element frame. Each
  // leg reads its OWN element, which is exactly why a nine-pin machine needs
  // no change to MAX_PINS.
  for (const p of pins) {
    const i = d.live.get(p.ref[0])?.i[p.ref[1]] ?? 0;
    if (i === 0) continue;
    const wall = p.side === 'L' ? g.body.u0 : g.body.u1;
    const col = p.side === 'L' ? g.colL : g.colR;
    const phase = d.dots.advance(dotKey(p.ref[0], p.ref[1]), i, d.dtSec);
    drawDots(ctx, cam, at(col, p.row), at(wall, p.row), phase, i);
  }

  // ---- damage: the package owns its children's glyphs now, so it owns
  // saying they are cooking or dead.
  drawHeat(d, g, at, pins);
}

function makeFrame<S>(
  d: ChipDraw<S>,
  g: ChipGeom,
  at: (u: number, v: number) => Px,
  tier: Tier,
): ChipFrame {
  const { ctx, cam } = d;
  const s = cam.scale;
  const path = (pts: [number, number][]) => {
    ctx.beginPath();
    const p0 = at(pts[0]![0], pts[0]![1]);
    ctx.moveTo(p0[0], p0[1]);
    for (let k = 1; k < pts.length; k++) {
      const p = at(pts[k]![0], pts[k]![1]);
      ctx.lineTo(p[0], p[1]);
    }
  };
  return {
    ctx,
    cam,
    tier,
    s,
    at,
    body: g.body,
    rail: g.rail,
    inner: g.inner,
    line(pts, color, w, alpha) {
      if (pts.length < 2) return;
      ctx.save();
      if (alpha !== undefined) ctx.globalAlpha = alpha;
      ctx.lineWidth = Math.max(0.6, w * s);
      ctx.strokeStyle = color;
      path(pts);
      ctx.stroke();
      ctx.restore();
    },
    dash(pts, color, w) {
      if (pts.length < 2) return;
      ctx.save();
      ctx.setLineDash([0.14 * s, 0.1 * s]);
      ctx.lineWidth = Math.max(0.6, w * s);
      ctx.strokeStyle = color;
      path(pts);
      ctx.stroke();
      ctx.restore();
    },
    box(u, v, w, h, fill, stroke, lw) {
      const [x, y] = at(u, v);
      if (fill) {
        ctx.fillStyle = fill;
        ctx.fillRect(x, y, w * s, h * s);
      }
      if (stroke) {
        ctx.strokeStyle = stroke;
        ctx.lineWidth = Math.max(0.6, (lw ?? 0.04) * s);
        ctx.strokeRect(x, y, w * s, h * s);
      }
    },
    disc(u, v, r, fill, stroke, lw) {
      const [x, y] = at(u, v);
      ctx.beginPath();
      ctx.arc(x, y, Math.max(0.5, r * s), 0, Math.PI * 2);
      if (fill) {
        ctx.fillStyle = fill;
        ctx.fill();
      }
      if (stroke) {
        ctx.strokeStyle = stroke;
        ctx.lineWidth = Math.max(0.6, (lw ?? 0.04) * s);
        ctx.stroke();
      }
    },
    text(t, u, v, size, color, align = 'left', baseline = 'middle') {
      const px = size * s;
      if (px < 5) return; // sub-legible text is noise, not information
      ctx.font = `${px.toFixed(1)}px ui-monospace, monospace`;
      ctx.fillStyle = color;
      ctx.textAlign = align;
      ctx.textBaseline = baseline;
      const [x, y] = at(u, v);
      ctx.fillText(t, x, y);
      ctx.textAlign = 'start';
      ctx.textBaseline = 'alphabetic';
    },
    lead(k, pts) {
      // Indexed into the FULL pin table, so a machine's `interior()` can
      // name its leads by declaration order and never see them renumber.
      const p = d.spec.pins[k];
      if (!p || pts.length < 1 || !d.children.has(p.ref[0])) return;
      const rail = p.side === 'L' ? g.rail.l : g.rail.r;
      const full: [number, number][] = [...pts, [rail, p.row]];
      const v = d.live.get(p.ref[0])?.v[p.ref[1]] ?? 0;
      const color = voltageColor(v);
      ctx.save();
      ctx.globalAlpha = 0.55;
      ctx.lineWidth = Math.max(0.6, 0.045 * s);
      ctx.lineJoin = 'miter';
      ctx.strokeStyle = color;
      path(full);
      ctx.stroke();
      // The pad: where the lead meets the rail, and the visual full stop
      // that says "this internal IS that leg".
      const [px_, py] = at(rail, p.row);
      ctx.globalAlpha = 0.85;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(px_, py, Math.max(1, 0.11 * s), 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    },
  };
}

function drawBands<S>(d: ChipDraw<S>, g: ChipGeom, f: ChipFrame, broken: boolean) {
  const { ctx, spec, state } = d;
  const { u0, u1, v0, v1 } = g.body;
  // Hairlines separating the two bands from the die.
  f.line(
    [
      [u0, v0 + 1],
      [u1, v0 + 1],
    ],
    BAND_LINE,
    0.03,
  );
  f.line(
    [
      [u0, v1 - 1],
      [u1, v1 - 1],
    ],
    BAND_LINE,
    0.03,
  );

  // ---- title band: identity on the left, live status on the right, ⓘ last.
  f.text(spec.title, u0 + 0.3, v0 + 0.52, 0.34, broken ? '#ff8f8f' : BODY_LINE);
  const [sc, st] = broken ? ['#ff8f8f', 'MOTOR BURNT OUT'] : spec.status(state);
  f.text(st, g.badge[0] - 0.45, v0 + 0.54, 0.22, sc, 'right');

  // ---- the ⓘ badge. Drawn and hit-tested under the same condition.
  const [bx, by] = f.at(g.badge[0], g.badge[1]);
  const r = Math.max(4, f.s * 0.22);
  ctx.beginPath();
  ctx.arc(bx, by, r, 0, Math.PI * 2);
  ctx.fillStyle = d.hot ? '#2b3444' : '#20262f';
  ctx.fill();
  ctx.lineWidth = Math.max(1, f.s * 0.025);
  ctx.strokeStyle = '#6d7d89';
  ctx.stroke();
  f.text('i', g.badge[0], g.badge[1] + 0.02, 0.28, '#9fb4c4', 'center');

  // ---- plate band: the printed constants a player designs against.
  const lines = spec.plate(state);
  const size = Math.min(0.2, 0.86 / Math.max(1, lines.length));
  let v = v1 - 1 + (1 - lines.length * size * 1.25) / 2 + size * 0.6;
  for (const [color, text] of lines) {
    f.text(text, u0 + 0.3, v, size, color);
    v += size * 1.25;
  }
}

function drawLabels<S>(
  d: ChipDraw<S>,
  g: ChipGeom,
  at: (u: number, v: number) => Px,
  pins: ChipPin[],
) {
  const { ctx, cam } = d;
  const s = cam.scale;
  // Names go INSIDE the body, inset from the wall, aligned away from it —
  // exactly the 555's split: legs and handles outside, names inside.
  ctx.font = `${Math.round(s * 0.2)}px ui-monospace, monospace`;
  ctx.textBaseline = 'middle';
  for (const p of pins) {
    const left = p.side === 'L';
    ctx.textAlign = left ? 'left' : 'right';
    // Dead legs go grey: a burnt part is an open circuit, and its label
    // should not keep advertising a live name.
    ctx.fillStyle = d.damage.get(p.ref[0])?.broken ? '#5b5b66' : LABEL_COLOR;
    const u = left ? g.body.u0 + LABEL_INSET : g.body.u1 - LABEL_INSET;
    const [tx, ty] = at(u, p.row);
    ctx.fillText(p.label, tx, ty);
  }
  ctx.textAlign = 'start';
  ctx.textBaseline = 'alphabetic';
}

/** Heat and death, per child device, over the package. */
function drawHeat<S>(
  d: ChipDraw<S>,
  g: ChipGeom,
  at: (u: number, v: number) => Px,
  pins: ChipPin[],
) {
  const { ctx, cam, spec } = d;
  const s = cam.scale;
  const seen = new Set<number>();
  for (const p of pins) {
    const id = p.ref[0];
    if (seen.has(id)) continue;
    const dm = d.damage.get(id);
    if (!dm) continue;
    seen.add(id);
    if (!dm.broken && dm.stress <= STRESS_WARM) continue;
    // Where the machine says this device is, or — for a machine that does not
    // say — its own cell: the mean of its legs' rows, just inside its bond
    // rail. That lane IS this device (every lead landing there belongs to it),
    // so even the fallback reads as "this device on this chip" without the
    // package knowing what the device is.
    const rows = pins.filter((q) => q.ref[0] === id);
    const left = rows[0]!.side === 'L';
    const where = spec.deviceAt?.(id) ?? [
      left ? g.rail.l + 0.6 : g.rail.r - 0.6,
      rows.reduce((a, q) => a + q.row, 0) / rows.length,
    ];
    const [x, y] = at(where[0], where[1]);
    if (!dm.broken) {
      const t = (dm.stress - STRESS_WARM) / (1 - STRESS_WARM);
      const rad = s * (0.7 + 0.5 * t);
      const grad = ctx.createRadialGradient(x, y, s * 0.1, x, y, rad);
      grad.addColorStop(0, `rgba(255,${Math.round(190 - 130 * t)},${Math.round(90 - 80 * t)},${(0.15 + 0.5 * t).toFixed(3)})`);
      grad.addColorStop(1, 'rgba(255,120,10,0)');
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.arc(x, y, rad, 0, Math.PI * 2);
      ctx.fill();
      continue;
    }
    const r = Math.max(4, s * 0.35);
    ctx.strokeStyle = BROKEN_MARK;
    ctx.lineWidth = Math.max(2, s * 0.06);
    ctx.beginPath();
    ctx.moveTo(x - r, y - r);
    ctx.lineTo(x + r, y + r);
    ctx.moveTo(x + r, y - r);
    ctx.lineTo(x - r, y + r);
    ctx.stroke();
  }
}

/** Below LOD_FULL: exactly what `drawElementsLod` gives any other package —
 * one neutral body rect and one coloured stub per pin, 55% of the way to the
 * centre — plus whatever the machine can still say with shape alone. */
function drawLod<S>(
  d: ChipDraw<S>,
  g: ChipGeom,
  at: (u: number, v: number) => Px,
  f: ChipFrame,
  pins: ChipPin[],
) {
  const { ctx, cam, spec } = d;
  const s = cam.scale;
  const cu = (g.body.u0 + g.body.u1) / 2;
  const cv = (g.body.v0 + g.body.v1) / 2;
  ctx.lineWidth = Math.max(1, s * 0.12);
  ctx.lineCap = 'butt';
  for (const p of pins) {
    const col = p.side === 'L' ? g.colL : g.colR;
    const [x, y] = at(col, p.row);
    const [cx, cy] = at(cu, cv);
    ctx.strokeStyle = voltageColor(d.live.get(p.ref[0])?.v[p.ref[1]] ?? 0);
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + (cx - x) * 0.55, y + (cy - y) * 0.55);
    ctx.stroke();
  }
  const [bx, by] = at(g.body.u0, g.body.v0);
  ctx.fillStyle = '#14141a';
  ctx.fillRect(bx, by, (g.body.u1 - g.body.u0) * s, (g.body.v1 - g.body.v0) * s);
  ctx.lineWidth = Math.max(1, s * 0.09);
  ctx.strokeStyle = LOD_BODY;
  ctx.strokeRect(bx, by, (g.body.u1 - g.body.u0) * s, (g.body.v1 - g.body.v0) * s);
  ctx.lineCap = 'round';
  // The state cue survives: "is the crate in the band" is a cross-the-room
  // legible fact even where the symbol is a smudge, for the same reason the
  // LOD pass keeps a broken part's cross.
  spec.lod?.(f, d.state);
  if (pins.some((p) => d.damage.get(p.ref[0])?.broken)) {
    const [cx, cy] = at(cu, cv);
    const r = Math.max(3, Math.min(7, s * 0.9));
    ctx.strokeStyle = BROKEN_MARK;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(cx - r, cy - r);
    ctx.lineTo(cx + r, cy + r);
    ctx.moveTo(cx + r, cy - r);
    ctx.lineTo(cx - r, cy + r);
    ctx.stroke();
  }
}
