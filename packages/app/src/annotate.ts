// THE TWO ANNOTATION PRIMITIVES. Both are LABELS. Neither carries any
// electrical or grouping meaning whatsoever, and neither may ever grow one.
//
// ------------------------------------------------------------- LABEL BOX
//
// A rectangle with a title that a player draws round some parts purely to say
// what they are (⇧J). Room state, live-synced and persisted exactly like a
// `Panel` — and that is the whole of the resemblance.
//
// A `Panel` MEANS something: it opens a mission-control window on every
// client, it collects the parts whose pins all sit inside it into a widget
// list, it captures scopes, it docks into the HUD rails, and it counts in the
// template census. That is why a label box is a SEPARATE list with a SEPARATE
// id space rather than a `no_window: true` flag on `Panel`: a flag would leave
// every one of those behaviours to be individually suppressed on every client
// forever, and one missed suppression is a control panel nobody asked for.
//
// Drawing a label box round a part changes nothing about that part. It is
// never passed to `PanelHost`, never appears in `widgetSpecs`, never owns a
// scope, and never touches the `eepanel:` localStorage namespace.
//
// ------------------------------------------------------------- NET LABEL
//
// A name pinned to a GRID POINT (⇧W), drawn on the wire and shown wherever
// that net is reported. The anchoring decision and everything that follows
// from it is written down where the choice lives: `NetLabel` in
// crates/server/src/main.rs. The short version, because this file draws the
// consequences:
//
//   * the anchor is a point, so no edit can destroy the label;
//   * a point with nothing connected to it is DETACHED — drawn dimmed,
//     reporting no net, left exactly where the player put it;
//   * two labels on one net are both real; readouts with room for one string
//     take the one the server picked (lowest y, then x);
//   * the same name on two separate nets joins NOTHING.
//
// Which net a label is on is derived server-side and arrives as the `netmap`
// message. The client never computes connectivity — it has no union-find and
// must not grow one, because a second implementation of net membership is a
// second answer to disagree with the solver's.

import type { Point } from './circuit';
import type { Camera } from './render';
import { LOD_FULL } from './render';
// A rectangle is a rectangle: the box borrows the panel's corner path and its
// handle vocabulary rather than growing a second copy of either. `main.ts`
// drives its resize drag through `resizePanelRect` for the same reason — the
// two have the same one-grid-unit minimum span.
import { roundRectPath, type PanelHandle } from './panel';

// ============================================================== LABEL BOX

export interface LabelBox {
  blid: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  name: string;
}

/** Client -> server label-box ops (mirrors the server's `LabelBoxOp`). */
export type LabelBoxOp =
  | { t: 'add'; x0: number; y0: number; x1: number; y1: number; name?: string }
  | { t: 'remove'; blid: number }
  | { t: 'rect'; blid: number; x0: number; y0: number; x1: number; y1: number }
  | { t: 'rename'; blid: number; name: string };

/** Mirrors the server's `MIN_LABEL_BOX_SPAN`: a stray click must not make one. */
export const MIN_LABEL_BOX_SPAN = 1;
/** Mirrors the server's `MAX_LABEL_BOXES`. */
export const MAX_LABEL_BOXES = 256;
/** Mirrors the server's `MAX_LABEL_BOX_NAME`, and the `maxLength` of the
 *  on-canvas rename box: the server truncates, but a field that silently eats
 *  what you type is a field that lied to you. */
export const MAX_LABEL_BOX_NAME = 28;

export function normLabelBoxRect(a: Point, b: Point): [number, number, number, number] | null {
  const x0 = Math.min(a[0], b[0]);
  const x1 = Math.max(a[0], b[0]);
  const y0 = Math.min(a[1], b[1]);
  const y1 = Math.max(a[1], b[1]);
  if (![x0, y0, x1, y1].every(Number.isFinite)) return null;
  if (x1 - x0 < MIN_LABEL_BOX_SPAN || y1 - y0 < MIN_LABEL_BOX_SPAN) return null;
  return [x0, y0, x1, y1];
}

/** Offline mirror of the server's `apply_label_box_op` (same validation). */
export function applyLabelBoxOp(
  boxes: LabelBox[],
  op: LabelBoxOp,
  allocBlid: () => number,
): LabelBox[] {
  if (op.t === 'add') {
    const r = normLabelBoxRect([op.x0, op.y0], [op.x1, op.y1]);
    if (!r || boxes.length >= MAX_LABEL_BOXES) return boxes;
    const blid = allocBlid();
    const name = op.name?.trim().slice(0, MAX_LABEL_BOX_NAME) || `LABEL ${blid}`;
    return [...boxes, { blid, x0: r[0], y0: r[1], x1: r[2], y1: r[3], name }];
  }
  if (op.t === 'remove') return boxes.filter((b) => b.blid !== op.blid);
  const b = boxes.find((x) => x.blid === op.blid);
  if (!b) return boxes;
  if (op.t === 'rect') {
    const r = normLabelBoxRect([op.x0, op.y0], [op.x1, op.y1]);
    if (r) [b.x0, b.y0, b.x1, b.y1] = r;
  } else {
    const name = op.name.trim().slice(0, MAX_LABEL_BOX_NAME);
    if (name) b.name = name;
  }
  return boxes;
}

// ------------------------------------------------------------ box drawing
//
// A label box is drawn in a deliberately DIFFERENT register from a control
// panel: a solid thin amber frame with the title sitting IN the top edge, the
// way a block heading is drawn on a real schematic, against the panel's dashed
// cyan region with a tab floating above it. A player has to be able to tell at
// a glance which boxes have windows behind them and which are just words.

const TITLE_H = 17;
const TITLE_FONT = '11px ui-monospace, monospace';
const CAPTION_PX = 10;
const CAPTION_FONT = `${CAPTION_PX}px ui-monospace, monospace`;
/** ui-monospace advance at 11px. Drawing and hit-testing share it, so the
 *  title a player clicks is exactly the title that was drawn. */
const CHAR_W = 6.7;
const CLOSE_W = 15;
const GRIP_R = 4.5;
const GRIP_HIT = 7;
const EDGE_HIT = 6;

const HANDLES: PanelHandle[] = ['nw', 'ne', 'se', 'sw', 'n', 'e', 's', 'w'];

const INK = '#d8a657';
const INK_HOT = '#ffcf7a';
const INK_DIM = '#8a6f45';

/** The title plate sits INSIDE the box's top edge, so a label box never
 *  overlaps whatever is above it and two stacked boxes cannot collide.
 *
 *  It is CLAMPED to the box's own width, and the × is measured out of that
 *  width before the name is: a plate that overflowed its box would put the
 *  delete glyph outside the thing it deletes, and a box too small to show its
 *  own × is a box a player cannot get rid of. The name is what gets truncated
 *  (and `drawLabelBoxes` clips it), never the control. */
function titleRect(cam: Camera, b: LabelBox): [number, number, number, number] {
  const want = Math.max(56, b.name.length * CHAR_W + 14 + CLOSE_W);
  const boxW = (b.x1 - b.x0) * cam.scale - 2;
  const w = Math.max(0, Math.min(want, boxW));
  return [cam.ox + b.x0 * cam.scale + 1, cam.oy + b.y0 * cam.scale + 1, w, TITLE_H];
}

/** Is the box wide enough to carry its title plate at all? Below this the box
 *  speaks for itself and Delete (pointer over it) is how it goes away. */
const titleUsable = (w: number) => w >= CLOSE_W + 12;

/** Same discipline panel tabs follow: only hit-test a glyph at the zoom where
 *  it is actually drawn, so a vanished title leaves no invisible click zone. */
function titleFits(cam: Camera): boolean {
  return cam.scale >= LOD_FULL;
}

function boxPx(cam: Camera, b: LabelBox): [number, number, number, number] {
  return [
    cam.ox + b.x0 * cam.scale,
    cam.oy + b.y0 * cam.scale,
    (b.x1 - b.x0) * cam.scale,
    (b.y1 - b.y0) * cam.scale,
  ];
}

function gripPx(cam: Camera, b: LabelBox, h: PanelHandle): [number, number] {
  const [X, Y, W, H] = boxPx(cam, b);
  return [
    h.includes('w') ? X : h.includes('e') ? X + W : X + W / 2,
    h.includes('n') ? Y : h.includes('s') ? Y + H : Y + H / 2,
  ];
}

/** Draw every label box. The hot one also shows its eight resize grips. */
export function drawLabelBoxes(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  boxes: LabelBox[],
  hotBlid: number | null = null,
) {
  if (boxes.length === 0) return;
  ctx.save();
  ctx.font = TITLE_FONT;
  ctx.textBaseline = 'middle';
  for (const b of boxes) {
    const hot = b.blid === hotBlid;
    const [X, Y, W, H] = boxPx(cam, b);
    // No interior wash, for the same reason a panel has none: the box frames
    // the parts inside it, it does not lie over them.
    roundRectPath(ctx, X, Y, W, H, Math.min(10, cam.scale * 0.35));
    ctx.lineWidth = hot ? 1.8 : 1.1;
    ctx.strokeStyle = hot ? INK_HOT : INK_DIM;
    ctx.stroke();

    if (hot) {
      ctx.lineWidth = 1.2;
      for (const h of HANDLES) {
        const [hx, hy] = gripPx(cam, b, h);
        ctx.beginPath();
        ctx.rect(hx - GRIP_R, hy - GRIP_R, GRIP_R * 2, GRIP_R * 2);
        ctx.fillStyle = '#1b1508';
        ctx.fill();
        ctx.strokeStyle = INK_HOT;
        ctx.stroke();
      }
    }

    if (!titleFits(cam)) {
      // Zoomed out: the heading is the only part worth keeping, clipped to
      // the box so it can never spill outside the thing it names.
      if (H >= CAPTION_PX * 2 && W >= CAPTION_PX * 2) {
        ctx.save();
        roundRectPath(ctx, X, Y, W, H, Math.min(10, cam.scale * 0.35));
        ctx.clip();
        ctx.font = CAPTION_FONT;
        ctx.textBaseline = 'alphabetic';
        ctx.fillStyle = hot ? INK_HOT : INK_DIM;
        ctx.fillText(b.name, X + 4, Y + CAPTION_PX + 3);
        ctx.restore();
        ctx.font = TITLE_FONT;
        ctx.textBaseline = 'middle';
      }
      continue;
    }
    const [tx, ty, tw, th] = titleRect(cam, b);
    if (!titleUsable(tw)) continue; // too small for chrome; Delete removes it
    ctx.save();
    roundRectPath(ctx, tx, ty, tw, th, 4);
    ctx.fillStyle = hot ? '#3a2c12' : '#241c0d';
    ctx.fill();
    // The NAME is clipped to the space left over by the ×, so a long title in
    // a short box runs out of room instead of running over the control.
    ctx.save();
    ctx.beginPath();
    ctx.rect(tx, ty, Math.max(0, tw - CLOSE_W), th);
    ctx.clip();
    ctx.fillStyle = hot ? INK_HOT : INK;
    ctx.fillText(b.name, tx + 7, ty + th / 2 + 0.5);
    ctx.restore();
    ctx.fillStyle = hot ? '#ff9a9a' : '#7d6a4a';
    ctx.fillText('×', tx + tw - CLOSE_W + 4, ty + th / 2 + 0.5);
    ctx.restore();
  }
  ctx.restore();
}

/** The in-progress box while the ⇧J tool is dragging. */
export function drawLabelBoxGhost(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  a: Point,
  b: Point,
) {
  const X = cam.ox + Math.min(a[0], b[0]) * cam.scale;
  const Y = cam.oy + Math.min(a[1], b[1]) * cam.scale;
  const W = Math.abs(b[0] - a[0]) * cam.scale;
  const H = Math.abs(b[1] - a[1]) * cam.scale;
  ctx.save();
  roundRectPath(ctx, X, Y, W, H, Math.min(10, cam.scale * 0.35));
  ctx.fillStyle = '#d8a65710';
  ctx.fill();
  ctx.setLineDash([4, 5]);
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = INK_HOT;
  ctx.stroke();
  ctx.restore();
}

export type LabelBoxZone =
  | { box: LabelBox; zone: 'title' | 'close' }
  | { box: LabelBox; zone: 'resize'; handle: PanelHandle };

/** Hit-test the titles and the resize grips. The box body stays
 *  click-through: the parts inside a label box are ordinary parts, and the
 *  whole point of the primitive is that drawing one changes nothing. */
export function labelBoxZoneAt(
  cam: Camera,
  boxes: LabelBox[],
  x: number,
  y: number,
): LabelBoxZone | null {
  for (let k = boxes.length - 1; k >= 0; k--) {
    const b = boxes[k]!;
    if (titleFits(cam)) {
      const [tx, ty, tw, th] = titleRect(cam, b);
      // Exactly the plate that was drawn, including the case where it was not
      // drawn at all: a vanished control must never leave a click zone.
      if (titleUsable(tw) && x >= tx && x <= tx + tw && y >= ty && y <= ty + th) {
        return { box: b, zone: x >= tx + tw - CLOSE_W ? 'close' : 'title' };
      }
    }
    for (const h of HANDLES) {
      const [hx, hy] = gripPx(cam, b, h);
      if (Math.abs(x - hx) <= GRIP_HIT && Math.abs(y - hy) <= GRIP_HIT) {
        return { box: b, zone: 'resize', handle: h };
      }
    }
  }
  return null;
}

/** The box the pointer is working with — its title, a grip, or its EDGE.
 *  Never its interior: a box that lit up whenever the pointer crossed it
 *  would put a permanent highlight over everything it contains. */
export function labelBoxHotAt(
  cam: Camera,
  boxes: LabelBox[],
  x: number,
  y: number,
): LabelBox | null {
  const z = labelBoxZoneAt(cam, boxes, x, y);
  if (z) return z.box;
  for (let k = boxes.length - 1; k >= 0; k--) {
    const b = boxes[k]!;
    const [X, Y, W, H] = boxPx(cam, b);
    const inOuter =
      x >= X - EDGE_HIT && x <= X + W + EDGE_HIT && y >= Y - EDGE_HIT && y <= Y + H + EDGE_HIT;
    if (!inOuter) continue;
    const inInner =
      x > X + EDGE_HIT && x < X + W - EDGE_HIT && y > Y + EDGE_HIT && y < Y + H - EDGE_HIT;
    if (!inInner) return b;
  }
  return null;
}

/** Where the rename box goes for a label box: over its own title plate. */
export function labelBoxTitleAnchor(cam: Camera, b: LabelBox): [number, number] {
  const [tx, ty] = titleRect(cam, b);
  return [tx, ty];
}

// ============================================================== NET LABEL

export interface NetLabel {
  nlid: number;
  /** The anchor: a GRID POINT, in the same integer lattice pins live in. */
  x: number;
  y: number;
  name: string;
}

/** Client -> server net-label ops (mirrors the server's `NetLabelOp`). */
export type NetLabelOp =
  | { t: 'add'; x: number; y: number; name?: string }
  | { t: 'remove'; nlid: number }
  | { t: 'move'; nlid: number; x: number; y: number }
  | { t: 'rename'; nlid: number; name: string };

/** Mirrors the server's `MAX_NET_LABELS`. */
export const MAX_NET_LABELS = 64;
/** Mirrors the server's `MAX_NET_LABEL_NAME` (= `MAX_NAME`, so a net name and
 *  a part name never make a ragged column). */
export const MAX_NET_LABEL_NAME = 24;
const MAX_NET_LABEL_COORD = 1_000_000_000;

/** Which nets are named what, derived by the SERVER from the compiled
 *  document and broadcast as `netmap`. Never persisted, never an input to
 *  anything — a pure read-out. */
export interface NetMap {
  /** The labels whose anchor is a junction of the live document. Everything
   *  else is DETACHED: drawn dimmed, reporting no net. */
  live: Set<number>;
  /** probe pid -> nlid of the label naming that probe's net. */
  probe: Map<number, number>;
}

export const emptyNetMap = (): NetMap => ({ live: new Set(), probe: new Map() });

/** Offline mirror of the server's `apply_net_label_op` (same validation,
 *  including "two labels may not share one point": the second would be
 *  unreachable under the first). */
export function applyNetLabelOp(
  labels: NetLabel[],
  op: NetLabelOp,
  allocNlid: () => number,
): NetLabel[] {
  if (op.t === 'add') {
    if (!Number.isInteger(op.x) || !Number.isInteger(op.y)) return labels;
    if (Math.abs(op.x) > MAX_NET_LABEL_COORD || Math.abs(op.y) > MAX_NET_LABEL_COORD) return labels;
    if (labels.length >= MAX_NET_LABELS) return labels;
    if (labels.some((l) => l.x === op.x && l.y === op.y)) return labels;
    const nlid = allocNlid();
    const name = op.name?.trim().slice(0, MAX_NET_LABEL_NAME) || `NET ${nlid}`;
    return [...labels, { nlid, x: op.x, y: op.y, name }];
  }
  if (op.t === 'remove') return labels.filter((l) => l.nlid !== op.nlid);
  const l = labels.find((x) => x.nlid === op.nlid);
  if (!l) return labels;
  if (op.t === 'move') {
    if (!Number.isInteger(op.x) || !Number.isInteger(op.y)) return labels;
    if (labels.some((q) => q.nlid !== op.nlid && q.x === op.x && q.y === op.y)) return labels;
    l.x = op.x;
    l.y = op.y;
  } else {
    const name = op.name.trim().slice(0, MAX_NET_LABEL_NAME);
    if (name) l.name = name;
  }
  return labels;
}

// --------------------------------------------------------- label drawing

const NET_FONT = '11px ui-monospace, monospace';
const NET_H = 15;
const NET_CHAR_W = 6.7;
/** How far above the anchor the plate floats, in screen px. */
const NET_LIFT = 13;
const NET_INK = '#7ce0b0';
const NET_INK_HOT = '#b6ffd8';
/** Detached: the anchor has nothing connected to it. Never an error — the
 *  player may simply have labelled the rail before drawing it. */
const NET_INK_DEAD = '#6a7d73';

/** The plate for one net label in screen px: `[x, y, w, h]`. */
export function netLabelRect(cam: Camera, l: NetLabel): [number, number, number, number] {
  const w = l.name.length * NET_CHAR_W + 12;
  const px = cam.ox + l.x * cam.scale;
  const py = cam.oy + l.y * cam.scale;
  return [Math.round(px - w / 2), Math.round(py - NET_LIFT - NET_H), w, NET_H];
}

/** Draw the net labels: a small flag on a stalk standing on its grid point,
 *  which is exactly what a net name is on paper. */
export function drawNetLabels(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  labels: NetLabel[],
  map: NetMap,
  hotNlid: number | null = null,
) {
  if (labels.length === 0) return;
  ctx.save();
  ctx.font = NET_FONT;
  ctx.textBaseline = 'middle';
  for (const l of labels) {
    const attached = map.live.has(l.nlid);
    const hot = l.nlid === hotNlid;
    const ink = hot ? NET_INK_HOT : attached ? NET_INK : NET_INK_DEAD;
    const [X, Y, W, H] = netLabelRect(cam, l);
    const px = cam.ox + l.x * cam.scale;
    const py = cam.oy + l.y * cam.scale;

    // The stalk, so which point is named is never ambiguous.
    ctx.beginPath();
    ctx.moveTo(px, py);
    ctx.lineTo(px, Y + H);
    ctx.lineWidth = 1;
    ctx.strokeStyle = ink;
    // A detached label says so in the line as well as the colour: nothing is
    // connected here, and a dashed stalk reads that way at any zoom.
    ctx.setLineDash(attached ? [] : [3, 3]);
    ctx.stroke();
    ctx.setLineDash([]);
    // The foot: a filled dot on the junction it names, hollow when detached.
    ctx.beginPath();
    ctx.arc(px, py, 2.6, 0, Math.PI * 2);
    if (attached) {
      ctx.fillStyle = ink;
      ctx.fill();
    } else {
      ctx.stroke();
    }

    roundRectPath(ctx, X, Y, W, H, 4);
    ctx.fillStyle = hot ? '#12291f' : '#0e1b16';
    ctx.fill();
    ctx.lineWidth = 1;
    ctx.strokeStyle = ink;
    ctx.stroke();
    ctx.fillStyle = ink;
    ctx.fillText(l.name, X + 6, Y + H / 2 + 0.5);
  }
  ctx.restore();
}

/** The label under the pointer (its plate), or null. */
export function netLabelAt(
  cam: Camera,
  labels: NetLabel[],
  x: number,
  y: number,
): NetLabel | null {
  for (let k = labels.length - 1; k >= 0; k--) {
    const l = labels[k]!;
    const [X, Y, W, H] = netLabelRect(cam, l);
    if (x >= X && x <= X + W && y >= Y && y <= Y + H) return l;
  }
  // ...and the ANCHOR itself, not only the plate.
  //
  // The plate is drawn offset from the grid point it names, so a player who
  // puts the cursor on the label — on the dot, where they clicked to make it
  // — was missing it entirely, and Delete did nothing. Measured: hovering the
  // exact anchor and pressing Delete left the label in place. A thing you can
  // make and cannot unmake is a trap, and "hover a few pixels up and to the
  // right of where you clicked" is not a discoverable escape from it.
  const r = Math.max(10, cam.scale * 0.35);
  for (let k = labels.length - 1; k >= 0; k--) {
    const l = labels[k]!;
    const px = cam.ox + l.x * cam.scale;
    const py = cam.oy + l.y * cam.scale;
    if (Math.hypot(px - x, py - y) <= r) return l;
  }
  return null;
}

/** The name to show for a probe's net, or null when it is on an unnamed one.
 *  One lookup, used by every readout, so the scope chip, the dock chip and
 *  the panel row can never disagree about what a net is called. */
export function netNameForProbe(
  pid: number,
  labels: NetLabel[],
  map: NetMap,
): string | null {
  const nlid = map.probe.get(pid);
  if (nlid === undefined) return null;
  return labels.find((l) => l.nlid === nlid)?.name ?? null;
}
