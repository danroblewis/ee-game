// M1 demo: the sim runs the moment the page loads — no run button, ever.

import init, { Sim } from './wasm/sim_wasm';
import { demoCircuit, unpackFrame, type ElementSpec, type InteractOp } from './circuit';
import { DotFlow, drawElement, hitTest, type Camera } from './render';

const DT = 10e-6; // 10 µs fixed timestep (plan resolution 1)
const MAX_STEPS_PER_FRAME = 4000; // wall budget: sim time dilates, UI never stalls

await init();

const sim = new Sim(DT);
const elements: ElementSpec[] = demoCircuit();
sim.setElements(elements);

const canvas = document.getElementById('canvas') as HTMLCanvasElement;
const hud = document.getElementById('hud') as HTMLDivElement;
const tip = document.getElementById('tip') as HTMLDivElement;
const ctx = canvas.getContext('2d')!;

const cam: Camera = { scale: 56, ox: 60, oy: 60 };
const dots = new DotFlow();
let mouse: { x: number; y: number } | null = null;

function resize() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = window.innerWidth * dpr;
  canvas.height = window.innerHeight * dpr;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  // center the demo circuit
  cam.scale = Math.min(72, Math.max(36, window.innerWidth / 22));
  cam.ox = (window.innerWidth - 18 * cam.scale) / 2;
  cam.oy = (window.innerHeight - 10 * cam.scale) / 2;
}
window.addEventListener('resize', resize);
resize();

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

canvas.addEventListener('pointermove', (ev) => {
  mouse = { x: ev.clientX, y: ev.clientY };
  canvas.style.cursor = elementAt(ev.clientX, ev.clientY)?.kind.t === 'Switch' ? 'pointer' : 'default';
});
canvas.addEventListener('pointerleave', () => (mouse = null));
canvas.addEventListener('pointerdown', (ev) => {
  const e = elementAt(ev.clientX, ev.clientY);
  if (e && e.kind.t === 'Switch') {
    e.kind.closed = !e.kind.closed;
    const op: InteractOp = { t: 'SetSwitch', closed: e.kind.closed };
    sim.interact(e.id, op);
  }
});

const fmt = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(2)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(2)} µ${unit}`;
  if (a >= 1e-9) return `${(v * 1e9).toFixed(2)} n${unit}`;
  return `0 ${unit}`;
};

let simDebt = 0; // fractional sim steps owed
let lastT = performance.now();

function frame(now: number) {
  const wallDt = Math.min(0.1, (now - lastT) / 1000);
  lastT = now;

  // Advance sim in real time: wallDt seconds -> wallDt/DT substeps.
  simDebt += wallDt / DT;
  const want = Math.floor(simDebt);
  const steps = sim.advance(Math.min(want, MAX_STEPS_PER_FRAME));
  simDebt -= want; // dropped steps = sim-time dilation, by design

  const live = unpackFrame(sim.frame());

  ctx.clearRect(0, 0, canvas.width, canvas.height);
  // grid
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

  // hover tooltip: live values, always on
  const hover = mouse ? elementAt(mouse.x, mouse.y) : undefined;
  if (hover && mouse) {
    const l = live.get(hover.id);
    if (l) {
      tip.style.display = 'block';
      tip.style.left = `${mouse.x + 14}px`;
      tip.style.top = `${mouse.y + 14}px`;
      tip.textContent =
        `${hover.kind.t}\n` +
        `V(a) ${fmt(l.va, 'V')}   V(b) ${fmt(l.vb, 'V')}\n` +
        `I ${fmt(l.current, 'A')}   P ${fmt(l.power, 'W')}`;
    }
  } else {
    tip.style.display = 'none';
  }

  hud.textContent =
    `EE Game — M1 demo   sim t = ${sim.time().toFixed(3)} s   ` +
    `${steps} steps/frame${sim.isQuarantined() ? '   ⚠ QUARANTINED' : ''}\n` +
    `click the switch`;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
