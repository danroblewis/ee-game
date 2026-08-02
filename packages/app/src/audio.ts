// Real-time audio: speakers, plus "listen" probes (press 3 over an element).
//
// Nothing in here synthesizes sound. The samples are the solver's own
// waveform — a Speaker element's terminal voltage, or the exact chunks the
// oscilloscope draws for a probe — handed to an AudioWorklet that keeps one
// ring buffer PER SOURCE, resamples each to the AudioContext rate, sums them
// and limits the sum. If you hear a tone, the circuit is oscillating; if you
// hear nothing, it isn't (design pillar: every number a player perceives
// comes from the solver).
//
// Honest limits:
//   • Speakers stream on the server's dedicated audio cadence: dt=20 µs × 4
//     = 12.5 kHz, so a 6.25 kHz Nyquist ceiling. A 440 Hz tone arrives with
//     28 samples per cycle and sounds like a 440 Hz tone; content above
//     6.25 kHz was already aliased by the server's decimation and folds down.
//   • Listen probes ride the SCOPE cadence (dt × 16 = 3.125 kHz, ~1.5 kHz of
//     real bandwidth) — 7 samples per cycle at 440 Hz, which is why speakers
//     got their own faster stream instead of borrowing this one.
//   • Offline (local WASM sim) there is no substep sampler, so the only
//     source is a listen probe fed one sample per animation frame (~60 Hz):
//     expect a rumble that tracks the envelope, not the tone. Speakers are
//     SILENT offline, and the HUD says so rather than playing aliased mush.
//   • Samples are produced on the server's SIM clock and consumed on the
//     sound card's WALL clock, and those never agree exactly (a dilated sim,
//     network jitter, crystal drift). The worklet rate-matches to hold
//     TARGET_BUF_S of buffer, which costs that much latency and reports its
//     own depth/underruns back here as `status().buf` so the dock can show
//     it. `status().ratio` is the server's own dilation figure, so "the sim
//     is at 0.6x" and "my network hiccuped" are distinguishable. When that
//     ratio sits beyond the rate matcher's ±3 % authority, the worklet
//     follows it outright: a sim at 0.6x is HEARD at 0.6x — lower pitch,
//     continuous sound — instead of the old burst/gap cycle of draining,
//     going silent and re-priming forever. Lost chunks (a best-effort
//     stream) are bridged in place, not answered with a 200 ms re-prime.

import {
  DEPTH_TAU_S,
  FADE_S,
  GAP_MAX_S,
  HP_HZ,
  RATE_MIN,
  RATE_TAU_S,
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
import { lsFlag, lsNum, lsSet } from './store';

/** Offline: animation-frame samples batched per message. */
const OFFLINE_BATCH = 4;
/** Level below which a source counts as silent (for the HUD and the glow). */
const QUIET = 0.004;
/** A level reading older than this is stale — the stream stopped. */
const LEVEL_TTL_S = 0.5;
/** Telemetry older than this is not worth showing (context parked, worklet
 * dead): the readout says "—" rather than a frozen depth. */
const STATS_TTL_MS = 1000;
/** An underrun this recently is still "happening" as far as the UI cares —
 * long enough to be seen at 10 Hz, short enough not to nag forever. */
const UNDERRUN_RECENT_MS = 5000;
/** The server's realtime ratio goes stale this long after the last audio
 * message (socket closed, room went quiet). */
const RATIO_TTL_MS = 2000;

const VOL_KEY = 'ee.audio.vol';
const MUTE_KEY = 'ee.audio.mute';

/** Worklet-side key for each kind of source. Speakers are element ids and
 * probes are pids; the two id spaces overlap, so they get separate prefixes. */
const speakerKey = (elem: number) => `s${elem}`;
const probeKey = (pid: number) => `p${pid}`;

/** Main-thread bookkeeping for one streamed source. The samples themselves
 * live only in the worklet; this is what the UI needs to draw. */
interface Source {
  /** Expected sample-clock time of the next chunk (gap detection). */
  nextT: number | null;
  /** 0..1 peak level of the most recent chunks (REF_VOLTS peak = 1). */
  lvl: number;
  /** performance.now() of the last chunk, so a dead stream reads 0. */
  lvlAt: number;
}

/** The slice of the player the scope dock's audio controls need. */
export interface AudioControls {
  readonly muted: boolean;
  readonly volume: number;
  setMuted(v: boolean): void;
  setVolume(v: number): void;
  status(): AudioStatus;
}

/** What the worklet reports about one source's ring buffer, or about the mix
 * (the mix is only as healthy as its thinnest source). Every field is
 * measured in the audio thread — none of it is inferred on the main thread,
 * where a stalled rAF would lie about time. */
export interface AudioBufferHealth {
  /** Buffered audio ahead of the read cursor, in milliseconds. */
  ms: number;
  /** Min/max of `ms` over the last ~1 s of reports. */
  minMs: number;
  maxMs: number;
  /** Depth the rate matcher is holding, in ms — as REPORTED by the worklet
   * (TARGET_BUF_S), so the readout cannot disagree with the loop. */
  targetMs: number;
  /** Starvation events since the session started (see `recent`). */
  underruns: number;
  /** Total audio time lost to starvation, in ms. Capped per event at
   * STALL_S — beyond that the producer is stalled, not late, and `stalled`
   * says so instead. */
  underrunMs: number;
  /** Ring overflows: the producer ran so far ahead that oldest audio had to
   * be discarded. Normal over-fill is drained by the rate matcher instead. */
  drops: number;
  /** Total ms of LOST stream time bridged over instead of re-primed: lost
   * chunks are concealed with an interpolated ramp between the real samples
   * on either side, and this is the honest count of that. */
  concealedMs: number;
  /** True while a source has been starved past STALL_S: the producer stopped
   * rather than fell behind. */
  stalled: boolean;
  /** Applied playback-rate trim, e.g. -0.012 = playing 1.2 % slow to refill.
   * |trim| <= TRIM_MAX always. */
  trim: number;
  /** Applied BASE playback rate: 1 nominal, <1 when the worklet is slaved to
   * a dilated sim (the sound is at the sim's own pitch; the dock's "sim
   * 0.6x" readout is the explanation). */
  rate: number;
  /** True when an underrun happened within the last few seconds. */
  recent: boolean;
  /** Sources included in this summary. */
  sources: number;
  /** Sources still filling their FIRST buffer (a new speaker, a re-prime
   * after a time gap). `ms` is the minimum across sources, so one priming
   * source pulls it to near zero: that is by design, not a warning, and the
   * readout must say "priming" rather than cry glitch. */
  priming: number;
}

export interface AudioStatus {
  /** Speaker sources currently streaming. */
  speakers: number;
  /** True while a '3' listen probe is latched. */
  listening: boolean;
  /** True when some source is actually above the noise floor. */
  sounding: boolean;
  /** True when audio is ready but the browser is waiting for a gesture. */
  needsGesture: boolean;
  /** Mix buffer health, or null before the first report / when stale. */
  buf: AudioBufferHealth | null;
  /** The SERVER's sim-seconds-per-wall-second, from the audio message, or
   * null offline / when stale. Below 1 the sim is dilated and audio
   * physically cannot keep up — a different problem from network jitter,
   * and the only way the player can tell the two apart. */
  ratio: number | null;
}

/** Raw per-source telemetry as the worklet sends it. */
interface SrcStats {
  ms: number;
  minMs: number;
  maxMs: number;
  underruns: number;
  underrunMs: number;
  drops: number;
  droppedMs: number;
  concealedMs: number;
  stalled: boolean;
  armed: boolean;
  priming: boolean;
  trim: number;
  rate: number;
}

interface MixStats {
  sources: number;
  priming: number;
  ms: number;
  minMs: number;
  maxMs: number;
  underruns: number;
  underrunMs: number;
  drops: number;
  concealedMs: number;
  stalled: boolean;
  trim: number;
  rate: number;
  target: number;
}

/**
 * Mixes N solver streams to the sound card. Sources are added by
 * `setSpeakers` (one per Speaker element in the document) and by `listen`
 * (the '3' probe). Feed them with `pushSpeakerChunk` / `pushChunk` online or
 * `pushPoint` offline; every push is a no-op for a source that does not
 * exist, so callers can pipe everything in without filtering.
 */
export class AudioPlayer implements AudioControls {
  private ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private booting: Promise<void> | null = null;
  private dead = false;
  /** pid of the '3'-listen probe, or null. */
  private src: number | null = null;
  /** Live sources by worklet key. */
  private srcs = new Map<string, Source>();
  /** Speaker element ids currently streamed (mirrors the document). */
  private speakers = new Set<number>();
  /** Per-speaker mute, and solo (which mutes every other speaker). */
  private mutedSpeakers = new Set<number>();
  private soloed: number | null = null;
  /** Offline batching buffer, per source key: [t, v, t, v, ...]. */
  private pts = new Map<string, number[]>();
  private vol = Math.min(1, Math.max(0, lsNum(VOL_KEY, 0.8)));
  private mute = lsFlag(MUTE_KEY);
  /** Latest worklet telemetry, and when it landed. */
  private mix: MixStats | null = null;
  private srcStats = new Map<string, SrcStats>();
  private statsAt = 0;
  /** Session totals. The worklet's counters die with their source (a deleted
   * speaker takes its history with it), so the deltas are accumulated here:
   * "3 underruns this session" is what a player can act on. */
  private totUnder = 0;
  private totUnderMs = 0;
  private totDrops = 0;
  private lastUnderAt = -Infinity;
  /** Per-source counter values already folded into the totals above. */
  private counted = new Map<string, { u: number; ms: number; d: number }>();
  /** Server dilation from the audio message: sim seconds per wall second. */
  private rt = 0;
  private rtAt = -Infinity;

  /** True once the page has had a real user gesture. An AudioContext CREATED
   * before that is permanently "blocked" in Chrome — later resume() calls on
   * it are refused and log an autoplay warning every time. So we do not build
   * one until this flips, and then we build it INSIDE the gesture handler,
   * where a fresh context starts already running. */
  private activated = false;

  constructor() {
    const kick = () => {
      this.activated = true;
      if (this.srcs.size === 0) return;
      // First gesture with audio waiting: create the context here, in the
      // gesture's own task (boot() constructs it synchronously before its
      // first await). Otherwise just un-park an existing one.
      if (!this.ctx) void this.boot();
      else this.wake();
    };
    window.addEventListener('pointerdown', kick);
    window.addEventListener('keydown', kick);
  }

  // ------------------------------------------------------------ listen probe

  /** pid of the probe being listened to, or null when not listening. */
  get pid(): number | null {
    return this.src;
  }

  /** 0..1 loudness of the '3'-listen stream (REF_VOLTS peak = 1), for the
   * listen glyph. Unchanged meaning from the single-source version. */
  get level(): number {
    return this.src === null ? 0 : this.levelOf(probeKey(this.src));
  }

  listen(pid: number) {
    if (this.src === pid) return;
    if (this.src !== null) this.remove(probeKey(this.src));
    this.src = pid;
    this.add(probeKey(pid));
    void this.boot();
  }

  stop() {
    if (this.src === null) return;
    this.remove(probeKey(this.src));
    this.src = null;
    this.idle();
  }

  // --------------------------------------------------------------- speakers

  /** Reconcile the streamed speaker set with the document. Speakers that
   * appeared start silent and fade in; ones that vanished fade out (a delete
   * must not click). Cheap enough to call whenever the document changes. */
  setSpeakers(ids: Iterable<number>) {
    const want = ids instanceof Set ? ids : new Set(ids);
    for (const id of this.speakers) {
      if (!want.has(id)) {
        this.remove(speakerKey(id));
        this.speakers.delete(id);
        this.mutedSpeakers.delete(id);
        if (this.soloed === id) this.soloed = null;
      }
    }
    for (const id of want) {
      if (this.speakers.has(id)) continue;
      this.speakers.add(id);
      this.add(speakerKey(id));
    }
    if (this.srcs.size === 0) this.idle();
  }

  /** 0..1 of what this speaker is putting into the player's EARS, for the
   * schematic glow: 0 when it is not streamed (offline, or past the server's
   * tap cap), when its own stream is silent, or when sound is muted. The
   * glyph means audible, not merely driven — the yellow arcs the renderer
   * draws from the solver frame already mean driven. */
  speakerLevel(elem: number): number {
    if (this.mute) return 0;
    return this.levelOf(speakerKey(elem));
  }

  /** True when this speaker exists as a source and is audible right now. */
  speakerStreamed(elem: number): boolean {
    return this.speakers.has(elem);
  }

  speakerMuted(elem: number): boolean {
    return this.gainOf(elem) === 0;
  }

  muteSpeaker(elem: number, mute: boolean) {
    if (mute) this.mutedSpeakers.add(elem);
    else this.mutedSpeakers.delete(elem);
    // Un-muting the soloed-out speaker is a request to end the solo.
    if (!mute && this.soloed !== null && this.soloed !== elem) this.soloed = null;
    this.pushGains();
  }

  /** Solo this speaker (every other one goes quiet); again to clear. */
  soloSpeaker(elem: number) {
    this.soloed = this.soloed === elem ? null : elem;
    this.pushGains();
  }

  isSoloed(elem: number): boolean {
    return this.soloed === elem;
  }

  // ----------------------------------------------------------------- global

  get muted(): boolean {
    return this.mute;
  }

  setMuted(v: boolean) {
    this.mute = v;
    lsSet(MUTE_KEY, v ? '1' : '0');
    this.pushMaster();
  }

  get volume(): number {
    return this.vol;
  }

  setVolume(v: number) {
    this.vol = Math.min(1, Math.max(0, v));
    lsSet(VOL_KEY, String(this.vol));
    this.pushMaster();
  }

  /**
   * The server's realtime ratio, straight off the audio message. Called for
   * every audio chunk; the last value wins and goes stale on its own so a
   * closed socket stops claiming to know.
   */
  setRealtimeRatio(r: number | null | undefined) {
    if (typeof r !== 'number' || !Number.isFinite(r) || r < 0) return;
    this.rt = r;
    this.rtAt = performance.now();
  }

  /** Buffer health for one speaker, for a per-speaker readout. */
  speakerBuffer(elem: number): AudioBufferHealth | null {
    return this.healthOf(this.srcStats.get(speakerKey(elem)));
  }

  status(): AudioStatus {
    let sounding = false;
    for (const key of this.srcs.keys()) {
      if (this.levelOf(key) > QUIET) {
        sounding = true;
        break;
      }
    }
    const now = performance.now();
    const fresh = this.mix !== null && now - this.statsAt <= STATS_TTL_MS;
    return {
      speakers: this.speakers.size,
      listening: this.src !== null,
      sounding: sounding && !this.mute,
      needsGesture:
        this.srcs.size > 0 && (!this.activated || this.ctx?.state === 'suspended'),
      buf: fresh ? this.healthOf(this.mix) : null,
      ratio: now - this.rtAt <= RATIO_TTL_MS ? this.rt : null,
    };
  }

  // ------------------------------------------------------------------ feeds

  /** Server speaker chunk: `samples[k]` is the coil voltage at
   * `t0 + k * dts`. Ignored for an element that is not a live source. */
  pushSpeakerChunk(elem: number, t0: number, dts: number, samples: ArrayLike<number>) {
    this.chunk(speakerKey(elem), t0, dts, samples, this.gainOf(elem));
  }

  /** Server probe chunk: `samples[k]` is the value at `t0 + k * dts`. */
  pushChunk(pid: number, t0: number, dts: number, samples: ArrayLike<number>) {
    if (pid !== this.src) return;
    this.chunk(probeKey(pid), t0, dts, samples, 1);
  }

  /** Offline path: one sample per animation frame, batched. */
  pushPoint(pid: number, t: number, v: number) {
    if (pid !== this.src) return;
    const key = probeKey(pid);
    if (!this.srcs.has(key)) return;
    let pts = this.pts.get(key);
    if (!pts) {
      pts = [];
      this.pts.set(key, pts);
    }
    if (pts.length && t <= pts[pts.length - 2]!) pts.length = 0; // sim restarted
    pts.push(t, v);
    if (pts.length < OFFLINE_BATCH * 2) return;
    const n = pts.length / 2;
    const dts = (pts[pts.length - 2]! - pts[0]!) / (n - 1);
    const buf = new Float32Array(n);
    for (let k = 0; k < n; k++) buf[k] = pts[k * 2 + 1]!;
    pts.length = 0;
    if (dts > 0) this.send(key, dts, buf, 1);
  }

  // --------------------------------------------------------------- internals

  private add(key: string) {
    if (this.srcs.has(key)) return;
    this.srcs.set(key, { nextT: null, lvl: 0, lvlAt: 0 });
    // The context may have been parked by `idle()` after the last source
    // went away. A context that has run once resumes without a new gesture,
    // so wake it here or a fresh speaker would be silent AND would make the
    // HUD ask for a click the player already gave.
    this.wake();
    // A key can come back before the worklet finished fading out the last
    // one (stop-then-listen, delete-then-undo): start it from a clean ring
    // rather than splicing onto a stale read cursor.
    this.node?.port.postMessage({ t: 'reset', id: key });
  }

  /** Remove a source: the worklet fades it out and drops it, so a deleted
   * speaker or a stopped listen never clicks. */
  private remove(key: string) {
    this.srcs.delete(key);
    this.pts.delete(key);
    this.node?.port.postMessage({ t: 'drop', id: key });
  }

  /** Nothing left to play: park the context so the tab stops burning an
   * audio thread. `add` and any user gesture wake it again. */
  private idle() {
    if (this.srcs.size === 0) void this.ctx?.suspend();
  }

  /** Wake a parked context. A no-op while the browser is still waiting for a
   * user gesture — `status().needsGesture` is what asks for one. */
  private wake() {
    if (!this.activated) return;
    if (this.ctx?.state === 'suspended') {
      void this.ctx.resume().catch(() => {
        /* autoplay policy: stay suspended until a gesture */
      });
    }
  }

  /** Effective gain for a speaker: solo beats per-speaker mute. */
  private gainOf(elem: number): number {
    if (this.soloed !== null) return this.soloed === elem ? 1 : 0;
    return this.mutedSpeakers.has(elem) ? 0 : 1;
  }

  private pushGains() {
    for (const elem of this.speakers) {
      this.node?.port.postMessage({ t: 'gain', id: speakerKey(elem), gain: this.gainOf(elem) });
    }
  }

  private pushMaster() {
    this.node?.port.postMessage({ t: 'master', gain: this.vol, mute: this.mute });
  }

  /** Worklet telemetry, ~10 Hz. Cheap: a handful of numbers per source. */
  private onStats(m: { t?: string; mix?: MixStats; s?: Record<string, SrcStats> }) {
    if (m.t !== 'stats' || !m.mix) return;
    this.mix = m.mix;
    this.statsAt = performance.now();
    const srcs = m.s ?? {};
    this.srcStats.clear();
    for (const [id, st] of Object.entries(srcs)) {
      this.srcStats.set(id, st);
      // Counters only ever grow within one source's life; a key reused after
      // a delete restarts at zero, so a negative delta means "new source",
      // not "un-underrun".
      const prev = this.counted.get(id);
      const du = prev ? Math.max(0, st.underruns - prev.u) : st.underruns;
      const dms = prev ? Math.max(0, st.underrunMs - prev.ms) : st.underrunMs;
      const dd = prev ? Math.max(0, st.drops - prev.d) : st.drops;
      this.counted.set(id, { u: st.underruns, ms: st.underrunMs, d: st.drops });
      this.totUnder += du;
      this.totUnderMs += dms;
      this.totDrops += dd;
      if (du > 0) this.lastUnderAt = this.statsAt;
    }
    for (const id of [...this.counted.keys()]) {
      if (!(id in srcs)) this.counted.delete(id);
    }
  }

  /** Shape either telemetry record into the UI's view of buffer health. The
   * mix carries the SESSION totals (sources come and go); a single source
   * carries its own. */
  private healthOf(s: MixStats | SrcStats | null | undefined): AudioBufferHealth | null {
    if (!s) return null;
    const isMix = 'sources' in s;
    const recent = performance.now() - this.lastUnderAt <= UNDERRUN_RECENT_MS;
    return {
      ms: s.ms,
      minMs: s.minMs,
      maxMs: s.maxMs,
      // The worklet reports the depth it is actually holding; trust that over
      // the constant, so a stale bundle can never show a target it is not
      // servoing (they are the same number when everything is in sync).
      targetMs: isMix ? (s as MixStats).target : TARGET_BUF_S * 1000,
      underruns: isMix ? this.totUnder : s.underruns,
      underrunMs: isMix ? this.totUnderMs : s.underrunMs,
      drops: isMix ? this.totDrops : s.drops,
      concealedMs: s.concealedMs ?? 0,
      stalled: s.stalled,
      trim: s.trim,
      rate: s.rate ?? 1,
      recent,
      sources: isMix ? (s as MixStats).sources : 1,
      priming: isMix ? (s as MixStats).priming : (s as SrcStats).priming ? 1 : 0,
    };
  }

  private levelOf(key: string): number {
    const s = this.srcs.get(key);
    if (!s) return 0;
    return (performance.now() - s.lvlAt) / 1000 > LEVEL_TTL_S ? 0 : s.lvl;
  }

  private chunk(
    key: string,
    t0: number,
    dts: number,
    samples: ArrayLike<number>,
    gain: number,
  ) {
    const s = this.srcs.get(key);
    if (!s || samples.length === 0 || !(dts > 0)) return;
    // A jump in the sample clock is classified, not punished uniformly. A
    // small FORWARD jump is a lost message or two on a best-effort stream
    // (a lagged socket skips whole 33 ms chunks): the worklet bridges the
    // hole and keeps everything it already buffered — a blip, as net.ts
    // promises. Resetting here (the old behavior) threw away up to 200 ms
    // of real audio and then sat silent through a full re-prime, turning
    // each 33 ms loss into ~7x its length of silence. Only a BACKWARD jump
    // or a gap too big to bridge (room restart, long stall, a quarantined
    // solver's frozen clock) still re-primes that ONE source; the other
    // sources keep playing either way.
    let gapN = 0;
    if (s.nextT !== null) {
      const gap = t0 - s.nextT;
      if (Math.abs(gap) > dts * 4) {
        if (gap > 0 && gap <= GAP_MAX_S) gapN = Math.round(gap / dts);
        else this.node?.port.postMessage({ t: 'reset', id: key });
      }
    }
    s.nextT = t0 + samples.length * dts;
    const buf = new Float32Array(samples.length);
    for (let k = 0; k < samples.length; k++) buf[k] = samples[k] ?? 0;
    this.send(key, dts, buf, gain, gapN);
  }

  private send(key: string, dts: number, buf: Float32Array, gain: number, gapN = 0) {
    const s = this.srcs.get(key);
    if (!s) return;
    let peak = 0;
    for (const v of buf) {
      const a = v < 0 ? -v : v;
      if (a > peak) peak = a;
    }
    const l = Math.min(1, peak / REF_VOLTS);
    s.lvl = l > s.lvl ? l : s.lvl * 0.7 + l * 0.3;
    s.lvlAt = performance.now();
    if (!this.node) {
      // First chunk of the session: the worklet is still loading (or has not
      // been started). Boot it and drop this chunk — a few ms of silence.
      void this.boot();
      return;
    }
    // The server's realtime ratio rides along while fresh: the worklet
    // slaves its base playback rate to a sim dilated beyond the trim's
    // reach (see the worklet header). Offline feeds carry none and play at
    // nominal rate, exactly as before.
    const rt =
      performance.now() - this.rtAt <= RATIO_TTL_MS ? this.rt : undefined;
    this.node.port.postMessage(
      { t: 'chunk', id: key, dts, gain, rt, gap: gapN, s: buf },
      [buf.buffer],
    );
  }

  private boot(): Promise<void> {
    if (this.dead) return Promise.resolve();
    // Pre-gesture: stay silent and quiet. Samples arriving now are dropped
    // (a few ms), and the HUD asks for the click that gets us here again.
    if (!this.activated) return Promise.resolve();
    if (this.booting) {
      if (this.srcs.size > 0) this.wake();
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
          rateTauSec: RATE_TAU_S,
          rateMin: RATE_MIN,
        },
      });
      node.port.onmessage = (ev: MessageEvent) => {
        this.onStats(ev.data as { t?: string; mix?: MixStats; s?: Record<string, SrcStats> });
      };
      node.connect(ctx.destination);
      this.ctx = ctx;
      this.node = node;
      this.pushMaster();
      // Everything stopped while the module was loading: idle instead of
      // running an audio thread for silence.
      if (this.srcs.size === 0) await ctx.suspend();
      else await ctx.resume();
    })().catch((err: unknown) => {
      // No AudioWorklet (or the page refuses to make noise): stay silent
      // forever rather than retrying on every chunk.
      console.warn('audio unavailable', err);
      this.dead = true;
      this.node = null;
      this.srcs.clear();
      this.src = null;
      this.mix = null;
      this.srcStats.clear();
    });
    return this.booting;
  }
}
