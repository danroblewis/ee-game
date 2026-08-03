// PREFERRED VALUES (the E-series) — PROTOTYPE, OFF BY DEFAULT.
//
// STATUS. This file is machinery plus an explanation. Nothing in it changes
// any value, any room, or any existing behaviour unless a player explicitly
// opts in (see `stdValuesMode` below, default 'off'). Constraining part
// values to a series is a GAMEPLAY decision the owner has not made, and
// making it silently would rewrite every saved room. So: the nearest-value
// machinery is built and tested, one non-blocking affordance is wired behind
// the flag, and the decision is left where it belongs.
//
// ---------------------------------------------------------------------------
// WHY 4.7 — the question worth answering properly
//
// The E-series are GEOMETRIC, not decimal. E-n is the n-th roots of ten:
// value(k) = round(10^(k/n)). Because the steps are a constant RATIO rather
// than a constant amount, the same twelve numbers work in every decade and in
// every unit — ohms, farads, henries — which is the first thing that makes
// them worth having.
//
// The ratio is not arbitrary. It is chosen so that TOLERANCE BANDS TILE THE
// NUMBER LINE. A ±10% part covers ±10% around its nominal value; consecutive
// E12 values differ by 10^(1/12) = 1.2115, i.e. 21.2%, and half of that step
// is 10.1% — just over the tolerance. So the band around one stock value ends
// where the band around the next begins: every value you could possibly want
// is within tolerance of something on the shelf, and no two parts overlap
// enough to be wasted stock. E6 does the same job for ±20% parts and E24 for
// ±5%. Pair the wrong series with the wrong tolerance and you get holes you
// cannot buy your way out of.
//
// And 4.7 is there because 10^(8/12) = 4.6416. The exact E12 ladder is
// 1.000 1.212 1.468 1.778 2.154 2.610 3.162 3.831 4.642 5.623 6.813 8.254,
// which rounds to the numbers printed on the parts: 10 12 15 18 22 27 33 39
// 47 56 68 82. (3.831 is where 3.9 comes from — a common mix-up.)
//
// TWO HONEST FOOTNOTES, because this game does not teach tidied-up facts:
//
//  1. The ROUNDING breaks the perfect tiling. 3.162 became 3.3 (+4.4%) and
//     2.610 became 2.7 (+3.4%) because those values were already in
//     warehouses in 1950 and IEC would not change them. The printed E12 at
//     ±10% therefore leaves a small hole between 1.32 and 1.35. The ideal is
//     geometric; the shipped series is the ideal rounded to numbers that fit
//     on a resistor body, and industry ate a ~2% coverage gap to get there.
//  2. E48 and E96 are marginally COARSER than their tolerance covers — the
//     "no gaps" property is true of E3..E24 and slightly false above.
//
// Provenance: not electronics at all. Col. Charles Renard, 1877, replaced 425
// balloon mooring-cable sizes with a geometric series of 17. ISO 3 today. IEC
// applied it to passives in 1950; IEC 60063 now. The `4k7` spelling the
// parser accepts is its sibling, IEC 60062 (the RKM code) — the prefix stands
// in for the decimal point because a printed dot goes missing in a photocopy
// and a letter does not.

export type SeriesName = 'E3' | 'E6' | 'E12' | 'E24' | 'E48' | 'E96';

/** Mantissas as they are actually printed on parts, 1.0 <= m < 10. */
export const E_SERIES: Record<SeriesName, number[]> = {
  E3: [1.0, 2.2, 4.7],
  E6: [1.0, 1.5, 2.2, 3.3, 4.7, 6.8],
  E12: [1.0, 1.2, 1.5, 1.8, 2.2, 2.7, 3.3, 3.9, 4.7, 5.6, 6.8, 8.2],
  E24: [
    1.0, 1.1, 1.2, 1.3, 1.5, 1.6, 1.8, 2.0, 2.2, 2.4, 2.7, 3.0, 3.3, 3.6, 3.9, 4.3, 4.7, 5.1, 5.6,
    6.2, 6.8, 7.5, 8.2, 9.1,
  ],
  E48: [
    1.0, 1.05, 1.1, 1.15, 1.21, 1.27, 1.33, 1.4, 1.47, 1.54, 1.62, 1.69, 1.78, 1.87, 1.96, 2.05,
    2.15, 2.26, 2.37, 2.49, 2.61, 2.74, 2.87, 3.01, 3.16, 3.32, 3.48, 3.65, 3.83, 4.02, 4.22, 4.42,
    4.64, 4.87, 5.11, 5.36, 5.62, 5.9, 6.19, 6.49, 6.81, 7.15, 7.5, 7.87, 8.25, 8.66, 9.09, 9.53,
  ],
  E96: [
    1.0, 1.02, 1.05, 1.07, 1.1, 1.13, 1.15, 1.18, 1.21, 1.24, 1.27, 1.3, 1.33, 1.37, 1.4, 1.43,
    1.47, 1.5, 1.54, 1.58, 1.62, 1.65, 1.69, 1.74, 1.78, 1.82, 1.87, 1.91, 1.96, 2.0, 2.05, 2.1,
    2.15, 2.21, 2.26, 2.32, 2.37, 2.43, 2.49, 2.55, 2.61, 2.67, 2.74, 2.8, 2.87, 2.94, 3.01, 3.09,
    3.16, 3.24, 3.32, 3.4, 3.48, 3.57, 3.65, 3.74, 3.83, 3.92, 4.02, 4.12, 4.22, 4.32, 4.42, 4.53,
    4.64, 4.75, 4.87, 4.99, 5.11, 5.23, 5.36, 5.49, 5.62, 5.76, 5.9, 6.04, 6.19, 6.34, 6.49, 6.65,
    6.81, 6.98, 7.15, 7.32, 7.5, 7.68, 7.87, 8.06, 8.25, 8.45, 8.66, 8.87, 9.09, 9.31, 9.53, 9.76,
  ],
};

/** Nominal tolerance the series is designed around, as a fraction. */
export const SERIES_TOLERANCE: Record<SeriesName, number> = {
  E3: 0.4,
  E6: 0.2,
  E12: 0.1,
  E24: 0.05,
  E48: 0.02,
  E96: 0.01,
};

/** Snap a mantissa exactly: 1.0 * 1000 in floating point is 1000.0000000001
 *  often enough to matter when the result is compared for equality against a
 *  document value, and a "nearest standard value" that is not itself a
 *  standard value would be an embarrassing thing to print. */
function scaled(mant: number, decade: number): number {
  return Number((mant * decade).toPrecision(12));
}

function decadeOf(v: number): number {
  return Math.pow(10, Math.floor(Math.log10(v)));
}

/** Every series value in the decade below, at, and above `v`. */
function candidates(v: number, name: SeriesName): number[] {
  const mants = E_SERIES[name];
  const d = decadeOf(v);
  const out: number[] = [];
  for (const dd of [d / 10, d, d * 10]) for (const m of mants) out.push(scaled(m, dd));
  out.sort((a, b) => a - b);
  return out;
}

/** Nearest stock value, measured in LOG space — which is the only measure
 *  that makes sense on a geometric ladder. In linear space 3.0 kΩ looks
 *  closer to 3.3 k than to 2.7 k (300 vs 300 — a tie), but the honest
 *  question is which is the smaller percentage error, and 2.7 is 10% low
 *  where 3.3 is 10% high. Log space asks that question directly. */
export function nearestPreferred(v: number, name: SeriesName): number {
  const a = Math.abs(v);
  if (!Number.isFinite(a) || a <= 0) return 0;
  const cands = candidates(a, name);
  let best = cands[0]!;
  let bestErr = Infinity;
  for (const c of cands) {
    const err = Math.abs(Math.log(c / a));
    if (err < bestErr - 1e-15) {
      bestErr = err;
      best = c;
    }
  }
  return v < 0 ? -best : best;
}

/** The two stock values a free value sits between, for a "2.7 k / 3.3 k"
 *  style hint. Equal to each other when the value is itself on the series. */
export function preferredNeighbours(v: number, name: SeriesName): [number, number] {
  const a = Math.abs(v);
  if (!Number.isFinite(a) || a <= 0) return [0, 0];
  const cands = candidates(a, name);
  if (isPreferred(a, name)) {
    const exact = nearestPreferred(a, name);
    return [exact, exact];
  }
  let lo = cands[0]!;
  let hi = cands[cands.length - 1]!;
  for (const c of cands) if (c <= a) lo = c;
  for (let i = cands.length - 1; i >= 0; i--) if (cands[i]! >= a) hi = cands[i]!;
  return [lo, hi];
}

/** Is this value already a stock value? Compared as a RATIO, not a
 *  difference, because 0.1 relative slack means something at every scale and
 *  1e-9 absolute means nothing at 100 pF. The default tolerance is a float
 *  wobble, not an engineering tolerance: this answers "is it on the ladder",
 *  not "is it within tolerance of the ladder". */
export function isPreferred(v: number, name: SeriesName, rel = 1e-9): boolean {
  const a = Math.abs(v);
  if (!Number.isFinite(a) || a <= 0) return false;
  const n = nearestPreferred(a, name);
  return n > 0 && Math.abs(a / n - 1) <= rel;
}

/** Relative error from a value to the nearest stock value, as a fraction. */
export function preferredError(v: number, name: SeriesName): number {
  const a = Math.abs(v);
  const n = nearestPreferred(a, name);
  return n > 0 ? a / n - 1 : 0;
}

/** One rung along the series — the detent function a series-aware knob-drag
 *  would use. Unwired: the canvas value-drag does not exist yet (docs/plan.md
 *  M2), and building the drag series-aware from the start is cheaper than
 *  retrofitting it, so this is here waiting rather than bolted on. */
export function stepPreferred(v: number, dir: 1 | -1, name: SeriesName): number {
  const a = Math.abs(v);
  if (!Number.isFinite(a) || a <= 0) return dir > 0 ? E_SERIES[name][0]! : 0;
  const cands = candidates(a, name);
  const sign = v < 0 ? -1 : 1;
  if (dir * sign > 0) return sign * (cands.find((c) => c > a * 1.000001) ?? cands[cands.length - 1]!);
  for (let i = cands.length - 1; i >= 0; i--) if (cands[i]! < a * 0.999999) return sign * cands[i]!;
  return sign * cands[0]!;
}

// ------------------------------------------------------------ the explainer

/** Written for a player who has never heard of E12 and does not yet care.
 *  One line for the hint row, a paragraph for the ⓘ card. The voice is
 *  hoist.ts's `hintText`: state the fact, then the reason, then the thing you
 *  can DO with it. */
export const SERIES_ONE_LINER =
  'Real parts come in a fixed ladder of values, not a continuum — this is the nearest one you could actually buy.';

export function seriesExplainer(name: SeriesName = 'E24'): string[] {
  const tol = Math.round(SERIES_TOLERANCE[name] * 100);
  const n = Number(name.slice(1));
  return [
    `WHY YOU CANNOT BUY A 3 Ω RESISTOR`,
    `Resistors, capacitors and inductors are not made to order. They come in a ladder of` +
      ` standard values that repeats in every decade: 10, 12, 15, 18, 22, 27, 33, 39, 47, 56,` +
      ` 68, 82 — then 100, 120, 150, and so on. That ladder is called E12. ${name} is the same` +
      ` idea with ${n} rungs per decade.`,
    `The rungs are spaced by a constant RATIO, not a constant amount. Twelve equal ratio` +
      ` steps have to multiply up to exactly 10 across a decade, so each step is the twelfth` +
      ` root of ten — 1.2115, about 21% up each time. That is the whole trick: a` +
      ` 10%-tolerance part is anything within ±10% of its printed value, so consecutive` +
      ` rungs very nearly cover the gap between them. Every value you could want is close to` +
      ` something on the shelf, using the fewest different parts. ${name} rungs are spaced` +
      ` for ±${tol}% parts.`,
    `"Very nearly" is doing real work there, and the tidied-up version of this story usually` +
      ` skips it. Half a 21.2% step is 10.1%, a hair more than the ±10% a part promises, so` +
      ` there is a sliver between rungs that nothing is guaranteed to reach — around` +
      ` 1.32–1.35, for instance. In practice parts beat their tolerance and nobody notices.`,
    `That is where 4.7 comes from. It is really 10^(8/12) = 4.6416, rounded so it fits on a` +
      ` resistor body. A few of the rungs were rounded further than the maths says — 3.162` +
      ` became 3.3 — because those values were already in warehouses when the standard was` +
      ` written in 1950, and the warehouses won.`,
    `So when you need 3 Ω: you put three 1 Ω resistors in series. That is not a workaround,` +
      ` it is the normal way to hit a value — and it splits the heat three ways, which is` +
      ` often the real reason you wanted more than one part.`,
  ];
}

// ------------------------------------------------------------ the opt-in

export type StdValuesMode = 'off' | 'hint';

const LS_KEY = 'ee.stdvalues';

/** OFF BY DEFAULT, and it must stay that way until the owner decides.
 *
 *  Opt in with `?stdvalues=hint` in the URL, or from the console with
 *  `setStdValuesMode('hint')`. When off, nothing in this file is ever
 *  consulted: no hint is rendered, no value is snapped, no room is touched.
 *  The mode is a CLIENT DISPLAY setting — it never travels on the wire and it
 *  never edits a document by itself. */
export function stdValuesMode(): StdValuesMode {
  try {
    // The URL param is STICKY, because main.ts rewrites `location.search` to
    // carry the room code once it has joined — so a query flag that is only
    // read live would work for two seconds and then evaporate.
    const q = new URLSearchParams(location.search).get('stdvalues');
    if (q === 'hint' || q === 'off') {
      localStorage.setItem(LS_KEY, q);
      return q;
    }
    return localStorage.getItem(LS_KEY) === 'hint' ? 'hint' : 'off';
  } catch {
    return 'off';
  }
}

export function setStdValuesMode(m: StdValuesMode): void {
  try {
    localStorage.setItem(LS_KEY, m);
  } catch {
    /* private mode */
  }
}
