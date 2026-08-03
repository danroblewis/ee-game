// Dev-only headless check for units.ts and eseries.ts. NOT shipped: nothing
// imports it, so the bundle never sees it.
//
//   pnpm --filter @ee/app unitcheck
//
// WHY THIS EXISTS. The defect this replaces was not a crash and not a type
// error — it was five copies of a formatter that returned the string
// "0 <unit>" for any value below its floor, and typechecked clean forever
// while printing "0.00 A" on a live ammeter. No compiler can catch that. The
// only thing that catches it is running the formatter on the numbers the game
// actually produces and looking at what comes out, which is what this file
// does.
//
// TEST AT THE LAYER THE DEFECT CAN OCCUR. Three properties, in order of how
// much they matter:
//
//  1. NEVER-ZERO. For every value in every decade the game can reach, a
//     non-zero input must not format to a string that reads as zero. This is
//     the design pillar in one assertion: the solver produced a number and
//     the display must not throw it away.
//  2. ROUND TRIP. parse(fmtEntry(v)) === v EXACTLY for every DECIMAL value
//     over decades from pico to giga — i.e. for every value a player or a
//     room author could have written. Values that arrived by float
//     arithmetic (a knob drag's accumulator) are held to within one ulp
//     instead, which is the honest bound: `3.3 * 1e-6` is a different double
//     from `3.3e-6` and no display can tell them apart. The guarantee that
//     opening a dialog and pressing enter never changes the document does not
//     rest on this test at all — it rests on the entry widget refusing to
//     emit an op when the text is unchanged.
//  3. GATE AGREEMENT. The parser must refuse exactly what the server refuses,
//     including the asymmetry: a source may be negative, a resistor may not.

import {
  MAX_FARADS,
  MAX_HENRIES,
  MAX_OHMS,
  MIN_FARADS,
  MIN_HENRIES,
  MIN_OHMS,
  asQuantity,
  checkRange,
  fmtEng,
  fmtEntry,
  fmtTight,
  parseEng,
  parseField,
  quantityOf,
  rangeText,
  stepLadder,
} from './units';
import {
  E_SERIES,
  SERIES_TOLERANCE,
  isPreferred,
  nearestPreferred,
  preferredNeighbours,
  seriesExplainer,
  stepPreferred,
} from './eseries';

declare const process: { exitCode: number };

let failures = 0;
let checks = 0;
function check(name: string, ok: boolean, got?: string) {
  checks++;
  if (!ok) {
    failures++;
    console.log(`  FAIL  ${name}${got === undefined ? '' : `  (got ${got})`}`);
  }
}
function section(s: string) {
  console.log(`\n${s}`);
}

// ------------------------------------------------------------------ 1. zero

section('never prints zero for a non-zero value');
{
  const units = ['Ω', 'F', 'H', 'V', 'A', 'W', 's', 'Hz'];
  // Every decade the gate can reach, plus a spread of mantissas inside each.
  const mants = [1, 1.001, 1.5, 2.2, 4.7, 6.8, 9.99, 9.999999];
  let worst = '';
  let bad = 0;
  for (const u of units) {
    for (let e = -18; e <= 15; e++) {
      for (const m of mants) {
        for (const sign of [1, -1]) {
          const v = sign * m * Math.pow(10, e);
          if (v === 0) continue;
          for (const s of [fmtEng(v, u), fmtTight(v, u), fmtEntry(v, u)]) {
            // "reads as zero" = every digit in it is 0.
            const digits = s.replace(/[^0-9]/g, '');
            if (digits !== '' && /^0+$/.test(digits)) {
              bad++;
              if (!worst) worst = `${v} ${u} -> "${s}"`;
            }
          }
        }
      }
    }
  }
  check('no non-zero value formats as zero', bad === 0, `${bad} cases, first ${worst}`);
}
{
  // The exact values that were broken before this module. These are the
  // regression: every one of them printed "0 F" / "0.00 A" / "1000.0 kΩ".
  const table: [number, string, string][] = [
    [1e-7, 'F', '100 nF'],
    [1e-11, 'F', '10.0 pF'],
    [1e6, 'Ω', '1.00 MΩ'],
    [1e9, 'Ω', '1.00 GΩ'],
    [4e-4, 'A', '400 µA'],
    [5e-9, 'A', '5.00 nA'],
    [4700, 'Ω', '4.70 kΩ'],
    [4.7e-6, 'F', '4.70 µF'],
    [1.5e-3, 'H', '1.50 mH'],
    [0, 'A', '0 A'],
    [-2.5e-3, 'A', '−2.50 mA'],
  ];
  for (const [v, u, want] of table) {
    const got = fmtEng(v, u);
    check(`fmtEng(${v}, ${u}) = ${want}`, got === want, `"${got}"`);
  }
  check('exact zero still reads zero', fmtEng(0, 'F') === '0 F', fmtEng(0, 'F'));
  check('non-finite reads as a dash', fmtEng(NaN, 'V') === '— V', fmtEng(NaN, 'V'));
}
{
  // Engineering notation means the mantissa is always in [1, 1000).
  let bad = 0;
  for (let e = -15; e <= 12; e++) {
    for (const m of [1, 3.3, 9.99]) {
      const s = fmtEng(m * Math.pow(10, e), 'Ω');
      const num = Number(s.replace(/[^0-9.]/g, ''));
      if (!(num >= 1 && num < 1000)) bad++;
    }
  }
  check('mantissa always in [1, 1000)', bad === 0, `${bad} outside`);
}

// ------------------------------------------------------------- 2. roundtrip

section('lossless round trip, pico to giga');
{
  const qs = [
    quantityOf('Resistor', 'ohms'),
    quantityOf('Capacitor', 'farads'),
    quantityOf('Inductor', 'henries'),
    quantityOf('VoltageSource', 'dc'),
    quantityOf('CurrentSource', 'amps'),
    quantityOf('VoltageSource', 'hz'),
    quantityOf('Lamp', 'rated_watts'),
    quantityOf('Npn', 'beta'),
    quantityOf('Potentiometer', 'wiper'),
  ];
  // Mantissas that exercise the awkward cases: exact powers, E-series values,
  // long decimals, and values that round UP across a decade boundary.
  const mants = [
    1, 1.2, 1.5, 2.2, 2.7, 3.3, 4.7, 5.6, 6.8, 8.2, 9.1, 9.99, 9.999, 9.9999999, 1.0000001, 3.14159,
    2.718281828, 1.001, 5.005,
  ];
  let bad = 0;
  let firstBad = '';
  let total = 0;
  let ulpBad = 0;
  let firstUlp = '';
  let ulpTotal = 0;
  for (const q of qs) {
    for (let e = -12; e <= 9; e++) {
      for (const m of mants) {
        for (const sign of q.signed ? [1, -1] : [1]) {
          // Built from a DECIMAL LITERAL, not by multiplying — `3.3 *
          // Math.pow(10,-6)` is a different double from `3.3e-6`, and only
          // the latter is a value a player or a room author can have written.
          // Those must round-trip EXACTLY.
          const v = Number(`${sign < 0 ? '-' : ''}${m}e${e}`);
          if (!Number.isFinite(v) || v === 0) continue;
          if (checkRange(v, q)) continue; // out of gate: not a document value
          if (q.integer && !Number.isInteger(v)) continue;
          total++;
          const s = fmtEntry(v, q);
          const p = parseEng(s, q);
          if (!p.ok || p.value !== v) {
            bad++;
            if (!firstBad) firstBad = `${v} ${q.unit} -> "${s}" -> ${p.ok ? p.value : p.err}`;
          }
          // The same value reached by float arithmetic — which is how a knob
          // drag or a solver-derived default gets there. Those may sit an ulp
          // off the decimal, and are required only to survive to within one.
          const w = sign * m * Math.pow(10, e);
          if (Number.isFinite(w) && w !== 0 && !checkRange(w, q) && !(q.integer && !Number.isInteger(w))) {
            ulpTotal++;
            const t2 = parseEng(fmtEntry(w, q), q);
            if (!t2.ok || Math.abs(t2.value - w) > Math.abs(w) * 1e-15) {
              ulpBad++;
              if (!firstUlp) firstUlp = `${w} -> "${fmtEntry(w, q)}"`;
            }
          }
        }
      }
    }
  }
  check(`parse(fmtEntry(v)) === v exactly, decimal values (${total})`, bad === 0, `${bad} bad, ${firstBad}`);
  check(
    `parse(fmtEntry(v)) within an ulp, float-derived values (${ulpTotal})`,
    ulpBad === 0,
    `${ulpBad} bad, ${firstUlp}`,
  );
}
{
  // Every value the shipped rooms and machines actually contain. If one of
  // these does not survive, a player opening that part's dialog and pressing
  // enter would edit the room.
  const real: [number, string][] = [
    [150e-12, 'F'],
    [220e-12, 'F'],
    [1.5e-9, 'F'],
    [6.8e-9, 'F'],
    [10e-9, 'F'],
    [15e-9, 'F'],
    [22e-9, 'F'],
    [33e-9, 'F'],
    [47e-9, 'F'],
    [100e-9, 'F'],
    [0.47e-6, 'F'],
    [1e-6, 'F'],
    [4.7e-6, 'F'],
    [6.8e-6, 'F'],
    [5e-3, 'F'],
    [1, 'Ω'],
    [8, 'Ω'],
    [45, 'Ω'],
    [60, 'Ω'],
    [90, 'Ω'],
    [100, 'Ω'],
    [160.03, 'Ω'],
    [1e3, 'Ω'],
    [3360.4, 'Ω'],
    [8329.8, 'Ω'],
    [10e3, 'Ω'],
    [47860, 'Ω'],
    [100e3, 'Ω'],
    [470e3, 'Ω'],
    [1e6, 'Ω'],
    [1.5e-3, 'H'],
    [5.1, 'V'],
    [5.6, 'V'],
    [9, 'V'],
    [12, 'V'],
    [0.025, 'A'],
  ];
  let bad = 0;
  let firstBad = '';
  for (const [v, u] of real) {
    const s = fmtEntry(v, u);
    const p = parseEng(s, u);
    if (!p.ok || p.value !== v) {
      bad++;
      if (!firstBad) firstBad = `${v} -> "${s}"`;
    }
  }
  check('every real room value round-trips', bad === 0, `${bad} bad, ${firstBad}`);
  // Spot-check the SHAPE, not just the value: the whole point is that a
  // player recognises what they typed.
  const shapes: [number, string, string][] = [
    [100e-9, 'F', '100 nF'],
    [4.7e-6, 'F', '4.7 µF'],
    [1e-5, 'F', '10 µF'],
    [470e3, 'Ω', '470 kΩ'],
    [1e6, 'Ω', '1 MΩ'],
    [4700, 'Ω', '4.7 kΩ'],
    [9, 'V', '9 V'],
    [1.5e-3, 'H', '1.5 mH'],
  ];
  for (const [v, u, want] of shapes) {
    const got = fmtEntry(v, u);
    check(`fmtEntry(${v}, ${u}) = ${want}`, got === want, `"${got}"`);
  }
}
{
  // A float artifact left in a saved room by an old knob drag (this exact
  // value is in the shipped synth room). It reads as 10.86 kΩ, which is what
  // it is to within four parts in 10^16 — and the DOCUMENT is still not
  // touched, because an unedited field emits no op. That second half is the
  // guarantee that matters, and it is asserted below.
  const v = 10860.000000000004;
  const s = fmtEntry(v, 'Ω');
  const p = parseEng(s, 'Ω');
  check('float artifact reads short', s === '10.86 kΩ', `"${s}"`);
  check('...and to within an ulp', p.ok && Math.abs(p.value - v) < v * 1e-15, `"${s}"`);
}

// ---------------------------------------------------------------- 3. parser

section('parser accepts what an engineer types');
{
  const F = quantityOf('Capacitor', 'farads');
  const R = quantityOf('Resistor', 'ohms');
  const V = quantityOf('VoltageSource', 'dc');
  const H = quantityOf('Inductor', 'henries');
  const ok: [string, number, ReturnType<typeof asQuantity>][] = [
    ['10u', 1e-5, F],
    ['10uF', 1e-5, F],
    ['10µF', 1e-5, F], // U+00B5 MICRO SIGN
    ['10μF', 1e-5, F], // U+03BC GREEK SMALL MU
    ['10 uF', 1e-5, F],
    ['  10uF  ', 1e-5, F],
    ['100n', 1e-7, F],
    ['100nF', 1e-7, F],
    ['.47u', 4.7e-7, F],
    ['0.47u', 4.7e-7, F],
    ['4u7', 4.7e-6, F],
    ['1e-7', 1e-7, F],
    ['100f', 100, F], // lone unit letter is the UNIT: 100 farads
    ['100fF', 1e-13, F], // femto needs the unit spelled out
    ['4.7k', 4700, R],
    ['4k7', 4700, R],
    ['4K7', 4700, R],
    ['1M', 1e6, R],
    ['1M5', 1.5e6, R],
    ['470R', 470, R],
    ['4R7', 4.7, R],
    ['1 kΩ', 1000, R],
    ['1kohm', 1000, R],
    ['4700', 4700, R],
    ['1G', 1e9, R],
    ['1T', 1e12, R],
    ['-5', -5, V],
    ['-5V', -5, V],
    ['−5V', -5, V], // U+2212 MINUS SIGN, which is what fmtEng emits
    ['+5', 5, V],
    ['0', 0, V],
    ['1m', 1e-3, H],
    ['1mH', 1e-3, H],
    ['1.5mH', 1.5e-3, H],
    ['1H', 1, H],
    ['1h', 1, H],
  ];
  for (const [s, want, q] of ok) {
    const r = parseEng(s, q);
    check(`parse "${s}" = ${want}`, r.ok && r.value === want, r.ok ? String(r.value) : r.err);
  }
}
{
  const R = quantityOf('Resistor', 'ohms');
  const V = quantityOf('VoltageSource', 'dc');
  // Case sensitivity on m/M is the one place strictness is mandatory: they
  // are twelve orders of magnitude apart.
  check('1m is milli', (parseEng('1m', R) as { value: number }).value === 1e-3);
  check('1M is mega', (parseEng('1M', R) as { value: number }).value === 1e6);
  const nonsense: [string, string][] = [
    ['', 'empty'],
    ['   ', 'blank'],
    ['abc', 'letters'],
    ['4.7k7', 'RKM with a decimal point too'],
    ['100c', 'centi'],
    ['10d', 'deci'],
    ['1 000', 'space as a thousands separator'],
    ['4,700', 'comma'],
    ['10 20', 'two numbers'],
    ['4.7kX', 'unknown unit after a prefix'],
    ['k', 'prefix with no number'],
    ['.', 'a lone point'],
    ['-', 'a lone sign'],
  ];
  for (const [s, why] of nonsense) {
    const r = parseEng(s, R);
    check(`reject ${why}: "${s}"`, !r.ok, r.ok ? String(r.value) : '');
    if (!r.ok) check(`  ...with a sentence`, r.err.length > 4 && !/NaN/.test(r.err), r.err);
  }
  // A prefix on a pure number is a category error, not a magnitude.
  const beta = quantityOf('Npn', 'beta');
  check('no prefixes on beta', !parseEng('100k', beta).ok);
  check('plain beta is fine', (parseEng('100', beta) as { value: number }).value === 100);
  // Never silently NaN.
  let nan = 0;
  for (const s of ['', 'x', '1x', '--5', '1..2', 'e5', '1e', '4k7k']) {
    const r = parseEng(s, V);
    if (r.ok && !Number.isFinite(r.value)) nan++;
  }
  check('never returns a non-finite value', nan === 0, String(nan));
}

section('parser agrees with the server gate');
{
  const R = quantityOf('Resistor', 'ohms');
  const C = quantityOf('Capacitor', 'farads');
  const L = quantityOf('Inductor', 'henries');
  const V = quantityOf('VoltageSource', 'dc');
  const P = quantityOf('Potentiometer', 'wiper');
  // Passives use a strictly-positive RANGE; sources use a MAGNITUDE bound.
  // Getting that asymmetry backwards either forbids a legal battery or waves
  // through a resistor the server will bounce.
  check('resistor rejects negative', !parseField('-5k', R).ok);
  check('resistor rejects zero', !parseField('0', R).ok);
  check('battery accepts negative', parseField('-5V', V).ok);
  check('battery accepts zero', parseField('0', V).ok);
  check('resistor accepts the gate floor', parseField('1u', R).ok);
  check('resistor rejects below the floor', !parseField('0.1u', R).ok);
  check('resistor accepts the gate ceiling', parseField('1T', R).ok);
  check('resistor rejects above the ceiling', !parseField('10T', R).ok);
  check('cap accepts 1 fF', parseField('1fF', C).ok);
  check('cap rejects 0.1 fF', !parseField('0.1fF', C).ok);
  check('cap rejects 10 kF', !parseField('10kF', C).ok);
  check('inductor accepts 1 pH', parseField('1pH', L).ok);
  check('source rejects 10 MV', !parseField('10MV', V).ok);
  check('wiper rejects 1.5', !parseField('1.5', P).ok);
  check('wiper accepts 0.5', parseField('0.5', P).ok);
  const seed = quantityOf('Noise', 'seed');
  check('seed rejects a fraction', !parseField('1.5', seed).ok);
  check('seed accepts an integer', parseField('12345', seed).ok);
  // The rejection sentence is the server's own, so both halves say the same
  // thing about the same rejection.
  const r = parseField('-5k', R);
  check(
    'rejection quotes the gate',
    !r.ok && /1 TΩ/.test(r.err),
    r.ok ? '' : r.err,
  );
  check('range text is readable', rangeText(R) === '1 µΩ … 1 TΩ', rangeText(R));
  check('signed range text', rangeText(V) === 'up to ±1 MV', rangeText(V));
  // The mirrored constants must not drift from validate.rs.
  check('gate constants mirrored', MIN_OHMS === 1e-6 && MAX_OHMS === 1e12);
  check('cap constants mirrored', MIN_FARADS === 1e-15 && MAX_FARADS === 1e3);
  check('ind constants mirrored', MIN_HENRIES === 1e-12 && MAX_HENRIES === 1e6);
}

section('arrow-key stepping');
{
  const R = quantityOf('Resistor', 'ohms');
  check('1k up is 2k', stepLadder(1000, 1, R) === 2000, String(stepLadder(1000, 1, R)));
  check('1k down is 500', stepLadder(1000, -1, R) === 500, String(stepLadder(1000, -1, R)));
  check('4.7k up is 5k', stepLadder(4700, 1, R) === 5000, String(stepLadder(4700, 1, R)));
  check('stepping stays in range', stepLadder(1e12, 1, R) <= 1e12, String(stepLadder(1e12, 1, R)));
  const V = quantityOf('VoltageSource', 'dc');
  // ArrowUp is numerically up, as on a native number input: toward zero on a
  // negative value, away from it on ArrowDown.
  check('-5V up is -2V', stepLadder(-5, 1, V) === -2, String(stepLadder(-5, 1, V)));
  check('-5V down is -10V', stepLadder(-5, -1, V) === -10, String(stepLadder(-5, -1, V)));
  const seed = quantityOf('Noise', 'seed');
  check('seed steps by one', stepLadder(7, 1, seed) === 8, String(stepLadder(7, 1, seed)));
  // With the Part 2 opt-in on, the arrows walk the stock ladder instead.
  check(
    'E24 stepping from 4.7k lands on 5.1k',
    stepLadder(4700, 1, R, E_SERIES.E24) === 5100,
    String(stepLadder(4700, 1, R, E_SERIES.E24)),
  );
}

// -------------------------------------------------------------- 4. e-series

section('preferred values (prototype, off by default)');
{
  // The series are geometric: every rung must be a constant ratio above the
  // last, within the rounding the printed values carry.
  for (const name of ['E6', 'E12', 'E24'] as const) {
    const m = E_SERIES[name];
    const ideal = Math.pow(10, 1 / m.length);
    let worst = 0;
    for (let i = 0; i < m.length; i++) {
      const next = i + 1 < m.length ? m[i + 1]! : m[0]! * 10;
      worst = Math.max(worst, Math.abs(next / m[i]! / ideal - 1));
    }
    check(`${name} steps are the ${m.length}th root of ten (±5%)`, worst < 0.05, worst.toFixed(4));
    check(`${name} has ${m.length} rungs`, m.length === Number(name.slice(1)));
    // Tolerance is half the log step — the property that makes the bands tile.
    const halfStep = Math.sqrt(ideal) - 1;
    check(
      `${name} tolerance ${SERIES_TOLERANCE[name]} matches its half-step ${halfStep.toFixed(3)}`,
      Math.abs(halfStep - SERIES_TOLERANCE[name]) < 0.02,
      halfStep.toFixed(4),
    );
  }
  // E24 nests E12 nests E6 nests E3 — that is what makes the ladders
  // compatible with each other.
  for (const [small, big] of [
    ['E3', 'E6'],
    ['E6', 'E12'],
    ['E12', 'E24'],
  ] as const) {
    check(`${big} contains ${small}`, E_SERIES[small].every((v) => E_SERIES[big].includes(v)));
  }
  // And 4.7 really is 10^(8/12).
  check('4.7 is 10^(8/12) rounded', Math.abs(Math.pow(10, 8 / 12) - 4.7) < 0.06, Math.pow(10, 8 / 12).toFixed(4));
  check('3.9 is 10^(7/12) rounded', Math.abs(Math.pow(10, 7 / 12) - 3.9) < 0.07, Math.pow(10, 7 / 12).toFixed(4));
}
{
  check('nearest to 3 kΩ in E12 is 3.3 k', nearestPreferred(3000, 'E12') === 3300, String(nearestPreferred(3000, 'E12')));
  check('3 kΩ is IN E24', isPreferred(3000, 'E24'));
  check('3 kΩ is NOT in E12', !isPreferred(3000, 'E12'));
  check('4.7 µF is in E12', isPreferred(4.7e-6, 'E12'));
  check('100 nF is in E12', isPreferred(1e-7, 'E12'));
  check('1 MΩ is in E12', isPreferred(1e6, 'E12'));
  check('5.1 V zener is in E24', isPreferred(5.1, 'E24'));
  check('47 kΩ is in E12', isPreferred(47000, 'E12'));
  const [lo, hi] = preferredNeighbours(3000, 'E12');
  check('3 kΩ sits between 2.7 k and 3.3 k', lo === 2700 && hi === 3300, `${lo}/${hi}`);
  check('nearest of an on-series value is itself', nearestPreferred(47000, 'E12') === 47000);
  check('step up from 4.7k in E12 is 5.6k', stepPreferred(4700, 1, 'E12') === 5600, String(stepPreferred(4700, 1, 'E12')));
  check('step down from 1k in E12 is 820', stepPreferred(1000, -1, 'E12') === 820, String(stepPreferred(1000, -1, 'E12')));
  // Nearest must never leave the ladder, at any scale.
  let off = 0;
  for (let e = -12; e <= 9; e++) {
    for (let m = 100; m < 1000; m += 7) {
      const v = (m / 100) * Math.pow(10, e);
      if (!isPreferred(nearestPreferred(v, 'E24'), 'E24')) off++;
    }
  }
  check('nearest always lands on the ladder', off === 0, String(off));
  // The explainer must actually be there, and must not lie about 4.7.
  const ex = seriesExplainer('E24');
  check('explainer exists', ex.length >= 4 && ex.join(' ').length > 400, String(ex.join(' ').length));
  check('explainer names the real root', /4\.6416/.test(ex.join(' ')));
  check('explainer avoids the 7/12 mistake', !/10\^\(7\/12\) = 4/.test(ex.join(' ')));
}
{
  // How much of the SHIPPED content is already on the ladder — the number
  // that decides whether preferred values would be a change or a ratification.
  const roomVals = [
    1, 8, 45, 60, 90, 100, 1e3, 10e3, 100e3, 470e3, 1e6, 5000, 50000, 950000, 4.7e-6, 0.47e-6,
    6.8e-6, 47e-9, 100e-9, 1e-6, 15e-9, 22e-9, 33e-9, 150e-12, 220e-12, 1.5e-9, 6.8e-9, 10e-9,
    5.1, 5.6, 1.5e-3,
  ];
  const on = roomVals.filter((v) => isPreferred(v, 'E24')).length;
  console.log(`  (info) ${on}/${roomVals.length} shipped values are already E24`);
  check('most shipped values are already standard', on / roomVals.length > 0.75, `${on}/${roomVals.length}`);
}

console.log(
  failures === 0
    ? `\nunitcheck: all ok (${checks} checks)`
    : `\nunitcheck: ${failures} FAILED of ${checks}`,
);
if (failures > 0) process.exitCode = 1;
