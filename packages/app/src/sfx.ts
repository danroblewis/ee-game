// UI sound effects — currently one: the noise a part makes when it dies.
//
// READ THIS BEFORE ADDING ANYTHING HERE.
//
// Everything in `audio.ts` / `audio-worklet.ts` is the SOLVER TALKING: a
// Speaker element's terminal voltage, or a listen probe's samples, streamed
// through per-source ring buffers and summed by the worklet. If you hear it,
// the circuit did it. This file is the opposite kind of sound, and the two
// must never be confusable:
//
//   • Nothing here is ever pushed into a worklet source. These voices are
//     ordinary WebAudio nodes on a SEPARATE bus that meets the solver stream
//     only at `ctx.destination`. A synthesized pop in a ring buffer would be
//     indistinguishable from a measured waveform at exactly the layer where
//     that distinction is the whole design pillar.
//   • They therefore never enter the worklet's shared peak-follower/limiter,
//     so a bang cannot duck the real speaker audio, and never appear in
//     `status().buf`, so the dock's honesty readout keeps describing only
//     the electrical stream.
//
// What IS honest about it: the EVENT is real. The server's damage model
// decided the part broke, from solver quantities (`crates/damage`), and the
// client hears about it in the `damage` snapshot. This is feedback for a
// real event — the sound is a label on it, like the toast and the scorch
// mark, not a measurement of it. Nothing here claims to be a waveform the
// circuit produced.
//
// Sounds are SYNTHESIZED, not sampled: no binary assets in the bundle, no
// extra fetch on a break, and a recipe is a dozen numbers you can read and
// tune in the diff. The whole palette below costs ~2 kB of source.

import type { ElementKind } from './circuit';

/** Kinds a break sound can be chosen for: every kind in the client document,
 * plus server-only fixtures (the hoist motor) that never appear in a doc op
 * but can still be broken by the damage model. */
export type BreakableKind = ElementKind['t'] | 'Motor';

/** A short broadband transient: the crack, rip or bang itself. Filtered
 * noise, swept from `hz` to `hz2` over its life. */
interface Crack {
  gain: number;
  dur: number;
  /** Seconds to peak. Tiny = a click; longer = a rip. */
  attack?: number;
  type: BiquadFilterType;
  hz: number;
  hz2?: number;
  q?: number;
}

/** The body resonance under the crack: a thud, a ping, a spin-down growl.
 * One oscillator gliding `hz` → `hz2`. */
interface Body {
  gain: number;
  dur: number;
  type: OscillatorType;
  hz: number;
  hz2?: number;
}

/** The aftermath: smoke hiss, electrolyte fizz, an arc still sizzling. */
interface Tail {
  gain: number;
  dur: number;
  type: BiquadFilterType;
  hz: number;
  hz2?: number;
  q?: number;
}

/** Fragments — n short blips scattered over `spread` seconds. Glass. */
interface Shards {
  n: number;
  gain: number;
  hz: number;
  spread: number;
  dur: number;
}

/** One break sound. Every layer is optional; a voice is whichever layers it
 * declares, summed. */
export interface BreakVoice {
  /** Only for the headless check and debugging — never shown to a player. */
  readonly label: string;
  readonly crack?: Crack;
  readonly body?: Body;
  readonly tail?: Tail;
  readonly shards?: Shards;
}

// --------------------------------------------------------------- the palette
//
// Each voice is a guess at what that failure sounds like on a real bench.
// They are meant to be told apart with your eyes shut: a filament lamp
// tinkles, an electrolytic thumps and fizzes, silicon ticks, carbon film
// cracks and then hisses smoke.

/** Fallback for any kind without a bespoke voice: a dry, mid snap. */
export const DEFAULT_VOICE: BreakVoice = {
  label: 'generic',
  crack: { gain: 0.9, dur: 0.1, type: 'bandpass', hz: 1800, hz2: 400, q: 0.8 },
  body: { gain: 0.5, dur: 0.16, type: 'triangle', hz: 190, hz2: 70 },
  tail: { gain: 0.12, dur: 0.5, type: 'lowpass', hz: 1400, hz2: 500 },
};

/** Glass envelope lets go, filament pings as it parts. */
const LAMP: BreakVoice = {
  label: 'lamp',
  crack: { gain: 0.8, dur: 0.07, type: 'highpass', hz: 1800, hz2: 3000 },
  body: { gain: 0.35, dur: 0.09, type: 'sine', hz: 1400, hz2: 420 },
  shards: { n: 7, gain: 0.22, hz: 3200, spread: 0.28, dur: 0.05 },
  tail: { gain: 0.06, dur: 0.35, type: 'lowpass', hz: 2200, hz2: 900 },
};

/** A 3 mm plastic dome: a tick, and nothing else. Small parts die small. */
const LED: BreakVoice = {
  label: 'led',
  crack: { gain: 0.55, dur: 0.035, type: 'bandpass', hz: 3400, hz2: 2200, q: 1.2 },
  body: { gain: 0.18, dur: 0.05, type: 'square', hz: 900, hz2: 500 },
};

/** Carbon film cracks its body open, then smokes. */
const RESISTOR: BreakVoice = {
  label: 'resistor',
  crack: { gain: 0.85, dur: 0.09, type: 'bandpass', hz: 1300, hz2: 380, q: 0.7 },
  body: { gain: 0.45, dur: 0.18, type: 'triangle', hz: 165, hz2: 62 },
  tail: { gain: 0.2, dur: 0.9, type: 'lowpass', hz: 900, hz2: 260 },
};

/** An electrolytic venting: the deep POP, then wet spray. The loudest thing
 * on a small bench, and everyone who has done it remembers the smell. */
const CAPACITOR: BreakVoice = {
  label: 'capacitor',
  crack: { gain: 1, dur: 0.13, type: 'lowpass', hz: 900, hz2: 180 },
  body: { gain: 0.8, dur: 0.3, type: 'sine', hz: 95, hz2: 38 },
  tail: { gain: 0.3, dur: 1.3, type: 'bandpass', hz: 2600, hz2: 900, q: 0.5 },
};

/** Windings cook: no crack, just a muffled thump and a dark smoulder. */
const INDUCTOR: BreakVoice = {
  label: 'inductor',
  crack: { gain: 0.5, dur: 0.12, attack: 0.006, type: 'lowpass', hz: 500, hz2: 150 },
  body: { gain: 0.6, dur: 0.35, type: 'sine', hz: 120, hz2: 55 },
  tail: { gain: 0.12, dur: 0.7, type: 'lowpass', hz: 600, hz2: 200 },
};

/** Silicon: fast, dry, high and over before you look up. */
const SEMI: BreakVoice = {
  label: 'semiconductor',
  crack: { gain: 0.75, dur: 0.045, type: 'highpass', hz: 2200, hz2: 4200 },
  body: { gain: 0.3, dur: 0.06, type: 'square', hz: 1200, hz2: 620 },
  tail: { gain: 0.1, dur: 0.22, type: 'bandpass', hz: 4000, hz2: 2600, q: 0.8 },
};

/** A plastic DIP: same die, more package to crack and more to sizzle. */
const IC: BreakVoice = {
  label: 'ic',
  crack: { gain: 0.8, dur: 0.06, type: 'bandpass', hz: 2600, hz2: 1100, q: 0.9 },
  body: { gain: 0.28, dur: 0.09, type: 'square', hz: 780, hz2: 380 },
  tail: { gain: 0.16, dur: 0.45, type: 'bandpass', hz: 3400, hz2: 1500, q: 0.7 },
};

/** A cone tearing: a rasp, not a crack. */
const SPEAKER: BreakVoice = {
  label: 'speaker',
  crack: { gain: 0.7, dur: 0.22, attack: 0.01, type: 'bandpass', hz: 700, hz2: 240, q: 2.5 },
  body: { gain: 0.5, dur: 0.28, type: 'sawtooth', hz: 130, hz2: 48 },
  tail: { gain: 0.1, dur: 0.4, type: 'lowpass', hz: 1200, hz2: 500 },
};

/** Something with mass in it stops: a clunk and a spin-down. */
const MOTOR: BreakVoice = {
  label: 'motor',
  crack: { gain: 0.7, dur: 0.1, type: 'lowpass', hz: 1200, hz2: 300 },
  body: { gain: 0.7, dur: 0.6, type: 'sawtooth', hz: 160, hz2: 34 },
  tail: { gain: 0.18, dur: 0.8, type: 'bandpass', hz: 500, hz2: 180, q: 1.5 },
};

/** The supply itself letting go — the one you feel in the bench. */
const SUPPLY: BreakVoice = {
  label: 'supply',
  crack: { gain: 1, dur: 0.18, type: 'lowpass', hz: 2400, hz2: 300 },
  body: { gain: 0.9, dur: 0.45, type: 'sine', hz: 70, hz2: 28 },
  tail: { gain: 0.25, dur: 1.1, type: 'lowpass', hz: 1100, hz2: 300 },
};

/**
 * THE SEAM. To give a part type its own break sound, add ONE line here
 * mapping the kind tag to a voice — either an existing one:
 *
 *     Potentiometer: RESISTOR,
 *
 * or a new `const` above. Kinds absent from this map get `DEFAULT_VOICE`,
 * which is why the map is deliberately incomplete: Potentiometer, Switch,
 * Button and CurrentSource are all on the generic snap today, and so is
 * anything the client cannot name (an id whose doc op has not landed yet).
 * The key type is checked, so a typo'd kind is a compile error rather than a
 * silently generic part.
 *
 * Wire, Ground, OpAmp and Ota are absent on purpose and will never be here:
 * `rating()` in crates/damage returns None for them, so they cannot break.
 */
export const VOICES: { readonly [K in BreakableKind]?: BreakVoice } = {
  Lamp: LAMP,
  Led: LED,
  Resistor: RESISTOR,
  Capacitor: CAPACITOR,
  Inductor: INDUCTOR,
  Diode: SEMI,
  Zener: SEMI,
  Npn: SEMI,
  Pnp: SEMI,
  Nmos: SEMI,
  Pmos: SEMI,
  Timer555: IC,
  Speaker: SPEAKER,
  Motor: MOTOR,
  VoltageSource: SUPPLY,
  Rail: SUPPLY,
};

/** The voice for a kind tag. Unknown or absent ⇒ the generic snap. */
export const voiceFor = (kind?: string): BreakVoice =>
  (kind ? VOICES[kind as BreakableKind] : undefined) ?? DEFAULT_VOICE;

// ------------------------------------------------------------------- limits

/** Concurrent break voices. A shorted rail can kill a dozen parts in ONE
 * 30 Hz snapshot; past this they are dropped, because the fifth simultaneous
 * bang carries no information and a wall of noise is worse than a bang. */
export const MAX_VOICES = 4;
/** Gain applied to the 1st…Nth voice of a burst. Four at once should read as
 * "several things just died", not as four times the level. */
const BURST_GAIN = [1, 0.72, 0.55, 0.45];
/** Each extra voice in a burst is nudged later by this much, so a bus fault
 * is a ragged crackle rather than one phase-coherent slam. */
const BURST_STAGGER_S = 0.022;
/** Longest tail in the palette, plus slack — how long after a break the
 * context still has work to do. */
export const TAIL_MS = 1600;
/** Headroom for the whole effect bus, before master volume. */
const BUS_GAIN = 0.55;
/** Seconds of shared white noise. Every crack/tail/shard reads a random
 * window of this one buffer, so a break allocates no sample data. */
const NOISE_S = 1.5;

const clampGain = (g: number) => (g > 0 ? (g < 1 ? g : 1) : 0);

/** Deterministic 0..1 from an integer — cosmetic variation only (a row of
 * identical resistors should not machine-gun), never a measurement. */
function hash01(n: number): number {
  let x = (n | 0) * 0x27d4eb2d;
  x = (x ^ (x >>> 15)) >>> 0;
  return x / 4294967296;
}

/**
 * A parallel bus of one-shot UI voices, sharing only the AudioContext with
 * the solver stream. Owns a gain node (the caller keeps it in step with the
 * player's master volume/mute) and a compressor that is the effects' OWN
 * limiter — the worklet's limiter guards the measured signal and must never
 * see any of this.
 */
export class SfxBus {
  private readonly ctx: BaseAudioContext;
  private readonly bus: GainNode;
  private noise: AudioBuffer | null = null;
  /** Context times at which each live voice goes quiet. */
  private ends: number[] = [];

  constructor(ctx: BaseAudioContext, out: AudioNode) {
    this.ctx = ctx;
    this.bus = ctx.createGain();
    this.bus.gain.value = 0;
    // The effects' own limiter. Isolated on purpose: a break can never duck
    // the speaker taps, and the taps can never duck a break.
    const comp = ctx.createDynamicsCompressor();
    comp.threshold.value = -8;
    comp.knee.value = 6;
    comp.ratio.value = 12;
    comp.attack.value = 0.002;
    comp.release.value = 0.12;
    this.bus.connect(comp);
    comp.connect(out);
  }

  /** Master level for effects, 0..1 (0 = muted). */
  setGain(v: number) {
    const g = clampGain(v) * BUS_GAIN;
    const t = this.ctx.currentTime;
    this.bus.gain.cancelScheduledValues(t);
    this.bus.gain.setValueAtTime(g, t);
  }

  /** Live voices right now (drops finished ones). For the check harness. */
  voices(): number {
    const now = this.ctx.currentTime;
    let n = 0;
    for (const e of this.ends) if (e > now) this.ends[n++] = e;
    this.ends.length = n;
    return n;
  }

  /**
   * Play the break sound for `kind` (see `voiceFor`). `seed` is any stable
   * integer — the element id — used only to detune this instance a few
   * percent so repeats sound like different parts. Returns false when the
   * voice cap dropped it.
   */
  playBreak(kind?: string, seed = 0): boolean {
    const n = this.voices();
    if (n >= MAX_VOICES) return false;
    const v = voiceFor(kind);
    const t0 = this.ctx.currentTime + n * BURST_STAGGER_S + 0.005;
    const g = BURST_GAIN[n] ?? BURST_GAIN[BURST_GAIN.length - 1]!;
    // ±7 % on pitch, ±10 % on length: cosmetic, deterministic per element.
    const r = hash01(seed);
    const pitch = 0.93 + 0.14 * r;
    const len = 0.9 + 0.2 * hash01(seed ^ 0x5bf0)!;

    let end = t0;
    if (v.crack) {
      const d = v.crack.dur * len;
      this.noiseLayer(t0, d, v.crack.gain * g, v.crack, v.crack.attack ?? 0.001, pitch);
      end = Math.max(end, t0 + d);
    }
    if (v.body) {
      const d = v.body.dur * len;
      this.toneLayer(t0, d, v.body.gain * g, v.body, pitch);
      end = Math.max(end, t0 + d);
    }
    if (v.shards) {
      const s = v.shards;
      for (let k = 0; k < s.n; k++) {
        const j = hash01(seed * 31 + k * 977);
        const at = t0 + 0.01 + s.spread * j * j;
        this.toneLayer(
          at,
          s.dur,
          s.gain * g * (0.5 + 0.5 * j),
          { type: 'triangle', hz: s.hz * (0.6 + 1.1 * j), hz2: s.hz * 0.5, dur: s.dur, gain: 1 },
          pitch,
        );
        end = Math.max(end, at + s.dur);
      }
    }
    if (v.tail) {
      const d = v.tail.dur * len;
      // The aftermath swells in rather than cracking: it is smoke, not glass.
      this.noiseLayer(t0 + 0.01, d, v.tail.gain * g, v.tail, Math.min(0.08, d * 0.3), pitch);
      end = Math.max(end, t0 + 0.01 + d);
    }
    this.ends.push(end);
    return true;
  }

  // ----------------------------------------------------------------- layers

  private sharedNoise(): AudioBuffer {
    if (this.noise) return this.noise;
    const n = Math.max(1, Math.floor(this.ctx.sampleRate * NOISE_S));
    const buf = this.ctx.createBuffer(1, n, this.ctx.sampleRate);
    const d = buf.getChannelData(0);
    // Deterministic LCG: the same bus always makes the same noise, so a
    // recording of a session is reproducible.
    let s = 0x2545f491;
    for (let i = 0; i < n; i++) {
      s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
      d[i] = s / 2147483648 - 1;
    }
    this.noise = buf;
    return buf;
  }

  private noiseLayer(
    t0: number,
    dur: number,
    gain: number,
    f: { type: BiquadFilterType; hz: number; hz2?: number; q?: number },
    attack: number,
    pitch: number,
  ) {
    const ctx = this.ctx;
    const src = ctx.createBufferSource();
    const buf = this.sharedNoise();
    src.buffer = buf;
    const bq = ctx.createBiquadFilter();
    bq.type = f.type;
    bq.Q.value = f.q ?? 1;
    this.sweep(bq.frequency, f.hz * pitch, (f.hz2 ?? f.hz) * pitch, t0, dur);
    const amp = ctx.createGain();
    this.envelope(amp.gain, t0, dur, gain, attack);
    src.connect(bq);
    bq.connect(amp);
    amp.connect(this.bus);
    // A random window of the shared buffer: two cracks never phase-align.
    const off = (t0 * 997) % Math.max(0.001, NOISE_S - dur - 0.05);
    src.start(t0, Math.max(0, off));
    src.stop(t0 + dur + 0.02);
  }

  private toneLayer(t0: number, dur: number, gain: number, b: Body, pitch: number) {
    const ctx = this.ctx;
    const osc = ctx.createOscillator();
    osc.type = b.type;
    this.sweep(osc.frequency, b.hz * pitch, (b.hz2 ?? b.hz) * pitch, t0, dur);
    const amp = ctx.createGain();
    this.envelope(amp.gain, t0, dur, gain, Math.min(0.004, dur * 0.2));
    osc.connect(amp);
    amp.connect(this.bus);
    osc.start(t0);
    osc.stop(t0 + dur + 0.02);
  }

  /** Percussive envelope: fast linear attack, exponential decay, then an
   * explicit ramp to true zero so nothing leaves a DC sliver on the bus. */
  private envelope(p: AudioParam, t0: number, dur: number, peak: number, attack: number) {
    const g = clampGain(peak);
    const a = Math.max(0.0005, Math.min(attack, dur * 0.5));
    p.setValueAtTime(0, t0);
    p.linearRampToValueAtTime(g, t0 + a);
    p.exponentialRampToValueAtTime(Math.max(1e-4, g * 0.001), t0 + dur);
    p.linearRampToValueAtTime(0, t0 + dur + 0.01);
  }

  /** Exponential glide, clamped away from 0 Hz (exponential ramps cannot
   * touch zero) and away from Nyquist. */
  private sweep(p: AudioParam, a: number, b: number, t0: number, dur: number) {
    const lo = 20;
    const hi = Math.max(lo + 1, this.ctx.sampleRate * 0.45);
    const f0 = Math.min(hi, Math.max(lo, a));
    const f1 = Math.min(hi, Math.max(lo, b));
    p.setValueAtTime(f0, t0);
    if (f1 !== f0) p.exponentialRampToValueAtTime(f1, t0 + dur);
  }
}
