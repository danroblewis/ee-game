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
// What it CANNOT prove: that the result sounds good. That needs a browser and
// a human.

import { FADE_S, HP_HZ, LATENCY_S, REF_VOLTS, RING, TARGET_FS, WORKLET_SRC } from './audio-worklet';

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
    console.log(`  ok    ${name}`);
  } else {
    failures++;
    console.log(`  FAIL  ${name}${detail ? `  — ${detail}` : ''}`);
  }
}

/** A live mixer plus the helpers a test needs to drive it. */
function mixer() {
  const p = new Ctor({
    processorOptions: {
      ringSize: RING,
      target: TARGET_FS,
      refV: REF_VOLTS,
      latencySec: LATENCY_S,
      fadeSec: FADE_S,
      hpHz: HP_HZ,
    },
  });
  const send = (m: unknown) => p.port.onmessage?.({ data: m });
  const out = new Float32Array(QUANTUM);
  /** Run `blocks` quanta, returning every output sample. */
  const run = (blocks: number): Float32Array => {
    const all = new Float32Array(blocks * QUANTUM);
    for (let b = 0; b < blocks; b++) {
      out.fill(0);
      p.process([], [[out]]);
      all.set(out, b * QUANTUM);
    }
    return all;
  };
  return { p, send, run };
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

console.log(
  failures === 0
    ? '\nALL CHECKS PASSED (DSP only — audibility still needs a browser and ears)'
    : `\n${failures} CHECK(S) FAILED`,
);
// No @types/node in this package (the bench has the same constraint), so the
// exit code goes out through a narrowly typed handle instead.
(globalThis as unknown as { process: { exitCode: number } }).process.exitCode =
  failures === 0 ? 0 : 1;
