// The oscilloscope: rolling or edge-triggered traces, 1-2-5 auto-scale with
// hysteresis, min-max column rendering with a mean overlay, auto-measurements,
// and an on-canvas control row.
//
// Design pillar: never-fight-the-scope — everything is automatic by default
// (AUTO y-scale, AUTO trigger level, auto timebase from the wheel), but every
// automatic decision has a manual override so the scope can also be *used* as
// an instrument. Every number here comes from TraceStore, i.e. from the solver.
//
// The controls are drawn on-canvas by `renderScopeInto` (bottom strip of the
// scope body) and hit-tested by `scopeControlAt`, which host code (main.ts)
// calls with body-local coordinates; `applyScopeControl` mutates the settings.
// One layout function feeds both the renderer and the hit test, so they can
// never disagree.

export interface Probe {
  pid: number;
  elem: number;
  pin: number;
  kind: 'v' | 'i';
  /** Differential reference [elem, pin]; absent/null = ground-referenced. */
  r?: [number, number] | null;
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

// ------------------------------------------------------------- settings

export type TriggerMode = 'off' | 'auto' | 'normal';
export type TriggerSlope = 'rising' | 'falling';

/** Vertical state the auto-scaler keeps between frames (hysteresis input). */
interface AutoState {
  step: number;
  center: number;
}

/** Per-scope instrument settings. Mutated in place by `applyScopeControl`. */
export interface ScopeSettings {
  /** Seconds across the full width (10 divisions). */
  timebase: number;
  /** 'auto' = per-channel 1-2-5 autoscale; 'manual' = yStep/yOffset below. */
  yMode: 'auto' | 'manual';
  /** Volts (or amps) per vertical division, manual mode only. */
  yStep: number;
  /** Value at the vertical centre of the plot, manual mode only. */
  yOffset: number;
  trigMode: TriggerMode;
  trigSlope: TriggerSlope;
  /** Trigger level in the source channel's units. */
  trigLevel: number;
  /** True while the level tracks the source's mean (auto-trigger). */
  trigAutoLevel: boolean;
  /** Index into the displayed-probe list; clamped at render time. */
  trigSource: number;
  /** Internal: last accepted trigger time, for NORMAL-mode hold. */
  lastTrigger: number | null;
  /** Internal: per-pid auto-scale state so the trace does not jitter. */
  auto: Map<number, AutoState>;
}

export function defaultScopeSettings(timebase = 5): ScopeSettings {
  return {
    timebase,
    yMode: 'auto',
    yStep: 1,
    yOffset: 0,
    trigMode: 'off',
    trigSlope: 'rising',
    trigLevel: 0,
    trigAutoLevel: true,
    trigSource: 0,
    lastTrigger: null,
    auto: new Map(),
  };
}

/** Auto-scale state used when a caller renders without a settings object. */
const legacyAuto = new Map<number, AutoState>();

const fmtSI = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(1)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(1)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(1)} µ${unit}`;
  return `0 ${unit}`;
};

/** Compact form for control-row readouts: 2 significant-ish digits, no space. */
const fmtTight = (v: number, unit: string) => fmtSI(v, unit).replace(' ', '');

// --------------------------------------------------------- 1-2-5 ladder

const MANTISSAS = [1, 2, 5];
const MIN_STEP = 1e-12;

/** Nearest/next/previous value of the 1-2-5-per-decade ladder. */
function nice125(x: number, dir: 'up' | 'down' | 'near'): number {
  const v = Math.abs(x);
  if (!isFinite(v) || v <= MIN_STEP) return MIN_STEP;
  const decade = Math.pow(10, Math.floor(Math.log10(v)));
  const cands: number[] = [];
  for (const d of [decade / 10, decade, decade * 10]) for (const m of MANTISSAS) cands.push(m * d);
  cands.sort((a, b) => a - b);
  if (dir === 'up') {
    for (const c of cands) if (c > v * 1.000001) return c;
    return cands[cands.length - 1]!;
  }
  if (dir === 'down') {
    for (let k = cands.length - 1; k >= 0; k--) if (cands[k]! < v * 0.999999) return cands[k]!;
    return MIN_STEP;
  }
  let best = cands[0]!;
  for (const c of cands) if (Math.abs(c - v) < Math.abs(best - v)) best = c;
  return best;
}

const stepUp = (x: number) => Math.max(MIN_STEP, nice125(x, 'up'));
const stepDown = (x: number) => Math.max(MIN_STEP, nice125(x, 'down'));

const DIVS_Y = 4;
const DIVS_X = 10;

/** Vertical autoscale: 1-2-5 step, centred, with fit hysteresis so the trace
 * stays put instead of breathing every frame. */
function autoScale(prev: AutoState | undefined, min: number, max: number): AutoState {
  const mid = (min + max) / 2;
  const span = max > min ? max - min : 0;
  if (prev && isFinite(prev.step) && prev.step > 0) {
    const half = (prev.step * DIVS_Y) / 2;
    const lo = prev.center - half;
    const hi = prev.center + half;
    const fits = min >= lo + half * 0.04 && max <= hi - half * 0.04;
    const roomy = span > half * 0.35; // zoomed too far out? rescale
    if (fits && (roomy || span === 0)) return prev;
  }
  // Aim for the signal to fill ~2.8 of the 4 divisions.
  const step = span > 0 ? stepUp(span / 2.8) : stepUp(Math.max(Math.abs(mid), 1) / 2.8);
  const center = Math.round(mid / (step / 2)) * (step / 2);
  return { step, center };
}

// ------------------------------------------------------------- controls

export type ScopeControlId =
  | 'tdec' | 'tinc'
  | 'ydec' | 'yinc' | 'yauto'
  | 'odec' | 'oinc'
  | 'trigmode' | 'trigslope' | 'triglvldec' | 'triglvlinc' | 'trigsrc';

interface ScopeButton {
  id: ScopeControlId;
  x: number;
  y: number;
  w: number;
  h: number;
  label: string;
  /** Filled = the mode this button selects is currently engaged. */
  on?: boolean;
}

const CTRL_H = 14;
const CTRL_PAD = 3;
/** Below these sizes the scope is a sparkline: no controls, no chrome. */
const CTRL_MIN_W = 176;
const CTRL_MIN_H = 74;
const CTRL_FULL_W = 302;
const CTRL_FULL_H = 96;

const trigModeLabel = (m: TriggerMode) => (m === 'off' ? 'ROLL' : m === 'auto' ? 'AUTO' : 'NORM');

/** The single source of truth for the control row: renderer and hit test both
 * walk this list, so a button can never be drawn where it is not clickable. */
function controlLayout(W: number, H: number, s: ScopeSettings, chans: number): ScopeButton[] {
  if (W < CTRL_MIN_W || H < CTRL_MIN_H) return [];
  const full = W >= CTRL_FULL_W && H >= CTRL_FULL_H;
  const y = H - CTRL_H - CTRL_PAD;
  const out: ScopeButton[] = [];
  let x = CTRL_PAD + 1;
  const push = (id: ScopeControlId, label: string, w: number, on?: boolean) => {
    out.push({ id, x, y, w, h: CTRL_H, label, on });
    x += w + 1;
  };
  const gap = () => {
    x += 6;
  };
  push('tdec', 't−', 18);
  push('tinc', 't+', 18);
  gap();
  push('ydec', 'y−', 18);
  push('yinc', 'y+', 18);
  push('yauto', 'A', 15, s.yMode === 'auto');
  if (full) {
    gap();
    push('odec', '↓', 15);
    push('oinc', '↑', 15);
  }
  gap();
  push('trigmode', trigModeLabel(s.trigMode), 34, s.trigMode !== 'off');
  if (full) {
    push('trigslope', s.trigSlope === 'rising' ? '⌐' : '⌐̸', 15);
    push('triglvldec', 'L−', 18);
    push('triglvlinc', 'L+', 18);
    if (chans > 1) push('trigsrc', `c${(s.trigSource % Math.max(1, chans)) + 1}`, 18);
  }
  return out.filter((b) => b.x + b.w <= W - CTRL_PAD);
}

/** Hit-test the control row. `lx`/`ly` are relative to the scope *body* rect
 * (the same rect handed to `renderScopeInto`). Returns null outside a button. */
export function scopeControlAt(
  W: number,
  H: number,
  lx: number,
  ly: number,
  s: ScopeSettings,
  chans = 1,
): ScopeControlId | null {
  for (const b of controlLayout(W, H, s, chans)) {
    if (lx >= b.x && lx <= b.x + b.w && ly >= b.y && ly <= b.y + b.h) return b.id;
  }
  return null;
}

/** True if the control row is visible at this size (host cursor feedback). */
export const scopeHasControls = (W: number, H: number) => W >= CTRL_MIN_W && H >= CTRL_MIN_H;

/** Apply one control press to a settings object. */
export function applyScopeControl(s: ScopeSettings, id: ScopeControlId, chans = 1): void {
  switch (id) {
    case 'tdec':
      s.timebase = Math.max(0.001, stepDown(s.timebase));
      break;
    case 'tinc':
      s.timebase = Math.min(60, stepUp(s.timebase));
      break;
    case 'ydec':
      s.yMode = 'manual';
      s.yStep = Math.max(MIN_STEP, stepDown(s.yStep));
      break;
    case 'yinc':
      s.yMode = 'manual';
      s.yStep = Math.min(1e9, stepUp(s.yStep));
      break;
    case 'yauto':
      s.yMode = s.yMode === 'auto' ? 'manual' : 'auto';
      break;
    case 'odec':
      s.yMode = 'manual';
      s.yOffset -= s.yStep / 2;
      break;
    case 'oinc':
      s.yMode = 'manual';
      s.yOffset += s.yStep / 2;
      break;
    case 'trigmode':
      s.trigMode = s.trigMode === 'off' ? 'auto' : s.trigMode === 'auto' ? 'normal' : 'off';
      s.lastTrigger = null;
      break;
    case 'trigslope':
      s.trigSlope = s.trigSlope === 'rising' ? 'falling' : 'rising';
      s.lastTrigger = null;
      break;
    case 'triglvldec':
      s.trigAutoLevel = false;
      s.trigLevel -= s.yStep / 4;
      break;
    case 'triglvlinc':
      s.trigAutoLevel = false;
      s.trigLevel += s.yStep / 4;
      break;
    case 'trigsrc':
      s.trigSource = chans > 0 ? (s.trigSource + 1) % chans : 0;
      s.lastTrigger = null;
      break;
  }
}

// --------------------------------------------------------- measurements

interface TraceStats {
  last: number;
  min: number;
  max: number;
  mean: number;
  freq: number;
}

/** Index of the first [t, v] pair with t >= t0. Samples are time-ordered, so
 * this keeps per-frame cost proportional to the visible window rather than to
 * the whole 120 s buffer. */
function lowerBound(a: number[], t0: number): number {
  let lo = 0;
  let hi = a.length >> 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (a[mid * 2]! < t0) lo = mid + 1;
    else hi = mid;
  }
  return lo * 2;
}

function windowStats(a: number[], t0: number, t1: number): TraceStats | null {
  let n = 0;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let last = 0;
  const start = lowerBound(a, t0);
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

// ------------------------------------------------------------- trigger

/** Most recent level crossing in [tMin, tMax], linearly interpolated. */
function findTrigger(
  a: number[],
  level: number,
  slope: TriggerSlope,
  tMin: number,
  tMax: number,
): number | null {
  for (let k = a.length - 2; k >= 2; k -= 2) {
    const t = a[k]!;
    if (t > tMax) continue;
    if (t < tMin) break;
    const v = a[k + 1]!;
    const pv = a[k - 1]!;
    const hit = slope === 'rising' ? pv < level && v >= level : pv > level && v <= level;
    if (!hit) continue;
    const pt = a[k - 2]!;
    const dv = v - pv;
    const f = Math.abs(dv) > 1e-30 ? Math.min(1, Math.max(0, (level - pv) / dv)) : 0;
    return pt + (t - pt) * f;
  }
  return null;
}

/** Trigger position inside the window (like a real scope's 25% pre-trigger). */
const TRIG_X = 0.25;

interface ScopeWindow {
  t0: number;
  t1: number;
  /** Trigger time when the window is anchored, else null (rolling). */
  anchor: number | null;
  /** True when the anchor is a stale hold (NORMAL mode, no new trigger). */
  held: boolean;
}

function solveWindow(
  store: TraceStore,
  probes: Probe[],
  s: ScopeSettings,
  tNow: number,
  level: number,
): ScopeWindow {
  const tb = s.timebase;
  const rolling: ScopeWindow = { t0: tNow - tb, t1: tNow, anchor: null, held: false };
  if (s.trigMode === 'off' || probes.length === 0) return rolling;
  const src = probes[Math.min(Math.max(0, s.trigSource), probes.length - 1)]!;
  const a = store.samples(src.pid);
  if (s.lastTrigger !== null && s.lastTrigger > tNow) s.lastTrigger = null; // sim restarted
  // Only accept a trigger with a full post-trigger window behind it, so the
  // display is always fully populated (no sweeping right edge).
  const tMax = tNow - tb * (1 - TRIG_X);
  const tc = findTrigger(a, level, s.trigSlope, tNow - 2 * tb, tMax);
  if (tc !== null) {
    s.lastTrigger = tc;
    return { t0: tc - tb * TRIG_X, t1: tc + tb * (1 - TRIG_X), anchor: tc, held: false };
  }
  // No trigger within 2x the window: AUTO rolls, NORMAL holds the last sweep.
  if (s.trigMode === 'normal' && s.lastTrigger !== null) {
    const oldest = a.length ? a[0]! : tNow;
    const t0 = s.lastTrigger - tb * TRIG_X;
    if (t0 >= oldest) {
      return { t0, t1: t0 + tb, anchor: s.lastTrigger, held: true };
    }
  }
  return rolling;
}

// ------------------------------------------------------------ rendering

/** Docked-panel entry point: render into a whole `<canvas>` element. */
export function renderScope(
  canvas: HTMLCanvasElement,
  store: TraceStore,
  probes: Probe[],
  timebase: number,
  settings?: ScopeSettings,
) {
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  const W = canvas.clientWidth;
  const H = canvas.clientHeight;
  if (W <= 0 || H <= 0) return;
  // Round the backing store: `canvas.width` truncates to an integer, so
  // comparing against a fractional W*dpr would resize (and clear) every single
  // frame on fractional-DPR displays — that is the trace "flash".
  const dpr = window.devicePixelRatio || 1;
  const bw = Math.max(1, Math.round(W * dpr));
  const bh = Math.max(1, Math.round(H * dpr));
  if (canvas.width !== bw || canvas.height !== bh) {
    canvas.width = bw;
    canvas.height = bh;
  }
  // Scale from the *actual* backing size so geometry matches the CSS box.
  ctx.setTransform(bw / W, 0, 0, bh / H, 0, 0);
  ctx.clearRect(0, 0, W, H);
  renderScopeInto(ctx, 0, 0, W, H, store, probes, timebase, settings);
}

/** Render traces into an arbitrary rect of an existing context (used by
 * both the docked panel and the in-place floating scopes). */
export function renderScopeInto(
  ctx: CanvasRenderingContext2D,
  X: number,
  Y: number,
  W: number,
  H: number,
  store: TraceStore,
  probes: Probe[],
  timebase: number,
  settings?: ScopeSettings,
) {
  ctx.save();
  ctx.beginPath();
  ctx.rect(X, Y, W, H);
  ctx.clip();
  ctx.translate(X, Y);

  const s = settings;
  const tb = Math.max(1e-6, s ? s.timebase : timebase);
  const compact = H < 80 || W < 180;
  /** Narrow enough that the full measurement chips would collide. */
  const terse = compact || W < 330;
  const buttons = s ? controlLayout(W, H, s, probes.length) : [];
  const tNow = store.maxTime();

  // Plot area: labels on top, control row (when shown) at the bottom.
  const top = compact ? 14 : 18;
  const bot = H - (buttons.length ? CTRL_H + CTRL_PAD * 2 : compact ? 3 : 8);
  const plotH = Math.max(8, bot - top);

  // Trigger level: AUTO tracks the source channel's mean (auto-trigger).
  let level = s ? s.trigLevel : 0;
  const srcIdx = s && probes.length ? Math.min(Math.max(0, s.trigSource), probes.length - 1) : -1;
  if (s && s.trigAutoLevel && srcIdx >= 0) {
    const st = windowStats(store.samples(probes[srcIdx]!.pid), tNow - tb, tNow);
    // 50% of pk-pk, not the mean: over a non-integer number of periods the mean
    // wanders (which would drag the anchor and shake the waveform), while the
    // midpoint of the extremes is steady. Plus a hysteresis band on top.
    if (st) {
      const mid = (st.min + st.max) / 2;
      if (Math.abs(mid - s.trigLevel) > Math.max((st.max - st.min) * 0.08, 1e-12)) {
        s.trigLevel = mid;
      }
      level = s.trigLevel;
    }
  }

  const win = s
    ? solveWindow(store, probes, s, tNow, level)
    : { t0: tNow - tb, t1: tNow, anchor: null, held: false };
  const { t0, t1 } = win;

  // Grid: half-pixel aligned so the 1px lines stay crisp (traces are NOT
  // snapped — they are antialiased polylines).
  ctx.strokeStyle = '#26262e';
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let k = 1; k < DIVS_X; k++) {
    const x = Math.round((W * k) / DIVS_X) + 0.5;
    ctx.moveTo(x, top);
    ctx.lineTo(x, bot);
  }
  for (let k = 0; k <= DIVS_Y; k++) {
    const y = Math.round(top + (plotH * k) / DIVS_Y) + 0.5;
    ctx.moveTo(0, y);
    ctx.lineTo(W, y);
  }
  ctx.stroke();

  ctx.font = '11px ui-monospace, monospace';
  ctx.textBaseline = 'alphabetic';
  // Time/div sits top-right; the measurement chips fill leftwards up to it.
  let chipRight = W - 8;
  if (!compact) {
    ctx.fillStyle = '#6a6a78';
    const info = `${fmtSI(tb / DIVS_X, 's')}/div`;
    const iw = ctx.measureText(info).width;
    ctx.fillText(info, W - 6 - iw, top - 5);
    chipRight = W - 14 - iw;
  }

  const xOf = (t: number) => ((t - t0) / tb) * W;

  let labelX = 10;
  // Boxed so the assignment inside the forEach below survives TS narrowing.
  const srcMap: { f: ((v: number) => number) | null } = { f: null };
  probes.forEach((p, idx) => {
    const a = store.samples(p.pid);
    const stats = windowStats(a, t0, t1);
    const color = probeColor(p.pid);
    const unit = p.kind === 'v' ? 'V' : 'A';
    if (!stats) {
      const dash = `${p.kind.toUpperCase()}${p.pid} —`;
      if (labelX + ctx.measureText(dash).width <= chipRight) {
        ctx.fillStyle = color;
        ctx.fillText(dash, labelX, 12);
      }
      labelX += 60;
      return;
    }

    // ------------------------------------------------ vertical mapping
    let step: number;
    let center: number;
    if (s && s.yMode === 'manual') {
      step = Math.max(MIN_STEP, s.yStep);
      center = s.yOffset;
    } else {
      const cache = s ? s.auto : legacyAuto;
      const st = autoScale(cache.get(p.pid), stats.min, stats.max);
      cache.set(p.pid, st);
      step = st.step;
      center = st.center;
    }
    const halfSpan = (step * DIVS_Y) / 2;
    // Clamped at the graticule edge like a real scope's display: manual scales
    // can drive a signal off-screen, and unbounded coordinates hurt nothing but
    // help nothing either.
    const yOf = (v: number) =>
      Math.min(
        bot + 3,
        Math.max(top - 3, top + plotH / 2 - ((v - center) / halfSpan) * (plotH / 2)),
      );
    if (idx === srcIdx) srcMap.f = yOf;

    // --------------------------------------------------------- traces
    // One pass over the window collecting per-column min/max/mean. When the
    // data is dense we fill the min-max band AND overlay the mean polyline so
    // edges read as solid; when it is sparse we draw the samples directly.
    const first = lowerBound(a, t0);
    const dense = W * 2; // more than 2 samples per pixel -> min-max columns
    let count = 0;
    for (let k = first; k < a.length; k += 2) {
      if (a[k]! > t1) break;
      if (++count > dense) break;
    }
    if (count === 0) return;

    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.strokeStyle = color;

    if (count > dense) {
      const cols = Math.max(1, Math.ceil(W));
      const cMin = new Float64Array(cols).fill(Infinity);
      const cMax = new Float64Array(cols).fill(-Infinity);
      const cSum = new Float64Array(cols);
      const cN = new Int32Array(cols);
      const xk = W / tb;
      for (let k = first; k < a.length; k += 2) {
        const t = a[k]!;
        if (t > t1) break;
        const c = Math.min(cols - 1, Math.max(0, Math.floor((t - t0) * xk)));
        const v = a[k + 1]!;
        if (v < cMin[c]!) cMin[c] = v;
        if (v > cMax[c]!) cMax[c] = v;
        cSum[c] = cSum[c]! + v;
        cN[c] = cN[c]! + 1;
      }
      // Band: forward along the maxima, back along the minima, one filled
      // polygon — no per-column strokes, so nothing shimmers frame to frame.
      ctx.beginPath();
      let open = false;
      let runStart = 0;
      const flush = (end: number) => {
        for (let c = end; c >= runStart; c--) {
          const y1 = yOf(cMin[c]!);
          const y0 = yOf(cMax[c]!);
          ctx.lineTo(c + 0.5, Math.max(y1, y0 + 0.6));
        }
        ctx.closePath();
      };
      for (let c = 0; c < cols; c++) {
        if (!cN[c]) {
          if (open) {
            flush(c - 1);
            open = false;
          }
          continue;
        }
        if (!open) {
          runStart = c;
          open = true;
          ctx.moveTo(c + 0.5, yOf(cMax[c]!));
        } else {
          ctx.lineTo(c + 0.5, yOf(cMax[c]!));
        }
        if (c === cols - 1) {
          flush(c);
          open = false;
        }
      }
      ctx.globalAlpha = 0.4;
      ctx.fillStyle = color;
      ctx.fill();
      ctx.globalAlpha = 1;
      // Mean polyline on top: this is what makes dense traces look solid.
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      let started = false;
      for (let c = 0; c < cols; c++) {
        const n = cN[c]!;
        if (!n) {
          started = false;
          continue;
        }
        const y = yOf(cSum[c]! / n);
        if (started) ctx.lineTo(c + 0.5, y);
        else ctx.moveTo(c + 0.5, y);
        started = true;
      }
      ctx.stroke();
    } else {
      // Sparse: a genuine antialiased polyline through the samples. No pixel
      // snapping here — snapping is what made slow traces look jumpy.
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      let started = false;
      for (let k = first; k < a.length; k += 2) {
        const t = a[k]!;
        if (t > t1) break;
        const x = xOf(t);
        const y = yOf(a[k + 1]!);
        if (started) ctx.lineTo(x, y);
        else ctx.moveTo(x, y);
        started = true;
      }
      ctx.stroke();
    }

    // ---------------------------------------------- measurement chips
    const name = `${p.kind.toUpperCase()}${p.pid}${p.r ? 'Δ' : ''}`;
    const label = terse
      ? `${name} ${fmtSI(stats.last, unit)}`
      : `${name} ${fmtSI(stats.last, unit)}  pp ${fmtSI(stats.max - stats.min, unit)}` +
        (stats.freq > 0.05 ? `  ${stats.freq.toFixed(stats.freq < 10 ? 2 : 0)} Hz` : '');
    const lw = ctx.measureText(label).width;
    if (labelX + lw <= chipRight) {
      ctx.fillStyle = color;
      ctx.fillText(label, labelX, compact ? 12 : 14);
    }
    labelX += lw + 18;
  });

  // ------------------------------------------------- trigger annotation
  if (s && s.trigMode !== 'off' && srcIdx >= 0) {
    const color = probeColor(probes[srcIdx]!.pid);
    const ly = srcMap.f
      ? Math.min(bot - 1, Math.max(top + 1, srcMap.f(level)))
      : top + plotH / 2;
    ctx.save();
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.globalAlpha = 0.55;
    ctx.lineWidth = 1;
    ctx.setLineDash([3, 4]);
    ctx.beginPath();
    ctx.moveTo(0, Math.round(ly) + 0.5);
    ctx.lineTo(W, Math.round(ly) + 0.5);
    ctx.stroke();
    ctx.setLineDash([]);
    // Level marker on the right edge.
    ctx.globalAlpha = 1;
    ctx.beginPath();
    ctx.moveTo(W, ly - 4);
    ctx.lineTo(W, ly + 4);
    ctx.lineTo(W - 6, ly);
    ctx.closePath();
    ctx.fill();
    // Anchor tick at the pre-trigger position.
    if (win.anchor !== null) {
      const ax = Math.round(W * TRIG_X) + 0.5;
      ctx.globalAlpha = win.held ? 0.4 : 0.85;
      ctx.beginPath();
      ctx.moveTo(ax, top);
      ctx.lineTo(ax, top + 6);
      ctx.moveTo(ax, bot - 6);
      ctx.lineTo(ax, bot);
      ctx.stroke();
      ctx.fillRect(ax - 2, top, 4, 2);
    }
    ctx.restore();
  }

  // ------------------------------------------------- manual-scale corner
  if (s && !compact) {
    const bits: string[] = [];
    if (s.yMode === 'manual') {
      const unit = probes.length && probes[0]!.kind === 'i' ? 'A' : 'V';
      bits.push(`${fmtTight(s.yStep, unit)}/div`);
      if (s.yOffset !== 0) bits.push(`y0 ${fmtTight(s.yOffset, unit)}`);
    }
    if (s.trigMode !== 'off') {
      const unit = srcIdx >= 0 && probes[srcIdx]!.kind === 'i' ? 'A' : 'V';
      bits.push(
        `${trigModeLabel(s.trigMode)}${s.trigSlope === 'rising' ? '↑' : '↓'}` +
          `${fmtTight(level, unit)}${win.anchor === null ? ' ?' : win.held ? ' hold' : ''}`,
      );
    }
    if (bits.length) {
      ctx.fillStyle = s.yMode === 'manual' ? '#c9c9d4' : '#8a8a98';
      const txt = bits.join('  ');
      ctx.fillText(txt, Math.max(4, W - 6 - ctx.measureText(txt).width), bot - 5);
    }
  }

  // ------------------------------------------------------- control row
  for (const b of buttons) {
    ctx.fillStyle = b.on ? '#3a4a6a' : '#20202a';
    ctx.strokeStyle = b.on ? '#5a8cff' : '#3a3a48';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.rect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = b.on ? '#dfe6ff' : '#9a9aa8';
    ctx.font = '9px ui-monospace, monospace';
    ctx.textBaseline = 'middle';
    const tw = ctx.measureText(b.label).width;
    ctx.fillText(b.label, b.x + (b.w - tw) / 2, b.y + b.h / 2 + 0.5);
    ctx.textBaseline = 'alphabetic';
  }

  ctx.restore();
}
