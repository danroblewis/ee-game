// EE Game client. The sim runs the moment the page loads — no run button.
// Online: this browser renders the server's authoritative sim and sends
// interactions/edits. Offline: the same engine runs locally in WASM.
//
// Player controls:
//   wheel            zoom to cursor
//   middle/right or space+drag   pan
//   drag on empty    draw wire
//   / or "+ part"    open the parts palette, then drag to place
//   click switch     toggle; drag lamp/resistor/pot vertically = knob
//   shift+drag       move an element;  hover + Delete/x = remove

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
import { CATALOG, makePins, searchParts, type PartDef } from './catalog';
import { connect } from './net';
import { DotFlow, drawElement, hitTest, type Camera } from './render';

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
  } else {
    const e = elements.find((x) => x.id === op.id);
    if (e) e.pins = op.pins;
  }
}

let idCounter = 1;
const newId = () => (myId > 0 ? myId : 999) * 1_000_000 + idCounter++;

const net = connect({
  onHello(you, serverElements) {
    online = true;
    myId = you;
    elements = serverElements;
    live = new Map();
    fitCamera();
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

const dragMode = (e: ElementSpec): 'log' | 'linear' | null =>
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
} | null = null;
let panDrag: { x: number; y: number; ox: number; oy: number } | null = null;
let wireDrag: { a: Point; b: Point } | null = null;
let placeDrag: { a: Point; b: Point } | null = null;
let moveDrag: {
  e: ElementSpec;
  startPins: Point[];
  start: Point;
  lastSent: number;
} | null = null;
let spaceHeld = false;
let lastCursorSent = 0;

canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  const k = Math.exp(-ev.deltaY * 0.0015);
  const s2 = Math.min(160, Math.max(8, cam.scale * k));
  cam.ox = ev.clientX - (ev.clientX - cam.ox) * (s2 / cam.scale);
  cam.oy = ev.clientY - (ev.clientY - cam.oy) * (s2 / cam.scale);
  cam.scale = s2;
}, { passive: false });
canvas.addEventListener('contextmenu', (ev) => ev.preventDefault());

canvas.addEventListener('pointerdown', (ev) => {
  canvas.setPointerCapture(ev.pointerId);
  if (ev.button === 1 || ev.button === 2 || spaceHeld) {
    panDrag = { x: ev.clientX, y: ev.clientY, ox: cam.ox, oy: cam.oy };
    return;
  }
  if (placing) {
    const p = snap(ev.clientX, ev.clientY);
    placeDrag = { a: p, b: p };
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
    moveDrag = { e, startPins: e.pins.map((p) => [...p] as Point), start: snap(ev.clientX, ev.clientY), lastSent: 0 };
    return;
  }
  if (e.kind.t === 'Switch') {
    interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
    return;
  }
  const mode = dragMode(e);
  if (mode) {
    const startVal =
      e.kind.t === 'Potentiometer' ? e.kind.wiper
      : e.kind.t === 'Resistor' || e.kind.t === 'Lamp' ? e.kind.ohms
      : 0;
    valueDrag = { e, mode, startY: ev.clientY, startVal, lastSent: 0 };
  } else {
    // Anything without a knob moves with a plain drag.
    moveDrag = { e, startPins: e.pins.map((p) => [...p] as Point), start: snap(ev.clientX, ev.clientY), lastSent: 0 };
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
  const over = elementAt(ev.clientX, ev.clientY);
  canvas.style.cursor = placing
    ? 'crosshair'
    : over?.kind.t === 'Switch'
      ? 'pointer'
      : over && dragMode(over)
        ? 'ns-resize'
        : 'default';
});

canvas.addEventListener('pointerup', (ev) => {
  canvas.releasePointerCapture(ev.pointerId);
  if (panDrag) {
    panDrag = null;
    return;
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    const pins = makePins(kind, placeDrag.a, placeDrag.b);
    editDoc({ t: 'Add', spec: { id: newId(), kind, pins } });
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
    }
    wireDrag = null;
    return;
  }
  if (moveDrag) {
    if (online) net.sendEdit({ t: 'Move', id: moveDrag.e.id, pins: moveDrag.e.pins });
    else localSim.setElements(elements);
    moveDrag = null;
    return;
  }
  valueDrag = null;
});
canvas.addEventListener('pointerleave', () => (mouse = null));

window.addEventListener('keydown', (ev) => {
  if (ev.target === psearch) return;
  if (ev.key === ' ') {
    spaceHeld = true;
    ev.preventDefault();
  } else if (ev.key === '/' || ev.key === 'p') {
    openPalette();
    ev.preventDefault();
  } else if (ev.key === 'Escape') {
    placing = null;
    closePalette();
    canvas.style.cursor = 'default';
  } else if ((ev.key === 'Delete' || ev.key === 'Backspace' || ev.key === 'x') && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e) editDoc({ t: 'Remove', id: e.id });
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

  drawCursors(now);

  const hover = mouse && !valueDrag ? elementAt(mouse.x, mouse.y) : valueDrag?.e;
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
    `\ndraw wires on empty grid · / = parts · click switch · drag knob ↕ · shift-drag move · hover+X delete · wheel zoom · space/right-drag pan`;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
