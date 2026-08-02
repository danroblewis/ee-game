// Quick SVG rendering of an ElementSpec JSON for eyeballing layouts.
const fs = require('fs');
const elems = JSON.parse(fs.readFileSync(process.argv[2]));
const S = 12; // px per grid unit
const xs = elems.flatMap((e) => e.pins.map((p) => p[0]));
const ys = elems.flatMap((e) => e.pins.map((p) => p[1]));
const x0 = Math.min(...xs) - 3, x1 = Math.max(...xs) + 3;
const y0 = Math.min(...ys) - 3, y1 = Math.max(...ys) + 3;
const W = (x1 - x0) * S, H = (y1 - y0) * S;
const X = (x) => (x - x0) * S, Y = (y) => (y - y0) * S;
let out = [`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">`,
  `<rect width="${W}" height="${H}" fill="#101418"/>`];
// grid dots via one pattern (thousands of circles choke the rasterizer)
out.push(`<defs><pattern id="g" width="${S}" height="${S}" patternUnits="userSpaceOnUse">` +
  `<circle cx="0" cy="0" r="0.7" fill="#26303a"/></pattern></defs>`);
out.push(`<rect width="${W}" height="${H}" fill="url(#g)"/>`);
const colors = { Wire: '#7fd08a', default: '#e8b04a' };
for (const e of elems) {
  const t = e.kind.t;
  const P = e.pins;
  if (t === 'Wire') {
    out.push(`<line x1="${X(P[0][0])}" y1="${Y(P[0][1])}" x2="${X(P[1][0])}" y2="${Y(P[1][1])}" stroke="#7fd08a" stroke-width="1.6"/>`);
    for (const p of P) out.push(`<circle cx="${X(p[0])}" cy="${Y(p[1])}" r="2" fill="#7fd08a"/>`);
    continue;
  }
  if (t === 'Ground') {
    out.push(`<line x1="${X(P[0][0])}" y1="${Y(P[0][1])}" x2="${X(P[0][0])}" y2="${Y(P[0][1]) + 6}" stroke="#8fa3b8" stroke-width="1.6"/>`);
    out.push(`<line x1="${X(P[0][0]) - 5}" y1="${Y(P[0][1]) + 6}" x2="${X(P[0][0]) + 5}" y2="${Y(P[0][1]) + 6}" stroke="#8fa3b8" stroke-width="2"/>`);
    continue;
  }
  if (P.length === 1) {
    out.push(`<circle cx="${X(P[0][0])}" cy="${Y(P[0][1])}" r="4" fill="none" stroke="#e8b04a"/>`);
    continue;
  }
  if (P.length === 2) {
    out.push(`<line x1="${X(P[0][0])}" y1="${Y(P[0][1])}" x2="${X(P[1][0])}" y2="${Y(P[1][1])}" stroke="#e8b04a" stroke-width="2"/>`);
    const mx = (X(P[0][0]) + X(P[1][0])) / 2, my = (Y(P[0][1]) + Y(P[1][1])) / 2;
    out.push(`<text x="${mx + 3}" y="${my - 3}" fill="#c7d2dd" font-size="8" font-family="monospace">${t.slice(0, 4)}${e.id}</text>`);
  } else {
    // multi-pin: filled bbox + pin legs to centroid
    const bx0 = Math.min(...P.map((p) => p[0])), bx1 = Math.max(...P.map((p) => p[0]));
    const by0 = Math.min(...P.map((p) => p[1])), by1 = Math.max(...P.map((p) => p[1]));
    out.push(`<rect x="${X(bx0)}" y="${Y(by0)}" width="${(bx1 - bx0) * S}" height="${(by1 - by0) * S}" fill="#3a2f1a" stroke="#e8b04a" stroke-width="1.5" opacity="0.9"/>`);
    out.push(`<text x="${X(bx0) + 3}" y="${Y(by0) + 10}" fill="#ffd27f" font-size="9" font-family="monospace">${t}${e.id}</text>`);
  }
  for (const p of P) out.push(`<circle cx="${X(p[0])}" cy="${Y(p[1])}" r="2.2" fill="#ff7f6a"/>`);
}
out.push('</svg>');
fs.writeFileSync(process.argv[3], out.join('\n'));
console.error(`wrote ${process.argv[3]} (${W}x${H})`);
