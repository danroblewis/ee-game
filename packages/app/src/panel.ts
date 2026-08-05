// Control panels: mission-control boxes decoupled from the schematic.
//
// A player drags a dotted PANEL REGION (key J) around a group of parts. The
// region is room-scoped shared state (server owns {plid, rect, name}), but
// membership is never stored: the elements whose pins ALL sit inside the
// rectangle are re-derived from geometry every frame, so moving a part in or
// out re-wires the panel live.
//
// Each region gets a floating HTML CONTROL WINDOW (not canvas) that
// auto-populates widgets from its members — sliders/knobs for pots, toggles
// for switches, indicators for lamps and LEDs, numeric+slider for DC
// sources, and a big meter readout for any probe on a member. Every number
// shown comes out of the solver frame (design pillar); widgets only ever
// send InteractOps through the same path the canvas uses.
//
// An in-place OSCILLOSCOPE whose rect lands inside a region is part of that
// panel's interface too (`scopeOwner`/`panelScopes`, geometry-derived like
// member elements): the schematic keeps only a draggable placeholder while the
// live instrument becomes a canvas widget row in the window.
//
// A window can float over the world or be dropped into one of the two
// collapsible HUD RAILS, where it sticks. The rails overlay the canvas (the
// camera stays window-sized) but swallow their own pointer events, so a
// click in a sidebar never reaches the schematic. Which rail a panel is in,
// and where in it, is per-player like everything else below.
//
// Pointing at a row highlights that part out on the canvas in the schematic's
// own "this one" blue, and pointing at a part on the canvas makes its row
// read hot in the region-cyan — the two ends of the same wire.
//
// Screen-anchored, per-player chrome (window position or rail slot,
// slider-vs-knob choice, widget row order) lives in localStorage keyed by
// plid; the region itself is shared, so everyone sees the same panel with
// their own layout.

import type { ElemLive, ElementSpec, InteractOp, Point } from './circuit';
import { LED_COLORS, LOD_FULL, type Camera } from './render';
import {
  applyScopeControl,
  channelName,
  probeColor,
  renderScope,
  scopeChannels,
  scopeControlAt,
  type FloatScope,
  type NetNames,
  type Probe,
  type ScopeControlId,
  type TraceStore,
} from './scope';
import { fmtEng, fmtEntry, parseField, quantityOf } from './units';
import { phonePosture } from './touchenv';

// ------------------------------------------------------------ shared state

export interface Panel {
  plid: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  name: string;
}

/** Client -> server panel ops (mirrors the server's PanelOp enum). */
export type PanelOp =
  | { t: 'add'; x0: number; y0: number; x1: number; y1: number; name?: string }
  | { t: 'remove'; plid: number }
  | { t: 'rect'; plid: number; x0: number; y0: number; x1: number; y1: number }
  | { t: 'rename'; plid: number; name: string };

/** Smallest accepted region in grid units (mirrors the server's rule). */
export const MIN_PANEL_SPAN = 1;

/** Just the geometry of a region — what a resize drag works on. */
export interface PanelRect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** Normalize a drag rectangle; null = too small / not finite (dropped). */
export function normPanelRect(a: Point, b: Point): [number, number, number, number] | null {
  const x0 = Math.min(a[0], b[0]);
  const x1 = Math.max(a[0], b[0]);
  const y0 = Math.min(a[1], b[1]);
  const y1 = Math.max(a[1], b[1]);
  if (![x0, y0, x1, y1].every(Number.isFinite)) return null;
  if (x1 - x0 < MIN_PANEL_SPAN || y1 - y0 < MIN_PANEL_SPAN) return null;
  return [x0, y0, x1, y1];
}

/** Offline mirror of the server's `apply_panel_op` (same validation). */
export function applyPanelOp(panels: Panel[], op: PanelOp, allocPlid: () => number): Panel[] {
  if (op.t === 'add') {
    const r = normPanelRect([op.x0, op.y0], [op.x1, op.y1]);
    if (!r) return panels;
    const plid = allocPlid();
    const name = op.name?.trim() || `PANEL ${plid}`;
    return [...panels, { plid, x0: r[0], y0: r[1], x1: r[2], y1: r[3], name }];
  }
  if (op.t === 'remove') return panels.filter((p) => p.plid !== op.plid);
  const p = panels.find((x) => x.plid === op.plid);
  if (!p) return panels;
  if (op.t === 'rect') {
    const r = normPanelRect([op.x0, op.y0], [op.x1, op.y1]);
    if (r) [p.x0, p.y0, p.x1, p.y1] = r;
  } else {
    const name = op.name.trim();
    if (name) p.name = name;
  }
  return panels;
}

/** A panel's members: elements with EVERY pin inside the region. */
export function panelMembers(panel: Panel, elements: ElementSpec[]): ElementSpec[] {
  return elements.filter(
    (e) =>
      e.pins.length > 0 &&
      e.pins.every(
        ([x, y]) => x >= panel.x0 && x <= panel.x1 && y >= panel.y0 && y <= panel.y1,
      ),
  );
}

/** True when a scope's rect is fully inside a region. */
const scopeInside = (panel: Panel, s: FloatScope) =>
  s.x >= panel.x0 && s.x + s.w <= panel.x1 && s.y >= panel.y0 && s.y + s.h <= panel.y1;

const panelArea = (p: Panel) => (p.x1 - p.x0) * (p.y1 - p.y0);

/** The panel a scope belongs to: the SMALLEST region that fully contains its
 * rect (lowest plid breaks a tie, so every client agrees). null = the scope is
 * a normal floating instrument on the schematic. Derived from geometry every
 * frame, exactly like `panelMembers` — dragging a scope out detaches it. */
export function scopeOwner(panels: Panel[], s: FloatScope): Panel | null {
  let best: Panel | null = null;
  for (const p of panels) {
    if (!scopeInside(p, s)) continue;
    if (!best) {
      best = p;
      continue;
    }
    const a = panelArea(p);
    const b = panelArea(best);
    if (a < b || (a === b && p.plid < best.plid)) best = p;
  }
  return best;
}

/** A panel's scopes: the floating scopes this region owns. `panels` is the
 * whole list so nested regions resolve to the innermost one. */
export function panelScopes(
  panel: Panel,
  scopes: FloatScope[],
  panels: Panel[] = [panel],
): FloatScope[] {
  return scopes.filter((s) => scopeOwner(panels, s)?.plid === panel.plid);
}

// --------------------------------------------------------- canvas regions

const TAB_H = 19;
const TAB_FONT = '11px ui-monospace, monospace';
/** The zoomed-out caption: small, dim, and clipped inside the region. */
const CAPTION_PX = 10;
const CAPTION_FONT = `${CAPTION_PX}px ui-monospace, monospace`;
/** ui-monospace advance at 11px. Drawing and hit-testing share it so the
 * tab the player clicks is exactly the tab that was drawn. */
const CHAR_W = 6.7;
const CLOSE_W = 17;

/** Resize grips: four corners plus four edge midpoints. */
export type PanelHandle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';
/** Corners first, so they win over an edge grip on a tiny region. */
const HANDLES: PanelHandle[] = ['nw', 'ne', 'se', 'sw', 'n', 'e', 's', 'w'];
/** Half the drawn grip square, and the (slightly larger) click radius. */
const GRIP_R = 4.5;
const GRIP_HIT = 7;
/** How wide the region's border reads as "the region" for hovering. */
const EDGE_HIT = 6;

export const PANEL_HANDLE_CURSOR: Record<PanelHandle, string> = {
  nw: 'nwse-resize',
  se: 'nwse-resize',
  ne: 'nesw-resize',
  sw: 'nesw-resize',
  n: 'ns-resize',
  s: 'ns-resize',
  e: 'ew-resize',
  w: 'ew-resize',
};

/** What the region's tab reads. A docked panel keeps its region on the
 * canvas and grows an arrow saying which sidebar its window went into —
 * the same courtesy a panel-owned scope's placeholder pays. */
function tabLabel(p: Panel): string {
  const side = dockSideOf(p.plid);
  return side ? `${p.name} ${side === 'left' ? '⇤' : '⇥'}` : p.name;
}

function tabRect(cam: Camera, p: Panel): [number, number, number, number] {
  // Drawing and hit-testing share tabLabel, so the tab the player clicks is
  // exactly the tab that was drawn, docked or not.
  const w = Math.max(72, tabLabel(p).length * CHAR_W + 18 + CLOSE_W);
  return [cam.ox + p.x0 * cam.scale, cam.oy + p.y0 * cam.scale - TAB_H - 3, w, TAB_H];
}

/** The tab is fixed screen-space chrome, so zooming out eventually makes it
 *  bigger than the region it labels. It therefore switches to a quiet caption
 *  at exactly the zoom where the schematic itself stops being drawn — the same
 *  moment the current dots go — so the whole view calms down at once instead
 *  of in stages. Applies to drawing AND hit-testing, so the vanished tab never
 *  leaves an invisible click zone behind. */
function tabFits(cam: Camera): boolean {
  return cam.scale >= LOD_FULL;
}

/** The region in screen pixels: [x, y, w, h]. */
function regionPx(cam: Camera, p: Panel): [number, number, number, number] {
  return [
    cam.ox + p.x0 * cam.scale,
    cam.oy + p.y0 * cam.scale,
    (p.x1 - p.x0) * cam.scale,
    (p.y1 - p.y0) * cam.scale,
  ];
}

/** Where one grip sits, in screen pixels. */
function gripPx(cam: Camera, p: Panel, h: PanelHandle): [number, number] {
  const [X, Y, W, H] = regionPx(cam, p);
  return [
    h.includes('w') ? X : h.includes('e') ? X + W : X + W / 2,
    h.includes('n') ? Y : h.includes('s') ? Y + H : Y + H / 2,
  ];
}

/** Drag `handle` of `base` to grid point (gx, gy). Snapped to the integer
 * grid, never thinner than MIN_PANEL_SPAN, and normalised: dragging an edge
 * past its opposite flips the rectangle instead of inverting it. */
export function resizePanelRect(
  base: PanelRect,
  handle: PanelHandle,
  gx: number,
  gy: number,
): PanelRect {
  // `back` = the dragged edge started on the low side of its anchor, which
  // is the direction to push it when the drag lands exactly on the anchor.
  const span = (anchor: number, moved: number, back: boolean): [number, number] => {
    const d = moved - anchor;
    const sign = d === 0 ? (back ? -1 : 1) : Math.sign(d);
    const m = anchor + sign * Math.max(MIN_PANEL_SPAN, Math.abs(d));
    return [Math.min(anchor, m), Math.max(anchor, m)];
  };
  let [x0, x1] = [base.x0, base.x1];
  let [y0, y1] = [base.y0, base.y1];
  if (handle.includes('w')) [x0, x1] = span(base.x1, Math.round(gx), true);
  else if (handle.includes('e')) [x0, x1] = span(base.x0, Math.round(gx), false);
  if (handle.includes('n')) [y0, y1] = span(base.y1, Math.round(gy), true);
  else if (handle.includes('s')) [y0, y1] = span(base.y0, Math.round(gy), false);
  return { x0, y0, x1, y1 };
}

/** Rounded-rect path. Exported so the schematic's panel-owned-scope
 * placeholder is drawn with exactly the region chrome's corner. */
export function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  const k = Math.max(0, Math.min(r, Math.min(w, h) / 2));
  ctx.beginPath();
  ctx.moveTo(x + k, y);
  ctx.arcTo(x + w, y, x + w, y + h, k);
  ctx.arcTo(x + w, y + h, x, y + h, k);
  ctx.arcTo(x, y + h, x, y, k);
  ctx.arcTo(x, y, x + w, y, k);
  ctx.closePath();
}

/** Draw every panel region: dotted rounded rect plus its name tab. The hot
 * one (pointer over it, or mid-drag) also shows its eight resize grips. */
export function drawPanelRegions(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  panels: Panel[],
  hoverPlid: number | null = null,
) {
  if (panels.length === 0) return;
  ctx.save();
  ctx.font = TAB_FONT;
  ctx.textBaseline = 'middle';
  for (const p of panels) {
    const hot = p.plid === hoverPlid;
    const [X, Y, W, H] = regionPx(cam, p);
    // No interior wash: the region is a boundary, not a pane of glass over
    // the parts inside it.
    roundRectPath(ctx, X, Y, W, H, Math.min(16, cam.scale * 0.5));
    ctx.setLineDash([5, 6]);
    ctx.lineWidth = hot ? 2 : 1.4;
    ctx.strokeStyle = hot ? '#8ee7ff' : '#4a8ea8';
    ctx.stroke();
    ctx.setLineDash([]);

    if (hot) {
      ctx.lineWidth = 1.2;
      for (const h of HANDLES) {
        const [hx, hy] = gripPx(cam, p, h);
        ctx.beginPath();
        ctx.rect(hx - GRIP_R, hy - GRIP_R, GRIP_R * 2, GRIP_R * 2);
        ctx.fillStyle = '#0d1c23';
        ctx.fill();
        ctx.strokeStyle = '#8ee7ff';
        ctx.stroke();
      }
    }

    if (!tabFits(cam)) {
      // Zoomed out: the name sits quietly in the bottom-left corner, clipped
      // to the region so it can never spill outside the box it names. Below a
      // couple of text heights even that is noise, so the box speaks for itself.
      if (H >= CAPTION_PX * 2 && W >= CAPTION_PX * 2) {
        ctx.save();
        roundRectPath(ctx, X, Y, W, H, Math.min(16, cam.scale * 0.5));
        ctx.clip();
        ctx.font = CAPTION_FONT;
        ctx.textBaseline = 'alphabetic';
        ctx.fillStyle = hot ? '#8ee7ff' : '#6f8b98';
        ctx.fillText(p.name, X + 4, Y + H - 4);
        ctx.restore();
        ctx.font = TAB_FONT;
        ctx.textBaseline = 'middle';
      }
      continue;
    }
    const [tx, ty, tw, th] = tabRect(cam, p);
    roundRectPath(ctx, tx, ty, tw, th, 5);
    ctx.fillStyle = hot ? '#22333d' : '#18242d';
    ctx.fill();
    ctx.lineWidth = 1;
    ctx.strokeStyle = hot ? '#8ee7ff' : '#3b5c6b';
    ctx.stroke();
    ctx.fillStyle = '#c6e8f4';
    ctx.fillText(tabLabel(p), tx + 8, ty + th / 2 + 0.5);
    ctx.fillStyle = hot ? '#ff9a9a' : '#7f97a2';
    ctx.fillText('×', tx + tw - CLOSE_W + 5, ty + th / 2 + 0.5);
  }
  ctx.restore();
}

/** The in-progress region while the J tool is dragging. */
export function drawPanelGhost(ctx: CanvasRenderingContext2D, cam: Camera, a: Point, b: Point) {
  const X = cam.ox + Math.min(a[0], b[0]) * cam.scale;
  const Y = cam.oy + Math.min(a[1], b[1]) * cam.scale;
  const W = Math.abs(b[0] - a[0]) * cam.scale;
  const H = Math.abs(b[1] - a[1]) * cam.scale;
  ctx.save();
  roundRectPath(ctx, X, Y, W, H, Math.min(16, cam.scale * 0.5));
  ctx.fillStyle = '#4ad4ff10';
  ctx.fill();
  ctx.setLineDash([4, 5]);
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = '#8ee7ff';
  ctx.stroke();
  ctx.restore();
}

export type PanelZone =
  | { panel: Panel; zone: 'tab' | 'close' }
  | { panel: Panel; zone: 'resize'; handle: PanelHandle };

/** Hit-test the name tabs and the resize grips (the region body stays
 * click-through so the schematic underneath keeps working normally). */
export function panelZoneAt(
  cam: Camera,
  panels: Panel[],
  x: number,
  y: number,
): PanelZone | null {
  for (let k = panels.length - 1; k >= 0; k--) {
    const p = panels[k]!;
    // Only when the tab is actually drawn — otherwise the strip above a
    // zoomed-out region would still swallow clicks meant for the schematic.
    if (tabFits(cam)) {
      const [tx, ty, tw, th] = tabRect(cam, p);
      if (x >= tx && x <= tx + tw && y >= ty && y <= ty + th) {
        return { panel: p, zone: x >= tx + tw - CLOSE_W ? 'close' : 'tab' };
      }
    }
    for (const h of HANDLES) {
      const [hx, hy] = gripPx(cam, p, h);
      if (Math.abs(x - hx) <= GRIP_HIT && Math.abs(y - hy) <= GRIP_HIT) {
        return { panel: p, zone: 'resize', handle: h };
      }
    }
  }
  return null;
}

/** The region the pointer is working with — its tab, a grip, or anywhere
 * inside it. Grips are drawn for this one, so every grip that can be hit is
 * a grip the player can see (the hot area covers the whole grip ring). */
export function panelHotAt(cam: Camera, panels: Panel[], x: number, y: number): Panel | null {
  // Whatever a click would act on wins, so the grips drawn are the grips hit.
  const z = panelZoneAt(cam, panels, x, y);
  if (z) return z.panel;
  // Only the EDGE counts as the region, never its interior. The parts inside a
  // panel are ordinary parts and must stay ordinary to hover and click — a
  // region that lit up whenever the pointer crossed it would put a permanent
  // highlight over everything it contains.
  for (let k = panels.length - 1; k >= 0; k--) {
    const p = panels[k]!;
    const [X, Y, W, H] = regionPx(cam, p);
    const inOuter = x >= X - EDGE_HIT && x <= X + W + EDGE_HIT && y >= Y - EDGE_HIT && y <= Y + H + EDGE_HIT;
    if (!inOuter) continue;
    const inInner = x > X + EDGE_HIT && x < X + W - EDGE_HIT && y > Y + EDGE_HIT && y < Y + H - EDGE_HIT;
    if (!inInner) return p;
  }
  return null;
}

// ------------------------------------------------------------- local prefs

/** Where a panel's window sits and whether it is collapsed is per PLAYER and
 * per ROOM: plids are room-scoped ids, so two rooms both holding a `plid: 1`
 * would otherwise share (and fight over) one stored position. The room code
 * goes in the key; the bare prefix stays the pre-rooms key, so a single-room
 * server and a player's existing layout are unchanged. */
let LS = 'eepanel';

/** Point the panel prefs at a room. Called from `resetForRoom`. */
export function setPanelRoom(code: string | null) {
  const next = code ? `eepanel:${code}` : 'eepanel';
  if (next === LS) return;
  LS = next;
  // Drop the cached rails so the next `ensureRails()` reads THIS room's saved
  // layout. They were built from whichever prefix was current when the first
  // panel arrived — and `hello` lands after boot, so that is the un-scoped
  // one. Without this the rails keep an empty default, every write goes to
  // the room-scoped key, and a docked layout is saved correctly and then
  // never read back: panels float again on every reload.
  //
  // `buildRail` is idempotent (it reuses `#rail-<side>` and its children by
  // id), so this re-reads prefs without orphaning DOM or the windows already
  // parented into a rail list.
  rails = null;
}

const lsGet = (key: string): string | null => {
  try {
    return localStorage.getItem(`${LS}:${key}`);
  } catch {
    return null; // private mode / storage disabled
  }
};
const lsSet = (key: string, value: string) => {
  try {
    localStorage.setItem(`${LS}:${key}`, value);
  } catch {
    /* ignore */
  }
};

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

function readPos(plid: number): { x: number; y: number } {
  const raw = lsGet(`${plid}:pos`);
  if (raw) {
    try {
      const p = JSON.parse(raw) as { x: number; y: number };
      if (Number.isFinite(p.x) && Number.isFinite(p.y)) {
        return {
          x: clamp(p.x, 0, Math.max(0, window.innerWidth - 120)),
          y: clamp(p.y, 0, Math.max(0, window.innerHeight - 40)),
        };
      }
    } catch {
      /* fall through to the default cascade */
    }
  }
  return {
    x: clamp(48 + ((plid * 28) % 200), 0, Math.max(0, window.innerWidth - 300)),
    y: clamp(96 + ((plid * 36) % 220), 0, Math.max(0, window.innerHeight - 200)),
  };
}

function readOrder(plid: number): string[] {
  const raw = lsGet(`${plid}:order`);
  if (!raw) return [];
  try {
    const a = JSON.parse(raw) as unknown;
    return Array.isArray(a) ? a.filter((k): k is string => typeof k === 'string') : [];
  } catch {
    return [];
  }
}

// -------------------------------------------------------------- HUD rails
//
// Two collapsible sidebars a panel window can be dropped into, where it
// sticks. They OVERLAY the canvas (the camera stays window-sized — insetting
// it would mean threading a viewport rect through every hit-test), but they
// swallow their own pointer events, so a click in a rail never reaches the
// schematic underneath.
//
// Which rail a panel lives in, and where in it, is per-player chrome exactly
// like its floating position: the region is shared, the furniture is not.
// The two `order` arrays ARE the placement — a plid in neither of them
// floats. There is deliberately no per-panel "dock" key: two sources of
// truth for where a window lives is a bug factory.

export type RailSide = 'left' | 'right';
const RAIL_SIDES: readonly RailSide[] = ['left', 'right'];

/** Where this player's copy of a panel window sits on screen. */
export type Placement =
  | { at: 'float'; x: number; y: number }
  | { at: 'rail'; side: RailSide; index: number };

/** What the player is pointing at inside a panel window, so the canvas can
 * light the matching part up. Set on pointerenter / focusin and cleared on
 * leave — never recomputed per frame. */
export interface PanelHover {
  /** Element ids to emphasise. */
  ids: number[];
  /** The owning region — its canvas rect goes hot too. */
  plid: number;
  /** 'row' = one part, strong; 'panel' = every member, weak. */
  kind: 'row' | 'panel';
}

/** Expanded rail width. 300 == `.pwin` width, so docking reflows nothing. */
const RAIL_W_DEFAULT = 300;
const RAIL_W_MIN = 240;
const RAIL_W_MAX = 460;
/** Collapsed width: the same 24 px strip the scope dock leaves behind. */
const RAIL_BAR_PX = 24;
/** The schematic never disappears. Whatever the two rails would LIKE to be,
 * they are fitted into `innerWidth - CANVAS_MIN_PX` between them, so on a
 * narrow window they can never overlap each other or cover the canvas. */
const CANVAS_MIN_PX = 200;
/** A RAIL THAT CAN NEVER OPEN IS A PANEL NOBODY CAN REACH.
 *
 * On a 390 px phone `innerWidth - CANVAS_MIN_PX` is 190 px, which is under
 * RAIL_W_MIN, so fitRails folded both rails to their 24 px strips — and the
 * strip's own click could not undo it, because the next fit folded it again.
 * Knobs, sliders and switches were landscape-only, silently.
 *
 * In the phone posture the schematic gives up its guaranteed share: one rail
 * may take nearly the screen, which is the honest answer on a phone (the
 * sidebar IS the view while it is open), and the strip that is left over is
 * one tap from putting it away. Anywhere else — including a desktop window
 * dragged down to 400 px, which is what `innerWidth` alone used to catch —
 * nothing changes at all. See touchenv.ts for why width is not enough. */
const canvasMinPx = () => (phonePosture() ? RAIL_BAR_PX + 8 : CANVAS_MIN_PX);
/** Dead zone before a header press becomes a drag (dock.ts uses the same). */
const DRAG_DEAD_PX = 4;
/** Slack outside a rail that still counts as aiming at it. */
const DROP_PAD_PX = 28;
/** Hold a window over a shut rail this long and it springs open. */
const DWELL_MS = 350;

interface RailState {
  readonly side: RailSide;
  /** Docked plids, top to bottom. Authoritative for order; pruned against
   * the shared panel list, so a deleted panel cannot leave a hole. */
  order: number[];
  /** What the PLAYER asked for. `folded` may still override it. */
  open: boolean;
  /** Open, but folded to its strip because the viewport is too narrow for
   * both rails. Derived every applyRails; never persisted, so widening the
   * window brings the sidebar straight back. */
  folded: boolean;
  /** Monotonic stamp of the last expand. The stale side folds first. */
  openedAt: number;
  /** Expanded width in px; the collapsed width is always RAIL_BAR_PX. */
  width: number;
  /** Width actually on screen right now, after fitRails had its say. This —
   * not `width` — is what the drop zones and the canvas insets are made of. */
  shownPx: number;
  readonly el: HTMLDivElement;
  readonly bar: HTMLElement;
  readonly list: HTMLDivElement;
  readonly caret: HTMLElement;
  readonly count: HTMLElement;
  /** Insertion marker, one per rail, reused (no per-drag allocation). */
  readonly caretLine: HTMLDivElement;
}

let rails: Record<RailSide, RailState> | null = null;
/** True only between a window being lifted and dropped: both rails show
 * themselves while a panel is in the air, so you can aim at one. Kept in
 * lockstep with PanelHost's drag session — tick() asserts it. */
let dragActive = false;
/** Ticks on every expand, so fitRails knows which side is the stale one. */
let openSeq = 0;
/** Set by PanelHost so the rails' own chrome can ask for a re-layout. */
let onRailsChanged: () => void = () => {};

function readRailPrefs(side: RailSide): { open: boolean; w: number; order: number[] } {
  // A rail that can now take the whole screen must not take it uninvited:
  // in the phone posture both sides start as strips whatever a wider session
  // preferred, and one tap on a strip opens it. (Widths and order are still
  // remembered — only the open/shut posture is overruled.)
  const narrow = phonePosture();
  const raw = lsGet(`rail:${side}`);
  if (raw) {
    try {
      const o = JSON.parse(raw) as { open?: unknown; w?: unknown; order?: unknown };
      return {
        open: !narrow && o.open !== false,
        w:
          typeof o.w === 'number' && Number.isFinite(o.w)
            ? clamp(o.w, RAIL_W_MIN, RAIL_W_MAX)
            : RAIL_W_DEFAULT,
        order: Array.isArray(o.order)
          ? o.order.filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
          : [],
      };
    } catch {
      /* fall through */
    }
  }
  return { open: !narrow, w: RAIL_W_DEFAULT, order: [] };
}

const writeRailPrefs = (r: RailState) =>
  lsSet(`rail:${r.side}`, JSON.stringify({ open: r.open, w: Math.round(r.width), order: r.order }));

/** The rail's on-screen width right now — what fitRails last granted it. */
const railPx = (r: RailState) => r.shownPx;
/** Expanded ON SCREEN: the player asked for it AND it fits. */
const railOpen = (r: RailState) => r.open && !r.folded;

/** Fit both rails into the viewport. They may narrow to RAIL_W_MIN together;
 * past that the side expanded longest ago folds to its 24 px strip, and past
 * THAT so does the other. `open` is never touched — folding is a fact about
 * the window size, not a preference, so a wider window undoes it for free.
 *
 * This is the whole answer to "what happens at 520 px": two 300 px rails do
 * not both fit, so exactly one of them is a strip and the canvas keeps its
 * CANVAS_MIN_PX. Rails can no longer overlap each other at any width. */
function fitRails(
  R: Record<RailSide, RailState>,
  disp: Record<RailSide, boolean>,
): { px: Record<RailSide, number>; folded: Record<RailSide, boolean> } {
  const avail = Math.max(2 * RAIL_BAR_PX, window.innerWidth - canvasMinPx());
  const folded: Record<RailSide, boolean> = { left: false, right: false };
  const measure = (): Record<RailSide, number> => ({
    left: !disp.left ? 0 : R.left.open && !folded.left ? R.left.width : RAIL_BAR_PX,
    right: !disp.right ? 0 : R.right.open && !folded.right ? R.right.width : RAIL_BAR_PX,
  });
  const fits = (p: Record<RailSide, number>) => p.left + p.right <= avail;

  let px = measure();
  if (fits(px)) return { px, folded };

  // 1. Share what there is between the expanded rails, no narrower than the
  //    width at which a control row stops being readable.
  const open = RAIL_SIDES.filter((s) => disp[s] && R[s].open);
  if (open.length > 0) {
    const fixed = RAIL_SIDES.reduce((a, s) => a + (open.includes(s) ? 0 : px[s]), 0);
    const share = Math.floor((avail - fixed) / open.length);
    for (const s of open) px[s] = Math.max(RAIL_W_MIN, Math.min(px[s], share));
    if (fits(px)) return { px, folded };
  }

  // 2. Fold, stale side first, and hand the survivor the remainder — unless
  //    the remainder is too cramped to be a sidebar at all, in which case
  //    the next turn of this loop folds that side too and both become strips.
  const stale: RailSide = R.left.openedAt <= R.right.openedAt ? 'left' : 'right';
  for (const s of [stale, stale === 'left' ? 'right' : 'left'] as const) {
    if (!disp[s] || !R[s].open) continue;
    folded[s] = true;
    px = measure();
    let usable = true;
    for (const t of RAIL_SIDES) {
      if (!disp[t] || !R[t].open || folded[t]) continue;
      const other: RailSide = t === 'left' ? 'right' : 'left';
      const room = Math.min(px[t]!, avail - px[other]!);
      if (room < RAIL_W_MIN) usable = false;
      px[t] = Math.max(RAIL_BAR_PX, room);
    }
    if (usable && fits(px)) return { px, folded };
  }
  return { px: measure(), folded };
}

/** Adopt the static markup from index.html; build it if it is missing, so
 * panel.ts still works against a bare document. */
function buildRail(side: RailSide): RailState {
  let el = document.getElementById(`rail-${side}`) as HTMLDivElement | null;
  if (!el) {
    el = document.createElement('div');
    el.id = `rail-${side}`;
    document.body.appendChild(el);
  }
  el.className = 'rail';
  let bar = el.querySelector<HTMLElement>('.rail-bar');
  let list = el.querySelector<HTMLDivElement>('.rail-list');
  let caret = el.querySelector<HTMLElement>('.rail-caret');
  let count = el.querySelector<HTMLElement>('.rail-count');
  let caretLine = el.querySelector<HTMLDivElement>('.rail-caretline');
  let grip = el.querySelector<HTMLElement>('.rail-grip');
  if (!bar || !list || !caret || !count || !caretLine || !grip) {
    el.replaceChildren();
    bar = document.createElement('button');
    bar.className = 'rail-bar';
    caret = document.createElement('span');
    caret.className = 'rail-caret';
    count = document.createElement('span');
    count.className = 'rail-count';
    bar.append(caret, count);
    list = document.createElement('div');
    list.className = 'rail-list';
    caretLine = document.createElement('div');
    caretLine.className = 'rail-caretline';
    list.appendChild(caretLine);
    grip = document.createElement('div');
    grip.className = 'rail-grip';
    el.append(bar, list, grip);
  }
  const prefs = readRailPrefs(side);
  const r: RailState = {
    side,
    order: prefs.order,
    open: prefs.open,
    folded: false,
    openedAt: 0,
    width: prefs.w,
    shownPx: prefs.open ? prefs.w : RAIL_BAR_PX,
    el,
    bar,
    list,
    caret,
    count,
    caretLine,
  };

  bar.addEventListener('click', (ev) => {
    ev.preventDefault();
    // Toggle what the player SEES: a folded rail looks shut, so the click
    // that follows must open it (and win the fit against the other side).
    r.open = !railOpen(r);
    if (r.open) r.openedAt = ++openSeq;
    onRailsChanged();
    bar!.blur(); // the canvas hotkeys listen on window; do not hold focus
  });

  // Drag the inner edge to resize. Same 4 px dead zone as the scope dock, so
  // a stray click on the grip never nudges the width.
  grip.addEventListener('pointerdown', (ev) => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    ev.stopPropagation();
    const sx = ev.clientX;
    const w0 = r.width;
    let moved = false;
    try {
      grip!.setPointerCapture(ev.pointerId);
    } catch {
      /* synthetic pointers */
    }
    const move = (m: PointerEvent) => {
      const d = side === 'left' ? m.clientX - sx : sx - m.clientX;
      if (!moved && Math.abs(d) <= DRAG_DEAD_PX) return;
      moved = true;
      r.width = clamp(w0 + d, RAIL_W_MIN, RAIL_W_MAX);
      onRailsChanged();
    };
    const up = () => {
      grip!.removeEventListener('pointermove', move);
      grip!.removeEventListener('pointerup', up);
      grip!.removeEventListener('pointercancel', up);
      if (moved) writeRailPrefs(r);
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
    grip.addEventListener('pointercancel', up);
  });

  return r;
}

function ensureRails(): Record<RailSide, RailState> {
  if (!rails) rails = { left: buildRail('left'), right: buildRail('right') };
  return rails;
}

/** Which rail holds this panel's window, or null when it floats. Read by the
 * canvas region chrome so a docked panel's tab says where its window went. */
function dockSideOf(plid: number): RailSide | null {
  if (!rails) return null;
  if (rails.left.order.includes(plid)) return 'left';
  if (rails.right.order.includes(plid)) return 'right';
  return null;
}

// ---------------------------------------------------------------- widgets

export interface PanelHostDeps {
  /** The document (server truth when online). */
  elements(): ElementSpec[];
  /** Latest solver frame by element id — the only source of shown numbers. */
  live(): Map<number, ElemLive>;
  probes(): Probe[];
  /** Sample history for the scope widgets: the very store the canvas scopes
   * draw, so a scope shows the same waveform wherever it is displayed. */
  traces(): TraceStore;
  /** Every in-place scope in the room. main.ts owns the array; panels only
   * borrow the ones their region contains. */
  scopes(): FloatScope[];
  /** probe pid -> net name, from the server's `netmap`. */
  netNames(): NetNames;
  /** Delete a floating scope (a panel-owned scope has no canvas chrome). */
  removeScope(sid: number): void;
  /** This widget just changed a scope's settings or channels IN PLACE.
   * Scopes are room state, so the change has to be replicated — the host
   * sends the same op the canvas surface sends. Every mutation of a
   * `FloatScope` in this file is followed by one of these calls. */
  scopeChanged(s: FloatScope): void;
  /** Same interact path the canvas uses (optimistic + server echo). */
  interact(e: ElementSpec, op: InteractOp): void;
  /** Panel ops (rename / delete from the window chrome). */
  op(op: PanelOp): void;
  /** The player is pointing at a control (or at a whole window); main.ts
   * draws the canvas emphasis. Fired on enter/leave only, so it costs
   * nothing per frame. */
  hover(h: PanelHover | null): void;
}

/** What a PanelWindow needs from the host it lives in. */
interface WinHost {
  /** The floating layer (#panels) — where an undocked window lives. */
  readonly root: HTMLElement;
  setHover(h: PanelHover | null): void;
  /** A header drag started: pull the window out of its rail and arm both.
   * `abort` is the gesture's own cancel path — the host calls it when the
   * gesture has to be ended from outside (a stray release, a lost focus, a
   * second drag starting), so a session can never outlive its pointer. */
  beginDrag(plid: number, pointerId: number, abort: () => void): void;
  /** Pointer moved mid-drag: resolve and show the drop target. */
  aimDrop(x: number, y: number): void;
  dropTarget(): { side: RailSide; index: number } | null;
  endDrag(plid: number, drop: { side: RailSide; index: number } | null): void;
  /** The header's dock button: a side docks, null undocks. */
  dockTo(plid: number, side: RailSide | null): void;
}

interface TickCtx {
  elems: ElementSpec[];
  byId: Map<number, ElementSpec>;
  live: Map<number, ElemLive>;
  probes: Probe[];
  /** The whole shared list: scope ownership is resolved against all regions. */
  panels: Panel[];
  scopes: FloatScope[];
  traces: TraceStore;
  /** probe pid -> the name of the net that probe sits on, when it has one.
   * Derived by the server (`netmap`); the panel never works out which parts
   * are on which net for itself. */
  netNames: NetNames;
}

interface Widget {
  key: string;
  el: HTMLDivElement;
  update(ctx: TickCtx): void;
}

interface RowParts {
  el: HTMLDivElement;
  lab: HTMLSpanElement;
  ctl: HTMLDivElement;
  val: HTMLSpanElement;
}

function makeRow(key: string, label: string): RowParts {
  const el = document.createElement('div');
  el.className = 'prow';
  el.dataset.key = key;
  const grip = document.createElement('span');
  grip.className = 'prow-grip';
  grip.textContent = '⠿';
  grip.title = 'drag to reorder';
  const lab = document.createElement('span');
  lab.className = 'prow-label';
  lab.textContent = label;
  const ctl = document.createElement('div');
  ctl.className = 'prow-ctl';
  const val = document.createElement('span');
  val.className = 'prow-val';
  val.textContent = '—';
  el.append(grip, lab, ctl, val);
  return { el, lab, ctl, val };
}

/** Throttled InteractOp sender: 40 ms while dragging, immediate on release
 * (the same cadence the canvas value-drag uses). */
function sender(deps: PanelHostDeps, get: () => ElementSpec | undefined) {
  let last = 0;
  return (op: InteractOp, force: boolean) => {
    const e = get();
    if (!e) return;
    const now = performance.now();
    if (!force && now - last < 40) return;
    last = now;
    deps.interact(e, op);
  };
}

/** What to print on a control's row.
 *
 *  A part's own name wins whenever it has one. The fallback is the kind and
 *  id — "POT #40" — which is honest but tells a player nothing, and is the
 *  reason the synth used to wrap every switch in its own panel region purely
 *  to borrow the region's name for it.
 *
 *  `extra` (a resistance, say) is appended only in the UNNAMED case: once a
 *  knob is called CUTOFF, its ohms are a detail for the property editor, not
 *  something to compete with the name for row width. */
function rowLabel(spec: ElementSpec | undefined, fallback: string, extra?: string): string {
  const named = spec?.name?.trim();
  if (named) return named;
  return extra ? `${fallback} ${extra}` : fallback;
}

/** Potentiometer: a real slider, or a knob you drag — per-widget choice. */
function potWidget(plid: number, id: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`pot:${id}`, `POT #${id}`);
  let spec: ElementSpec | undefined;
  const push = sender(deps, () => spec);

  const slider = document.createElement('input');
  slider.className = 'pslider';
  slider.type = 'range';
  slider.min = '0.01';
  slider.max = '0.99';
  slider.step = '0.002';
  slider.value = '0.5';

  const knob = document.createElement('div');
  knob.className = 'pknob';
  knob.title = 'drag ↕ to turn';
  const needle = document.createElement('i');
  knob.appendChild(needle);

  const tog = document.createElement('button');
  tog.className = 'ptog';

  let mode: 'slider' | 'knob' = lsGet(`${plid}:mode:${id}`) === 'knob' ? 'knob' : 'slider';
  const applyMode = () => {
    slider.style.display = mode === 'slider' ? '' : 'none';
    knob.style.display = mode === 'knob' ? '' : 'none';
    tog.textContent = mode === 'slider' ? '◉ knob' : '▭ slider';
    tog.title = `switch to the ${mode === 'slider' ? 'knob' : 'slider'}`;
  };
  tog.onclick = () => {
    mode = mode === 'slider' ? 'knob' : 'slider';
    lsSet(`${plid}:mode:${id}`, mode);
    applyMode();
  };

  let dragging = false;
  const setWiper = (v: number, force: boolean) => {
    const w = clamp(v, 0.01, 0.99);
    slider.value = String(w);
    needle.style.transform = `rotate(${-135 + w * 270}deg)`;
    push({ t: 'SetValue', value: w }, force);
  };

  slider.addEventListener('pointerdown', () => (dragging = true));
  slider.addEventListener('input', () => setWiper(Number(slider.value), false));
  slider.addEventListener('change', () => {
    dragging = false;
    setWiper(Number(slider.value), true);
  });

  knob.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    const start = ev.clientY;
    const base = spec?.kind.t === 'Potentiometer' ? spec.kind.wiper : Number(slider.value);
    dragging = true;
    try {
      knob.setPointerCapture(ev.pointerId);
    } catch {
      /* synthetic pointers */
    }
    const move = (m: PointerEvent) => setWiper(base + (start - m.clientY) / 160, false);
    const up = (m: PointerEvent) => {
      knob.removeEventListener('pointermove', move);
      knob.removeEventListener('pointerup', up);
      knob.removeEventListener('pointercancel', up);
      dragging = false;
      setWiper(base + (start - m.clientY) / 160, true);
    };
    knob.addEventListener('pointermove', move);
    knob.addEventListener('pointerup', up);
    knob.addEventListener('pointercancel', up);
  });

  applyMode();
  ctl.append(slider, knob, tog);

  return {
    key: `pot:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'Potentiometer') return;
      lab.textContent = rowLabel(
        spec,
        `POT #${id}`,
        fmtEntry(k.ohms, quantityOf('Potentiometer', 'ohms')),
      );
      if (!dragging) {
        slider.value = String(k.wiper);
        needle.style.transform = `rotate(${-135 + k.wiper * 270}deg)`;
      }
      const l = ctx.live.get(id);
      // The wiper pin voltage is solver output, not a UI guess.
      val.textContent = `${(k.wiper * 100).toFixed(0)}% ${l ? fmtEng(l.v[1] ?? 0, 'V') : ''}`;
    },
  };
}

/** Switch / button: a toggle. */
function switchWidget(id: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`sw:${id}`, `SW #${id}`);
  let spec: ElementSpec | undefined;
  const btn = document.createElement('button');
  btn.className = 'pswitch';
  btn.textContent = 'OFF';
  btn.onclick = () => {
    const e = spec;
    if (e?.kind.t === 'Switch') deps.interact(e, { t: 'SetSwitch', closed: !e.kind.closed });
  };
  ctl.appendChild(btn);
  return {
    key: `sw:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'Switch') return;
      lab.textContent = rowLabel(spec, `SW #${id}`);
      btn.textContent = k.closed ? 'ON' : 'OFF';
      btn.classList.toggle('on', k.closed);
      const l = ctx.live.get(id);
      val.textContent = l ? fmtEng(l.i[0] ?? 0, 'A') : '—';
    },
  };
}

/** Lamp / LED: an indicator whose brightness tracks live power or current. */
function indicatorWidget(id: number): Widget {
  const { el, lab, ctl, val } = makeRow(`ind:${id}`, `#${id}`);
  const dot = document.createElement('span');
  dot.className = 'pdot';
  ctl.appendChild(dot);
  return {
    key: `ind:${id}`,
    el,
    update(ctx) {
      const k = ctx.byId.get(id)?.kind;
      const l = ctx.live.get(id);
      let bright = 0;
      let color = '#ffd66b';
      if (k?.t === 'Lamp') {
        // Same normalization the schematic symbol uses: P / rated W.
        bright = clamp(Math.abs(l?.power ?? 0) / Math.max(1e-9, k.rated_watts), 0, 1);
        lab.textContent = rowLabel(ctx.byId.get(id), `LAMP #${id}`);
        val.textContent = l ? fmtEng(l.power, 'W') : '—';
      } else if (k?.t === 'Led') {
        bright = clamp(Math.abs(l?.i[0] ?? 0) / 0.02, 0, 1);
        color = LED_COLORS[k.color] ?? LED_COLORS[0]!;
        lab.textContent = rowLabel(ctx.byId.get(id), `LED #${id}`);
        val.textContent = l ? fmtEng(l.i[0] ?? 0, 'A') : '—';
      }
      dot.style.background = color;
      dot.style.opacity = String(0.16 + 0.84 * bright);
      dot.style.boxShadow = bright > 0.02 ? `0 0 ${(3 + bright * 15).toFixed(1)}px ${color}` : 'none';
    },
  };
}

/** DC voltage source: numeric entry plus a slider. */
function sourceWidget(id: number, dc0: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`src:${id}`, `SRC #${id}`);
  let spec: ElementSpec | undefined;
  const push = sender(deps, () => spec);
  let span = Math.max(12, Math.ceil(Math.abs(dc0) * 2));

  // Text, not number: the panel's source box takes the same engineering
  // notation as every other entry field, so "-4.5", "250m" and "1k2" all
  // mean what they say. A `type=number` input would have discarded the last
  // two before any of our code ran.
  const Q = quantityOf('VoltageSource', 'dc');
  const num = document.createElement('input');
  num.className = 'pnum';
  num.type = 'text';
  num.inputMode = 'decimal';
  num.autocomplete = 'off';
  num.spellcheck = false;
  num.title = 'volts — try 5, -4.5, 250m';
  const slider = document.createElement('input');
  slider.className = 'pslider';
  slider.type = 'range';
  slider.step = '0.05';
  const applySpan = () => {
    slider.min = String(-span);
    slider.max = String(span);
  };
  applySpan();

  let dragging = false;
  const set = (v: number, force: boolean) => {
    if (!Number.isFinite(v)) return;
    slider.value = String(v);
    num.value = fmtEntry(v, Q);
    num.classList.remove('bad');
    push({ t: 'SetValue', value: v }, force);
  };
  slider.addEventListener('pointerdown', () => (dragging = true));
  slider.addEventListener('input', () => set(Number(slider.value), false));
  slider.addEventListener('change', () => {
    dragging = false;
    set(Number(slider.value), true);
  });
  num.addEventListener('change', () => {
    const r = parseField(num.value, Q);
    if (!r.ok) {
      // The box says so rather than reverting silently: a value that vanishes
      // when you press enter teaches nothing about why it was refused.
      num.classList.add('bad');
      num.title = r.err;
      return;
    }
    num.title = 'volts — try 5, -4.5, 250m';
    span = Math.max(span, Math.ceil(Math.abs(r.value) * 1.2));
    applySpan();
    set(r.value, true);
  });
  ctl.append(num, slider);

  return {
    key: `src:${id}`,
    el,
    update(ctx) {
      spec = ctx.byId.get(id);
      const k = spec?.kind;
      if (k?.t !== 'VoltageSource') return;
      lab.textContent = rowLabel(spec, `SRC #${id}`);
      if (Math.abs(k.dc) > span) {
        span = Math.ceil(Math.abs(k.dc) * 1.2);
        applySpan();
      }
      if (!dragging && document.activeElement !== num) {
        slider.value = String(k.dc);
        num.value = fmtEntry(k.dc, Q);
        num.classList.remove('bad');
      }
      const l = ctx.live.get(id);
      val.textContent = l ? fmtEng(l.i[0] ?? 0, 'A') : '—';
    },
  };
}

/** A probe on a member element becomes a fixed voltmeter / ammeter. Waveforms
 * come from `scopeWidget` below: park an oscilloscope inside the region and it
 * moves into this window. */
function probeWidget(pid: number): Widget {
  const { el, lab, ctl, val } = makeRow(`pr:${pid}`, `METER ${pid}`);
  const seg = document.createElement('span');
  seg.className = 'pseg';
  seg.style.color = probeColor(pid);
  seg.textContent = '–.––';
  ctl.appendChild(seg);
  return {
    key: `pr:${pid}`,
    el,
    update(ctx) {
      const p = ctx.probes.find((q) => q.pid === pid);
      if (!p) {
        seg.textContent = '–.––';
        return;
      }
      const unit = p.kind === 'v' ? 'V' : 'A';
      // A meter on a NAMED net says which net. `VOLT 3` is an index into a
      // list nobody keeps; `VOLT 3 · CUTOFF` is the wire you meant.
      const net = ctx.netNames?.get(pid);
      lab.textContent = `${p.kind === 'v' ? 'VOLT' : 'AMP'} ${pid}${net ? ` · ${net}` : ''}`;
      const l = ctx.live.get(p.elem);
      if (!l) {
        seg.textContent = `–.–– ${unit}`;
        val.textContent = `#${p.elem}.${p.pin}`;
        return;
      }
      let v = p.kind === 'v' ? (l.v[p.pin] ?? 0) : (l.i[p.pin] ?? 0);
      if (p.kind === 'v' && p.r) v -= ctx.live.get(p.r[0])?.v[p.r[1]] ?? 0;
      seg.textContent = fmtEng(v, unit);
      val.textContent = `#${p.elem}.${p.pin}${p.r ? ' Δ' : ''}`;
    },
  };
}

/** An oscilloscope parked inside this region: the panel IS its enclosure, so
 * the instrument is drawn into a row canvas instead of onto the schematic.
 *
 * It is the same instrument, not a copy: the widget looks the scope up by sid
 * every tick and renders from main.ts's TraceStore with the scope's own
 * ScopeSettings object, so timebase / trigger / manual scale are shared with
 * the canvas (drag it out of the region and it keeps every knob).
 *
 * `renderScope` is `renderScopeInto(ctx, 0, 0, W, H, …)` over a whole canvas
 * plus the fractional-devicePixelRatio backing-store fix — one copy of that
 * sizing logic beats two. */
function scopeWidget(sid: number, deps: PanelHostDeps): Widget {
  const { el, lab, ctl, val } = makeRow(`sc:${sid}`, `SCOPE ${sid}`);
  el.classList.add('prow-scope');

  const chans = document.createElement('div');
  chans.className = 'pchans';
  chans.title = 'channels: click a probe to show/hide it';
  const kill = document.createElement('button');
  kill.className = 'ptog';
  kill.textContent = '×';
  kill.title = 'delete this oscilloscope';
  kill.onclick = () => deps.removeScope(sid);
  ctl.append(chans, kill);

  const cv = document.createElement('canvas');
  cv.className = 'pscope';
  el.appendChild(cv);

  let scope: FloatScope | undefined;
  let probes: Probe[] = [];
  const active = () => (scope ? scopeChannels(scope, probes) : []);

  /** Body-local coordinates: `.pscope` has no border or padding, so the
   * element's own box is exactly the rect renderScopeInto drew into. */
  const ctrlAt = (ev: { clientX: number; clientY: number }): ScopeControlId | null => {
    if (!scope) return null;
    const r = cv.getBoundingClientRect();
    return scopeControlAt(
      cv.clientWidth,
      cv.clientHeight,
      ev.clientX - r.left,
      ev.clientY - r.top,
      scope.set,
      active().length,
    );
  };
  cv.addEventListener('pointerdown', (ev) => {
    const id = ctrlAt(ev);
    if (!scope || !id) return;
    ev.preventDefault();
    applyScopeControl(scope.set, id, active().length);
    deps.scopeChanged(scope);
  });
  cv.addEventListener('pointermove', (ev) => {
    cv.style.cursor = ctrlAt(ev) ? 'pointer' : 'default';
  });
  // Wheel over a scope is its timebase, wherever the scope is drawn.
  cv.addEventListener(
    'wheel',
    (ev) => {
      if (!scope) return;
      ev.preventDefault();
      ev.stopPropagation();
      scope.set.timebase = clamp(scope.set.timebase * Math.exp(ev.deltaY * 0.001), 0.001, 60);
      deps.scopeChanged(scope);
    },
    { passive: false },
  );

  const toggle = (pid: number) => {
    const s = scope;
    if (!s) return;
    if (s.pids === null) s.pids = probes.map((p) => p.pid);
    s.pids = s.pids.includes(pid) ? s.pids.filter((x) => x !== pid) : [...s.pids, pid];
    deps.scopeChanged(s);
  };

  // The canvas scope's channel dots live in its title bar, which a panel-owned
  // scope does not have: rebuild them here whenever the selection changes.
  let chanSig = '';
  const syncChans = (netNames: NetNames) => {
    const on = new Set(active().map((p) => p.pid));
    // The net name is part of the signature: renaming a net has to re-title
    // the dots, or the tooltip keeps saying what the wire used to be called.
    const sig =
      `${probes.map((p) => `${p.pid}${p.kind}:${netNames?.get(p.pid) ?? ''}`).join(',')}` +
      `|${[...on].join(',')}`;
    if (sig === chanSig) return;
    chanSig = sig;
    chans.replaceChildren(
      ...probes.map((p) => {
        const b = document.createElement('button');
        b.className = on.has(p.pid) ? 'pchan on' : 'pchan';
        b.style.color = probeColor(p.pid);
        b.title = channelName(p, netNames);
        b.onclick = () => toggle(p.pid);
        return b;
      }),
    );
  };

  return {
    key: `sc:${sid}`,
    el,
    update(ctx) {
      scope = ctx.scopes.find((s) => s.sid === sid);
      probes = ctx.probes;
      if (!scope) return;
      lab.textContent = `SCOPE ${sid}`;
      syncChans(ctx.netNames);
      val.textContent = `${fmtEng(scope.set.timebase / 10, 's', { trim: true })}/div`;
      renderScope(cv, ctx.traces, active(), scope.set.timebase, scope.set, ctx.netNames);
    },
  };
}

type WidgetSpec = { key: string; make: () => Widget };

/** Which widgets a panel's members deserve, in document order. */
function widgetSpecs(
  plid: number,
  members: ElementSpec[],
  probes: Probe[],
  scopes: FloatScope[],
  deps: PanelHostDeps,
): WidgetSpec[] {
  const out: WidgetSpec[] = [];
  const ids = new Set(members.map((e) => e.id));
  for (const e of members) {
    const id = e.id;
    switch (e.kind.t) {
      case 'Potentiometer':
        out.push({ key: `pot:${id}`, make: () => potWidget(plid, id, deps) });
        break;
      case 'Switch':
        out.push({ key: `sw:${id}`, make: () => switchWidget(id, deps) });
        break;
      case 'Lamp':
      case 'Led':
        out.push({ key: `ind:${id}`, make: () => indicatorWidget(id) });
        break;
      case 'VoltageSource': {
        // AC sources have no single knob to offer; DC ones do.
        const dc = e.kind.dc;
        if (e.kind.amp === 0) out.push({ key: `src:${id}`, make: () => sourceWidget(id, dc, deps) });
        break;
      }
      default:
        break;
    }
  }
  for (const p of probes) {
    if (ids.has(p.elem)) out.push({ key: `pr:${p.pid}`, make: () => probeWidget(p.pid) });
  }
  // Oscilloscopes the region encloses; keyed by sid so the saved row order
  // treats them like any other widget.
  for (const s of scopes) out.push({ key: `sc:${s.sid}`, make: () => scopeWidget(s.sid, deps) });
  return out;
}

// -------------------------------------------------------- control windows

/** Narrowest the name field ever gets, px. */
const TITLE_MIN_W = 46;
/** Header space kept clear of the name field so there is always a strip to
 * grab, px. */
const DRAG_MIN_W = 34;

class PanelWindow {
  readonly el: HTMLDivElement;
  private title: HTMLInputElement;
  private body: HTMLDivElement;
  private hd: HTMLDivElement;
  private grab: HTMLSpanElement;
  private close: HTMLButtonElement;
  /** Hidden twin of the title, used to measure the text width. */
  private mez: HTMLSpanElement;
  /** Last (text, focus) the field was sized for — layout reads are not free. */
  private fitKey = '';
  private widgets = new Map<string, Widget>();
  private sig = '';
  private name = '';
  /** Header buttons: fold the body away, dock/undock. */
  private shut: HTMLButtonElement;
  private dockBtn: HTMLButtonElement;
  /** Last placement applied, as a key. Diffed so re-running applyRails is
   * free; cleared when the DOM parent is changed behind its back. */
  private placeKey = '';
  /** false inside a collapsed rail: the window does no work at all. */
  private shown = true;
  /** Folded (header only). Per-player, like every other window preference. */
  private folded: boolean;
  /** Latest probe list — hover resolution for `pr:` rows, on demand. */
  private lastProbes: Probe[] = [];
  /** The elements this region currently contains. Borrowed from the array
   * `update` already builds, so hovering allocates nothing new. */
  private lastMembers: ElementSpec[] = [];

  constructor(
    private plid: number,
    private deps: PanelHostDeps,
    private host: WinHost,
  ) {
    this.el = document.createElement('div');
    this.el.className = 'pwin';
    const hd = document.createElement('div');
    hd.className = 'pwin-hd';
    this.hd = hd;
    const grab = document.createElement('span');
    grab.className = 'pwin-grab';
    grab.textContent = '⣿';
    this.grab = grab;
    this.title = document.createElement('input');
    this.title.className = 'pwin-title';
    this.title.spellcheck = false;
    this.title.title = 'click to rename · ⌘/ctrl+drag moves the window';
    this.mez = document.createElement('span');
    this.mez.className = 'pwin-mez';
    this.mez.setAttribute('aria-hidden', 'true');
    this.folded = lsGet(`${plid}:shut`) === '1';
    const shut = document.createElement('button');
    shut.className = 'pwin-shut';
    shut.onclick = () => {
      this.folded = !this.folded;
      lsSet(`${plid}:shut`, this.folded ? '1' : '0');
      this.applyFold();
      shut.blur();
    };
    this.shut = shut;
    const dockBtn = document.createElement('button');
    dockBtn.className = 'pwin-dock';
    dockBtn.onclick = () => {
      const side = dockSideOf(plid);
      // Floating: dock to whichever rail this window is already nearer.
      host.dockTo(
        plid,
        side ? null : this.el.offsetLeft + this.el.offsetWidth / 2 > window.innerWidth / 2
          ? 'right'
          : 'left',
      );
      dockBtn.blur();
    };
    this.dockBtn = dockBtn;
    const close = document.createElement('button');
    close.className = 'pwin-x';
    close.textContent = '×';
    close.title = 'delete this panel';
    close.onclick = () => deps.op({ t: 'remove', plid });
    this.close = close;
    hd.append(grab, this.title, this.mez, shut, dockBtn, close);
    this.applyFold();
    this.setDockGlyph(null);

    this.body = document.createElement('div');
    this.body.className = 'pwin-body';
    this.el.append(hd, this.body);
    host.root.appendChild(this.el);

    const pos = readPos(plid);
    this.el.style.left = `${pos.x}px`;
    this.el.style.top = `${pos.y}px`;
    this.placeKey = 'float';

    // Pointing anywhere in this window says "the parts on it": weak canvas
    // emphasis over every member, plus its region goes hot. A row's own
    // enter fires afterwards and overwrites it with the strong single-part
    // highlight, so the more specific gesture always wins.
    this.el.addEventListener('pointerenter', () => this.hoverPanel());
    this.el.addEventListener('pointerleave', () => host.setHover(null));

    // Title-bar drag. The name field is content-sized, so the header space
    // to its right is a real drag strip; ⌘/ctrl held drags from ANYWHERE in
    // the header, the name included, instead of editing it. Dragging past
    // the dead zone LIFTS the window: docked, that is how it leaves a rail;
    // released over one, that is how it joins.
    let lastPress = -1e9;
    hd.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      const force = ev.ctrlKey || ev.metaKey;
      const btn = ev.target === close || ev.target === shut || ev.target === dockBtn;
      if (!force && (btn || ev.target === this.title)) return;
      if (force && btn) return; // ⌘+× still deletes
      ev.preventDefault();
      // Double-press on the bar itself renames. Counted here rather than via
      // dblclick because the drag's preventDefault can swallow that event.
      const dbl = ev.timeStamp - lastPress < 350;
      lastPress = ev.timeStamp;
      if (dbl && !force) {
        this.title.focus();
        this.title.select();
        return;
      }
      // Grabbing the bar leaves the name field: preventDefault above means
      // the browser will not blur it for us, and a focused field would keep
      // swallowing canvas hotkeys (panelHost.owns).
      if (document.activeElement === this.title) this.title.blur();
      host.setHover(null);
      const [sx, sy] = [ev.clientX, ev.clientY];
      const pid = ev.pointerId;
      const from: Placement = this.currentPlacement();
      let lifted = false;
      let done = false;
      let held = false;
      let gx = 0;
      let gy = 0;

      // A docked window is a static flex child: the ONE layout read in this
      // gesture takes its on-screen box, then beginDrag hands it back to the
      // floating layer at exactly that spot, so it does not jump on grab.
      const lift = () => {
        const r = this.el.getBoundingClientRect();
        gx = sx - r.left;
        gy = sy - r.top;
        // Undocks + arms both rails. This REPARENTS the window out of the
        // rail list — which is exactly why nothing in this gesture may be
        // bound to the window's own DOM (see the listeners below).
        host.beginDrag(plid, pid, () => finish(true));
        this.el.classList.add('dragging');
        this.el.style.left = `${r.left}px`;
        this.el.style.top = `${r.top}px`;
        lifted = true;
      };

      const move = (m: PointerEvent) => {
        if (m.pointerId !== pid || done) return;
        if (m.buttons !== 0) held = true;
        // The button came up somewhere this gesture never saw (a native drag
        // took over, a capture was stolen, a window lost focus mid-press).
        // A move with nothing pressed IS the drop: never keep flying.
        else if (held) return finish(false);
        if (!lifted) {
          if (
            Math.abs(m.clientX - sx) <= DRAG_DEAD_PX &&
            Math.abs(m.clientY - sy) <= DRAG_DEAD_PX
          ) {
            return;
          }
          lift();
        }
        this.el.style.left = `${clamp(m.clientX - gx, 0, window.innerWidth - 90)}px`;
        this.el.style.top = `${clamp(m.clientY - gy, 0, window.innerHeight - 30)}px`;
        host.aimDrop(m.clientX, m.clientY);
      };

      const unbind = () => {
        window.removeEventListener('pointermove', move, true);
        window.removeEventListener('pointerup', up, true);
        window.removeEventListener('pointercancel', cancelled, true);
        window.removeEventListener('keydown', esc, true);
      };

      /** The single exit. Idempotent, and it ALWAYS hands the rails back:
       * whatever happens in between, endDrag runs from the finally. */
      const finish = (cancel: boolean) => {
        if (done) return;
        done = true;
        unbind();
        if (!lifted) return; // a click, not a drag: nothing moved
        let drop: { side: RailSide; index: number } | null = null;
        try {
          this.el.classList.remove('dragging');
          drop = host.dropTarget();
          if (cancel) {
            // Escape restores exactly where it was — rail slot or float spot.
            drop = from.at === 'rail' ? { side: from.side, index: from.index } : null;
            if (from.at === 'float') {
              this.el.style.left = `${from.x}px`;
              this.el.style.top = `${from.y}px`;
            }
          } else if (!drop) {
            lsSet(`${plid}:pos`, JSON.stringify({ x: this.el.offsetLeft, y: this.el.offsetTop }));
          }
        } catch (err) {
          // Losing the drop is survivable; leaving the HUD armed is not.
          console.error('[panel] drag settle failed', err);
          drop = from.at === 'rail' ? { side: from.side, index: from.index } : null;
        } finally {
          host.endDrag(plid, drop);
        }
      };
      const up = (m: PointerEvent) => {
        if (m.pointerId === pid) finish(false);
      };
      const cancelled = (m: PointerEvent) => {
        if (m.pointerId === pid) finish(true);
      };
      const esc = (k: KeyboardEvent) => {
        if (k.key !== 'Escape') return;
        // Cancelling a drag is a layer AHEAD of the canvas Escape ladder.
        k.preventDefault();
        k.stopPropagation();
        finish(true);
      };
      // On WINDOW, in the capture phase — never on the header. Lifting the
      // window reparents it into or out of a rail, and a reparented node
      // drops its pointer capture, so listeners bound to the header would
      // from then on only fire while the cursor happened to be inside its
      // ~29 px band: any drag quicker than that per event would freeze
      // mid-gesture. A drag must not care where its element lives in the
      // DOM. setPointerCapture is an optimisation, not a contract, so this
      // gesture no longer takes one at all.
      window.addEventListener('pointermove', move, true);
      window.addEventListener('pointerup', up, true);
      window.addEventListener('pointercancel', cancelled, true);
      window.addEventListener('keydown', esc, true);
    });

    // A modified press on the name must not steal focus — mousedown is where
    // focus is decided, so that is where it has to be refused.
    this.title.addEventListener('mousedown', (ev) => {
      if (ev.ctrlKey || ev.metaKey) ev.preventDefault();
    });
    // macOS turns ctrl+click into a context menu; the ctrl-drag owns it here.
    hd.addEventListener('contextmenu', (ev) => {
      if (ev.ctrlKey || ev.metaKey) ev.preventDefault();
    });
    this.title.addEventListener('change', () => {
      const v = this.title.value.trim();
      if (v && v !== this.name) deps.op({ t: 'rename', plid, name: v });
      else this.title.value = this.name;
    });
    this.title.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') this.title.blur();
      if (ev.key === 'Escape') {
        this.title.value = this.name;
        this.title.blur();
      }
    });
    // The drag strip has to track the text as it changes.
    for (const evt of ['input', 'focus', 'blur', 'change'] as const) {
      this.title.addEventListener(evt, () => this.fitTitle());
    }
    this.fitTitle();
  }

  destroy() {
    this.el.remove();
  }

  // ------------------------------------------------------------ placement

  /** Where this window is right now. Float carries its pixel spot so an
   * Escape mid-drag can put it back exactly. */
  currentPlacement(): Placement {
    const side = dockSideOf(this.plid);
    if (side) {
      const i = ensureRails()[side].order.indexOf(this.plid);
      return { at: 'rail', side, index: Math.max(0, i) };
    }
    return { at: 'float', x: this.el.offsetLeft, y: this.el.offsetTop };
  }

  /** Move the window to where the rail state says it belongs. Diffed against
   * the last applied placement, so calling this every time anything changes
   * costs one string compare per window. */
  setPlacement(p: Placement, rail: RailState | null) {
    const key = p.at === 'float' ? 'float' : `${p.side}:${p.index}`;
    if (key === this.placeKey) return;
    this.placeKey = key;
    if (p.at === 'rail' && rail) {
      this.el.classList.add('docked');
      this.el.style.left = '';
      this.el.style.top = '';
      let seen = 0;
      let before: Element | null = null;
      for (const c of rail.list.children) {
        if (c === rail.caretLine || c === this.el) continue;
        if (seen++ === p.index) {
          before = c;
          break;
        }
      }
      rail.list.insertBefore(this.el, before);
      this.setDockGlyph(p.side);
    } else {
      this.el.classList.remove('docked');
      if (this.el.parentElement !== this.host.root) this.host.root.appendChild(this.el);
      const pos = readPos(this.plid);
      this.el.style.left = `${pos.x}px`;
      this.el.style.top = `${pos.y}px`;
      this.setDockGlyph(null);
    }
  }

  /** A window in a collapsed rail is display:none. It stops updating
   * entirely — no DOM writes, no scope re-render, and fitTitle never hits
   * its unlaid-out fallback. Clearing `sig` forces one rebuild on re-show,
   * so a membership change that happened while hidden still lands. */
  setShown(v: boolean) {
    if (v === this.shown) return;
    this.shown = v;
    if (!v) this.sig = '';
  }

  get visible() {
    return this.shown;
  }

  private setDockGlyph(side: RailSide | null) {
    this.dockBtn.textContent = side ? '⇱' : '⇥';
    const label = side
      ? `pop this panel out of the ${side} sidebar`
      : 'dock this panel into a sidebar (or drag its header there)';
    this.dockBtn.title = label;
    this.dockBtn.setAttribute('aria-label', label);
  }

  private applyFold() {
    this.el.classList.toggle('shut', this.folded);
    this.shut.textContent = this.folded ? '▸' : '▾';
    const label = this.folded ? 'show this panel’s controls' : 'fold this panel to its title bar';
    this.shut.title = label;
    this.shut.setAttribute('aria-label', label);
  }

  // ---------------------------------------------------------- highlighting

  /** Element ids a row points at. Resolved on hover, never per frame. */
  private rowIds(key: string): number[] {
    const c = key.indexOf(':');
    if (c < 0) return [];
    const t = key.slice(0, c);
    const n = Number(key.slice(c + 1));
    if (!Number.isFinite(n)) return [];
    if (t === 'pot' || t === 'sw' || t === 'ind' || t === 'src') return [n];
    if (t === 'pr') {
      const p = this.lastProbes.find((q) => q.pid === n);
      if (!p) return [];
      // A differential probe measures BETWEEN two parts: light both.
      return p.r ? [p.elem, p.r[0]] : [p.elem];
    }
    return []; // 'sc' — a scope owns no element of its own
  }

  private probeElem(key: string): number | null {
    const n = Number(key.slice(3));
    return this.lastProbes.find((q) => q.pid === n)?.elem ?? null;
  }

  private hoverPanel() {
    const ids: number[] = [];
    for (const e of this.lastMembers) ids.push(e.id);
    this.host.setHover({ ids, plid: this.plid, kind: 'panel' });
  }

  /** Rows are hover targets: pointing at one (or tabbing into its control)
   * says "that part, out there". focusin/focusout is the keyboard path and
   * adds no tab stops — the sliders and toggles are already focusable. */
  private wireRowHover(row: HTMLDivElement) {
    if (row.dataset.hoverWired) return;
    row.dataset.hoverWired = '1';
    const on = () => {
      const key = row.dataset.key ?? '';
      this.host.setHover({ ids: this.rowIds(key), plid: this.plid, kind: 'row' });
    };
    // Leaving a row still leaves the pointer inside the window, so fall back
    // to the whole-panel highlight rather than clearing it.
    row.addEventListener('pointerenter', on);
    row.addEventListener('pointerleave', () => this.hoverPanel());
    row.addEventListener('focusin', on);
    row.addEventListener('focusout', () => this.host.setHover(null));
  }

  /** The other direction: the CANVAS pointer landed on element `id`, so the
   * row that controls it reads hot. Cyan here on purpose — this says "that
   * part belongs to this panel", which is region vocabulary. */
  setHotRow(id: number | null) {
    const suffix = `:${id}`;
    let any = false;
    for (const [key, w] of this.widgets) {
      const hot =
        id !== null &&
        (key.startsWith('pr:') ? this.probeElem(key) === id : !key.startsWith('sc:') && key.endsWith(suffix));
      w.el.classList.toggle('hot', hot);
      any = any || hot;
    }
    this.el.classList.toggle('hot', any);
  }

  /** Size the name field to its own text (clamped), leaving the rest of the
   * header as drag surface. Measured with the hidden twin so the CSS font,
   * letter-spacing and uppercase transform are all accounted for. */
  private fitTitle() {
    const focused = document.activeElement === this.title;
    // JSON rather than a NUL separator. The NUL was unambiguous, but it made
    // the whole FILE read as binary to grep and friends, which silently
    // disables a tool nobody expects to be disabled.
    const key = JSON.stringify([this.title.value, focused]);
    if (key === this.fitKey) return;
    this.fitKey = key;
    // A focused field shows the raw text; unfocused it is uppercased by CSS.
    this.mez.classList.toggle('raw', focused);
    this.mez.textContent = this.title.value || ' ';
    // The fold button carries margin-left:auto, so its left edge marks the
    // end of the usable strip whatever the field's current width is.
    const laid = this.hd.getBoundingClientRect().width > 0;
    const room = laid
      ? this.shut.getBoundingClientRect().left - this.grab.getBoundingClientRect().right - DRAG_MIN_W
      : 140;
    const want = Math.ceil(this.mez.getBoundingClientRect().width) + 6;
    const cap = Math.max(TITLE_MIN_W, room);
    this.title.style.width = `${Math.round(Math.max(TITLE_MIN_W, Math.min(want, cap)))}px`;
  }

  update(panel: Panel, ctx: TickCtx) {
    // Hidden inside a collapsed rail: do nothing at all this frame.
    if (!this.shown) return;
    this.name = panel.name;
    if (document.activeElement !== this.title) this.title.value = panel.name;
    this.fitTitle();
    // Borrowed, not copied: hover resolution reads this array on demand.
    this.lastProbes = ctx.probes;
    this.lastMembers = panelMembers(panel, ctx.elems);
    const specs = widgetSpecs(
      this.plid,
      this.lastMembers,
      ctx.probes,
      panelScopes(panel, ctx.scopes, ctx.panels),
      this.deps,
    );
    const sig = specs.map((s) => s.key).join('|');
    if (sig !== this.sig) {
      this.sig = sig;
      this.rebuild(specs);
    }
    for (const w of this.widgets.values()) w.update(ctx);
    // Nothing to control: show the title bar and nothing else. This is
    // rendering state, NOT the player's own shut/open choice (`:shut`), so a
    // panel they left open springs back the moment a part lands inside it.
    this.el.classList.toggle('empty', this.widgets.size === 0);
  }

  /** Member set changed: re-lay the rows, honoring the saved drag order and
   * keeping live widget elements (so a slider mid-drag survives). */
  private rebuild(specs: WidgetSpec[]) {
    const order = readOrder(this.plid);
    const rank = (k: string) => {
      const i = order.indexOf(k);
      return i < 0 ? order.length + 1 : i;
    };
    const sorted = [...specs].sort(
      (a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key),
    );
    const old = this.widgets;
    this.widgets = new Map();
    for (const s of sorted) {
      const w = old.get(s.key) ?? s.make();
      this.widgets.set(s.key, w);
      this.wireRowDrag(w.el);
      this.wireRowHover(w.el);
      this.body.appendChild(w.el); // appending an existing child re-orders it
    }
    for (const [key, w] of old) if (!this.widgets.has(key)) w.el.remove();
  }

  /** Rows are re-orderable by dragging their grip; order persists per plid. */
  private wireRowDrag(row: HTMLDivElement) {
    const grip = row.querySelector('.prow-grip');
    if (!(grip instanceof HTMLElement) || grip.dataset.wired) return;
    grip.dataset.wired = '1';
    grip.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      const pid = ev.pointerId;
      let done = false;
      let held = false;
      row.classList.add('dragging');
      // Same rule as the header drag: this gesture MOVES the element it was
      // started from, so it listens on window, not on the grip. A row that
      // slid out from under its own listeners would stay `.dragging` for
      // ever and never persist its new order.
      const move = (m: PointerEvent) => {
        if (m.pointerId !== pid || done) return;
        if (m.buttons !== 0) held = true;
        else if (held) return up(m); // the release happened out of our sight
        let before: HTMLElement | null = null;
        for (const c of this.body.children) {
          if (!(c instanceof HTMLElement) || c === row) continue;
          const b = c.getBoundingClientRect();
          if (m.clientY < b.top + b.height / 2) {
            before = c;
            break;
          }
        }
        this.body.insertBefore(row, before);
      };
      const up = (m: PointerEvent) => {
        if (m.pointerId !== pid || done) return;
        done = true;
        window.removeEventListener('pointermove', move, true);
        window.removeEventListener('pointerup', up, true);
        window.removeEventListener('pointercancel', up, true);
        row.classList.remove('dragging');
        this.persistOrder();
      };
      window.addEventListener('pointermove', move, true);
      window.addEventListener('pointerup', up, true);
      window.addEventListener('pointercancel', up, true);
    });
  }

  private persistOrder() {
    const keys: string[] = [];
    for (const c of this.body.children) {
      if (c instanceof HTMLElement && c.dataset.key) keys.push(c.dataset.key);
    }
    lsSet(`${this.plid}:order`, JSON.stringify(keys));
    const reordered = new Map<string, Widget>();
    for (const k of keys) {
      const w = this.widgets.get(k);
      if (w) reordered.set(k, w);
    }
    this.widgets = reordered;
  }
}

/** Owns every panel control window: one per shared panel region, plus the
 * two HUD rails they can be docked into. */
export class PanelHost implements WinHost {
  readonly root: HTMLElement;
  private wins = new Map<number, PanelWindow>();
  /** Plids in the shared list right now, or null before the first tick. */
  private alive: Set<number> | null = null;
  /** A NON-EMPTY shared list has arrived at least once. Until it has, the
   * empty list is "not loaded yet", not "every panel was deleted": pruning
   * the saved rail order against it would wipe the layout on every reload,
   * in the frames between boot and the server's hello. */
  private seenPanels = false;
  /** Last published rail insets, so --rail-l/r are written only on change. */
  private insL = 0;
  private insR = 0;
  /** THE drag session. At most one exists, and it lives exactly as long as
   * the pointer gesture that owns it — see beginDrag / abortDrag / tick. */
  private drag: {
    plid: number;
    pointerId: number;
    drop: { side: RailSide; index: number } | null;
    dwellSide: RailSide | null;
    dwellTimer: number;
    abort: () => void;
  } | null = null;
  private canvasHover: number | null = null;
  private hovering = false;

  constructor(private deps: PanelHostDeps) {
    const found = document.getElementById('panels');
    if (found) {
      this.root = found;
    } else {
      const d = document.createElement('div');
      d.id = 'panels';
      document.body.appendChild(d);
      this.root = d;
    }
    ensureRails();
    onRailsChanged = () => this.applyRails();
    this.applyRails();

    // [ and ] toggle the rails. Registered here rather than in main.ts's
    // keydown block so this feature adds nothing to the most contested
    // region of that file; `owns` keeps it out of the rename field.
    window.addEventListener('keydown', (ev) => {
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      if (ev.key !== '[' && ev.key !== ']') return;
      if (this.owns(ev.target) || ev.target instanceof HTMLInputElement) return;
      ev.preventDefault();
      this.toggleRail(ev.key === '[' ? 'left' : 'right');
    });
    // Starting anything on the canvas must not leave a highlight behind.
    window.addEventListener(
      'pointerdown',
      (ev) => {
        if (!this.owns(ev.target)) this.setHover(null);
      },
      true,
    );
    window.addEventListener('blur', () => {
      this.setHover(null);
      this.abortDrag('the window lost focus');
    });
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) this.abortDrag('the tab went away');
    });
    // Backstop for the pointer itself. The gesture's own window listeners
    // are what normally settle a drag; if one of them somehow does not run,
    // the release still passed through here, and a session whose pointer is
    // gone is by definition orphaned. Deferred by a task so the ordinary
    // path always wins the race and this never fires for a healthy drag.
    const released = (ev: PointerEvent) => {
      const id = ev.pointerId;
      if (!this.drag || this.drag.pointerId !== id) return;
      setTimeout(() => {
        if (this.drag && this.drag.pointerId === id) this.abortDrag('its pointer was released');
      }, 0);
    };
    window.addEventListener('pointerup', released, true);
    window.addEventListener('pointercancel', released, true);
    // The rails are sized against the viewport, not against a constant: a
    // window narrow enough that both cannot fit has to re-fit them.
    let pending = 0;
    window.addEventListener('resize', () => {
      if (pending) return;
      pending = requestAnimationFrame(() => {
        pending = 0;
        this.applyRails();
      });
    });
  }

  /** True for events originating inside a panel window OR a rail — main.ts
   * uses this to keep canvas hotkeys out of panel text inputs, and the rails
   * live outside #panels. */
  owns(target: EventTarget | null): boolean {
    if (!(target instanceof Node)) return false;
    if (this.root.contains(target)) return true;
    const R = rails;
    return !!R && (R.left.el.contains(target) || R.right.el.contains(target));
  }

  // ---------------------------------------------------------------- rails

  /** Viewport each rail is eating, px. `fitRect` frames the canvas the
   * player can actually see rather than the part behind a sidebar. */
  railInsets(): { left: number; right: number } {
    return { left: this.insL, right: this.insR };
  }

  toggleRail(side: RailSide) {
    const r = ensureRails()[side];
    r.open = !railOpen(r);
    if (r.open) r.openedAt = ++openSeq;
    this.applyRails();
  }

  private setRailOpen(side: RailSide, v: boolean) {
    const r = ensureRails()[side];
    if (railOpen(r) === v) return;
    r.open = v;
    if (v) r.openedAt = ++openSeq;
    this.applyRails();
  }

  /** The only writer of rail DOM. Called on hydrate, dock, undock, reorder,
   * toggle, resize and the tick prune — never from the render loop. */
  private applyRails() {
    const R = ensureRails();
    let insL = 0;
    let insR = 0;
    // Pass 1: whose slots are real, and which sides are on screen at all.
    const live: Record<RailSide, number[]> = { left: [], right: [] };
    const disp: Record<RailSide, boolean> = { left: false, right: false };
    for (const side of RAIL_SIDES) {
      const r = R[side];
      // A slot whose panel is not in the shared list shows nothing and holds
      // no space — but it is only FORGOTTEN once we know the list is real,
      // so a saved layout survives the boot frames before hello lands.
      const ids = this.alive ? r.order.filter((plid) => this.alive!.has(plid)) : r.order;
      if (this.seenPanels && ids.length !== r.order.length) r.order = ids;
      live[side] = ids;
      disp[side] = ids.length > 0 || dragActive;
    }
    // Pass 2: the viewport decides how much of what they asked for they get.
    const fit = fitRails(R, disp);
    for (const side of RAIL_SIDES) {
      const r = R[side];
      r.folded = fit.folded[side];
      const px = disp[side] ? fit.px[side]! : railOpen(r) ? r.width : RAIL_BAR_PX;
      r.shownPx = disp[side] ? px : 0;
      const n = live[side].length;
      const open = railOpen(r);
      r.el.classList.toggle('on', disp[side]);
      r.el.classList.toggle('collapsed', !open);
      r.el.style.width = `${px}px`;
      r.bar.setAttribute('aria-expanded', open ? 'true' : 'false');
      // The caret points the way the strip would move.
      r.caret.textContent = open === (side === 'left') ? '◂' : '▸';
      r.count.textContent = n === 0 ? 'drop a panel here' : `${n} panel${n === 1 ? '' : 's'}`;
      r.bar.title = r.folded
        ? `expand the ${side} sidebar (${side === 'left' ? '[' : ']'}) — too narrow for both`
        : `${open ? 'collapse' : 'expand'} the ${side} sidebar (${side === 'left' ? '[' : ']'})`;
      for (let k = 0; k < n; k++) {
        const w = this.wins.get(live[side][k]!);
        if (!w) continue;
        w.setPlacement({ at: 'rail', side, index: k }, r);
        w.setShown(open);
      }
      // An empty rail eats nothing, however armed it looks mid-drag.
      if (side === 'left') insL = n > 0 ? px : 0;
      else insR = n > 0 ? px : 0;
      writeRailPrefs(r);
    }
    for (const [plid, w] of this.wins) {
      if (dockSideOf(plid)) continue;
      w.setPlacement({ at: 'float', x: 0, y: 0 }, null);
      w.setShown(true);
    }
    const css = document.documentElement.style;
    if (insL !== this.insL) {
      this.insL = insL;
      css.setProperty('--rail-l', `${insL}px`);
    }
    if (insR !== this.insR) {
      this.insR = insR;
      css.setProperty('--rail-r', `${insR}px`);
    }
  }

  // ------------------------------------------------------- docking gesture

  beginDrag(plid: number, pointerId: number, abort: () => void) {
    // One session, always. A drag that is somehow still open when another
    // starts is over — two lifted windows is exactly the stuck state.
    if (this.drag) this.abortDrag('a second drag started');
    const R = ensureRails();
    for (const s of RAIL_SIDES) R[s].order = R[s].order.filter((p) => p !== plid);
    dragActive = true;
    this.drag = { plid, pointerId, drop: null, dwellSide: null, dwellTimer: 0, abort };
    this.applyRails();
  }

  /** End the drag from OUTSIDE the gesture. The gesture's own cancel path
   * runs first (so the window goes back exactly where it came from); if it
   * declines to settle, the session is torn down regardless. Either way the
   * HUD comes out of this call consistent. */
  private abortDrag(why: string) {
    const d = this.drag;
    if (!d) return;
    console.warn(`[panel] drag ended by the host: ${why}`);
    try {
      d.abort();
    } catch (err) {
      console.error('[panel] drag abort failed', err);
    }
    if (this.drag === d) this.endDrag(d.plid, null);
  }

  aimDrop(x: number, y: number) {
    const d = this.drag;
    if (!d) return;
    const R = ensureRails();
    const side: RailSide | null =
      x <= railPx(R.left) + DROP_PAD_PX
        ? 'left'
        : x >= window.innerWidth - railPx(R.right) - DROP_PAD_PX
          ? 'right'
          : null;

    // Spring-loading: hold a window over a shut rail and it opens, folder
    // style, so you can aim at a slot inside it.
    if (d.dwellSide !== side) {
      d.dwellSide = side;
      clearTimeout(d.dwellTimer);
      if (side && !R[side].open) {
        d.dwellTimer = window.setTimeout(() => {
          if (this.drag && this.drag.dwellSide === side) this.setRailOpen(side, true);
        }, DWELL_MS);
      }
    }

    for (const s of RAIL_SIDES) {
      R[s].el.classList.toggle('armed', s === side);
      if (s !== side) R[s].caretLine.classList.remove('on');
    }
    if (!side) {
      d.drop = null;
      return;
    }
    const rail = R[side];
    if (!railOpen(rail)) {
      // Dropping onto a shut rail still docks: it lands at the end and the
      // count badge says where it went. A panel is never lost.
      rail.caretLine.classList.remove('on');
      d.drop = { side, index: rail.order.length };
      return;
    }
    let index = 0;
    let before: Element | null = null;
    for (const c of rail.list.children) {
      if (!(c instanceof HTMLElement) || c === rail.caretLine) continue;
      const b = c.getBoundingClientRect();
      if (y < b.top + b.height / 2) {
        before = c;
        break;
      }
      index++;
    }
    rail.list.insertBefore(rail.caretLine, before);
    rail.caretLine.classList.add('on');
    d.drop = { side, index };
  }

  dropTarget() {
    return this.drag?.drop ?? null;
  }

  /** The ONE way out of a drag, and it is total: it clears the session, the
   * spring timer, both armed borders, both carets and every lifted-window
   * class, whether or not a session was actually open and whatever `plid`
   * it is handed. Nothing it touches can be left half-done, so no caller
   * has to be careful for the HUD to end up consistent. */
  endDrag(plid: number, drop: { side: RailSide; index: number } | null) {
    const R = ensureRails();
    if (this.drag) clearTimeout(this.drag.dwellTimer);
    this.drag = null;
    dragActive = false;
    for (const s of RAIL_SIDES) {
      R[s].el.classList.remove('armed');
      R[s].caretLine.classList.remove('on');
      R[s].order = R[s].order.filter((p) => p !== plid);
    }
    for (const w of this.wins.values()) w.el.classList.remove('dragging');
    if (drop) {
      const o = R[drop.side].order;
      o.splice(clamp(drop.index, 0, o.length), 0, plid);
      if (!railOpen(R[drop.side])) this.flashRail(drop.side);
    }
    this.applyRails();
  }

  /** It went into a collapsed rail: say so, or it looks like it vanished.
   * Deliberately NOT the `armed` class — armed means "a window is in the air
   * and would land here", which is the state assertNotStuck polices. This
   * says "it landed here", and it is over in 400 ms. */
  private flashRail(side: RailSide) {
    const el = ensureRails()[side].el;
    el.classList.add('flash');
    window.setTimeout(() => el.classList.remove('flash'), 400);
  }

  dockTo(plid: number, side: RailSide | null) {
    const R = ensureRails();
    for (const s of RAIL_SIDES) R[s].order = R[s].order.filter((p) => p !== plid);
    if (side) {
      R[side].order.push(plid);
      if (!railOpen(R[side])) {
        R[side].open = true;
        R[side].openedAt = ++openSeq;
      }
    }
    this.applyRails();
  }

  // ------------------------------------------------------------- highlight

  setHover(h: PanelHover | null) {
    if (h === null && !this.hovering) return;
    this.hovering = h !== null;
    this.deps.hover(h);
  }

  /** Reverse direction: the id the CANVAS hover resolved to, or null. Guards
   * on an unchanged value, so main.ts may call it every frame. */
  setCanvasHover(id: number | null) {
    if (id === this.canvasHover) return;
    this.canvasHover = id;
    for (const w of this.wins.values()) w.setHotRow(id);
  }

  /** THE HUD INVARIANT, checked every frame.
   *
   * Mid-drag look — a phantom empty rail holding 300 px open, an armed cyan
   * border, a lit insertion caret, a window wearing `.dragging` — exists if
   * and only if there is a live drag session, and a session exists only
   * while the pointer gesture that made it is still in flight. So: no
   * session ⟹ none of that chrome, and a session whose window has gone ⟹
   * no session. Both directions are repaired here rather than merely
   * reported, because a HUD that lies about where a panel will land is
   * worse than one that briefly forgets a drag.
   *
   * This is the assertion that catches a stuck drag: whatever leaves the
   * gesture half-finished — an unforeseen event, a thrown exception, a
   * listener that never ran — is one frame away from being cleaned up. */
  private assertNotStuck() {
    const R = rails;
    if (!R) return;
    if (this.drag && !this.wins.has(this.drag.plid)) {
      this.abortDrag('the dragged window is gone');
      return;
    }
    if (this.drag) return; // a live session is allowed to look like one
    // Allocation-free: this runs every frame.
    let armed = dragActive;
    for (const s of RAIL_SIDES) {
      if (R[s].el.classList.contains('armed') || R[s].caretLine.classList.contains('on')) {
        armed = true;
      }
    }
    if (!armed) {
      for (const w of this.wins.values()) {
        if (w.el.classList.contains('dragging')) {
          armed = true;
          break;
        }
      }
    }
    if (!armed) return;
    console.error('[panel] HUD left in mid-drag state with no drag; settling');
    this.endDrag(-1, null);
  }

  /** Called once per frame: sync windows to the shared list, then refresh
   * every widget from the latest solver frame. */
  tick(panels: Panel[]) {
    this.assertNotStuck();
    const alive = new Set(panels.map((p) => p.plid));
    let churn = this.alive === null;
    for (const [plid, w] of [...this.wins]) {
      if (!alive.has(plid)) {
        w.destroy();
        this.wins.delete(plid);
        churn = true;
        if (this.hovering) this.setHover(null);
      }
    }
    this.alive = alive;
    if (panels.length > 0) this.seenPanels = true;
    if (panels.length === 0) {
      if (churn) this.applyRails();
      return;
    }
    const elems = this.deps.elements();
    const ctx: TickCtx = {
      elems,
      byId: new Map(elems.map((e) => [e.id, e])),
      live: this.deps.live(),
      probes: this.deps.probes(),
      panels,
      scopes: this.deps.scopes(),
      traces: this.deps.traces(),
      netNames: this.deps.netNames(),
    };
    for (const p of panels) {
      let w = this.wins.get(p.plid);
      if (!w) {
        // A region this browser has never met opens DOCKED, in the left rail:
        // a panel you just drew should land somewhere you can see it, not as
        // one more floating window over the schematic.
        //
        // `:seen` records only that we have met the panel — the two `order`
        // arrays stay the single truth about WHERE its window lives. Without
        // it, a panel deliberately undocked to float (which removes it from
        // both arrays) would read as new again on the next reload and get
        // re-docked under the player.
        if (lsGet(`${p.plid}:seen`) !== '1') {
          lsSet(`${p.plid}:seen`, '1');
          const R = ensureRails();
          if (!R.left.order.includes(p.plid) && !R.right.order.includes(p.plid)) {
            R.left.order.push(p.plid);
            // Docking into a rail the player has shut would file the new
            // panel out of sight, which reads as "nothing happened".
            R.left.open = true;
            writeRailPrefs(R.left);
          }
        }
        w = new PanelWindow(p.plid, this.deps, this);
        this.wins.set(p.plid, w);
        churn = true;
      }
    }
    // New or removed windows: re-seat everything against the rail order
    // before any of them draws, so a restored dock is never a visible flash.
    if (churn) this.applyRails();
    for (const p of panels) {
      const w = this.wins.get(p.plid);
      if (w && w.visible) w.update(p, ctx);
    }
  }
}
