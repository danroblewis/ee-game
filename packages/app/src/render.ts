// Falstad-feel Canvas2D renderer: voltage-colored conductors, animated
// current dots, proper schematic symbols for every device. (The WebGL
// canvas package arrives with the S2 spike; this is the visual reference.)

import { pinLabels } from './circuit';
import type { ElemLive, ElementSpec, Point } from './circuit';

export interface Camera {
  scale: number; // px per grid unit
  ox: number;
  oy: number;
}

const V_FULL = 10; // voltage at full color saturation

export function voltageColor(v: number): string {
  const t = Math.max(-1, Math.min(1, v / V_FULL));
  // gray at 0V -> green positive, red negative (Falstad convention)
  const base = 96;
  if (t >= 0) {
    const g = Math.round(base + (255 - base) * t);
    return `rgb(${Math.round(base * (1 - t))},${g},${Math.round(base * (1 - t))})`;
  }
  const r = Math.round(base + (255 - base) * -t);
  return `rgb(${r},${Math.round(base * (1 + t))},${Math.round(base * (1 + t))})`;
}

export const LED_COLORS = ['#ff4b3e', '#4bff6a', '#4b9dff', '#ffe14b', '#f2f2f2'];

type Px = [number, number];

const px = (cam: Camera, p: Point): Px => [cam.ox + p[0] * cam.scale, cam.oy + p[1] * cam.scale];

const lerp = (a: Px, b: Px, t: number): Px => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
const add = (a: Px, b: Px, s = 1): Px => [a[0] + b[0] * s, a[1] + b[1] * s];
const sub = (a: Px, b: Px): Px => [a[0] - b[0], a[1] - b[1]];
const mag = (a: Px) => Math.hypot(a[0], a[1]);
const norm = (a: Px): Px => {
  const m = mag(a) || 1;
  return [a[0] / m, a[1] / m];
};
const perp = (a: Px): Px => [-a[1], a[0]];

const DOT_SPACING = 0.55; // grid units between dots
/** Dots only exist where current flows (Falstad convention). 1 µA is the
 * floor of "a branch is conducting" — bias strings and op-amp feedback
 * networks live at tens of µA and must animate. */
const DOT_MIN_AMPS = 1e-6;
/** Dot speed limits, grid units/sec. The upper bound also keeps the dots
 * below the strobe threshold: DOT_SPACING/2 per frame at 60 fps is
 * 16.5 grid/sec, so 6 never aliases. */
const DOT_SPEED_MIN = 0.35;
const DOT_SPEED_MAX = 6;

/** Dot travel speed in grid units/sec for a branch current, log-compressed
 * over the ~7 decades a real circuit spans: 1 µA crawls at 0.6, 50 µA at
 * 1.45, 6 mA at 2.49, 100 mA at 3.1. Linear speed would make anything
 * below a milliamp look dead. Sign carries the direction. */
export function dotSpeed(current: number): number {
  const a = Math.abs(current);
  if (a < DOT_MIN_AMPS) return 0;
  const v = 0.6 + 0.5 * Math.log10(a / DOT_MIN_AMPS);
  return Math.sign(current) * Math.min(DOT_SPEED_MAX, Math.max(DOT_SPEED_MIN, v));
}

/** Animated dot phase per element, advanced by simulated current. */
export class DotFlow {
  private phase = new Map<number, number>();

  advance(id: number, current: number, dtSec: number): number {
    const p = (this.phase.get(id) ?? 0) + dotSpeed(current) * dtSec;
    this.phase.set(id, p);
    return p;
  }
}

export function drawDots(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  a: Px,
  b: Px,
  phase: number,
  current: number,
) {
  if (Math.abs(current) < DOT_MIN_AMPS) return;
  const len = mag(sub(b, a)) / cam.scale;
  if (len < 1e-6) return;
  ctx.fillStyle = '#ffe95e';
  let p = ((phase % DOT_SPACING) + DOT_SPACING) % DOT_SPACING;
  for (; p < len; p += DOT_SPACING) {
    const [x, y] = lerp(a, b, p / len);
    ctx.beginPath();
    ctx.arc(x, y, Math.max(2, cam.scale * 0.055), 0, Math.PI * 2);
    ctx.fill();
  }
}

/** Dots along a multi-segment lead, phase continuing across the corners. */
function drawDotsPath(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  pts: Px[],
  phase: number,
  current: number,
) {
  if (Math.abs(current) < DOT_MIN_AMPS) return;
  let ph = phase;
  for (let k = 0; k + 1 < pts.length; k++) {
    const a = pts[k]!;
    const b = pts[k + 1]!;
    drawDots(ctx, cam, a, b, ph, current);
    ph -= mag(sub(b, a)) / cam.scale;
  }
}

// ------------------------------------------------------------------ damage
//
// Overload is a TEACHING tool, so it has to be legible long before anything
// fails: a part discolours as it heats, smokes as it approaches its limit,
// pops, and then stays visibly charred until somebody repairs it. Every
// number behind this comes from the server's `damage` snapshot, which is
// computed from solver output — the client only picks colours.

/** Live damage for one part, from the server's `damage` snapshot. */
export interface DamageState {
  /** Normalised temperature, 0..1. 1 means it let go. */
  stress: number;
  broken: boolean;
  /** `performance.now()` when this client saw it pop — drives the one-shot
   * magic-smoke burst. Absent for parts that were already dead on arrival. */
  poppedAt?: number;
}

/** Stress at which the heat tint becomes visible. */
export const STRESS_WARM = 0.35;
/** Stress at which the part starts smoking — the last warning. */
export const STRESS_SMOKE = 0.7;
/** Magic-smoke burst duration, ms. */
const POP_MS = 1600;
/** Scorch/char colour, and the "find me" colour when zoomed out. */
const CHAR = '#1a1216';
const BROKEN_MARK = '#ff6a3d';

/** Cheap deterministic 0..1 from an integer — per-part smoke placement that
 * does not shimmer between frames. */
const hash01 = (n: number): number => {
  const x = Math.sin(n * 12.9898 + 78.233) * 43758.5453;
  return x - Math.floor(x);
};

/** Heat colour for a stress level: amber at the first warning, deep red at
 * the edge of failure. */
function heatColor(stress: number, alpha: number): string {
  const t = Math.max(0, Math.min(1, (stress - STRESS_WARM) / (1 - STRESS_WARM)));
  const r = 255;
  const g = Math.round(190 - 130 * t);
  const b = Math.round(90 - 80 * t);
  return `rgba(${r},${g},${b},${alpha.toFixed(3)})`;
}

/** Centre of a part's pin chain, in px. */
function centerOf(P: Px[]): Px {
  let x = 0;
  let y = 0;
  for (const p of P) {
    x += p[0];
    y += p[1];
  }
  return [x / Math.max(1, P.length), y / Math.max(1, P.length)];
}

/** The heat glow behind a stressed part, and the wisps of smoke that come
 * before the bang. Drawn UNDER the symbol so the schematic stays readable. */
function drawStress(d: DrawCtx, e: ElementSpec, P: Px[], stress: number) {
  const { ctx, cam } = d;
  const s = cam.scale;
  const c = centerOf(P);
  const t = (stress - STRESS_WARM) / (1 - STRESS_WARM);
  const rad = s * (0.7 + 0.5 * t);
  const g = ctx.createRadialGradient(c[0], c[1], s * 0.1, c[0], c[1], rad);
  g.addColorStop(0, heatColor(stress, 0.15 + 0.5 * t));
  g.addColorStop(1, heatColor(stress, 0));
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(c[0], c[1], rad, 0, Math.PI * 2);
  ctx.fill();

  if (stress < STRESS_SMOKE || d.time === undefined) return;
  // Thin wisps: three puffs on a staggered 1.8 s cycle, rising and fading.
  const k = (stress - STRESS_SMOKE) / (1 - STRESS_SMOKE);
  ctx.fillStyle = '#c9c9d4';
  for (let n = 0; n < 3; n++) {
    const phase = ((d.time / 1800 + hash01(e.id * 7 + n)) % 1 + 1) % 1;
    const a = (1 - phase) * (0.12 + 0.3 * k);
    if (a <= 0.01) continue;
    const dx = (hash01(e.id * 13 + n) - 0.5) * s * 0.5 * phase;
    ctx.globalAlpha = a;
    ctx.beginPath();
    ctx.arc(c[0] + dx, c[1] - s * (0.35 + 1.1 * phase), s * (0.08 + 0.16 * phase), 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
}

/** A part that has let go: scorch mark, cracked body, and — for the first
 * moment and a half — the blue smoke leaving it. */
function drawBroken(d: DrawCtx, e: ElementSpec, P: Px[]) {
  const { ctx, cam } = d;
  const s = cam.scale;
  const c = centerOf(P);

  // Scorch: a sooty smudge that survives on the schematic until repair.
  const g = ctx.createRadialGradient(c[0], c[1], s * 0.05, c[0], c[1], s * 0.95);
  g.addColorStop(0, 'rgba(20,14,16,0.85)');
  g.addColorStop(0.65, 'rgba(30,20,22,0.55)');
  g.addColorStop(1, 'rgba(30,20,22,0)');
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(c[0], c[1], s * 0.95, 0, Math.PI * 2);
  ctx.fill();

  // Charred, crumpled body: a dark blob with two cracks through it.
  ctx.fillStyle = CHAR;
  ctx.beginPath();
  ctx.arc(c[0], c[1], s * 0.3, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = BROKEN_MARK;
  ctx.lineWidth = Math.max(1.5, s * 0.05);
  for (const dir of [-1, 1]) {
    ctx.beginPath();
    ctx.moveTo(c[0] - s * 0.42 * dir, c[1] - s * 0.34);
    ctx.lineTo(c[0] - s * 0.08 * dir, c[1] - s * 0.06);
    ctx.lineTo(c[0] + s * 0.16 * dir, c[1] + s * 0.12);
    ctx.lineTo(c[0] + s * 0.4 * dir, c[1] + s * 0.36);
    ctx.stroke();
  }

  // The magic smoke, one shot, blue as tradition demands.
  if (d.time === undefined || d.dmg?.poppedAt === undefined) return;
  const age = (d.time - d.dmg.poppedAt) / POP_MS;
  if (age < 0 || age > 1) return;
  for (let n = 0; n < 6; n++) {
    const phase = Math.min(1, age * (1.1 + 0.5 * hash01(e.id * 31 + n)));
    const a = (1 - phase) * 0.6;
    if (a <= 0.01) continue;
    const dx = (hash01(e.id * 17 + n) - 0.5) * s * 1.1 * phase;
    ctx.fillStyle = `rgba(120,170,255,${a.toFixed(3)})`;
    ctx.beginPath();
    ctx.arc(
      c[0] + dx,
      c[1] - s * (0.2 + 1.7 * phase),
      s * (0.12 + 0.42 * phase),
      0,
      Math.PI * 2,
    );
    ctx.fill();
  }
}

interface DrawCtx {
  ctx: CanvasRenderingContext2D;
  cam: Camera;
  live?: ElemLive;
  dots: DotFlow;
  dtSec: number;
  /** Server-computed damage for this part, if it has any. */
  dmg?: DamageState;
  /** `performance.now()`, for the smoke animations. Absent for ghosts. */
  time?: number;
  /** Speakers only: what this part is doing to the player's ears, from the
   * audio tap. `level` is the 0..1 amplitude of the stream that actually
   * reached the sound card (the 30 Hz render frame cannot see a 440 Hz
   * waveform; the 12.5 kHz tap can). Absent = not streamed at all. */
  sound?: { level: number; muted: boolean };
}

function stroke(ctx: CanvasRenderingContext2D, color: string, path: Px[]) {
  ctx.strokeStyle = color;
  ctx.beginPath();
  ctx.moveTo(...path[0]!);
  for (const p of path.slice(1)) ctx.lineTo(...p);
  ctx.stroke();
}

function arrowHead(ctx: CanvasRenderingContext2D, tip: Px, dir: Px, size: number, color: string) {
  const n = perp(dir);
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(...tip);
  ctx.lineTo(...add(add(tip, dir, -size), n, size * 0.5));
  ctx.lineTo(...add(add(tip, dir, -size), n, -size * 0.5));
  ctx.closePath();
  ctx.fill();
}

/** Zigzag resistor body between two px points, gradient-colored. */
function zigzag(ctx: CanvasRenderingContext2D, a: Px, b: Px, va: number, vb: number, s: number) {
  const u = norm(sub(b, a));
  const n = perp(u);
  const amp = s * 0.16;
  const grad = ctx.createLinearGradient(...a, ...b);
  grad.addColorStop(0, voltageColor(va));
  grad.addColorStop(1, voltageColor(vb));
  const pts: Px[] = [a];
  const zags = 6;
  for (let k = 0; k < zags; k++) {
    const t = (k + 0.5) / zags;
    pts.push(add(lerp(a, b, t), n, k % 2 === 0 ? amp : -amp));
  }
  pts.push(b);
  ctx.strokeStyle = grad as unknown as string;
  ctx.beginPath();
  ctx.moveTo(...pts[0]!);
  for (const p of pts.slice(1)) ctx.lineTo(...p);
  ctx.stroke();
}

export function drawElement(d: DrawCtx, e: ElementSpec) {
  const { ctx, cam } = d;
  const s = cam.scale;
  const P = e.pins.map((p) => px(cam, p));
  // Heat goes down first, so the symbol stays readable on top of it. A
  // broken part still draws its symbol — you have to recognise WHAT died —
  // and gets charred over afterwards.
  if (d.dmg && !d.dmg.broken && d.dmg.stress > STRESS_WARM) {
    drawStress(d, e, P, d.dmg.stress);
  }
  const v = (i: number) => d.live?.v[i] ?? 0;
  const iPin = (i: number) => d.live?.i[i] ?? 0;
  const i0 = iPin(0);
  ctx.lineWidth = Math.max(2, s * 0.07);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';

  const A = P[0]!;
  const B = P[1] ?? A;
  const u = norm(sub(B, A));
  const n = perp(u);
  const mid = lerp(A, B, 0.5);

  const twoPinDots = (current = i0) =>
    drawDots(ctx, cam, A, B, d.dots.advance(e.id, current, d.dtSec), current);

  switch (e.kind.t) {
    case 'Wire': {
      stroke(ctx, voltageColor(v(0)), [A, B]);
      twoPinDots();
      break;
    }
    case 'Ground': {
      ctx.strokeStyle = voltageColor(v(0));
      ctx.beginPath();
      ctx.moveTo(A[0], A[1]);
      ctx.lineTo(A[0], A[1] + s * 0.25);
      for (const [w, dy] of [[0.5, 0.25], [0.33, 0.47], [0.16, 0.69]] as const) {
        ctx.moveTo(A[0] - s * w * 0.5, A[1] + s * dy);
        ctx.lineTo(A[0] + s * w * 0.5, A[1] + s * dy);
      }
      ctx.stroke();
      break;
    }
    case 'Rail': {
      // Ground's mirror image: a stem UP from the pin to a ring — the
      // implicit return path is the ground symbol it points away from. DC
      // shows a '+' in the ring; AC shows a tilde. The ring is deliberately
      // big: it is the part's whole body, so it has to be an easy grab
      // (hitTest treats the stem+ring as the clickable body).
      const r = s * 0.28;
      const cy = A[1] - s * 0.55;
      ctx.strokeStyle = voltageColor(v(0));
      ctx.beginPath();
      ctx.moveTo(A[0], A[1]);
      ctx.lineTo(A[0], cy + r);
      ctx.stroke();
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.arc(A[0], cy, r, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = '#c9c9d4';
      ctx.font = `${Math.round(s * 0.4)}px ui-monospace`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(e.kind.amp === 0 ? '+' : '∿', A[0], cy + 0.5);
      ctx.textAlign = 'start';
      ctx.textBaseline = 'alphabetic';
      break;
    }
    case 'Resistor': {
      const e1 = add(mid, u, -s * 0.45);
      const e2 = add(mid, u, s * 0.45);
      stroke(ctx, voltageColor(v(0)), [A, e1]);
      stroke(ctx, voltageColor(v(1)), [e2, B]);
      zigzag(ctx, e1, e2, v(0), v(1), s);
      twoPinDots();
      break;
    }
    case 'Potentiometer': {
      const C = P[2]!;
      const uAC = norm(sub(C, A));
      const m2 = lerp(A, C, 0.5);
      const e1 = add(m2, uAC, -s * 0.45);
      const e2 = add(m2, uAC, s * 0.45);
      stroke(ctx, voltageColor(v(0)), [A, e1]);
      stroke(ctx, voltageColor(v(2)), [e2, C]);
      zigzag(ctx, e1, e2, v(0), v(2), s);
      // wiper arrow from pin 1 toward the body position given by `wiper`
      const w = e.kind.wiper;
      const tip = add(lerp(e1, e2, w), perp(uAC), Math.sign(dot(sub(P[1]!, m2), perp(uAC))) * s * 0.2);
      stroke(ctx, voltageColor(v(1)), [P[1]!, tip]);
      arrowHead(ctx, tip, norm(sub(tip, P[1]!)), s * 0.18, voltageColor(v(1)));
      break;
    }
    case 'Capacitor': {
      const gap = s * 0.11;
      const plate = s * 0.42;
      const p1 = add(mid, u, -gap);
      const p2 = add(mid, u, gap);
      stroke(ctx, voltageColor(v(0)), [A, p1]);
      stroke(ctx, voltageColor(v(1)), [p2, B]);
      stroke(ctx, voltageColor(v(0)), [add(p1, n, -plate), add(p1, n, plate)]);
      stroke(ctx, voltageColor(v(1)), [add(p2, n, -plate), add(p2, n, plate)]);
      drawDots(ctx, cam, A, p1, d.dots.advance(e.id, i0, d.dtSec), i0);
      drawDots(ctx, cam, p2, B, d.dots.advance(e.id + 1_000_000, i0, d.dtSec), i0);
      break;
    }
    case 'Inductor': {
      const e1 = add(mid, u, -s * 0.45);
      const e2 = add(mid, u, s * 0.45);
      stroke(ctx, voltageColor(v(0)), [A, e1]);
      stroke(ctx, voltageColor(v(1)), [e2, B]);
      const grad = ctx.createLinearGradient(...e1, ...e2);
      grad.addColorStop(0, voltageColor(v(0)));
      grad.addColorStop(1, voltageColor(v(1)));
      ctx.strokeStyle = grad as unknown as string;
      ctx.beginPath();
      ctx.moveTo(...e1);
      const bumps = 3;
      for (let k = 0; k < bumps; k++) {
        const from = lerp(e1, e2, k / bumps);
        const to = lerp(e1, e2, (k + 1) / bumps);
        const cp = add(lerp(from, to, 0.5), n, -s * 0.38);
        ctx.quadraticCurveTo(cp[0], cp[1], to[0], to[1]);
      }
      ctx.stroke();
      twoPinDots();
      break;
    }
    case 'VoltageSource': {
      if (e.kind.amp === 0) {
        // battery plates, pin 0 = +
        const gap = s * 0.14;
        const p1 = add(mid, u, -gap);
        const p2 = add(mid, u, gap);
        stroke(ctx, voltageColor(v(0)), [A, p1]);
        stroke(ctx, voltageColor(v(1)), [p2, B]);
        stroke(ctx, voltageColor(v(0)), [add(p1, n, -s * 0.45), add(p1, n, s * 0.45)]);
        stroke(ctx, voltageColor(v(1)), [add(p2, n, -s * 0.22), add(p2, n, s * 0.22)]);
        ctx.fillStyle = '#c9c9d4';
        ctx.font = `${Math.round(s * 0.3)}px ui-monospace`;
        ctx.fillText('+', p1[0] + n[0] * s * 0.45 + 6, p1[1] + n[1] * s * 0.45);
      } else {
        // AC source: circle with a sine squiggle
        const r = s * 0.42;
        stroke(ctx, voltageColor(v(0)), [A, add(mid, u, -r)]);
        stroke(ctx, voltageColor(v(1)), [add(mid, u, r), B]);
        ctx.strokeStyle = '#c9c9d4';
        ctx.beginPath();
        ctx.arc(mid[0], mid[1], r, 0, Math.PI * 2);
        ctx.stroke();
        ctx.beginPath();
        const w = r * 0.55;
        ctx.moveTo(mid[0] - w, mid[1]);
        ctx.quadraticCurveTo(mid[0] - w / 2, mid[1] - w * 1.6, mid[0], mid[1]);
        ctx.quadraticCurveTo(mid[0] + w / 2, mid[1] + w * 1.6, mid[0] + w, mid[1]);
        ctx.stroke();
      }
      twoPinDots(-i0);
      break;
    }
    case 'CurrentSource': {
      const r = s * 0.42;
      stroke(ctx, voltageColor(v(0)), [A, add(mid, u, -r)]);
      stroke(ctx, voltageColor(v(1)), [add(mid, u, r), B]);
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.arc(mid[0], mid[1], r, 0, Math.PI * 2);
      ctx.stroke();
      stroke(ctx, '#c9c9d4', [add(mid, u, -r * 0.5), add(mid, u, r * 0.5)]);
      arrowHead(ctx, add(mid, u, r * 0.5), u, s * 0.2, '#c9c9d4');
      twoPinDots();
      break;
    }
    case 'Noise': {
      // A source circle (same family as the AC source) with a jagged trace
      // instead of a sine. The trace is a FIXED sequence, not Math.random:
      // the symbol must not shimmer, and a redraw is not a new sample.
      const r = s * 0.42;
      stroke(ctx, voltageColor(v(0)), [A, add(mid, u, -r)]);
      stroke(ctx, voltageColor(v(1)), [add(mid, u, r), B]);
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.arc(mid[0], mid[1], r, 0, Math.PI * 2);
      ctx.stroke();
      const HISS = [0.15, -0.85, 0.55, -0.3, 0.95, -0.6, 0.35, -0.95, 0.7, -0.1];
      ctx.beginPath();
      for (let k = 0; k < HISS.length; k++) {
        const t = k / (HISS.length - 1);
        const p = add(add(mid, u, (t - 0.5) * r * 1.3), n, HISS[k]! * r * 0.6);
        if (k === 0) ctx.moveTo(...p);
        else ctx.lineTo(...p);
      }
      ctx.stroke();
      twoPinDots(-i0);
      break;
    }
    case 'Switch': {
      const t1 = lerp(A, B, 0.3);
      const t2 = lerp(A, B, 0.7);
      stroke(ctx, voltageColor(v(0)), [A, t1]);
      stroke(ctx, voltageColor(v(1)), [t2, B]);
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.moveTo(...t1);
      if (e.kind.closed) {
        ctx.lineTo(...t2);
      } else {
        const lever = sub(t2, t1);
        const ang = -0.55;
        ctx.lineTo(
          t1[0] + lever[0] * Math.cos(ang) - lever[1] * Math.sin(ang),
          t1[1] + lever[0] * Math.sin(ang) + lever[1] * Math.cos(ang),
        );
      }
      ctx.stroke();
      for (const p of [t1, t2]) {
        ctx.fillStyle = '#c9c9d4';
        ctx.beginPath();
        ctx.arc(p[0], p[1], s * 0.06, 0, Math.PI * 2);
        ctx.fill();
      }
      if (e.kind.closed) twoPinDots();
      break;
    }
    case 'Button': {
      // Momentary contact: a bridging bar under a round plunger cap. The
      // cap sinks onto the contacts while it is held down.
      const t1 = lerp(A, B, 0.3);
      const t2 = lerp(A, B, 0.7);
      stroke(ctx, voltageColor(v(0)), [A, t1]);
      stroke(ctx, voltageColor(v(1)), [t2, B]);
      const lift = e.kind.closed ? s * 0.02 : s * 0.26;
      const b1 = add(t1, n, -lift);
      const b2 = add(t2, n, -lift);
      stroke(ctx, '#c9c9d4', [b1, b2]);
      const stem = lerp(b1, b2, 0.5);
      const cap = add(stem, n, -s * (e.kind.closed ? 0.3 : 0.42));
      stroke(ctx, '#c9c9d4', [stem, cap]);
      ctx.fillStyle = e.kind.closed ? '#ffe95e' : '#c9c9d4';
      ctx.beginPath();
      ctx.arc(cap[0], cap[1], s * 0.2, 0, Math.PI * 2);
      ctx.fill();
      for (const p of [t1, t2]) {
        ctx.fillStyle = '#c9c9d4';
        ctx.beginPath();
        ctx.arc(p[0], p[1], s * 0.06, 0, Math.PI * 2);
        ctx.fill();
      }
      if (e.kind.closed) twoPinDots();
      break;
    }
    case 'Timer555': {
      // Local package frame: x runs THR -> OUT (across the DIP), y runs
      // VCC -> GND (down the left edge), both in grid units.
      const [Vp, Gp, Op_, Hp] = [P[0]!, P[1]!, P[4]!, P[3]!];
      const ex = norm(sub(Op_, Hp));
      const ey = norm(sub(Gp, Vp));
      const w = mag(sub(Op_, Hp)) / s;
      const h = mag(sub(Gp, Vp)) / s;
      const at = (x: number, y: number): Px => add(add(Vp, ex, x * s), ey, y * s);
      const lx = (p: Px) => dot(sub(p, Vp), ex) / s;
      const ly = (p: Px) => dot(sub(p, Vp), ey) / s;
      const stub = Math.min(0.9, w * 0.25);
      const [x0, x1, y0, y1] = [stub, w - stub, -0.5, h + 0.5];
      // Pin stubs into the nearest package edge.
      P.forEach((p, k) => {
        const left = lx(p) < w * 0.5;
        stroke(ctx, voltageColor(v(k)), [p, at(left ? x0 : x1, ly(p))]);
      });
      // Package body.
      ctx.fillStyle = '#181820';
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.moveTo(...at(x0, y0));
      ctx.lineTo(...at(x1, y0));
      ctx.lineTo(...at(x1, y1));
      ctx.lineTo(...at(x0, y1));
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      if (cam.scale > 40) {
        // Faint hint of the innards: the 2/3–1/3 divider tap points and
        // the two comparators the latch listens to.
        ctx.save();
        ctx.globalAlpha = 0.3;
        ctx.lineWidth = Math.max(1, s * 0.02);
        ctx.strokeStyle = '#c9c9d4';
        ctx.fillStyle = '#c9c9d4';
        const dvx = x0 + (x1 - x0) * 0.78;
        stroke(ctx, '#c9c9d4', [at(dvx, y0 + 0.35), at(dvx, y1 - 0.35)]);
        for (const f of [0.25, 0.5, 0.75]) {
          const c = at(dvx, y0 + 0.35 + (y1 - y0 - 0.7) * f);
          ctx.beginPath();
          ctx.arc(c[0], c[1], Math.max(1.5, s * 0.05), 0, Math.PI * 2);
          ctx.fill();
        }
        const cmpx = x0 + (x1 - x0) * 0.26;
        for (const cy of [h * 0.3, h * 0.7]) {
          ctx.beginPath();
          ctx.moveTo(...at(cmpx - 0.18, cy - 0.34));
          ctx.lineTo(...at(cmpx - 0.18, cy + 0.34));
          ctx.lineTo(...at(cmpx + 0.5, cy));
          ctx.closePath();
          ctx.stroke();
        }
        ctx.restore();
      }
      if (cam.scale > 24) {
        const labels = pinLabels(e.kind);
        ctx.fillStyle = '#8a8a98';
        ctx.font = `${Math.round(s * 0.2)}px ui-monospace`;
        P.forEach((p, k) => {
          const left = lx(p) < w * 0.5;
          ctx.textAlign = left ? 'left' : 'right';
          const [tx, ty] = at(left ? x0 + 0.16 : x1 - 0.16, ly(p) + 0.08);
          ctx.fillText(labels[k] ?? '', tx, ty);
        });
        ctx.textAlign = 'center';
        ctx.fillStyle = '#c9c9d4';
        ctx.font = `${Math.round(s * 0.34)}px ui-monospace`;
        ctx.fillText('555', ...at(w * 0.5, h * 0.5 + 0.12));
        ctx.textAlign = 'start';
      }
      const io = iPin(4);
      drawDots(ctx, cam, at(x1, ly(Op_)), Op_, d.dots.advance(e.id, io, d.dtSec), io);
      break;
    }
    case 'Diode':
    case 'Zener':
    case 'Led': {
      const half = s * 0.28;
      const t1 = add(mid, u, -half);
      const t2 = add(mid, u, half);
      stroke(ctx, voltageColor(v(0)), [A, t1]);
      stroke(ctx, voltageColor(v(1)), [t2, B]);
      if (e.kind.t === 'Led') {
        const glow = Math.min(1.5, Math.abs(i0) / 0.02);
        if (glow > 0.02) {
          const color = LED_COLORS[e.kind.color] ?? LED_COLORS[0]!;
          const g = ctx.createRadialGradient(mid[0], mid[1], s * 0.1, mid[0], mid[1], s * (0.8 + glow * 0.8));
          g.addColorStop(0, color + 'e0');
          g.addColorStop(1, color + '00');
          ctx.fillStyle = g;
          ctx.beginPath();
          ctx.arc(mid[0], mid[1], s * (0.8 + glow * 0.8), 0, Math.PI * 2);
          ctx.fill();
        }
      }
      // anode triangle
      ctx.fillStyle = voltageColor(v(0));
      ctx.beginPath();
      ctx.moveTo(...add(t1, n, s * 0.3));
      ctx.lineTo(...add(t1, n, -s * 0.3));
      ctx.lineTo(...t2);
      ctx.closePath();
      ctx.fill();
      // cathode bar (bent for zener)
      const barColor = voltageColor(v(1));
      stroke(ctx, barColor, [add(t2, n, s * 0.3), add(t2, n, -s * 0.3)]);
      if (e.kind.t === 'Zener') {
        stroke(ctx, barColor, [add(t2, n, s * 0.3), add(add(t2, n, s * 0.3), u, -s * 0.12)]);
        stroke(ctx, barColor, [add(t2, n, -s * 0.3), add(add(t2, n, -s * 0.3), u, s * 0.12)]);
      }
      twoPinDots();
      break;
    }
    case 'Lamp': {
      const r = s * 0.5;
      const frac = Math.min(1.6, Math.abs(d.live?.power ?? 0) / e.kind.rated_watts);
      if (frac > 0.01) {
        const g = ctx.createRadialGradient(mid[0], mid[1], r * 0.2, mid[0], mid[1], r * (1.5 + frac));
        g.addColorStop(0, `rgba(255,241,150,${Math.min(0.95, frac)})`);
        g.addColorStop(1, 'rgba(255,241,150,0)');
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(mid[0], mid[1], r * (1.5 + frac), 0, Math.PI * 2);
        ctx.fill();
      }
      stroke(ctx, voltageColor(v(0)), [A, add(mid, u, -r)]);
      stroke(ctx, voltageColor(v(1)), [add(mid, u, r), B]);
      ctx.strokeStyle = frac > 0.02 ? '#ffe95e' : '#8a8a95';
      ctx.beginPath();
      ctx.arc(mid[0], mid[1], r, 0, Math.PI * 2);
      ctx.stroke();
      const k = r * Math.SQRT1_2;
      ctx.beginPath();
      ctx.moveTo(mid[0] - k, mid[1] - k);
      ctx.lineTo(mid[0] + k, mid[1] + k);
      ctx.moveTo(mid[0] + k, mid[1] - k);
      ctx.lineTo(mid[0] - k, mid[1] + k);
      ctx.stroke();
      twoPinDots();
      break;
    }
    case 'Speaker': {
      // Classic loudspeaker: the voice coil as a circle on the lead axis,
      // the cone flaring off one face.
      const r = s * 0.22;
      const ang = Math.atan2(n[1], n[0]);
      // What is reaching the ears, from the 12.5 kHz audio tap. `undefined`
      // means this speaker is not streamed (offline, or past the server's
      // simultaneous-tap cap) — then it draws plainly idle, which is the
      // truth: it is making no sound you can hear.
      const heard = d.sound && !d.sound.muted ? d.sound.level : 0;
      // Halo behind everything: the "this one is audible" cue, and the only
      // thing here that knows about the audio band.
      if (heard > 0.01) {
        const rad = r + s * (0.55 + heard * 0.7);
        const g = ctx.createRadialGradient(mid[0], mid[1], r * 0.5, mid[0], mid[1], rad);
        g.addColorStop(0, `rgba(142,231,255,${Math.min(0.5, 0.12 + heard * 0.5)})`);
        g.addColorStop(1, 'rgba(142,231,255,0)');
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(mid[0], mid[1], rad, 0, Math.PI * 2);
        ctx.fill();
      }
      stroke(ctx, voltageColor(v(0)), [A, add(mid, u, -r)]);
      stroke(ctx, voltageColor(v(1)), [add(mid, u, r), B]);
      ctx.fillStyle = '#181820';
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.arc(mid[0], mid[1], r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      const throat = add(mid, n, r * 0.72);
      const mouth = add(mid, n, r + s * 0.4);
      ctx.beginPath();
      ctx.moveTo(...add(throat, u, -r * 0.7));
      ctx.lineTo(...add(mouth, u, -s * 0.42));
      ctx.lineTo(...add(mouth, u, s * 0.42));
      ctx.lineTo(...add(throat, u, r * 0.7));
      ctx.stroke();
      // Radiating arcs. One set, and its COLOUR is the whole message:
      //   cyan  — audible, amplitude from the 12.5 kHz tap (what you hear)
      //   amber — driven but not reaching your ears (muted, offline, or past
      //           the server's tap cap), amplitude from the solver frame
      // A speaker doing nothing electrically draws neither, so a silent
      // circuit draws a silent speaker exactly as before.
      const drive = Math.min(1, Math.abs(v(0) - v(1)) / 5);
      const audible = heard > 0.01;
      const amp = audible ? Math.min(1, heard * 1.6) : drive;
      if (amp > 0.02) {
        ctx.strokeStyle = audible ? '#8ee7ff' : '#ffe95e';
        for (let k = 0; k < (audible ? 3 : 2); k++) {
          ctx.globalAlpha = Math.max(0, amp - k * 0.3);
          ctx.beginPath();
          ctx.arc(mid[0], mid[1], s * (0.75 + k * 0.2), ang - 0.55, ang + 0.55);
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
      }
      if (d.sound?.muted) {
        // Muted at the mixer: a slash across the arcs. Plainly not sounding,
        // and plainly not sounding ON PURPOSE — the amber arcs still show the
        // coil is being driven.
        ctx.lineWidth = Math.max(1.4, s * 0.05);
        const c = add(mid, n, r + s * 0.62);
        const q = s * 0.19;
        stroke(ctx, '#ff8f8f', [add(add(c, u, -q), n, -q), add(add(c, u, q), n, q)]);
        ctx.lineWidth = Math.max(2, s * 0.07);
      }
      twoPinDots();
      break;
    }
    case 'Npn':
    case 'Pnp': {
      // Textbook BJT: base lead perpendicular into a straight base bar,
      // collector/emitter legs leaving the bar at ~50° and routing to their
      // pins. Everything derives from the three pin positions, so the
      // symbol is correct at any orientation.
      const [Bp, Cp, Ep] = [P[0]!, P[1]!, P[2]!];
      const axis = sub(lerp(Cp, Ep, 0.5), Bp);
      const ub = norm(axis); // base lead: base pin -> bar
      const bn0 = perp(ub);
      // Bar normal points from the collector side toward the emitter side.
      const bn: Px = dot(bn0, sub(Ep, Cp)) < 0 ? [-bn0[0], -bn0[1]] : bn0;
      const axLen = Math.max(mag(axis), s * 0.5);
      const barC = add(Bp, ub, axLen * 0.42); // bar center
      const att = s * 0.28; // leg attachment offset along the bar
      const legOf = (Q: Px): { att: Px; knee: Px } => {
        const dp = dot(sub(Q, barC), bn); // perpendicular offset of the pin
        const da = Math.max(dot(sub(Q, barC), ub), s * 0.1); // axial distance
        const sg = dp >= 0 ? 1 : -1;
        const a0 = add(barC, bn, sg * att);
        // tan(50°) ≈ 1.19 axial units per unit of perpendicular climb
        const run = Math.min(da, Math.abs(dp - sg * att) * 1.19);
        return { att: a0, knee: add(add(barC, bn, dp), ub, run) };
      };
      const cl = legOf(Cp);
      const el = legOf(Ep);
      const cPath: Px[] = [Cp, cl.knee, cl.att];
      const ePath: Px[] = [el.att, el.knee, Ep];
      stroke(ctx, voltageColor(v(0)), [Bp, barC]);
      stroke(ctx, voltageColor(v(1)), cPath);
      stroke(ctx, voltageColor(v(2)), ePath);
      stroke(ctx, '#c9c9d4', [add(barC, bn, s * 0.5), add(barC, bn, -s * 0.5)]);
      // Emitter arrow, on the emitter leg only: away from the bar for NPN
      // (conventional current out of the emitter), toward it for PNP.
      const slant = sub(el.knee, el.att);
      const away = norm(slant);
      const npn = e.kind.t === 'Npn';
      const asize = Math.min(s * 0.24, Math.max(s * 0.1, mag(slant) * 0.55));
      arrowHead(
        ctx,
        lerp(el.att, el.knee, npn ? 0.72 : 0.3),
        npn ? away : [-away[0], -away[1]],
        asize,
        voltageColor(v(2)),
      );
      // Dots on all three leads (base current is honest information).
      const ic = iPin(1);
      const ie = -iPin(2);
      const ib = iPin(0);
      drawDotsPath(ctx, cam, [Bp, barC], d.dots.advance(e.id, ib, d.dtSec), ib);
      drawDotsPath(ctx, cam, cPath, d.dots.advance(e.id + 1_000_000, ic, d.dtSec), ic);
      drawDotsPath(ctx, cam, ePath, d.dots.advance(e.id + 2_000_000, ie, d.dtSec), ie);
      break;
    }
    case 'Nmos':
    case 'Pmos': {
      // Enhancement MOSFET with legs: gate lead -> gate plate, air gap,
      // broken channel plate, and drain/source legs that step off the
      // channel perpendicular before running to their pins.
      const [Gp, Dp, Sp] = [P[0]!, P[1]!, P[2]!];
      const axis = sub(lerp(Dp, Sp, 0.5), Gp);
      const ug = norm(axis); // gate pin -> plates
      const bn0 = perp(ug);
      // Plate normal points from the drain side toward the source side.
      const bn: Px = dot(bn0, sub(Sp, Dp)) < 0 ? [-bn0[0], -bn0[1]] : bn0;
      const axLen = Math.max(mag(axis), s * 0.5);
      const gateC = add(Gp, ug, axLen * 0.4); // gate plate center
      const chC = add(gateC, ug, s * 0.17); // channel plate center (after gap)
      const halfCh = s * 0.55;
      stroke(ctx, voltageColor(v(0)), [Gp, gateC]);
      stroke(ctx, '#c9c9d4', [add(gateC, bn, s * 0.42), add(gateC, bn, -s * 0.42)]);
      for (const [k0, k1] of [[-1, -0.4], [-0.22, 0.22], [0.4, 1]] as const) {
        stroke(ctx, '#c9c9d4', [add(chC, bn, halfCh * k0), add(chC, bn, halfCh * k1)]);
      }
      // Legs: channel end -> perpendicular stub -> across -> pin.
      const legOf = (Q: Px, end: number): { pts: Px[]; stub: [Px, Px] } => {
        const dp = dot(sub(Q, chC), bn);
        const da = Math.max(dot(sub(Q, chC), ug), s * 0.2);
        const root = add(chC, bn, end * halfCh);
        const run = Math.min(s * 0.5, da * 0.6);
        const k1 = add(root, ug, run);
        const k2 = add(add(chC, bn, dp), ug, run);
        return { pts: [root, k1, k2, Q], stub: [root, k1] };
      };
      const dl = legOf(Dp, -1);
      const sl = legOf(Sp, 1);
      stroke(ctx, voltageColor(v(1)), dl.pts);
      stroke(ctx, voltageColor(v(2)), sl.pts);
      // Source-leg arrow: into the channel for NMOS, out of it for PMOS.
      const nmos = e.kind.t === 'Nmos';
      const stubLen = mag(sub(sl.stub[1], sl.stub[0]));
      const asize = Math.min(s * 0.22, Math.max(s * 0.09, stubLen * 0.6));
      arrowHead(
        ctx,
        lerp(sl.stub[0], sl.stub[1], nmos ? 0.12 : 0.9),
        nmos ? [-ug[0], -ug[1]] : ug,
        asize,
        voltageColor(v(2)),
      );
      // The gate draws no current, so only the conducting legs get dots.
      const idd = iPin(1);
      drawDotsPath(ctx, cam, [...dl.pts].reverse(), d.dots.advance(e.id, idd, d.dtSec), idd);
      drawDotsPath(ctx, cam, sl.pts, d.dots.advance(e.id + 1_000_000, idd, d.dtSec), idd);
      break;
    }
    case 'OpAmp':
    case 'Ota': {
      // Classic proportions: back-edge height comes from the input-pin
      // spacing (2 grid units -> 2.3 units tall) and the length is 1.3x the
      // height. Anything left over becomes the output lead.
      const [Pp, Mp, Op] = [P[0]!, P[1]!, P[2]!];
      const back = lerp(Pp, Mp, 0.5);
      const inSpan = mag(sub(Pp, Mp));
      const pmDir = inSpan > 1e-6 ? norm(sub(Pp, Mp)) : perp(norm(sub(Op, back)));
      const axVec = sub(Op, back);
      const axDist = mag(axVec);
      const uo = axDist > 1e-6 ? norm(axVec) : perp(pmDir);
      // Height from the input spacing, but never so tall that the 1.3:1
      // body would overshoot the output pin.
      const half = Math.max(s * 0.45, Math.min(inSpan * 0.575, axDist / 2.6));
      const triLen = Math.max(s * 0.5, Math.min(axDist, half * 2.6));
      const apex = add(back, uo, triLen);
      const v1 = add(back, pmDir, half);
      const v2 = add(back, pmDir, -half);
      // Input leads, for placements whose pins sit outside the back edge.
      for (const [k, Q] of [[0, Pp], [1, Mp]] as const) {
        const dp = dot(sub(Q, back), pmDir);
        const edge = add(back, pmDir, Math.max(-half * 0.8, Math.min(half * 0.8, dp)));
        if (mag(sub(edge, Q)) > s * 0.05) stroke(ctx, voltageColor(v(k)), [Q, edge]);
      }
      // Output lead for the axial slack the triangle does not use.
      const io = -iPin(2); // current leaving the output pin
      if (axDist - triLen > s * 0.02) {
        stroke(ctx, voltageColor(v(2)), [apex, Op]);
        drawDots(ctx, cam, apex, Op, d.dots.advance(e.id, io, d.dtSec), io);
      } else {
        d.dots.advance(e.id, io, d.dtSec);
      }
      ctx.fillStyle = '#181820';
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.moveTo(...v1);
      ctx.lineTo(...v2);
      ctx.lineTo(...apex);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      // +/- inside the triangle, next to their inputs.
      ctx.save();
      ctx.fillStyle = '#c9c9d4';
      ctx.font = `${Math.round(Math.min(s * 0.3, half * 0.42))}px ui-monospace`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      const inset = add(back, uo, Math.min(s * 0.34, triLen * 0.2));
      const lab = Math.min(half * 0.55, s * 0.62);
      ctx.fillText('+', ...add(inset, pmDir, lab));
      ctx.fillText('−', ...add(inset, pmDir, -lab));
      ctx.restore();
      if (e.kind.t === 'Ota' && P[3]) {
        // Bias lead into the triangle's belly; a double bar marks the
        // current-output nature.
        stroke(ctx, voltageColor(v(3)), [P[3]!, lerp(back, apex, 0.45)]);
        const no = perp(uo);
        ctx.strokeStyle = '#c9c9d4';
        for (const k of [0.62, 0.74]) {
          const c = lerp(back, apex, k);
          const h = half * (1 - k) * 0.8;
          ctx.beginPath();
          ctx.moveTo(...add(c, no, h));
          ctx.lineTo(...add(c, no, -h));
          ctx.stroke();
        }
      }
      break;
    }
  }

  // Char goes on last: a dead part is recognisable but plainly ruined.
  if (d.dmg?.broken) drawBroken(d, e, P);
}

const dot = (a: Px, b: Px) => a[0] * b[0] + a[1] * b[1];

// -------------------------------------------------------------- LOD grid
//
// The background grid is what makes a far-out view a PLACE instead of a
// void, so it must survive to the bottom of the zoom range — and it must do
// it in a bounded number of primitives. The spacing is the coarsest decade
// (1 / 10 / 100 / 1000 grid units) that still keeps dots or lines at least
// GRID_MIN_PX apart; the decade below it is drawn faded so crossing a level
// does not pop.

/** On-screen spacing floor for the current grid level, px. */
export const GRID_MIN_PX = 12;
/** Faintest spacing worth drawing at all, px. */
const GRID_FADE_PX = 4;
/** Hard bound on the fine dot pass, whatever the window size. */
const MAX_GRID_DOTS = 4500;

/** Grid spacing in grid units for a zoom level: 1, 10, 100, 1000, 10000. */
export function gridStep(scale: number): number {
  let step = 1;
  while (step < 10000 && step * scale < GRID_MIN_PX) step *= 10;
  return step;
}

/** Grid lines at `step` grid units as one path; null when too dense to
 * bother. Segment count is bounded by (W + H) / (step * scale). */
function gridLines(cam: Camera, step: number, W: number, H: number): Path2D | null {
  const sp = step * cam.scale;
  if (sp < GRID_FADE_PX) return null;
  const p = new Path2D();
  const gx0 = Math.ceil(-cam.ox / cam.scale / step) * step;
  const gy0 = Math.ceil(-cam.oy / cam.scale / step) * step;
  for (let x = cam.ox + gx0 * cam.scale; x < W; x += sp) {
    const xr = Math.round(x) + 0.5;
    p.moveTo(xr, 0);
    p.lineTo(xr, H);
  }
  for (let y = cam.oy + gy0 * cam.scale; y < H; y += sp) {
    const yr = Math.round(y) + 0.5;
    p.moveTo(0, yr);
    p.lineTo(W, yr);
  }
  return p;
}

/** The background: Falstad dots close in, faded decade lines far out. */
export function drawGrid(ctx: CanvasRenderingContext2D, cam: Camera, W: number, H: number) {
  const step = gridStep(cam.scale);
  const sp = step * cam.scale;

  if (step === 1) {
    const gx0 = Math.ceil(-cam.ox / cam.scale);
    const gy0 = Math.ceil(-cam.oy / cam.scale);
    const px0 = cam.ox + gx0 * cam.scale;
    const py0 = cam.oy + gy0 * cam.scale;
    const cols = Math.floor((W - px0) / sp) + 1;
    const rows = Math.floor((H - py0) / sp) + 1;
    if (cols <= 0 || rows <= 0) return;
    if (cols * rows <= MAX_GRID_DOTS) {
      ctx.fillStyle = '#1c1c22';
      for (let c = 0; c < cols; c++) {
        const x = px0 + c * sp - 1;
        for (let r = 0; r < rows; r++) ctx.fillRect(x, py0 + r * sp - 1, 2, 2);
      }
      return;
    }
    // A big window at 12-20 px/unit would need more dots than the budget
    // allows: fall through and draw the same pitch as lines instead.
  }

  ctx.save();
  ctx.lineWidth = 1;
  if (step > 1) {
    const sub = gridLines(cam, step / 10, W, H);
    const fade = Math.min(1, Math.max(0, (sp / 10 - GRID_FADE_PX) / (GRID_MIN_PX - GRID_FADE_PX)));
    if (sub && fade > 0.02) {
      ctx.globalAlpha = fade;
      ctx.strokeStyle = '#191920';
      ctx.stroke(sub);
      ctx.globalAlpha = 1;
    }
  }
  const minor = gridLines(cam, step, W, H);
  if (minor) {
    ctx.strokeStyle = '#191920';
    ctx.stroke(minor);
  }
  const major = gridLines(cam, step * 10, W, H);
  if (major) {
    ctx.strokeStyle = '#26262f';
    ctx.stroke(major);
  }
  ctx.restore();
}

// ------------------------------------------------------- zoomed-out LOD pass
//
// Below a few px per grid unit a full symbol is sub-pixel noise: the zigzag,
// the current dots and the pin labels all land inside one or two pixels and
// cost more than they show. These passes draw the same conductors as plain
// segments, colored by the SOLVER's own pin voltage (quantized to a fixed
// ramp only so thousands of elements collapse into a handful of stroke
// calls — the value still comes from the frame, never from the UI).

/** Voltage buckets in the LOD ramp (odd, so 0 V lands exactly in the middle). */
const LOD_STEPS = 17;

const LOD_RAMP: string[] = Array.from({ length: LOD_STEPS }, (_, k) =>
  voltageColor(((k / (LOD_STEPS - 1)) * 2 - 1) * V_FULL),
);

function lodBucket(v: number): number {
  const t = Math.max(-1, Math.min(1, v / V_FULL));
  return Math.round(((t + 1) / 2) * (LOD_STEPS - 1));
}

/** Draw a whole (already culled) element list without symbol detail.
 * `single` collapses each element to one segment, first pin to last —
 * for the far zoom band where even the pin chain is a smudge. */
export function drawElementsLod(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  elems: ElementSpec[],
  live: Map<number, ElemLive>,
  single: boolean,
  dmg?: Map<number, DamageState>,
) {
  const paths: (Path2D | undefined)[] = new Array<Path2D | undefined>(LOD_STEPS);
  const tick = Math.max(1, cam.scale * 0.3);
  // Finding the dead part is the repair job, so a broken one keeps a marker
  // that survives all the way out: at this zoom the symbol is a smudge, but
  // the cross is still a cross.
  const dead: Px[] = [];
  for (const e of elems) {
    if (dmg?.get(e.id)?.broken && e.pins.length > 0) {
      let cx = 0;
      let cy = 0;
      for (const p of e.pins) {
        cx += p[0];
        cy += p[1];
      }
      dead.push([
        cam.ox + (cx / e.pins.length) * cam.scale,
        cam.oy + (cy / e.pins.length) * cam.scale,
      ]);
    }
  }
  for (const e of elems) {
    const P = e.pins;
    if (P.length === 0) continue;
    const l = live.get(e.id);
    const k = lodBucket(l?.v[0] ?? 0);
    let path = paths[k];
    if (!path) {
      path = new Path2D();
      paths[k] = path;
    }
    const ax = cam.ox + P[0]![0] * cam.scale;
    const ay = cam.oy + P[0]![1] * cam.scale;
    if (P.length === 1) {
      // Single-pin parts (ground) get a stub so they are not invisible.
      path.moveTo(ax - tick, ay);
      path.lineTo(ax + tick, ay);
      continue;
    }
    path.moveTo(ax, ay);
    if (single) {
      const last = P[P.length - 1]!;
      path.lineTo(cam.ox + last[0] * cam.scale, cam.oy + last[1] * cam.scale);
      continue;
    }
    if (P.length > 4) {
      // Packages (the 555) are boxes, not chains: a polyline through DIP
      // pins would read as a scribble. Outline the pin bbox instead.
      let x0 = Infinity;
      let y0 = Infinity;
      let x1 = -Infinity;
      let y1 = -Infinity;
      for (const p of P) {
        if (p[0] < x0) x0 = p[0];
        if (p[0] > x1) x1 = p[0];
        if (p[1] < y0) y0 = p[1];
        if (p[1] > y1) y1 = p[1];
      }
      const px0 = cam.ox + x0 * cam.scale;
      const py0 = cam.oy + y0 * cam.scale;
      path.moveTo(px0, py0);
      path.lineTo(cam.ox + x1 * cam.scale, py0);
      path.lineTo(cam.ox + x1 * cam.scale, cam.oy + y1 * cam.scale);
      path.lineTo(px0, cam.oy + y1 * cam.scale);
      path.lineTo(px0, py0);
      continue;
    }
    for (let i = 1; i < P.length; i++) {
      path.lineTo(cam.ox + P[i]![0] * cam.scale, cam.oy + P[i]![1] * cam.scale);
    }
    // Three-pin parts close the triangle so a transistor still reads as one.
    if (P.length === 3) {
      path.moveTo(ax, ay);
      path.lineTo(cam.ox + P[2]![0] * cam.scale, cam.oy + P[2]![1] * cam.scale);
    }
  }
  ctx.lineWidth = Math.max(1, cam.scale * 0.12);
  ctx.lineCap = 'butt';
  for (let k = 0; k < LOD_STEPS; k++) {
    const path = paths[k];
    if (!path) continue;
    ctx.strokeStyle = LOD_RAMP[k]!;
    ctx.stroke(path);
  }
  ctx.lineCap = 'round';
  if (dead.length > 0) {
    // One path for all of them: a district full of dead parts costs one
    // stroke call, and each mark stays legible (screen-space size) however
    // far out the camera is.
    const r = Math.max(3, Math.min(7, cam.scale * 0.9));
    const marks = new Path2D();
    for (const [x, y] of dead) {
      marks.moveTo(x - r, y - r);
      marks.lineTo(x + r, y + r);
      marks.moveTo(x + r, y - r);
      marks.lineTo(x - r, y + r);
    }
    ctx.strokeStyle = BROKEN_MARK;
    ctx.lineWidth = 2;
    ctx.stroke(marks);
  }
}

/** Distance in px from (x, y) to the element (nearest pin-chain segment;
 * 3-pin parts also count their body around the centroid, packages their
 * whole pin bounding box). */
export function hitTest(cam: Camera, e: ElementSpec, x: number, y: number): number {
  const P = e.pins.map((p) => px(cam, p));
  if (P.length === 1) {
    const [ax, ay] = P[0]!;
    // One-pin parts draw a body away from the pin (Rail up, Ground down):
    // the whole stem-to-symbol span is the clickable body, or the part
    // could only ever be grabbed by its connection point.
    const ext = e.kind.t === 'Rail' ? -0.85 : e.kind.t === 'Ground' ? 0.72 : 0;
    const ey = ay + cam.scale * ext;
    const t = ext === 0 ? 0 : Math.max(0, Math.min(1, (y - ay) / (ey - ay)));
    return Math.hypot(x - ax, y - (ay + t * (ey - ay)));
  }
  let best = Infinity;
  const segs: [Px, Px][] = [];
  for (let k = 0; k + 1 < P.length; k++) segs.push([P[k]!, P[k + 1]!]);
  if (P.length >= 3) segs.push([P[0]!, P[2]!]);
  for (const [a, b] of segs) {
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const l2 = dx * dx + dy * dy;
    let t = l2 === 0 ? 0 : ((x - a[0]) * dx + (y - a[1]) * dy) / l2;
    t = Math.max(0, Math.min(1, t));
    best = Math.min(best, Math.hypot(x - (a[0] + t * dx), y - (a[1] + t * dy)));
  }
  if (P.length > 4) {
    // Packages (the 6-pin 555) are boxes: anywhere inside the pin bounding
    // box is the body, so the whole chip is grabbable.
    const xs = P.map((p) => p[0]);
    const ys = P.map((p) => p[1]);
    const dx = Math.max(Math.min(...xs) - x, 0, x - Math.max(...xs));
    const dy = Math.max(Math.min(...ys) - y, 0, y - Math.max(...ys));
    best = Math.min(best, Math.hypot(dx, dy));
  } else if (P.length >= 3) {
    // Body hit: centroid of the first three pins (triangle for op-amps).
    const cx = (P[0]![0] + P[1]![0] + P[2]![0]) / 3;
    const cy = (P[0]![1] + P[1]![1] + P[2]![1]) / 3;
    best = Math.min(best, Math.max(0, Math.hypot(x - cx, y - cy) - cam.scale * 0.8));
  }
  return best;
}
