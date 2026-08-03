// EXTERNAL INPUTS, the part with no hardware in it: a rectangle of the world
// that a real-world source is pointed at, and the geometry that decides which
// parts read what is under them.
//
// Split from `sensor.ts` deliberately. Everything here is pure — types,
// rectangle maths, a canvas draw — so `net.ts` (and therefore the headless
// `wirecheck`) can depend on the LAYER without dragging in `getUserMedia`, a
// worker URL or any other browser capability. The camera lives next door.
//
// THE ONE IDEA. A camera is not an input binding, it is a LAYER IN THE WORLD.
// You draw a rectangle, you point your camera at it, and a photocell placed
// over it reads the light in the patch it covers. There is no binding dialog
// anywhere in this feature, and there must never be one: which part reads
// which region is re-derived from element geometry every frame, exactly the
// way `panelMembers` and `scopeOwner` already work, so dragging a part off
// the layer unbinds it with no op at all.
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
// THE RATE. The server retires one write per part per tick and the tick is
// 30 Hz, so every external input — camera, microphone, gamepad — is a 30 Hz
// signal with 15 Hz of bandwidth, whatever the hardware does. You can make
// loudness dim a lamp; you cannot whistle a tone into a circuit. The UI says
// so rather than letting players discover it.
//
// The microphone and gamepad providers are designed and measured but not
// built: `docs/design_external-inputs.md`.

import type { ElementSpec, Point } from './circuit';
import type { Camera } from './render';
import { roundRectPath } from './panel';

// ------------------------------------------------------------ shared state

/** A sensor layer: room state, exactly like a `Panel`. Only the rectangle
 *  and the name are stored — never a device id, a label, a resolution or a
 *  capability blob. Whose eye is behind it is per-session (`claims`). */
export interface Layer {
  lid: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  name: string;
}

export type LayerOp =
  | { t: 'add'; x0: number; y0: number; x1: number; y1: number; name?: string }
  | { t: 'remove'; lid: number }
  | { t: 'rect'; lid: number; x0: number; y0: number; x1: number; y1: number }
  | { t: 'rename'; lid: number; name: string };

/** Mirrors the server's `MIN_LAYER_SPAN`: a sensor layer has to be big
 *  enough to aim a part at a REGION of it. */
export const MIN_LAYER_SPAN = 4;

/** Mirrors the server's `MAX_LAYERS`. */
export const MAX_LAYERS = 32;

export function normLayerRect(a: Point, b: Point): [number, number, number, number] | null {
  const x0 = Math.min(a[0], b[0]);
  const x1 = Math.max(a[0], b[0]);
  const y0 = Math.min(a[1], b[1]);
  const y1 = Math.max(a[1], b[1]);
  if (![x0, y0, x1, y1].every(Number.isFinite)) return null;
  if (x1 - x0 < MIN_LAYER_SPAN || y1 - y0 < MIN_LAYER_SPAN) return null;
  return [x0, y0, x1, y1];
}

/** Offline mirror of the server's `apply_layer_op` (same validation). */
export function applyLayerOp(layers: Layer[], op: LayerOp, allocLid: () => number): Layer[] {
  if (op.t === 'add') {
    const r = normLayerRect([op.x0, op.y0], [op.x1, op.y1]);
    if (!r || layers.length >= MAX_LAYERS) return layers;
    const lid = allocLid();
    return [
      ...layers,
      { lid, x0: r[0], y0: r[1], x1: r[2], y1: r[3], name: op.name?.trim() || `CAMERA ${lid}` },
    ];
  }
  if (op.t === 'remove') return layers.filter((l) => l.lid !== op.lid);
  const l = layers.find((x) => x.lid === op.lid);
  if (!l) return layers;
  if (op.t === 'rect') {
    const r = normLayerRect([op.x0, op.y0], [op.x1, op.y1]);
    if (r) [l.x0, l.y0, l.x1, l.y1] = r;
  } else if (op.name.trim()) {
    l.name = op.name.trim();
  }
  return layers;
}

/** A part's footprint in world units: its pin bbox, never smaller than one
 *  grid unit so a two-pin part laid flat still has an aperture with area. */
export function apertureOf(e: ElementSpec): [number, number, number, number] {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const [x, y] of e.pins) {
    if (x < x0) x0 = x;
    if (y < y0) y0 = y;
    if (x > x1) x1 = x;
    if (y > y1) y1 = y;
  }
  if (!Number.isFinite(x0)) return [0, 0, 0, 0];
  const padX = Math.max(0, (1 - (x1 - x0)) / 2);
  const padY = Math.max(0, (1 - (y1 - y0)) / 2);
  return [x0 - padX, y0 - padY, x1 + padX, y1 + padY];
}

/** The layer a part reads: the SMALLEST layer containing its aperture
 *  centre, lowest lid breaking a tie. MUST match the server's `layer_under`
 *  exactly — the server re-derives the same answer before honouring a
 *  reading, so a client that disagrees simply gets its readings dropped. */
export function layerUnder(layers: Layer[], e: ElementSpec): Layer | null {
  if (e.pins.length === 0) return null;
  let cx = 0;
  let cy = 0;
  for (const [x, y] of e.pins) {
    cx += x;
    cy += y;
  }
  cx /= e.pins.length;
  cy /= e.pins.length;
  let best: Layer | null = null;
  let bestArea = Infinity;
  for (const l of layers) {
    if (cx < l.x0 || cx > l.x1 || cy < l.y0 || cy > l.y1) continue;
    const area = (l.x1 - l.x0) * (l.y1 - l.y0);
    if (area < bestArea || (area === bestArea && best !== null && l.lid < best.lid)) {
      best = l;
      bestArea = area;
    }
  }
  return best;
}

/** One part's window onto its layer, in LAYER-normalized coordinates
 *  (0..1 across the rectangle). The worker turns this into source pixels —
 *  it is the only side that knows the frame's size or aspect. */
export interface Aperture {
  id: number;
  u0: number;
  v0: number;
  u1: number;
  v1: number;
}

/** Every photocell sitting on `layer`, as apertures. Pure geometry, computed
 *  from the live document: no stored binding exists to go stale.
 *
 *  `all` is the WHOLE layer list, not a courtesy: the server decides which
 *  layer owns a part with `layer_under` over every layer in the room, and
 *  drops a reading whose layer is not the one the sender claimed. Testing
 *  membership against `layer` alone made the client disagree the moment two
 *  layers overlapped — it sampled and sent, the server discarded every
 *  frame, and nothing anywhere said so. Same rule, same answer. */
export function aperturesOn(
  layer: Layer,
  all: Layer[],
  elements: ElementSpec[],
  out: Aperture[],
): Aperture[] {
  out.length = 0;
  const w = layer.x1 - layer.x0;
  const h = layer.y1 - layer.y0;
  if (w <= 0 || h <= 0) return out;
  for (const e of elements) {
    if (e.kind.t !== 'Photocell') continue;
    if (layerUnder(all, e)?.lid !== layer.lid) continue;
    const [ax0, ay0, ax1, ay1] = apertureOf(e);
    out.push({
      id: e.id,
      u0: Math.max(0, Math.min(1, (ax0 - layer.x0) / w)),
      v0: Math.max(0, Math.min(1, (ay0 - layer.y0) / h)),
      u1: Math.max(0, Math.min(1, (ax1 - layer.x0) / w)),
      v1: Math.max(0, Math.min(1, (ay1 - layer.y0) / h)),
    });
  }
  return out;
}

// ---------------------------------------------------------------- rendering

const LABEL_FONT = '12px ui-monospace, SFMono-Regular, monospace';

/** The name plate's geometry, in screen pixels. */
const PLATE_PAD = 6;
const PLATE_H = 20;
const PLATE_MAX_W = 260;

/**
 * THE PLATE, computed ONCE for both the drawing and the hit test.
 *
 * It used to be pinned to the layer's own top-left corner, which meant that
 * zooming in far enough to aim a camera scrolled the only clickable thing in
 * the feature off the top of the screen: the layer filled the viewport, the
 * player clicked it, and nothing happened, forever. So the plate is STICKY —
 * it rides the visible part of the rectangle, the way a table header does, and
 * is reachable whenever any of the layer is.
 *
 * Returns null when the visible sliver is too small to hold a legible plate;
 * callers must then say something rather than leave a dead target.
 */
export function layerPlateRect(
  cam: Camera,
  l: Layer,
  viewW: number,
  viewH: number,
  viewTop = 0,
): [number, number, number, number] | null {
  const X = cam.ox + l.x0 * cam.scale;
  const Y = cam.oy + l.y0 * cam.scale;
  const X1 = cam.ox + l.x1 * cam.scale;
  const Y1 = cam.oy + l.y1 * cam.scale;
  // The layer's box clipped to the screen: what the player can actually see.
  // `viewTop` keeps it out from under the HUD rail — a button hidden behind
  // an overlay is the same dead button as one scrolled off the top.
  const vx0 = Math.max(X, 0);
  const vy0 = Math.max(Y, viewTop);
  const vx1 = Math.min(X1, viewW);
  const vy1 = Math.min(Y1, viewH);
  if (vx1 - vx0 < 48 || vy1 - vy0 < PLATE_H + PLATE_PAD) return null;
  const w = Math.min(PLATE_MAX_W, vx1 - vx0 - 2 * PLATE_PAD);
  if (w < 40) return null;
  return [vx0 + PLATE_PAD, vy0 + PLATE_PAD, w, PLATE_H];
}

/** Draw the sensor layers under the schematic: a dashed frame, the video (if
 *  this client is the one driving it) letterboxed inside, and a name plate
 *  that always says who is behind it. `preview` is the driving client's own
 *  video element — nobody else's browser has one, ever. */
export function drawSensorLayers(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  layers: Layer[],
  claims: Map<number, number>,
  me: number,
  preview: HTMLVideoElement | null,
  hotLid: number | null,
  viewW: number,
  viewH: number,
  viewTop = 0,
  /** MY camera is up but no frame has arrived for a while. A track can sit
   *  at `readyState: "live"` forever and send nothing, and a plate reading
   *  "● LIVE" over a black rectangle is a false statement, not a missing
   *  one. Only ever true for the layer this client drives. */
  stalled = false,
) {
  if (layers.length === 0) return;
  ctx.save();
  ctx.font = LABEL_FONT;
  ctx.textBaseline = 'middle';
  for (const l of layers) {
    const X = cam.ox + l.x0 * cam.scale;
    const Y = cam.oy + l.y0 * cam.scale;
    const W = (l.x1 - l.x0) * cam.scale;
    const H = (l.y1 - l.y0) * cam.scale;
    const driver = claims.get(l.lid);
    const mine = driver === me;
    const live = mine && preview !== null && preview.readyState >= 2;

    ctx.save();
    roundRectPath(ctx, X, Y, W, H, Math.min(14, cam.scale * 0.4));
    ctx.clip();
    // The letterbox bars are DRAWN, so the mapping from world to pixels is
    // something a player can see rather than infer.
    ctx.fillStyle = '#0a1013';
    ctx.fillRect(X, Y, W, H);
    if (live && preview) {
      const src = preview.videoWidth / preview.videoHeight;
      const dst = W / H;
      const dw = src > dst ? W : H * src;
      const dh = src > dst ? W / src : H;
      ctx.drawImage(preview, X + (W - dw) / 2, Y + (H - dh) / 2, dw, dh);
    }
    ctx.restore();

    roundRectPath(ctx, X, Y, W, H, Math.min(14, cam.scale * 0.4));
    ctx.setLineDash(live ? [] : [6, 6]);
    ctx.lineWidth = live ? 2.5 : 1.4;
    // A RED ring while a real camera is live, and it is not subtle: the
    // browser's own indicator is necessary and nowhere near sufficient when
    // the game is the thing asking for the camera.
    ctx.strokeStyle = live ? '#ff5a5a' : l.lid === hotLid ? '#8ee7ff' : '#4a8ea8';
    ctx.stroke();
    ctx.setLineDash([]);

    // The plate is the button. It is drawn from the SAME rectangle the hit
    // test uses, so "I clicked it and nothing happened" cannot come back.
    const plate = layerPlateRect(cam, l, viewW, viewH, viewTop);
    if (plate) {
      const [px, py, pw, ph] = plate;
      const label = live
        ? stalled
          ? `● NO FRAMES — ${l.name} — click to stop`
          : `● LIVE — ${l.name} — click to stop`
        : driver !== undefined && !mine
          ? `${l.name} — driven by player ${driver}`
          : `▶ ${l.name} — click for camera`;
      const hot = l.lid === hotLid;
      ctx.fillStyle = live ? '#3a1216' : hot ? '#1d3d4a' : '#12222a';
      roundRectPath(ctx, px, py, pw, ph, 5);
      ctx.fill();
      ctx.strokeStyle = live ? '#ff5a5a' : hot ? '#8ee7ff' : '#3d6a7c';
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.fillStyle = live ? '#ff9c9c' : '#9fc4d4';
      ctx.save();
      roundRectPath(ctx, px, py, pw, ph, 5);
      ctx.clip();
      ctx.fillText(label, px + 7, py + ph / 2 + 1);
      ctx.restore();
    }
    // Said out loud, on the layer, because a player placing a camera-driven
    // part in a shared room will and should ask.
    if (H > 70 && W > 240 && !live) {
      ctx.fillStyle = '#5d7883';
      ctx.fillText('you see the reading, never the picture', X + 13, Y + H - 14);
    }
  }
  ctx.restore();
}

/** The in-progress layer while the tool is dragging. */
export function drawLayerGhost(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  a: Point,
  b: Point,
  ok: boolean,
) {
  const X = cam.ox + Math.min(a[0], b[0]) * cam.scale;
  const Y = cam.oy + Math.min(a[1], b[1]) * cam.scale;
  const W = Math.abs(b[0] - a[0]) * cam.scale;
  const H = Math.abs(b[1] - a[1]) * cam.scale;
  ctx.save();
  roundRectPath(ctx, X, Y, W, H, 10);
  ctx.fillStyle = ok ? '#ffd67a12' : '#ff5a5a10';
  ctx.fill();
  ctx.setLineDash([4, 5]);
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = ok ? '#ffd67a' : '#7c5b5b';
  ctx.stroke();
  ctx.restore();
}

/** Hit-test a layer's name plate — the one control the feature has. Shares
 *  `layerPlateRect` with the draw, so the button is exactly where it looks. */
export function layerPlateAt(
  cam: Camera,
  layers: Layer[],
  x: number,
  y: number,
  viewW: number,
  viewH: number,
  viewTop = 0,
): Layer | null {
  for (let k = layers.length - 1; k >= 0; k--) {
    const l = layers[k]!;
    const p = layerPlateRect(cam, l, viewW, viewH, viewTop);
    if (!p) continue;
    if (x >= p[0] && x <= p[0] + p[2] && y >= p[1] && y <= p[1] + p[3]) return l;
  }
  return null;
}

/** Hit-test a layer's BODY. Used only to answer a click that would otherwise
 *  have done nothing at all: a player who clicks the rectangle is asking
 *  about the camera and has to be told something. */
export function layerAt(cam: Camera, layers: Layer[], x: number, y: number): Layer | null {
  for (let k = layers.length - 1; k >= 0; k--) {
    const l = layers[k]!;
    const X = cam.ox + l.x0 * cam.scale;
    const Y = cam.oy + l.y0 * cam.scale;
    const X1 = cam.ox + l.x1 * cam.scale;
    const Y1 = cam.oy + l.y1 * cam.scale;
    if (x >= X && x <= X1 && y >= Y && y <= Y1) return l;
  }
  return null;
}

