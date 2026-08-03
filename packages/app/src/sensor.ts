// EXTERNAL INPUTS: the camera itself.
//
// THE ONE PROMISE. No pixel and no audio sample ever leaves this file's
// worker. The only thing that crosses the socket is
// `{t:'sensor', s:[[element_id, q]]}` with `q` a u16 — twelve bytes per moved
// sensor per tick. There is no encoder here, no upload, no recorder, and no
// code path that could serialize a frame:
//
//   * the raw track is TRANSFERRED into the sampler worker and the main
//     thread keeps no reference to it;
//   * the worker reads the decoded LUMA plane into a fixed `Uint8Array`
//     allocated once and reduces each aperture to one mean — so there is no
//     ring buffer of frames to leak, because there is no ring buffer;
//   * `wirecheck` asserts the outbound shape AND statically pins the count of
//     `RTCPeerConnection|MediaRecorder|getDisplayMedia|toDataURL|toBlob` in
//     `src/` at zero.
//
// CONSENT. `start()` is only ever called from the click that claims a layer —
// never on page load, never on room join, never restored from localStorage,
// and never silently re-opened on reload even where the browser would allow
// it. That is commit ecc8cb6's lesson ("never build the AudioContext before a
// user gesture") pointed the other way.

import type { Aperture } from './layer';

// ------------------------------------------------------------ the camera

/** What the sampler reports for one part. Nothing else ever comes back. */
export interface Reading {
  id: number;
  /** 0..1. Mean luma of the patch the part covers. */
  v: number;
}

export type SourceState = 'off' | 'starting' | 'live' | 'denied' | 'unsupported' | 'ended';

export interface CameraStatus {
  state: SourceState;
  /** Set when the camera refused to hold its exposure. An AGC actively
   *  cancels the change a photocell is trying to read — a cell fighting one
   *  is a dishonest instrument, and this project does not ship those
   *  silently. */
  autoExposure: boolean;
  /** Frames the sampler has actually reduced. Zero while live means the
   *  track is delivering nothing. */
  frames: number;
  /** Mean cost of one sampled frame, in ms, over the last 60. */
  msPerFrame: number;
  detail: string;
}

/**
 * One player's camera, sampled in a worker.
 *
 * The consent flow is the whole design of this class: `start()` is only ever
 * called from the click that claims a layer — never on page load, never on
 * room join, never restored from localStorage, and never re-opened silently
 * on reload even though the browser would allow it. That is commit ecc8cb6's
 * lesson ("never build the AudioContext before a user gesture") pointed the
 * other way.
 */
export class CameraSource {
  private worker: Worker | null = null;
  private stream: MediaStream | null = null;
  private video: HTMLVideoElement | null = null;
  private readings = new Map<number, number>();
  private status: CameraStatus = {
    state: 'off',
    autoExposure: false,
    frames: 0,
    msPerFrame: 0,
    detail: '',
  };
  /** Reused across every message: the sampler must not allocate per frame. */
  private apertures: Aperture[] = [];
  /** The in-flight `start()`. A second click must JOIN it, not be told "yes"
   *  and handed nothing — see `start()`. */
  private pending: Promise<boolean> | null = null;
  /** When the track went live, and when the sampler last saw a frame. The
   *  two together are the only evidence that a "live" camera is real — see
   *  `silentMs()`. */
  private liveAt = 0;
  private lastFrameAt = 0;

  constructor(private onChange: () => void) {}

  getStatus(): CameraStatus {
    return this.status;
  }

  /** This client's own preview element, for drawing the layer. Null on every
   *  other client in the room — there is no message that could carry it. */
  previewEl(): HTMLVideoElement | null {
    return this.status.state === 'live' ? this.video : null;
  }

  /** Latest reading for a part, or null when the sampler has nothing for it
   *  (part is not over the layer, or the camera is off). */
  read(id: number): number | null {
    const v = this.readings.get(id);
    return v === undefined ? null : v;
  }

  isLive(): boolean {
    return this.status.state === 'live';
  }

  /**
   * How long the sampler has gone WITHOUT A FRAME, in ms. Zero when the
   * camera is off.
   *
   * A track can be `readyState: "live"` and deliver nothing at all, forever —
   * a camera another app has grabbed, a virtual device with no producer, a
   * headless Chrome whose fake device was killed by `--disable-gpu`. The
   * status already carried `frames` with a comment saying "zero while live
   * means the track is delivering nothing", and nothing ever read it: the
   * plate said LIVE and the chip said LIVE over a black rectangle, which is
   * worse than saying nothing, because it is a claim that is false.
   */
  silentMs(): number {
    if (this.status.state !== 'live') return 0;
    const since = this.lastFrameAt || this.liveAt;
    return since === 0 ? 0 : performance.now() - since;
  }

  /**
   * MUST be called from a user gesture. Returns true once the track is up.
   *
   * A second call while the first is still waiting on the browser's
   * permission prompt JOINS that call and settles with it. It used to return
   * `true` on sight of `state === 'starting'`, which meant a player who
   * ignored the prompt once had a plate that reported success and produced
   * no camera on every click thereafter — the silent failure this whole file
   * exists to avoid.
   */
  start(): Promise<boolean> {
    if (this.status.state === 'live') return Promise.resolve(true);
    if (this.pending) return this.pending;
    const p = this.begin();
    this.pending = p;
    void p.finally(() => {
      if (this.pending === p) this.pending = null;
    });
    return p;
  }

  private async begin(): Promise<boolean> {
    this.set({ state: 'starting', detail: 'asking for the camera' });
    // SECURE CONTEXT. This is not a bug and cannot be fixed in code — a
    // browser will not hand out a camera to a plain-http origin, and it does
    // not even define `navigator.mediaDevices` there. Nothing about that is
    // obvious to a player looking at a rectangle that will not turn on, so
    // the message names the origin AND the two origins that do work.
    if (!window.isSecureContext) {
      this.set({
        state: 'unsupported',
        detail:
          `${location.protocol}//${location.host} is not a secure origin — ` +
          'browsers only hand out a camera over https:// or on localhost',
      });
      return false;
    }
    if (!navigator.mediaDevices?.getUserMedia) {
      this.set({ state: 'unsupported', detail: 'this browser has no camera API' });
      return false;
    }
    let stream: MediaStream;
    try {
      // Exposure and white balance pinned where the browser allows it. A
      // camera whose AGC keeps hunting will actively undo what the player is
      // doing with their hand, so we ask, and we SAY SO when refused.
      stream = await navigator.mediaDevices.getUserMedia({
        video: {
          width: { ideal: 640 },
          height: { ideal: 480 },
          frameRate: { ideal: 30 },
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          advanced: [{ exposureMode: 'manual' }, { whiteBalanceMode: 'manual' }] as any,
        },
        audio: false,
      });
    } catch (err) {
      // The NAME is the actionable half (`NotAllowedError` = you or the OS
      // said no, `NotFoundError` = there is no camera on this machine), so it
      // goes in the message a player sees rather than only in the console.
      const e = err as Error;
      this.set({
        state: 'denied',
        detail: e?.name ? `${e.name}${e.message ? `: ${e.message}` : ''}` : String(err),
      });
      return false;
    }
    this.stream = stream;
    const track = stream.getVideoTracks()[0];
    if (!track) {
      this.stopTracks();
      this.set({ state: 'denied', detail: 'no video track' });
      return false;
    }
    // Did the camera actually hold exposure?
    let auto = true;
    try {
      const s = track.getSettings() as Record<string, unknown>;
      auto = s.exposureMode !== 'manual';
    } catch {
      auto = true;
    }

    // The player ALWAYS sees what the camera sees: the only place the video
    // renders is the layer they placed in the world, and this element feeds
    // exactly that.
    const video = document.createElement('video');
    video.playsInline = true;
    video.muted = true;
    video.srcObject = stream;
    await video.play().catch(() => undefined);
    this.video = video;

    try {
      this.worker = new Worker(new URL('./sensor-worker.ts', import.meta.url), {
        type: 'module',
      });
    } catch (err) {
      this.stopTracks();
      this.set({ state: 'unsupported', detail: `worker: ${String(err)}` });
      return false;
    }
    this.worker.onmessage = (ev: MessageEvent) => this.onWorker(ev.data);

    // A CLONE of the track is TRANSFERRED into the worker, and the original
    // stays here feeding nothing but the on-canvas preview.
    //
    // Two reasons it is a clone rather than the track itself. A transfer
    // DETACHES the track — Chrome then refuses to keep previewing it, and
    // the player would be running a camera they cannot see, which is exactly
    // the configuration this feature must never have. And a
    // `MediaStreamTrackProcessor` consumes its track exclusively, so the
    // preview and the sampler genuinely need two.
    //
    // After the transfer the main thread holds no reference the worker's
    // frames could be read through, which is the property the privacy claim
    // rests on. The worker stops its own clone on `stop`.
    const forWorker = track.clone();
    try {
      this.worker.postMessage({ t: 'track', track: forWorker }, [
        forWorker as unknown as Transferable,
      ]);
    } catch (err) {
      // Browsers where a track is not transferable: build the processor here
      // and transfer its READABLE instead, which is transferable everywhere
      // WebCodecs is. Same worker, same sampling, same egress.
      try {
        const Proc = (
          window as unknown as {
            MediaStreamTrackProcessor?: new (o: { track: MediaStreamTrack }) => {
              readable: ReadableStream;
            };
          }
        ).MediaStreamTrackProcessor;
        if (!Proc) throw err;
        const readable = new Proc({ track: forWorker }).readable;
        this.worker.postMessage({ t: 'readable', readable }, [readable as unknown as Transferable]);
      } catch (err2) {
        forWorker.stop();
        this.stop();
        this.set({ state: 'unsupported', detail: `no frame transport: ${String(err2)}` });
        return false;
      }
    }
    this.liveAt = performance.now();
    this.lastFrameAt = 0;
    this.set({
      state: 'live',
      autoExposure: auto,
      frames: 0,
      detail: auto ? 'camera would not hold exposure' : '',
    });
    return true;
  }

  /** Push the current aperture set. Called when the document or the layer
   *  moves — cheap, and it is what makes "drag the part onto the light" work
   *  with no binding op anywhere. */
  setApertures(list: Aperture[], layerAspect: number) {
    this.apertures.length = 0;
    for (const a of list) this.apertures.push(a);
    // A part that just left the layer must stop reporting immediately.
    for (const id of [...this.readings.keys()]) {
      if (!list.some((a) => a.id === id)) this.readings.delete(id);
    }
    this.worker?.postMessage({ t: 'apertures', list, aspect: layerAspect });
  }

  /**
   * STOP. `track.stop()`, never `enabled = false`: only stop() extinguishes
   * the hardware indicator and clears the browser's own chrome. Every bound
   * sensor then falls to dark within one tick — visibly, for everyone in the
   * room, so switching the camera off is legible to the other players and
   * not just to you.
   */
  stop() {
    this.pending = null;
    const w = this.worker;
    this.worker = null;
    if (w) {
      // Ask first, then terminate: the worker owns a CLONE of the track and
      // only it can stop that clone. Killing the thread with the clone still
      // running would leave the hardware indicator lit with nothing reading
      // it — the single worst outcome this feature could have.
      w.postMessage({ t: 'stop' });
      setTimeout(() => w.terminate(), 250);
    }
    this.stopTracks();
    if (this.video) {
      this.video.srcObject = null;
      this.video = null;
    }
    this.readings.clear();
    this.set({ state: 'off', autoExposure: false, frames: 0, msPerFrame: 0, detail: '' });
  }

  private stopTracks() {
    for (const t of this.stream?.getTracks() ?? []) t.stop();
    this.stream = null;
  }

  private onWorker(m: { t: string; ids?: number[]; vs?: number[]; ms?: number; detail?: string }) {
    if (m.t === 's' && m.ids && m.vs) {
      for (let i = 0; i < m.ids.length; i++) this.readings.set(m.ids[i]!, m.vs[i]!);
      this.status.frames++;
      // Liveness, counted even when the frame drove nothing: a camera with no
      // photocell under it yet is still a working camera, and must not be
      // reported as a dead one.
      this.lastFrameAt = performance.now();
      if (typeof m.ms === 'number') {
        // EMA over ~60 frames, so the HUD reports a real cost rather than
        // one lucky frame.
        this.status.msPerFrame = this.status.msPerFrame * 0.98 + m.ms * 0.02;
      }
    } else if (m.t === 'err') {
      this.set({ state: 'ended', detail: m.detail ?? 'sampler stopped' });
      this.stopTracks();
    }
  }

  private set(p: Partial<CameraStatus>) {
    this.status = { ...this.status, ...p };
    this.onChange();
  }
}
