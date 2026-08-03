// THE SAMPLER. The only place in this application where a camera frame
// exists, and it is a worker, so the render loop never sees this feature at
// all.
//
// WHAT IT DOES, PER FRAME: one `VideoFrame.copyTo(fixedBuffer)` of the
// decoded planes, then a mean over the LUMA bytes inside each aperture
// rectangle. Luma is exactly what a light sensor wants — no RGB conversion,
// no gamma argument, no canvas.
//
// WHAT IT NEVER DOES, and these are load-bearing:
//
//   * `drawImage` + `getImageData` on the fast path. That readback is a fixed
//     GPU->CPU sync of ~0.6 ms and it is nearly independent of region size,
//     so per-sensor readback would cost the entire frame budget for eight
//     sensors. It exists here only as the fallback for browsers with no
//     `MediaStreamTrackProcessor`, and even then it is ONE readback serving
//     every aperture.
//   * `copyTo` with the `rect` option. It returns a correct layout and a
//     buffer of zeros in shipping Chrome — it fails SILENTLY. Full plane,
//     then reduce in JS.
//   * allocate per frame. The plane buffer and the two id/value arrays are
//     allocated once and reused, which is also what makes "no frame is ever
//     retained" true by construction rather than by intention.
//   * keep, encode, upload or post a frame. The only thing that leaves this
//     worker is one integer per aperture.

interface Aperture {
  id: number;
  u0: number;
  v0: number;
  u1: number;
  v1: number;
}

// This file runs as a module WORKER. The app's tsconfig ships the DOM lib,
// not WebWorker, so the worker global is narrowed by hand rather than by
// pulling a second lib in for one file.
const wk = self as unknown as {
  postMessage(m: unknown): void;
  onmessage: ((ev: MessageEvent) => void) | null;
  MediaStreamTrackProcessor?: new (o: { track: MediaStreamTrack }) => {
    readable: ReadableStream<VideoFrameLike>;
  };
  OffscreenCanvas?: new (w: number, h: number) => OffscreenCanvas;
  ImageCapture?: new (t: MediaStreamTrack) => { grabFrame(): Promise<ImageBitmap> };
};

/** The slice of `VideoFrame` this file uses. Declared rather than imported:
 *  WebCodecs is not in the DOM lib this project builds against. */
interface VideoFrameLike {
  codedWidth: number;
  codedHeight: number;
  format: string | null;
  allocationSize(): number;
  copyTo(dst: Uint8Array): Promise<{ offset: number; stride: number }[]>;
  close(): void;
}

let apertures: Aperture[] = [];
/** Width/height of the LAYER rectangle in world units. The letterbox needs
 *  it and only the main thread knows it, so it rides the aperture message. */
let layerAspect = 4 / 3;
let running = true;
/** The track this worker owns (a clone of the player's). Only this side can
 *  stop it, which is why `stop` is a message and not a `terminate`. */
let held: MediaStreamTrack | null = null;

/** Reused buffers. Grown only when the frame's size grows. */
let plane: Uint8Array = new Uint8Array(0);
let outIds: number[] = [];
let outVs: number[] = [];

const fail = (detail: string) => wk.postMessage({ t: 'err', detail });

/**
 * Mean luma over one aperture, in LAYER-normalized coordinates.
 *
 * The letterbox lives here because this is the only side that knows the
 * frame's aspect: the source is fitted inside the layer rectangle preserving
 * aspect, so an aperture over a bar reads dark and one straddling the edge
 * reads only the covered part. "Move the sensor closer to the light" is
 * literally moving it in world coordinates.
 */
function meanLuma(
  a: Aperture,
  w: number,
  h: number,
  stride: number,
  offset: number,
  layerAspect: number,
): number {
  const srcAspect = w / h;
  // Video extent inside the layer, in layer-normalized units.
  let uSpan = 1;
  let vSpan = 1;
  if (srcAspect > layerAspect) vSpan = layerAspect / srcAspect;
  else uSpan = srcAspect / layerAspect;
  const u0 = (1 - uSpan) / 2;
  const v0 = (1 - vSpan) / 2;
  // Aperture -> frame pixels, clamped to the image.
  const px0 = Math.max(0, Math.min(w - 1, Math.floor(((a.u0 - u0) / uSpan) * w)));
  const px1 = Math.max(0, Math.min(w, Math.ceil(((a.u1 - u0) / uSpan) * w)));
  const py0 = Math.max(0, Math.min(h - 1, Math.floor(((a.v0 - v0) / vSpan) * h)));
  const py1 = Math.max(0, Math.min(h, Math.ceil(((a.v1 - v0) / vSpan) * h)));
  if (px1 <= px0 || py1 <= py0) return 0;
  // Cap the work: a huge aperture on a 4K frame is subsampled, never scanned
  // whole. 32x32 taps is far more than a mean needs and is a hard ceiling on
  // this function's cost whatever a player draws.
  const stepX = Math.max(1, Math.floor((px1 - px0) / 32));
  const stepY = Math.max(1, Math.floor((py1 - py0) / 32));
  let sum = 0;
  let n = 0;
  for (let y = py0; y < py1; y += stepY) {
    const row = offset + y * stride;
    for (let x = px0; x < px1; x += stepX) {
      sum += plane[row + x]!;
      n++;
    }
  }
  return n === 0 ? 0 : sum / n / 255;
}

function emit(w: number, h: number, stride: number, offset: number, t0: number) {
  outIds.length = 0;
  outVs.length = 0;
  for (const a of apertures) {
    outIds.push(a.id);
    outVs.push(meanLuma(a, w, h, stride, offset, layerAspect));
  }
  if (outIds.length === 0) return;
  wk.postMessage({ t: 's', ids: outIds, vs: outVs, ms: performance.now() - t0 });
}

/** The fast path: decoded frames, no canvas, no GPU round trip. */
async function runProcessor(track: MediaStreamTrack): Promise<boolean> {
  const Proc = wk.MediaStreamTrackProcessor;
  if (!Proc) return false;
  await runReadable(new Proc({ track }).readable);
  return true;
}

/** The loop itself, over whichever way the frames arrived. */
async function runReadable(readable: ReadableStream<VideoFrameLike>) {
  const reader = readable.getReader();
  while (running) {
    const { value: frame, done } = await reader.read();
    if (done || !frame) break;
    const t0 = performance.now();
    try {
      const w = frame.codedWidth;
      const h = frame.codedHeight;
      const size = frame.allocationSize();
      if (plane.length < size) plane = new Uint8Array(size);
      const layout = await frame.copyTo(plane);
      const fmt = String(frame.format ?? '');
      if (fmt.startsWith('I420') || fmt.startsWith('NV12') || fmt.startsWith('I444')) {
        // Plane 0 IS the luma plane — the most honest photocell reading
        // available, since it never went through the browser's YUV->RGB
        // expansion.
        const l0 = layout[0]!;
        emit(w, h, l0.stride, l0.offset, t0);
      } else {
        // Packed RGBA/BGRA: fold to luma in place over the aperture scan by
        // treating the green byte as the luma proxy (index 1 in both
        // orderings), which costs nothing and is within a few percent of
        // Rec.601 for a mean.
        const l0 = layout[0]!;
        for (let i = l0.offset + 1, o = 0; o < w * h; i += 4, o++) plane[o] = plane[i]!;
        emit(w, h, w, 0, t0);
      }
    } catch (err) {
      fail(String(err));
      frame.close();
      return;
    }
    // Closed IMMEDIATELY, every frame, on every path. A leaked VideoFrame
    // holds a decoder buffer and stalls the camera within a few frames.
    frame.close();
  }
  await reader.cancel().catch(() => undefined);
}

/** Fallback for browsers with no `MediaStreamTrackProcessor`: ONE small
 *  canvas readback per frame, serving every aperture. ~0.6 ms, and it is
 *  still off the render thread. */
async function runCanvas(track: MediaStreamTrack) {
  if (!wk.OffscreenCanvas || !wk.ImageCapture) {
    fail('no frame source in this browser');
    return;
  }
  const cap = new wk.ImageCapture(track);
  const cv = new wk.OffscreenCanvas(64, 48);
  const c2 = cv.getContext('2d', { willReadFrequently: true }) as OffscreenCanvasRenderingContext2D;
  while (running) {
    const t0 = performance.now();
    let bmp: ImageBitmap;
    try {
      bmp = await cap.grabFrame();
    } catch (err) {
      fail(String(err));
      return;
    }
    c2.drawImage(bmp, 0, 0, 64, 48);
    bmp.close();
    const d = c2.getImageData(0, 0, 64, 48).data;
    if (plane.length < 64 * 48) plane = new Uint8Array(64 * 48);
    for (let i = 0, o = 0; o < 64 * 48; i += 4, o++) plane[o] = d[i + 1]!;
    emit(64, 48, 64, 0, t0);
    await new Promise((r) => setTimeout(r, 33));
  }
}

wk.onmessage = (ev: MessageEvent) => {
  const m = ev.data as {
    t: string;
    track?: MediaStreamTrack;
    list?: Aperture[];
    aspect?: number;
    readable?: ReadableStream<VideoFrameLike>;
  };
  if (m.t === 'apertures') {
    apertures = m.list ?? [];
    if (typeof m.aspect === 'number' && m.aspect > 0) layerAspect = m.aspect;
  } else if (m.t === 'track' && m.track) {
    held = m.track;
    const track = m.track;
    void (async () => {
      if (!(await runProcessor(track))) await runCanvas(track);
    })();
  } else if (m.t === 'readable' && m.readable) {
    // The transfer fallback: the frames arrive as a stream instead of a
    // track. Identical from here on.
    void runReadable(m.readable);
  } else if (m.t === 'stop') {
    running = false;
    // The clone this worker owns is the LAST thing holding the camera open.
    // Stopping it is what puts the hardware indicator out.
    held?.stop();
    held = null;
  }
};
