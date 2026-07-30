// Control panels: mission-control boxes decoupled from the schematic.
//
// A player drags a dotted PANEL REGION (key J) around a group of parts. The
// region is room-scoped shared state (server owns {plid, rect, name}), but
// membership is never stored: the elements whose pins ALL sit inside the
// rectangle are re-derived from geometry every frame, so moving a part in or
// out re-wires the panel live.
//
// Each region gets a floating HTML CONTROL WINDOW (not canvas) that
// auto-populates widgets from its members — sliders/knobs for pots, toggles
// for switches, indicators for lamps and LEDs, numeric+slider for DC
// sources, and a big meter readout for any probe on a member. Every number
// shown comes out of the solver frame (design pillar); widgets only ever
// send InteractOps through the same path the canvas uses.
//
// An in-place OSCILLOSCOPE whose rect lands inside a region is part of that
// panel's interface too (`scopeOwner`/`panelScopes`, geometry-derived like
// member elements): the schematic keeps only a draggable placeholder while the
// live instrument becomes a canvas widget row in the window.
//
// Screen-anchored, per-player chrome (window position, slider-vs-knob
// choice, widget row order) lives in localStorage keyed by plid; the region
// itself is shared, so everyone sees the same panel with their own layout.

import type { ElemLive, ElementSpec, InteractOp, Point } from './circuit';
import { LED_COLORS, type Camera } from './render';
import {
  applyScopeControl,
  probeColor,
  renderScope,
  scopeChannels,
  scopeControlAt,
  type FloatScope,
  type Probe,
  type ScopeControlId,
  type TraceStore,
} from './scope';

// ------------------------------------------------------------ shared state

export interface Panel {
  plid: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  name: string;
}

/** Client -> server panel ops (mirrors the server's PanelOp enum). */
export type PanelOp =
  | { t: 'add'; x0: number; y0: number; x1: number; y1: number; name?: string }
  | { t: 'remove'; plid: number }
  | { t: 'rect'; plid: number; x0: number; y0: number; x1: number; y1: number }
  | { t: 'rename'; plid: number; name: string };

/** Smallest accepted region in grid units (mirrors the server's rule). */
export const MIN_PANEL_SPAN = 1;

/** Normalize a drag rectangle; null = too small / not finite (dropped). */
export function normPanelRect(a: Point, b: Point): [number, number, number, number] | null {
  const x0 = Math.min(a[0], b[0]);
  const x1 = Math.max(a[0], b[0]);
  const y0 = Math.min(a[1], b[1]);
  const y1 = Math.max(a[1], b[1]);
  if (![x0, y0, x1, y1].every(Number.isFinite)) return null;
  if (x1 - x0 < MIN_PANEL_SPAN || y1 - y0 < MIN_PANEL_SPAN) return null;
  return [x0, y0, x1, y1];
}

/** Offline mirror of the server's `apply_panel_op` (same validation). */
export function applyPanelOp(panels: Panel[], op: PanelOp, allocPlid: () => number): Panel[] {
  if (op.t === 'add') {
    const r = normPanelRect([op.x0, op.y0], [op.x1, op.y1]);
    if (!r) return panels;
    const plid = allocPlid();
    const name = op.name?.trim() || `PANEL ${plid}`;
    return [...panels, { plid, x0: r[0], y0: r[1], x1: r[2], y1: r[3], name }];
  }
  if (op.t === 'remove') return panels.filter((p) => p.plid !== op.plid);
  const p = panels.find((x) => x.plid === op.plid);
  if (!p) return panels;
  if (op.t === 'rect') {
    const r = normPanelRect([op.x0, op.y0], [op.x1, op.y1]);
    if (r) [p.x0, p.y0, p.x1, p.y1] = r;
  } else {
    const name = op.name.trim();
    if (name) p.name = name;
  }
  return panels;
}

/** A panel's members: elements with EVERY pin inside the region. */
export function panelMembers(panel: Panel, elements: ElementSpec[]): ElementSpec[] {
  return elements.filter(
    (e) =>
      e.pins.length > 0 &&
      e.pins.every(
        ([x, y]) => x >= panel.x0 && x <= panel.x1 && y >= panel.y0 && y <= panel.y1,
      ),
  );
}

/** True when a scope's rect is fully inside a region. */
const scopeInside = (panel: Panel, s: FloatScope) =>
  s.x >= panel.x0 && s.x + s.w <= panel.x1 && s.y >= panel.y0 && s.y + s.h <= panel.y1;

const panelArea = (p: Panel) => (p.x1 - p.x0) * (p.y1 - p.y0);

/** The panel a scope belongs to: the SMALLEST region that fully contains its
 * rect (lowest plid breaks a tie, so every client agrees). null = the scope is
 * a normal floating instrument on the schematic. Derived from geometry every
 * frame, exactly like `panelMembers` — dragging a scope out detaches it. */
export function scopeOwner(panels: Panel[], s: FloatScope): Panel | null {
  let best: Panel | null = null;
  for (const p of panels) {
    if (!scopeInside(p, s)) continue;
    if (!best) {
      best = p;
      continue;
    }
    const a = panelArea(p);
    const b = panelArea(best);
    if (a < b || (a === b && p.plid < best.plid)) best = p;
  }
  return best;
}

/** A panel's scopes: the floating scopes this region owns. `panels` is the
 * whole list so nested regions resolve to the innermost one. */
export function panelScopes(
  panel: Panel,
  scopes: FloatScope[],
  panels: Panel[] = [panel],
): FloatScope[] {
  return scopes.filter((s) => scopeOwner(panels, s)?.plid === panel.plid);
}

// --------------------------------------------------------- canvas regions

const TAB_H = 19;
const TAB_FONT = '11px ui-monospace, monospace';
/** ui-monospace advance at 11px. Drawing and hit-testing share it so the
 * tab the player clicks is exactly the tab that was drawn. */
const CHAR_W = 6.7;
const CLOSE_W = 17;

function tabRect(cam: Camera, p: Panel): [number, number, number, number] {
  const w = Math.max(72, p.name.length * CHAR_W + 18 + CLOSE_W);
  return [cam.ox + p.x0 * cam.scale, cam.oy + p.y0 * cam.scale - TAB_H - 3, w, TAB_H];
}

/** Rounded-rect path. Exported so the schematic's panel-owned-scope
 * placeholder is drawn with exactly the region chrome's corner. */
export function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  const k = Math.max(0, Math.min(r, Math.min(w, h) / 2));
  ctx.beginPath();
  ctx.moveTo(x + k, y);
  ctx.arcTo(x + w, y, x + w, y + h, k);
  ctx.arcTo(x + w, y + h, x, y + h, k);
  ctx.arcTo(x, y + h, x, y, k);
  ctx.arcTo(x, y, x + w, y, k);
  ctx.closePath();
}

/** Draw every panel region: dotted rounded rect plus its name tab. */
export function drawPanelRegions(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  panels: Panel[],
  hoverPlid: number | null = null,
) {
  if (panels.length === 0) return;
  ctx.save();
  ctx.font = TAB_FONT;
  ctx.textBaseline = 'middle';
  for (const p of panels) {
    const hot = p.plid === hoverPlid;
    const X = cam.ox + p.x0 * cam.scale;
    const Y = cam.oy + p.y0 * cam.scale;
    const W = (p.x1 - p.x0) * cam.scale;
    const H = (p.y1 - p.y0) * cam.scale;
    roundRectPath(ctx, X, Y, W, H, Math.min(16, cam.scale * 0.5));
    ctx.fillStyle = hot ? '#4ad4ff12' : '#4ad4ff08';
    ctx.fill();
    ctx.setLineDash([5, 6]);
    ctx.lineWidth = hot ? 2 : 1.4;
    ctx.strokeStyle = hot ? '#8ee7ff' : '#4a8ea8';
    ctx.stroke();
    ctx.setLineDash([]);

    const [tx, ty, tw, th] = tabRect(cam, p);
    roundRectPath(ctx, tx, ty, tw, th, 5);
    ctx.fillStyle = hot ? '#22333d' : '#18242d';
    ctx.fill();
    ctx.lineWidth = 1;
    ctx.strokeStyle = hot ? '#8ee7ff' : '#3b5c6b';
    ctx.stroke();
    ctx.fillStyle = '#c6e8f4';
    ctx.fillText(p.name, tx + 8, ty + th / 2 + 0.5);
    ctx.fillStyle = hot ? '#ff9a9a' : '#7f97a2';
    ctx.fillText('×', tx + tw - CLOSE_W + 5, ty + th / 2 + 0.5);
  }
  ctx.restore();
}

/** The in-progress region while the J tool is dragging. */
export function drawPanelGhost(ctx: CanvasRenderingContext2D, cam: Camera, a: Point, b: Point) {
  const X = cam.ox + Math.min(a[0], b[0]) * cam.scale;
  const Y = cam.oy + Math.min(a[1], b[1]) * cam.scale;
  const W = Math.abs(b[0] - a[0]) * cam.scale;
  const H = Math.abs(b[1] - a[1]) * cam.scale;
  ctx.save();
  roundRectPath(ctx, X, Y, W, H, Math.min(16, cam.scale * 0.5));
  ctx.fillStyle = '#4ad4ff10';
  ctx.fill();
  ctx.setLineDash([4, 5]);
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = '#8ee7ff';
  ctx.stroke();
  ctx.restore();
}

export type PanelZone = { panel: Panel; zone: 'tab' | 'close' };

/** Hit-test the name tabs (the region body stays click-through so the
 * schematic underneath keeps working normally). */
export function panelZoneAt(
  cam: Camera,
  panels: Panel[],
  x: number,
  y: number,
): PanelZone | null {
  for (let k = panels.length - 1; k >= 0; k--) {
    const p = panels[k]!;
    const [tx, ty, tw, th] = tabRect(cam, p);
    if (x >= tx && x <= tx + tw && y >= ty && y <= ty + th) {
      return { panel: p, zone: x >= tx + tw - CLOSE_W ? 'close' : 'tab' };
    }
  }
  return null;
}

// ------------------------------------------------------------- local prefs

const LS = 'eepanel';
const lsGet = (key: string): string | null => {
  try {
    return localStorage.getItem(`${LS}:${key}`);
  } catch {
    return null; // private mode / storage disabled
  }
};
const lsSet = (key: string, value: string) => {
  try {
    localStorage.setItem(`${LS}:${key}`, value);
  } catch {
    /* ignore */
  }
};

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

function readPos(plid: number): { x: number; y: number } {
  const raw = lsGet(`${plid}:pos`);
  if (raw) {
    try {
      const p = JSON.parse(raw) as { x: number; y: number };
      if (Number.isFinite(p.x) && Number.isFinite(p.y)) {
        return {
          x: clamp(p.x, 0, Math.max(0, window.innerWidth - 120)),
          y: clamp(p.y, 0, Math.max(0, window.innerHeight - 40)),
        };
      }
    } catch {
      /* fall through to the default cascade */
    }
  }
  return {
    x: clamp(48 + ((plid * 28) % 200), 0, Math.max(0, window.innerWidth - 300)),
    y: clamp(96 + ((plid * 36) % 220), 0, Math.max(0, window.innerHeight - 200)),
  };
}

function readOrder(plid: number): string[] {
  const raw = lsGet(`${plid}:order`);
  if (!raw) return [];
  try {
    const a = JSON.parse(raw) as unknown;
    return Array.isArray(a) ? a.filter((k): k is string => typeof k === 'string') : [];
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------- widgets

const fmtSI = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (!Number.isFinite(v)) return `— ${unit}`;
  if (a >= 1000) return `${(v / 1000).toFixed(2)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(1)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(1)} µ${unit}`;
  return `0.00 ${unit}`;
};

export interface PanelHostDeps {
  /** The document (server truth when online). */
  elements(): ElementSpec[];
  /** Latest solver frame by element id — the only source of shown numbers. */
  live(): Map<number, ElemLive>;
  probes(): Probe[];
  /** Sample history for the scope widgets: the very store the canvas scopes
   * draw, so a scope shows the same waveform wherever it is displayed. */
  traces(): TraceStore;
  /** Every client-local floating scope. main.ts owns the array; panels only
   * borrow the ones their region contains. */
  scopes(): FloatScope[];
  /** Delete a floating scope (a panel-owned scope has no canvas chrome). */
  removeScope(sid: number): void;
  /** Same interact path the canvas uses (optimistic + server echo). */
  interact(e: ElementSpec, op: InteractOp): void;
  /** Panel ops (rename / delete from the window chrome). */
  op(op: PanelOp): void;
}

interface TickCtx {
  elems: ElementSpec[];
  byId: Map<number, ElementSpec>;
  live: Map<number, ElemLive>;
  probes: Probe[];
  /** The whole shared list: scope ownership is resolved against all regions. */
  panels: Panel[];
  scopes: FloatScope[];
  traces: TraceStore;
}

interface Widget {
  key: string;
  el: HTMLDivElement;
  update(ctx: TickCtx): void;
}

interface RowParts {
  el: HTMLDivElement;
  lab: HTMLSpanElement;
  ctl: HTMLDivElement;
  val: HTMLSpanElement;
}

function makeRow(key: string, label: string): RowParts {
  const el = document.createElement('div');
  el.className = 'prow';
  el.dataset.key = key;
  const grip = document.createElement('span');
  grip.className = 'prow-grip';
  grip.textContent = '⠿';
  grip.title = 'drag to reorder';
  const lab = document.createElement('span');
  lab.className = 'prow-label';
  lab.textContent = label;
  const ctl = document.createElement('div');
  ctl.className = 'prow-ctl';
  const val = document.createElement('span');
  val.className = 'prow-val';
  val.textContent = '—';
  el.append(grip, lab, ctl, val);
  return { el, lab, ctl, val };
}

/** Throttled InteractOp sender: 40 ms while dragging, immediate on release
 * (the same cadence the canvas value-drag uses). */
function sender(deps: PanelHostDeps, get: () => ElementSpec | undefined) {
  let last = 0;
  return (op: InteractOp, force: boolean) => {
    const e = get();
    if (!e) return;
    const now = performance.now();
    if (!force && now - last < 40) return;
    last = now;
    deps.interact(e, op);
  };
}

/** Potentiometer: a real slider, or a knob you drag — per-widget choice. */
function potWidget(plid: number, id: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`pot:${id}`, `POT #${id}`);
  let spec: ElementSpec | undefined;
  const push = sender(deps, () => spec);

  const slider = document.createElement('input');
  slider.className = 'pslider';
  slider.type = 'range';
  slider.min = '0.01';
  slider.max = '0.99';
  slider.step = '0.002';
  slider.value = '0.5';

  const knob = document.createElement('div');
  knob.className = 'pknob';
  knob.title = 'drag ↕ to turn';
  const needle = document.createElement('i');
  knob.appendChild(needle);

  const tog = document.createElement('button');
  tog.className = 'ptog';

  let mode: 'slider' | 'knob' = lsGet(`${plid}:mode:${id}`) === 'knob' ? 'knob' : 'slider';
  const applyMode = () => {
    slider.style.display = mode === 'slider' ? '' : 'none';
    knob.style.display = mode === 'knob' ? '' : 'none';
    tog.textContent = mode === 'slider' ? '◉ knob' : '▭ slider';
    tog.title = `switch to the ${mode === 'slider' ? 'knob' : 'slider'}`;
  };
  tog.onclick = () => {
    mode = mode === 'slider' ? 'knob' : 'slider';
    lsSet(`${plid}:mode:${id}`, mode);
    applyMode();
  };

  let dragging = false;
  const setWiper = (v: number, force: boolean) => {
    const w = clamp(v, 0.01, 0.99);
    slider.value = String(w);
    needle.style.transform = `rotate(${-135 + w * 270}deg)`;
    push({ t: 'SetValue', value: w }, force);
  };

  slider.addEventListener('pointerdown', () => (dragging = true));
  slider.addEventListener('input', () => setWiper(Number(slider.value), false));
  slider.addEventListener('change', () => {
    dragging = false;
    setWiper(Number(slider.value), true);
  });

  knob.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    const start = ev.clientY;
    const base = spec?.kind.t === 'Potentiometer' ? spec.kind.wiper : Number(slider.value);
    dragging = true;
    try {
      knob.setPointerCapture(ev.pointerId);
    } catch {
      /* synthetic pointers */
    }
    const move = (m: PointerEvent) => setWiper(base + (start - m.clientY) / 160, false);
    const up = (m: PointerEvent) => {
      knob.removeEventListener('pointermove', move);
      knob.removeEventListener('pointerup', up);
      knob.removeEventListener('pointercancel', up);
      dragging = false;
      setWiper(base + (start - m.clientY) / 160, true);
    };
    knob.addEventListener('pointermove', move);
    knob.addEventListener('pointerup', up);
    knob.addEventListener('pointercancel', up);
  });

  applyMode();
  ctl.append(slider, knob, tog);

  return {
    key: `pot:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'Potentiometer') return;
      lab.textContent = `POT #${id} ${fmtSI(k.ohms, 'Ω')}`;
      if (!dragging) {
        slider.value = String(k.wiper);
        needle.style.transform = `rotate(${-135 + k.wiper * 270}deg)`;
      }
      const l = ctx.live.get(id);
      // The wiper pin voltage is solver output, not a UI guess.
      val.textContent = `${(k.wiper * 100).toFixed(0)}% ${l ? fmtSI(l.v[1] ?? 0, 'V') : ''}`;
    },
  };
}

/** Switch / button: a toggle. */
function switchWidget(id: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`sw:${id}`, `SW #${id}`);
  let spec: ElementSpec | undefined;
  const btn = document.createElement('button');
  btn.className = 'pswitch';
  btn.textContent = 'OFF';
  btn.onclick = () => {
    const e = spec;
    if (e?.kind.t === 'Switch') deps.interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
  };
  ctl.appendChild(btn);
  return {
    key: `sw:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'Switch') return;
      lab.textContent = `SW #${id}`;
      btn.textContent = k.closed ? 'ON' : 'OFF';
      btn.classList.toggle('on', k.closed);
      const l = ctx.live.get(id);
      val.textContent = l ? fmtSI(l.i[0] ?? 0, 'A') : '—';
    },
  };
}

/** Lamp / LED: an indicator whose brightness tracks live power or current. */
function indicatorWidget(id: number): Widget {
  const { el, lab, ctl, val } = makeRow(`ind:${id}`, `#${id}`);
  const dot = document.createElement('span');
  dot.className = 'pdot';
  ctl.appendChild(dot);
  return {
    key: `ind:${id}`,
    el,
    update(ctx) {
      const k = ctx.byId.get(id)?.kind;
      const l = ctx.live.get(id);
      let bright = 0;
      let color = '#ffd66b';
      if (k?.t === 'Lamp') {
        // Same normalization the schematic symbol uses: P / rated W.
        bright = clamp(Math.abs(l?.power ?? 0) / Math.max(1e-9, k.rated_watts), 0, 1);
        lab.textContent = `LAMP #${id}`;
        val.textContent = l ? fmtSI(l.power, 'W') : '—';
      } else if (k?.t === 'Led') {
        bright = clamp(Math.abs(l?.i[0] ?? 0) / 0.02, 0, 1);
        color = LED_COLORS[k.color] ?? LED_COLORS[0]!;
        lab.textContent = `LED #${id}`;
        val.textContent = l ? fmtSI(l.i[0] ?? 0, 'A') : '—';
      }
      dot.style.background = color;
      dot.style.opacity = String(0.16 + 0.84 * bright);
      dot.style.boxShadow = bright > 0.02 ? `0 0 ${(3 + bright * 15).toFixed(1)}px ${color}` : 'none';
    },
  };
}

/** DC voltage source: numeric entry plus a slider. */
function sourceWidget(id: number, dc0: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`src:${id}`, `SRC #${id}`);
  let spec: ElementSpec | undefined;
  const push = sender(deps, () => spec);
  let span = Math.max(12, Math.ceil(Math.abs(dc0) * 2));

  const num = document.createElement('input');
  num.className = 'pnum';
  num.type = 'number';
  num.step = 'any';
  const slider = document.createElement('input');
  slider.className = 'pslider';
  slider.type = 'range';
  slider.step = '0.05';
  const applySpan = () => {
    slider.min = String(-span);
    slider.max = String(span);
  };
  applySpan();

  let dragging = false;
  const set = (v: number, force: boolean) => {
    if (!Number.isFinite(v)) return;
    slider.value = String(v);
    num.value = String(Number(v.toFixed(3)));
    push({ t: 'SetValue', value: v }, force);
  };
  slider.addEventListener('pointerdown', () => (dragging = true));
  slider.addEventListener('input', () => set(Number(slider.value), false));
  slider.addEventListener('change', () => {
    dragging = false;
    set(Number(slider.value), true);
  });
  num.addEventListener('change', () => {
    const v = Number(num.value);
    if (!Number.isFinite(v)) return;
    span = Math.max(span, Math.ceil(Math.abs(v) * 1.2));
    applySpan();
    set(v, true);
  });
  ctl.append(num, slider);

  return {
    key: `src:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'VoltageSource') return;
      lab.textContent = `SRC #${id}`;
      if (Math.abs(k.dc) > span) {
        span = Math.ceil(Math.abs(k.dc) * 1.2);
        applySpan();
      }
      if (!dragging && document.activeElement !== num) {
        slider.value = String(k.dc);
        num.value = String(Number(k.dc.toFixed(3)));
      }
      const l = ctx.live.get(id);
      val.textContent = l ? fmtSI(l.i[0] ?? 0, 'A') : '—';
    },
  };
}

/** A probe on a member element becomes a fixed voltmeter / ammeter. Waveforms
 * come from `scopeWidget` below: park an oscilloscope inside the region and it
 * moves into this window. */
function probeWidget(pid: number): Widget {
  const { el, lab, ctl, val } = makeRow(`pr:${pid}`, `METER ${pid}`);
  const seg = document.createElement('span');
  seg.className = 'pseg';
  seg.style.color = probeColor(pid);
  seg.textContent = '–.––';
  ctl.appendChild(seg);
  return {
    key: `pr:${pid}`,
    el,
    update(ctx) {
      const p = ctx.probes.find((q) => q.pid === pid);
      if (!p) {
        seg.textContent = '–.––';
        return;
      }
      const unit = p.kind === 'v' ? 'V' : 'A';
      lab.textContent = `${p.kind === 'v' ? 'VOLT' : 'AMP'} ${pid}`;
      const l = ctx.live.get(p.elem);
      if (!l) {
        seg.textContent = `–.–– ${unit}`;
        val.textContent = `#${p.elem}.${p.pin}`;
        return;
      }
      let v = p.kind === 'v' ? (l.v[p.pin] ?? 0) : (l.i[p.pin] ?? 0);
      if (p.kind === 'v' && p.r) v -= ctx.live.get(p.r[0])?.v[p.r[1]] ?? 0;
      seg.textContent = fmtSI(v, unit);
      val.textContent = `#${p.elem}.${p.pin}${p.r ? ' Δ' : ''}`;
    },
  };
}

/** An oscilloscope parked inside this region: the panel IS its enclosure, so
 * the instrument is drawn into a row canvas instead of onto the schematic.
 *
 * It is the same instrument, not a copy: the widget looks the scope up by sid
 * every tick and renders from main.ts's TraceStore with the scope's own
 * ScopeSettings object, so timebase / trigger / manual scale are shared with
 * the canvas (drag it out of the region and it keeps every knob).
 *
 * `renderScope` is `renderScopeInto(ctx, 0, 0, W, H, …)` over a whole canvas
 * plus the fractional-devicePixelRatio backing-store fix — one copy of that
 * sizing logic beats two. */
function scopeWidget(sid: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`sc:${sid}`, `SCOPE ${sid}`);
  el.classList.add('prow-scope');

  const chans = document.createElement('div');
  chans.className = 'pchans';
  chans.title = 'channels: click a probe to show/hide it';
  const kill = document.createElement('button');
  kill.className = 'ptog';
  kill.textContent = '×';
  kill.title = 'delete this oscilloscope';
  kill.onclick = () => deps.removeScope(sid);
  ctl.append(chans, kill);

  const cv = document.createElement('canvas');
  cv.className = 'pscope';
  el.appendChild(cv);

  let scope: FloatScope | undefined;
  let probes: Probe[] = [];
  const active = () => (scope ? scopeChannels(scope, probes) : []);

  /** Body-local coordinates: `.pscope` has no border or padding, so the
   * element's own box is exactly the rect renderScopeInto drew into. */
  const ctrlAt = (ev: { clientX: number; clientY: number }): ScopeControlId | null => {
    if (!scope) return null;
    const r = cv.getBoundingClientRect();
    return scopeControlAt(
      cv.clientWidth,
      cv.clientHeight,
      ev.clientX - r.left,
      ev.clientY - r.top,
      scope.set,
      active().length,
    );
  };
  cv.addEventListener('pointerdown', (ev) => {
    const id = ctrlAt(ev);
    if (!scope || !id) return;
    ev.preventDefault();
    applyScopeControl(scope.set, id, active().length);
  });
  cv.addEventListener('pointermove', (ev) => {
    cv.style.cursor = ctrlAt(ev) ? 'pointer' : 'default';
  });
  // Wheel over a scope is its timebase, wherever the scope is drawn.
  cv.addEventListener(
    'wheel',
    (ev) => {
      if (!scope) return;
      ev.preventDefault();
      ev.stopPropagation();
      scope.set.timebase = clamp(scope.set.timebase * Math.exp(ev.deltaY * 0.001), 0.001, 60);
    },
    { passive: false },
  );

  const toggle = (pid: number) => {
    const s = scope;
    if (!s) return;
    if (s.pids === null) s.pids = probes.map((p) => p.pid);
    s.pids = s.pids.includes(pid) ? s.pids.filter((x) => x !== pid) : [...s.pids, pid];
  };

  // The canvas scope's channel dots live in its title bar, which a panel-owned
  // scope does not have: rebuild them here whenever the selection changes.
  let chanSig = '';
  const syncChans = () => {
    const on = new Set(active().map((p) => p.pid));
    const sig = `${probes.map((p) => `${p.pid}${p.kind}`).join(',')}|${[...on].join(',')}`;
    if (sig === chanSig) return;
    chanSig = sig;
    chans.replaceChildren(
      ...probes.map((p) => {
        const b = document.createElement('button');
        b.className = on.has(p.pid) ? 'pchan on' : 'pchan';
        b.style.color = probeColor(p.pid);
        b.title = `${p.kind === 'v' ? 'V' : 'I'}${p.pid}`;
        b.onclick = () => toggle(p.pid);
        return b;
      }),
    );
  };

  return {
    key: `sc:${sid}`,
    el,
    update(ctx) {
      scope = ctx.scopes.find((s) => s.sid === sid);
      probes = ctx.probes;
      if (!scope) return;
      lab.textContent = `SCOPE ${sid}`;
      syncChans();
      val.textContent = `${fmtSI(scope.set.timebase / 10, 's')}/div`;
      renderScope(cv, ctx.traces, active(), scope.set.timebase, scope.set);
    },
  };
}

type WidgetSpec = { key: string; make: () => Widget };

/** Which widgets a panel's members deserve, in document order. */
function widgetSpecs(
  plid: number,
  members: ElementSpec[],
  probes: Probe[],
  scopes: FloatScope[],
  deps: PanelHostDeps,
): WidgetSpec[] {
  const out: WidgetSpec[] = [];
  const ids = new Set(members.map((e) => e.id));
  for (const e of members) {
    const id = e.id;
    switch (e.kind.t) {
      case 'Potentiometer':
        out.push({ key: `pot:${id}`, make: () => potWidget(plid, id, deps) });
        break;
      case 'Switch':
        out.push({ key: `sw:${id}`, make: () => switchWidget(id, deps) });
        break;
      case 'Lamp':
      case 'Led':
        out.push({ key: `ind:${id}`, make: () => indicatorWidget(id) });
        break;
      case 'VoltageSource': {
        // AC sources have no single knob to offer; DC ones do.
        const dc = e.kind.dc;
        if (e.kind.amp === 0) out.push({ key: `src:${id}`, make: () => sourceWidget(id, dc, deps) });
        break;
      }
      default:
        break;
    }
  }
  for (const p of probes) {
    if (ids.has(p.elem)) out.push({ key: `pr:${p.pid}`, make: () => probeWidget(p.pid) });
  }
  // Oscilloscopes the region encloses; keyed by sid so the saved row order
  // treats them like any other widget.
  for (const s of scopes) out.push({ key: `sc:${s.sid}`, make: () => scopeWidget(s.sid, deps) });
  return out;
}

// -------------------------------------------------------- control windows

class PanelWindow {
  readonly el: HTMLDivElement;
  private title: HTMLInputElement;
  private body: HTMLDivElement;
  private hint: HTMLDivElement;
  private widgets = new Map<string, Widget>();
  private sig = '';
  private name = '';

  constructor(
    private plid: number,
    private deps: PanelHostDeps,
    root: HTMLElement,
  ) {
    this.el = document.createElement('div');
    this.el.className = 'pwin';
    const hd = document.createElement('div');
    hd.className = 'pwin-hd';
    const grab = document.createElement('span');
    grab.className = 'pwin-grab';
    grab.textContent = '⣿';
    this.title = document.createElement('input');
    this.title.className = 'pwin-title';
    this.title.spellcheck = false;
    this.title.title = 'rename this panel';
    const close = document.createElement('button');
    close.className = 'pwin-x';
    close.textContent = '×';
    close.title = 'delete this panel';
    close.onclick = () => deps.op({ t: 'remove', plid });
    hd.append(grab, this.title, close);

    this.body = document.createElement('div');
    this.body.className = 'pwin-body';
    this.hint = document.createElement('div');
    this.hint.className = 'pwin-hint';
    this.hint.textContent =
      'no controls in this region — enclose a pot, switch, lamp, LED, DC source, ' +
      'probe or oscilloscope';
    this.el.append(hd, this.body, this.hint);
    root.appendChild(this.el);

    const pos = readPos(plid);
    this.el.style.left = `${pos.x}px`;
    this.el.style.top = `${pos.y}px`;

    hd.addEventListener('pointerdown', (ev) => {
      if (ev.target === close || ev.target === this.title) return;
      ev.preventDefault();
      const [sx, sy] = [ev.clientX, ev.clientY];
      const [ox, oy] = [this.el.offsetLeft, this.el.offsetTop];
      try {
        hd.setPointerCapture(ev.pointerId);
      } catch {
        /* synthetic pointers */
      }
      const move = (m: PointerEvent) => {
        this.el.style.left = `${clamp(ox + m.clientX - sx, 0, window.innerWidth - 90)}px`;
        this.el.style.top = `${clamp(oy + m.clientY - sy, 0, window.innerHeight - 30)}px`;
      };
      const up = () => {
        hd.removeEventListener('pointermove', move);
        hd.removeEventListener('pointerup', up);
        hd.removeEventListener('pointercancel', up);
        lsSet(`${plid}:pos`, JSON.stringify({ x: this.el.offsetLeft, y: this.el.offsetTop }));
      };
      hd.addEventListener('pointermove', move);
      hd.addEventListener('pointerup', up);
      hd.addEventListener('pointercancel', up);
    });

    this.title.addEventListener('change', () => {
      const v = this.title.value.trim();
      if (v && v !== this.name) deps.op({ t: 'rename', plid, name: v });
      else this.title.value = this.name;
    });
    this.title.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') this.title.blur();
      if (ev.key === 'Escape') {
        this.title.value = this.name;
        this.title.blur();
      }
    });
  }

  destroy() {
    this.el.remove();
  }

  update(panel: Panel, ctx: TickCtx) {
    this.name = panel.name;
    if (document.activeElement !== this.title) this.title.value = panel.name;
    const specs = widgetSpecs(
      this.plid,
      panelMembers(panel, ctx.elems),
      ctx.probes,
      panelScopes(panel, ctx.scopes, ctx.panels),
      this.deps,
    );
    const sig = specs.map((s) => s.key).join('|');
    if (sig !== this.sig) {
      this.sig = sig;
      this.rebuild(specs);
    }
    for (const w of this.widgets.values()) w.update(ctx);
    this.hint.style.display = this.widgets.size === 0 ? 'block' : 'none';
  }

  /** Member set changed: re-lay the rows, honoring the saved drag order and
   * keeping live widget elements (so a slider mid-drag survives). */
  private rebuild(specs: WidgetSpec[]) {
    const order = readOrder(this.plid);
    const rank = (k: string) => {
      const i = order.indexOf(k);
      return i < 0 ? order.length + 1 : i;
    };
    const sorted = [...specs].sort(
      (a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key),
    );
    const old = this.widgets;
    this.widgets = new Map();
    for (const s of sorted) {
      const w = old.get(s.key) ?? s.make();
      this.widgets.set(s.key, w);
      this.wireRowDrag(w.el);
      this.body.appendChild(w.el); // appending an existing child re-orders it
    }
    for (const [key, w] of old) if (!this.widgets.has(key)) w.el.remove();
  }

  /** Rows are re-orderable by dragging their grip; order persists per plid. */
  private wireRowDrag(row: HTMLDivElement) {
    const grip = row.querySelector('.prow-grip');
    if (!(grip instanceof HTMLElement) || grip.dataset.wired) return;
    grip.dataset.wired = '1';
    grip.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      try {
        grip.setPointerCapture(ev.pointerId);
      } catch {
        /* synthetic pointers */
      }
      row.classList.add('dragging');
      const move = (m: PointerEvent) => {
        let before: HTMLElement | null = null;
        for (const c of this.body.children) {
          if (!(c instanceof HTMLElement) || c === row) continue;
          const b = c.getBoundingClientRect();
          if (m.clientY < b.top + b.height / 2) {
            before = c;
            break;
          }
        }
        this.body.insertBefore(row, before);
      };
      const up = () => {
        grip.removeEventListener('pointermove', move);
        grip.removeEventListener('pointerup', up);
        grip.removeEventListener('pointercancel', up);
        row.classList.remove('dragging');
        this.persistOrder();
      };
      grip.addEventListener('pointermove', move);
      grip.addEventListener('pointerup', up);
      grip.addEventListener('pointercancel', up);
    });
  }

  private persistOrder() {
    const keys: string[] = [];
    for (const c of this.body.children) {
      if (c instanceof HTMLElement && c.dataset.key) keys.push(c.dataset.key);
    }
    lsSet(`${this.plid}:order`, JSON.stringify(keys));
    const reordered = new Map<string, Widget>();
    for (const k of keys) {
      const w = this.widgets.get(k);
      if (w) reordered.set(k, w);
    }
    this.widgets = reordered;
  }
}

/** Owns every panel control window: one per shared panel region. */
export class PanelHost {
  private root: HTMLElement;
  private wins = new Map<number, PanelWindow>();

  constructor(private deps: PanelHostDeps) {
    const found = document.getElementById('panels');
    if (found) {
      this.root = found;
    } else {
      const d = document.createElement('div');
      d.id = 'panels';
      document.body.appendChild(d);
      this.root = d;
    }
  }

  /** True for events originating inside a panel window — main.ts uses this
   * to keep canvas hotkeys out of panel text inputs. */
  owns(target: EventTarget | null): boolean {
    return target instanceof Node && this.root.contains(target);
  }

  /** Called once per frame: sync windows to the shared list, then refresh
   * every widget from the latest solver frame. */
  tick(panels: Panel[]) {
    const alive = new Set(panels.map((p) => p.plid));
    for (const [plid, w] of [...this.wins]) {
      if (!alive.has(plid)) {
        w.destroy();
        this.wins.delete(plid);
      }
    }
    if (panels.length === 0) return;
    const elems = this.deps.elements();
    const ctx: TickCtx = {
      elems,
      byId: new Map(elems.map((e) => [e.id, e])),
      live: this.deps.live(),
      probes: this.deps.probes(),
      panels,
      scopes: this.deps.scopes(),
      traces: this.deps.traces(),
    };
    for (const p of panels) {
      let w = this.wins.get(p.plid);
      if (!w) {
        w = new PanelWindow(p.plid, this.deps, this.root);
        this.wins.set(p.plid, w);
      }
      w.update(p, ctx);
    }
  }
}
