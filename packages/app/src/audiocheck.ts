// Dev-only headless check for the audio mixer. NOT shipped: nothing imports
// it, so the bundle never sees it.
//
//   pnpm --filter @ee/app audiocheck
//
// A browser is the only place you can HEAR this code, and CI has no ears, so
// this runs the exact `WORKLET_SRC` that ships against a stubbed
// AudioWorkletProcessor and asserts on the samples it produces. What it
// proves: two sources really are summed, the limiter keeps a loud pair inside
// full scale instead of clipping flat, an underrun coasts to exact silence,
// a dropped source fades to EXACT zero (not a DC sliver), mute is exact
// silence, a mid-stream time gap re-primes without poisoning the output, and
// no path anywhere produces NaN.
//
// The second half is about the CLOCK MISMATCH the rate matcher exists for:
// the samples come off the server's sim clock and leave through the sound
// card's clock. Those tests drive the mixer with a deliberately slow, fast,
// jittery or stopped producer and assert on both the audio and the telemetry
// the worklet reports — buffer depth, underruns, applied rate trim — plus the
// measured OUTPUT FREQUENCY, because rate matching is pitch shifting and an
// unbounded one would be worse than the glitch it prevents.
//
// What it CANNOT prove: that the result sounds good, or what the real device
// latency is. That needs a browser, a sound card and a human.

import {
  DEPTH_TAU_S,
  FADE_S,
  HP_HZ,
  REF_VOLTS,
  RING,
  STALL_S,
  STATS_HZ,
  TARGET_BUF_S,
  TARGET_FS,
  TRIM_DEADBAND_S,
  TRIM_KP,
  TRIM_MAX,
  TRIM_TAU_S,
  WORKLET_SRC,
} from './audio-worklet';

// ------------------------------------------------------------------- stubs
const SAMPLE_RATE = 48000;
const QUANTUM = 128;

interface Port {
  onmessage: ((ev: { data: unknown }) => void) | null;
  postMessage(m: unknown): void;
}
interface Processor {
  port: Port;
  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean;
}
type ProcessorCtor = new (options: { processorOptions: Record<string, unknown> }) => Processor;

class StubProcessor {
  readonly port: Port;
  constructor() {
    // The worklet writes `this.port.onmessage = ...`; the harness delivers
    // messages by calling it directly, exactly like the real message port
    // (minus the thread hop, which the DSP cannot observe).
    this.port = { onmessage: null, postMessage: () => {} };
  }
}

let registered: ProcessorCtor | null = null;
Object.assign(globalThis, {
  AudioWorkletProcessor: StubProcessor,
  sampleRate: SAMPLE_RATE,
  registerProcessor: (_name: string, ctor: ProcessorCtor) => {
    registered = ctor;
  },
});
// Evaluate the shipped source in this scope's globals.
new Function(WORKLET_SRC)();
if (!registered) throw new Error('worklet did not register a processor');
const Ctor: ProcessorCtor = registered;

// ------------------------------------------------------------------ harness
let failures = 0;
function check(name: string, ok: boolean, detail = '') {
  if (ok) {
    // Print the measured value on PASS too. Half the point of a DSP harness is
    // the numbers — "ok, 440.02 Hz" is a report; "ok" is a promise.
    console.log(`  ok    ${name}${detail ? `  — ${detail}` : ''}`);
  } else {
    failures++;
    console.log(`  FAIL  ${name}${detail ? `  — ${detail}` : ''}`);
  }
}

// ------------------------------------------------------------- telemetry
/** The buffer telemetry the worklet posts back, mirrored here so the harness
 * checks the wire shape as well as the values. */
interface SrcTel {
  ms: number;
  minMs: number;
  maxMs: number;
  underruns: number;
  underrunMs: number;
  drops: number;
  droppedMs: number;
  stalled: boolean;
  armed: boolean;
  priming: boolean;
  trim: number;
}
interface MixTel {
  sources: number;
  priming: number;
  ms: number;
  minMs: number;
  maxMs: number;
  underruns: number;
  underrunMs: number;
  drops: number;
  stalled: boolean;
  trim: number;
  target: number;
}
interface Tel {
  t: string;
  mix: MixTel;
  s: Record<string, SrcTel>;
}

/** Worst |trim| seen anywhere in this whole run: the ±TRIM_MAX clamp is a
 * promise about audible pitch, so it is checked globally, not per test. */
let worstTrim = 0;
/** Reports seen anywhere, so "telemetry is actually being sent" is a check. */
let reports = 0;

/** A live mixer plus the helpers a test needs to drive it. */
function mixer() {
  const p = new Ctor({
    processorOptions: {
      ringSize: RING,
      target: TARGET_FS,
      refV: REF_VOLTS,
      targetSec: TARGET_BUF_S,
      fadeSec: FADE_S,
      hpHz: HP_HZ,
      trimMax: TRIM_MAX,
      deadbandSec: TRIM_DEADBAND_S,
      kp: TRIM_KP,
      trimTauSec: TRIM_TAU_S,
      depthTauSec: DEPTH_TAU_S,
      statsHz: STATS_HZ,
      stallSec: STALL_S,
    },
  });
  const tel: Tel[] = [];
  // The real main thread reads these off the message port; here they are
  // captured so the checks can assert on them.
  p.port.postMessage = (m: unknown) => {
    const t = m as Tel;
    if (t && t.t === 'stats') {
      tel.push(t);
      reports++;
      for (const st of Object.values(t.s)) {
        const a = Math.abs(st.trim);
        if (a > worstTrim) worstTrim = a;
      }
    }
  };
  const send = (m: unknown) => p.port.onmessage?.({ data: m });
  const out = new Float32Array(QUANTUM);
  /** Run `blocks` quanta, returning every output sample. */
  const run = (blocks: number): Float32Array => {
    const all = new Float32Array(Math.max(0, blocks) * QUANTUM);
    for (let b = 0; b < blocks; b++) {
      out.fill(0);
      p.process([], [[out]]);
      all.set(out, b * QUANTUM);
    }
    return all;
  };
  /** Latest telemetry report (the tests always run long enough for one). */
  const last = (): Tel => {
    const t = tel[tel.length - 1];
    if (!t) throw new Error('no telemetry report arrived');
    return t;
  };
  /** Worst |trim| this mixer ever applied. */
  const peakTrim = () => {
    let a = 0;
    for (const r of tel) for (const st of Object.values(r.s)) a = Math.max(a, Math.abs(st.trim));
    return a;
  };
  /** Total underruns counted across every source that ever reported. */
  const underruns = () => {
    let n = 0;
    for (const [id, st] of Object.entries(last().s)) {
      void id;
      n += st.underruns;
    }
    return n;
  };
  return { p, send, run, tel, last, peakTrim, underruns };
}

/** `n` samples of a sine at `hz`, `dts` seconds apart, starting at phase t0. */
function sine(hz: number, amp: number, dts: number, n: number, t0 = 0): Float32Array {
  const s = new Float32Array(n);
  for (let k = 0; k < n; k++) s[k] = amp * Math.sin(2 * Math.PI * hz * (t0 + k * dts));
  return s;
}

const peak = (a: Float32Array | number[]) => {
  let m = 0;
  for (const v of a) m = Math.max(m, Math.abs(v));
  return m;
};
const rms = (a: Float32Array) => {
  let s = 0;
  for (const v of a) s += v * v;
  return Math.sqrt(s / Math.max(1, a.length));
};
const finite = (a: Float32Array) => a.every((v) => Number.isFinite(v));
const allZero = (a: Float32Array) => a.every((v) => v === 0);
/** Index of the first sample that is not exactly zero, or -1. Exact zero is
 * the mixer's own definition of "not playing yet" (the hush path forces it),
 * so this is the honest measure of startup latency. */
const firstSound = (a: Float32Array) => a.findIndex((v) => v !== 0);
/** Biggest sample-to-sample step: a fade-in is small, a splice is not. */
const maxStep = (a: Float32Array) => {
  let m = 0;
  for (let k = 1; k < a.length; k++) m = Math.max(m, Math.abs(a[k]! - a[k - 1]!));
  return m;
};
/** Fraction of samples pinned at full scale — a flat-topped clipper. */
const pinned = (a: Float32Array) => {
  let n = 0;
  for (const v of a) if (Math.abs(v) >= 0.999) n++;
  return n / Math.max(1, a.length);
};

/** Goertzel magnitude at `hz`, normalized by length: "how much of THIS tone
 * is in the output". The right question for a mixer — the limiter deliberately
 * re-normalizes total loudness, so rms cannot tell you whether one source was
 * muted, but its own frequency vanishing can. */
function tone(a: Float32Array, hz: number): number {
  const w = 2 * Math.cos((2 * Math.PI * hz) / SAMPLE_RATE);
  let s1 = 0;
  let s2 = 0;
  for (const v of a) {
    const s = v + w * s1 - s2;
    s2 = s1;
    s1 = s;
  }
  return Math.sqrt(s1 * s1 + s2 * s2 - w * s1 * s2) / a.length;
}

/** The server's speaker cadence: dt=20 µs x 4. */
const AUDIO_DTS = 20e-6 * 4;
/** One tick's worth of speaker samples at 30 Hz. */
const TICK_N = Math.round(1 / 30 / AUDIO_DTS);

/** Feed `blocks` quanta while topping each source up every tick, so the ring
 * never starves — what the real client does from onAudio. */
function stream(
  m: ReturnType<typeof mixer>,
  srcs: { id: string; hz: number; amp: number; gain?: number }[],
  ticks: number,
): Float32Array {
  const chunks: Float32Array[] = [];
  const blocksPerTick = Math.round(SAMPLE_RATE / 30 / QUANTUM);
  let t = 0;
  for (let k = 0; k < ticks; k++) {
    for (const s of srcs) {
      m.send({
        t: 'chunk',
        id: s.id,
        dts: AUDIO_DTS,
        gain: s.gain ?? 1,
        s: sine(s.hz, s.amp, AUDIO_DTS, TICK_N, t),
      });
    }
    t += TICK_N * AUDIO_DTS;
    chunks.push(m.run(blocksPerTick));
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const all = new Float32Array(total);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.length;
  }
  return all;
}

/** Source samples in one 30 Hz tick, exactly (not rounded like TICK_N): the
 * clock-mismatch tests measure buffer depth in milliseconds, so the harness's
 * own producer must not carry a rounding error of its own. */
const TICK_SRC = 1 / 30 / AUDIO_DTS;
/** Output frames in one 30 Hz tick — 12.5 quanta, hence the accumulator. */
const TICK_FRAMES = SAMPLE_RATE / 30;

interface Feed {
  id: string;
  hz: number;
  amp: number;
  gain?: number;
}

/**
 * Drive the mixer for `seconds` of OUTPUT time with a producer running at
 * `rate` x realtime, and return every output sample.
 *
 * This is the whole point of the exercise: `rate` < 1 is a dilated server sim
 * (a heavy circuit produces less than a second of audio per wall second),
 * `rate` > 1 is a server catching up, and `hold` drops whole ticks and
 * delivers them late (network jitter). The samples themselves are always a
 * continuous tone in SOURCE time — the tone the solver computed — so any
 * frequency error in the output is the resampler's, not the generator's.
 */
function drive(
  m: ReturnType<typeof mixer>,
  feeds: Feed[],
  seconds: number,
  rate = 1,
  hold = 0,
): Float32Array {
  const ticks = Math.max(1, Math.round(seconds * 30));
  const chunks: Float32Array[] = [];
  const sent = new Map<string, number>();
  let blocksDone = 0;
  for (let k = 1; k <= ticks; k++) {
    // `hold` ticks out of every hold+1 deliver nothing; the next tick then
    // delivers the backlog in one burst, which is what a stalled socket does.
    const deliver = hold <= 0 || k % (hold + 1) === 0 || k === ticks;
    if (deliver) {
      for (const f of feeds) {
        const have = sent.get(f.id) ?? 0;
        const want = Math.round(k * TICK_SRC * rate);
        const n = want - have;
        if (n > 0) {
          m.send({
            t: 'chunk',
            id: f.id,
            dts: AUDIO_DTS,
            gain: f.gain ?? 1,
            s: sine(f.hz, f.amp, AUDIO_DTS, n, have * AUDIO_DTS),
          });
          sent.set(f.id, want);
        }
      }
    }
    const wantBlocks = Math.floor((k * TICK_FRAMES) / QUANTUM);
    chunks.push(m.run(wantBlocks - blocksDone));
    blocksDone = wantBlocks;
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const all = new Float32Array(total);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.length;
  }
  return all;
}

/**
 * Frequency of a tone, from interpolated upward zero crossings: the interval
 * between the first and last crossing divided by the cycles between them.
 *
 * Precise to a fraction of a cent on a clean sine, and immune to the
 * limiter's amplitude glide (which moves the peaks, not the zeros) — which a
 * Goertzel bin would not be. 0 when there is no tone at all.
 */
function freq(a: Float32Array): number {
  let first = -1;
  let last = -1;
  let cycles = 0;
  for (let k = 1; k < a.length; k++) {
    const p = a[k - 1]!;
    const c = a[k]!;
    if (p < 0 && c >= 0) {
      const at = k - 1 + (c === p ? 0 : -p / (c - p));
      if (first < 0) first = at;
      else {
        last = at;
        cycles++;
      }
    }
  }
  if (cycles < 1 || last <= first) return 0;
  return (cycles * SAMPLE_RATE) / (last - first);
}

/** Pitch error in cents — the unit that says whether a human could hear it. */
const cents = (hz: number, ref: number) => (hz > 0 ? 1200 * Math.log2(hz / ref) : NaN);
/** The window worth measuring: after the target buffer has primed. */
const afterPrime = (a: Float32Array, skipS = TARGET_BUF_S + 0.06) =>
  a.subarray(Math.min(a.length, Math.round(skipS * SAMPLE_RATE)));

// -------------------------------------------------------------------- tests
console.log('EE Game audio mixer check — worklet DSP under node, no browser');
console.log(`(sampleRate ${SAMPLE_RATE}, quantum ${QUANTUM}, tap ${1 / AUDIO_DTS} Hz)\n`);

console.log('one source: a 440 Hz 5 V speaker');
{
  const m = mixer();
  const y = stream(m, [{ id: 's1', hz: 440, amp: 5 }], 12);
  const tail = y.subarray(y.length - 4 * QUANTUM);
  check('output is finite', finite(y));
  check('output is inside full scale', peak(y) <= 1);
  check('a 5 V source lands near the target level', Math.abs(peak(tail) - TARGET_FS) < 0.06,
    `peak ${peak(tail).toFixed(3)}, target ${TARGET_FS}`);
  check('it is not clipping', pinned(y) === 0);
  // 440 Hz must survive resampling: count zero crossings over a known window.
  let cross = 0;
  for (let k = 1; k < tail.length; k++) if (tail[k - 1]! < 0 !== tail[k]! < 0) cross++;
  const cycles = (tail.length / SAMPLE_RATE) * 440;
  check('440 Hz survives to the output', Math.abs(cross - 2 * cycles) <= 2,
    `${cross} crossings for ${cycles.toFixed(1)} cycles`);
}

console.log('\ntwo sources are summed, and the limiter is on the sum');
{
  const one = mixer();
  const yOne = stream(one, [{ id: 's1', hz: 220, amp: 1 }], 12);
  const two = mixer();
  // Two quiet, harmonically unrelated sources: the sum must be measurably
  // louder than either alone (i.e. they really are mixed, not switched).
  const yTwo = stream(two, [
    { id: 's1', hz: 220, amp: 1 },
    { id: 's2', hz: 313, amp: 1 },
  ], 12);
  const w = 16 * QUANTUM;
  const r1 = rms(yOne.subarray(yOne.length - w));
  const r2 = rms(yTwo.subarray(yTwo.length - w));
  check('two quiet sources are louder than one', r2 > r1 * 1.25,
    `rms ${r1.toFixed(4)} -> ${r2.toFixed(4)}`);
  // Both tones must be present in the SAME output: that is what "mixed" means.
  const tail = yTwo.subarray(yTwo.length - w);
  const a220 = tone(tail, 220);
  const a313 = tone(tail, 313);
  const floor = tone(tail, 1500); // a frequency nobody is emitting
  check('both tones are in the mix at once', a220 > floor * 8 && a313 > floor * 8,
    `220 Hz ${a220.toFixed(5)}, 313 Hz ${a313.toFixed(5)}, floor ${floor.toFixed(5)}`);
  check('summed output is finite', finite(yTwo));

  const loud = mixer();
  const yLoud = stream(loud, [
    { id: 's1', hz: 220, amp: 120 },
    { id: 's2', hz: 313, amp: 120 },
  ], 20);
  check('two LOUD sources stay inside full scale', peak(yLoud) <= 1,
    `peak ${peak(yLoud).toFixed(4)}`);
  const settled = yLoud.subarray(yLoud.length - 8 * QUANTUM);
  check('two loud sources do not clip flat', pinned(settled) === 0,
    `${(pinned(settled) * 100).toFixed(1)}% of samples pinned`);
  check('loud output is finite', finite(yLoud));
}

console.log('\nunderrun: the stream stops mid-flight');
{
  const m = mixer();
  stream(m, [{ id: 's1', hz: 440, amp: 5 }], 10);
  // Nothing more is fed. It must coast, fade and then sit at exact silence.
  const y = m.run(200);
  check('underrun output is finite', finite(y));
  const late = y.subarray(y.length - 8 * QUANTUM);
  check('underrun settles to EXACT silence', allZero(late), `peak ${peak(late)}`);
  // ...and it must recover when samples come back.
  const y2 = stream(m, [{ id: 's1', hz: 440, amp: 5 }], 12);
  check('it recovers after an underrun', peak(y2.subarray(y2.length - 4 * QUANTUM)) > 0.05);
}

console.log('\na time gap re-primes one source without touching the other');
{
  const m = mixer();
  stream(m, [
    { id: 's1', hz: 440, amp: 5 },
    { id: 's2', hz: 313, amp: 5 },
  ], 8);
  m.send({ t: 'reset', id: 's1' }); // what the client sends on a t0 jump
  const y = stream(m, [
    { id: 's1', hz: 440, amp: 5 },
    { id: 's2', hz: 313, amp: 5 },
  ], 8);
  check('a re-prime produces no NaN', finite(y));
  check('the surviving source keeps playing', peak(y.subarray(y.length - 4 * QUANTUM)) > 0.05);
  check('a re-prime does not clip', peak(y) <= 1);
}

console.log('\nremoving a source');
{
  const m = mixer();
  stream(m, [
    { id: 's1', hz: 440, amp: 5 },
    { id: 's2', hz: 313, amp: 5 },
  ], 10);
  m.send({ t: 'drop', id: 's2' }); // speaker deleted
  const y = stream(m, [{ id: 's1', hz: 440, amp: 5 }], 10);
  check('after a drop the mix is finite', finite(y));
  check('the remaining source still plays', peak(y.subarray(y.length - 4 * QUANTUM)) > 0.05);

  // Drop the last one too: EXACT zero, forever, no NaN, no DC sliver.
  m.send({ t: 'drop', id: 's1' });
  const fade = m.run(20);
  check('a drop fades rather than cutting', !allZero(fade.subarray(0, 8)));
  const after = m.run(200);
  check('with every source gone the output is EXACTLY zero', allZero(after), `peak ${peak(after)}`);
  check('and still finite', finite(after));
  // A source added back after everything was dropped must work.
  const again = stream(m, [{ id: 's3', hz: 440, amp: 5 }], 12);
  check('a new source after silence plays', peak(again.subarray(again.length - 4 * QUANTUM)) > 0.05);
}

console.log('\nmute, volume and per-source gain');
{
  const m = mixer();
  stream(m, [{ id: 's1', hz: 440, amp: 5 }], 10);
  m.send({ t: 'master', gain: 1, mute: true });
  const muted = m.run(120);
  check('global mute reaches exact silence', allZero(muted.subarray(muted.length - 8 * QUANTUM)),
    `peak ${peak(muted.subarray(muted.length - 8 * QUANTUM))}`);
  check('muting produces no NaN', finite(muted));

  m.send({ t: 'master', gain: 1, mute: false });
  const back = stream(m, [{ id: 's1', hz: 440, amp: 5 }], 12);
  check('unmute restores sound', peak(back.subarray(back.length - 4 * QUANTUM)) > 0.05);

  // Half volume must be quieter, not silent. The limiter tracks the SUM
  // before the master, so the ratio is the master ratio.
  const full = rms(back.subarray(back.length - 8 * QUANTUM));
  m.send({ t: 'master', gain: 0.5, mute: false });
  const half = stream(m, [{ id: 's1', hz: 440, amp: 5 }], 12);
  const halfR = rms(half.subarray(half.length - 8 * QUANTUM));
  check('half volume is about half as loud', Math.abs(halfR / full - 0.5) < 0.08,
    `ratio ${(halfR / full).toFixed(3)}`);

  // Per-speaker mute / solo: gain 0 on one of two sources. The right test is
  // spectral — the muted speaker's TONE must go, and the other must stay. The
  // limiter deliberately re-normalizes what is left, so overall loudness does
  // NOT drop (and asserting that it does would be asserting a bug).
  const s = mixer();
  const w = 16 * QUANTUM;
  const both = stream(s, [
    { id: 's1', hz: 220, amp: 5 },
    { id: 's2', hz: 313, amp: 5 },
  ], 14).subarray(-w);
  s.send({ t: 'gain', id: 's2', gain: 0 });
  const solo = stream(s, [
    { id: 's1', hz: 220, amp: 5 },
    { id: 's2', hz: 313, amp: 5, gain: 0 },
  ], 14).subarray(-w);
  check('both tones present before the mute', tone(both, 220) > 0.002 && tone(both, 313) > 0.002,
    `220 ${tone(both, 220).toFixed(5)}, 313 ${tone(both, 313).toFixed(5)}`);
  check('the muted speaker\'s tone is gone', tone(solo, 313) < tone(both, 313) / 20,
    `313 Hz ${tone(both, 313).toFixed(5)} -> ${tone(solo, 313).toFixed(5)}`);
  check('the other speaker is still audible', tone(solo, 220) > tone(both, 220) / 2,
    `220 Hz ${tone(both, 220).toFixed(5)} -> ${tone(solo, 220).toFixed(5)}`);
  check('per-source mute produces no NaN', finite(solo));
  check('per-source mute does not clip', peak(solo) <= 1);
}

console.log('\na silent circuit is silent');
{
  const m = mixer();
  // A speaker across a dead node: real samples, all zero.
  const y = stream(m, [{ id: 's1', hz: 440, amp: 0 }], 12);
  check('zero samples in, exact zero out', allZero(y), `peak ${peak(y)}`);
}

// ===========================================================================
// Clock mismatch: the producer is the server's sim clock, the consumer is the
// sound card. Everything below drives them out of step on purpose.
// ===========================================================================

const TARGET_MS = TARGET_BUF_S * 1000;

console.log('\nstartup: prime to the TARGET before the first sample plays');
{
  // The old behaviour was to play as soon as anything arrived, which starts
  // one chunk deep and underruns on the first hiccup. Priming to the target
  // trades TARGET_BUF_S of startup silence for a buffer that can absorb one.
  const m = mixer();
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 1);
  const at = firstSound(y) / SAMPLE_RATE;
  check('it does play eventually', at >= 0, `first sound at ${(at * 1000).toFixed(0)} ms`);
  check(
    'nothing plays until roughly the target is buffered',
    at >= TARGET_BUF_S * 0.75,
    `first sound at ${(at * 1000).toFixed(1)} ms, target ${TARGET_MS.toFixed(0)} ms`,
  );
  check(
    'and it does not wait much longer than that',
    at <= TARGET_BUF_S + 0.06,
    `first sound at ${(at * 1000).toFixed(1)} ms`,
  );
  check(
    'the start is a fade-in, not a step',
    maxStep(y.subarray(0, Math.round((at + 0.001) * SAMPLE_RATE))) < 0.02,
    `largest step ${maxStep(y.subarray(0, Math.round((at + 0.001) * SAMPLE_RATE))).toFixed(5)}`,
  );
  // Telemetry has to distinguish "filling up" from "about to glitch", or the
  // dock would flash a warning every time a speaker is placed.
  check(
    'a filling source reports PRIMING, not an underrun',
    m.tel.some((t) => t.mix.priming === 1) &&
      m.tel.every((t) => t.mix.priming === 0 || t.mix.underruns === 0),
    `${m.tel.filter((t) => t.mix.priming === 1).length} priming reports`,
  );
  check(
    'priming clears once it is playing',
    m.last().mix.priming === 0 && m.last().s['s1']?.priming === false,
  );
  check('startup produces no NaN and no clipping', finite(y) && peak(y) <= 1);
}

console.log('\nbuffer telemetry: a healthy stream, exactly realtime');
{
  const m = mixer();
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 1.5);
  const st = m.last();
  check('telemetry is reported at all', m.tel.length > 0, `${m.tel.length} reports`);
  check(
    'reports arrive at ~10 Hz, not per quantum',
    Math.abs(m.tel.length / 1.5 - STATS_HZ) < 2,
    `${(m.tel.length / 1.5).toFixed(1)} Hz`,
  );
  check(
    'the mix reports one source at the target depth',
    st.mix.sources === 1 && Math.abs(st.mix.ms - TARGET_MS) <= 45,
    `${st.mix.ms.toFixed(1)} ms buffered, target ${TARGET_MS} ms`,
  );
  check(
    'the 1 s min/max bracket the current depth plausibly',
    st.mix.minMs <= st.mix.ms + 1 &&
      st.mix.maxMs >= st.mix.ms - 1 &&
      st.mix.maxMs - st.mix.minMs < 80,
    `${st.mix.minMs.toFixed(1)}–${st.mix.maxMs.toFixed(1)} ms`,
  );
  check('a healthy stream never underruns', st.mix.underruns === 0 && st.mix.underrunMs === 0);
  check('a healthy stream never overflows', st.mix.drops === 0);
  check('per-source telemetry exists and is armed', st.s['s1']?.armed === true);
  check(
    'a healthy stream needs NO rate trim (inside the deadband)',
    m.peakTrim() < 1e-6,
    `peak |trim| ${(m.peakTrim() * 100).toFixed(4)} %`,
  );
  const f = freq(afterPrime(y));
  check(
    '440 Hz comes out at 440 Hz, to within a few cents',
    Math.abs(cents(f, 440)) < 6,
    `${f.toFixed(3)} Hz (${cents(f, 440).toFixed(2)} cents)`,
  );
  check('healthy output is finite', finite(y));
}

console.log('\nrate matching: a producer running 10 % SLOW for 2 s');
{
  const m = mixer();
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 2, 0.9);
  const st = m.last();
  check('no NaN under rate matching', finite(y));
  check(
    'ZERO underruns: the buffer absorbed it',
    st.mix.underruns === 0 && st.mix.underrunMs === 0,
    `${st.mix.underruns} underruns, ${st.mix.underrunMs} ms lost`,
  );
  check(
    'the buffer was defended, not abandoned',
    st.mix.ms > 5,
    `${st.mix.ms.toFixed(1)} ms left after 2 s of 10 % deficit`,
  );
  check(
    'it did drop below the deadband (the loop really engaged)',
    st.mix.ms < TARGET_MS - TRIM_DEADBAND_S * 1000,
    `${st.mix.ms.toFixed(1)} ms`,
  );
  check(
    'the trim pulled slow, and stayed inside the clamp',
    m.peakTrim() > 0.005 && m.peakTrim() <= TRIM_MAX + 1e-9,
    `peak |trim| ${(m.peakTrim() * 100).toFixed(3)} % (clamp ${TRIM_MAX * 100} %)`,
  );
  const tail = y.subarray(y.length - 8 * QUANTUM);
  check('it is still playing at the end', peak(tail) > 0.05, `peak ${peak(tail).toFixed(3)}`);
  check('it never clipped', peak(y) <= 1);
  // Pitch: rate matching IS pitch shifting, so this is the number that says
  // whether the cure is worse than the disease. Slowing playback by the
  // clamp's 3 % is 52 cents; a resampler that simply followed a 10 % slow
  // producer would be 176 cents flat, i.e. a whole tone out of tune.
  const all = freq(afterPrime(y));
  const early = freq(afterPrime(y).subarray(0, Math.round(0.75 * SAMPLE_RATE)));
  check(
    '440 Hz is held inside the ±3 % clamp (never dragged to 0.9x)',
    Math.abs(cents(all, 440)) < 1200 * Math.log2(1 + TRIM_MAX) + 1,
    `${all.toFixed(2)} Hz over the whole run (${cents(all, 440).toFixed(1)} cents)`,
  );
  check(
    'and while the buffer is still healthy it is within a few cents',
    Math.abs(cents(early, 440)) < 12,
    `${early.toFixed(2)} Hz early on (${cents(early, 440).toFixed(1)} cents)`,
  );
}

console.log('\nrate matching: a producer running 10 % FAST for 2 s');
{
  const m = mixer();
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 2, 1.1);
  const st = m.last();
  check('no NaN when the producer runs ahead', finite(y));
  check('no overflow: nothing was thrown away', st.mix.drops === 0, `${st.mix.drops} drops`);
  check('no underruns either', st.mix.underruns === 0);
  check(
    'the surplus is being drained, not hoarded',
    m.peakTrim() > 0.005 && m.peakTrim() <= TRIM_MAX + 1e-9,
    `peak |trim| ${(m.peakTrim() * 100).toFixed(3)} %`,
  );
  check(
    'the trim ran FAST (positive) to drain it',
    (st.s['s1']?.trim ?? 0) > 0,
    `trim ${(((st.s['s1']?.trim ?? 0) * 100)).toFixed(3)} %`,
  );
  check(
    'depth grew but stayed far inside the ring',
    st.mix.ms > TARGET_MS && st.mix.ms < RING * AUDIO_DTS * 1000 * 0.8,
    `${st.mix.ms.toFixed(1)} ms of ${(RING * AUDIO_DTS * 1000).toFixed(0)} ms ring`,
  );
  check('still playing, still unclipped', peak(y.subarray(-8 * QUANTUM)) > 0.05 && peak(y) <= 1);
}

console.log('\nnetwork jitter: whole ticks arrive late, in bursts');
{
  const m = mixer();
  // Every third tick delivers nothing and the next one delivers the backlog:
  // 66 ms of lumpiness on a 200 ms buffer, the case rate matching exists for.
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 2, 1, 2);
  const st = m.last();
  check('jitter produces no NaN', finite(y));
  check(
    'jitter is absorbed with ZERO underruns',
    st.mix.underruns === 0,
    `${st.mix.underruns} underruns, depth ${st.mix.ms.toFixed(1)} ms`,
  );
  check(
    'and with (almost) no pitch correction — the deadband ate it',
    m.peakTrim() < 0.005,
    `peak |trim| ${(m.peakTrim() * 100).toFixed(4)} %`,
  );
  const f = freq(afterPrime(y));
  check(
    'a jittery 440 Hz stays within a few cents',
    Math.abs(cents(f, 440)) < 6,
    `${f.toFixed(3)} Hz (${cents(f, 440).toFixed(2)} cents)`,
  );
}

console.log('\nslow drift: 0.2 % forever, which is the NORMAL case');
{
  // Nothing exotic: the server advances 1664 substeps (33.28 ms) per 33.33 ms
  // tick, and a sound card's crystal is tens of ppm off nominal anyway. A
  // fixed resample ratio walks the buffer to one end and glitches; the trim
  // has to find a steady state and sit there. This is the test that says
  // "audio still works after twenty minutes", scaled down to 20 s.
  const m = mixer();
  const y = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 20, 0.998);
  const st = m.last();
  const trim = st.s['s1']?.trim ?? 0;
  check(
    'a permanent 0.2 % deficit never underruns',
    st.mix.underruns === 0 && st.mix.drops === 0,
    `${st.mix.underruns} underruns, ${st.mix.drops} drops over 20 s`,
  );
  check(
    'the trim settles at about the deficit, not at the clamp',
    trim < -0.001 && trim > -0.006,
    `trim ${(trim * 100).toFixed(3)} % against a 0.200 % deficit`,
  );
  // P-only: steady state sits kp-proportionally BELOW the target rather than
  // on it (0.2 % / 0.3 = 6.7 ms plus the 45 ms deadband ≈ 148 ms). That is a
  // deliberate choice — an integrator would wind up during a stall and then
  // slam the rate when the producer came back.
  check(
    'and holds a stable depth below the target (P-only, by design)',
    st.mix.ms > 100 && st.mix.ms < TARGET_MS && st.mix.maxMs - st.mix.minMs < 80,
    `${st.mix.ms.toFixed(1)} ms, 1 s range ${st.mix.minMs.toFixed(0)}–${st.mix.maxMs.toFixed(0)} ms`,
  );
  const f = freq(afterPrime(y, 2));
  check(
    'a 0.2 % drift is inaudible: still 440 Hz to a few cents',
    Math.abs(cents(f, 440)) < 8,
    `${f.toFixed(3)} Hz (${cents(f, 440).toFixed(2)} cents)`,
  );
  check('20 s of drift correction produces no NaN', finite(y) && peak(y) <= 1);
}

console.log('\nthe producer stops entirely, then comes back');
{
  const m = mixer();
  drive(m, [{ id: 's1', hz: 440, amp: 5 }], 1);
  // Nothing arrives for 1 s: fade, hold silence, and say so in telemetry.
  const gap = m.run(Math.round(SAMPLE_RATE / QUANTUM));
  const stalled = m.last();
  check('the gap is finite', finite(gap));
  check(
    'a stopped producer fades to EXACT zero',
    allZero(gap.subarray(gap.length - 8 * QUANTUM)),
    `peak ${peak(gap.subarray(gap.length - 8 * QUANTUM))}`,
  );
  check('the underrun is counted', stalled.mix.underruns >= 1, `${stalled.mix.underruns}`);
  check(
    'a long stop is reported as STALLED, not as endless underrun ms',
    stalled.mix.stalled && stalled.mix.underrunMs <= STALL_S * 1000 + 5,
    `stalled=${stalled.mix.stalled}, ${stalled.mix.underrunMs.toFixed(1)} ms counted`,
  );
  check('a stalled source reports an empty buffer', stalled.mix.ms < 1, `${stalled.mix.ms} ms`);
  // Data returns: it must re-prime to the target and play cleanly again.
  const back = drive(m, [{ id: 's1', hz: 440, amp: 5 }], 1.2);
  const st = m.last();
  check('it re-primes and plays again', peak(back.subarray(-8 * QUANTUM)) > 0.05);
  check('re-priming produces no NaN', finite(back));
  check('the stall flag clears', !st.mix.stalled);
  check(
    'it re-primes to the TARGET depth, not to a sliver',
    Math.abs(st.mix.ms - TARGET_MS) <= 45,
    `${st.mix.ms.toFixed(1)} ms`,
  );
  check(
    'the re-primed tone is on pitch',
    Math.abs(cents(freq(afterPrime(back)), 440)) < 6,
    `${freq(afterPrime(back)).toFixed(3)} Hz`,
  );
  check('no clipping across the whole stop/restart', peak(gap) <= 1 && peak(back) <= 1);
}

console.log('\ncatastrophe: a producer at half speed, and one that floods');
{
  // 50 % slow for 3 s cannot be rate-matched by anyone — the clamp must hold
  // and the output must degrade into silence-and-re-prime, never into NaN,
  // never into a pitch-shifted-by-half buzz.
  const slow = mixer();
  const y = drive(slow, [{ id: 's1', hz: 440, amp: 5 }], 3, 0.5);
  check('a 2x-too-slow producer produces no NaN', finite(y));
  check('...and no clipping', peak(y) <= 1);
  check(
    '...and still respects the ±3 % clamp',
    slow.peakTrim() <= TRIM_MAX + 1e-9,
    `peak |trim| ${(slow.peakTrim() * 100).toFixed(3)} %`,
  );
  check(
    '...and reports the damage instead of hiding it',
    slow.last().mix.underruns >= 1,
    `${slow.last().mix.underruns} underruns, ${slow.last().mix.underrunMs.toFixed(0)} ms lost`,
  );

  // A flood: 3x realtime for 6 s overruns the ring. No trim can drain a
  // permanent 3x surplus (that would need +200 %), so oldest audio has to go
  // — and it has to be COUNTED rather than silently absorbed.
  const fast = mixer();
  const yf = drive(fast, [{ id: 's1', hz: 440, amp: 5 }], 6, 3);
  const st = fast.last();
  const ringMs = RING * AUDIO_DTS * 1000;
  const deepest = Math.max(...fast.tel.map((t) => t.mix.maxMs));
  check('a flooding producer produces no NaN', finite(yf));
  check(
    'a ring overflow drops oldest audio and counts it',
    st.mix.drops >= 1,
    `${st.mix.drops} drops, ${st.s['s1']?.droppedMs?.toFixed(0) ?? '?'} ms discarded`,
  );
  // The depth cannot be asserted to sit AT the target here: the drop resets it
  // to the target and the flood immediately refills it, so where in that cycle
  // the last report landed is arbitrary. What must hold is that it is bounded
  // by the ring (no runaway) and that audio never stops.
  check(
    'depth stays bounded by the ring — no runaway',
    deepest <= ringMs + 1 && st.mix.ms <= ringMs + 1,
    `deepest ${deepest.toFixed(0)} ms of a ${ringMs.toFixed(0)} ms ring, now ${st.mix.ms.toFixed(0)} ms`,
  );
  check(
    'it never stopped playing through the flood',
    peak(yf.subarray(-8 * QUANTUM)) > 0.05 && st.mix.underruns === 0,
    `peak ${peak(yf.subarray(-8 * QUANTUM)).toFixed(3)}, ${st.mix.underruns} underruns`,
  );
  check('...and inside the clamp', fast.peakTrim() <= TRIM_MAX + 1e-9);

  // The same event, deterministically: one chunk BIGGER than the whole ring.
  // The policy is "keep the newest target's worth and count the rest", and
  // this is the only way to observe it without racing the 10 Hz reports.
  const burst = mixer();
  drive(burst, [{ id: 's1', hz: 440, amp: 5 }], 0.6); // primed and playing
  const beforeMs = burst.last().mix.ms;
  const burstN = RING + 5000;
  burst.send({
    t: 'chunk',
    id: 's1',
    dts: AUDIO_DTS,
    gain: 1,
    s: sine(440, 5, AUDIO_DTS, burstN),
  });
  const yb = drive(burst, [{ id: 's1', hz: 440, amp: 5 }], 0.4);
  const bst = burst.last();
  const src = bst.s['s1'];
  // Everything that was queued plus the burst, minus the target's worth kept.
  const expectMs = beforeMs + burstN * AUDIO_DTS * 1000 - TARGET_MS;
  check(
    'an over-ring burst is ONE drop event, not a storm',
    src?.drops === 1,
    `${src?.drops ?? '?'} drops for a ${(burstN * AUDIO_DTS * 1000).toFixed(0)} ms chunk`,
  );
  // The tolerance is one chunk plus one report period: `beforeMs` is the depth
  // at the last 10 Hz report before the burst, and the depth sawtooths by a
  // 33 ms chunk in between. Loose enough not to be flaky, tight enough to
  // catch an accounting error (getting this wrong means ms lost is a fiction).
  check(
    'it discards exactly the surplus and reports the ms lost',
    Math.abs((src?.droppedMs ?? 0) - expectMs) < 60,
    `${(src?.droppedMs ?? 0).toFixed(0)} ms discarded, expected ~${expectMs.toFixed(0)} ms`,
  );
  check(
    'and it is left holding the TARGET, not a full ring of latency',
    Math.abs(bst.mix.ms - TARGET_MS) <= 45,
    `${bst.mix.ms.toFixed(1)} ms after the burst (target ${TARGET_MS} ms)`,
  );
  check(
    'the burst neither underran nor clipped nor went NaN',
    bst.mix.underruns === 0 && finite(yb) && peak(yb) <= 1 && peak(yb.subarray(-8 * QUANTUM)) > 0.05,
    `${bst.mix.underruns} underruns, peak ${peak(yb).toFixed(3)}`,
  );
}

console.log('\nthe rate clamp, over every scenario above');
check(
  `|trim| never exceeded the ±${TRIM_MAX * 100} % clamp`,
  worstTrim <= TRIM_MAX + 1e-9,
  `worst |trim| ${(worstTrim * 100).toFixed(4)} % = ${Math.abs(
    1200 * Math.log2(1 + worstTrim),
  ).toFixed(1)} cents at any frequency`,
);
check('telemetry was flowing throughout', reports > 100, `${reports} reports`);

console.log(
  failures === 0
    ? '\nALL CHECKS PASSED (DSP only — audibility still needs a browser and ears)'
    : `\n${failures} CHECK(S) FAILED`,
);
// No @types/node in this package (the bench has the same constraint), so the
// exit code goes out through a narrowly typed handle instead.
(globalThis as unknown as { process: { exitCode: number } }).process.exitCode =
  failures === 0 ? 0 : 1;
