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
// cursor, rate-matched resample step, fade envelope and buffer telemetry;
// summed; then ONE shared output stage (DC block -> peak follower -> limiter
// glide -> master gain -> clamp). The limiter has to be on the SUM, not per
// source, or two loud speakers would each pass at full scale and clip into
// distortion when added.
//
// ---------------------------------------------------------------------------
// TWO CLOCKS, AND WHY THIS FILE HAS A CONTROL LOOP
//
// The samples are produced on the SERVER's sim clock and consumed by the
// sound card's clock. Those clocks are unrelated:
//   • the server advances a fixed step budget per 30 Hz tick, so a heavy
//     circuit makes sim time run SLOWER than wall time (by design — dilate
//     sim time, never stall the loop). Audio then arrives slower than it is
//     played, and the ring drains;
//   • network jitter and tick scheduling do the same thing in smaller doses,
//     in both directions;
//   • even with a perfect producer, a crystal mismatch between the server's
//     nominal 12.5 kHz sample clock and the device's 48 kHz is a permanent
//     drift of tens of ppm, which walks the buffer to one end eventually.
// A fixed resample ratio has no answer to any of that: the buffer walks to
// empty and the worklet underruns, which is what "sounds weird" means. So
// the read cursor is rate-matched — a proportional controller trims the
// playback rate by a fraction of a percent to hold a target buffer depth —
// and every source reports its buffer depth, underruns and applied trim back
// to the main thread so the player can SEE the state instead of guessing.
//
// What rate matching cannot do: manufacture samples. A producer that is
// permanently 10 % slow (a sim dilated to 0.9x by a heavy circuit) cannot
// be tracked by a 3 % trim. Draining, fading and re-priming — the original
// answer — turns sustained dilation into a burst/gap limit cycle: play
// 0.2/(0.97-rt) seconds, go silent for a full re-prime, repeat forever,
// which a player hears as "audio never keeps up". So when the SERVER'S OWN
// realtime ratio (it rides every audio chunk) sits beyond the trim's
// authority, the base playback rate is SLAVED to it. That is not
// fabrication: the samples are still exactly the solver's waveform, heard
// on the sim's own timeline — a sim at 0.6x genuinely oscillates at 0.6x
// the wall-clock frequency — and the dock shows "sim 0.6x" so the player
// knows why the pitch fell. The slave engages with hysteresis and glides,
// so a healthy stream is still pitch-perfect and never wobbles.

/** Peak volts at and above which the output sits at `TARGET_FS`. */
export const REF_VOLTS = 5;
/**
 * Ceiling: the output level a REF_VOLTS-or-louder signal is held at.
 *
 * Was 0.2, which cost 14 dB for nothing — even a perfect 5 V circuit could
 * not exceed a fifth of full scale. 0.89 leaves ~1 dB of headroom under the
 * hard clamp for the limiter's glide to work in, which is all it needs
 * because the ducking side of that glide is ~2 ms.
 */
export const TARGET_FS = 0.89;
/**
 * Compression exponent: `out = TARGET_FS * (peak/REF_VOLTS)^AGC_SLOPE`.
 *
 * 1.0 would be no compression (and leave quiet rooms inaudible); 0 would be
 * flat normalisation (and throw away loudness entirely, so a whisper and a
 * rail would sound identical). 0.4 is a 2.5:1 ratio in dB — it lifts the
 * measured rooms by ~28 dB while keeping a louder circuit audibly louder.
 */
export const AGC_SLOPE = 0.4;
/**
 * Gain ceiling, so near-silence is not amplified into hiss. The audio wire
 * format quantises to 1e-4 V, so 200x puts one quantum of dither at 0.02 of
 * full scale — audible only when there is genuinely nothing else.
 */
export const AGC_MAX_GAIN = 200;

/**
 * Buffer depth the rate matcher holds, and the depth a source primes to
 * before its first sample plays, in seconds.
 *
 * This number is derived, not chosen by taste. It is the size of the
 * producer deficit the client can absorb without a glitch:
 *   • one server tick delivers 33.3 ms of audio, so ~33 ms is the smallest
 *     buffer that is not underrunning between every chunk;
 *   • the trim can only bend the consumption rate by TRIM_MAX (3 %), so a
 *     producer running 10 % slow is absorbed almost entirely by BUFFER:
 *     2 s of that costs 2 s x (10 % - ~1.5 % of trim help) ≈ 170 ms of
 *     depth. Surviving a 2 s load spike therefore needs ~200 ms;
 *   • the same 200 ms rides out a complete 200 ms network dropout with no
 *     audible event at all.
 * The price is latency: a speaker's sound is heard up to 200 ms after the
 * solver computed it. For continuous circuit sound (a buzzing coil, an
 * oscillator's tone) that is imperceptible; for a tone that starts when you
 * flip a switch it is a just-noticeable delay. That trade is deliberate —
 * a glitch is far more objectionable than 200 ms of latency — and it is one
 * constant to change if a future mode wants tighter response instead.
 *
 * (120 ms was the first guess and does NOT survive the case above: 2 s at
 * 10 % slow eats ~170 ms of depth, so 120 ms underruns audibly. The harness
 * asserts the 2 s case with ZERO underruns, which is what pins this at 200.)
 */
export const TARGET_BUF_S = 0.2;
/** Click-free fade in/out at start, stop, removal and underrun. */
export const FADE_S = 0.006;
/** DC blocker corner: a cone cannot follow DC, and neither do we. */
export const HP_HZ = 20;
/** Ring capacity per source, in source samples (~1.3 s at 12.5 kHz). */
export const RING = 1 << 14;

/**
 * Hard limit on the playback-rate trim, as a fraction of nominal rate.
 *
 * Rate matching is pitch shifting: ±3 % at 440 Hz is ±13 Hz, i.e. ±51 cents,
 * which is audible on a sustained tone if you go looking for it (the
 * just-noticeable difference for a slow pitch change is ~5-10 cents). 3 % is
 * therefore an EMERGENCY ceiling, not an operating point: the deadband and
 * the low gain below keep ordinary jitter and drift correction inside a few
 * cents, and the clamp only engages when the alternative is an underrun —
 * a 51-cent bend nobody notices beats a click everybody notices.
 */
export const TRIM_MAX = 0.03;
/**
 * No trim at all inside ±this of the target, in seconds.
 *
 * The producer delivers one 33.3 ms chunk per 30 Hz tick, so the buffer
 * depth sawtooths by a full chunk even when the server, the network and the
 * sound card are all perfect. A deadband narrower than one chunk would make
 * the controller chase that sawtooth (frequency-modulating every source at
 * 30 Hz); 45 ms clears it with margin, and inside it playback runs at
 * EXACTLY the nominal rate, so a healthy stream is pitch-perfect.
 */
export const TRIM_DEADBAND_S = 0.045;
/**
 * Proportional gain, trim per second of buffer error beyond the deadband.
 *
 * 0.3 /s puts the full ±3 % at ~150 ms of error, i.e. at a 50 ms buffer
 * (about to underrun) or a 350 ms buffer (latency worth spending pitch on).
 * A 60 ms error — a bad tick, a jittery socket — is corrected with 0.45 %,
 * about 8 cents. Deliberately gentle: this loop's job is to keep the buffer
 * off the rails over seconds, not to track the producer sample-accurately.
 */
export const TRIM_KP = 0.3;
/** Time constant of the trim glide: rate changes must not step. */
export const TRIM_TAU_S = 0.05;
/**
 * Time constant of the buffer-depth estimate the controller acts on.
 *
 * The producer delivers in 33 ms lumps and the network delivers those lumps
 * unevenly, so the instantaneous depth is a sawtooth riding on the thing we
 * actually care about (the slow drift of one clock against the other). The
 * loop therefore servos a smoothed depth: a burst of three late chunks is
 * averaged away instead of being answered with a pitch bend, and only a real
 * trend moves the rate. Telemetry still reports the RAW depth plus its 1 s
 * min/max — that is what a human debugging a hiccup needs to see.
 *
 * Cost: the estimate lags a steady drain by tau x drain rate (0.3 s x 10 % =
 * 30 ms of extra depth spent before the correction arrives), which is paid
 * out of TARGET_BUF_S.
 */
export const DEPTH_TAU_S = 0.3;
/** Telemetry reports per second, over the message port. */
export const STATS_HZ = 10;
/**
 * Starvation longer than this stops counting as an underrun and becomes a
 * STALL: the producer is gone, not late. Without the distinction a stopped
 * server would run the "total underrun ms" figure up forever, which tells
 * the player nothing they cannot already hear (silence).
 */
export const STALL_S = 0.25;
/**
 * Largest FORWARD jump in a source's sample clock that is concealed by
 * bridging instead of resetting, in seconds.
 *
 * The speaker stream is best-effort: a lagged socket silently skips whole
 * 33 ms messages. Resetting on every skip (the original behavior, threshold
 * 4 samples) discards up to TARGET_BUF_S of already-buffered REAL solver
 * audio and then sits silent through a full re-prime — one lost 33 ms
 * message cost ~7x its length in silence, and periodic losses turned
 * playback into mostly-priming. Instead, a gap of up to this many seconds
 * is written into the ring as a linear bridge between the last received
 * sample and the next chunk's first sample (the same interpolation the
 * resampler already does between adjacent samples, over a longer span), so
 * a lost message is what net.ts always claimed it was: a blip, not a reset.
 * Beyond this, continuity is genuinely gone (room restart, long stall) and
 * the source re-primes from scratch. Concealed time is counted and reported
 * as `concealedMs` — the player can always see how much was bridged.
 */
export const GAP_MAX_S = 0.25;
/**
 * Time constant of the smoothed server realtime ratio the rate slave acts
 * on, in seconds. The server already smooths rt (EMA, ~150 ms); this adds
 * enough on top that a one-tick spike cannot yank the base rate, while a
 * real dilation engages the slave before the buffer is fully drained.
 */
export const RATE_TAU_S = 0.3;
/**
 * Floor on the slaved base rate. A ratio below this (a quarantined or
 * near-stopped sim) is a producer that has effectively STOPPED — playing at
 * a twentieth of nominal would be an unrecognizable drone, not a dilated
 * version of the circuit's sound. Let it starve and report stalled instead.
 */
export const RATE_MIN = 0.25;

export const WORKLET_SRC = `
const r2 = (v) => Math.round(v * 100) / 100;
/** Telemetry history buckets: one per report, so ~1 s at STATS_HZ = 10. */
const BUCKETS = 10;

/** One streamed source: its own ring, cursor, rate matcher, fade and stats. */
class Src {
  constructor(size, cfg, gain) {
    this.ring = new Float32Array(size);
    this.size = size;
    this.cfg = cfg;        // shared tuning, see SimAudioProcessor
    this.fadeInc = cfg.fadeInc;
    this.gain = gain;      // per-source target gain (mute = 0)
    this.g = gain;         // ...and the glided value actually applied
    this.w = 0;            // absolute write count
    this.r = 0;            // absolute read cursor (fractional)
    this.step = 0;         // source samples consumed per output frame
    this.dts = 0;          // seconds per source sample (the producer's clock)
    this.prime = 0;        // source samples buffered before playback starts
    this.armed = false;
    this.ever = false;     // has played at least once since it appeared/reset
    this.fade = 0;
    this.last = 0;
    this.doomed = false;   // removed: fade out, then drop
    // ---- rate matching
    this.trim = 0;         // applied rate trim, -cfg.trimMax..+cfg.trimMax
    this.dAvg = 0;         // smoothed buffer depth in seconds (the loop's input)
    // ---- rate slaving (a sim dilated beyond the trim's authority)
    this.base = 1;         // applied base rate multiplier (1 = nominal)
    this.rtLast = 1;       // most recent server realtime ratio received
    this.rtAvg = 1;        // ...and its smoothed value (cfg.rateTau)
    this.rtSeen = false;   // first ratio initializes rtAvg directly
    this.slaved = false;   // hysteresis state: following rtAvg, not nominal
    // ---- telemetry
    this.avail = 0;        // buffered source samples, sampled per block
    this.underruns = 0;    // starvation events since this source appeared
    this.starved = 0;      // output frames lost to starvation (bounded, see stall)
    this.starving = 0;     // consecutive starved frames this event
    this.stalled = false;  // starved past cfg.stallFrames: producer is gone
    this.drops = 0;        // ring overflows: oldest audio discarded
    this.dropped = 0;      // source samples discarded by those overflows
    this.concealed = 0;    // source samples bridged over lost chunks
    this.bkMin = Infinity; // depth min/max since the last report, in ms
    this.bkMax = 0;
    this.hMin = new Float64Array(BUCKETS);
    this.hMax = new Float64Array(BUCKETS);
    this.hAt = 0;
    this.hN = 0;
  }

  setRate(dts) {
    if (!(dts > 0)) return;
    const step = 1 / (dts * sampleRate);
    if (Math.abs(step - this.step) < 1e-12) return;
    this.dts = dts;
    this.step = step;
    // Prime to the TARGET depth, not to some smaller "enough to start"
    // figure: starting thin just means underrunning a moment later. The 1 %
    // shave matters: delivery is quantized in whole chunks, and the server's
    // real chunks are 416 samples (integer division of the tick budget), so
    // 6 of them buffer 2496 samples — 4 SHORT of an exact round(0.2/80 µs) =
    // 2500. Without the shave every prime waited a whole 7th chunk (+33 ms)
    // for those 4 samples.
    this.prime = Math.max(
      4,
      Math.min(this.size >> 1, Math.round((0.99 * this.cfg.target) / dts)),
    );
  }

  reset() {
    this.w = 0;
    this.r = 0;
    this.step = 0;
    this.armed = false;
    // Back to PRIMING, not to "thin": a client-driven re-prime (a t0 jump, a
    // fresh speaker) is a stream that has not started, and the readout must
    // not accuse a filling buffer of being about to glitch.
    this.ever = false;
    this.last = 0;
    // A re-prime is a stream discontinuity, not a buffer failure: keep the
    // cumulative counters (they are session totals) but drop the in-flight
    // starvation state and the trim. The rate-slave state (rtAvg, slaved,
    // base) survives on purpose — dilation is a property of the PRODUCER,
    // not of this buffer, and restarting a dilated stream at nominal rate
    // would just walk it straight back into the drain that caused the reset.
    this.trim = 0;
    this.dAvg = 0;
    this.starving = 0;
    this.stalled = false;
    this.avail = 0;
  }

  write(s) {
    for (let k = 0; k < s.length; k++) {
      this.ring[this.w % this.size] = s[k];
      this.w++;
    }
    this.overflow();
  }

  /**
   * Conceal a LOST stretch of the stream: n source samples never arrived
   * (a lagged socket skipped whole messages), and the next chunk starts at
   * \`to\`. Write a linear bridge from the last received sample to \`to\` —
   * the same interpolation the read cursor already does between adjacent
   * samples, spanning the hole — so the buffered REAL audio on either side
   * keeps playing instead of being thrown away for a full re-prime. The
   * bridged time is counted: it is the one thing in this file that is not
   * a solver sample, and telemetry must say so.
   */
  bridge(n, to) {
    if (this.w === 0) return; // nothing to bridge from: prime normally
    let cap = this.size >> 1; // the main thread caps harder; belt and braces
    if (n > cap) n = cap;
    const from = this.ring[(this.w - 1) % this.size];
    for (let k = 1; k <= n; k++) {
      this.ring[this.w % this.size] = from + ((to - from) * k) / (n + 1);
      this.w++;
    }
    this.concealed += n;
    this.overflow();
  }

  /** Over-full is the rate matcher's job (it runs slightly fast and drains
   * the surplus inaudibly). The ONLY thing handled here is a genuine ring
   * overflow — the writer lapping the reader — which loses audio whatever
   * we do, so it is counted instead of being silently absorbed. */
  overflow() {
    const cap = this.size - 2;
    if (this.w - this.r > cap) {
      const keep = Math.min(cap, Math.max(this.prime, 4));
      this.dropped += this.w - this.r - keep;
      this.r = this.w - keep;
      this.drops++;
    }
  }

  /** Latest server realtime ratio (sim seconds per wall second), straight
   * off the audio message this source's chunks ride. Smoothed in control(). */
  ratio(rt) {
    if (!(rt >= 0) || !isFinite(rt)) return;
    this.rtLast = rt;
    if (!this.rtSeen) {
      // First report: adopt it outright, so a stream that STARTS dilated
      // slaves immediately instead of draining while the EMA crawls down.
      this.rtAvg = rt;
      this.rtSeen = true;
    }
  }

  /** Once per render quantum: hold the buffer at the target depth by trimming
   * the playback rate, and sample the depth for telemetry. */
  control(frames) {
    const avail = this.w - this.r;
    this.avail = avail > 0 ? avail : 0;
    const ms = this.avail * this.dts * 1000;
    if (ms < this.bkMin) this.bkMin = ms;
    if (ms > this.bkMax) this.bkMax = ms;
    const cfg = this.cfg;
    // Smooth the depth before servoing on it: chunk lumpiness is not drift.
    const d = this.avail * this.dts;
    this.dAvg += (d - this.dAvg) * Math.min(1, frames / (cfg.depthTau * sampleRate));
    let want = 0;
    if (this.armed && this.step > 0) {
      // Positive error = too much buffer = run fast to drain it.
      const e = this.dAvg - cfg.target;
      const over = (e > 0 ? e : -e) - cfg.deadband;
      if (over > 0) {
        want = cfg.kp * (e > 0 ? over : -over);
        if (want > cfg.trimMax) want = cfg.trimMax;
        else if (want < -cfg.trimMax) want = -cfg.trimMax;
      }
    }
    // Glide, so the rate itself never steps (a step is a click's worth of
    // phase discontinuity spread over one block).
    const a = Math.min(1, frames / (cfg.trimTau * sampleRate));
    this.trim += (want - this.trim) * a;
    if (Math.abs(want - this.trim) < 1e-7) this.trim = want;
    // The clamp is a guarantee, not a consequence of the maths above.
    if (this.trim > cfg.trimMax) this.trim = cfg.trimMax;
    else if (this.trim < -cfg.trimMax) this.trim = -cfg.trimMax;
    // ---- rate slave (see the header): the trim above can bend consumption
    // by ±trimMax around the BASE rate; this loop moves the base itself when
    // the server says its sim is dilated further than the trim can follow.
    // Wall-clock EMA of the reported ratio, then hysteresis: engage only
    // once the deficit is beyond the trim's authority, disengage only once
    // the trim could comfortably take over again — a ratio hovering at the
    // threshold must not gate the slave on and off audibly.
    if (this.rtSeen) {
      this.rtAvg += (this.rtLast - this.rtAvg) * Math.min(1, frames / (cfg.rateTau * sampleRate));
      if (this.slaved) {
        if (this.rtAvg > cfg.rateOff) this.slaved = false;
      } else if (this.rtAvg < cfg.rateOn) {
        this.slaved = true;
      }
    }
    const wantBase = this.slaved ? Math.max(cfg.rateMin, Math.min(1, this.rtAvg)) : 1;
    this.base += (wantBase - this.base) * a;
    if (Math.abs(wantBase - this.base) < 1e-7) this.base = wantBase;
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
        this.r += this.step * this.base * (1 + this.trim);
        this.last = s;
        this.starving = 0;
        if (!this.doomed) this.fade = Math.min(1, this.fade + this.fadeInc);
        else this.fade = Math.max(0, this.fade - this.fadeInc);
      } else {
        // Underrun: the producer did not keep up and there is nothing left to
        // interpolate. Coast and fade to silence rather than stretch the last
        // sample indefinitely (that is a buzz, not a note).
        this.armed = false;
        this.underruns++;
        this.starving = 1;
        this.starved++;
        this.fade = Math.max(0, this.fade - this.fadeInc);
      }
    } else {
      this.fade = Math.max(0, this.fade - this.fadeInc);
      if (this.starving > 0) {
        if (this.starving < this.cfg.stallFrames) {
          this.starving++;
          this.starved++;
        } else {
          // Long silence is a stalled producer, reported as a STATE. It stops
          // inflating the underrun total, which is about audible glitches.
          this.stalled = true;
        }
      }
      // Re-prime. A FRESH source (or one reset by a real discontinuity)
      // waits for the full target: starting thin just underruns a moment
      // later. But a source that was already playing and got starved by a
      // producer hiccup re-arms at HALF the target: the full-prime rule made
      // every glitch cost >=200 ms of silence on top of the glitch itself,
      // and half the target still rides out one whole late tick while the
      // trim (running slow) refills the rest in the background.
      const need = this.ever ? Math.max(4, this.prime >> 1) : this.prime;
      if (!this.doomed && this.fade <= 0 && this.step > 0 && this.w - this.r >= need) {
        this.r = this.w - need;
        this.armed = true;
        this.ever = true;
        this.trim = 0;
        // Start the loop centred on the depth we just primed to, or it would
        // read the pre-arm ramp as a huge deficit and pull the clamp.
        this.dAvg = need * this.dts;
        this.starving = 0;
        this.stalled = false;
      }
    }
    // Exact zero once faded out or muted: a removed source must not leave a
    // DC sliver sitting in the mix forever.
    const a = this.fade * this.g;
    if (a <= 0) return 0;
    return s * a;
  }

  /** Telemetry snapshot. Rotates the ~1 s min/max history, so it must be
   * called exactly once per report. */
  stats() {
    const now = this.avail * this.dts * 1000;
    this.hMin[this.hAt] = this.bkMin === Infinity ? now : this.bkMin;
    this.hMax[this.hAt] = this.bkMax;
    this.hAt = (this.hAt + 1) % BUCKETS;
    if (this.hN < BUCKETS) this.hN++;
    this.bkMin = Infinity;
    this.bkMax = 0;
    let lo = Infinity;
    let hi = 0;
    for (let k = 0; k < this.hN; k++) {
      if (this.hMin[k] < lo) lo = this.hMin[k];
      if (this.hMax[k] > hi) hi = this.hMax[k];
    }
    return {
      ms: r2(now),
      minMs: r2(lo === Infinity ? now : lo),
      maxMs: r2(hi),
      underruns: this.underruns,
      underrunMs: r2((this.starved / sampleRate) * 1000),
      drops: this.drops,
      droppedMs: r2(this.dropped * this.dts * 1000),
      concealedMs: r2(this.concealed * this.dts * 1000),
      stalled: this.stalled,
      armed: this.armed,
      // The rate actually applied to the read cursor, trim aside: 1 nominal,
      // <1 slaved to a dilated sim (the honest pitch the player is hearing).
      rate: Math.round(this.base * 1000) / 1000,
      // Filling up for the first time: the depth is LOW BY DESIGN, so the
      // readout says "priming" instead of warning about a thin buffer.
      priming: !this.ever,
      trim: Math.round(this.trim * 1e6) / 1e6,
    };
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
    // Two different "targets" live in this file: fsTarget is the limiter's
    // full-scale aim, cfg.target is the buffer depth the rate matcher holds.
    this.fsTarget = o.target || 0.2;
    this.refV = o.refV || 5;
    // The compression law. See the auto-gain block in process().
    this.slope = o.slope || 0.4;
    this.maxGain = o.maxGain || 200;
    // A REF_VOLTS sine's RMS: the loudness that maps to the ceiling.
    this.refRms = this.refV / Math.SQRT2;
    this.ms = 0;
    // Loudness follower coefficient, tau ~0.3 s. Slow enough that it reads
    // the programme rather than the waveform, fast enough to follow a knob.
    this.msA = 1 - Math.exp(-1 / (0.3 * sampleRate));

    const fadeSec = o.fadeSec || 0.006;
    const trimMax = o.trimMax === undefined ? 0.03 : o.trimMax;
    // Shared, immutable tuning handed to every source.
    this.cfg = {
      target: o.targetSec || 0.2,
      deadband: o.deadbandSec === undefined ? 0.045 : o.deadbandSec,
      kp: o.kp === undefined ? 0.3 : o.kp,
      trimMax: trimMax,
      trimTau: o.trimTauSec || 0.05,
      depthTau: o.depthTauSec || 0.3,
      fadeInc: 1 / Math.max(1, fadeSec * sampleRate),
      stallFrames: Math.max(1, Math.round((o.stallSec || 0.25) * sampleRate)),
      rateTau: o.rateTauSec || 0.3,
      rateMin: o.rateMin === undefined ? 0.25 : o.rateMin,
      // Slave hysteresis, derived from the trim clamp: engage once the
      // dilation is beyond what the trim could track, disengage only when
      // the trim could hold the residual with room to spare.
      rateOn: 1 - trimMax,
      rateOff: 1 - trimMax / 4,
    };
    this.hpCoef = 1 - (2 * Math.PI * (o.hpHz || 20)) / sampleRate;
    // Consecutive all-zero-sum samples before the output stage is hard-reset.
    // Without this the DC blocker rings down exponentially and "silence" is a
    // denormal tail that never quite reaches zero.
    this.hushMax = Math.max(8, Math.round(fadeSec * sampleRate));
    this.hush = 0;
    this.srcs = new Map();
    this.live = [];       // per-block snapshot of srcs, reused (no allocation)
    this.master = 1;      // 0..1 volume
    this.mute = false;
    this.masterFade = 1;  // click-free mute/unmute
    this.peak = 0;
    // Start where the OLD law sat: exactly right for a REF_VOLTS circuit, and
    // quiet enough for anything else that the ramp is heard as a fade-in
    // rather than a jump. A multiplicative ramp also cannot start at 0.
    this.gain = this.fsTarget / this.refV;
    this.hpX = 0;
    this.hpY = 0;
    // Telemetry cadence. Never per render quantum: at 48 kHz that would be
    // 375 postMessage calls a second to tell the UI something it redraws 10
    // times a second.
    this.statsEvery = Math.max(1, Math.round(sampleRate / (o.statsHz || 10)));
    this.sinceStats = 0;
    this.port.onmessage = (ev) => {
      const m = ev.data;
      if (m.t === 'chunk') {
        const s = this.src(m.id, m.gain);
        s.doomed = false;
        if (m.gain !== undefined) s.gain = m.gain;
        s.setRate(m.dts);
        if (m.rt !== undefined) s.ratio(m.rt);
        // The main thread saw a small forward jump in this source's sample
        // clock: m.gap samples were LOST in transit. Bridge them so the
        // audio buffered on both sides survives, instead of resetting.
        if (m.gap > 0) s.bridge(m.gap, m.s.length ? m.s[0] : 0);
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
      s = new Src(this.size, this.cfg, gain === undefined ? 1 : gain);
      this.srcs.set(id, s);
    }
    return s;
  }

  /** Buffer health for the main thread: one entry per source plus a mix
   * summary (the mix is only as healthy as its thinnest source). */
  report() {
    this.sinceStats = 0;
    const s = {};
    let ms = Infinity;
    let minMs = Infinity;
    let maxMs = 0;
    let underruns = 0;
    let underrunMs = 0;
    let drops = 0;
    let stalled = false;
    let trim = 0;
    let n = 0;
    let priming = 0;
    let rate = 1;
    let concealedMs = 0;
    for (const [id, src] of this.srcs) {
      const st = src.stats();
      s[id] = st;
      if (src.doomed) continue; // on its way out: not part of the health story
      n++;
      if (st.priming) priming++;
      if (st.ms < ms) ms = st.ms;
      if (st.minMs < minMs) minMs = st.minMs;
      if (st.maxMs > maxMs) maxMs = st.maxMs;
      underruns += st.underruns;
      underrunMs += st.underrunMs;
      drops += st.drops;
      concealedMs += st.concealedMs;
      if (st.stalled) stalled = true;
      if (Math.abs(st.trim) > Math.abs(trim)) trim = st.trim;
      if (st.rate < rate) rate = st.rate; // most-slaved source: the mix's pitch story
    }
    this.port.postMessage({
      t: 'stats',
      mix: {
        sources: n,
        // Sources still filling their first buffer. The depth below is the
        // MINIMUM across sources, so one priming source drags it to near
        // zero — true, but not a warning, and the UI needs to know which.
        priming: priming,
        ms: n ? r2(ms) : 0,
        minMs: n ? r2(minMs) : 0,
        maxMs: r2(maxMs),
        underruns: underruns,
        underrunMs: r2(underrunMs),
        drops: drops,
        concealedMs: r2(concealedMs),
        stalled: stalled,
        trim: trim,
        rate: rate,
        target: this.cfg.target * 1000,
      },
      s: s,
    });
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
      this.sinceStats += out.length;
      if (this.sinceStats >= this.statsEvery) this.report();
      return true;
    }
    const live = this.live;
    live.length = 0;
    for (const s of this.srcs.values()) live.push(s);
    // Rate matching runs once per block, not per sample: the buffer moves by
    // one chunk every 33 ms, so a 2.7 ms control period is already 12x
    // faster than the thing it is controlling.
    for (let j = 0; j < live.length; j++) live[j].control(out.length);
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
          this.ms = 0;
          this.gain = this.fsTarget / this.refV;
      
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
      // COMPRESSIVE AUTO-GAIN on the SUM.
      //
      // The old law was fsTarget / max(refV, peak), which floored the
      // denominator at refV and therefore could only ever DUCK. Anything
      // quieter than 5 V got one fixed tiny gain, so the measured rooms --
      // 0.20 V for the ladder, 0.28 V for the TR-808 -- played at -42 and
      // -39 dBFS while the same laptop plays a YouTube sine at 0. That is
      // roughly 90x too small in amplitude, and no amount of system volume
      // fixes it because the loss is here, not in the speakers.
      //
      // Loudness is still information, which is why this is a power law and
      // not a flat normaliser: out = ceil * (peak/refV)^slope, capped at
      // ceil. A louder circuit is still audibly louder; the range is just
      // compressed into one the ear and a laptop speaker can both use. At
      // slope 0.4 a 74 dB span of input becomes a 30 dB span of output --
      // a 2.5:1 ratio, ordinary mastering-compressor territory.
      //
      // Above refV the min() pins the output at ceil, so this is still the
      // same limiter it always was: a 100 V rail lands at ceil, not on the
      // clipper. Below it, gain is capped at maxGain so a nearly-silent room
      // does not amplify the wire format's 1e-4 V quantisation into hiss.
      // TWO FOLLOWERS, because they answer different questions.
      //
      // LOUDNESS (mean square) drives the gain. Driving it from the PEAK
      // instead — which is what the first version of this did — penalises
      // crest factor twice over: a spiky signal is made quiet because of its
      // spikes, and adding a second speaker makes the room QUIETER, because
      // two uncorrelated tones raise the peak by 2x while raising loudness
      // by only 1.41x. The harness caught exactly that (two quiet sources
      // measured 0.331 -> 0.309). RMS is also simply the better proxy for
      // what an ear calls "loud".
      //
      // PEAK drives the limiter, which is now the only thing standing
      // between the AGC and the clipper, and is therefore allowed to
      // override it downward at any time.
      const mag = y < 0 ? -y : y;
      this.peak = mag > this.peak ? mag : this.peak * 0.99995 + mag * 0.00005;
      this.ms += (y * y - this.ms) * this.msA;
      const rms = Math.sqrt(this.ms);
      const agc =
        rms > 0
          ? (this.refRms * this.fsTarget * Math.min(1, Math.pow(rms / this.refRms, this.slope))) /
            (this.refV * rms)
          : this.maxGain;
      // The limiter always wins: whatever loudness asks for, the peak may
      // not exceed the ceiling.
      const want = Math.min(this.maxGain, agc, this.peak > 0 ? this.fsTarget / this.peak : this.maxGain);
      // Limiter glide: duck fast (~2 ms) so a sudden 100 V rail does not sit
      // on the clipper, recover slowly (~25 ms) so it does not pump.
      // Down fast (~2 ms), up SLOWLY AND MULTIPLICATIVELY.
      //
      // The old proportional rise, gain += (want - gain) * k, moves in
      // proportion to how far away the target is, so with a maxGain of 200
      // an empty follower slams the gain up by a factor of hundreds within a
      // few milliseconds. That defeated the source's own 6 ms fade-in
      // completely (measured: a 0.49 discontinuity at the first sample),
      // because the peak cap then held the output at the ceiling from sample
      // one. Rising by a fixed FRACTION of the current gain instead makes
      // the ramp exponential in dB and independent of the distance — about
      // +4 dB per 100 ms — so a fade stays a fade and a knob still feels
      // immediate.
      if (want < this.gain) this.gain += (want - this.gain) * 0.01;
      else this.gain = Math.min(want, this.gain * 1.0002);
      // Mute/volume glides too, so toggling it is silent rather than a click.
      this.masterFade += (wantMaster - this.masterFade) * 0.02;
      if (Math.abs(wantMaster - this.masterFade) < 1e-4) this.masterFade = wantMaster;
      // THE LIMITER IS NOT ALLOWED TO BE LATE. The glide is how loudness
      // moves smoothly, but a glide is by definition behind its target, so
      // at a source's first sample — or any sudden onset — the gliding gain
      // can sit far above what the current peak can afford, and the ~2 ms
      // duck arrives after the damage. The peak follower attacks
      // instantaneously, so fsTarget/peak is an exact per-sample bound that
      // is available now; applying it to the OUTPUT rather than to the glide
      // state makes clipping arithmetically impossible (|y| <= peak, so
      // |y| * fsTarget/peak <= fsTarget), while leaving the glide free to go
      // on being smooth underneath.
      const cap = this.peak > 0 ? this.fsTarget / this.peak : this.maxGain;
      const v = y * Math.min(this.gain, cap) * this.masterFade;
      out[k] = v > 1 ? 1 : v < -1 ? -1 : v;
    }
    // Reap finished removals after the block, never mid-loop.
    for (const [id, s] of this.srcs) if (s.gone) this.srcs.delete(id);
    for (let c = 1; c < ch.length; c++) ch[c].set(out);
    this.sinceStats += out.length;
    if (this.sinceStats >= this.statsEvery) this.report();
    return true;
  }
}
registerProcessor('sim-audio', SimAudioProcessor);
`;
