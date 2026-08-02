// Engineering notation: ONE formatter and ONE parser for every number a
// player reads or types.
//
// WHY THIS FILE EXISTS. There were five hand-rolled copies of the same
// `fmtSI` ladder (scope.ts, dock.ts, panel.ts, hoist.ts, and a dead one in
// main.ts). Every copy stopped at kilo on top and at micro or nano on the
// bottom, and every copy returned the literal string "0 <unit>" below its
// floor. Measured, before this file landed:
//
//     1 MΩ   -> "1000.0 kΩ"      (a prefix exists for that; it is M)
//     1 GΩ   -> "1000000.0 kΩ"
//     100 nF -> "0 F"            <-- the synth rooms are full of 100 nF caps
//     10 pF  -> "0 F"
//     400 µA -> "0.00 A"         <-- on the in-game multimeter
//
// The last three are not a cosmetic problem. The design pillar is that every
// number a player sees comes from the solver and nothing about the
// electrical behaviour is faked. A meter that receives 4e-4 A from the solver
// and prints "0.00 A" has thrown the solver's answer away and printed a
// falsehood in its place. It teaches the player that a live circuit carries
// no current. So: the hard invariant of this module is
//
//     A NON-ZERO VALUE NEVER FORMATS AS ZERO.
//
// It is asserted over the whole pico..giga range in `unitcheck.ts`, which is
// the acceptance test for this file.
//
// Engineering notation (as opposed to plain scientific notation) means the
// exponent is always a multiple of three, so the mantissa always lands in
// [1, 1000) and there is always a spoken prefix for it: 4.7 kΩ, 47.0 nF,
// 220 pF, 1.00 MΩ. That is how the values are written on the parts, in the
// datasheets, and by the room authors in this repo (`4.7e-6`, `47e-9`).
//
// ---------------------------------------------------------------------------
// ROUND TRIP
//
// The entry fields must satisfy: opening a dialog and pressing enter never
// changes the document. Two independent mechanisms guarantee that, because
// one alone is not enough:
//
//  1. `fmtEntry` searches for the SHORTEST significant-figure count whose
//     string parses back to the identical double. 1e-5 prints "10 µF", not
//     "10.0000 µF"; 4700 prints "4.7 kΩ". A value the player typed comes back
//     looking like what they typed.
//  2. The entry widget also refuses to emit an op when the text is unchanged
//     from what was rendered into it. That covers the pathological residue —
//     e.g. `10860.000000000004`, a float artifact left in a saved room by an
//     old knob drag, which no short decimal string can reproduce exactly.
//
// Exactness comes from never doing float arithmetic on the way back: the
// formatter builds its digits by shifting the decimal point in
// `Number.prototype.toExponential`'s output (pure string work, correctly
// rounded by the runtime), and the parser hands the digits straight back to
// `Number("<digits>e<exp>")`, which is also correctly rounded. Multiplying by
// `Math.pow(10, -9)` instead would lose a unit in the last place and the
// round trip would fail on perfectly ordinary values.

import { E_SERIES, type SeriesName } from './eseries';

// ---------------------------------------------------------------- quantities

/** What a number IS: its unit, its legal range, and how to print it.
 *
 *  A single `fmt(v, unit)` is not enough for this game, because six of the
 *  editable fields must NOT get metric prefixes — `beta`, `wiper`, `phase`,
 *  `color` and `seed` are pure numbers, and "500 m" for a beta of 0.5 would
 *  be actively wrong. So the unit travels with a descriptor, and the same
 *  descriptor is what the parser range-checks against. */
export interface Quantity {
  /** Display unit, already in the glyphs the UI uses (Ω is U+03A9). */
  unit: string;
  /** Do metric prefixes apply? False for pure numbers. */
  prefixed: boolean;
  /** Significant figures for a LIVE readout. Fixed, so a meter that is
   *  wobbling in the 4th digit does not change width every frame. */
  sig: number;
  /** Lower bound. Meaningless when `signed` — use `max` as a magnitude. */
  min: number;
  max: number;
  /** True => the bound is on the magnitude, and zero and negatives are legal
   *  (a battery may be -5 V). False => a strictly-positive range (a resistor
   *  may not be -5 kΩ). This asymmetry is exactly the server's: `mag_ok` vs
   *  `in_range` in crates/sim-core/src/validate.rs. */
  signed: boolean;
  /** Whole numbers only (seed, color). */
  integer?: boolean;
  /** The sentence shown when the value is out of range. Kept word-for-word
   *  from validate.rs where one exists, so the client and the server say the
   *  same thing rather than two different things about one rejection. */
  hint: string;
  /** PART 2, PROTOTYPE ONLY, OFF BY DEFAULT. Which preferred-value series a
   *  real-world part of this kind is stocked in — set only for the passives a
   *  player actually buys (R, C, L and zener Vz), never for machine internals
   *  or sources. Nothing reads this unless the player opts in; see
   *  eseries.ts. */
  series?: SeriesName;
}

const DEFAULTS: Quantity = {
  unit: '',
  prefixed: true,
  sig: 3,
  min: -Infinity,
  max: Infinity,
  signed: true,
  hint: 'must be a finite number',
};

/** Build a Quantity from a unit string plus overrides. */
export function quantity(unit: string, over: Partial<Quantity> = {}): Quantity {
  return { ...DEFAULTS, unit, ...over };
}

/** Anything a display site may pass as its "unit": a full descriptor, or the
 *  bare unit string for the ad-hoc readouts (scope divisions, dock chips)
 *  that have no document field behind them. */
export type UnitLike = Quantity | string;

const bareCache = new Map<string, Quantity>();
export function asQuantity(q: UnitLike): Quantity {
  if (typeof q !== 'string') return q;
  let hit = bareCache.get(q);
  if (!hit) {
    hit = quantity(q);
    bareCache.set(q, hit);
  }
  return hit;
}

// The server's gate, mirrored. crates/sim-core/src/validate.rs:131.
export const MIN_OHMS = 1e-6;
export const MAX_OHMS = 1e12;
export const MIN_FARADS = 1e-15;
export const MAX_FARADS = 1e3;
export const MIN_HENRIES = 1e-12;
export const MAX_HENRIES = 1e6;
export const MAX_SOURCE_VOLTS = 1e6;
export const MAX_SOURCE_AMPS = 1e6;
export const MAX_HZ = 1e9;

const OHMS = quantity('Ω', {
  min: MIN_OHMS,
  max: MAX_OHMS,
  signed: false,
  hint: 'resistance must be a finite value between 1 µΩ and 1 TΩ',
});

/** Per-field quantities. Keyed `Kind.field`, falling back to `field`, so a
 *  motor's winding inductance can differ from an inductor's (the motor's may
 *  be zero — an ideal winding — where a part may not) without duplicating the
 *  whole table. */
const BY_FIELD: Record<string, Quantity> = {
  ohms: OHMS,
  farads: quantity('F', {
    min: MIN_FARADS,
    max: MAX_FARADS,
    signed: false,
    hint: 'capacitance must be a finite value between 1 fF and 1 kF',
  }),
  henries: quantity('H', {
    min: MIN_HENRIES,
    max: MAX_HENRIES,
    signed: false,
    hint: 'inductance must be a finite value between 1 pH and 1 MH',
  }),
  dc: quantity('V', { max: MAX_SOURCE_VOLTS, hint: 'source voltage is limited to 1 MV' }),
  amp: quantity('V', { max: MAX_SOURCE_VOLTS, hint: 'source voltage is limited to 1 MV' }),
  volts: quantity('V', { max: MAX_SOURCE_VOLTS, hint: 'noise amplitude is limited to 1 MV' }),
  amps: quantity('A', { max: MAX_SOURCE_AMPS, hint: 'source current is limited to 1 MA' }),
  hz: quantity('Hz', { max: MAX_HZ, hint: 'source frequency is limited to 1 GHz' }),
  rated_watts: quantity('W', {
    min: 1e-9,
    max: 1e12,
    signed: false,
    hint: 'power rating must be a positive finite value',
  }),
  vz: quantity('V', {
    min: 0,
    max: MAX_SOURCE_VOLTS,
    signed: false,
    hint: 'zener voltage must be a finite value between 0 and 1 MV',
  }),
  vt: quantity('V', { max: MAX_SOURCE_VOLTS, hint: 'threshold voltage is limited to 1 MV' }),
  rail: quantity('V', {
    min: 0,
    max: MAX_SOURCE_VOLTS,
    signed: false,
    hint: 'op-amp rail must be a finite value between 0 and 1 MV',
  }),
  isc: quantity('A', {
    min: 1e-6,
    max: MAX_SOURCE_AMPS,
    signed: false,
    hint: 'op-amp output current limit must be between 1 µA and 1 MA',
  }),
  bemf: quantity('V·s/rad', {
    max: MAX_SOURCE_VOLTS,
    hint: 'back-EMF is limited to 1 MV per rad/s',
  }),
  k: quantity('A/V²', {
    min: 0,
    max: 1e9,
    signed: false,
    hint: 'transconductance must be a non-negative finite value',
  }),
  // ---- pure numbers. No prefixes: "500 m" for a beta of 0.5 is nonsense.
  beta: quantity('', {
    prefixed: false,
    min: 1e-3,
    max: 1e9,
    signed: false,
    hint: 'transistor beta must be a positive finite value',
  }),
  wiper: quantity('', {
    prefixed: false,
    sig: 2,
    min: 0,
    max: 1,
    signed: false,
    hint: 'wiper position must be between 0 and 1',
  }),
  phase: quantity('rad', { prefixed: false, hint: 'phase must be finite' }),
  color: quantity('', {
    prefixed: false,
    integer: true,
    min: 0,
    max: 4,
    signed: false,
    hint: 'colour is a whole number from 0 to 4',
  }),
  seed: quantity('', {
    prefixed: false,
    integer: true,
    min: 0,
    max: 4294967295,
    signed: false,
    hint: 'seed is a whole number from 0 to 4294967295',
  }),
};

/** Which fields are a PURCHASED PASSIVE, and therefore stocked in a
 *  preferred-value series. A motor winding is a physical fact about a motor,
 *  not a part off a reel, so it is deliberately absent — and so are all the
 *  sources, because a 9 V battery is 9 V. Part 2 prototype only. */
const SERIES_BY_FIELD: Record<string, SeriesName> = {
  // Film/carbon resistors are stocked E24 at 5%, and E24 nests E12.
  'Resistor.ohms': 'E24',
  'Potentiometer.ohms': 'E6',
  // Ceramic and film caps are E12 at best, electrolytics only E6 — the
  // dielectric cannot be trimmed to tolerance the way a film resistor can.
  'Capacitor.farads': 'E12',
  'Inductor.henries': 'E12',
  // Zeners genuinely are E24, which is where 5.1 V and 5.6 V come from —
  // both already in this repo's rooms.
  'Zener.vz': 'E24',
};

const kindOverrides: Record<string, Quantity> = {
  // A motor with no winding inductance is an ideal motor, not a broken one.
  'Motor.henries': quantity('H', {
    min: 0,
    max: MAX_HENRIES,
    signed: false,
    hint: 'winding inductance must be a non-negative finite value',
  }),
  'Noise.ohms': quantity('Ω', {
    min: MIN_OHMS,
    max: MAX_OHMS,
    signed: false,
    hint: 'source impedance must be a finite value between 1 µΩ and 1 TΩ',
  }),
  'Motor.ohms': quantity('Ω', {
    min: MIN_OHMS,
    max: MAX_OHMS,
    signed: false,
    hint: 'winding resistance must be a finite value between 1 µΩ and 1 TΩ',
  }),
};

const qCache = new Map<string, Quantity>();

/** The descriptor for one editable field of one kind. Unknown fields get a
 *  permissive dimensionless quantity rather than throwing: a field this table
 *  has not heard of is still better edited as a plain number than not at
 *  all. */
export function quantityOf(kind: string, field: string): Quantity {
  const key = `${kind}.${field}`;
  const hit = qCache.get(key);
  if (hit) return hit;
  const base = kindOverrides[key] ?? BY_FIELD[field] ?? quantity('', { prefixed: false });
  const series = SERIES_BY_FIELD[key];
  const q = series ? { ...base, series } : base;
  qCache.set(key, q);
  return q;
}

/** Human sentence for a field's legal range, for a tooltip / placeholder. */
export function rangeText(q: Quantity): string {
  if (!Number.isFinite(q.max)) return '';
  if (q.signed) return `up to ±${fmtEntry(q.max, q)}`;
  return `${fmtEntry(q.min, q)} … ${fmtEntry(q.max, q)}`;
}

// ---------------------------------------------------------------- prefixes

/** Exponent -> prefix, for OUTPUT. Every one of these is accepted by the
 *  parser too, which is what makes the round trip closed: the formatter can
 *  never emit something the parser would reject. */
const OUT_PREFIX = new Map<number, string>([
  [15, 'P'],
  [12, 'T'],
  [9, 'G'],
  [6, 'M'],
  [3, 'k'],
  [0, ''],
  [-3, 'm'],
  [-6, 'µ'],
  [-9, 'n'],
  [-12, 'p'],
  [-15, 'f'],
  [-18, 'a'],
]);

/** Prefix -> exponent, for INPUT. Deliberately CASE-SENSITIVE on m/M: they
 *  are milli and mega, six orders of magnitude apart in each direction, and
 *  silently guessing which one a player meant is exactly the class of bug
 *  this module exists to prevent. `K` is accepted for kilo (nobody minds) and
 *  `g` for giga (SPICE spells it that way); `m` is never anything but milli.
 *
 *  `E` (exa) is deliberately absent: "1E12" is exponent notation and would be
 *  ambiguous. Nothing in this game reaches 1e18 anyway. */
const IN_PREFIX: Record<string, number> = {
  P: 15,
  T: 12,
  G: 9,
  g: 9,
  M: 6,
  k: 3,
  K: 3,
  m: -3,
  u: -6,
  n: -9,
  p: -12,
  f: -15,
  a: -18,
};

/** Prefixes a player might type that this game refuses, with the reason.
 *  Refusing loudly beats reading "100c" as 1 F. */
const REJECTED_PREFIX: Record<string, string> = {
  c: 'centi',
  d: 'deci',
  D: 'deka',
  h: 'hecto',
};

/** Unit spellings accepted on input, per canonical unit. Case-insensitive
 *  except where noted. `R` is the resistance unit in the RKM code (IEC
 *  60062) — the same convention that gives us `4k7`, and the reason a
 *  datasheet writes 6R8 rather than 6.8Ω. */
const UNIT_ALIASES: Record<string, string[]> = {
  // Lower-cased forms only — `unitMatches` lower-cases before comparing, and
  // `String.toLowerCase()` maps U+03A9 OMEGA to U+03C9, not to itself.
  'Ω': ['ω', 'ohm', 'ohms', 'r'],
  F: ['f', 'farad', 'farads'],
  H: ['h', 'henry', 'henries', 'henrys'],
  V: ['v', 'volt', 'volts'],
  A: ['a', 'amp', 'amps', 'ampere', 'amperes'],
  Hz: ['hz', 'hertz'],
  W: ['w', 'watt', 'watts'],
  s: ['s', 'sec', 'secs', 'second', 'seconds'],
  m: ['m', 'metre', 'metres', 'meter', 'meters'],
  J: ['j', 'joule', 'joules'],
  rad: ['rad', 'radian', 'radians'],
};

function unitMatches(s: string, q: Quantity): boolean {
  if (s === '') return false;
  const aliases = UNIT_ALIASES[q.unit];
  const lower = s.toLowerCase();
  if (aliases) return aliases.includes(lower);
  // Compound units (A/V², V·s/rad) accept only their own spelling.
  return lower === q.unit.toLowerCase();
}

/** Normalise the glyphs that come in more than one spelling.
 *
 *  U+00B5 MICRO SIGN and U+03BC GREEK SMALL LETTER MU are visually identical
 *  and both are what a player's keyboard or paste buffer produces; plain
 *  ASCII `u` is the third spelling of micro and by far the most typed.
 *  Likewise U+03A9 GREEK CAPITAL OMEGA and U+2126 OHM SIGN.
 *
 *  U+2212 MINUS SIGN is in here because the FORMATTER emits it — so without
 *  this line the module could not read its own output and every negative
 *  value failed to round-trip. The round-trip property test caught exactly
 *  that, which is the argument for having written it.  */
function normalise(s: string): string {
  return s
    .replace(/[\u00b5\u03bc]/g, 'u')
    .replace(/[\u2126]/g, '\u03a9')
    .replace(/[\u2212\u2013\u2014]/g, '-');
}

// ---------------------------------------------------------------- formatting

export interface FmtOpts {
  /** Override the quantity's significant figures. */
  sig?: number;
  /** Drop trailing zeros: "4.7 k" rather than "4.70 k". Right for values off
   *  a discrete ladder (scope divisions) and for entry fields; wrong for a
   *  live meter, where a changing digit count is visual noise. */
  trim?: boolean;
  /** Space between number and unit. Off for the scope's cramped corners. */
  space?: boolean;
  /** Force a sign on positive values (trigger levels, offsets). */
  plus?: boolean;
}

/** Split a positive finite number into engineering mantissa digits and a
 *  power-of-1000 exponent, WITHOUT float arithmetic on the mantissa.
 *
 *  `toExponential(sig-1)` is correctly rounded by the runtime and gives us
 *  exactly `sig` digits plus a decimal exponent; the engineering form is then
 *  reached by moving the decimal point 0, 1 or 2 places right, which is pure
 *  string surgery and cannot lose a bit. Dividing by `Math.pow(10, e3)`
 *  instead loses a unit in the last place on ordinary values (try 1e-7) and
 *  breaks the round trip. */
function engDigits(a: number, sig: number): { mant: string; e3: number } {
  const es = a.toExponential(Math.max(0, Math.min(100, sig - 1)));
  const cut = es.indexOf('e');
  const digits = es.slice(0, cut).replace('.', '');
  const exp = Number(es.slice(cut + 1));
  const e3 = Math.floor(exp / 3) * 3;
  const shift = exp - e3; // 0, 1 or 2 — how many places the point moves right
  let ip: string;
  let fp: string;
  if (digits.length <= shift + 1) {
    ip = digits.padEnd(shift + 1, '0');
    fp = '';
  } else {
    ip = digits.slice(0, shift + 1);
    fp = digits.slice(shift + 1);
  }
  return { mant: fp ? `${ip}.${fp}` : ip, e3 };
}

function trimZeros(mant: string): string {
  if (!mant.includes('.')) return mant;
  return mant.replace(/0+$/, '').replace(/\.$/, '');
}

/** The formatter. Engineering notation with a real prefix, or a scientific
 *  fallback outside the prefix table — but NEVER "0 unit" for a value that is
 *  not zero. That is the invariant the whole module exists for. */
export function fmtEng(v: number, unit: UnitLike, opts: FmtOpts = {}): string {
  const q = asQuantity(unit);
  const sp = opts.space === false ? '' : ' ';
  const tail = q.unit ? `${sp}${q.unit}` : '';
  if (!Number.isFinite(v)) return `—${tail}`;
  const sig = Math.max(1, Math.round(opts.sig ?? q.sig));
  const sign = v < 0 ? '−' : opts.plus ? '+' : '';
  const a = Math.abs(v);
  if (a === 0) return `0${tail}`;

  if (!q.prefixed) {
    // Pure numbers: significant figures, no prefix, no exponent games. A
    // wiper of 0.5 is "0.5", not "500 m".
    let s = a.toPrecision(sig);
    if (s.includes('e')) s = String(Number(s));
    else if (opts.trim !== false) s = trimZeros(s);
    return `${sign}${s}${tail}`;
  }

  const { mant, e3 } = engDigits(a, sig);
  const prefix = OUT_PREFIX.get(e3);
  if (prefix === undefined) {
    // Past atto or peta. Scientific, still exact, still not zero.
    return `${sign}${a.toExponential(sig - 1)}${tail}`;
  }
  const body = opts.trim ? trimZeros(mant) : mant;
  return `${sign}${body}${sp}${prefix}${q.unit}`;
}

/** Compact form for the scope's corner readouts: no space anywhere, trailing
 *  zeros dropped. `4.7kΩ/div`, `100µs/div`. Replaces scope.ts's old
 *  `fmtTight`, which was `fmtSI(...).replace(' ', '')` over the broken
 *  ladder and so printed `0A/div` for any division below a microamp — while
 *  the very ladder that SET the division goes down to 1e-12. */
export function fmtTight(v: number, unit: UnitLike, opts: FmtOpts = {}): string {
  return fmtEng(v, unit, { trim: true, space: false, ...opts });
}

/** Distance at which two doubles are the same NUMBER for display purposes:
 *  about four units in the last place.
 *
 *  This is not sloppiness, it is the difference between a readable field and
 *  an unreadable one. The saved rooms contain values like
 *  `10860.000000000004` — an accumulator artifact from an old knob drag,
 *  which is 10860 to within 4e-16 relative and is not distinguishable from it
 *  by any measurement, any part, or any player. Demanding an EXACT round trip
 *  forces `fmtEntry` all the way out to 17 digits and puts
 *  "10.860000000000004 kΩ" in the box. Allowing a few ULP puts "10.86 kΩ"
 *  there instead, and the document is still never edited, because an
 *  untouched field emits no op at all. */
const ULP = Math.pow(2, -50);

function same(a: number, b: number): boolean {
  return a === b || Math.abs(a - b) <= Math.abs(b) * ULP;
}

/** The string an ENTRY FIELD shows for a stored value.
 *
 *  Shortest significant-figure count that parses back to the same number, so
 *  1e-5 shows "10 µF" and not "10.0000 µF", and a value the player typed
 *  comes back looking like what they typed. 17 digits always reproduce a
 *  double exactly, so the loop always terminates. */
export function fmtEntry(v: number, unit: UnitLike): string {
  const q = asQuantity(unit);
  if (!Number.isFinite(v)) return '';
  if (v === 0) return q.unit ? `0 ${q.unit}` : '0';
  for (let sig = 1; sig <= 17; sig++) {
    const s = fmtEng(v, q, { sig, trim: true });
    const p = parseEng(s, q);
    if (p.ok && same(p.value, v)) return s;
  }
  return fmtEng(v, q, { sig: 17, trim: true });
}

// ---------------------------------------------------------------- parsing

export type ParseResult = { ok: true; value: number } | { ok: false; err: string };

const bad = (err: string): ParseResult => ({ ok: false, err });

/** Resolve a suffix like "kΩ", "u", "nF", "R", "" into a power of ten.
 *
 *  Order matters: the WHOLE suffix is tested against the unit first, so a
 *  lone "F" on a capacitor is farads rather than femto and a lone "H" on an
 *  inductor is henries rather than hecto. Write "100fF" if you really mean
 *  femtofarads — which is the gate floor, so it is reachable, just not by
 *  accident. */
function suffixExp(sfx: string, q: Quantity): { exp: number } | { err: string } {
  if (sfx === '') return { exp: 0 };
  if (unitMatches(sfx, q)) return { exp: 0 };
  const head = sfx[0]!;
  const rest = sfx.slice(1);
  if (Object.prototype.hasOwnProperty.call(IN_PREFIX, head)) {
    if (!q.prefixed) return { err: `this value is a plain number — no ${head} prefix` };
    if (rest === '' || unitMatches(rest, q)) return { exp: IN_PREFIX[head]! };
    return { err: `"${rest}" is not ${q.unit ? `a spelling of ${q.unit}` : 'a unit here'}` };
  }
  if (Object.prototype.hasOwnProperty.call(REJECTED_PREFIX, head)) {
    return { err: `${REJECTED_PREFIX[head]} ("${head}") is not used here — use m, k, M, µ, n, p` };
  }
  return { err: `"${sfx}" is not a unit or prefix I know` };
}

const EXPONENT_FORM = /^([+-]?(?:\d+\.?\d*|\.\d+)[eE][+-]?\d+)(.*)$/;
// RKM code (IEC 60062): the prefix stands in for the decimal point, because a
// printed or photocopied dot goes missing and a letter does not. 4k7, 6R8,
// 1M5, 4u7. Digits-only either side, so "4.7k7" cannot match and is an error
// rather than a silently different number.
const RKM_FORM = /^([+-]?)(\d+)([A-Za-zΩ])(\d+)([A-Za-zΩ]*)$/;
const PLAIN_FORM = /^([+-]?)(\d+\.?\d*|\.\d+)(.*)$/;

/** Parse what an engineer actually types.
 *
 *  Accepts: 10u, 10uF, 10µF, 10 µF, 4.7k, 4k7, 1M, 1M5, 100n, .47u, 470R,
 *  6R8, 4700, 1e-7, -5, 5V, 100 kHz, 1 kΩ.
 *  Rejects, with a sentence rather than a NaN or a wrong magnitude: 4.7k7,
 *  100c, "1 000", 4,700, empty, "abc", "10 20".
 *
 *  Does NOT range-check — that is `parseField`, so a caller that only wants a
 *  number (the scope's timebase box) is not forced to invent a Quantity. */
export function parseEng(input: string, unit: UnitLike = ''): ParseResult {
  const q = asQuantity(unit);
  const raw = String(input).trim();
  if (raw === '') return bad('enter a value');
  if (raw.includes(',')) return bad('no thousands separators — write 4.7k or 4700');
  // "1 000" and "10 20" are two numbers, not one. Catch before whitespace is
  // stripped, or they silently become 1000 and 1020.
  if (/[\d.]\s+[\d.]/.test(raw)) return bad('one number, please');
  const s = normalise(raw).replace(/\s+/g, '');

  let mant: string;
  let sfx: string;
  let baseExp = 0;

  const ex = EXPONENT_FORM.exec(s);
  const rkm = ex ? null : RKM_FORM.exec(s);
  if (ex) {
    const lit = ex[1]!.toLowerCase();
    const at = lit.indexOf('e');
    mant = lit.slice(0, at);
    baseExp = Number(lit.slice(at + 1));
    sfx = ex[2]!;
  } else if (rkm) {
    const mid = rkm[3]!;
    mant = `${rkm[1]}${rkm[2]}.${rkm[4]}`;
    sfx = rkm[5]!;
    // The middle letter is a unit (6R8 = 6.8 Ω) or a prefix (4k7 = 4.7 kΩ).
    // Unit first, same precedence as a trailing suffix, so 4F7 is 4.7 F.
    let midExp: number;
    if (unitMatches(mid, q)) midExp = 0;
    else if (Object.prototype.hasOwnProperty.call(IN_PREFIX, mid) && q.prefixed) {
      midExp = IN_PREFIX[mid]!;
    } else {
      return bad(`"${mid}" cannot stand in for a decimal point here`);
    }
    const tail = suffixExp(sfx, q);
    if ('err' in tail) return bad(tail.err);
    if (tail.exp !== 0) return bad('one prefix per value');
    const value = Number(`${mant}e${midExp}`);
    if (!Number.isFinite(value)) return bad('not a number');
    return { ok: true, value };
  } else {
    const p = PLAIN_FORM.exec(s);
    if (!p) return bad(`"${raw}" is not a number`);
    mant = `${p[1]}${p[2]}`;
    sfx = p[3]!;
  }
  if (mant === '' || mant === '+' || mant === '-' || mant === '.') return bad('enter a value');

  const r = suffixExp(sfx, q);
  if ('err' in r) return bad(r.err);
  // Rebuild as one decimal literal and let the runtime's correctly-rounded
  // parser do the scaling. `Number("100e-9")` is exactly 1e-7; multiplying
  // 100 by Math.pow(10,-9) is not.
  const value = Number(`${mant}e${baseExp + r.exp}`);
  if (!Number.isFinite(value)) return bad('that is too large to be a value');
  return { ok: true, value };
}

/** Why a value is not allowed in a field, or null if it is.
 *
 *  Mirrors the server gate's ASYMMETRY exactly: sources are bounded by
 *  magnitude, so -5 V is a perfectly good battery; passives are bounded by
 *  range, so -5 kΩ is not a resistor. Getting that backwards would either
 *  forbid a legal circuit or wave through one the server will reject with a
 *  message the player never asked for. */
export function checkRange(v: number, q: Quantity): string | null {
  if (!Number.isFinite(v)) return 'must be a finite number';
  if (q.integer && !Number.isInteger(v)) return 'must be a whole number';
  if (q.signed) {
    if (Math.abs(v) > q.max) return q.hint;
  } else if (v < q.min || v > q.max) {
    return q.hint;
  }
  return null;
}

/** Parse AND range-check against the field's own gate. The entry widgets use
 *  this, so a value the server would refuse never leaves the client and the
 *  player sees the reason under the field instead of a rejection bouncing
 *  back off the wire a round trip later. */
export function parseField(s: string, q: Quantity): ParseResult {
  const r = parseEng(s, q);
  if (!r.ok) return r;
  const why = checkRange(r.value, q);
  return why ? bad(why) : r;
}

// ---------------------------------------------------------------- stepping

/** The 1-2-5 ladder, the house default for arrow-key stepping. Same shape as
 *  scope.ts's `nice125`, which is where the game already speaks it. */
const LADDER_125 = [1, 2, 5];

/** One rung up or down a per-decade ladder, for the arrow keys in an entry
 *  field. A text input has no native spinner, and stepping by 1 (which is
 *  what `type=number` did) is useless on a 100 nF capacitor.
 *
 *  `mants` lets the caller substitute a preferred-value series — which is how
 *  the Part 2 prototype makes the arrows walk E24 rather than 1-2-5 when the
 *  player has opted in. */
export function stepLadder(v: number, dir: 1 | -1, q: Quantity, mants: number[] = LADDER_125): number {
  if (!q.prefixed || q.integer) {
    const n = (Number.isFinite(v) ? v : 0) + dir;
    return q.integer ? Math.round(n) : n;
  }
  const a = Math.abs(v);
  const floorV = q.signed ? 0 : q.min;
  if (!Number.isFinite(a) || a <= 0) return dir > 0 ? Math.max(floorV, mants[0]!) : floorV;
  const dec = Math.pow(10, Math.floor(Math.log10(a)));
  const cands: number[] = [];
  for (const d of [dec / 10, dec, dec * 10]) for (const m of mants) cands.push(Number((m * d).toPrecision(12)));
  cands.sort((x, y) => x - y);
  const sign = v < 0 ? -1 : 1;
  // ArrowUp always moves the value NUMERICALLY up, which is what a native
  // number input does and therefore what the fingers already expect. On a
  // negative value that means toward zero: -5 V steps to -2 V, not -10 V.
  const want = dir * sign;
  let out = a;
  if (want > 0) {
    out = cands.find((c) => c > a * 1.000001) ?? cands[cands.length - 1]!;
  } else {
    for (let i = cands.length - 1; i >= 0; i--) {
      if (cands[i]! < a * 0.999999) {
        out = cands[i]!;
        break;
      }
    }
  }
  const signed = sign * out;
  if (!q.signed) return Math.min(q.max, Math.max(q.min, signed));
  return Math.abs(signed) > q.max ? sign * q.max : signed;
}

/** Mantissas of a preferred-value series, for `stepLadder`. Part 2 only. */
export function seriesLadder(name: SeriesName): number[] {
  return E_SERIES[name];
}
