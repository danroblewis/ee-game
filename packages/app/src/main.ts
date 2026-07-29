// EE Game client. The sim runs the moment the page loads — no run button.
// Online: this browser renders the server's authoritative sim and sends
// interactions/edits. Offline: the same engine runs locally in WASM.
//
// Player controls:
//   wheel                 zoom to cursor (over a scope: its timebase)
//   middle/right/space    pan
//   drag from pin/empty   draw wire;  drag on body = move (knobs: value)
//   click                 select part or probe flag (properties panel)
//   /                     parts palette;  R rotate selected part
//   hover + V / I         toggle voltage probe / current clamp
//   hover + G             set selected V-probe's reference (differential)
//   O                     drop an in-place oscilloscope at the cursor
//   hover + X / Delete    remove part

import init, { Sim } from './wasm/sim_wasm';
import {
  demoCircuit,
  pinLabels,
  unpackFrame,
  type DocOp,
  type ElementSpec,
  type ElemLive,
  type InteractOp,
  type Point,
} from './circuit';
import { makePins, searchParts, type PartDef } from './catalog';
import { connect } from './net';
import { DotFlow, drawElement, hitTest, type Camera } from './render';
import { probeColor, renderScope, renderScopeInto, TraceStore, type Probe } from './scope';

const DT = 10e-6;
const MAX_STEPS_PER_FRAME = 4000; // local-mode wall budget

await init();

// ---------------------------------------------------------------- state
let elements: ElementSpec[] = demoCircuit();
let live = new Map<number, ElemLive>();
let simTime = 0;
let online = false;
let population = 0;
let myId = -1;
const cursors = new Map<number, { x: number; y: number; seen: number }>();

let probes: Probe[] = [];
const traces = new TraceStore();
let localPidCounter = 1;
let scopeTimebase = 5; // docked panel, seconds across

type Selection = { t: 'elem'; id: number } | { t: 'probe'; pid: number } | null;
let selected: Selection = null;

/** In-place oscilloscopes: world-anchored, per-player instruments. */
interface FloatScope {
  sid: number;
  x: number; // grid units
  y: number;
  w: number;
  h: number;
  tb: number; // seconds across
  pids: number[] | null; // null = all probes
}
let floatScopes: FloatScope[] = [];
let sidCounter = 1;

const localSim = new Sim(DT);
localSim.setElements(elements);

function applyOp(e: ElementSpec, op: InteractOp) {
  if (op.t === 'SetSwitch' && e.kind.t === 'Switch') e.kind.closed = op.closed;
  if (op.t === 'SetValue') {
    if (e.kind.t === 'Resistor' || e.kind.t === 'Lamp') e.kind.ohms = op.value;
    else if (e.kind.t === 'Capacitor') e.kind.farads = op.value;
    else if (e.kind.t === 'Inductor') e.kind.henries = op.value;
    else if (e.kind.t === 'VoltageSource') e.kind.dc = op.value;
    else if (e.kind.t === 'CurrentSource') e.kind.amps = op.value;
    else if (e.kind.t === 'Potentiometer') e.kind.wiper = Math.min(0.99, Math.max(0.01, op.value));
  }
}

function applyDoc(op: DocOp) {
  if (op.t === 'Add') {
    if (!elements.some((e) => e.id === op.spec.id)) elements.push(op.spec);
  } else if (op.t === 'Remove') {
    elements = elements.filter((e) => e.id !== op.id);
    if (selected?.t === 'elem' && selected.id === op.id) selected = null;
  } else if (op.t === 'Move') {
    const e = elements.find((x) => x.id === op.id);
    if (e) e.pins = op.pins;
  } else if (op.t === 'SetKind') {
    const e = elements.find((x) => x.id === op.id);
    if (e) e.kind = op.kind;
  }
}

let idCounter = 1;
const newId = () => (myId > 0 ? myId : 999) * 1_000_000 + idCounter++;

let firstHello = true;
const net = connect({
  onHello(you, serverElements, serverProbes) {
    online = true;
    myId = you;
    elements = serverElements;
    probes = serverProbes;
    live = new Map();
    // Only auto-fit on the first join — a mid-session reconnect must not
    // yank the camera away from where the player is working.
    if (firstHello) {
      firstHello = false;
      fitCamera();
    }
  },
  onFrame(f) {
    simTime = f.time;
    const m = new Map<number, ElemLive>();
    for (const [id, npins, v0, v1, v2, i0, i1, i2, power] of f.e) {
      m.set(id, { id, npins, v: [v0, v1, v2], i: [i0, i1, i2], power });
    }
    live = m;
  },
  onOp(id, op) {
    const e = elements.find((x) => x.id === id);
    if (e) applyOp(e, op);
  },
  onDoc(op) {
    applyDoc(op); // idempotent for our own echoes
  },
  onProbes(list) {
    probes = list;
    const alive = new Set(list.map((p) => p.pid));
    traces.prune(alive);
    if (selected?.t === 'probe' && !alive.has(selected.pid)) selected = null;
    for (const s of floatScopes) if (s.pids) s.pids = s.pids.filter((pid) => alive.has(pid));
  },
  onSamples(t0, dts, s) {
    for (const [pid, samples] of Object.entries(s)) {
      traces.appendChunk(Number(pid), t0, dts, samples);
    }
  },
  onPresence(n) {
    population = n;
  },
  onCursor(who, x, y) {
    if (who !== myId) cursors.set(who, { x, y, seen: performance.now() });
  },
  onClose() {
    if (online) {
      // Server went away: carry on locally from the same document.
      online = false;
      localSim.setElements(elements);
    }
  },
});

function interact(e: ElementSpec, op: InteractOp) {
  applyOp(e, op); // optimistic; server echo confirms
  if (online) net.sendInteract(e.id, op);
  else localSim.interact(e.id, op);
}

function editDoc(op: DocOp) {
  applyDoc(op); // optimistic
  if (online) net.sendEdit(op);
  else localSim.setElements(elements);
}

/** Toggle a probe. Online the server owns the list; offline we mirror
 * the same toggle semantics locally. */
function toggleProbe(elem: number, pin: number, kind: 'v' | 'i') {
  if (online) {
    net.sendProbe(elem, pin, kind);
    return;
  }
  const k = probes.findIndex((p) => p.elem === elem && p.pin === pin && p.kind === kind);
  if (k >= 0) probes.splice(k, 1);
  else if (probes.length < 8) probes.push({ pid: localPidCounter++, elem, pin, kind });
  traces.prune(new Set(probes.map((p) => p.pid)));
}

function setProbeRef(pid: number, elem: number, pin: number) {
  if (online) {
    net.sendProbeRef(pid, elem, pin);
    return;
  }
  const p = probes.find((x) => x.pid === pid);
  if (!p) return;
  p.r = p.r && p.r[0] === elem && p.r[1] === pin ? null : [elem, pin];
}

/** Pin index of `e` nearest to the cursor. */
function nearestPin(e: ElementSpec, x: number, y: number): number {
  let best = 0;
  let bestD = Infinity;
  e.pins.forEach((p, k) => {
    const d = Math.hypot(cam.ox + p[0] * cam.scale - x, cam.oy + p[1] * cam.scale - y);
    if (d < bestD) {
      bestD = d;
      best = k;
    }
  });
  return best;
}

// ---------------------------------------------------------------- canvas
const canvas = document.getElementById('canvas') as HTMLCanvasElement;
const hud = document.getElementById('hud') as HTMLDivElement;
const tip = document.getElementById('tip') as HTMLDivElement;
const ctx = canvas.getContext('2d')!;

const cam: Camera = { scale: 48, ox: 60, oy: 60 };
// Exposed for end-to-end tests: lets them convert grid coords to pixels
// without replicating camera math.
(window as unknown as { __cam: Camera }).__cam = cam;
const dots = new DotFlow();
let mouse: { x: number; y: number } | null = null;

const toGrid = (x: number, y: number): [number, number] => [
  (x - cam.ox) / cam.scale,
  (y - cam.oy) / cam.scale,
];
const snap = (x: number, y: number): Point => {
  const [gx, gy] = toGrid(x, y);
  return [Math.round(gx), Math.round(gy)];
};

function fitCamera() {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const e of elements) {
    for (const p of e.pins) {
      x0 = Math.min(x0, p[0]);
      y0 = Math.min(y0, p[1]);
      x1 = Math.max(x1, p[0]);
      y1 = Math.max(y1, p[1]);
    }
  }
  if (!isFinite(x0)) {
    cam.scale = 48;
    cam.ox = window.innerWidth / 2;
    cam.oy = window.innerHeight / 2;
    return;
  }
  const w = x1 - x0 + 4;
  const ht = y1 - y0 + 4;
  cam.scale = Math.max(20, Math.min(window.innerWidth / w, window.innerHeight / ht));
  cam.ox = (window.innerWidth - (x0 + x1) * cam.scale) / 2;
  cam.oy = (window.innerHeight - (y0 + y1) * cam.scale) / 2;
}

function resize() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = window.innerWidth * dpr;
  canvas.height = window.innerHeight * dpr;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}
window.addEventListener('resize', resize);
resize();
fitCamera();

// ---------------------------------------------------------------- palette
const palette = document.getElementById('palette') as HTMLDivElement;
const psearch = document.getElementById('psearch') as HTMLInputElement;
const plist = document.getElementById('plist') as HTMLDivElement;
const pbtn = document.getElementById('pbtn') as HTMLButtonElement;

let placing: PartDef | null = null;

function renderPaletteList() {
  const parts = searchParts(psearch.value);
  plist.innerHTML = '';
  parts.forEach((p, k) => {
    const row = document.createElement('div');
    row.textContent = p.name;
    if (k === 0) row.className = 'sel';
    row.onclick = () => choosePart(p);
    plist.appendChild(row);
  });
}
function openPalette() {
  palette.style.display = 'block';
  psearch.value = '';
  renderPaletteList();
  psearch.focus();
}
function closePalette() {
  palette.style.display = 'none';
  psearch.blur();
}
function choosePart(p: PartDef) {
  placing = p;
  closePalette();
  canvas.style.cursor = 'crosshair';
}
pbtn.onclick = () => (palette.style.display === 'block' ? closePalette() : openPalette());
psearch.oninput = renderPaletteList;
psearch.onkeydown = (ev) => {
  if (ev.key === 'Enter') {
    const top = searchParts(psearch.value)[0];
    if (top) choosePart(top);
  } else if (ev.key === 'Escape') {
    closePalette();
  }
  ev.stopPropagation();
};

// ---------------------------------------------------------------- props
const propsDiv = document.getElementById('props') as HTMLDivElement;
let propsShownFor = ''; // JSON key of what the panel currently shows

const FIELD_LABELS: Record<string, string> = {
  ohms: 'resistance Ω',
  rated_watts: 'rated W',
  farads: 'capacitance F',
  henries: 'inductance H',
  dc: 'DC volts',
  amp: 'AC amplitude V',
  hz: 'frequency Hz',
  phase: 'phase rad',
  amps: 'current A',
  closed: 'closed',
  vz: 'zener V',
  color: 'color 0-4',
  beta: 'beta',
  vt: 'threshold V',
  k: 'k A/V²',
  rail: 'rail ±V',
  wiper: 'wiper 0-1',
};

function syncPropsPanel() {
  const target =
    selected?.t === 'elem' ? elements.find((e) => e.id === (selected as { id: number }).id) : undefined;
  if (!target) {
    propsDiv.style.display = 'none';
    propsShownFor = '';
    return;
  }
  const key = JSON.stringify([target.id, target.kind]);
  if (key === propsShownFor) return;
  // Don't yank the DOM out from under an actively-editing user.
  if (propsDiv.contains(document.activeElement) && propsShownFor.startsWith(`[${target.id},`)) {
    return;
  }
  propsShownFor = key;
  propsDiv.style.display = 'block';
  propsDiv.innerHTML = '';
  const h = document.createElement('h3');
  h.textContent = `${target.kind.t}  #${target.id}`;
  propsDiv.appendChild(h);

  for (const [field, value] of Object.entries(target.kind)) {
    if (field === 't') continue;
    const label = document.createElement('label');
    const span = document.createElement('span');
    span.textContent = FIELD_LABELS[field] ?? field;
    label.appendChild(span);
    const input = document.createElement('input');
    if (typeof value === 'boolean') {
      input.type = 'checkbox';
      input.checked = value;
      input.onchange = () => {
        const kind = { ...target.kind, [field]: input.checked } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        propsShownFor = JSON.stringify([target.id, kind]);
      };
    } else {
      input.type = 'number';
      input.step = 'any';
      input.value = String(value);
      input.onchange = () => {
        const num = Number(input.value);
        if (!Number.isFinite(num)) return;
        const kind = { ...target.kind, [field]: num } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        propsShownFor = JSON.stringify([target.id, kind]);
      };
    }
    label.appendChild(input);
    propsDiv.appendChild(label);
  }

  const row = document.createElement('div');
  row.className = 'row';
  const rot = document.createElement('button');
  rot.textContent = '⟳ rotate (R)';
  rot.onclick = () => rotateSelected();
  const del = document.createElement('button');
  del.textContent = '✕ delete';
  del.onclick = () => editDoc({ t: 'Remove', id: target.id });
  row.appendChild(rot);
  row.appendChild(del);
  propsDiv.appendChild(row);
}

function rotateSelected() {
  if (selected?.t !== 'elem') return;
  const e = elements.find((x) => x.id === (selected as { id: number }).id);
  if (!e) return;
  const cx = Math.round(e.pins.reduce((s, p) => s + p[0], 0) / e.pins.length);
  const cy = Math.round(e.pins.reduce((s, p) => s + p[1], 0) / e.pins.length);
  // 90° clockwise about the centroid: (dx, dy) -> (-dy, dx).
  const pins = e.pins.map(([x, y]) => [cx - (y - cy), cy + (x - cx)] as Point);
  editDoc({ t: 'Move', id: e.id, pins });
}

// ---------------------------------------------------------------- input
function elementAt(x: number, y: number): ElementSpec | undefined {
  let best: ElementSpec | undefined;
  let bestD = 14;
  for (const e of elements) {
    const d = hitTest(cam, e, x, y);
    if (d < bestD) {
      bestD = d;
      best = e;
    }
  }
  return best;
}

/** Grid point of the nearest element pin, if the cursor is on one. Wires
 * start from pins — dragging a pin must never move the component. */
function pinAt(x: number, y: number): Point | null {
  const r = Math.min(14, cam.scale * 0.4);
  let best: Point | null = null;
  let bestD = r;
  for (const e of elements) {
    for (const p of e.pins) {
      const d = Math.hypot(cam.ox + p[0] * cam.scale - x, cam.oy + p[1] * cam.scale - y);
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
  }
  return best;
}

/** Probe flag center in px (must match drawProbeMarkers). */
function probeFlagPx(p: Probe): [number, number] | null {
  const e = elements.find((x) => x.id === p.elem);
  if (!e) return null;
  const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
  return [cam.ox + pin[0] * cam.scale + 14, cam.oy + pin[1] * cam.scale - 18];
}

function probeAt(x: number, y: number): Probe | undefined {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (c && Math.hypot(x - c[0], y - c[1]) < 9) return p;
  }
  return undefined;
}

// Floating-scope hit testing. Title bar is a fixed 18 px strip.
const SCOPE_TITLE_PX = 18;
type ScopeZone =
  | { s: FloatScope; zone: 'title' | 'body' | 'close' | 'resize' }
  | { s: FloatScope; zone: 'chan'; pid: number };

function scopeRectPx(s: FloatScope): [number, number, number, number] {
  return [cam.ox + s.x * cam.scale, cam.oy + s.y * cam.scale, s.w * cam.scale, s.h * cam.scale];
}

function scopeZoneAt(x: number, y: number): ScopeZone | null {
  for (let k = floatScopes.length - 1; k >= 0; k--) {
    const s = floatScopes[k]!;
    const [X, Y, W, H] = scopeRectPx(s);
    if (x < X || x > X + W || y < Y || y > Y + H) continue;
    if (y <= Y + SCOPE_TITLE_PX) {
      if (x >= X + W - 18) return { s, zone: 'close' };
      // channel dots start after the title text
      const dotStart = X + 64;
      const k2 = Math.floor((x - dotStart) / 16);
      if (x >= dotStart && k2 >= 0 && k2 < probes.length) {
        return { s, zone: 'chan', pid: probes[k2]!.pid };
      }
      return { s, zone: 'title' };
    }
    if (x >= X + W - 14 && y >= Y + H - 14) return { s, zone: 'resize' };
    return { s, zone: 'body' };
  }
  return null;
}

const scopeProbes = (s: FloatScope): Probe[] =>
  s.pids === null ? probes : probes.filter((p) => s.pids!.includes(p.pid));

const dragModeOf = (e: ElementSpec): 'log' | 'linear' | null =>
  e.kind.t === 'Resistor' || e.kind.t === 'Lamp'
    ? 'log'
    : e.kind.t === 'Potentiometer'
      ? 'linear'
      : null;

let valueDrag: {
  e: ElementSpec;
  mode: 'log' | 'linear';
  startY: number;
  startVal: number;
  lastSent: number;
  moved: boolean;
} | null = null;
let panDrag: { x: number; y: number; ox: number; oy: number } | null = null;
let wireDrag: { a: Point; b: Point } | null = null;
let placeDrag: { a: Point; b: Point } | null = null;
let moveDrag: {
  e: ElementSpec;
  startPins: Point[];
  start: Point;
  lastSent: number;
  moved: boolean;
} | null = null;
let scopeDrag: { s: FloatScope; dx: number; dy: number } | null = null;
let scopeResize: { s: FloatScope } | null = null;
let spaceHeld = false;
let lastCursorSent = 0;

canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  // Over a floating scope, the wheel is its timebase.
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  if (z) {
    z.s.tb = Math.min(60, Math.max(0.05, z.s.tb * Math.exp(ev.deltaY * 0.001)));
    return;
  }
  const k = Math.exp(-ev.deltaY * 0.0015);
  const s2 = Math.min(160, Math.max(8, cam.scale * k));
  cam.ox = ev.clientX - (ev.clientX - cam.ox) * (s2 / cam.scale);
  cam.oy = ev.clientY - (ev.clientY - cam.oy) * (s2 / cam.scale);
  cam.scale = s2;
}, { passive: false });
canvas.addEventListener('contextmenu', (ev) => ev.preventDefault());

canvas.addEventListener('pointerdown', (ev) => {
  try { canvas.setPointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  if (ev.button === 1 || ev.button === 2 || spaceHeld) {
    panDrag = { x: ev.clientX, y: ev.clientY, ox: cam.ox, oy: cam.oy };
    return;
  }
  if (placing) {
    const p = snap(ev.clientX, ev.clientY);
    placeDrag = { a: p, b: p };
    return;
  }
  // Floating scopes sit above the schematic.
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  if (z) {
    if (z.zone === 'close') {
      floatScopes = floatScopes.filter((s) => s.sid !== z.s.sid);
    } else if (z.zone === 'chan') {
      const s = z.s;
      if (s.pids === null) s.pids = probes.map((p) => p.pid);
      s.pids = s.pids.includes(z.pid) ? s.pids.filter((x) => x !== z.pid) : [...s.pids, z.pid];
    } else if (z.zone === 'title') {
      const [gx, gy] = toGrid(ev.clientX, ev.clientY);
      scopeDrag = { s: z.s, dx: gx - z.s.x, dy: gy - z.s.y };
    } else if (z.zone === 'resize') {
      scopeResize = { s: z.s };
    }
    return; // body swallows the event: no wires under scopes
  }
  // Probe flags select the probe.
  const pr = probeAt(ev.clientX, ev.clientY);
  if (pr) {
    selected = { t: 'probe', pid: pr.pid };
    return;
  }
  // Pins take priority: dragging from any terminal draws a wire.
  if (!ev.shiftKey) {
    const pin = pinAt(ev.clientX, ev.clientY);
    if (pin) {
      wireDrag = { a: pin, b: pin };
      return;
    }
  }
  const e = elementAt(ev.clientX, ev.clientY);
  if (!e) {
    const p = snap(ev.clientX, ev.clientY);
    wireDrag = { a: p, b: p };
    return;
  }
  if (ev.shiftKey) {
    moveDrag = {
      e,
      startPins: e.pins.map((p) => [...p] as Point),
      start: snap(ev.clientX, ev.clientY),
      lastSent: 0,
      moved: false,
    };
    return;
  }
  if (e.kind.t === 'Switch') {
    interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
    selected = { t: 'elem', id: e.id };
    return;
  }
  const mode = dragModeOf(e);
  if (mode) {
    const startVal =
      e.kind.t === 'Potentiometer' ? e.kind.wiper
      : e.kind.t === 'Resistor' || e.kind.t === 'Lamp' ? e.kind.ohms
      : 0;
    valueDrag = { e, mode, startY: ev.clientY, startVal, lastSent: 0, moved: false };
  } else {
    // Knob-less parts: drag moves, click selects (resolved on pointerup).
    moveDrag = {
      e,
      startPins: e.pins.map((p) => [...p] as Point),
      start: snap(ev.clientX, ev.clientY),
      lastSent: 0,
      moved: false,
    };
  }
});

canvas.addEventListener('pointermove', (ev) => {
  mouse = { x: ev.clientX, y: ev.clientY };
  const now = performance.now();
  if (online && now - lastCursorSent > 50) {
    lastCursorSent = now;
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    net.sendCursor(gx, gy);
  }
  if (panDrag) {
    cam.ox = panDrag.ox + (ev.clientX - panDrag.x);
    cam.oy = panDrag.oy + (ev.clientY - panDrag.y);
    return;
  }
  if (scopeDrag) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    scopeDrag.s.x = Math.round(gx - scopeDrag.dx);
    scopeDrag.s.y = Math.round(gy - scopeDrag.dy);
    return;
  }
  if (scopeResize) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    scopeResize.s.w = Math.max(6, Math.round(gx - scopeResize.s.x));
    scopeResize.s.h = Math.max(4, Math.round(gy - scopeResize.s.y));
    return;
  }
  if (placeDrag) {
    placeDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (wireDrag) {
    wireDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (moveDrag) {
    const here = snap(ev.clientX, ev.clientY);
    const dx = here[0] - moveDrag.start[0];
    const dy = here[1] - moveDrag.start[1];
    if (dx !== 0 || dy !== 0) moveDrag.moved = true;
    if (!moveDrag.moved) return;
    const pins = moveDrag.startPins.map(([x, y]) => [x + dx, y + dy] as Point);
    moveDrag.e.pins = pins;
    if (now - moveDrag.lastSent > 60) {
      moveDrag.lastSent = now;
      if (online) net.sendEdit({ t: 'Move', id: moveDrag.e.id, pins });
      else localSim.setElements(elements);
    }
    return;
  }
  if (valueDrag) {
    const dy = valueDrag.startY - ev.clientY;
    if (Math.abs(dy) > 3) valueDrag.moved = true;
    if (!valueDrag.moved) return;
    const value =
      valueDrag.mode === 'log'
        ? valueDrag.startVal * Math.pow(10, dy / 160)
        : Math.min(0.99, Math.max(0.01, valueDrag.startVal + dy / 200));
    if (now - valueDrag.lastSent > 40) {
      valueDrag.lastSent = now;
      interact(valueDrag.e, { t: 'SetValue', value });
    }
    return;
  }
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  const over = z ? undefined : elementAt(ev.clientX, ev.clientY);
  canvas.style.cursor = placing
    ? 'crosshair'
    : z
      ? z.zone === 'title'
        ? 'move'
        : z.zone === 'resize'
          ? 'nwse-resize'
          : 'default'
      : over?.kind.t === 'Switch'
        ? 'pointer'
        : over && dragModeOf(over)
          ? 'ns-resize'
          : 'default';
});

canvas.addEventListener('pointerup', (ev) => {
  try { canvas.releasePointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  if (panDrag) {
    panDrag = null;
    return;
  }
  if (scopeDrag || scopeResize) {
    scopeDrag = null;
    scopeResize = null;
    return;
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    const pins = makePins(kind, placeDrag.a, placeDrag.b);
    const id = newId();
    editDoc({ t: 'Add', spec: { id, kind, pins } });
    selected = { t: 'elem', id };
    if (!ev.shiftKey) {
      placing = null;
      canvas.style.cursor = 'default';
    }
    placeDrag = null;
    return;
  }
  if (wireDrag) {
    if (wireDrag.a[0] !== wireDrag.b[0] || wireDrag.a[1] !== wireDrag.b[1]) {
      editDoc({ t: 'Add', spec: { id: newId(), kind: { t: 'Wire' }, pins: [wireDrag.a, wireDrag.b] } });
    } else {
      selected = null; // click on nothing deselects
    }
    wireDrag = null;
    return;
  }
  if (moveDrag) {
    if (moveDrag.moved) {
      if (online) net.sendEdit({ t: 'Move', id: moveDrag.e.id, pins: moveDrag.e.pins });
      else localSim.setElements(elements);
    } else {
      selected = { t: 'elem', id: moveDrag.e.id };
    }
    moveDrag = null;
    return;
  }
  if (valueDrag) {
    if (!valueDrag.moved) selected = { t: 'elem', id: valueDrag.e.id };
    valueDrag = null;
  }
});
canvas.addEventListener('pointerleave', () => (mouse = null));

window.addEventListener('keydown', (ev) => {
  if (ev.target === psearch || (ev.target instanceof Node && propsDiv.contains(ev.target))) return;
  if (ev.key === ' ') {
    spaceHeld = true;
    ev.preventDefault();
  } else if (ev.key === '/' || ev.key === 'p') {
    openPalette();
    ev.preventDefault();
  } else if (ev.key === 'Escape') {
    placing = null;
    selected = null;
    closePalette();
    canvas.style.cursor = 'default';
  } else if (ev.key === 'Delete' || ev.key === 'Backspace' || ev.key === 'x') {
    const e = mouse ? elementAt(mouse.x, mouse.y) : undefined;
    if (e) editDoc({ t: 'Remove', id: e.id });
    else if (selected?.t === 'elem') editDoc({ t: 'Remove', id: selected.id });
  } else if (ev.key === 'r') {
    rotateSelected();
  } else if (ev.key === 'o' && mouse) {
    const [gx, gy] = toGrid(mouse.x, mouse.y);
    floatScopes.push({
      sid: sidCounter++,
      x: Math.round(gx),
      y: Math.round(gy),
      w: 12,
      h: 6,
      tb: 5,
      pids: null,
    });
  } else if (ev.key === 'v' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, nearestPin(e, mouse.x, mouse.y), 'v');
  } else if (ev.key === 'i' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    // Current clamp reads pin 0: current flowing a -> b through the part.
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, 0, 'i');
  } else if (ev.key === 'g' && mouse) {
    // Set the differential reference of the selected (or latest) V-probe.
    const target =
      selected?.t === 'probe'
        ? probes.find((p) => p.pid === (selected as { pid: number }).pid)
        : [...probes].reverse().find((p) => p.kind === 'v');
    const e = elementAt(mouse.x, mouse.y);
    if (target && target.kind === 'v' && e && e.kind.t !== 'Ground') {
      setProbeRef(target.pid, e.id, nearestPin(e, mouse.x, mouse.y));
    }
  }
});
window.addEventListener('keyup', (ev) => {
  if (ev.key === ' ') spaceHeld = false;
});

// ---------------------------------------------------------------- render
const fmt = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(2)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(2)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(2)} µ${unit}`;
  if (a >= 1e-9) return `${(v * 1e9).toFixed(2)} n${unit}`;
  return `0 ${unit}`;
};

function describeValue(e: ElementSpec): string {
  switch (e.kind.t) {
    case 'Resistor':
    case 'Lamp':
      return `R ${fmt(e.kind.ohms, 'Ω')}  (drag ↕)`;
    case 'Capacitor':
      return `C ${fmt(e.kind.farads, 'F')}`;
    case 'Inductor':
      return `L ${fmt(e.kind.henries, 'H')}`;
    case 'VoltageSource':
      return e.kind.amp === 0
        ? `${fmt(e.kind.dc, 'V')} DC`
        : `${fmt(e.kind.dc, 'V')} ± ${fmt(e.kind.amp, 'V')} @ ${e.kind.hz} Hz`;
    case 'Potentiometer':
      return `${fmt(e.kind.ohms, 'Ω')} @ ${(e.kind.wiper * 100).toFixed(0)}%  (drag ↕)`;
    case 'Npn':
    case 'Pnp':
      return `β ${e.kind.beta}`;
    case 'Nmos':
    case 'Pmos':
      return `Vt ${fmt(e.kind.vt, 'V')}`;
    case 'Zener':
      return `Vz ${fmt(e.kind.vz, 'V')}`;
    case 'OpAmp':
      return `rail ±${fmt(e.kind.rail, 'V')}`;
    default:
      return '';
  }
}

const scopeDiv = document.getElementById('scope') as HTMLDivElement;
const scopeCv = document.getElementById('scopecv') as HTMLCanvasElement;
scopeCv.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  ev.stopPropagation();
  scopeTimebase = Math.min(60, Math.max(0.05, scopeTimebase * Math.exp(ev.deltaY * 0.001)));
}, { passive: false });

function drawProbeMarkers() {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (!c) continue;
    const e = elements.find((x) => x.id === p.elem)!;
    const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
    const x = cam.ox + pin[0] * cam.scale;
    const y = cam.oy + pin[1] * cam.scale;
    const color = probeColor(p.pid);
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + 10, y - 14);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(c[0], c[1], 6, 0, Math.PI * 2);
    ctx.fill();
    if (selected?.t === 'probe' && selected.pid === p.pid) {
      ctx.beginPath();
      ctx.arc(c[0], c[1], 9, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.fillStyle = '#101014';
    ctx.font = 'bold 9px ui-monospace';
    ctx.fillText(p.kind === 'v' ? 'V' : 'I', c[0] - 3, c[1] + 3);

    // Differential reference marker.
    if (p.r) {
      const re = elements.find((x) => x.id === p.r![0]);
      if (re) {
        const rp = re.pins[Math.min(p.r[1], re.pins.length - 1)]!;
        const rx = cam.ox + rp[0] * cam.scale;
        const ry = cam.oy + rp[1] * cam.scale;
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(rx - 6, ry + 8);
        ctx.lineTo(rx + 6, ry + 8);
        ctx.moveTo(rx - 4, ry + 11);
        ctx.lineTo(rx + 4, ry + 11);
        ctx.moveTo(rx - 2, ry + 14);
        ctx.lineTo(rx + 2, ry + 14);
        ctx.stroke();
      }
    }
  }
}

function drawSelection() {
  if (selected?.t !== 'elem') return;
  const e = elements.find((x) => x.id === (selected as { id: number }).id);
  if (!e) return;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const p of e.pins) {
    x0 = Math.min(x0, p[0]);
    y0 = Math.min(y0, p[1]);
    x1 = Math.max(x1, p[0]);
    y1 = Math.max(y1, p[1]);
  }
  const pad = 0.65 * cam.scale;
  ctx.strokeStyle = '#5a8cff';
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 4]);
  ctx.strokeRect(
    cam.ox + x0 * cam.scale - pad,
    cam.oy + y0 * cam.scale - pad,
    (x1 - x0) * cam.scale + pad * 2,
    (y1 - y0) * cam.scale + pad * 2,
  );
  ctx.setLineDash([]);
}

function drawFloatScopes() {
  for (const s of floatScopes) {
    const [X, Y, W, H] = scopeRectPx(s);
    ctx.fillStyle = '#101016f0';
    ctx.fillRect(X, Y, W, H);
    ctx.strokeStyle = '#3a3a48';
    ctx.lineWidth = 1;
    ctx.strokeRect(X, Y, W, H);
    // title bar
    ctx.fillStyle = '#191922';
    ctx.fillRect(X, Y, W, SCOPE_TITLE_PX);
    ctx.fillStyle = '#8a8a98';
    ctx.font = '11px ui-monospace, monospace';
    ctx.fillText(`scope ${s.sid}`, X + 8, Y + 13);
    // channel dots
    const active = scopeProbes(s);
    probes.forEach((p, k) => {
      const cx = X + 64 + k * 16 + 5;
      const cy = Y + SCOPE_TITLE_PX / 2;
      ctx.beginPath();
      ctx.arc(cx, cy, 5, 0, Math.PI * 2);
      if (active.some((a) => a.pid === p.pid)) {
        ctx.fillStyle = probeColor(p.pid);
        ctx.fill();
      } else {
        ctx.strokeStyle = probeColor(p.pid);
        ctx.stroke();
      }
    });
    // close ×
    ctx.fillStyle = '#8a8a98';
    ctx.fillText('×', X + W - 13, Y + 13);
    // resize handle
    ctx.strokeStyle = '#3a3a48';
    ctx.beginPath();
    ctx.moveTo(X + W - 12, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 12);
    ctx.moveTo(X + W - 7, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 7);
    ctx.stroke();
    // content
    if (H - SCOPE_TITLE_PX > 20) {
      renderScopeInto(ctx, X + 1, Y + SCOPE_TITLE_PX, W - 2, H - SCOPE_TITLE_PX - 1, traces, active, s.tb);
    }
  }
}

function drawCursors(now: number) {
  for (const [who, c] of cursors) {
    if (now - c.seen > 4000) {
      cursors.delete(who);
      continue;
    }
    const x = cam.ox + c.x * cam.scale;
    const y = cam.oy + c.y * cam.scale;
    const hue = (who * 137.5) % 360;
    ctx.fillStyle = `hsl(${hue} 80% 60%)`;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + 12, y + 5);
    ctx.lineTo(x + 5, y + 12);
    ctx.closePath();
    ctx.fill();
    ctx.font = '11px ui-monospace';
    ctx.fillText(`P${who}`, x + 10, y + 22);
  }
}

let simDebt = 0;
let lastT = performance.now();

function frame(now: number) {
  const wallDt = Math.min(0.1, (now - lastT) / 1000);
  lastT = now;

  if (!online) {
    simDebt += wallDt / DT;
    const want = Math.floor(simDebt);
    localSim.advance(Math.min(want, MAX_STEPS_PER_FRAME));
    simDebt -= want;
    live = unpackFrame(localSim.frame());
    simTime = localSim.time();
    // Offline probes sample once per rendered frame (differential too).
    for (const p of probes) {
      const l = live.get(p.elem);
      if (!l) continue;
      let v = p.kind === 'v' ? l.v[p.pin] ?? 0 : l.i[p.pin] ?? 0;
      if (p.kind === 'v' && p.r) {
        const rl = live.get(p.r[0]);
        v -= rl?.v[p.r[1]] ?? 0;
      }
      traces.appendPoint(p.pid, simTime, v);
    }
  }

  ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
  if (cam.scale >= 14) {
    ctx.fillStyle = '#1c1c22';
    const gx0 = Math.ceil(-cam.ox / cam.scale);
    const gy0 = Math.ceil(-cam.oy / cam.scale);
    for (let gx = gx0; gx * cam.scale + cam.ox < window.innerWidth; gx++) {
      for (let gy = gy0; gy * cam.scale + cam.oy < window.innerHeight; gy++) {
        ctx.fillRect(cam.ox + gx * cam.scale - 1, cam.oy + gy * cam.scale - 1, 2, 2);
      }
    }
  }

  for (const e of elements) {
    drawElement({ ctx, cam, live: live.get(e.id), dots, dtSec: wallDt }, e);
  }

  // Ghost previews for in-progress edits.
  ctx.globalAlpha = 0.45;
  if (wireDrag) {
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind: { t: 'Wire' }, pins: [wireDrag.a, wireDrag.b] });
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, placeDrag.a, placeDrag.b) });
  } else if (placing && mouse) {
    const kind = placing.make();
    const a = snap(mouse.x, mouse.y);
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, a, a) });
  }
  ctx.globalAlpha = 1;

  drawSelection();
  drawProbeMarkers();
  drawFloatScopes();
  drawCursors(now);
  syncPropsPanel();

  // Docked scope panel: visible whenever anything is probed.
  if (probes.length > 0) {
    scopeDiv.style.display = 'block';
    renderScope(scopeCv, traces, probes, scopeTimebase);
  } else {
    scopeDiv.style.display = 'none';
  }

  const hover =
    mouse && !valueDrag && !scopeZoneAt(mouse.x, mouse.y)
      ? elementAt(mouse.x, mouse.y)
      : valueDrag?.e;
  if (hover && mouse && !placing && !wireDrag) {
    const l = live.get(hover.id);
    tip.style.display = 'block';
    tip.style.left = `${mouse.x + 14}px`;
    tip.style.top = `${mouse.y + 14}px`;
    const val = describeValue(hover);
    const labels = pinLabels(hover.kind);
    tip.textContent =
      `${hover.kind.t}${val ? '  ' + val : ''}\n` +
      (l
        ? labels.map((lb, p) => `${lb}: ${fmt(l.v[p] ?? 0, 'V')} ${fmt(l.i[p] ?? 0, 'A')}`).join('\n') +
          `\nP ${fmt(l.power, 'W')}`
        : '');
  } else {
    tip.style.display = 'none';
  }

  hud.textContent =
    `EE Game   sim t = ${simTime.toFixed(2)} s   ` +
    (online ? `● ONLINE — ${population} player${population === 1 ? '' : 's'}` : '○ offline (local sim)') +
    (placing ? `   placing: ${placing.name} (drag to orient, Esc cancels)` : '') +
    `\nclick select · drag pins = wire · / parts · V/I probe · G probe ref · O scope here · R rotate · X delete · wheel zoom · right-drag pan`;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
