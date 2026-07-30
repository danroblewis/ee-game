// Dev-only headless benchmark for the big-world client paths. NOT shipped:
// nothing imports it, so the bundle never sees it.
//
//   pnpm --filter @ee/app bench
//
// It answers the questions that decide whether a 50k-element room is
// playable in the browser: how long the spatial index takes to build, what
// one frame's viewport cull costs at each zoom band, how many elements
// survive the cull, and how much the pointer hit-test saves by asking the
// index instead of scanning the document. Canvas RASTERIZATION is NOT
// measured here (there is no GPU under node) — the numbers below are the
// JS-side per-frame work only, and the report says so.

import type { ElementSpec, ElemLive, Point } from './circuit';
import { drawElementsLod, drawGrid, gridStep, hitTest, type Camera } from './render';
import { SpatialIndex } from './spatial';

// ---------------------------------------------------------------- stubs
/** Minimal Path2D / 2D-context stand-ins: the LOD pass builds paths and
 * strokes them, and we want the cost of OUR loop, not of a rasterizer. */
/** Every constructed path counts its segments here, so the grid pass can be
 * checked against its "bounded primitives" claim. */
let segments = 0;
class StubPath {
  moveTo() {}
  lineTo() {
    segments++;
  }
}
class StubCtx {
  strokeStyle = '';
  fillStyle = '';
  lineWidth = 1;
  lineCap = 'round';
  globalAlpha = 1;
  strokes = 0;
  rects = 0;
  save() {}
  restore() {}
  stroke() {
    this.strokes++;
  }
  fillRect() {
    this.rects++;
  }
}
Object.assign(globalThis, { Path2D: StubPath });

const VIEW_W = 1920;
const VIEW_H = 1080;

// ------------------------------------------------------- synthetic world
/** One 16x16-unit cell holding a 5-element loop, tiled into a square city.
 * 5000 elements => 1000 cells => a 512x512-unit district; 20000 => 1024². */
function syntheticDoc(n: number): ElementSpec[] {
  const out: ElementSpec[] = [];
  const cells = Math.ceil(n / 5);
  const side = Math.ceil(Math.sqrt(cells));
  const PITCH = 16;
  let id = 1;
  for (let c = 0; c < cells && out.length < n; c++) {
    const cx = (c % side) * PITCH;
    const cy = Math.floor(c / side) * PITCH;
    const p = (dx: number, dy: number): Point => [cx + dx, cy + dy];
    const cell: ElementSpec[] = [
      { id: id++, kind: { t: 'VoltageSource', dc: 9, amp: 0, hz: 0, phase: 0 }, pins: [p(0, 0), p(0, 6)] },
      { id: id++, kind: { t: 'Wire' }, pins: [p(0, 0), p(6, 0)] },
      { id: id++, kind: { t: 'Resistor', ohms: 1000 }, pins: [p(6, 0), p(10, 0)] },
      { id: id++, kind: { t: 'Lamp', ohms: 90, rated_watts: 1 }, pins: [p(10, 0), p(10, 6)] },
      { id: id++, kind: { t: 'Ground' }, pins: [p(0, 6)] },
    ];
    for (const e of cell) if (out.length < n) out.push(e);
  }
  return out;
}

/** A plausible solver frame: real numbers so the LOD color bucketing does
 * the same work it does in the game. */
function syntheticLive(elems: ElementSpec[]): Map<number, ElemLive> {
  const live = new Map<number, ElemLive>();
  for (const e of elems) {
    const v0 = ((e.id * 37) % 200) / 10 - 10; // -10..+10 V
    live.set(e.id, {
      id: e.id,
      npins: e.pins.length,
      v: [v0, v0 * 0.5, 0, 0, 0, 0],
      i: [0.01, -0.01, 0, 0, 0, 0],
      power: 0.09,
    });
  }
  return live;
}

// ---------------------------------------------------------------- timing
function bestOf(runs: number, fn: () => void): { best: number; mean: number } {
  let best = Infinity;
  let total = 0;
  for (let k = 0; k < runs; k++) {
    const t0 = performance.now();
    fn();
    const dt = performance.now() - t0;
    if (dt < best) best = dt;
    total += dt;
  }
  return { best, mean: total / runs };
}

const ms = (v: number) => `${v.toFixed(3)} ms`;
const us = (v: number) => `${(v * 1000).toFixed(1)} µs`;

/** Same camera math main.ts uses: center the view on the world's middle. */
function camAt(scale: number, elems: ElementSpec[]): Camera {
  let x1 = 0;
  let y1 = 0;
  for (const e of elems) {
    for (const p of e.pins) {
      if (p[0] > x1) x1 = p[0];
      if (p[1] > y1) y1 = p[1];
    }
  }
  return { scale, ox: VIEW_W / 2 - (x1 / 2) * scale, oy: VIEW_H / 2 - (y1 / 2) * scale };
}

const ZOOMS = [48, 24, 12, 6, 2, 0.8, 0.4];
const LOD_FULL = 6;
const LOD_CHAIN = 2;
const SORT_LIMIT = 3000;

/** The grid pass is document-independent: it must stay bounded at every
 * zoom, which is the whole point of the LOD levels. */
function gridReport() {
  console.log('\n=== background grid (bounded primitives) ===');
  console.log('zoom px/unit    step   primitives          time');
  for (const scale of ZOOMS) {
    const cam: Camera = { scale, ox: 137.5, oy: 61.25 }; // a deliberately unaligned origin
    const gctx = new StubCtx();
    const t = bestOf(50, () => {
      gctx.rects = 0;
      segments = 0;
      drawGrid(gctx as unknown as CanvasRenderingContext2D, cam, VIEW_W, VIEW_H);
    });
    const prims = gctx.rects > 0 ? `${gctx.rects} dots` : `${segments} lines`;
    console.log(
      `${String(scale).padStart(11)}   ${String(gridStep(scale)).padStart(5)}   ` +
        `${prims.padStart(11)}   ${ms(t.best).padStart(11)}`,
    );
  }
}

function report(n: number) {
  const elems = syntheticDoc(n);
  const live = syntheticLive(elems);
  const space = new SpatialIndex();

  const build = bestOf(5, () => space.rebuild(elems));
  const st = space.stats();
  console.log(`\n=== ${n} elements ===`);
  console.log(
    `index build       ${ms(build.best)} best, ${ms(build.mean)} mean  ` +
      `(${st.cells} buckets, ${st.load.toFixed(1)} ids/bucket, ${st.big} oversized)`,
  );

  // Incremental maintenance: what a move-drag or a paste costs per element.
  const movers = elems.slice(0, 1000);
  const inc = bestOf(20, () => {
    for (const e of movers) {
      e.pins = e.pins.map(([x, y]) => [x + 1, y] as Point);
      space.update(e);
    }
  });
  console.log(`update() x1000    ${ms(inc.best)} best  => ${us(inc.best / 1000)}/element`);
  const churn = bestOf(20, () => {
    for (const e of movers) space.remove(e.id);
    for (const e of movers) space.insert(e);
  });
  console.log(`remove+insert x1k ${ms(churn.best)} best  => ${us(churn.best / 2000)}/op`);
  space.rebuild(elems);

  // Per-frame cull + LOD draw-list build at each zoom band.
  const ctx = new StubCtx() as unknown as CanvasRenderingContext2D;
  const visible: ElementSpec[] = [];
  console.log('zoom px/unit   visible   cull(+sort)      draw-list      LOD');
  for (const scale of ZOOMS) {
    const cam = camAt(scale, elems);
    const [gx0, gy0] = [(0 - cam.ox) / scale, (0 - cam.oy) / scale];
    const [gx1, gy1] = [(VIEW_W - cam.ox) / scale, (VIEW_H - cam.oy) / scale];
    const cull = bestOf(50, () => {
      space.query(gx0 - 2, gy0 - 2, gx1 + 2, gy1 + 2, visible);
      if (visible.length <= SORT_LIMIT) space.sortByDoc(visible);
    });
    const lod = scale >= LOD_FULL ? 'symbols' : scale < LOD_CHAIN ? '1 seg/elem' : 'chains';
    let draw = { best: 0, mean: 0 };
    if (scale < LOD_FULL) {
      draw = bestOf(50, () => drawElementsLod(ctx, cam, visible, live, scale < LOD_CHAIN));
    }
    console.log(
      `${String(scale).padStart(11)}   ${String(visible.length).padStart(7)}   ` +
        `${ms(cull.best).padStart(11)}   ${(scale < LOD_FULL ? ms(draw.best) : '—').padStart(11)}   ${lod}`,
    );
  }

  // Pointer paths: index-backed hit-test vs the old full-document scan.
  const cam = camAt(48, elems);
  const spots = Array.from({ length: 200 }, (_, k) => ({
    x: (k * 97) % VIEW_W,
    y: (k * 61) % VIEW_H,
  }));
  const scratch: ElementSpec[] = [];
  const indexed = bestOf(20, () => {
    for (const s of spots) {
      const gx = (s.x - cam.ox) / cam.scale;
      const gy = (s.y - cam.oy) / cam.scale;
      const pad = 14 / cam.scale + 1;
      space.query(gx - pad, gy - pad, gx + pad, gy + pad, scratch);
      let best = 14;
      for (const e of scratch) best = Math.min(best, hitTest(cam, e, s.x, s.y));
    }
  });
  const scanned = bestOf(3, () => {
    for (const s of spots) {
      let best = 14;
      for (const e of elems) best = Math.min(best, hitTest(cam, e, s.x, s.y));
    }
  });
  console.log(
    `elementAt         ${us(indexed.best / spots.length)}/call indexed  vs  ` +
      `${us(scanned.best / spots.length)}/call full scan  ` +
      `(${(scanned.best / indexed.best).toFixed(0)}x)`,
  );
}

console.log(
  'EE Game client scale bench — viewport 1920x1080, JS-side work only\n' +
    '(no canvas rasterization: this measures index/cull/hit-test, not the GPU)',
);
gridReport();
report(5000);
report(20000);
console.log(
  '\nNOTE: the SIMULATION is a separate budget. sim-core solves a dense LU\n' +
    '(O(n^3) refactor); the sparse fixed-pattern path is milestone S3 and is\n' +
    'not built yet, so a room with thousands of NODES will dilate sim time\n' +
    'long before rendering breaks a sweat.',
);
