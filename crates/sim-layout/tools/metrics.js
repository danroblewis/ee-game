// Readability metrics for ee-game schematic layouts.
// Input: JSON array of ElementSpec {id, kind:{t,...}, pins:[[x,y],...]}.
// Model of what the client draws (approximating render.ts):
//   Wire            -> one segment pin0-pin1                      (class W)
//   2-pin part      -> one segment pin0-pin1 (body sits on it)    (class P)
//   3+-pin part     -> star: each pin -> pin centroid             (class L)
//                      plus an obstacle bbox of its pins
//   Ground/Rail     -> point symbol, no segment
// Connectivity ground truth (engine.rs compile()): pins connect only by
// exact grid-point coincidence; Wire elements merge their two endpoints.

const fs = require('fs');

const EPS = 1e-9;
const key = (p) => p[0] + ',' + p[1];

function segsOf(e) {
  const t = e.kind.t;
  const pins = e.pins;
  if (t === 'Ground' || t === 'Rail') return [];
  if (t === 'Wire') return [{ a: pins[0], b: pins[1], cls: 'W', id: e.id }];
  if (pins.length === 2) return [{ a: pins[0], b: pins[1], cls: 'P', id: e.id }];
  // multi-pin: star to centroid
  const cx = pins.reduce((s, p) => s + p[0], 0) / pins.length;
  const cy = pins.reduce((s, p) => s + p[1], 0) / pins.length;
  return pins.map((p) => ({ a: p, b: [cx, cy], cls: 'L', id: e.id }));
}

// ---- segment intersection ------------------------------------------------
const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
const cross = (a, b) => a[0] * b[1] - a[1] * b[0];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
const eq = (a, b) => Math.abs(a[0] - b[0]) < EPS && Math.abs(a[1] - b[1]) < EPS;
const len = (s) => Math.hypot(s.b[0] - s.a[0], s.b[1] - s.a[1]);

// classify intersection of two segments:
// null | {type:'cross'|'touch'|'endpoint'|'overlap'}
// cross    = interior x interior
// touch    = endpoint of one, interior of the other (false junction look)
// endpoint = shared endpoint (a junction; electrically real iff pins coincide)
// overlap  = collinear with positive shared length
function hit(s1, s2) {
  const r = sub(s1.b, s1.a), s = sub(s2.b, s2.a);
  const qp = sub(s2.a, s1.a);
  const rxs = cross(r, s);
  if (Math.abs(rxs) < EPS) {
    if (Math.abs(cross(qp, r)) > EPS) return null; // parallel, apart
    // collinear: overlap length
    const rr = dot(r, r);
    if (rr < EPS) return null;
    let t0 = dot(qp, r) / rr;
    let t1 = t0 + dot(s, r) / rr;
    if (t0 > t1) [t0, t1] = [t1, t0];
    const lo = Math.max(0, t0), hi = Math.min(1, t1);
    if (hi - lo > EPS) return { type: 'overlap', l: (hi - lo) * Math.sqrt(rr) };
    if (hi - lo > -EPS) {
      // touch at a single collinear point -> endpoint or touch
      return pointClass(s1, s2, [s1.a[0] + r[0] * lo, s1.a[1] + r[1] * lo]);
    }
    return null;
  }
  const t = cross(qp, s) / rxs;
  const u = cross(qp, r) / rxs;
  if (t < -EPS || t > 1 + EPS || u < -EPS || u > 1 + EPS) return null;
  const p = [s1.a[0] + t * r[0], s1.a[1] + t * r[1]];
  return pointClass(s1, s2, p);
}
function pointClass(s1, s2, p) {
  const e1 = eq(p, s1.a) || eq(p, s1.b);
  const e2 = eq(p, s2.a) || eq(p, s2.b);
  if (e1 && e2) return { type: 'endpoint', p };
  if (e1 || e2) return { type: 'touch', p };
  return { type: 'cross', p };
}

// ---- well-formed footprint checks ----------------------------------------
// Looser than exact makePins congruence: axis-aligned, symmetric, and
// proportioned like the drawn symbol, uniform stretch allowed. This is the
// READABILITY test; exact canonical (below) is the coming rigidity invariant.
function isWellFormed(e) {
  const t = e.kind.t;
  const P = e.pins;
  const perp = (u) => [-u[1], u[0]];
  const eqp = (a, b) => a[0] === b[0] && a[1] === b[1];
  const add = (p, u, k) => [p[0] + u[0] * k, p[1] + u[1] * k];
  const axisOf = (d) => (d[0] !== 0 && d[1] === 0) || (d[0] === 0 && d[1] !== 0)
    ? [Math.sign(d[0]), Math.sign(d[1])] : null;
  if (t === 'Potentiometer') {
    const [a, w, b] = P;
    const u = axisOf(sub(b, a));
    if (!u) return false;
    const mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    const dw = sub(w, mid);
    // wiper on the perpendicular through the midpoint (offset 0..4), either side
    const along = dot(dw, u), across = dot(dw, perp(u));
    return Math.abs(along) < EPS && Math.abs(across) <= 4;
  }
  if (['Npn', 'Pnp', 'Nmos', 'Pmos'].includes(t)) {
    const [g, c, en] = P;
    const b = [(c[0] + en[0]) / 2, (c[1] + en[1]) / 2];
    const u = axisOf(sub(b, g));
    if (!u) return false;
    const dc_ = sub(c, b);
    // c/e symmetric about the axis, on the perpendicular at b
    return Math.abs(dot(dc_, u)) < EPS && Math.abs(dot(dc_, perp(u))) >= 1 &&
      eqp(c, add(b, sub(b, en), 1));
  }
  if (t === 'OpAmp' || t === 'Ota') {
    const [ip, im, o] = P;
    const a = [(ip[0] + im[0]) / 2, (ip[1] + im[1]) / 2];
    const u = axisOf(sub(o, a));
    if (!u) return false;
    const di = sub(ip, a);
    if (Math.abs(dot(di, u)) > EPS || Math.abs(dot(di, perp(u))) < 1) return false;
    if (!eqp(ip, add(a, sub(a, im), 1))) return false;
    if (t === 'Ota') {
      // bias anywhere alongside the body, not off in space
      const bias = P[3];
      const alo = dot(sub(bias, a), u), aco = Math.abs(dot(sub(bias, a), perp(u)));
      const L = Math.abs(dot(sub(o, a), u));
      return alo >= -1 && alo <= L + 1 && aco <= 4;
    }
    return true;
  }
  if (t === 'Timer555') {
    // generalized DIP: two axis-aligned pin columns, left = vcc/trig/thr/gnd
    // in order, right = dis/out between the rails, same orientation.
    const [vcc, gnd, trig, thr, out, dis] = P;
    for (const u of [[1, 0], [-1, 0], [0, 1], [0, -1]]) for (const sv of [1, -1]) {
      const v = [perp(u)[0] * sv, perp(u)[1] * sv];
      const X = (p) => dot(p, u), Y = (p) => dot(p, v);
      if (X(vcc) !== X(gnd) || X(vcc) !== X(trig) || X(vcc) !== X(thr)) continue;
      if (X(out) !== X(dis) || X(out) <= X(vcc)) continue;
      const yv = Y(vcc), yg = Y(gnd), yt = Y(trig), yh = Y(thr);
      if (!(yv < yt && yt < yh && yh < yg)) continue;
      const yd = Y(dis), yo = Y(out);
      if (!(yv < yd && yd < yo && yo < yg)) continue;
      return true;
    }
    return false;
  }
  return true;
}

// ---- canonical footprint checks (catalog.ts makePins, rot4 x mirror) -----
function isCanonical(e) {
  const t = e.kind.t;
  const P = e.pins;
  const axes = [[1, 0], [-1, 0], [0, 1], [0, -1]];
  const perp = (u) => [-u[1], u[0]];
  const eqp = (a, b) => a[0] === b[0] && a[1] === b[1];
  const add = (p, u, k) => [p[0] + u[0] * k, p[1] + u[1] * k];
  if (t === 'Potentiometer') {
    const [a, w, b] = P;
    const d = sub(b, a);
    if ((d[0] !== 0 && d[1] !== 0) || (d[0] === 0 && d[1] === 0)) return false;
    const mid = [Math.round((a[0] + b[0]) / 2), Math.round((a[1] + b[1]) / 2)];
    const pu = perp([Math.sign(d[0]), Math.sign(d[1])]);
    return eqp(w, add(mid, pu, 2)) || eqp(w, add(mid, pu, -2));
  }
  if (['Npn', 'Pnp', 'Nmos', 'Pmos'].includes(t)) {
    const [g, c, en] = P;
    const b = [(c[0] + en[0]) / 2, (c[1] + en[1]) / 2];
    if (b[0] !== Math.round(b[0]) || b[1] !== Math.round(b[1])) return false;
    const d = sub(b, g);
    if ((d[0] !== 0 && d[1] !== 0) || (d[0] === 0 && d[1] === 0)) return false;
    const pu = perp([Math.sign(d[0]), Math.sign(d[1])]);
    return (eqp(c, add(b, pu, -2)) && eqp(en, add(b, pu, 2))) ||
           (eqp(c, add(b, pu, 2)) && eqp(en, add(b, pu, -2)));
  }
  if (t === 'OpAmp') {
    const [ip, im, o] = P;
    const a = [(ip[0] + im[0]) / 2, (ip[1] + im[1]) / 2];
    if (a[0] !== Math.round(a[0]) || a[1] !== Math.round(a[1])) return false;
    const d = sub(o, a);
    if ((d[0] !== 0 && d[1] !== 0) || (d[0] === 0 && d[1] === 0)) return false;
    const pu = perp([Math.sign(d[0]), Math.sign(d[1])]);
    return (eqp(ip, add(a, pu, -1)) && eqp(im, add(a, pu, 1))) ||
           (eqp(ip, add(a, pu, 1)) && eqp(im, add(a, pu, -1)));
  }
  if (t === 'Ota') {
    const [ip, im, o, bias] = P;
    const a = [(ip[0] + im[0]) / 2, (ip[1] + im[1]) / 2];
    if (a[0] !== Math.round(a[0]) || a[1] !== Math.round(a[1])) return false;
    const d = sub(o, a);
    if ((d[0] !== 0 && d[1] !== 0) || (d[0] === 0 && d[1] === 0)) return false;
    const u = [Math.sign(d[0]), Math.sign(d[1])];
    const pu = perp(u);
    for (const s of [1, -1]) {
      if (eqp(ip, add(a, pu, -s)) && eqp(im, add(a, pu, s))) {
        const tip = add(o, u, -1);
        if (eqp(bias, add(tip, pu, -2 * s)) || eqp(bias, add(tip, pu, 2 * s))) return true;
      }
    }
    return false;
  }
  if (t === 'Timer555') {
    // [vcc,gnd,trig,thr,out,dis] = at(0,0),(0,4),(0,1),(0,3),(4,3),(4,1)
    const proto = [[0, 0], [0, 4], [0, 1], [0, 3], [4, 3], [4, 1]];
    for (const ux of axes) for (const sy of [1, -1]) {
      const uy = [(-ux[1]) * sy, ux[0] * sy];
      const at = (q) => [P[0][0] + ux[0] * q[0] + uy[0] * q[1], P[0][1] + ux[1] * q[0] + uy[1] * q[1]];
      if (proto.every((q, i) => eqp(P[i], at(q)))) return true;
    }
    return false;
  }
  return true; // 2-pin and 1-pin parts have no canonical footprint to break
}

// ---- union-find over grid points (wires merge endpoints) -----------------
function nets(elems) {
  const idx = new Map();
  const pts = [];
  const at = (p) => {
    const k = key(p);
    if (!idx.has(k)) { idx.set(k, pts.length); pts.push(p); }
    return idx.get(k);
  };
  for (const e of elems) for (const p of e.pins) at(p);
  const par = pts.map((_, i) => i);
  const find = (i) => { while (par[i] !== i) { par[i] = par[par[i]]; i = par[i]; } return i; };
  for (const e of elems) {
    if (e.kind.t === 'Wire') {
      const a = find(at(e.pins[0])), b = find(at(e.pins[1]));
      par[a] = b;
    }
  }
  // ground: all Ground pins merge (node 0)
  let groot = -1;
  for (const e of elems) if (e.kind.t === 'Ground') {
    const a = find(at(e.pins[0]));
    if (groot < 0) groot = a; else par[a] = groot = find(groot);
  }
  return { at, find, pts };
}

function analyze(name, elems) {
  const parts = elems.filter((e) => e.kind.t !== 'Wire');
  const wires = elems.filter((e) => e.kind.t === 'Wire');
  const segs = elems.flatMap(segsOf);
  const wireSegs = segs.filter((s) => s.cls === 'W');
  const partAxes = segs.filter((s) => s.cls === 'P');

  // pin coincidence degree per grid point
  const deg = new Map();
  for (const e of elems) for (const p of e.pins) {
    deg.set(key(p), (deg.get(key(p)) || 0) + 1);
  }
  const degs = [...deg.values()];
  const hubs = degs.filter((d) => d >= 4).length;
  const maxDeg = Math.max(...degs);

  // pairwise segment interactions (different elements only)
  let nCross = 0, nTouch = 0, nOverlap = 0, overlapLen = 0;
  const crossBreak = {};
  for (let i = 0; i < segs.length; i++) for (let j = i + 1; j < segs.length; j++) {
    const s1 = segs[i], s2 = segs[j];
    if (s1.id === s2.id) continue;
    const h = hit(s1, s2);
    if (!h) continue;
    if (h.type === 'cross') {
      nCross++;
      const k = [s1.cls, s2.cls].sort().join('');
      crossBreak[k] = (crossBreak[k] || 0) + 1;
    } else if (h.type === 'touch') nTouch++;
    else if (h.type === 'overlap') { nOverlap++; overlapLen += h.l; }
  }

  // wires/axes through multi-pin part bodies (obstacle bboxes)
  let bodyHits = 0;
  const boxes = parts.filter((e) => e.pins.length >= 3).map((e) => {
    const xs = e.pins.map((p) => p[0]), ys = e.pins.map((p) => p[1]);
    return { id: e.id, x0: Math.min(...xs), x1: Math.max(...xs), y0: Math.min(...ys), y1: Math.max(...ys) };
  });
  for (const s of segs) for (const b of boxes) {
    if (s.id === b.id) continue;
    // does segment s pass through the OPEN interior of box b?
    const steps = 64;
    let inside = false;
    for (let k = 1; k < steps; k++) {
      const x = s.a[0] + ((s.b[0] - s.a[0]) * k) / steps;
      const y = s.a[1] + ((s.b[1] - s.a[1]) * k) / steps;
      if (x > b.x0 + 0.25 && x < b.x1 - 0.25 && y > b.y0 + 0.25 && y < b.y1 - 0.25) { inside = true; break; }
    }
    if (inside) bodyHits++;
  }

  // diagonals & lengths
  const diagWires = wireSegs.filter((s) => s.a[0] !== s.b[0] && s.a[1] !== s.b[1]).length;
  const diagAxes = partAxes.filter((s) => s.a[0] !== s.b[0] && s.a[1] !== s.b[1]).length;
  const wireLen = wireSegs.reduce((t, s) => t + len(s), 0);
  const axisLens = partAxes.map(len);
  const longAxes = axisLens.filter((l) => l > 6).length; // stretched 2-pin part
  const maxAxis = axisLens.length ? Math.max(...axisLens) : 0;

  // multi-pin footprint compliance + span
  const multi = parts.filter((e) => e.pins.length >= 3);
  const malformed = multi.filter((e) => !isWellFormed(e));
  const nonCanon = multi.filter((e) => !isCanonical(e));
  const spans = multi.map((e) => {
    const xs = e.pins.map((p) => p[0]), ys = e.pins.map((p) => p[1]);
    return Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
  });
  const maxSpan = spans.length ? Math.max(...spans) : 0;

  // nets + detour: wire length spent per net vs MST lower bound over its terminals
  const { at, find } = nets(elems);
  const netTerm = new Map(); // root -> set of terminal point keys (non-wire pins)
  for (const e of parts) for (const p of e.pins) {
    const r = find(at(p));
    if (!netTerm.has(r)) netTerm.set(r, new Map());
    netTerm.get(r).set(key(p), p);
  }
  const netWire = new Map();
  const wireEnds = new Map(); // root -> set of wire-endpoint keys
  for (const w of wires) {
    const r = find(at(w.pins[0]));
    netWire.set(r, (netWire.get(r) || 0) + len({ a: w.pins[0], b: w.pins[1] }));
    if (!wireEnds.has(r)) wireEnds.set(r, new Set());
    wireEnds.get(r).add(key(w.pins[0]));
    wireEnds.get(r).add(key(w.pins[1]));
  }
  let detourNum = 0, detourDen = 0, nNets = 0, nMultiNets = 0;
  for (const [r, term] of netTerm) {
    nNets++;
    if ([...term.values()].length >= 2) nMultiNets++;
    // detour: wires vs Manhattan MST over the terminals the wires REACH.
    // Terminals joined by direct pin abutment need no wire and are excluded.
    const we = wireEnds.get(r);
    if (!we) continue;
    const pts = [...term.entries()].filter(([k]) => we.has(k)).map(([, p]) => p);
    if (pts.length < 2) continue;
    // Manhattan MST (Prim)
    const inT = new Array(pts.length).fill(false);
    const d = new Array(pts.length).fill(Infinity);
    d[0] = 0;
    let mst = 0;
    for (let n = 0; n < pts.length; n++) {
      let bi = -1;
      for (let i = 0; i < pts.length; i++) if (!inT[i] && (bi < 0 || d[i] < d[bi])) bi = i;
      inT[bi] = true; mst += d[bi];
      for (let i = 0; i < pts.length; i++) if (!inT[i]) {
        const dd = Math.abs(pts[i][0] - pts[bi][0]) + Math.abs(pts[i][1] - pts[bi][1]);
        if (dd < d[i]) d[i] = dd;
      }
    }
    detourNum += netWire.get(r) || 0;
    detourDen += mst;
  }

  const xs = elems.flatMap((e) => e.pins.map((p) => p[0]));
  const ys = elems.flatMap((e) => e.pins.map((p) => p[1]));
  const area = (Math.max(...xs) - Math.min(...xs)) * (Math.max(...ys) - Math.min(...ys));

  const nE = elems.length;
  const m = {
    name,
    elements: nE,
    parts: parts.length,
    wires: wires.length,
    nets: nNets,
    multiNets: nMultiNets,
    bboxArea: area,
    crossings: nCross,
    crossPer10El: +(10 * nCross / nE).toFixed(2),
    crossBreak,
    falseTouches: nTouch,
    collinearOverlaps: nOverlap,
    overlapLen: +overlapLen.toFixed(1),
    bodyIntrusions: bodyHits,
    diagWires,
    diagPartAxes: diagAxes,
    diagFrac: +((diagWires + diagAxes) / Math.max(1, wireSegs.length + partAxes.length)).toFixed(3),
    wireLen: +wireLen.toFixed(1),
    stretched2pin: longAxes,
    max2pinLen: +maxAxis.toFixed(1),
    multiPinParts: multi.length,
    malformedMulti: malformed.length,
    malformedIds: malformed.map((e) => e.id + ':' + e.kind.t),
    nonCanonicalMulti: nonCanon.length,
    maxMultiSpan: maxSpan,
    // rubber-band abuse: grid length parts are stretched past a nominal
    // symbol (4 for a 2-pin body, 6 for a package span)
    partStretchExcess: +(
      axisLens.reduce((t, l) => t + Math.max(0, l - 4), 0) +
      spans.reduce((t, s) => t + Math.max(0, s - 6), 0)
    ).toFixed(1),
    hubs4plus: hubs,
    maxPinDegree: maxDeg,
    detourRatio: detourDen ? +(detourNum / detourDen).toFixed(2) : 0,
  };
  // Composite (per element, weights from Purchase et al. + analog ASG work:
  // crossings dominate; ambiguity (touch/overlap/body) next; distortion of
  // known part shapes and diagonals are schematic-specific readability
  // killers; hubs measure the rat's-nest star look.)
  const bad =
    3.0 * nCross + 2.0 * nTouch + 5.0 * nOverlap + 2.0 * bodyHits +
    2.0 * (diagWires + diagAxes) + 3.0 * malformed.length +
    1.0 * longAxes + 1.5 * hubs;
  m.insanityPer10El = +((10 * bad) / nE).toFixed(2);
  return m;
}

const dir = __dirname;
const out = [];
for (const f of process.argv.slice(2)) {
  const elems = JSON.parse(fs.readFileSync(dir + '/' + f + '.json'));
  out.push(analyze(f, elems));
}
console.log(JSON.stringify(out, null, 2));
