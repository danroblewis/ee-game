// Falstad-feel Canvas2D renderer for the M1 demo: voltage-colored
// conductors, animated current dots, glowing lamp. (The real WebGL canvas
// package arrives with the S2 spike; this proves the sim feels alive.)

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

const px = (cam: Camera, p: Point): [number, number] => [
  cam.ox + p[0] * cam.scale,
  cam.oy + p[1] * cam.scale,
];

function lerp(a: [number, number], b: [number, number], t: number): [number, number] {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
}

/** Animated dot phase per element, advanced by simulated current. */
export class DotFlow {
  private phase = new Map<number, number>();

  advance(id: number, current: number, dtSec: number): number {
    const p = (this.phase.get(id) ?? 0) + current * dtSec * 6; // grid units/sec per amp... tuned
    this.phase.set(id, p);
    return p;
  }
}

const DOT_SPACING = 0.55; // grid units between dots

/** Dots only exist where current flows (Falstad convention). */
const DOT_MIN_AMPS = 1e-4;

export function drawDots(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  a: [number, number],
  b: [number, number],
  phase: number,
  current: number,
) {
  if (Math.abs(current) < DOT_MIN_AMPS) return;
  const len = Math.hypot(b[0] - a[0], b[1] - a[1]) / cam.scale;
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

interface DrawCtx {
  ctx: CanvasRenderingContext2D;
  cam: Camera;
  live?: ElemLive;
  dots: DotFlow;
  dtSec: number;
}

export function drawElement(d: DrawCtx, e: ElementSpec) {
  const { ctx, cam } = d;
  const A = px(cam, e.a);
  const B = px(cam, e.b);
  const va = d.live?.va ?? 0;
  const vb = d.live?.vb ?? 0;
  const i = d.live?.current ?? 0;
  ctx.lineWidth = Math.max(2, cam.scale * 0.07);
  ctx.lineCap = 'round';

  switch (e.kind.t) {
    case 'Wire': {
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(...A);
      ctx.lineTo(...B);
      ctx.stroke();
      drawDots(ctx, cam, A, B, d.dots.advance(e.id, i, d.dtSec), i);
      break;
    }
    case 'Ground': {
      ctx.strokeStyle = voltageColor(va);
      const s = cam.scale * 0.5;
      ctx.beginPath();
      ctx.moveTo(A[0], A[1]);
      ctx.lineTo(A[0], A[1] + s * 0.5);
      for (const [w, dy] of [
        [0.5, 0.5],
        [0.33, 0.72],
        [0.16, 0.94],
      ] as const) {
        ctx.moveTo(A[0] - s * w, A[1] + s * dy);
        ctx.lineTo(A[0] + s * w, A[1] + s * dy);
      }
      ctx.stroke();
      break;
    }
    case 'VoltageSource': {
      // Leads up to the plates; a (+, long plate) at terminal a.
      const mid = lerp(A, B, 0.5);
      const dir = [(B[0] - A[0]), (B[1] - A[1])];
      const len = Math.hypot(dir[0]!, dir[1]!) || 1;
      const u = [dir[0]! / len, dir[1]! / len]; // a -> b unit
      const n = [-u[1]!, u[0]!]; // normal
      const gap = cam.scale * 0.14;
      const p1: [number, number] = [mid[0] - u[0]! * gap, mid[1] - u[1]! * gap];
      const p2: [number, number] = [mid[0] + u[0]! * gap, mid[1] + u[1]! * gap];
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(...A);
      ctx.lineTo(...p1);
      ctx.stroke();
      ctx.strokeStyle = voltageColor(vb);
      ctx.beginPath();
      ctx.moveTo(...p2);
      ctx.lineTo(...B);
      ctx.stroke();
      // plates
      const long = cam.scale * 0.45;
      const short = cam.scale * 0.22;
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(p1[0] - n[0]! * long, p1[1] - n[1]! * long);
      ctx.lineTo(p1[0] + n[0]! * long, p1[1] + n[1]! * long);
      ctx.stroke();
      ctx.strokeStyle = voltageColor(vb);
      ctx.beginPath();
      ctx.moveTo(p2[0] - n[0]! * short, p2[1] - n[1]! * short);
      ctx.lineTo(p2[0] + n[0]! * short, p2[1] + n[1]! * short);
      ctx.stroke();
      ctx.fillStyle = '#c9c9d4';
      ctx.font = `${Math.round(cam.scale * 0.3)}px ui-monospace`;
      ctx.fillText('+', p1[0] + n[0]! * long + 6, p1[1] + n[1]! * long);
      drawDots(ctx, cam, A, B, d.dots.advance(e.id, -i, d.dtSec), i);
      break;
    }
    case 'Switch': {
      const t1 = lerp(A, B, 0.3);
      const t2 = lerp(A, B, 0.7);
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(...A);
      ctx.lineTo(...t1);
      ctx.stroke();
      ctx.strokeStyle = voltageColor(vb);
      ctx.beginPath();
      ctx.moveTo(...t2);
      ctx.lineTo(...B);
      ctx.stroke();
      // lever
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.moveTo(...t1);
      if (e.kind.closed) {
        ctx.lineTo(...t2);
      } else {
        const ang = -0.55;
        const dx = t2[0] - t1[0];
        const dy = t2[1] - t1[1];
        ctx.lineTo(
          t1[0] + dx * Math.cos(ang) - dy * Math.sin(ang),
          t1[1] + dx * Math.sin(ang) + dy * Math.cos(ang),
        );
      }
      ctx.stroke();
      for (const p of [t1, t2]) {
        ctx.fillStyle = '#c9c9d4';
        ctx.beginPath();
        ctx.arc(p[0], p[1], cam.scale * 0.06, 0, Math.PI * 2);
        ctx.fill();
      }
      if (e.kind.closed) drawDots(ctx, cam, A, B, d.dots.advance(e.id, i, d.dtSec), i);
      break;
    }
    case 'Lamp': {
      const mid = lerp(A, B, 0.5);
      const r = cam.scale * 0.5;
      const frac = Math.min(1.6, Math.abs(d.live?.power ?? 0) / e.kind.rated_watts);
      // glow halo
      if (frac > 0.01) {
        const g = ctx.createRadialGradient(mid[0], mid[1], r * 0.2, mid[0], mid[1], r * (1.5 + frac));
        g.addColorStop(0, `rgba(255,241,150,${Math.min(0.95, frac)})`);
        g.addColorStop(1, 'rgba(255,241,150,0)');
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(mid[0], mid[1], r * (1.5 + frac), 0, Math.PI * 2);
        ctx.fill();
      }
      // leads
      const dir = [(B[0] - A[0]), (B[1] - A[1])];
      const len = Math.hypot(dir[0]!, dir[1]!) || 1;
      const u = [dir[0]! / len, dir[1]! / len];
      const e1: [number, number] = [mid[0] - u[0]! * r, mid[1] - u[1]! * r];
      const e2: [number, number] = [mid[0] + u[0]! * r, mid[1] + u[1]! * r];
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(...A);
      ctx.lineTo(...e1);
      ctx.stroke();
      ctx.strokeStyle = voltageColor(vb);
      ctx.beginPath();
      ctx.moveTo(...e2);
      ctx.lineTo(...B);
      ctx.stroke();
      // bulb
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
      drawDots(ctx, cam, A, B, d.dots.advance(e.id, i, d.dtSec), i);
      break;
    }
    default: {
      // Generic two-terminal box (resistor etc.) — enough for M1.
      ctx.strokeStyle = voltageColor(va);
      ctx.beginPath();
      ctx.moveTo(...A);
      ctx.lineTo(...B);
      ctx.stroke();
      drawDots(ctx, cam, A, B, d.dots.advance(e.id, i, d.dtSec), i);
    }
  }
}

/** Distance from point to segment, in px. */
export function hitTest(cam: Camera, e: ElementSpec, x: number, y: number): number {
  const A = px(cam, e.a);
  const B = px(cam, e.b);
  const dx = B[0] - A[0];
  const dy = B[1] - A[1];
  const l2 = dx * dx + dy * dy;
  if (l2 === 0) return Math.hypot(x - A[0], y - A[1]);
  let t = ((x - A[0]) * dx + (y - A[1]) * dy) / l2;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(x - (A[0] + t * dx), y - (A[1] + t * dy));
}
