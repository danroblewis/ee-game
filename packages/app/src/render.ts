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

/** Animated dot phase per element, advanced by simulated current. */
export class DotFlow {
  private phase = new Map<number, number>();

  advance(id: number, current: number, dtSec: number): number {
    const p = (this.phase.get(id) ?? 0) + current * dtSec * 6;
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

interface DrawCtx {
  ctx: CanvasRenderingContext2D;
  cam: Camera;
  live?: ElemLive;
  dots: DotFlow;
  dtSec: number;
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
    case 'Npn':
    case 'Pnp': {
      const [Bp, Cp, Ep] = [P[0]!, P[1]!, P[2]!];
      const ceMid = lerp(Cp, Ep, 0.5);
      const ub = norm(sub(ceMid, Bp)); // base -> body
      const barC = add(ceMid, ub, -s * 0.28);
      const barN = perp(ub);
      const halfBar = s * 0.4;
      // leads
      stroke(ctx, voltageColor(v(0)), [Bp, barC]);
      const sC = Math.sign(dot(sub(Cp, barC), barN)) || 1;
      const cAtt = add(barC, barN, sC * halfBar * 0.7);
      const eAtt = add(barC, barN, -sC * halfBar * 0.7);
      stroke(ctx, voltageColor(v(1)), [Cp, cAtt]);
      stroke(ctx, voltageColor(v(2)), [Ep, eAtt]);
      // base bar
      stroke(ctx, '#c9c9d4', [add(barC, barN, halfBar), add(barC, barN, -halfBar)]);
      // emitter arrow: out for NPN, in for PNP
      const eDir = norm(sub(Ep, eAtt));
      const eColor = voltageColor(v(2));
      if (e.kind.t === 'Npn') {
        arrowHead(ctx, lerp(eAtt, Ep, 0.6), eDir, s * 0.22, eColor);
      } else {
        arrowHead(ctx, lerp(Ep, eAtt, 0.6), norm(sub(eAtt, Ep)), s * 0.22, eColor);
      }
      // dots along collector->emitter through the device
      const ic = iPin(1);
      drawDots(ctx, cam, Cp, Ep, d.dots.advance(e.id, ic, d.dtSec), ic);
      break;
    }
    case 'Nmos':
    case 'Pmos': {
      const [Gp, Dp, Sp] = [P[0]!, P[1]!, P[2]!];
      const dsMid = lerp(Dp, Sp, 0.5);
      const toG = norm(sub(Gp, dsMid));
      // channel bar on the D-S line, gate bar offset toward the gate pin
      const chA = lerp(Dp, Sp, 0.22);
      const chB = lerp(Dp, Sp, 0.78);
      stroke(ctx, voltageColor(v(1)), [Dp, chA]);
      stroke(ctx, voltageColor(v(2)), [Sp, chB]);
      stroke(ctx, '#c9c9d4', [chA, chB]);
      const gA = add(lerp(Dp, Sp, 0.28), toG, s * 0.18);
      const gB = add(lerp(Dp, Sp, 0.72), toG, s * 0.18);
      stroke(ctx, '#c9c9d4', [gA, gB]);
      stroke(ctx, voltageColor(v(0)), [Gp, lerp(gA, gB, 0.5)]);
      // arrow into (NMOS) / out of (PMOS) the channel at the source end
      const sDir = norm(sub(chB, Sp));
      if (e.kind.t === 'Nmos') {
        arrowHead(ctx, lerp(Sp, chB, 0.7), sDir, s * 0.2, voltageColor(v(2)));
      } else {
        arrowHead(ctx, lerp(chB, Sp, 0.7), norm(sub(Sp, chB)), s * 0.2, voltageColor(v(2)));
      }
      const idd = iPin(1);
      drawDots(ctx, cam, Dp, Sp, d.dots.advance(e.id, idd, d.dtSec), idd);
      break;
    }
    case 'OpAmp':
    case 'Ota': {
      const [Pp, Mp, Op] = [P[0]!, P[1]!, P[2]!];
      const back = lerp(Pp, Mp, 0.5);
      const pmDir = norm(sub(Pp, Mp));
      const half = Math.max(s * 0.9, mag(sub(Pp, Mp)) * 0.75);
      const v1 = add(back, pmDir, half);
      const v2 = add(back, pmDir, -half);
      ctx.fillStyle = '#181820';
      ctx.strokeStyle = '#c9c9d4';
      ctx.beginPath();
      ctx.moveTo(...v1);
      ctx.lineTo(...v2);
      ctx.lineTo(...Op);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      ctx.fillStyle = '#c9c9d4';
      ctx.font = `${Math.round(s * 0.32)}px ui-monospace`;
      const inset = add(back, norm(sub(Op, back)), s * 0.22);
      ctx.fillText('+', inset[0] + (Pp[0] - back[0]) * 0.55 - s * 0.1, inset[1] + (Pp[1] - back[1]) * 0.55 + s * 0.1);
      ctx.fillText('−', inset[0] + (Mp[0] - back[0]) * 0.55 - s * 0.1, inset[1] + (Mp[1] - back[1]) * 0.55 + s * 0.1);
      if (e.kind.t === 'Ota' && P[3]) {
        // Bias lead into the triangle's belly; a double bar marks the
        // current-output nature.
        const center = lerp(back, Op, 0.45);
        stroke(ctx, voltageColor(v(3)), [P[3]!, center]);
        const uo = norm(sub(Op, back));
        const no = perp(uo);
        ctx.strokeStyle = '#c9c9d4';
        for (const k of [0.62, 0.74]) {
          const c = lerp(back, Op, k);
          ctx.beginPath();
          ctx.moveTo(...add(c, no, s * 0.16));
          ctx.lineTo(...add(c, no, -s * 0.16));
          ctx.stroke();
        }
      }
      const io = iPin(2);
      drawDots(ctx, cam, Op, add(Op, norm(sub(Op, back)), s * 0.01), d.dots.advance(e.id, io, d.dtSec), 0);
      break;
    }
  }
}

const dot = (a: Px, b: Px) => a[0] * b[0] + a[1] * b[1];

/** Distance in px from (x, y) to the element (nearest pin-chain segment;
 * 3-pin parts also count their body around the centroid, packages their
 * whole pin bounding box). */
export function hitTest(cam: Camera, e: ElementSpec, x: number, y: number): number {
  const P = e.pins.map((p) => px(cam, p));
  if (P.length === 1) return Math.hypot(x - P[0]![0], y - P[0]![1]);
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
