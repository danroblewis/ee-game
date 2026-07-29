// EE Game client. The sim runs the moment the page loads — no run button.
// Online: this browser renders the server's authoritative sim and sends
// interactions/edits. Offline: the same engine runs locally in WASM.
//
// Controls (Falstad-style):
//   letter keys           arm a part for placement (R resistor, L inductor,
//                         C capacitor, W wire, G ground, V battery, F AC,
//                         I current src, D diode, Z zener, E LED, N npn,
//                         P pnp, M nmos, shift+M pmos, A op-amp, S switch,
//                         B lamp, T pot) — click places (Q rotates first),
//                         drag places with drag orientation, Esc exits
//   click                 select part / probe flag;  drag on empty = marquee
//   drag from a pin       draw wire (pins must overlap to connect)
//   ⌘/Ctrl+C, ⌘/Ctrl+V    copy selection / paste bound to cursor
//   Q                     rotate placement ghost, paste ghost, or selection
//   1 / 2                 voltage probe / current clamp at hover
//   3                     listen: play that node's waveform (WebAudio)
//   0                     set selected V-probe's reference (differential)
//   O                     drop an in-place oscilloscope;  X delete;  / palette
//   wheel zoom (over a scope: timebase) · middle/right/space drag pan

import init, { Sim } from './wasm/sim_wasm';
import {
  demoCircuit,
  pinLabels,
  unpackFrame,
  type DocOp,
  type ElementKind,
  type ElementSpec,
  type ElemLive,
  type InteractOp,
  type Point,
} from './circuit';
import { AudioPlayer } from './audio';
import { CATALOG, makePins, searchParts, type PartDef } from './catalog';
import { connect } from './net';
import { DotFlow, drawElement, hitTest, type Camera } from './render';
import { probeColor, renderScope, renderScopeInto, TraceStore, type Probe } from './scope';

const DT = 10e-6;
const MAX_STEPS_PER_FRAME = 4000; // local-mode wall budget

const PART_HOTKEYS: Record<string, string> = {
  w: 'Wire',
  r: 'Resistor',
  c: 'Capacitor',
  l: 'Inductor',
  g: 'Ground',
  v: 'Battery',
  f: 'AC Source',
  i: 'Current Source',
  d: 'Diode',
  z: 'Zener',
  e: 'LED',
  n: 'NPN',
  p: 'PNP',
  m: 'NMOS',
  M: 'PMOS',
  a: 'Op-Amp',
  u: 'OTA',
  s: 'Switch',
  b: 'Lamp',
  t: 'Potentiometer',
};

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

/** '3' listens to one probe's stream; online the pid arrives with the
 * server's probe list, so remember what we asked to hear. */
const audio = new AudioPlayer();
let listenWanted: { elem: number; pin: number } | null = null;

let selectedIds = new Set<number>();
let selectedProbe: number | null = null;

/** Copy/paste: kinds + pins relative to the selection centroid. */
type ClipItem = { kind: ElementKind; pins: Point[] };
let clipboard: ClipItem[] = [];
let pasting: ClipItem[] | null = null;

/** In-place oscilloscopes: world-anchored, per-player instruments. */
interface FloatScope {
  sid: number;
  x: number;
  y: number;
  w: number;
  h: number;
  tb: number;
  pids: number[] | null; // null = all probes
}
let floatScopes: FloatScope[] = [];
let sidCounter = 1;

const localSim = new Sim(DT);
localSim.setElements(elements);

function applyOp(e: ElementSpec, op: InteractOp) {
  if (op.t === 'SetSwitch' && e.kind.t === 'Switch') e.kind.closed = op.closed;
  if (op.t === 'SetValue') {
    if (e.kind.t === 'Resistor' || e.kind.t === 'Lamp' || e.kind.t === 'Speaker') e.kind.ohms = op.value;
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
    selectedIds.delete(op.id);
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
    if (firstHello) {
      firstHello = false;
      fitCamera();
    }
  },
  onFrame(f) {
    simTime = f.time;
    const m = new Map<number, ElemLive>();
    for (const r of f.e) {
      m.set(r[0]!, {
        id: r[0]!,
        npins: r[1]!,
        v: [r[2]!, r[3]!, r[4]!, r[5]!],
        i: [r[6]!, r[7]!, r[8]!, r[9]!],
        power: r[10]!,
      });
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
    if (selectedProbe !== null && !alive.has(selectedProbe)) selectedProbe = null;
    for (const s of floatScopes) if (s.pids) s.pids = s.pids.filter((pid) => alive.has(pid));
    if (listenWanted) {
      const p = list.find(
        (x) => x.elem === listenWanted!.elem && x.pin === listenWanted!.pin && x.kind === 'v',
      );
      if (p) {
        listenWanted = null;
        audio.listen(p.pid);
      }
    }
    if (audio.pid !== null && !alive.has(audio.pid)) audio.stop();
  },
  onSamples(t0, dts, s) {
    for (const [pid, samples] of Object.entries(s)) {
      traces.appendChunk(Number(pid), t0, dts, samples);
      audio.pushChunk(Number(pid), t0, dts, samples);
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

/** '3': hear this node. The audio source is a normal voltage probe's sample
 * stream, so make sure one exists here, then latch the player onto it —
 * pressing '3' again on the same pin stops (the probe stays). */
function toggleListen(elem: number, pin: number) {
  const here = probes.find((p) => p.elem === elem && p.pin === pin && p.kind === 'v');
  if (here) {
    listenWanted = null;
    if (audio.pid === here.pid) audio.stop();
    else audio.listen(here.pid);
    return;
  }
  listenWanted = { elem, pin };
  toggleProbe(elem, pin, 'v');
  const made = probes.find((p) => p.elem === elem && p.pin === pin && p.kind === 'v');
  if (made) {
    listenWanted = null;
    audio.listen(made.pid);
  }
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
// Exposed for end-to-end tests.
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
const toPx = (p: Point): [number, number] => [cam.ox + p[0] * cam.scale, cam.oy + p[1] * cam.scale];

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
  cam.scale = Math.max(18, Math.min(window.innerWidth / w, window.innerHeight / ht));
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
let placeRot = 0; // 0..3, quarter turns; Q rotates

const ROT_DIRS: Point[] = [
  [1, 0],
  [0, 1],
  [-1, 0],
  [0, -1],
];

/** Default far endpoint for a click-place at `a` with the armed rotation. */
function placeEnd(a: Point): Point {
  const d = ROT_DIRS[placeRot]!;
  return [a[0] + d[0] * 4, a[1] + d[1] * 4];
}

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
  pasting = null;
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
let propsShownFor = '';

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
    selectedIds.size === 1
      ? elements.find((e) => e.id === [...selectedIds][0])
      : undefined;
  if (!target) {
    propsDiv.style.display = 'none';
    propsShownFor = '';
    return;
  }
  const key = JSON.stringify([target.id, target.kind]);
  if (key === propsShownFor) return;
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
  rot.textContent = '⟳ rotate (Q)';
  rot.onclick = () => rotateSelection();
  const del = document.createElement('button');
  del.textContent = '✕ delete';
  del.onclick = () => editDoc({ t: 'Remove', id: target.id });
  row.appendChild(rot);
  row.appendChild(del);
  propsDiv.appendChild(row);
}

/** Rotate the whole selection 90° clockwise about its shared centroid. */
function rotateSelection() {
  const sel = elements.filter((e) => selectedIds.has(e.id));
  if (sel.length === 0) return;
  let sx = 0;
  let sy = 0;
  let n = 0;
  for (const e of sel) {
    for (const p of e.pins) {
      sx += p[0];
      sy += p[1];
      n++;
    }
  }
  const cx = Math.round(sx / n);
  const cy = Math.round(sy / n);
  for (const e of sel) {
    const pins = e.pins.map(([x, y]) => [cx - (y - cy), cy + (x - cx)] as Point);
    editDoc({ t: 'Move', id: e.id, pins });
  }
}

function copySelection() {
  const sel = elements.filter((e) => selectedIds.has(e.id));
  if (sel.length === 0) return;
  let sx = 0;
  let sy = 0;
  let n = 0;
  for (const e of sel) {
    for (const p of e.pins) {
      sx += p[0];
      sy += p[1];
      n++;
    }
  }
  const cx = Math.round(sx / n);
  const cy = Math.round(sy / n);
  clipboard = sel.map((e) => ({
    kind: JSON.parse(JSON.stringify(e.kind)) as ElementKind,
    pins: e.pins.map(([x, y]) => [x - cx, y - cy] as Point),
  }));
}

function commitPaste(at: Point) {
  if (!pasting) return;
  const ids: number[] = [];
  for (const item of pasting) {
    const id = newId();
    ids.push(id);
    editDoc({
      t: 'Add',
      spec: {
        id,
        kind: JSON.parse(JSON.stringify(item.kind)) as ElementKind,
        pins: item.pins.map(([x, y]) => [x + at[0], y + at[1]] as Point),
      },
    });
  }
  selectedIds = new Set(ids);
  selectedProbe = null;
  pasting = null;
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

/** Grid point of the nearest element pin, if the cursor is on one. */
function pinAt(x: number, y: number): Point | null {
  const r = Math.min(14, cam.scale * 0.4);
  let best: Point | null = null;
  let bestD = r;
  for (const e of elements) {
    for (const p of e.pins) {
      const [px, py] = toPx(p);
      const d = Math.hypot(px - x, py - y);
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
  }
  return best;
}

/** Does any element have a pin exactly at grid point `p`? */
function pinExistsAt(p: Point, excludeId = -1): boolean {
  return elements.some(
    (e) => e.id !== excludeId && e.pins.some((q) => q[0] === p[0] && q[1] === p[1]),
  );
}

function probeFlagPx(p: Probe): [number, number] | null {
  const e = elements.find((x) => x.id === p.elem);
  if (!e) return null;
  const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
  const [x, y] = toPx(pin);
  return [x + 14, y - 18];
}

function probeAt(x: number, y: number): Probe | undefined {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (c && Math.hypot(x - c[0], y - c[1]) < 9) return p;
  }
  return undefined;
}

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
  e.kind.t === 'Resistor' || e.kind.t === 'Lamp' || e.kind.t === 'Speaker'
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
let marquee: { x0: number; y0: number; x1: number; y1: number; add: boolean } | null = null;
let moveDrag: {
  items: { id: number; startPins: Point[] }[];
  start: Point;
  lastSent: number;
  moved: boolean;
  clickTarget: number;
} | null = null;
let scopeDrag: { s: FloatScope; dx: number; dy: number } | null = null;
let scopeResize: { s: FloatScope } | null = null;
let spaceHeld = false;
let lastCursorSent = 0;

canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
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
  if (pasting) {
    commitPaste(snap(ev.clientX, ev.clientY));
    return;
  }
  if (placing) {
    const p = snap(ev.clientX, ev.clientY);
    placeDrag = { a: p, b: p };
    return;
  }
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
    return;
  }
  const pr = probeAt(ev.clientX, ev.clientY);
  if (pr) {
    selectedProbe = pr.pid;
    selectedIds.clear();
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
    marquee = {
      x0: ev.clientX,
      y0: ev.clientY,
      x1: ev.clientX,
      y1: ev.clientY,
      add: ev.shiftKey,
    };
    return;
  }
  if (ev.shiftKey) {
    // Shift+click toggles membership in the selection.
    if (selectedIds.has(e.id)) selectedIds.delete(e.id);
    else selectedIds.add(e.id);
    return;
  }
  const startMove = (ids: number[]) => {
    moveDrag = {
      items: ids
        .map((id) => elements.find((x) => x.id === id))
        .filter((x): x is ElementSpec => !!x)
        .map((x) => ({ id: x.id, startPins: x.pins.map((p) => [...p] as Point) })),
      start: snap(ev.clientX, ev.clientY),
      lastSent: 0,
      moved: false,
      clickTarget: e.id,
    };
  };
  // Dragging a member of a multi-selection moves the whole group.
  if (selectedIds.has(e.id) && selectedIds.size > 1) {
    startMove([...selectedIds]);
    return;
  }
  if (e.kind.t === 'Switch') {
    interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
    selectedIds = new Set([e.id]);
    selectedProbe = null;
    return;
  }
  const mode = dragModeOf(e);
  if (mode) {
    const startVal =
      e.kind.t === 'Potentiometer' ? e.kind.wiper
      : e.kind.t === 'Resistor' || e.kind.t === 'Lamp' || e.kind.t === 'Speaker' ? e.kind.ohms
      : 0;
    valueDrag = { e, mode, startY: ev.clientY, startVal, lastSent: 0, moved: false };
  } else {
    startMove([e.id]);
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
  if (marquee) {
    marquee.x1 = ev.clientX;
    marquee.y1 = ev.clientY;
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
    for (const item of moveDrag.items) {
      const e = elements.find((x) => x.id === item.id);
      if (e) e.pins = item.startPins.map(([x, y]) => [x + dx, y + dy] as Point);
    }
    if (now - moveDrag.lastSent > 60) {
      moveDrag.lastSent = now;
      for (const item of moveDrag.items) {
        const e = elements.find((x) => x.id === item.id);
        if (e && online) net.sendEdit({ t: 'Move', id: e.id, pins: e.pins });
      }
      if (!online) localSim.setElements(elements);
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
  canvas.style.cursor = placing || pasting
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
  if (marquee) {
    const [gx0, gy0] = toGrid(Math.min(marquee.x0, marquee.x1), Math.min(marquee.y0, marquee.y1));
    const [gx1, gy1] = toGrid(Math.max(marquee.x0, marquee.x1), Math.max(marquee.y0, marquee.y1));
    const dragged = Math.abs(marquee.x1 - marquee.x0) + Math.abs(marquee.y1 - marquee.y0) > 6;
    if (!marquee.add) {
      selectedIds.clear();
      selectedProbe = null;
    }
    if (dragged) {
      for (const e of elements) {
        if (e.pins.some(([x, y]) => x >= gx0 && x <= gx1 && y >= gy0 && y <= gy1)) {
          selectedIds.add(e.id);
        }
      }
    }
    marquee = null;
    return;
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    const a = placeDrag.a;
    const clicked = placeDrag.b[0] === a[0] && placeDrag.b[1] === a[1];
    const b = clicked ? placeEnd(a) : placeDrag.b;
    const id = newId();
    editDoc({ t: 'Add', spec: { id, kind, pins: makePins(kind, a, b) } });
    selectedIds = new Set([id]);
    selectedProbe = null;
    placeDrag = null;
    return; // tool stays armed (Falstad-style); Esc exits
  }
  if (wireDrag) {
    if (wireDrag.a[0] !== wireDrag.b[0] || wireDrag.a[1] !== wireDrag.b[1]) {
      editDoc({ t: 'Add', spec: { id: newId(), kind: { t: 'Wire' }, pins: [wireDrag.a, wireDrag.b] } });
    }
    wireDrag = null;
    return;
  }
  if (moveDrag) {
    if (moveDrag.moved) {
      for (const item of moveDrag.items) {
        const e = elements.find((x) => x.id === item.id);
        if (e && online) net.sendEdit({ t: 'Move', id: e.id, pins: e.pins });
      }
      if (!online) localSim.setElements(elements);
    } else {
      selectedIds = new Set([moveDrag.clickTarget]);
      selectedProbe = null;
    }
    moveDrag = null;
    return;
  }
  if (valueDrag) {
    if (!valueDrag.moved) {
      selectedIds = new Set([valueDrag.e.id]);
      selectedProbe = null;
    }
    valueDrag = null;
  }
});
canvas.addEventListener('pointerleave', () => (mouse = null));

window.addEventListener('keydown', (ev) => {
  if (ev.target === psearch || (ev.target instanceof Node && propsDiv.contains(ev.target))) return;

  // Clipboard first: ⌘/Ctrl+C copies, ⌘/Ctrl+V arms pasting at the cursor.
  if (ev.metaKey || ev.ctrlKey) {
    if (ev.key === 'c') {
      copySelection();
      ev.preventDefault();
    } else if (ev.key === 'v') {
      if (clipboard.length > 0) {
        pasting = clipboard.map((c) => ({ kind: c.kind, pins: c.pins }));
        placing = null;
        canvas.style.cursor = 'crosshair';
      }
      ev.preventDefault();
    }
    return;
  }
  if (ev.altKey) return;

  if (ev.key === ' ') {
    spaceHeld = true;
    ev.preventDefault();
    return;
  }
  if (ev.key === '/') {
    openPalette();
    ev.preventDefault();
    return;
  }
  if (ev.key === 'Escape') {
    placing = null;
    pasting = null;
    selectedIds.clear();
    selectedProbe = null;
    closePalette();
    canvas.style.cursor = 'default';
    return;
  }
  if (ev.key === 'Delete' || ev.key === 'Backspace' || ev.key === 'x') {
    const e = mouse ? elementAt(mouse.x, mouse.y) : undefined;
    if (selectedIds.size > 0) {
      for (const id of [...selectedIds]) editDoc({ t: 'Remove', id });
    } else if (e) {
      editDoc({ t: 'Remove', id: e.id });
    }
    return;
  }
  if (ev.key === 'q' || ev.key === 'Q') {
    if (placing) {
      placeRot = (placeRot + 1) % 4;
    } else if (pasting) {
      // Rotate the paste ghost 90° clockwise about its centroid (origin).
      pasting = pasting.map((c) => ({
        kind: c.kind,
        pins: c.pins.map(([x, y]) => [-y, x] as Point),
      }));
    } else {
      rotateSelection();
    }
    return;
  }
  if (ev.key === 'o' && mouse) {
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
    return;
  }
  if (ev.key === '1' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, nearestPin(e, mouse.x, mouse.y), 'v');
    return;
  }
  if (ev.key === '2' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, 0, 'i');
    return;
  }
  if (ev.key === '3' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleListen(e.id, nearestPin(e, mouse.x, mouse.y));
    return;
  }
  if (ev.key === '0' && mouse) {
    const target =
      selectedProbe !== null
        ? probes.find((p) => p.pid === selectedProbe)
        : [...probes].reverse().find((p) => p.kind === 'v');
    const e = elementAt(mouse.x, mouse.y);
    if (target && target.kind === 'v' && e && e.kind.t !== 'Ground') {
      setProbeRef(target.pid, e.id, nearestPin(e, mouse.x, mouse.y));
    }
    return;
  }
  // Part hotkeys (Falstad-style). 'M' (shift+m) = PMOS.
  const partName = PART_HOTKEYS[ev.key];
  if (partName) {
    const part = CATALOG.find((p) => p.name === partName);
    if (part) choosePart(part);
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
    case 'Speaker':
      return `${fmt(e.kind.ohms, 'Ω')} coil  (drag ↕, 3 listens)`;
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
    case 'Ota':
      return 'Iout = Iabc·tanh(vd/2Vt)';
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

/** Blue highlight over an element: its pin-chain plus dots on every pin
 * (pins must overlap exactly to connect — make them visible). */
function drawHighlight(e: ElementSpec, strong: boolean) {
  ctx.strokeStyle = strong ? '#5a8cff' : '#4a7de0';
  ctx.fillStyle = '#7db1ff';
  ctx.lineWidth = Math.max(4, cam.scale * 0.16);
  ctx.globalAlpha = strong ? 0.5 : 0.35;
  const P = e.pins.map(toPx);
  ctx.beginPath();
  for (let k = 0; k + 1 < P.length; k++) {
    ctx.moveTo(...P[k]!);
    ctx.lineTo(...P[k + 1]!);
  }
  if (P.length === 3) {
    ctx.moveTo(...P[0]!);
    ctx.lineTo(...P[2]!);
  }
  if (P.length === 1) {
    ctx.moveTo(P[0]![0] - cam.scale * 0.3, P[0]![1]);
    ctx.lineTo(P[0]![0] + cam.scale * 0.3, P[0]![1]);
  }
  ctx.stroke();
  ctx.globalAlpha = 0.9;
  for (const [x, y] of P) {
    ctx.beginPath();
    ctx.arc(x, y, Math.max(3, cam.scale * 0.11), 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
}

/** Little speaker next to the flag of the probe we are listening to; its
 * arcs ride the stream's own amplitude. */
function drawListenGlyph(x: number, y: number, color: string) {
  const lvl = audio.level;
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(x, y - 2);
  ctx.lineTo(x + 3, y - 2);
  ctx.lineTo(x + 6, y - 5);
  ctx.lineTo(x + 6, y + 5);
  ctx.lineTo(x + 3, y + 2);
  ctx.lineTo(x, y + 2);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.2;
  for (let k = 0; k < 2; k++) {
    ctx.globalAlpha = Math.min(1, 0.25 + lvl * 2.5 - k * 0.35);
    ctx.beginPath();
    ctx.arc(x + 6, y, 4 + k * 3.5, -0.9, 0.9);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
}

function drawProbeMarkers() {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (!c) continue;
    const e = elements.find((x) => x.id === p.elem)!;
    const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
    const [x, y] = toPx(pin);
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
    if (selectedProbe === p.pid) {
      ctx.beginPath();
      ctx.arc(c[0], c[1], 9, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.fillStyle = '#101014';
    ctx.font = 'bold 9px ui-monospace';
    ctx.fillText(p.kind === 'v' ? 'V' : 'I', c[0] - 3, c[1] + 3);
    if (audio.pid === p.pid) drawListenGlyph(c[0] + 9, c[1], color);

    if (p.r) {
      const re = elements.find((x) => x.id === p.r![0]);
      if (re) {
        const rp = re.pins[Math.min(p.r[1], re.pins.length - 1)]!;
        const [rx, ry] = toPx(rp);
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

function drawSelectionBoxes() {
  if (selectedIds.size === 0) return;
  ctx.strokeStyle = '#5a8cff';
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 4]);
  for (const id of selectedIds) {
    const e = elements.find((x) => x.id === id);
    if (!e) continue;
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const p of e.pins) {
      x0 = Math.min(x0, p[0]);
      y0 = Math.min(y0, p[1]);
      x1 = Math.max(x1, p[0]);
      y1 = Math.max(y1, p[1]);
    }
    const pad = 0.55 * cam.scale;
    ctx.strokeRect(
      cam.ox + x0 * cam.scale - pad,
      cam.oy + y0 * cam.scale - pad,
      (x1 - x0) * cam.scale + pad * 2,
      (y1 - y0) * cam.scale + pad * 2,
    );
  }
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
    ctx.fillStyle = '#191922';
    ctx.fillRect(X, Y, W, SCOPE_TITLE_PX);
    ctx.fillStyle = '#8a8a98';
    ctx.font = '11px ui-monospace, monospace';
    ctx.fillText(`scope ${s.sid}`, X + 8, Y + 13);
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
    ctx.fillStyle = '#8a8a98';
    ctx.fillText('×', X + W - 13, Y + 13);
    ctx.strokeStyle = '#3a3a48';
    ctx.beginPath();
    ctx.moveTo(X + W - 12, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 12);
    ctx.moveTo(X + W - 7, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 7);
    ctx.stroke();
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
    for (const p of probes) {
      const l = live.get(p.elem);
      if (!l) continue;
      let v = p.kind === 'v' ? l.v[p.pin] ?? 0 : l.i[p.pin] ?? 0;
      if (p.kind === 'v' && p.r) {
        const rl = live.get(p.r[0]);
        v -= rl?.v[p.r[1]] ?? 0;
      }
      traces.appendPoint(p.pid, simTime, v);
      audio.pushPoint(p.pid, simTime, v);
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

  // Hover highlight (blue element + pin dots), Falstad-style.
  const zHover = mouse ? scopeZoneAt(mouse.x, mouse.y) : null;
  const hover =
    mouse && !valueDrag && !moveDrag && !placing && !pasting && !zHover
      ? elementAt(mouse.x, mouse.y)
      : valueDrag?.e;
  if (hover) drawHighlight(hover, true);
  for (const id of selectedIds) {
    const e = elements.find((x) => x.id === id);
    if (e && e !== hover) drawHighlight(e, false);
  }

  // Ghost previews for in-progress edits.
  ctx.globalAlpha = 0.45;
  if (wireDrag) {
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind: { t: 'Wire' }, pins: [wireDrag.a, wireDrag.b] });
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    const clicked = placeDrag.b[0] === placeDrag.a[0] && placeDrag.b[1] === placeDrag.a[1];
    const b = clicked ? placeEnd(placeDrag.a) : placeDrag.b;
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, placeDrag.a, b) });
  } else if (placing && mouse) {
    const kind = placing.make();
    const a = snap(mouse.x, mouse.y);
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, a, placeEnd(a)) });
  }
  if (pasting && mouse) {
    const at = snap(mouse.x, mouse.y);
    for (const item of pasting) {
      drawElement(
        { ctx, cam, dots, dtSec: 0 },
        { id: 0, kind: item.kind, pins: item.pins.map(([x, y]) => [x + at[0], y + at[1]] as Point) },
      );
    }
  }
  ctx.globalAlpha = 1;

  // Wire-endpoint connect indicator: green when the endpoint lands on an
  // existing pin (overlapping pins = connected), gray otherwise.
  if (wireDrag) {
    const [bx, by] = toPx(wireDrag.b);
    const connects = pinExistsAt(wireDrag.b);
    ctx.strokeStyle = connects ? '#4bff6a' : '#8a8a98';
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(bx, by, Math.max(5, cam.scale * 0.18), 0, Math.PI * 2);
    ctx.stroke();
  }

  if (marquee) {
    ctx.strokeStyle = '#5a8cff';
    ctx.fillStyle = '#5a8cff18';
    ctx.lineWidth = 1;
    const x = Math.min(marquee.x0, marquee.x1);
    const y = Math.min(marquee.y0, marquee.y1);
    const w = Math.abs(marquee.x1 - marquee.x0);
    const h = Math.abs(marquee.y1 - marquee.y0);
    ctx.fillRect(x, y, w, h);
    ctx.strokeRect(x, y, w, h);
  }

  drawSelectionBoxes();
  drawProbeMarkers();
  drawFloatScopes();
  drawCursors(now);
  syncPropsPanel();

  if (probes.length > 0) {
    scopeDiv.style.display = 'block';
    renderScope(scopeCv, traces, probes, scopeTimebase);
  } else {
    scopeDiv.style.display = 'none';
  }

  if (hover && mouse && !placing && !pasting && !wireDrag) {
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

  const mode = pasting
    ? `pasting ${pasting.length} parts (Q rotates, click places, Esc cancels)`
    : placing
      ? `placing: ${placing.name} (click or drag, Q rotates, Esc exits)`
      : selectedIds.size > 1
        ? `${selectedIds.size} selected (drag moves, Q rotates, ⌘C copies, X deletes)`
        : '';
  hud.textContent =
    `EE Game   sim t = ${simTime.toFixed(2)} s   ` +
    (online ? `● ONLINE — ${population} player${population === 1 ? '' : 's'}` : '○ offline (local sim)') +
    (mode ? `   ${mode}` : '') +
    `\nparts: R C L W G V D N P M A U S B T Z E F I · Q rotate · drag pin = wire · drag empty = select · ⌘C/⌘V copy/paste · 1/2 probe · 3 listen · 0 ref · O scope · X delete · / search`;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
