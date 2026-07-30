// The AudioWorklet mixer, as a source string.
//
// It lives in its own module for two reasons: audio.ts stays about policy
// (which sources exist, what the player muted) instead of DSP, and
// `audiocheck.ts` can run this exact code under node against a stubbed
// AudioWorkletProcessor. What ships and what is tested are the same string.
//
// Nothing in here synthesizes anything. Every sample came out of the MNA
// solver — a speaker's terminal voltage or a probe's node waveform — and the
// only processing is resampling to the device rate, summing, DC blocking and
// limiting. If you hear nothing, the circuit is doing nothing.
//
// Structure: N independent sources, each with its own ring buffer, read
// cursor, resample step and fade envelope; summed; then ONE shared output
// stage (DC block -> peak follower -> limiter glide -> master gain -> clamp).
// The limiter has to be on the SUM, not per source, or two loud speakers
// would each pass at full scale and clip into distortion when added.

/** Peak volts that maps to `TARGET_FS` of full scale. */
export const REF_VOLTS = 5;
/** Full-scale target for a REF_VOLTS-peak signal. */
export const TARGET_FS = 0.2;
/** Buffer depth the worklet builds before it starts playing, in seconds. */
export const LATENCY_S = 0.06;
/** Click-free fade in/out at start, stop, removal and underrun. */
export const FADE_S = 0.006;
/** DC blocker corner: a cone cannot follow DC, and neither do we. */
export const HP_HZ = 20;
/** Ring capacity per source, in source samples (~1.3 s at 12.5 kHz). */
export const RING = 1 << 14;

export const WORKLET_SRC = `
/** One streamed source: its own ring, cursor, rate and fade. */
class Src {
  constructor(size, latency, fadeInc, gain) {
    this.ring = new Float32Array(size);
    this.size = size;
    this.latency = latency;
    this.fadeInc = fadeInc;
    this.gain = gain;      // per-source target gain (mute = 0)
    this.g = gain;         // ...and the glided value actually applied
    this.w = 0;            // absolute write count
    this.r = 0;            // absolute read cursor (fractional)
    this.step = 0;         // source samples consumed per output frame
    this.prime = 0;        // source samples buffered before playback starts
    this.armed = false;
    this.fade = 0;
    this.last = 0;
    this.doomed = false;   // removed: fade out, then drop
  }

  setRate(dts) {
    if (!(dts > 0)) return;
    const step = 1 / (dts * sampleRate);
    if (Math.abs(step - this.step) < 1e-12) return;
    this.step = step;
    this.prime = Math.max(4, Math.min(this.size >> 1, Math.round(this.latency / dts)));
  }

  reset() {
    this.w = 0;
    this.r = 0;
    this.step = 0;
    this.armed = false;
    this.last = 0;
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

  /** Next sample, faded and gain-glided. Underrun coasts to silence instead
   * of clicking, and a mute ramps over FADE_S instead of stepping. */
  next() {
    if (this.g < this.gain) this.g = Math.min(this.gain, this.g + this.fadeInc);
    else if (this.g > this.gain) this.g = Math.max(this.gain, this.g - this.fadeInc);
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
        if (!this.doomed) this.fade = Math.min(1, this.fade + this.fadeInc);
        else this.fade = Math.max(0, this.fade - this.fadeInc);
      } else {
        this.armed = false; // underrun: coast, fade to silence
        this.fade = Math.max(0, this.fade - this.fadeInc);
      }
    } else {
      this.fade = Math.max(0, this.fade - this.fadeInc);
      if (!this.doomed && this.fade <= 0 && this.step > 0 && this.w - this.r >= this.prime) {
        this.r = this.w - this.prime;
        this.armed = true;
      }
    }
    // Exact zero once faded out or muted: a removed source must not leave a
    // DC sliver sitting in the mix forever.
    const a = this.fade * this.g;
    if (a <= 0) return 0;
    return s * a;
  }

  /** True once a removed source has finished fading and can be dropped. */
  get gone() {
    return this.doomed && this.fade <= 0;
  }
}

class SimAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = (options && options.processorOptions) || {};
    this.size = o.ringSize || 16384;
    this.target = o.target || 0.2;
    this.refV = o.refV || 5;
    this.latency = o.latencySec || 0.06;
    this.fadeInc = 1 / Math.max(1, (o.fadeSec || 0.006) * sampleRate);
    this.hpCoef = 1 - (2 * Math.PI * (o.hpHz || 20)) / sampleRate;
    // Consecutive all-zero-sum samples before the output stage is hard-reset.
    // Without this the DC blocker rings down exponentially and "silence" is a
    // denormal tail that never quite reaches zero.
    this.hushMax = Math.max(8, Math.round((o.fadeSec || 0.006) * sampleRate));
    this.hush = 0;
    this.srcs = new Map();
    this.live = [];       // per-block snapshot of srcs, reused (no allocation)
    this.master = 1;      // 0..1 volume
    this.mute = false;
    this.masterFade = 1;  // click-free mute/unmute
    this.peak = 0;
    this.gain = 0;
    this.hpX = 0;
    this.hpY = 0;
    this.port.onmessage = (ev) => {
      const m = ev.data;
      if (m.t === 'chunk') {
        const s = this.src(m.id, m.gain);
        s.doomed = false;
        if (m.gain !== undefined) s.gain = m.gain;
        s.setRate(m.dts);
        s.write(m.s);
      } else if (m.t === 'gain') {
        // Immediate response to a mute/solo click; chunks carry gain too, so
        // this only saves the ~33 ms until the next one arrives.
        const s = this.srcs.get(m.id);
        if (s) s.gain = m.gain;
      } else if (m.t === 'drop') {
        // Fade, then drop — removing the ring outright would click.
        const s = this.srcs.get(m.id);
        if (s) s.doomed = true;
      } else if (m.t === 'master') {
        this.master = m.gain;
        this.mute = !!m.mute;
      } else if (m.t === 'reset') {
        if (m.id === undefined) {
          this.srcs.clear();
          this.peak = 0;
          this.gain = 0;
          this.hpX = 0;
          this.hpY = 0;
        } else {
          const s = this.srcs.get(m.id);
          if (s) s.reset();
        }
      }
    };
  }

  src(id, gain) {
    let s = this.srcs.get(id);
    if (!s) {
      s = new Src(this.size, this.latency, this.fadeInc, gain === undefined ? 1 : gain);
      this.srcs.set(id, s);
    }
    return s;
  }

  process(_inputs, outputs) {
    const ch = outputs[0];
    const out = ch && ch[0];
    if (!out) return true;
    const wantMaster = this.mute ? 0 : this.master;
    // No sources at all: EXACT silence, and the output stage forgets its
    // history so the next source starts from a clean filter instead of
    // riding out an 8 ms DC-blocker tail.
    if (this.srcs.size === 0) {
      out.fill(0);
      this.hpX = 0;
      this.hpY = 0;
      this.peak = 0;
      this.hush = this.hushMax;
      this.masterFade = wantMaster;
      for (let c = 1; c < ch.length; c++) ch[c].set(out);
      return true;
    }
    const live = this.live;
    live.length = 0;
    for (const s of this.srcs.values()) live.push(s);
    for (let k = 0; k < out.length; k++) {
      let sum = 0;
      for (let j = 0; j < live.length; j++) sum += live[j].next();
      // Every source silent (starved, muted, faded out) for a whole fade:
      // stop ringing the DC blocker down and be EXACTLY quiet.
      if (sum === 0) {
        if (this.hush < this.hushMax) this.hush++;
        if (this.hush >= this.hushMax) {
          this.hpX = 0;
          this.hpY = 0;
          this.peak = 0;
          out[k] = 0;
          continue;
        }
      } else {
        this.hush = 0;
      }
      // DC block: a biased node (a speaker across a rail) must not eat the
      // headroom or thump when a source joins.
      const y = sum - this.hpX + this.hpCoef * this.hpY;
      this.hpX = sum;
      this.hpY = y;
      // Soft normalization on the SUM: REF_VOLTS peak -> TARGET_FS. Louder
      // signals are pulled down; quieter ones stay quiet (loudness is
      // information). Two loud speakers duck together instead of clipping.
      const mag = y < 0 ? -y : y;
      this.peak = mag > this.peak ? mag : this.peak * 0.99995 + mag * 0.00005;
      const want = this.target / Math.max(this.refV, this.peak);
      // Limiter glide: duck fast (~2 ms) so a sudden 100 V rail does not sit
      // on the clipper, recover slowly (~25 ms) so it does not pump.
      this.gain += (want - this.gain) * (want < this.gain ? 0.01 : 0.0008);
      // Mute/volume glides too, so toggling it is silent rather than a click.
      this.masterFade += (wantMaster - this.masterFade) * 0.02;
      if (Math.abs(wantMaster - this.masterFade) < 1e-4) this.masterFade = wantMaster;
      const v = y * this.gain * this.masterFade;
      out[k] = v > 1 ? 1 : v < -1 ? -1 : v;
    }
    // Reap finished removals after the block, never mid-loop.
    for (const [id, s] of this.srcs) if (s.gone) this.srcs.delete(id);
    for (let c = 1; c < ch.length; c++) ch[c].set(out);
    return true;
  }
}
registerProcessor('sim-audio', SimAudioProcessor);
`;
