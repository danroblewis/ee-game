// EE Game client. The sim runs the moment the page loads — no run button.
// Online: this browser renders the server's authoritative sim and sends
// interactions. Offline (no server): the same engine runs locally in WASM.

import init, { Sim } from './wasm/sim_wasm';
import {
  demoCircuit,
  pinLabels,
  unpackFrame,
  type ElementSpec,
  type ElemLive,
  type InteractOp,
} from './circuit';
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

// ---------------------------------------------------------------- canvas
const canvas = document.getElementById('canvas') as HTMLCanvasElement;
const hud = document.getElementById('hud') as HTMLDivElement;
const tip = document.getElementById('tip') as HTMLDivElement;
const ctx = canvas.getContext('2d')!;

const cam: Camera = { scale: 48, ox: 60, oy: 60 };
const dots = new DotFlow();
let mouse: { x: number; y: number } | null = null;

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
  if (!isFinite(x0)) return;
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
  fitCamera();
}
window.addEventListener('resize', resize);
resize();

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

// Knob-draggable kinds: resistive values sweep log (a decade per 160 px);
// the pot wiper sweeps linearly.
const dragMode = (e: ElementSpec): 'log' | 'linear' | null =>
  e.kind.t === 'Resistor' || e.kind.t === 'Lamp'
    ? 'log'
    : e.kind.t === 'Potentiometer'
      ? 'linear'
      : null;

let drag: {
  e: ElementSpec;
  mode: 'log' | 'linear';
  startY: number;
  startVal: number;
  lastSent: number;
} | null = null;
let lastCursorSent = 0;

canvas.addEventListener('pointermove', (ev) => {
  mouse = { x: ev.clientX, y: ev.clientY };
  const now = performance.now();
  if (online && now - lastCursorSent > 50) {
    lastCursorSent = now;
    net.sendCursor((ev.clientX - cam.ox) / cam.scale, (ev.clientY - cam.oy) / cam.scale);
  }
  if (drag) {
    // EveryCircuit-style vertical knob drag, live within a frame:
    // log values sweep a decade per 160 px, the pot wiper is linear.
    const dy = drag.startY - ev.clientY;
    const value =
      drag.mode === 'log'
        ? drag.startVal * Math.pow(10, dy / 160)
        : Math.min(0.99, Math.max(0.01, drag.startVal + dy / 200));
    if (now - drag.lastSent > 40) {
      drag.lastSent = now;
      interact(drag.e, { t: 'SetValue', value });
    }
    return;
  }
  const over = elementAt(ev.clientX, ev.clientY);
  canvas.style.cursor =
    over?.kind.t === 'Switch' ? 'pointer' : over && dragMode(over) ? 'ns-resize' : 'default';
});
canvas.addEventListener('pointerleave', () => (mouse = null));
canvas.addEventListener('pointerdown', (ev) => {
  const e = elementAt(ev.clientX, ev.clientY);
  if (!e) return;
  const mode = dragMode(e);
  if (e.kind.t === 'Switch') {
    interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
  } else if (mode) {
    const startVal =
      e.kind.t === 'Potentiometer'
        ? e.kind.wiper
        : e.kind.t === 'Resistor' || e.kind.t === 'Lamp'
          ? e.kind.ohms
          : 0;
    drag = { e, mode, startY: ev.clientY, startVal, lastSent: 0 };
    canvas.setPointerCapture(ev.pointerId);
  }
});
canvas.addEventListener('pointerup', (ev) => {
  if (drag) {
    canvas.releasePointerCapture(ev.pointerId);
    drag = null;
  }
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
  ctx.fillStyle = '#1c1c22';
  const gx0 = Math.ceil(-cam.ox / cam.scale);
  const gy0 = Math.ceil(-cam.oy / cam.scale);
  for (let gx = gx0; gx * cam.scale + cam.ox < window.innerWidth; gx++) {
    for (let gy = gy0; gy * cam.scale + cam.oy < window.innerHeight; gy++) {
      ctx.fillRect(cam.ox + gx * cam.scale - 1, cam.oy + gy * cam.scale - 1, 2, 2);
    }
  }

  for (const e of elements) {
    drawElement({ ctx, cam, live: live.get(e.id), dots, dtSec: wallDt }, e);
  }
  drawCursors(now);

  const hover = mouse && !drag ? elementAt(mouse.x, mouse.y) : drag?.e;
  if (hover && mouse) {
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
    `\nclick the switch · drag a lamp ↕ to change resistance`;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
