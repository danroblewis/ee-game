// MACHINE-AS-CHIP — the generic package presentation for a machine.
//
// A machine is drawn the way every other multi-pin part is drawn: a
// rectangular package with its terminals on legs OUTSIDE the body, exactly
// the 555's grammar (render.ts's Timer555 case). What makes a machine
// different is only what goes INSIDE the body — the live physics, and the
// internal leads that tie each moving thing to the leg the player wires to.
//
// The one decision everything else follows from: the server's footprint rect
// is the machine's CELL on the grid, not its body. The server puts the pin
// columns inside the rect and the body is inset a leg-length further in from
// wherever they landed, so the legs point INWARD from pin to body wall. Every
// pin therefore still lies inside the rect the server broadcasts — which is
// why the server's "every fixture pin is inside HOIST_RECT" invariant, the
// persistence, the move validation and the undo machinery all survived this
// change untouched.
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
// `ChipSpec`: a pin table (which locked element+pin each leg belongs to and
// its ≤5-character name), a title/status/plate, and an `interior()` that
// draws the mechanism into a `ChipFrame` of body-local grid coordinates.
// `renderChip` owns everything else — geometry, legs in solver colour, the
// body, the two bands, the ⓘ badge, pin labels, the LOD form, current dots
// and the damage overlay. See machines/hoist.ts for the first one and
// machines/index.ts for the registry.
//
// GEOMETRY IS MEASURED, NOT DECLARED. A ChipSpec used to declare its footprint
// `size` and the inset of its pin columns, with a comment saying they had to
// match the server's. They did not have to: nothing checked, and a spec whose
// size was two units out drew its right-hand legs two units off the real
// terminals — wires and legs silently stopped meeting. So the package no
// longer declares any of it. The footprint comes from the rect the server
// broadcast and every leg is placed at the child element's OWN pin, read out
// of the document and expressed in footprint-local units (`chipLegs`). Body,
// rails, bands and label lanes are all derived from those legs. A machine
// therefore CANNOT put a leg anywhere but on its terminal, and the second
// machine's first bug is a bug that no longer exists.

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
// declares neither a footprint nor a body box. The only inputs are the rect
// the server broadcast and where the child elements' pins actually are.

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

/** One leg of the package, as DECLARED: which terminal it is and what it is
 * called. Deliberately no coordinates — see "geometry is measured" above. */
export interface ChipPin {
  /** Which locked element and pin this leg actually belongs to. The leg's
   * colour, its current dots, its POSITION and its damage all come from that
   * element — which is why a nine-pin machine needs no change to MAX_PINS:
   * the chip is a VIEW over four ordinary elements. */
  ref: [elemId: number, pin: number];
  /** ≤ 5 characters: the label lane between the bond rail and the wall is
   * exactly one grid unit wide. */
  label: string;
}

/** A leg RESOLVED against the document: the declaration plus where the
 * terminal actually is, in footprint-local grid units.
 *
 * This is the only place a leg's position comes from, and it comes from the
 * element the player wires to. A machine that "moved" a pin without the
 * server moving it would not draw a leg in the wrong place — there is no
 * other place for it to draw one. */
export interface ChipLeg {
  /** Index into `spec.pins`: the machine's own stable numbering, which is what
   * `interior()`'s `lead(k, …)` and `pinout`'s measurements are keyed by. */
  k: number;
  ref: [elemId: number, pin: number];
  label: string;
  /** Which pin column, from which half of the footprint the terminal is in. */
  side: 'L' | 'R';
  /** The terminal itself, footprint-local. */
  col: number;
  row: number;
}

/** The legs of a package standing on `rect`, in declaration order.
 *
 * A pin whose element (or whose pin index) is not in the document yields no
 * leg: the package must never show a terminal that nothing is attached to,
 * and a document that has not finished arriving is exactly that case. */
export function chipLegs<S>(
  spec: ChipSpec<S>,
  rect: [number, number, number, number],
  children: Map<number, ElementSpec>,
): ChipLeg[] {
  const mid = (rect[0] + rect[2]) / 2;
  const legs: ChipLeg[] = [];
  spec.pins.forEach((p, k) => {
    const at = children.get(p.ref[0])?.pins[p.ref[1]];
    if (!at) return;
    // The server's own invariant is that every fixture pin is inside the rect
    // it broadcast. If that is ever false the package would draw a leg outside
    // its own cell, so say so once and loudly rather than render it quietly.
    if (at[0] < rect[0] || at[0] > rect[2] || at[1] < rect[1] || at[1] > rect[3]) {
      warnOnce(
        `${spec.kind}: pin ${p.label} (element ${p.ref[0]}.${p.ref[1]}) is at ` +
          `${at[0]},${at[1]}, outside the machine's footprint ${rect.join(',')}`,
      );
    }
    legs.push({
      k,
      ref: p.ref,
      label: p.label,
      side: at[0] <= mid ? 'L' : 'R',
      col: at[0] - rect[0],
      row: at[1] - rect[1],
    });
  });
  return legs;
}

/** A geometry complaint is a bug in the machine, not in the frame: say it once
 * per distinct message instead of 60 times a second. */
const WARNED = new Set<string>();
function warnOnce(msg: string) {
  if (WARNED.has(msg)) return;
  WARNED.add(msg);
  console.error(`[chip] ${msg}`);
}

/** What the solver says about a leg this frame, keyed by the machine's own
 * pin index. `null` where there is no element or no frame yet: a machine must
 * print "—" there rather than a zero it did not measure. */
export interface ChipMeas {
  v(k: number): number | null;
  i(k: number): number | null;
}

/** Measurements for one package, from the legs and the live frame. */
export function chipMeas(legs: ChipLeg[], live: Map<number, ElemLive>): ChipMeas {
  const read = (k: number, f: (l: ElemLive, pin: number) => number | undefined) => {
    const leg = legs.find((l) => l.k === k);
    if (!leg) return null;
    const l = live.get(leg.ref[0]);
    if (!l) return null;
    const v = f(l, leg.ref[1]);
    return v === undefined || !Number.isFinite(v) ? null : v;
  };
  return {
    v: (k) => read(k, (l, p) => l.v[p]),
    i: (k) => read(k, (l, p) => l.i[p]),
  };
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
  /** The legs, in the order they are numbered. Nothing here says WHERE they
   * are: `chipLegs` reads that off the document. */
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
  /** The ⓘ card's pinout table, in pin order.
   *
   * `meas` is this frame's SOLVER reading for each leg, keyed by pin index.
   * A datasheet may of course print nameplate constants (a resistance, a
   * rating) — but where a row states what a terminal is doing right now, that
   * number comes from here and not from a nominal the machine computed. */
  pinout(s: S, meas: ChipMeas): PinoutRow[];
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
  body: { u0: number; u1: number; v0: number; v1: number };
  rail: { l: number; r: number };
  inner: { u0: number; u1: number; v0: number; v1: number };
  /** Centre of the ⓘ badge. */
  badge: [number, number];
}

/** Smallest body a package is drawn with, in grid units: two units across
 * leaves the two bond rails distinct, and three down leaves one unit of die
 * between the title and plate bands. Only a degenerate machine (every pin in
 * one column, or all on one row) can reach these. */
const MIN_BODY_W = 2;
const MIN_BODY_H = 3;

/** Everything about the package's shape, from where its terminals ACTUALLY
 * are. Null when the document has no terminals to stand it on — a package
 * with no legs is not a package, and drawing an empty body over the grid
 * would be inventing a machine that is not in the document yet. */
export function chipGeom(legs: ChipLeg[]): ChipGeom | null {
  if (legs.length === 0) return null;
  let colL = Infinity;
  let colR = -Infinity;
  let lo = Infinity;
  let hi = -Infinity;
  for (const l of legs) {
    if (l.col < colL) colL = l.col;
    if (l.col > colR) colR = l.col;
    if (l.row < lo) lo = l.row;
    if (l.row > hi) hi = l.row;
  }
  // The legs point INWARD from the pin columns, so the body wall is one leg
  // length inside each column and the overhang tops and tails it.
  const cu = (colL + colR) / 2;
  const halfW = Math.max((colR - colL) / 2 - STUB, MIN_BODY_W / 2);
  const cv = (lo + hi) / 2;
  const halfH = Math.max((hi - lo) / 2 + OVERHANG, MIN_BODY_H / 2);
  const u0 = cu - halfW;
  const u1 = cu + halfW;
  const v0 = cv - halfH;
  const v1 = cv + halfH;
  return {
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
 *
 * Takes the same resolved legs the renderer drew with, so the box you can
 * grab is the box you can see, by construction.
 */
export function chipZoneAt(
  cam: Camera,
  legs: ChipLeg[],
  rect: [number, number, number, number],
  x: number,
  y: number,
): 'body' | 'info' | null {
  const g = chipGeom(legs);
  if (!g) return null;
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
  /** The machine's locked child elements, by id. This is where the package's
   * whole geometry comes from (`chipLegs`), and a leg whose element is not in
   * the document is not drawn: the package must never show a terminal that
   * nothing is actually attached to — and with no legs at all there is no
   * package to draw yet. */
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

/** Draw a machine as a package. Returns silently if the footprint is off
 * screen or degenerate — a machine at the far zoom is still cheap. */
export function renderChip<S>(d: ChipDraw<S>): void {
  const { ctx, cam, spec, rect, state } = d;
  const s = cam.scale;
  const legs = chipLegs(spec, rect, d.children);
  const g = chipGeom(legs);
  if (!g) return;
  const ox = cam.ox + rect[0] * s;
  const oy = cam.oy + rect[1] * s;
  const W = (rect[2] - rect[0]) * s;
  const H = (rect[3] - rect[1]) * s;
  if (!Number.isFinite(W) || !Number.isFinite(H) || !(W > 2 && H > 2)) return;
  if (ox + W < 0 || oy + H < 0 || ox > window.innerWidth || oy > window.innerHeight) return;

  const tier = tierFor(s);
  const at = (u: number, v: number): Px => [ox + u * s, oy + v * s];
  const f = makeFrame(d, g, legs, at, tier);

  if (tier === 'lod') {
    drawLod(d, g, at, f, legs);
    return;
  }

  const volts = (p: ChipLeg) => d.live.get(p.ref[0])?.v[p.ref[1]] ?? 0;

  // ---- legs, one per pin, each in its OWN solver colour and running from the
  // terminal in to the body wall. A part is not at one potential, so the
  // colour belongs to the lead and never to the package (render.ts:1126).
  ctx.lineWidth = Math.max(2, s * 0.07);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  for (const p of legs) {
    const wall = p.side === 'L' ? g.body.u0 : g.body.u1;
    ctx.strokeStyle = voltageColor(volts(p));
    ctx.beginPath();
    ctx.moveTo(...at(p.col, p.row));
    ctx.lineTo(...at(wall, p.row));
    ctx.stroke();
  }

  // ---- body, painted over any leg overshoot.
  const anyBroken = legs.some((p) => d.damage.get(p.ref[0])?.broken);
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
    drawLabels(d, g, at, legs);
  }

  // ---- current dots on every leg, from that leg's own element frame. Each
  // leg reads its OWN element, which is exactly why a nine-pin machine needs
  // no change to MAX_PINS.
  for (const p of legs) {
    const i = d.live.get(p.ref[0])?.i[p.ref[1]] ?? 0;
    if (i === 0) continue;
    const wall = p.side === 'L' ? g.body.u0 : g.body.u1;
    const phase = d.dots.advance(dotKey(p.ref[0], p.ref[1]), i, d.dtSec);
    drawDots(ctx, cam, at(p.col, p.row), at(wall, p.row), phase, i);
  }

  // ---- damage: the package owns its children's glyphs now, so it owns
  // saying they are cooking or dead.
  drawHeat(d, g, at, legs);
}

function makeFrame<S>(
  d: ChipDraw<S>,
  g: ChipGeom,
  legs: ChipLeg[],
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
      const p = legs.find((l) => l.k === k);
      if (!p || pts.length < 1) return;
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
  pins: ChipLeg[],
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
  pins: ChipLeg[],
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
  pins: ChipLeg[],
) {
  const { ctx, cam, spec } = d;
  const s = cam.scale;
  const cu = (g.body.u0 + g.body.u1) / 2;
  const cv = (g.body.v0 + g.body.v1) / 2;
  ctx.lineWidth = Math.max(1, s * 0.12);
  ctx.lineCap = 'butt';
  for (const p of pins) {
    const [x, y] = at(p.col, p.row);
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
