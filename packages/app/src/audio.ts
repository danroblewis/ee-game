// Real-time audio for "listen" probes (press 3 over an element).
//
// Nothing in here synthesizes sound. The samples are the solver's own
// waveform — the exact chunks the oscilloscope draws — handed to an
// AudioWorklet that keeps a ring buffer and linearly upsamples from the sim
// sample rate to the AudioContext rate. If you hear a tone, the circuit is
// oscillating; if you hear nothing, it isn't (design pillar: every number a
// player perceives comes from the solver).
//
// Honest limits of v1: online, probe chunks arrive at dt=20 µs × 16 = 320 µs
// per sample, i.e. a 3.125 kHz source rate, so only content below ~1.5 kHz
// is real — anything faster was already aliased by the server's decimation
// and folds down into the audible band. Offline we feed one sample per
// animation frame (~60 Hz), which is far coarser still: expect a rumble that
// tracks the envelope, not the tone.

/** Peak volts that maps to `TARGET_FS` of full scale. */
const REF_VOLTS = 5;
/** Full-scale target for a REF_VOLTS-peak signal. */
const TARGET_FS = 0.2;
/** Buffer depth the worklet builds before it starts playing, in seconds. */
const LATENCY_S = 0.06;
/** Click-free fade in/out at start, stop and underrun. */
const FADE_S = 0.006;
/** DC blocker corner: a cone cannot follow DC, and neither do we. */
const HP_HZ = 20;
/** Ring capacity in source samples (~5 s at 3.125 kHz). */
const RING = 1 << 14;
/** Offline: animation-frame samples batched per message. */
const OFFLINE_BATCH = 4;

/** The worklet, inlined so no extra file has to be served. */
const WORKLET_SRC = `
class SimAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = (options && options.processorOptions) || {};
    this.size = o.ringSize || 16384;
    this.ring = new Float32Array(this.size);
    this.target = o.target || 0.2;
    this.refV = o.refV || 5;
    this.latency = o.latencySec || 0.06;
    this.fadeInc = 1 / Math.max(1, (o.fadeSec || 0.006) * sampleRate);
    this.hpCoef = 1 - (2 * Math.PI * (o.hpHz || 20)) / sampleRate;
    this.w = 0;      // absolute write count
    this.r = 0;      // absolute read cursor (fractional)
    this.step = 0;   // source samples consumed per output frame
    this.prime = 0;  // source samples buffered before playback starts
    this.armed = false;
    this.fade = 0;
    this.gain = 0;
    this.peak = 0;
    this.last = 0;
    this.hpX = 0;
    this.hpY = 0;
    this.port.onmessage = (ev) => {
      const m = ev.data;
      if (m.t === 'chunk') {
        this.setRate(m.dts);
        this.write(m.s);
      } else if (m.t === 'reset') {
        this.w = 0;
        this.r = 0;
        this.step = 0;
        this.armed = false;
        this.peak = 0;
        this.gain = 0;
        this.last = 0;
        this.hpX = 0;
        this.hpY = 0;
      }
    };
  }

  setRate(dts) {
    if (!(dts > 0)) return;
    const step = 1 / (dts * sampleRate);
    if (Math.abs(step - this.step) < 1e-12) return;
    this.step = step;
    this.prime = Math.max(4, Math.min(this.size >> 1, Math.round(this.latency / dts)));
  }

  write(s) {
    for (let k = 0; k < s.length; k++) {
      this.ring[this.w % this.size] = s[k];
      this.w++;
    }
    // Drift control: if the reader falls too far behind (slow start, tab
    // throttled, a burst of catch-up chunks) skip forward rather than let
    // the writer lap it. The fade keeps the jump from clicking.
    if (this.w - this.r > Math.min(this.size - 2, this.prime * 4)) {
      this.r = this.w - this.prime;
    }
  }

  process(_inputs, outputs) {
    const ch = outputs[0];
    const out = ch && ch[0];
    if (!out) return true;
    for (let k = 0; k < out.length; k++) {
      let s = this.last;
      if (this.armed) {
        if (this.w - this.r > 1) {
          const i = Math.floor(this.r);
          const f = this.r - i;
          const a = this.ring[i % this.size];
          const b = this.ring[(i + 1) % this.size];
          s = a + (b - a) * f;
          this.r += this.step;
          this.last = s;
          this.fade = Math.min(1, this.fade + this.fadeInc);
        } else {
          this.armed = false; // underrun: coast, fade to silence
          this.fade = Math.max(0, this.fade - this.fadeInc);
        }
      } else {
        this.fade = Math.max(0, this.fade - this.fadeInc);
        if (this.fade <= 0 && this.step > 0 && this.w - this.r >= this.prime) {
          this.r = this.w - this.prime;
          this.armed = true;
        }
      }
      // DC block: a biased node (speaker across a rail) must not eat the
      // headroom or thump when you start listening.
      const y = s - this.hpX + this.hpCoef * this.hpY;
      this.hpX = s;
      this.hpY = y;
      // Soft normalization: REF_VOLTS peak -> TARGET_FS. Louder signals are
      // pulled down; quieter ones stay quiet (loudness is information).
      const mag = y < 0 ? -y : y;
      this.peak = mag > this.peak ? mag : this.peak * 0.99995 + mag * 0.00005;
      const want = this.target / Math.max(this.refV, this.peak);
      // Limiter glide: duck fast (~2 ms) so a sudden 100 V rail does not sit
      // on the clipper, recover slowly (~25 ms) so it does not pump.
      this.gain += (want - this.gain) * (want < this.gain ? 0.01 : 0.0008);
      const v = y * this.gain * this.fade;
      out[k] = v > 1 ? 1 : v < -1 ? -1 : v;
    }
    for (let c = 1; c < ch.length; c++) ch[c].set(out);
    return true;
  }
}
registerProcessor('sim-audio', SimAudioProcessor);
`;

/**
 * Plays ONE probe's waveform. `listen(pid)` selects the source (switching is
 * a re-prime, not a crossfade), `stop()` silences it. Feed it with
 * `pushChunk` online or `pushPoint` offline; both are no-ops for any pid
 * that is not the current source, so callers can pipe every probe in.
 */
export class AudioPlayer {
  private ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private booting: Promise<void> | null = null;
  private dead = false;
  private src: number | null = null;
  /** Expected sample-clock time of the next chunk (gap detection). */
  private nextT: number | null = null;
  /** Offline batching buffer: [t, v, t, v, ...]. */
  private pts: number[] = [];
  private lvl = 0;
  private lvlAt = 0;

  constructor() {
    // Autoplay policy: a context created before any user gesture starts
    // suspended. '3' is itself a gesture, but if the browser suspends us
    // later (or the first attempt raced the policy) any click or key wakes
    // it back up.
    const kick = () => {
      if (this.src !== null && this.ctx && this.ctx.state === 'suspended') void this.ctx.resume();
    };
    window.addEventListener('pointerdown', kick);
    window.addEventListener('keydown', kick);
  }

  /** pid of the probe being listened to, or null when silent. */
  get pid(): number | null {
    return this.src;
  }

  /** 0..1 loudness of the live stream (REF_VOLTS peak = 1), for the glyph. */
  get level(): number {
    if (this.src === null) return 0;
    return (performance.now() - this.lvlAt) / 1000 > 0.5 ? 0 : this.lvl;
  }

  listen(pid: number) {
    if (this.src === pid) return;
    this.src = pid;
    this.nextT = null;
    this.pts.length = 0;
    this.node?.port.postMessage({ t: 'reset' });
    void this.boot();
  }

  stop() {
    this.src = null;
    this.nextT = null;
    this.pts.length = 0;
    this.lvl = 0;
    this.node?.port.postMessage({ t: 'reset' });
    void this.ctx?.suspend();
  }

  /** Server waveform chunk: `samples[k]` is the value at `t0 + k * dts`. */
  pushChunk(pid: number, t0: number, dts: number, samples: ArrayLike<number>) {
    if (pid !== this.src || samples.length === 0 || !(dts > 0)) return;
    // A jump in the sample clock means continuity is gone (lagged socket,
    // room restart): re-prime instead of splicing a discontinuity in.
    if (this.nextT !== null && Math.abs(t0 - this.nextT) > dts * 4) {
      this.node?.port.postMessage({ t: 'reset' });
    }
    this.nextT = t0 + samples.length * dts;
    const buf = new Float32Array(samples.length);
    for (let k = 0; k < samples.length; k++) buf[k] = samples[k] ?? 0;
    this.send(dts, buf);
  }

  /** Offline path: one sample per animation frame, batched. */
  pushPoint(pid: number, t: number, v: number) {
    if (pid !== this.src) return;
    const pts = this.pts;
    if (pts.length && t <= pts[pts.length - 2]!) pts.length = 0; // sim restarted
    pts.push(t, v);
    if (pts.length < OFFLINE_BATCH * 2) return;
    const n = pts.length / 2;
    const dts = (pts[pts.length - 2]! - pts[0]!) / (n - 1);
    const buf = new Float32Array(n);
    for (let k = 0; k < n; k++) buf[k] = pts[k * 2 + 1]!;
    pts.length = 0;
    if (dts > 0) this.send(dts, buf);
  }

  private send(dts: number, buf: Float32Array) {
    let peak = 0;
    for (const v of buf) {
      const a = v < 0 ? -v : v;
      if (a > peak) peak = a;
    }
    const l = Math.min(1, peak / REF_VOLTS);
    this.lvl = l > this.lvl ? l : this.lvl * 0.7 + l * 0.3;
    this.lvlAt = performance.now();
    // Dropped while the worklet is still loading: a few frames of silence.
    this.node?.port.postMessage({ t: 'chunk', dts, s: buf }, [buf.buffer]);
  }

  private boot(): Promise<void> {
    if (this.dead) return Promise.resolve();
    if (this.booting) {
      if (this.ctx?.state === 'suspended') void this.ctx.resume();
      return this.booting;
    }
    this.booting = (async () => {
      const ctx = new AudioContext({ latencyHint: 'interactive' });
      const url = URL.createObjectURL(new Blob([WORKLET_SRC], { type: 'text/javascript' }));
      try {
        await ctx.audioWorklet.addModule(url);
      } finally {
        URL.revokeObjectURL(url);
      }
      const node = new AudioWorkletNode(ctx, 'sim-audio', {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        processorOptions: {
          ringSize: RING,
          target: TARGET_FS,
          refV: REF_VOLTS,
          latencySec: LATENCY_S,
          fadeSec: FADE_S,
          hpHz: HP_HZ,
        },
      });
      node.connect(ctx.destination);
      this.ctx = ctx;
      this.node = node;
      // Stopped while the module was still loading: idle instead of running.
      if (this.src === null) await ctx.suspend();
      else await ctx.resume();
    })().catch((err: unknown) => {
      // No AudioWorklet (or the page refuses to make noise): stay silent
      // forever rather than retrying on every keypress.
      console.warn('listen: audio unavailable', err);
      this.dead = true;
      this.node = null;
      this.src = null;
    });
    return this.booting;
  }
}
