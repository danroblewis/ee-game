// Dev-only: what does a circuit of a given amplitude actually PLAY AT?
//
//   pnpm --filter @ee/app gaincheck
//
// Runs the shipped WORKLET_SRC against a stub, feeds it a sine of a known
// peak voltage, and reports the output level in dBFS. This is the number a
// player experiences, and until now nothing measured it: `audiocheck` asserts
// that a 5 V source lands near TARGET_FS, but real rooms are nowhere near
// 5 V — the measured peaks are 0.20 V (the-ladder) to 0.28 V (TR-808), and
// the old law pinned everything below REF_VOLTS to one fixed tiny gain.
//
// It also prints the compression law so a change to it is visible as a curve
// rather than as one assertion passing.

import {
  AGC_MAX_GAIN, AGC_SLOPE, DEPTH_TAU_S, FADE_S, HP_HZ, RATE_MIN, RATE_TAU_S, REF_VOLTS, RING, STALL_S,
  STATS_HZ, TARGET_BUF_S, TARGET_FS, TRIM_DEADBAND_S, TRIM_KP, TRIM_MAX, TRIM_TAU_S,
  WORKLET_SRC,
} from './audio-worklet';

const QUANTUM = 128;
const SR = 48000;
const AUDIO_DTS = 1 / 12500; // the server's audio tap rate
const TICK_SRC = Math.round(12500 / 30);
const TICK_FRAMES = SR / 30;

/* eslint-disable @typescript-eslint/no-explicit-any */
class Stub {
  port: any = { postMessage: () => {}, onmessage: null };
  constructor(public options: any) {}
}
(globalThis as any).AudioWorkletProcessor = Stub;
(globalThis as any).sampleRate = SR;
(globalThis as any).currentTime = 0;
let Ctor: any;
(globalThis as any).registerProcessor = (_n: string, c: any) => (Ctor = c);
// eslint-disable-next-line @typescript-eslint/no-implied-eval
new Function(WORKLET_SRC)();

function mixer() {
  const p = new Ctor({
    processorOptions: {
      ringSize: RING, target: TARGET_FS, refV: REF_VOLTS, targetSec: TARGET_BUF_S,
      slope: AGC_SLOPE, maxGain: AGC_MAX_GAIN,
      fadeSec: FADE_S, hpHz: HP_HZ, trimMax: TRIM_MAX, deadbandSec: TRIM_DEADBAND_S,
      kp: TRIM_KP, trimTauSec: TRIM_TAU_S, depthTauSec: DEPTH_TAU_S, statsHz: STATS_HZ,
      stallSec: STALL_S, rateTauSec: RATE_TAU_S, rateMin: RATE_MIN,
    },
  });
  const out = new Float32Array(QUANTUM);
  return {
    send: (m: unknown) => p.port.onmessage?.({ data: m }),
    run: (blocks: number) => {
      const all = new Float32Array(Math.max(0, blocks) * QUANTUM);
      for (let b = 0; b < blocks; b++) {
        out.fill(0);
        p.process([], [[out]]);
        all.set(out, b * QUANTUM);
      }
      return all;
    },
  };
}

/** Steady-state output for a sine of `amp` volts peak, after the AGC settles. */
function play(amp: number, hz = 220, seconds = 12): Float32Array {
  const m = mixer();
  const chunks: Float32Array[] = [];
  let sent = 0;
  let done = 0;
  const ticks = Math.round(seconds * 30);
  for (let k = 1; k <= ticks; k++) {
    const want = k * TICK_SRC;
    const n = want - sent;
    const s = new Float32Array(n);
    for (let i = 0; i < n; i++) s[i] = amp * Math.sin(2 * Math.PI * hz * (sent + i) * AUDIO_DTS);
    m.send({ t: 'chunk', id: 'a', dts: AUDIO_DTS, gain: 1, s });
    sent = want;
    const wb = Math.floor((k * TICK_FRAMES) / QUANTUM);
    chunks.push(m.run(wb - done));
    done = wb;
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const all = new Float32Array(total);
  let at = 0;
  for (const c of chunks) { all.set(c, at); at += c.length; }
  // Last 25%: the recovery glide is ~25 ms but the peak tracker decays slowly.
  return all.subarray(Math.floor(all.length * 0.75));
}

/** EVERY sample, including the onset. Clipping happens on the way up, not in
 *  the steady state — checking only the settled tail let a mutation that
 *  removed the limiter's per-sample cap pass unnoticed. */
function playAll(amp: number, hz = 220, seconds = 4): Float32Array {
  const m = mixer();
  const chunks: Float32Array[] = [];
  let sent = 0;
  let done = 0;
  for (let k = 1; k <= Math.round(seconds * 30); k++) {
    const want = k * TICK_SRC;
    const n = want - sent;
    const s = new Float32Array(n);
    for (let i = 0; i < n; i++) s[i] = amp * Math.sin(2 * Math.PI * hz * (sent + i) * AUDIO_DTS);
    m.send({ t: 'chunk', id: 'a', dts: AUDIO_DTS, gain: 1, s });
    sent = want;
    const wb = Math.floor((k * TICK_FRAMES) / QUANTUM);
    chunks.push(m.run(wb - done));
    done = wb;
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const all = new Float32Array(total);
  let at = 0;
  for (const c of chunks) { all.set(c, at); at += c.length; }
  return all;
}

const peak = (a: Float32Array) => a.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
const rms = (a: Float32Array) => Math.sqrt(a.reduce((s, v) => s + v * v, 0) / a.length);
const dB = (x: number) => (x > 0 ? 20 * Math.log10(x) : -Infinity);
const pinned = (a: Float32Array) => a.reduce((n, v) => n + (Math.abs(v) >= 0.999 ? 1 : 0), 0);

// The measured peaks of the shipped rooms, plus the range around them.
const CASES: Array<[number, string]> = [
  [0.063, 'the-ladder rms'],
  [0.2, 'the-ladder / vco-555 peak'],
  [0.28, 'TR-808 peak'],
  [1.0, 'a 1 V circuit'],
  [5.0, 'REF_VOLTS'],
  [9.0, 'bass++ peak (op-amp rail)'],
  [100.0, 'a 100 V rail — must not clip'],
];

console.log('input peak   what                          output peak   dBFS     clipped');
let worstQuiet = -Infinity;
for (const [amp, what] of CASES) {
  const y = play(amp);
  const p = peak(y);
  console.log(
    `${amp.toFixed(3).padStart(9)} V   ${what.padEnd(28)}  ${p.toFixed(4).padStart(9)}   ` +
      `${dB(p).toFixed(1).padStart(6)}   ${pinned(y) ? 'CLIPPED ' + pinned(y) : 'no'}`,
  );
  if (amp <= 0.3) worstQuiet = Math.max(worstQuiet, dB(p));
  void rms;
}
console.log(
  `\nQuietest real room plays at ${worstQuiet.toFixed(1)} dBFS. ` +
    `For reference a normalised YouTube track sits near -14 LUFS with peaks at 0 dBFS.`,
);

// ------------------------------------------------------------- assertions
//
// Printing is not guarding. These are the three properties the auto-gain
// exists to hold, and each one FAILED before it was written.
let bad = 0;
const must = (name: string, ok: boolean, detail = '') => {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
  if (!ok) bad++;
};

// 1. A REAL ROOM IS AUDIBLE. The measured peaks of the shipped synth rooms
//    are 0.20-0.28 V. Under the old law those played at -42 and -39 dBFS,
//    which is what "I can barely hear it" meant. -20 dBFS is the line: still
//    far below a loud room, but unambiguously present.
for (const amp of [0.2, 0.28]) {
  const d = dB(peak(play(amp)));
  must(`a ${amp} V room is audible`, d > -20, `${d.toFixed(1)} dBFS`);
}

// 2. NOTHING CLIPS, at any input, including a rail far outside the design
//    range. The limiter must hold this on its own.
for (const amp of [0.2, 5, 9, 100, 1000]) {
  const y = playAll(amp);
  must(`${amp} V does not clip`, pinned(y) === 0 && peak(y) <= 1, `peak ${peak(y).toFixed(4)}`);
}

// 3. LOUDNESS IS STILL INFORMATION. Compression is not normalisation: a
//    louder circuit must still be audibly louder, or the player loses the
//    only cue that tells them their amplifier is working. Full
//    normalisation would make these equal and fail here.
const STEPS = [0.05, 0.2, 1, 5] as const;
const levels = STEPS.map((a) => dB(peak(play(a))));
for (let i = 1; i < STEPS.length; i++) {
  const lo = levels[i - 1] as number;
  const hi = levels[i] as number;
  must(
    `${STEPS[i]} V is louder than ${STEPS[i - 1]} V`,
    hi > lo + 1.5,
    `${lo.toFixed(1)} -> ${hi.toFixed(1)} dBFS`,
  );
}

console.log(bad ? `\n${bad} CHECK(S) FAILED` : '\nALL CHECKS PASSED');
(globalThis as unknown as { process: { exitCode: number } }).process.exitCode = bad ? 1 : 0;
