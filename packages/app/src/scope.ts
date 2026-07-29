// The oscilloscope: rolling traces, per-trace auto-scale, min-max column
// rendering, auto-measurements. No knobs to fight — the scope just shows
// the signal (design pillar: never-fight-the-scope).

export interface Probe {
  pid: number;
  elem: number;
  pin: number;
  kind: 'v' | 'i';
}

export const PROBE_COLORS = [
  '#ffd54a', '#4ad4ff', '#ff5ac8', '#7dff6a', '#ff8c42', '#b18cff', '#8cffd9', '#ff6a6a',
];
export const probeColor = (pid: number) => PROBE_COLORS[pid % PROBE_COLORS.length]!;

const KEEP_SECONDS = 120;

/** Per-probe sample history as interleaved [t, v, t, v, ...]. */
export class TraceStore {
  private data = new Map<number, number[]>();

  private arr(pid: number): number[] {
    let a = this.data.get(pid);
    if (!a) {
      a = [];
      this.data.set(pid, a);
    }
    return a;
  }

  appendChunk(pid: number, t0: number, dts: number, samples: number[]) {
    const a = this.arr(pid);
    // A time jump backwards means the room restarted: reset the trace.
    if (a.length && t0 < a[a.length - 2]!) a.length = 0;
    for (let k = 0; k < samples.length; k++) a.push(t0 + k * dts, samples[k]!);
    this.trim(a);
  }

  appendPoint(pid: number, t: number, v: number) {
    const a = this.arr(pid);
    if (a.length && t < a[a.length - 2]!) a.length = 0;
    a.push(t, v);
    this.trim(a);
  }

  private trim(a: number[]) {
    const cutoff = (a[a.length - 2] ?? 0) - KEEP_SECONDS;
    let k = 0;
    while (k < a.length && a[k]! < cutoff) k += 2;
    if (k > 0) a.splice(0, k);
  }

  prune(alive: Set<number>) {
    for (const pid of [...this.data.keys()]) if (!alive.has(pid)) this.data.delete(pid);
  }

  maxTime(): number {
    let t = 0;
    for (const a of this.data.values()) if (a.length) t = Math.max(t, a[a.length - 2]!);
    return t;
  }

  samples(pid: number): number[] {
    return this.data.get(pid) ?? [];
  }
}

const fmtSI = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(1)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(1)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(1)} µ${unit}`;
  return `0 ${unit}`;
};

interface TraceStats {
  last: number;
  min: number;
  max: number;
  mean: number;
  freq: number;
}

function windowStats(a: number[], t0: number, t1: number): TraceStats | null {
  let n = 0;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let last = 0;
  let start = a.length;
  for (let k = 0; k < a.length; k += 2) {
    if (a[k]! >= t0) {
      start = k;
      break;
    }
  }
  for (let k = start; k < a.length; k += 2) {
    if (a[k]! > t1) break;
    const v = a[k + 1]!;
    n++;
    sum += v;
    min = Math.min(min, v);
    max = Math.max(max, v);
    last = v;
  }
  if (n === 0) return null;
  const mean = sum / n;
  // Frequency: rising crossings of the mean.
  let crossings = 0;
  let firstT = 0;
  let lastT = 0;
  let prev = 0;
  let seeded = false;
  for (let k = start; k < a.length; k += 2) {
    if (a[k]! > t1) break;
    const v = a[k + 1]! - mean;
    if (seeded && prev <= 0 && v > 0) {
      crossings++;
      if (crossings === 1) firstT = a[k]!;
      lastT = a[k]!;
    }
    prev = v;
    seeded = true;
  }
  const freq = crossings > 1 ? (crossings - 1) / (lastT - firstT) : 0;
  return { last, min, max, mean, freq };
}

export function renderScope(
  canvas: HTMLCanvasElement,
  store: TraceStore,
  probes: Probe[],
  timebase: number,
) {
  const ctx = canvas.getContext('2d')!;
  const dpr = window.devicePixelRatio || 1;
  const W = canvas.clientWidth;
  const H = canvas.clientHeight;
  if (canvas.width !== W * dpr || canvas.height !== H * dpr) {
    canvas.width = W * dpr;
    canvas.height = H * dpr;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, W, H);

  const tNow = store.maxTime();
  const t0 = tNow - timebase;

  // grid: 10 horizontal divisions, 4 vertical
  ctx.strokeStyle = '#26262e';
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let k = 1; k < 10; k++) {
    const x = (W * k) / 10;
    ctx.moveTo(x, 0);
    ctx.lineTo(x, H);
  }
  for (let k = 1; k < 4; k++) {
    const y = (H * k) / 4;
    ctx.moveTo(0, y);
    ctx.lineTo(W, y);
  }
  ctx.stroke();

  ctx.font = '11px ui-monospace, monospace';
  ctx.fillStyle = '#6a6a78';
  ctx.fillText(`${fmtSI(timebase / 10, 's')}/div`, W - 78, H - 6);

  let labelX = 10;
  probes.forEach((p) => {
    const a = store.samples(p.pid);
    const stats = windowStats(a, t0, tNow);
    const color = probeColor(p.pid);
    const unit = p.kind === 'v' ? 'V' : 'A';
    if (stats) {
      // Per-trace auto-scale with 10% padding; zero-span traces get ±1.
      let lo = stats.min;
      let hi = stats.max;
      if (hi - lo < 1e-9) {
        lo -= 1;
        hi += 1;
      }
      const pad = (hi - lo) * 0.12;
      lo -= pad;
      hi += pad;
      const yOf = (v: number) => H - ((v - lo) / (hi - lo)) * (H - 26) - 18;
      const xOf = (t: number) => ((t - t0) / timebase) * W;

      // Min-max column rendering: one pass over the window.
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.4;
      ctx.beginPath();
      let colX = -1;
      let cMin = 0;
      let cMax = 0;
      let started = false;
      let prevY: number | null = null;
      for (let k = 0; k < a.length; k += 2) {
        const t = a[k]!;
        if (t < t0) continue;
        if (t > tNow) break;
        const x = Math.floor(xOf(t));
        const v = a[k + 1]!;
        if (x !== colX) {
          if (started) {
            const y0 = yOf(cMax);
            const y1 = yOf(cMin);
            if (prevY !== null) ctx.lineTo(colX + 0.5, prevY);
            ctx.moveTo(colX + 0.5, y0);
            ctx.lineTo(colX + 0.5, Math.max(y1, y0 + 0.5));
            prevY = (y0 + y1) / 2;
          }
          colX = x;
          cMin = v;
          cMax = v;
          started = true;
        } else {
          cMin = Math.min(cMin, v);
          cMax = Math.max(cMax, v);
        }
      }
      if (started) {
        ctx.moveTo(colX + 0.5, yOf(cMax));
        ctx.lineTo(colX + 0.5, Math.max(yOf(cMin), yOf(cMax) + 0.5));
      }
      ctx.stroke();

      // measurement chips
      const name = `${p.kind.toUpperCase()}${p.pid}`;
      const label =
        `${name} ${fmtSI(stats.last, unit)}  pp ${fmtSI(stats.max - stats.min, unit)}` +
        (stats.freq > 0.05 ? `  ${stats.freq.toFixed(stats.freq < 10 ? 2 : 0)} Hz` : '');
      ctx.fillStyle = color;
      ctx.fillText(label, labelX, 14);
      labelX += ctx.measureText(label).width + 18;
    } else {
      ctx.fillStyle = color;
      ctx.fillText(`${p.kind.toUpperCase()}${p.pid} —`, labelX, 14);
      labelX += 60;
    }
  });
}
