// EE Game client. The sim runs the moment the page loads — no run button.
// Online: this browser renders the server's authoritative sim and sends
// interactions/edits. Offline: the same engine runs locally in WASM.
//
// Controls (Falstad-style):
//   letter keys           arm a part for placement (R resistor, L inductor,
//                         C capacitor, W wire, G ground, V battery, F AC,
//                         I current src, D diode, Z zener, E LED, N npn,
//                         P pnp, M nmos, shift+M pmos, A op-amp, S switch,
//                         B lamp, T pot) — click places (Q rotates first),
//                         drag places with drag orientation, Esc exits
//   click                 select part / probe flag;  drag on empty = marquee
//   shift+click/drag      ADD to the selection (never removes)
//   alt+click/drag        REMOVE from the selection (ctrl is map panning)
//   probe flag            Del removes it; right-click = delete/reference/listen
//   drag a part body      move it (whole selection if it is in one)
//   drag a machine        the freight hoist is a CHIP: nine pins on legs
//                         outside a package whose face is the whole handle.
//                         Drag anywhere on the body to move the assembly; its
//                         four fixtures travel with it and ⌘Z undoes the whole
//                         gesture. Its pins still draw wires and its children
//                         still select individually; shift+drag still marquees.
//                         The ⓘ badge on its title band opens its datasheet.
//   double-click a part   floating property editor next to it
//   right-click           cascading context menu — on a part:
//                         edit/rotate/delete/probe/listen/copy; on empty
//                         canvas: Add part ▸ category ▸ part, paste, scope,
//                         panel, select all.  There is no side palette.
//   ? or /                toggle the help block in the HUD
//   drag from a pin       reshape that part: stretch/shrink/reorient it by
//                         carrying the pin (wires come from W + drag; pins
//                         must overlap to connect)
//   TOUCH: two fingers    the camera, always and in every state — a pinch
//                         zooms about its own centroid and pans by however far
//                         that centroid travels, one gesture. A SECOND FINGER
//                         LANDING CANCELS AND ROLLS BACK whatever the first
//                         had begun, so a pinch can never half-place a part.
//                         One finger still means exactly what a mouse press
//                         means above; there is no keyboard on glass yet, so
//                         arming a part still needs one (the palette is a
//                         later stage).
//   ⌘/Ctrl+C, ⌘/Ctrl+V    copy selection / paste bound to cursor
//   ⌘/Ctrl+Z, +Shift, ^Y  undo / redo — this player's own edits only
//   Q                     rotate placement ghost, paste ghost, or selection
//   X / Y                 mirror the paste ghost or selection left-right (X)
//                         or top-bottom (Y), about its own centroid
//   1 / 2                 voltage probe / current clamp at hover
//   3                     listen: play that node's waveform (WebAudio)
//                         — Speakers need no probe: every Speaker element in
//                         the document is streamed to the mixer on its own
//                         12.5 kHz tap, muted/soloed from its right-click
//                         menu, with global mute + volume in the scope bar
//   0                     set selected V-probe's reference (differential)
//   O                     drop an in-place oscilloscope;  Del/Backspace delete
//   ` (backquote)         collapse/expand the bottom scope dock (starts collapsed)
//   K                     repair tool: the wrench cursor; click a charred
//                         part to put it back into service (parts break when
//                         you overload them — the server decides, from the
//                         solver, and says so with a toast)
//   Y / ⇧Y                the external-input pair. ⇧Y drags out a CAMERA
//                         LAYER — a rectangle of the world you point a real
//                         camera at (click its plate to allow the camera;
//                         that click is the only thing in this app that ever
//                         asks for one, and clicking again stops it). Y
//                         places a PHOTOCELL: drop it on the layer and it
//                         reads the light in the patch it covers, which the
//                         server turns into a real resistance in the real
//                         solve. No pixels leave your browser — the wire
//                         carries one integer per cell per tick, and the
//                         other players see the reading, never the picture.
//                         Every external input is a 30 Hz signal (15 Hz of
//                         bandwidth): loudness can dim a lamp, a whistle
//                         cannot be a waveform.
//   J                     drag a control-panel region around some parts
//                         (its floating instrument window follows) — a scope
//                         parked inside a region becomes a widget in that
//                         panel's window; drag it out to detach it again
//   ⇧J                    a LABEL BOX: the same drag, and only words. A box
//                         with a title that says what the parts inside it
//                         are. No window, no widget list, no membership —
//                         drawing one round a part changes nothing about
//                         that part. Double-click the title to rename, ×
//                         (or Del with the pointer on it) deletes, the grips
//                         resize. Shared and saved.
//   ⇧W                    NAME A NET: click a point on it. The name is drawn
//                         on the wire and shown wherever that net is
//                         reported (probe rows, scope chips, dock chips).
//                         It is a LABEL and nothing else — the same name on
//                         two separate nets joins them by exactly nothing.
//                         The anchor is a grid POINT, so no edit can destroy
//                         it: delete everything under it and it goes dimmed
//                         and dashed, still saying what you wrote, until
//                         something is connected there again. Shift+click
//                         deletes (so does Del), drag re-anchors,
//                         double-click renames.
//   H / shift+H           frame the home district / the whole document
//   ⇧R                    the room browser: which room you are in, every
//                         other room on the server (join / rename / delete),
//                         and "new room" from a TEMPLATE — a whole room
//                         setup, parts + panels + scope channels + camera +
//                         whether it comes with a machine. The chip in the
//                         top-right corner always says where you are and
//                         opens the same browser; a room switch reconnects
//                         the socket in place, it never reloads the page.
//   wheel zoom (over a scope: timebase) · pan: middle-drag, ctrl+drag, space+drag
//
// The world is large (tens of thousands of parts): the draw loop and the
// pointer hit-tests go through the grid-space spatial index in spatial.ts,
// and the zoom band 0.4..200 px/unit drops symbol detail below ~6 px/unit.

import init, { Sim, checkEdit, frameStride } from './wasm/sim_wasm';
import {
  demoCircuit,
  MAX_PINS,
  MAX_NAME,
  FRAME_STRIDE,
  type Wave,
  unpackFrame,
  type DocOp,
  type ElementKind,
  type ElementSpec,
  type ElemLive,
  type GateOp,
  type InteractOp,
  type Point,
} from './circuit';
import { AudioPlayer } from './audio';
import {
  CATALOG,
  CATEGORIES,
  isRigidPlacement,
  makePins,
  partsInCategory,
  pinCount,
  pinGesture,
  pinPivot,
  reshapePins,
  rigidHint,
  straightenPins,
  type PartDef,
  type PinGesture,
} from './catalog';
import {
  applyLabelBoxOp,
  applyNetLabelOp,
  drawLabelBoxes,
  drawLabelBoxGhost,
  drawNetLabels,
  emptyNetMap,
  labelBoxHotAt,
  labelBoxTitleAnchor,
  labelBoxZoneAt,
  MAX_LABEL_BOX_NAME,
  MAX_NET_LABEL_NAME,
  netLabelAt,
  netLabelRect,
  netNameForProbe,
  normLabelBoxRect,
  type LabelBox,
  type LabelBoxOp,
  type NetLabel,
  type NetLabelOp,
  type NetMap,
} from './annotate';
import { History, isTypingTarget } from './history';
import { createHoist, type MachineRect } from './hoist';
import { createGfx } from './gfx';
import { createLesson } from './lesson';
import { connect, MAX_CHAT_LEN, type RoomHello } from './net';
import { createPalette, type Armed, type ToolId } from './palette';
// NOT loadBench/saveBench: the intro branch predates scopes becoming room
// state, and those two were deleted when the per-browser bench went away.
import { createRooms } from './rooms';
import {
  applyPanelOp,
  drawPanelGhost,
  drawPanelRegions,
  normPanelRect,
  PANEL_HANDLE_CURSOR,
  PanelHost,
  panelHotAt,
  panelZoneAt,
  resizePanelRect,
  roundRectPath,
  scopeOwner,
  setPanelRoom,
  type Panel,
  type PanelHandle,
  type PanelHover,
  type PanelOp,
  type PanelRect,
} from './panel';
import {
  DotFlow,
  drawElement,
  drawElementsLod,
  drawGrid,
  hitTest,
  LOD_FULL,
  type Camera,
  type DamageState,
} from './render';
import { SpatialIndex } from './spatial';
import {
  applyScopeControl,
  applyScopeOp,
  applyWireScopes,
  defaultScopeSettings,
  probeColor,
  renderScopeInto,
  scopeChannels,
  scopeControlAt,
  scopeRectOp,
  scopeSetOp,
  scopeToSeed,
  seedToScope,
  TraceStore,
  wireScopeSet,
  type FloatScope,
  type Probe,
  type ScopeControlId,
  type ScopeOp,
  type WireScope,
} from './scope';
import { createDock } from './dock';
import {
  fmtEntry,
  parseEng,
  parseField,
  quantityOf,
  rangeText,
  seriesLadder,
  stepLadder,
  type Quantity,
} from './units';
import {
  isPreferred,
  nearestPreferred,
  preferredNeighbours,
  seriesExplainer,
  stdValuesMode,
} from './eseries';
import {
  applyLayerOp,
  aperturesOn,
  drawLayerGhost,
  drawSensorLayers,
  layerAt,
  layerPlateAt,
  layerPlateRect,
  normLayerRect,
  type Aperture,
  type Layer,
  type LayerOp,
} from './layer';
import { CameraSource } from './sensor';

const DT = 10e-6;
const MAX_STEPS_PER_FRAME = 4000; // local-mode wall budget

const PART_HOTKEYS: Record<string, string> = {
  w: 'Wire',
  r: 'Resistor',
  c: 'Capacitor',
  l: 'Inductor',
  g: 'Ground',
  v: 'Battery',
  V: 'V Rail',
  f: 'AC Source',
  i: 'Current Source',
  d: 'Diode',
  z: 'Zener',
  e: 'LED',
  n: 'NPN',
  N: 'Noise',
  p: 'PNP',
  m: 'NMOS',
  M: 'PMOS',
  a: 'Op-Amp',
  u: 'OTA',
  '5': '555 Timer',
  s: 'Switch',
  b: 'Lamp',
  B: 'Button',
  t: 'Potentiometer',
  y: 'Photocell',
  // The logic family. All 26 unshifted letters were already taken, so these
  // are shifted; the names match the catalogue entries exactly.
  D: 'NAND Gate',
  I: 'Inverter',
  F: 'D Flip-Flop',
  L: 'D Latch',
  S: 'Shift Register',
  C: 'Counter',
  U: 'Multiplexer',
};

// Read `?stdvalues=` BEFORE anything can rewrite the query string. `rooms.ts`
// replaces `location.search` with `?room=<id>` once it has joined, and the only
// other caller of `stdValuesMode()` is the property editor — which a player
// cannot open until long after that rewrite. So the flag's own stickiness
// (mode → localStorage) never got the chance to arm itself, and the documented
// URL opt-in silently did nothing. The call is idempotent; this one is for its
// side effect.
stdValuesMode();

await init();

// The frame stream is walked in fixed-size records, so `FRAME_STRIDE` here and
// `sim_core::FRAME_STRIDE` there must agree EXACTLY. If they drift by one slot
// every element after the first is read from the wrong offsets — voltages
// arriving as currents, currents as power — with no error anywhere and every
// number on screen quietly wrong. Raising MAX_PINS for a wider chip is now an
// ordinary thing to do, so assert it instead of trusting the mirror.
if (frameStride() !== FRAME_STRIDE) {
  throw new Error(
    `frame stride mismatch: wasm says ${frameStride()}, client says ${FRAME_STRIDE} ` +
      `— update MAX_PINS in circuit.ts to match sim_core::MAX_PINS`,
  );
}

// ---------------------------------------------------------------- state
let elements: ElementSpec[] = demoCircuit();
/** Viewport cull + hit-test index over `elements` (grid-space hash grid).
 * Every geometry change must go through applyDoc or call space.update — the
 * draw loop and the pointer paths trust its cached bboxes. */
const space = new SpatialIndex();
space.rebuild(elements);
/** O(1) id lookup: `elements.find` is a full scan and the world is big now. */
const elemById = (id: number): ElementSpec | undefined => space.get(id);
let live = new Map<number, ElemLive>();
let simTime = 0;
let online = false;
let population = 0;
let myId = -1;
const cursors = new Map<number, { x: number; y: number; seen: number }>();

/** Server-computed damage, keyed by element id: how hot each part is running
 * and whether it has let go. Every value here is authoritative — the client
 * never decides that something is overloaded, it only draws what the room
 * tells it (see the `damage` snapshot in net.ts). Offline there is no damage
 * model at all, so this stays empty and nothing can break. */
let damage = new Map<number, DamageState>();
/** True once the first snapshot has landed: parts that were already dead when
 * we joined must not each fire a magic-smoke burst and a toast. */
let damageSeen = false;

let probes: Probe[] = [];
/** Shared control-panel regions (room-scoped, like probes). */
let panels: Panel[] = [];

// ---- ANNOTATION. Two primitives, both room state, both pure LABELS: a box
// with a title drawn round some parts, and a name pinned to a grid point.
// Neither reaches the solver, neither groups anything, and neither opens a
// window. See annotate.ts.
let labelBoxes: LabelBox[] = [];
let netLabels: NetLabel[] = [];
/** WHICH net is named what, derived by the server (see `onNetMap`). */
let netMap: NetMap = emptyNetMap();
let localBlidCounter = 1;
let localNlidCounter = 1;
/** probe pid -> net name, rebuilt only when the inputs change. Every readout
 * that shows a channel takes its string from HERE, so the scope chip, the
 * dock chip and the panel meter row can never disagree about what a net is
 * called. Empty (undefined) when nothing is named, so a room with no net
 * labels pays nothing and every label reads exactly as it did before. */
let netNamesCache: Map<number, string> | undefined;
let netNamesKey = '';
function netNames(): Map<number, string> | undefined {
  // A room with no net labels pays one length check per frame and nothing
  // else — not even the cache key, which is the only allocation here.
  if (netLabels.length === 0 || netMap.probe.size === 0) {
    netNamesKey = '';
    netNamesCache = undefined;
    return undefined;
  }
  const key = `${netMap.probe.size}:${[...netMap.probe].join(',')}:${netLabels
    .map((l) => `${l.nlid}=${l.name}`)
    .join(',')}`;
  if (key === netNamesKey) return netNamesCache;
  netNamesKey = key;
  const m = new Map<number, string>();
  for (const p of probes) {
    const n = netNameForProbe(p.pid, netLabels, netMap);
    if (n) m.set(p.pid, n);
  }
  netNamesCache = m.size > 0 ? m : undefined;
  return netNamesCache;
}

// ---- EXTERNAL INPUTS. `layers` and `claims` are ROOM state (the server's
// broadcast is the truth); the camera behind a layer is this browser's alone
// and is never replicated, never described on the wire, never restored.
let layers: Layer[] = [];
/** lid -> the client id driving it. Session-scoped: a reload drops it. */
let claims = new Map<number, number>();
let localLidCounter = 1;
/** What a control panel is pointing at: hovering a row (or a whole window)
 * highlights the part out on the canvas. Set by panel.ts on enter/leave. */
let panelHover: PanelHover | null = null;
let localPlidCounter = 1;
const traces = new TraceStore();
let localPidCounter = 1;
/** Docked-panel instrument settings (timebase, y-scale, trigger). */
const dockScope = defaultScopeSettings(5);

/** Sound. Two kinds of source, mixed in one worklet: every Speaker element
 * in the document (the server streams their coil voltage at 12.5 kHz), and
 * the '3'-listen probe. Online the listen pid arrives with the server's probe
 * list, so remember what we asked to hear. */
const audio = new AudioPlayer();
// Exposed for end-to-end tests (like __cam): headless runs cannot hear, so
// they assert on pid/level instead.
(window as unknown as { __audio: AudioPlayer }).__audio = audio;
let listenWanted: { elem: number; pin: number } | null = null;

/** Bumped by every change to `elements`. Deriving the speaker set is an O(n)
 * scan and the document can hold 50k parts, so it is re-derived when the
 * document changes, not once per frame. */
let docVersion = 0;
let speakerVersion = -1;
let speakerOnline = false;
let speakerIds: number[] = [];

/** Keep AudioPlayer's source set equal to the document's Speaker set: a
 * speaker that appears starts streaming, one that is deleted fades out.
 * Offline it is deliberately EMPTY — the local WASM sim has no substep
 * sampler, so a speaker there has no waveform to play and pretending
 * otherwise would mean playing 60 Hz aliasing mush. The HUD says so. */
function syncSpeakers() {
  if (docVersion === speakerVersion && online === speakerOnline) return;
  speakerVersion = docVersion;
  speakerOnline = online;
  speakerIds = [];
  for (const e of elements) if (e.kind.t === 'Speaker') speakerIds.push(e.id);
  audio.setSpeakers(online ? speakerIds : []);
}

let selectedIds = new Set<number>();
let selectedProbe: number | null = null;
/** Third selection flavour, beside parts and probe flags: the machine
 * assembly. Deliberately not faked as an element — it has no id in the
 * document, and pretending otherwise would put a phantom into every path that
 * walks `selectedIds`. */
let selectedMachine = false;

/** Copy/paste: kinds + pins relative to the selection centroid, plus the
 * two per-instance document properties (rating tier, symbol rotation) —
 * copying a 5 W resistor has to give you a 5 W resistor. */
type ClipItem = { kind: ElementKind; pins: Point[]; tier?: number; rot?: number };
let clipboard: ClipItem[] = [];
let pasting: ClipItem[] | null = null;

/** In-place oscilloscopes: world-anchored SHARED instruments (the shape lives
 * in scope.ts because panel.ts renders the ones a region encloses).
 *
 * Online this is a mirror of room state — the server owns the list, mints the
 * sids and broadcasts every change. `sidCounter` is only for the offline
 * client, which runs the same rules locally. */
let floatScopes: FloatScope[] = [];
let sidCounter = 1;

/** Scopes are room state: online the server owns the list (its broadcast is
 * the truth), offline we apply the same rules locally. Exactly the deal
 * `panelOp` and `layerOp` strike, and the one scopes never had. */
function scopeOp(op: ScopeOp) {
  if (online) {
    net.sendScope(op);
    return;
  }
  floatScopes = applyScopeOp(floatScopes, op, () => {
    for (const s of floatScopes) sidCounter = Math.max(sidCounter, s.sid + 1);
    return sidCounter++;
  });
}

// Retuning: the throttle and the echo hold.
//
// Every on-canvas control, every panel-widget button and every channel toggle
// ends in `scopeRetuned`, so no way of changing an instrument can forget to
// replicate. Two of those ways are STREAMS, not clicks — the timebase wheel
// and a dragged trigger level — which is why this looks like the drag path
// rather than like a plain send.
/** Per-scope throttle state. KEYED BY SID, and that is the whole point: one
 * shared slot meant that retuning scope 1 and then touching scope 2 inside
 * the same 80 ms window overwrote scope 1's pending op with scope 2's, and
 * scope 1's last notch was silently dropped. It converged — the broadcast is
 * the whole list, so the very op that caused the loss re-synced the scope it
 * lost — but "one notch of a fast two-scope flick doesn't take" is still a
 * lie told to the player's fingers. A stream per instrument needs a slot per
 * instrument. */
type Retune = { at: number; pending: FloatScope | null; timer: number };
const retune = new Map<number, Retune>();
/** The scope whose SETTINGS this client is currently in the middle of
 * changing, and until when. A wheel gesture has no pointer-up to end it, so
 * the hold is a short window past the last change rather than a flag. */
let retuneHeld: { sid: number; until: number } | null = null;

/** Send this scope's settings and channel list as they stand now: at most one
 * op per 60 ms PER SCOPE, and always one more after the last change, because
 * the value the player meant is the one they stopped on. */
function scopeRetuned(s: FloatScope) {
  const now = performance.now();
  retuneHeld = { sid: s.sid, until: now + 250 };
  let r = retune.get(s.sid);
  if (!r) {
    r = { at: 0, pending: null, timer: 0 };
    retune.set(s.sid, r);
  }
  if (now - r.at > 60) {
    r.at = now;
    scopeOp(scopeSetOp(s));
    return;
  }
  r.pending = s;
  if (!r.timer) {
    r.timer = window.setTimeout(() => {
      const cur = retune.get(s.sid);
      if (!cur) return;
      cur.timer = 0;
      cur.at = performance.now();
      if (cur.pending) scopeOp(scopeSetOp(cur.pending));
      cur.pending = null;
    }, 80);
  }
}

/**
 * The room's scope list, applied. Merged rather than replaced, so instrument
 * identity — and this client's own autoscale and trigger state — survives.
 *
 * THE ECHO. A scope drag sends one op per 60 ms and gets each of them back;
 * applying our own confirmation of where the pointer WAS would fight the
 * pointer where it IS, and the scope would stutter backwards under the
 * cursor. So while this client holds a scope it owns that rectangle, and the
 * pointer-up sends the final rect that reconciles the two. Identical to the
 * bargain `onDoc` already strikes for a machine drag.
 */
function applyScopes(list: WireScope[]) {
  floatScopes = applyWireScopes(floatScopes, list, (sid, what) =>
    what === 'rect'
      ? scopeDrag?.s.sid === sid || scopeResize?.s.sid === sid
      : retuneHeld?.sid === sid && performance.now() < retuneHeld.until,
  );
}

const localSim = new Sim(DT);
localSim.setElements(elements);

function applyOp(e: ElementSpec, op: InteractOp) {
  if (op.t === 'SetSwitch' && (e.kind.t === 'Switch' || e.kind.t === 'Button')) {
    e.kind.closed = op.closed;
  }
  if (op.t === 'SetValue') {
    if (e.kind.t === 'Resistor' || e.kind.t === 'Lamp' || e.kind.t === 'Speaker') e.kind.ohms = op.value;
    else if (e.kind.t === 'Capacitor') e.kind.farads = op.value;
    else if (e.kind.t === 'Inductor') e.kind.henries = op.value;
    else if (e.kind.t === 'VoltageSource' || e.kind.t === 'Rail') e.kind.dc = op.value;
    else if (e.kind.t === 'CurrentSource') e.kind.amps = op.value;
    // The knob is the level. The seed is identity, not a value.
    else if (e.kind.t === 'Noise') e.kind.volts = op.value;
    else if (e.kind.t === 'Potentiometer') e.kind.wiper = Math.min(0.99, Math.max(0.01, op.value));
  }
}

function applyDoc(op: DocOp) {
  docVersion++;
  if (op.t === 'Add') {
    if (!space.get(op.spec.id)) {
      elements.push(op.spec);
      space.insert(op.spec);
    }
  } else if (op.t === 'Remove') {
    elements = elements.filter((e) => e.id !== op.id);
    space.remove(op.id);
    selectedIds.delete(op.id);
  } else if (op.t === 'Move') {
    const e = elemById(op.id);
    if (e) {
      e.pins = op.pins;
      if (op.rot !== undefined) e.rot = op.rot & 3;
      space.update(e);
    }
  } else if (op.t === 'SetKind') {
    const e = elemById(op.id);
    if (e) {
      e.kind = op.kind;
      space.update(e); // pin count can change with the kind
    }
  } else if (op.t === 'SetName') {
    const e = elemById(op.id);
    // No `space.update`: a name has no geometry, so the spatial index has
    // nothing to re-file. It is a label, all the way down.
    if (e) e.name = op.name;
  }
  // WHICH PART READS WHICH PATCH IS GEOMETRY, so it has to be re-derived
  // whenever the geometry changes. This is the hook that was missing: the
  // aperture set was only ever recomputed when a LAYER moved or a claim
  // changed, never when the document did — so a photocell dropped onto a
  // camera that was ALREADY live was never handed to the sampler, and the
  // feature's own documented order (click the plate, then press Y and drop
  // the part on the video) produced a live camera driving nothing. Free
  // unless a camera is running: `pushApertures` returns immediately when it
  // is not.
  pushApertures();
}

let idCounter = 1;
const newId = () => (myId > 0 ? myId : 999) * 1_000_000 + idCounter++;

/** THE HOIST: the machine's package on the canvas plus the goal card.
 * hoist.ts + chip.ts own every pixel and every DOM node of it, INCLUDING the
 * glyphs of the four fixture parts it stands on — inside a package a device
 * symbol is internal schematic, not a free-standing part, exactly as the 555
 * draws its own divider and comparators. Rendering is not existence: those
 * four stay real elements for the solver, for wiring, for probes, for
 * tooltips, for damage and for `elementAt`. Only their glyph moved. */
const hoist = createHoist(document.body, { reset: () => net.sendMachineReset() });

/** Ids 900..999 are the server's machine fixtures: players wire to them but
 * cannot move, rotate, retype or delete them INDIVIDUALLY. The server rejects
 * those ops, so applying one locally would only desync this client. The whole
 * assembly moves together through the machine op below. */
const isFixtureId = (id: number) => id >= 900 && id <= 999;

// ------------------------------------------------------ the machine assembly
//
// The freight hoist behaves like a part because it IS presented as one: a
// chip. Click its package face to select it, drag the face to move it — the
// same rule render.ts already applies to any part with more than four pins
// (`hitTest`: "packages are boxes, so the whole chip is grabbable"), which is
// also why it needs no title bar. It is NOT an element, though — the four
// fixtures bolted inside it are, and the server owns them —
// so the move travels as its own op and translates the footprint and all four
// children atomically, without touching the mechanism (height, velocity, hold
// timer and landing count all survive a move; it is a translation, not a
// reset).
//
// SEAM, client side. A future generic `Container` part would need exactly four
// things, and everything below is written against only these:
//   * children()   — the element ids the container owns (here: the reserved id
//                    range, since this room has exactly one machine);
//   * footprint()  — its rect in grid units (here: hoist.rect(), server state);
//   * a move op    — one per instance (here: net.sendMachineMove, room-scoped
//                    because there is one machine; a Container needs an id);
//   * zone hit-tests that always YIELD to pins and to child elements (here:
//     hoist.zoneAt, consulted only after pinAt/elementAt come up empty).
// Per-instance world state (here: the one Hoist mechanism) rides along with the
// footprint on the server and needs no client involvement.

/** The elements the machine owns. */
const machineChildren = (): ElementSpec[] => elements.filter((e) => isFixtureId(e.id));

/** World range the server will accept for the machine (server/main.rs
 * `WORLD_LIMIT`). Mirrored here so a drag can never produce an op the server
 * refuses: a refused op would leave this client's optimistic placement
 * permanently ahead of the room. */
const MACHINE_WORLD_LIMIT = 1_000_000;
/** Largest single move the server accepts (server/main.rs `MAX_MACHINE_STEP`).
 * No gesture can reach it — the pointer would have to leave the window — but
 * mirroring it makes "this client never issues an op the server refuses" a
 * property of the code instead of a property of the zoom range. */
const MACHINE_MAX_STEP = 100_000;

/** The largest part of (dx, dy) that keeps the whole footprint in range. */
function clampMachineDelta(r: MachineRect, dx: number, dy: number): [number, number] {
  const lim = (v: number, span: number) =>
    Math.min(Math.max(v, -MACHINE_WORLD_LIMIT), MACHINE_WORLD_LIMIT - span);
  return [
    lim(r[0] + dx, r[2] - r[0]) - r[0],
    lim(r[1] + dy, r[3] - r[1]) - r[1],
  ];
}

/** Translate the assembly by an integer grid delta: footprint, package and
 * children together, optimistically here and authoritatively on the server.
 * Used by undo/redo (with the gesture's delta negated); the drag itself places
 * from its own snapshot so a long gesture cannot accumulate rounding. */
function moveMachineBy(dx: number, dy: number) {
  const r = hoist.rect();
  if (!r || (dx === 0 && dy === 0)) return;
  const [cx, cy] = clampMachineDelta(r, dx, dy);
  if (cx === 0 && cy === 0) return;
  // Refuse exactly what the server would refuse, rather than moving locally
  // and desyncing: better a dead undo than a machine only this client can see.
  if (Math.abs(cx) > MACHINE_MAX_STEP || Math.abs(cy) > MACHINE_MAX_STEP) return;
  // One-shot placement: claim it, then immediately hand the footprint back to
  // the server's next broadcast (no pointer is holding it).
  hoist.setLocalRect([r[0] + cx, r[1] + cy, r[2] + cx, r[3] + cy]);
  hoist.endLocalDrag();
  for (const c of machineChildren()) {
    c.pins = c.pins.map(([x, y]) => [x + cx, y + cy] as Point);
    space.update(c);
  }
  docVersion++;
  if (online) net.sendMachineMove(cx, cy);
  else localSim.setElements(elements); // offline: the local netlist follows
}

// -------------------------------------------------------------- the room
//
// Which room this client is in, and the chrome for changing that. The chip
// and the browser live in rooms.ts; everything below is the wiring, and the
// one piece that must be right: what gets THROWN AWAY when the room changes.

/** The room we are currently rendering ('' = a server with no room list).
 * Compared against every hello, because a hello also arrives on reconnect
 * and re-entering the SAME room must not yank the camera or bin the undo
 * stack the player still has a use for. */
let roomKey: string | null = null;

const roomsUI = createRooms({
  join: (code) => net.join(code),
  // The camera and the in-place scopes are this client's own state: the
  // server has never seen them, so a template can only get them from here.
  view: () => ({
    home: camRect(),
    scopes: floatScopes.map(scopeToSeed),
  }),
  toast: (m) => toast(m),
});

/** The intro-series lesson card (lesson.ts). Shown only in rooms made from
 * an `intro-*` template; its step checks read the same live map every other
 * instrument reads. */
const lessonUI = createLesson(document.body, {
  elements: () => elements,
  live: () => live,
  isBroken: (id) => isBroken(id),
  machine: () => hoist.state(),
  join: (code) => net.join(code),
  toast: (m) => toast(m),
});

/** Graphics preferences (⇧G). Per-player display state only — nothing here
 *  reaches the document, the wire or the solver, so two players may disagree
 *  about every setting and still be looking at the same circuit. */
const gfxUI = createGfx(document.body);

/**
 * Leave a room. Every line here is state that is scoped to ONE room and
 * would be wrong — not merely stale — in the next one:
 *
 *   * undo entries carry element ids and captured specs from a document that
 *     no longer exists; replaying one would re-add a stranger's part;
 *   * traces, probe selections and scope channels are keyed by pid;
 *   * the clipboard, the cursors and the audio tap all point at the old room;
 *   * the goal card latches visible, so a machineless room would inherit a
 *     frozen objective for a machine that is nowhere in the world.
 *
 * This is the cost of switching in place instead of reloading the page, and
 * paying it explicitly is what makes the in-place switch correct.
 */
function resetForRoom(room: RoomHello | null) {
  history.clear();
  floatScopes = [];
  sidCounter = 1;
  traces.prune(new Set());
  selectedIds.clear();
  selectedProbe = null;
  selectedMachine = false;
  cursors.clear();
  clipboard = [];
  pasting = null;
  placing = null;
  disarmTools();
  canvas.style.cursor = 'default';
  // Leaving a room STOPS the camera. A capture must never outlive the place
  // it was pointed at; the next room's `hello` brings its own layers.
  camera.stop();
  layers = [];
  claims = new Map();
  localLidCounter = 1;
  // The conversation belongs to the room it happened in: the next room's
  // tail arrives right after its hello, as ordinary chat messages.
  chatClear();
  // Annotation is room state, so it leaves with the room. The next hello
  // brings the new room's own.
  labelBoxes = [];
  netLabels = [];
  netMap = emptyNetMap();
  localBlidCounter = 1;
  localNlidCounter = 1;
  pendingBoxName = null;
  pendingNetName = null;
  closeNameEditor();
  listenWanted = null;
  audio.stop();
  damage = new Map();
  damageSeen = false;
  // A room that SAYS it has no machine has no goal card and no fixture to
  // hit-test. `room === null` is a different thing — a server from before
  // rooms existed, which has told us nothing either way — so leave the
  // machine alone there rather than hiding a hoist that is really running.
  if (room && !room.machine) hoist.clear();
  // Panel window positions are per plid, and plids are room-scoped ids.
  setPanelRoom(room?.id ?? null);

  const home = room?.view?.home;
  homeAuthored = !!home && home.length === 4;
  homeRect = homeAuthored
    ? { x0: home![0]!, y0: home![1]!, x1: home![2]!, y1: home![3]! }
    : { ...DEFAULT_HOME };

  // The camera frames the template's SEED rects — where the author aimed it.
  // Deliberately the seeds and not the room's live scopes: `home` is a fixed
  // place to arrive at, and it must not drift every time somebody drags an
  // instrument across the district.
  const seeds = room?.view?.scopes ?? [];
  homeSeeds = seeds.map((s) => {
    const r = seedToScope(s, 0);
    return [r.x, r.y, r.x + r.w, r.y + r.h] as [number, number, number, number];
  });
  fitHome();
  // The bench itself is NOT built here any more. It is room state now, so it
  // arrives with `hello` like the parts and the panels do — including the
  // seeds, which the server materialized once when the room was created.
  //
  // What used to be here was a per-browser `localStorage` bench, and it is
  // worth recording why it went: it made scopes look replicated whenever the
  // two clients being tested were two tabs of one browser (same store, so a
  // reload in B showed what A had done) and not replicated at all between two
  // players. Nothing about a scope ever reached the server.
}

/** `?room=CODE` in the address bar is an invite link: it is where a shared
 * URL lands, and it is what a reload comes back to. No code = the server's
 * default room, exactly as a bare `/ws` behaved before rooms existed. */
const roomFromUrl = (): string | null => {
  const c = new URLSearchParams(location.search).get('room');
  return c && c.trim() ? c.trim().toUpperCase() : null;
};

const net = connect({
  onHello(
    you,
    serverElements,
    serverProbes,
    serverPanels,
    serverScopes,
    serverBoxes,
    serverNets,
    room,
  ) {
    online = true;
    myId = you;
    elements = serverElements;
    docVersion++;
    space.rebuild(elements);
    pushApertures(); // a whole new document is the biggest geometry change there is
    probes = serverProbes;
    panels = serverPanels;
    live = new Map();
    // Damage is room state: forget the old room's, and treat whatever the
    // first snapshot carries as history, not as news (no toast, no pop for
    // parts that were already dead when we joined).
    damage = new Map();
    damageSeen = false;
    // A hello also arrives on every reconnect. Only a DIFFERENT room is a
    // room change: re-entering this one must not bin the undo stack or yank
    // the camera back to the district the player deliberately left.
    const key = room?.id ?? '';
    if (key !== roomKey) {
      roomKey = key;
      resetForRoom(room);
    }
    // AFTER `resetForRoom`, which empties the bench: the room's instruments
    // arrive with the hello, exactly like its parts and its panels. Routed
    // through the same handler the live broadcast uses so a late joiner and a
    // running client can never end up with different rules.
    applyScopes(serverScopes);
    // AFTER `resetForRoom`, not before: leaving a room drops its annotation
    // (see `resetForRoom`), and assigning the new room's boxes above that
    // call would have them wiped by the very reset that is meant to clear the
    // OLD room's. That is exactly what happened — the labels arrived, were
    // deleted a line later, and every net went nameless.
    labelBoxes = serverBoxes;
    netLabels = serverNets;
    // Derived state: the server re-sends `netmap` on the tick after a join,
    // so start from "nothing is attached" rather than from the last room's
    // answer.
    netMap = emptyNetMap();
    roomsUI.onHello(room);
    lessonUI.onRoom(room ? { id: room.id, template: room.template } : null);
  },
  onFrame(f) {
    simTime = f.time;
    dock.onRt(typeof f.rt === 'number' ? f.rt : null);
    const m = new Map<number, ElemLive>();
    for (const r of f.e) {
      const v: number[] = [];
      const i: number[] = [];
      for (let p = 0; p < MAX_PINS; p++) {
        v.push(r[2 + p]!);
        i.push(r[2 + MAX_PINS + p]!);
      }
      m.set(r[0]!, { id: r[0]!, npins: r[1]!, v, i, power: r[2 + 2 * MAX_PINS]! });
    }
    live = m;
  },
  onOp(id, op) {
    const e = elemById(id);
    if (e) applyOp(e, op);
  },
  onDoc(op) {
    // While THIS client drags the machine it owns the children's geometry: the
    // server's echo of a throttled increment lags the pointer by up to 60 ms,
    // and applying it would rubber-band the terminals against the package.
    // The final op on release reconciles everything.
    if (machineDrag && op.t === 'Move' && isFixtureId(op.id)) return;
    applyDoc(op); // idempotent for our own echoes
  },
  onProbes(list) {
    probes = list;
    pruneProbeUsers();
    if (listenWanted) {
      const p = list.find(
        (x) => x.elem === listenWanted!.elem && x.pin === listenWanted!.pin && x.kind === 'v',
      );
      if (p) {
        listenWanted = null;
        audio.listen(p.pid);
      }
    }
  },
  onPanels(list) {
    panels = list;
  },
  onScopes(list) {
    applyScopes(list);
  },
  onLabelBoxes(list) {
    labelBoxes = list;
    resolvePendingNames();
  },
  onNetLabels(list) {
    netLabels = list;
    resolvePendingNames();
  },
  onNetMap(liveNlids, probePairs) {
    netMap = { live: new Set(liveNlids), probe: new Map(probePairs) };
  },
  onLayers(list, cl) {
    layers = list;
    claims = new Map(cl);
    // Lost the claim (released, taken, or the layer was deleted)? The camera
    // now has nothing to drive, so it STOPS — hardware indicator and all.
    // A capture with no purpose is a capture that should not be running.
    if (camera.isLive() && ![...claims.values()].includes(myId)) camera.stop();
    pushApertures();
  },
  onSensors(list) {
    // EVERY client applies these, not just the one holding the camera: a
    // photocell is room state and everybody watches it move. The value is
    // the server's, straight out of `ParamWrite::Light`, so nobody is ever
    // looking at a locally-guessed number — the design pillar, unbent.
    for (const [id, q] of list) {
      const e = elemById(id);
      if (e && e.kind.t === 'Photocell') e.kind.light = q / 65535;
    }
  },
  onMachine(m) {
    hoist.onMachine(m);
  },
  onDamage(parts) {
    // A full snapshot replaces the map: anything absent is healthy again
    // (repaired, cooled off, or deleted).
    const next = new Map<number, DamageState>();
    for (const [id, stress, broken] of parts) {
      const was = damage.get(id);
      const dead = broken !== 0;
      // The pop is a one-shot: remember when THIS client first saw it, and
      // never fire it for parts that were already dead when we joined.
      const poppedAt =
        dead && damageSeen && !was?.broken ? performance.now() : was?.poppedAt;
      next.set(id, { stress, broken: dead, poppedAt });
      if (dead && damageSeen && !was?.broken) magicSmoke(id);
    }
    damage = next;
    damageSeen = true;
  },
  onSamples(t0, dts, s) {
    for (const [pid, samples] of Object.entries(s)) {
      traces.appendChunk(Number(pid), t0, dts, samples);
      audio.pushChunk(Number(pid), t0, dts, samples);
    }
  },
  onAudio(t0, dts, s, rt) {
    // Speaker taps, keyed by element id. Not a trace: these never reach the
    // TraceStore, so a 12.5 kHz stream cannot swamp the scopes.
    //
    // The server's realtime ratio comes with them: below 1 the sim is dilated
    // and audio physically cannot keep up, which the dock reports as "sim
    // 0.6x" instead of letting the player think the sound is broken.
    audio.setRealtimeRatio(rt);
    for (const [elem, samples] of Object.entries(s)) {
      audio.pushSpeakerChunk(Number(elem), t0, dts, samples);
    }
  },
  onPresence(n) {
    population = n;
    roomsUI.onPresence(n);
  },
  onRoomMeta(id, name) {
    roomsUI.onRoomMeta(id, name);
  },
  onRoomGone(id, reason) {
    // The room we are standing in is not going to answer again — it was
    // deleted, or the code in the URL is not on this server. Drop what
    // belonged to it BEFORE rooms.ts moves us, so nothing survives the
    // handover; `roomKey` is cleared so the landing hello counts as a
    // change and re-fits the camera.
    //
    // ONCE, though. A server with no rooms sends one of these every reconnect
    // for as long as the player sits there, and the teardown ends in
    // fitHome(): repeating it would snatch the camera back to the default
    // district every 2.5 s while they are panning around the local sim. If
    // `roomKey` is already null there is nothing left to drop.
    if (roomKey !== null) {
      roomKey = null;
      resetForRoom(null);
      hoist.clear(); // whatever machine that room had, it is not ours any more
      lessonUI.onRoom(null);
    }
    roomsUI.onGone(id, reason);
  },
  onCursor(who, x, y) {
    if (who !== myId) cursors.set(who, { x, y, seen: performance.now() });
  },
  onChat(who, text) {
    // Our own line comes back through the broadcast too — render it from
    // here, not optimistically, so what we see is what everyone saw.
    chatAddLine(who, text);
  },
  onWireDrift(drift) {
    // The server said something this client does not understand. Fields that
    // arrive this way fail by NOT HAPPENING — a camera that never flies, a
    // scope that never appears — so it gets said out loud in the world
    // instead of only in a console nobody has open.
    toast(`this client and this server disagree about ${drift.map((d) => d.field).join(', ')}`);
  },
    onReject(r) {
    // Only the sender acts on it; everyone else's document never changed.
    //
    // This should be RARE now: `refused()` runs the identical Rust gate
    // before we send, so a refusal here means either a race (someone else's
    // edit landed first) or a path the client cannot pre-check — a machine
    // move, or an op from a client that predates the pre-send gate. Say what
    // happened either way: a silently-refused op used to leave a ghost part
    // on this canvas forever with no explanation.
    if (r.who !== myId) return;
    showReject(r.hint, r.ids, r.ctx);
  },
  onClose() {
    if (online) {
      online = false;
      localSim.setElements(elements);
      // Offline: no server, no dilation to report — drop the stale ratio.
      dock.onRt(null);
      // The local sim has no damage model, so keeping the last snapshot
      // would leave parts drawn as broken that are now conducting.
      damage = new Map();
      damageSeen = false;
      repairing = false;
      roomsUI.onOffline();
    }
  },
}, roomFromUrl());

// ------------------------------------------------------- damage + repair
//
// Nothing here decides that a part is overloaded: the server does, from
// solver output, and sends a `damage` snapshot. The client announces the
// failure, draws the wreckage, and offers the wrench.

/** Transient announcements, newest at the bottom. */
const toastBox = document.createElement('div');
toastBox.id = 'toasts';
document.body.appendChild(toastBox);

function toast(text: string) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.textContent = text;
  toastBox.appendChild(el);
  while (toastBox.childElementCount > 4) toastBox.firstElementChild?.remove();
  setTimeout(() => el.remove(), 7000);
}

// ------------------------------------------------------------- room chat
//
// Enter opens the input, Enter sends, Escape closes (keeping the draft).
// While the input has focus every part hotkey is dead — the same two
// mechanisms the panel name field relies on: the input stops propagation of
// its own keydowns (like #nameedit), AND the window handler returns early on
// anything the chat owns (like panelHost.owns). Lose either and "hi there"
// litters the room with resistors and a rotated inductor.
//
// The lines are DOM, not canvas: textContent only, so whatever another
// player typed renders as text — never markup, never a link. Identity is the
// cursor's identity: the same `who` and the same hue formula, so the name on
// a line and the triangle on the sheet agree about who is who.

const chatBox = document.createElement('div');
chatBox.id = 'chat';
const chatLog = document.createElement('div');
chatLog.style.display = 'contents';
const chatInput = document.createElement('input');
chatInput.id = 'chatinput';
chatInput.type = 'text';
chatInput.maxLength = MAX_CHAT_LEN;
chatInput.autocomplete = 'off';
chatInput.spellcheck = false;
chatInput.placeholder = 'say something…  (Enter sends, Esc closes)';
chatBox.append(chatLog, chatInput);
document.body.appendChild(chatBox);

const CHAT_KEEP = 40; // lines held in the DOM for the input-open backlog
const CHAT_FADE_MS = 9000;

/** Same formula as `drawCursors`: one identity, two renderings. */
const whoHue = (who: number) => (who * 137.5) % 360;

function chatAddLine(who: number, text: string) {
  const el = document.createElement('div');
  el.className = 'cline';
  el.style.setProperty('--cwho', `hsl(${whoHue(who)} 80% 60%)`);
  const name = document.createElement('span');
  name.className = 'cwho';
  name.textContent = who === myId ? `P${who} (you)` : `P${who}`;
  const body = document.createElement('span');
  body.textContent = text; // text, never markup
  el.append(name, body);
  chatLog.appendChild(el);
  while (chatLog.childElementCount > CHAT_KEEP) chatLog.firstElementChild?.remove();
  // Quiet by default: fade, then stop occupying the corner. The `gone` class
  // is display:none, which #chat.open overrides — so opening the input brings
  // the whole kept tail back.
  setTimeout(() => el.classList.add('faded'), CHAT_FADE_MS);
  setTimeout(() => {
    el.classList.add('gone');
    syncToastOffset();
  }, CHAT_FADE_MS + 700);
  syncToastOffset();
}

/** Keep the damage toasts above the chat instead of on top of it. */
function syncToastOffset() {
  const h = chatBox.getBoundingClientRect().height;
  document.documentElement.style.setProperty('--toasts-b', h > 4 ? `${42 + h}px` : '36px');
}

function chatOpen() {
  chatBox.classList.add('open');
  chatInput.focus();
  syncToastOffset();
}

/** Close the input but keep the draft: a stray click must not eat a
 * half-typed sentence. Escape clears it deliberately. */
function chatClose() {
  chatBox.classList.remove('open');
  chatInput.blur();
  syncToastOffset();
}

function chatClear() {
  chatLog.replaceChildren();
  chatInput.value = '';
  chatClose();
}

const chatOwns = (t: EventTarget | null) => t instanceof Node && chatBox.contains(t);

chatInput.addEventListener('keydown', (ev) => {
  ev.stopPropagation(); // never let a part hotkey fire while typing a line
  if (ev.key === 'Enter') {
    ev.preventDefault();
    const text = chatInput.value.trim();
    if (text) net.sendChat(text);
    chatInput.value = '';
    chatClose();
  } else if (ev.key === 'Escape') {
    ev.preventDefault();
    chatInput.value = '';
    chatClose();
  }
});
// A click on the canvas mid-sentence: close quietly, keep the draft.
chatInput.addEventListener('blur', () => chatClose());

// ------------------------------------------------- the placement gate (DRC)
//
// The SAME Rust implementation the server enforces, reached through
// `checkEdit` in sim-wasm. Running it here before we send means the two
// sides cannot disagree about what is placeable: a move that breaks the
// simulation is refused at the moment it is made, with a sentence saying
// why, instead of being applied optimistically and then silently dropped —
// which used to leave a ghost part on this canvas forever, burning an id the
// server had never heard of.
//
// It is also the ONLY gate offline. `localSim.setElements` was called raw,
// so every refusal class — a wire across a source, 1 V against 5 V, a 9 V
// battery straight across an LED — froze the local sim with no explanation.

/** Above this the pre-send check is skipped and the server's refusal (plus
 * its `reject` callout) is relied on instead.
 *
 * The gate is two compiles, two dense factorizations of the whole document,
 * and a short convergence trial of each independent circuit in it. It runs on
 * the UI thread, so "the sim never stalls the UI" is the invariant that
 * decides the trade.
 *
 * Every number here was measured on this tree (release, native, the same code
 * path the wasm build compiles). SHAPE MATTERS MORE THAN COUNT, because the
 * trial is per independent circuit: a room of separate builds splits into
 * many small blocks, while one shared bus with everything tapping it is a
 * single wide block and cannot split. Both were measured, and the game's own
 * design pillar is the shared grid, so it is the one to plan against:
 *
 *     elements        4    147    250    400    600    800   1200
 *     separate      0.05  0.48   0.63   0.91   1.51   2.37   5.83   ms
 *     shared bus      -   0.6    0.9    2.9    -      1.8    -      ms
 *
 * The shared-bus peak is at 400-405, where the widest block is still trialled;
 * past it TRIAL_CEILING gives that block up and the cost falls back. That is a
 * taper into a ceiling, not a cliff.
 *
 * Those are healthy rooms. The worst document anyone can construct inside the
 * cap — 720 parts drawn as 48 dense diode meshes, each with an open switch
 * and an AC source, so every one of them runs all four trial states — costs
 * 4.1 ms, a quarter of a frame, and only on the frame an edit lands.
 *
 * 800 is where a healthy room reaches 2.4 ms — 14% of a 60 fps frame — and it
 * is more than five times the size of the room a fresh server stands up (147).
 * Past it the curve turns: the two whole-document factorizations are O(n³) in
 * the MNA unknowns and nothing caps them, which is why the cap keys on the
 * document and not, like sim-core's `TRIAL_CEILING`, on one circuit.
 *
 * The previous constant here was 600 and the numbers beside it were wrong by
 * 20x: it claimed "2.2 ms at the worst size", when the worst size inside the
 * old cap actually cost 42.9 ms — 258% of a frame, on this thread. The guard
 * admitted exactly the band that hurt. It is worth saying plainly, because
 * the fix was not a different constant: sim-core now trials each MNA block
 * separately, which is what took 400 elements from 42.9 ms to 0.91 ms.
 *
 * The honest residual is unchanged: a room past the cap loses pre-send
 * prevention, and offline it loses the gate entirely. Nothing becomes more
 * placeable there — the server still refuses it — the callout just arrives a
 * round trip later. */
const GATE_MAX_ELEMENTS = 800;

/** The document `op` would produce, WITHOUT touching the live one. Mirrors
 * the server's `apply_doc_op_to`: same verbs, same order, applied to a copy.
 * Only the changed element is cloned; the gate serializes and never
 * mutates. */
function candidateDoc(op: DocOp): ElementSpec[] {
  if (op.t === 'Add') return space.get(op.spec.id) ? elements : [...elements, op.spec];
  if (op.t === 'Remove') return elements.filter((e) => e.id !== op.id);
  return elements.map((e) => {
    if (e.id !== op.id) return e;
    if (op.t === 'Move') return { ...e, pins: op.pins };
    if (op.t === 'SetName') return { ...e, name: op.name };
    return { ...e, kind: op.kind };
  });
}

/** Point the player at the parts a refusal named, and say what is wrong.
 * Selecting them is the pointing: these are exactly the parts that have to
 * change, and the selection is already how this client says "these ones". */
function showReject(hint: string, ids: number[], ctx: string) {
  const live = ids.filter((id) => !!space.get(id));
  if (live.length) {
    selectedIds = new Set(live);
    selectedMachine = false;
  }
  toast(hint || `that ${ctx || 'change'} was refused`);
}

/** Run the gate on a candidate document. Returns true when it was REFUSED
 * (and the callout has already been shown), so callers read as
 * `if (refused(...)) return;`.
 *
 * `checkEdit`, not `checkDocument`: the shape rule is a rule about the
 * CHANGE, so the gate is given the live document as well as the candidate.
 * Every caller here builds its candidate from `elements`, which is exactly
 * the document the server is about to replace, so the two sides judge the
 * same before/after pair. */
function refused(candidate: ElementSpec[], ctx: string): boolean {
  if (candidate.length > GATE_MAX_ELEMENTS) return false;
  // The client sim runs at its own dt. Structural refusals do not depend on
  // it at all; the convergence trial does, marginally — and the server has
  // the final say either way.
  type GateReject = { hint?: string; ids?: number[]; id?: number | null };
  let r: GateReject | null;
  try {
    r = checkEdit(elements, candidate, DT) as GateReject | null;
  } catch {
    return false; // a gate that cannot run must never block a legal edit
  }
  if (!r) return false;
  const ids = Array.isArray(r.ids) ? r.ids : typeof r.id === 'number' ? [r.id] : [];
  showReject(r.hint ?? '', ids, ctx);
  return true;
}

/** The oldest joke in electronics, and the clearest possible failure notice. */
function magicSmoke(id: number) {
  const e = elemById(id);
  // Fired from the false→true edge of the server's `broken` bit, once per
  // part, never for damage that was already there when we joined.
  audio.playBreak(e?.kind.t, id);
  toast(`${e ? e.kind.t : 'Part'} #${id} released its magic smoke — press K, then click it to repair`);
}

/** K: the repair tool. A wrench-shaped cursor; click a broken part to put it
 * back into service. */
let repairing = false;
const WRENCH_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">' +
  '<path d="M21 4.5a5.5 5.5 0 0 1-7.2 6.9L6.4 18.8a2 2 0 0 1-2.8-2.8l7.4-7.4A5.5 5.5 0 0 1 17.9 1.4' +
  'l-3.3 3.3 2.7 2.7 3.3-3.3c.3.4.4.9.4 1.4z" fill="#ffd66b" stroke="#101014" stroke-width="1.4" ' +
  'stroke-linejoin="round"/></svg>';
const REPAIR_CURSOR = `url("data:image/svg+xml,${encodeURIComponent(WRENCH_SVG)}") 4 20, cell`;

const isBroken = (id: number): boolean => damage.get(id)?.broken === true;

/** Ask the server to repair a part. Deliberately NOT an editDoc call: a
 * repair is a world event, not a document edit, so it never enters the undo
 * history (⌘Z must not un-repair anything) and it is allowed on the
 * server-owned hoist fixture that refuses every document op. */
function repair(id: number) {
  if (!isBroken(id) || !online) return;
  net.sendRepair(id);
}

function armRepair() {
  disarmTools();
  repairing = true;
  placing = null;
  pasting = null;
  canvas.style.cursor = REPAIR_CURSOR;
  // Offline there is no damage model at all, so there is never anything to
  // repair — say so rather than leaving the wrench clicking on nothing.
  if (!online) toast('offline: the local sim has no damage model — nothing can break, or be fixed');
}

function interact(e: ElementSpec, op: InteractOp) {
  // Gate BEFORE the optimistic apply, so a refused switch flip or knob write
  // never has to be rolled back — it simply does not happen. (The server
  // gates interacts too; this is the same code, so the two agree.)
  const cand = elements.map((x) =>
    x.id === e.id ? (JSON.parse(JSON.stringify(x)) as ElementSpec) : x,
  );
  const target = cand.find((x) => x.id === e.id);
  if (target) {
    applyOp(target, op);
    if (refused(cand, 'interact')) return;
  }
  applyOp(e, op); // optimistic; server echo confirms
  if (online) net.sendInteract(e.id, op);
  else localSim.interact(e.id, op);
}

/** Local undo/redo: every edit this player makes funnels through editDoc. */
const history = new History(editDoc);

function editDoc(op: DocOp) {
  if (isFixtureId(op.t === 'Add' ? op.spec.id : op.id)) return; // locked fixture
  // Prevent, don't revert: judge the document the op WOULD produce and drop
  // the op before anything is applied, recorded or sent. Nothing to roll
  // back, no undo entry for an edit that never happened, no ghost part.
  if (refused(candidateDoc(op), 'edit')) return;
  history.record(op, elements); // before applyDoc: captures the prior state
  applyDoc(op); // optimistic
  if (online) net.sendEdit(op);
  else localSim.setElements(elements);
}

/** Drop everything that pointed at a probe that no longer exists: its trace,
 * the probe selection, float-scope channel lists and the audio source.
 * Online this runs on every server probe list, offline after a local toggle
 * (a probe's differential reference is a [elem, pin] node, not another
 * probe, so deleting a probe never invalidates one). */
function pruneProbeUsers() {
  const alive = new Set(probes.map((p) => p.pid));
  traces.prune(alive);
  if (selectedProbe !== null && !alive.has(selectedProbe)) selectedProbe = null;
  // Local mirror of what the server does on its side of a probe removal
  // (`prune_scope_pids`), so an offline client behaves the same and an online
  // one does not draw one frame of a channel that has just died. The
  // authoritative version arrives as the next `scopes` broadcast.
  for (const s of floatScopes) if (s.pids) s.pids = s.pids.filter((pid) => alive.has(pid));
  if (audio.pid !== null && !alive.has(audio.pid)) audio.stop();
}

function toggleProbe(elem: number, pin: number, kind: 'v' | 'i') {
  if (online) {
    net.sendProbe(elem, pin, kind);
    return;
  }
  const k = probes.findIndex((p) => p.elem === elem && p.pin === pin && p.kind === kind);
  if (k >= 0) probes.splice(k, 1);
  else if (probes.length < 8) probes.push({ pid: localPidCounter++, elem, pin, kind });
  pruneProbeUsers();
}

/** Delete one probe: the same toggle '1'/'2' use, aimed at a probe that
 * exists, so the server (or the offline branch) removes it. */
function deleteProbe(p: Probe) {
  if (selectedProbe === p.pid) selectedProbe = null;
  toggleProbe(p.elem, p.pin, p.kind);
}

/** '3': hear this node. The audio source is a normal voltage probe's sample
 * stream, so make sure one exists here, then latch the player onto it —
 * pressing '3' again on the same pin stops (the probe stays). */
function toggleListen(elem: number, pin: number) {
  const here = probes.find((p) => p.elem === elem && p.pin === pin && p.kind === 'v');
  if (here) {
    listenWanted = null;
    if (audio.pid === here.pid) audio.stop();
    else audio.listen(here.pid);
    return;
  }
  listenWanted = { elem, pin };
  toggleProbe(elem, pin, 'v');
  const made = probes.find((p) => p.elem === elem && p.pin === pin && p.kind === 'v');
  if (made) {
    listenWanted = null;
    audio.listen(made.pid);
  }
}

function setProbeRef(pid: number, elem: number, pin: number) {
  if (online) {
    net.sendProbeRef(pid, elem, pin);
    return;
  }
  const p = probes.find((x) => x.pid === pid);
  if (!p) return;
  p.r = p.r && p.r[0] === elem && p.r[1] === pin ? null : [elem, pin];
}

/** Panels are room state: online the server owns the list (its broadcast is
 * the truth), offline we apply the same rules locally. */
function panelOp(op: PanelOp) {
  if (online) {
    net.sendPanel(op);
    return;
  }
  panels = applyPanelOp(panels, op, () => {
    // Never reuse a plid a restored/server panel already holds.
    for (const p of panels) localPlidCounter = Math.max(localPlidCounter, p.plid + 1);
    return localPlidCounter++;
  });
}

// A NEW ANNOTATION NAMES ITSELF. Both primitives exist to carry a word, so
// creating one that says "LABEL 3" and leaving the player to find the rename
// gesture would be creating nothing. The rename box therefore opens the moment
// the object appears — but the object appears when the SERVER says so, and the
// server owns the id, so what is remembered here is the only thing this client
// knows about it: where it was put. `deadline` means a dropped op (budget
// full, a rect the server refused) expires instead of ambushing the next
// label that happens to land on that spot.
let pendingBoxName: { x0: number; y0: number; deadline: number } | null = null;
let pendingNetName: { x: number; y: number; deadline: number } | null = null;
const PENDING_NAME_MS = 3000;

/** Open the rename box over an annotation this client just created. Called
 * from the server broadcast handlers AND from the offline apply, so the
 * gesture feels identical with or without a server. */
function resolvePendingNames() {
  const now = performance.now();
  if (pendingBoxName) {
    const want = pendingBoxName;
    const b = labelBoxes.find((q) => q.x0 === want.x0 && q.y0 === want.y0);
    if (b) {
      pendingBoxName = null;
      renameLabelBox(b);
    } else if (now > want.deadline) {
      pendingBoxName = null;
    }
  }
  if (pendingNetName) {
    const want = pendingNetName;
    const l = netLabels.find((q) => q.x === want.x && q.y === want.y);
    if (l) {
      pendingNetName = null;
      renameNetLabel(l);
    } else if (now > want.deadline) {
      pendingNetName = null;
    }
  }
}

/** Rename a label box in place: the field lands over its own title plate. */
function renameLabelBox(b: LabelBox) {
  const [x, y] = labelBoxTitleAnchor(cam, b);
  const w = Math.max(90, (b.x1 - b.x0) * cam.scale - 6);
  openNameEditor(x, y, w, b.name, MAX_LABEL_BOX_NAME, (name) => {
    if (name !== b.name) labelBoxOp({ t: 'rename', blid: b.blid, name });
  });
}

/** Rename a net label in place: the field lands over its own plate. */
function renameNetLabel(l: NetLabel) {
  const [x, y, w] = netLabelRect(cam, l);
  openNameEditor(x, y, Math.max(90, w), l.name, MAX_NET_LABEL_NAME, (name) => {
    if (name !== l.name) netLabelOp({ t: 'rename', nlid: l.nlid, name });
  });
}

/** Label boxes are room state, exactly like panels: online the server owns
 * the list (its broadcast is the truth), offline the same rules run locally.
 *
 * NOTHING ELSE happens here. There is no window to open, no membership to
 * recompute and no widget list to touch — the entire feature is "the box now
 * says this, and everybody can see it". */
function labelBoxOp(op: LabelBoxOp) {
  if (online) {
    net.sendLabelBox(op);
    return;
  }
  labelBoxes = applyLabelBoxOp(labelBoxes, op, () => {
    for (const b of labelBoxes) localBlidCounter = Math.max(localBlidCounter, b.blid + 1);
    return localBlidCounter++;
  });
  resolvePendingNames();
}

/** Net labels, same rule. Offline there is no `netmap` broadcast (nothing is
 * deriving one), so every label reads detached until a server answers —
 * which is honest: this client genuinely does not know which net it is on. */
function netLabelOp(op: NetLabelOp) {
  if (online) {
    net.sendNetLabel(op);
    return;
  }
  netLabels = applyNetLabelOp(netLabels, op, () => {
    for (const l of netLabels) localNlidCounter = Math.max(localNlidCounter, l.nlid + 1);
    return localNlidCounter++;
  });
  resolvePendingNames();
}

// ------------------------------------------------------- external inputs
//
// THE PLAYER'S WHOLE INTERACTION, end to end:
//
//   1. press ⇧Y and drag out a rectangle — a CAMERA LAYER in the world;
//   2. click its plate: the browser asks for the camera (that click IS the
//      consent gesture, and nothing else in this app ever calls
//      getUserMedia);
//   3. press Y and drop a PHOTOCELL on top of the video;
//   4. wire it into a circuit. Wave at the camera and the circuit responds.
//
// There is no binding dialog and there must never be one. Which part reads
// which patch is re-derived from geometry every time the document or the
// layer moves — drag the part off and it goes dark, drag it back and it
// reads again.

/** Layers are room state: online the server owns the list, offline the same
 *  rules run locally (so the whole feature works with no server at all). */
function layerOp(op: LayerOp) {
  if (online) {
    net.sendLayer(op);
    return;
  }
  layers = applyLayerOp(layers, op, () => {
    for (const l of layers) localLidCounter = Math.max(localLidCounter, l.lid + 1);
    return localLidCounter++;
  });
  pushApertures();
}

/** The layer this client is driving, if any. */
const myLayer = (): Layer | null =>
  layers.find((l) => claims.get(l.lid) === myId) ?? (online ? null : (layers[0] ?? null));

/** Reused across every push: this runs on document edits and layer moves,
 *  and it must not litter. */
const apertureScratch: Aperture[] = [];

/** Re-derive which parts are over the driven layer and hand the sampler the
 *  new geometry. Cheap, and called from edits — never from the draw loop. */
function pushApertures() {
  const l = myLayer();
  if (!l || !camera.isLive()) return;
  aperturesOn(l, layers, elements, apertureScratch);
  camera.setApertures(apertureScratch, (l.x1 - l.x0) / (l.y1 - l.y0));
}

/** This browser's camera. Constructed here, started only from a click. */
const camera = new CameraSource(() => {
  syncSensorChrome();
  pushApertures();
});

/** Claim a layer and open the camera. MUST be called from a user gesture. */
async function claimLayer(l: Layer) {
  const holder = claims.get(l.lid);
  if (holder !== undefined && holder !== myId) {
    toast(`${l.name} is already driven by player ${holder}`);
    return;
  }
  if (camera.isLive() && holder === myId) {
    stopSensing();
    return;
  }
  if (online) net.sendLayerClaim(l.lid, true);
  else claims.set(l.lid, myId);
  // A click while the browser's own permission prompt is still up joins that
  // request rather than starting a second one — but it must not look like a
  // dead button while it waits.
  if (camera.getStatus().state === 'starting') {
    toast('still waiting on the browser — answer its camera prompt');
  }
  const ok = await camera.start();
  if (!ok) {
    if (online) net.sendLayerClaim(l.lid, false);
    else claims.delete(l.lid);
    const st = camera.getStatus();
    // THE REASON, ALWAYS. A player who cannot have a camera is told which of
    // the three things happened: the browser will not offer one on this
    // origin, there is none, or the answer was no.
    toast(
      st.state === 'denied'
        ? `no camera — nothing was captured (${st.detail})`
        : `no camera here — ${st.detail}`,
    );
  }
  syncSensorChrome();
  pushApertures();
}

/** ONE stop, always reachable, and it really stops the hardware. Every
 *  photocell it was driving falls dark within a tick, visibly, for everyone
 *  in the room — so switching your camera off is legible to the other
 *  players and not just to you. */
function stopSensing() {
  const l = myLayer();
  camera.stop();
  if (l) {
    if (online) net.sendLayerClaim(l.lid, false);
    else claims.delete(l.lid);
  }
  syncSensorChrome();
}

// Auto-stop: a camera must not stay live because a tab was left open.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'hidden' && camera.isLive()) stopSensing();
});

/**
 * The live indicator. Persistent and non-dismissible for as long as a track
 * is live — the browser's own tab dot is necessary and nowhere near
 * sufficient when it is the GAME asking for the camera.
 */
const sensorChip = document.createElement('div');
sensorChip.style.cssText =
  'position:fixed;left:50%;transform:translateX(-50%);bottom:14px;z-index:60;display:none;' +
  'align-items:center;gap:10px;padding:7px 12px;border-radius:20px;cursor:pointer;' +
  'background:#3a1216;border:1px solid #ff5a5a;color:#ffd2d2;font:12px ui-monospace,monospace';
sensorChip.onclick = () => stopSensing();
document.body.appendChild(sensorChip);

function syncSensorChrome() {
  const st = camera.getStatus();
  if (st.state !== 'live') {
    sensorChip.style.display = 'none';
    return;
  }
  const n = apertureScratch.length;
  sensorChip.style.display = 'flex';
  // A TRACK THAT DELIVERS NOTHING still reports `readyState: "live"`, so
  // "live" on its own is not a claim this chip is entitled to make. When no
  // frame has arrived for `CAMERA_SILENT_MS` the chip stops saying LIVE and
  // says what is actually true instead — the alternative is a black
  // rectangle under a label insisting the camera works.
  const silent = camera.silentMs();
  if (silent > CAMERA_SILENT_MS) {
    sensorChip.textContent =
      `● CAMERA DELIVERING NO FRAMES for ${(silent / 1000).toFixed(0)}s — ` +
      'the device is on but sending nothing; every sensor reads dark · click to stop';
    return;
  }
  sensorChip.textContent =
    `● CAMERA LIVE — driving ${n} sensor${n === 1 ? '' : 's'} · ` +
    `${st.msPerFrame.toFixed(2)} ms/frame · click to stop` +
    (st.autoExposure ? ' · AUTO-EXPOSURE fights the sensor' : '');
}

/** How long a live track may deliver nothing before the UI stops calling it
 *  live. Generous: a real camera can take a moment to produce its first
 *  frame, and crying dead on a slow start would be its own lie. */
const CAMERA_SILENT_MS = 2500;

/** Re-render the chip when, and only when, the stalled state flips. The chip
 *  is a DOM write, so it must not happen every tick just to age a counter;
 *  but a camera that dies while the document is untouched fires no other
 *  event, so something has to notice. `pumpSensors` already runs at 30 Hz. */
let cameraWasSilent = false;
function watchCameraLiveness() {
  const silent = camera.isLive() && camera.silentMs() > CAMERA_SILENT_MS;
  if (silent !== cameraWasSilent) {
    cameraWasSilent = silent;
    if (silent) toast('the camera is on but delivering no frames — every sensor is reading dark');
    syncSensorChrome();
  } else if (silent) {
    syncSensorChrome(); // the counter is part of the message
  }
}

/**
 * THE SENDER. One message per tick, at most, carrying `[[id, q]]` and
 * nothing else.
 *
 * Deliberately a timer and NOT the render loop: an external input must never
 * be able to make a frame late, and the render loop must never be the thing
 * that decides how fast a sensor streams. 30 Hz because the server retires
 * one write per part per tick and the tick is 30 Hz — sending faster buys
 * exactly nothing.
 */
const SENSOR_HZ = 30;
/** Below this many u16 counts, a reading has not moved. ~0.15% of full
 *  scale: it kills sensor noise on a still scene (and with it the server's
 *  refactorization) without being visible in a circuit. */
const SENSOR_DEADBAND = 100;
/** Resend an unchanged value at least this often anyway — the server fails a
 *  part to dark after 3 silent ticks, and a still scene is not a dead one. */
const SENSOR_KEEPALIVE_TICKS = 2;

const sensorLastSent = new Map<number, number>();
const sensorAge = new Map<number, number>();
const sensorBatch: [number, number][] = [];

function pumpSensors() {
  watchCameraLiveness();
  if (!camera.isLive()) {
    sensorLastSent.clear();
    sensorAge.clear();
    return;
  }
  const l = myLayer();
  if (!l) return;
  sensorBatch.length = 0;
  for (const a of apertureScratch) {
    const v = camera.read(a.id);
    if (v === null) continue;
    const q = Math.max(0, Math.min(65535, Math.round(v * 65535)));
    const was = sensorLastSent.get(a.id);
    const age = (sensorAge.get(a.id) ?? 99) + 1;
    if (was !== undefined && Math.abs(q - was) < SENSOR_DEADBAND && age < SENSOR_KEEPALIVE_TICKS) {
      sensorAge.set(a.id, age);
      continue;
    }
    sensorLastSent.set(a.id, q);
    sensorAge.set(a.id, 0);
    sensorBatch.push([a.id, q]);
    // Offline there is no server to write it, so the local sim gets the same
    // value through the same clamp — one code path, two hosts.
    if (!online) {
      const e = elemById(a.id);
      if (e && e.kind.t === 'Photocell') e.kind.light = q / 65535;
    }
  }
  // Once per tick, not once per sensor: offline this is a full recompile,
  // which is the local sim's only way in.
  if (!online && sensorBatch.length > 0) localSim.setElements(elements);
  if (online && sensorBatch.length > 0) net.sendSensor(sensorBatch);
  syncSensorChrome();
}
window.setInterval(pumpSensors, 1000 / SENSOR_HZ);

/** The floating instrument windows. panel.ts owns all DOM and widget logic;
 * we only hand it the shared list, the document, the live frame, the probe
 * traces (for scopes a region encloses) and the interact path. The
 * `floatScopes` array stays ours — panels borrow it, never own it. */
const panelHost = new PanelHost({
  elements: () => elements,
  live: () => live,
  probes: () => probes,
  traces: () => traces,
  scopes: () => floatScopes,
  netNames,
  removeScope: (sid) => {
    // Drop the throttle slot with the scope, or a closed instrument leaves a
    // pending timer that fires an op for a sid the room no longer has.
    const r = retune.get(sid);
    if (r?.timer) window.clearTimeout(r.timer);
    retune.delete(sid);
    scopeOp({ t: 'remove', sid });
  },
  // A widget retuned an instrument (control row, timebase wheel, channel
  // button). Same op the canvas sends, so the two surfaces cannot drift.
  scopeChanged: scopeRetuned,
  interact: (e, op) => interact(e, op),
  op: panelOp,
  hover: (h) => {
    panelHover = h;
  },
});

function nearestPin(e: ElementSpec, x: number, y: number): number {
  let best = 0;
  let bestD = Infinity;
  e.pins.forEach((p, k) => {
    const d = Math.hypot(cam.ox + p[0] * cam.scale - x, cam.oy + p[1] * cam.scale - y);
    if (d < bestD) {
      bestD = d;
      best = k;
    }
  });
  return best;
}

// ---------------------------------------------------------------- canvas
const canvas = document.getElementById('canvas') as HTMLCanvasElement;
const hud = document.getElementById('hud') as HTMLDivElement;
const ctx = canvas.getContext('2d')!;

/** The screen a sensor layer's plate has to stay inside: the canvas minus the
 *  HUD rail it would otherwise hide under. One definition, so the draw and
 *  the hit test cannot drift apart. */
function plateView(): [number, number, number] {
  const r = hud.getBoundingClientRect();
  return [window.innerWidth, window.innerHeight, r.height > 0 ? r.bottom + 6 : 0];
}

const cam: Camera = { scale: 48, ox: 60, oy: 60 };
// Exposed for end-to-end tests.
(window as unknown as { __cam: Camera }).__cam = cam;
// Likewise the document. A gesture test has to be able to ask what the pins
// actually ARE: screenshots cannot answer it (the current dots animate, so two
// frames of an untouched schematic differ), and "it looked right" is not a
// measurement. A getter, because `elements` is rebound whenever a room loads.
Object.defineProperty(window, '__els', { get: () => elements });
// And the instruments, for the same reason: a probe placed by an armed touch
// tool draws a flag on a canvas, and a canvas cannot be asked what it means.
Object.defineProperty(window, '__probes', { get: () => probes });
const dots = new DotFlow();
let mouse: { x: number; y: number } | null = null;

const toGrid = (x: number, y: number): [number, number] => [
  (x - cam.ox) / cam.scale,
  (y - cam.oy) / cam.scale,
];
const snap = (x: number, y: number): Point => {
  const [gx, gy] = toGrid(x, y);
  return [Math.round(gx), Math.round(gy)];
};
const toPx = (p: Point): [number, number] => [cam.ox + p[0] * cam.scale, cam.oy + p[1] * cam.scale];

/** Zoom range, px per grid unit. 0.4 shows ~4800 grid units across a
 * 1920 px window (the world is big now); 200 is knee-deep in one symbol. */
const MIN_SCALE = 0.4;
const MAX_SCALE = 200;

/** The starter district: joining frames THIS, not the whole document, so a
 * world with a 40k-element city two thousand units east still opens on the
 * demo bench. 'H' returns here, shift+H frames everything.
 *
 * It is ROOM DATA now, not a constant: a template says where its own world
 * begins (`view.home` in the hello), so THE HOIST frames the cabinet and the
 * bench in front of it rather than the showcase two hundred units away. The
 * literal below is only the fallback — offline, and against a server from
 * before rooms existed. */
const DEFAULT_HOME = { x0: -10, y0: -10, x1: 60, y1: 60 };
let homeRect = { ...DEFAULT_HOME };

/** True when `homeRect` came from a room's `view.home` — i.e. somebody chose
 * it — rather than from the fallback literal above. It is the difference
 * between a rect that is AUTHORED and one that is merely a default, and
 * `fitHome` treats the two completely differently. See there. */
let homeAuthored = false;

/** The rects, in grid units, of the instruments the room's template ships.
 * Part of the landing view: an author who puts a scope under the bench meant
 * the player to see the scope. Taken from the template's seeds, NOT from the
 * live bench, so 'H' stays idempotent and dragging a scope to the next county
 * never redefines home. */
let homeSeeds: [number, number, number, number][] = [];

/** The rect the camera is looking at right now, in grid units. This is the
 * `view.home` a template gets when the player saves this room as one. */
const camRect = (): [number, number, number, number] => {
  const [x0, y0] = toGrid(0, 0);
  const [x1, y1] = toGrid(window.innerWidth, window.innerHeight);
  return [x0, y0, x1, y1];
};

/** Frame a grid-space rect, with margin, clamped to a usable zoom band. */
function fitRect(x0: number, y0: number, x1: number, y1: number, loScale = 4, hiScale = 60) {
  const w = Math.max(1, x1 - x0 + 4);
  const ht = Math.max(1, y1 - y0 + 4);
  // The HUD rails overlay the canvas, so "fit" means fit into the part of it
  // that is not behind a sidebar. Nothing else in the camera is inset.
  const ins = panelHost.railInsets();
  const vw = Math.max(160, window.innerWidth - ins.left - ins.right);
  const fit = Math.min(vw / w, window.innerHeight / ht);
  cam.scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, Math.max(loScale, Math.min(hiScale, fit))));
  cam.ox = ins.left + (vw - (x0 + x1) * cam.scale) / 2;
  cam.oy = (window.innerHeight - (y0 + y1) * cam.scale) / 2;
}

/** Pin bbox of a list of elements; null when the list is empty. */
function pinBounds(list: ElementSpec[]): [number, number, number, number] | null {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const e of list) {
    for (const p of e.pins) {
      x0 = Math.min(x0, p[0]);
      y0 = Math.min(y0, p[1]);
      x1 = Math.max(x1, p[0]);
      y1 = Math.max(y1, p[1]);
    }
  }
  return isFinite(x0) ? [x0, y0, x1, y1] : null;
}

/**
 * 'H' / join / reset: go home.
 *
 * There are two kinds of home and they deserve opposite treatment.
 *
 * An AUTHORED home is a rect somebody chose — `view.home` from a room's
 * template — and it is honoured as written. Framing the parts inside it
 * instead would silently overrule the author: a district whose parts sit in
 * one corner would land the player on that corner, and the empty bench the
 * author left room for (to build on, to drop instruments on, to walk into)
 * would be off-screen. That is not a hypothetical either — it is what the
 * hoist room did: it framed the four fixture parts and left the scope the
 * template ships almost entirely outside the camera. Deliberate empty space
 * is CONTENT in a level, so the rect is the floor: the camera shows all of
 * it, plus the instruments the template ships, and never less.
 *
 * The DEFAULT home is not a choice, it is a fallback — offline, or against a
 * server from before rooms existed. Nobody framed it, so there is nothing to
 * honour, and the old behaviour is the right one: frame whatever is actually
 * standing in the starter district and only fall back to the bare rect when
 * it is empty.
 */
function fitHome() {
  if (homeAuthored) {
    let [x0, y0, x1, y1] = [homeRect.x0, homeRect.y0, homeRect.x1, homeRect.y1];
    for (const [sx0, sy0, sx1, sy1] of homeSeeds) {
      x0 = Math.min(x0, sx0);
      y0 = Math.min(y0, sy0);
      x1 = Math.max(x1, sx1);
      y1 = Math.max(y1, sy1);
    }
    // MIN_SCALE, not 8: a lower clamp on zoom is a licence to cut the rect in
    // half, which is the one thing this branch exists to prevent.
    fitRect(x0, y0, x1, y1, MIN_SCALE, 60);
    return;
  }
  const inHome = space
    .query(homeRect.x0, homeRect.y0, homeRect.x1, homeRect.y1)
    .filter((e) =>
      e.pins.every(
        ([x, y]) => x >= homeRect.x0 && x <= homeRect.x1 && y >= homeRect.y0 && y <= homeRect.y1,
      ),
    );
  const b = pinBounds(inHome);
  if (b) fitRect(...b, 8, 60);
  else fitRect(homeRect.x0, homeRect.y0, homeRect.x1, homeRect.y1, MIN_SCALE, 60);
}

/** shift+H: frame the whole document, however far it sprawls. */
function fitAll() {
  const b = pinBounds(elements);
  // Sensor layers count as content. A room whose parts sit below a camera
  // layer must frame BOTH, or ⇧H hides the very thing the parts are reading
  // — and a room that is nothing but a layer must still frame something.
  let r = b;
  for (const l of layers) {
    r = r
      ? [Math.min(r[0], l.x0), Math.min(r[1], l.y0), Math.max(r[2], l.x1), Math.max(r[3], l.y1)]
      : [l.x0, l.y0, l.x1, l.y1];
  }
  if (r) fitRect(...r, MIN_SCALE, 60);
  else fitHome();
}

function resize() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = window.innerWidth * dpr;
  canvas.height = window.innerHeight * dpr;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}
window.addEventListener('resize', resize);
resize();
fitHome();

// ------------------------------------------------------------- part arming
let placing: PartDef | null = null;
let placeRot = 0; // 0..3, quarter turns; Q rotates

// ---- the armed instruments, which exist only for fingers
//
// '1', '2' and '3' probe WHATEVER THE MOUSE IS HOVERING. A finger does not
// hover: it is down or it is gone, so those three keys have no translation at
// all — the only honest one is to arm the instrument first and let the next
// tap say where. The touch palette arms it, `touchDown`/`touchUp` fire it and
// `disarmTools` drops it; nothing else in the file reads it, so no mouse or
// pen gesture can ever reach this state. (Declared up here with the other
// armed-tool state, not down in the touch block, so `disarmTools` can never
// be reached before it exists.)
type TouchTool = 'v' | 'i' | 'listen';
let touchTool: TouchTool | null = null;
/** The finger carrying an armed instrument. `live` goes false the moment the
 *  finger disqualifies itself — by travelling (that is a pan, not an aim) or
 *  by a second finger joining (two fingers are ALWAYS the camera). Firing on
 *  the way UP rather than on the way down is what makes that possible. */
let touchShot: { id: number; x: number; y: number; live: boolean } | null = null;
/** How far a finger may slide and still count as a tap, in CSS px. */
const TOUCH_TAP_SLOP = 12;

const ROT_DIRS: Point[] = [
  [1, 0],
  [0, 1],
  [-1, 0],
  [0, -1],
];

/** Default far endpoint for a click-place at `a` with the armed rotation. */
function placeEnd(a: Point): Point {
  const d = ROT_DIRS[placeRot]!;
  return [a[0] + d[0] * 4, a[1] + d[1] * 4];
}

// Armed mirrors, so an orientation can be chosen BEFORE the part lands.
// Sticky across placements, like placeRot.
let placeFlipX = false;
let placeFlipY = false;

/** Mirror pins about their own bounding box — the same exact involution the
 *  selection flip uses, so armed and after-the-fact flips agree. */
function mirrorPins(pins: Point[], axis: 'x' | 'y'): Point[] {
  const i = axis === 'x' ? 0 : 1;
  let lo = Infinity;
  let hi = -Infinity;
  for (const p of pins) {
    if (p[i] < lo) lo = p[i];
    if (p[i] > hi) hi = p[i];
  }
  const sum = lo + hi;
  return pins.map(([x, y]) => (i === 0 ? [sum - x, y] : [x, sum - y]) as Point);
}

/** What each pin gesture is called in the undo list. */
const GESTURE_LABEL: Record<PinGesture, string> = {
  free: 'reshape part',
  swing: 'reorient part',
  carry: 'move part',
};

/** Put a part that predates the shape rule back into formation, the moment a
 *  player takes hold of one of its terminals. Returns false only if the
 *  straightened document would not run, in which case the drag never starts.
 *
 *  DELIBERATELY NOT UNDOABLE back to the skew, and this is the one place the
 *  editor throws away a state on purpose. The reason is that the shape rule
 *  is monotone — a skewed part may be moved, rotated and flipped, but nothing
 *  may become newly skewed — so an undo entry holding the old shape would be
 *  an entry the server refuses. Better to have no way back to a shape the
 *  editor can no longer draw than a ⌘Z that fails with a sentence about
 *  rigid bodies. The straightening is announced instead, which is what an
 *  irreversible change owes the player.
 *
 *  Old rooms are the only place this fires: it is the migration, done one
 *  part at a time, by the person who asked for that part to move. Nothing
 *  rewrites a saved document behind anyone's back. */
function straightenBeforeDrag(e: ElementSpec): boolean {
  if (pinCount(e.kind) <= 2 || isRigidPlacement(e.kind, e.pins)) return true;
  const op: DocOp = { t: 'Move', id: e.id, pins: straightenPins(e.kind, e.pins) };
  if (refused(candidateDoc(op), 'edit')) return false;
  applyDoc(op);
  if (online) net.sendEdit(op);
  else localSim.setElements(elements);
  toast(`#${e.id} straightened — ${rigidHint(e.kind)}`);
  return true;
}

/** Pin layout for a placement, with the armed mirrors applied. */
function placePins(kind: ElementKind, a: Point, b: Point): Point[] {
  let pins = makePins(kind, a, b);
  if (placeFlipX) pins = mirrorPins(pins, 'x');
  if (placeFlipY) pins = mirrorPins(pins, 'y');
  return pins;
}

/** Drop every armed tool. ARMING IS EXCLUSIVE: the region tools are hit
 * before `placing` in pointerdown, so a tool left armed from a minute ago
 * would silently eat the next part a player tried to place. */
function disarmTools() {
  panelTool = false;
  panelDrag = null;
  labelBoxTool = false;
  labelBoxDrag = null;
  netLabelTool = false;
  layerTool = false;
  layerDrag = null;
  repairing = false;
  // The touch instruments are armed tools like any other, so Esc, a hotkey
  // and the palette's × all put them down through this one door. Always null
  // on a desktop session: only the touch palette can ever set it.
  touchTool = null;
}

function choosePart(p: PartDef) {
  disarmTools();
  placing = p;
  pasting = null;
  closeCtxMenu();
  canvas.style.cursor = 'crosshair';
}

// ------------------------------------------------------ on-canvas rename
//
// The one genuinely new bit of UI the annotation primitives needed. A panel
// renames through its window header; a label box and a net label have no
// window, so the field comes to the plate instead: double-click a title and a
// text box appears exactly over it, sized like it, committing on Enter or
// blur and abandoning on Escape.
//
// It is length-capped CLIENT-SIDE as well as on the server. The server
// truncates whatever arrives, but a field that silently eats the end of what
// you typed is a field that lied to you — which is exactly the gap the panel
// title input still has.

const nameEdit = document.getElementById('nameedit') as HTMLInputElement;
/** What the open editor will do with the text. Null = nothing is open. */
let nameEditCommit: ((name: string) => void) | null = null;

function closeNameEditor() {
  if (!nameEditCommit) return;
  nameEditCommit = null;
  nameEdit.style.display = 'none';
  nameEdit.value = '';
  nameEdit.blur();
}

/** Open the rename box at `[x, y]` screen px, `w` wide, over `current`.
 * `commit` is called with the trimmed text only when it actually changed. */
function openNameEditor(
  x: number,
  y: number,
  w: number,
  current: string,
  maxLen: number,
  commit: (name: string) => void,
) {
  closeNameEditor();
  nameEditCommit = commit;
  nameEdit.maxLength = maxLen;
  nameEdit.value = current;
  nameEdit.style.display = 'block';
  nameEdit.style.width = `${Math.max(70, Math.round(w))}px`;
  // Kept inside the viewport: a box drawn at the edge of the world must not
  // put its own rename field off-screen.
  const r = nameEdit.getBoundingClientRect();
  nameEdit.style.left = `${Math.round(Math.min(Math.max(4, x), Math.max(4, window.innerWidth - r.width - 4)))}px`;
  nameEdit.style.top = `${Math.round(Math.min(Math.max(4, y), Math.max(4, window.innerHeight - r.height - 4)))}px`;
  nameEdit.focus();
  nameEdit.select();
}

nameEdit.addEventListener('keydown', (ev) => {
  ev.stopPropagation(); // never let a part hotkey fire while typing a name
  if (ev.key === 'Enter') {
    ev.preventDefault();
    nameEdit.blur(); // blur commits
  } else if (ev.key === 'Escape') {
    ev.preventDefault();
    closeNameEditor(); // clears the callback first, so the blur does nothing
  }
});
nameEdit.addEventListener('blur', () => {
  const commit = nameEditCommit;
  if (!commit) return;
  const name = nameEdit.value.trim();
  closeNameEditor();
  if (name) commit(name);
});

// ---------------------------------------------------------------- props
const propsDiv = document.getElementById('props') as HTMLDivElement;
const propsDlg = document.getElementById('propsdlg') as HTMLDivElement;
/** Element the floating editor is open for (double-click / context menu). */
let dlgFor: number | null = null;

// The unit is no longer part of the label: it is part of the VALUE now, and
// the field carries it (`4.7 kΩ`, not `4700` under a heading that says Ω).
// Repeating it in the label just made the two disagree whenever the value
// needed a prefix.
const FIELD_LABELS: Record<string, string> = {
  ohms: 'resistance',
  rated_watts: 'rated power',
  farads: 'capacitance',
  henries: 'inductance',
  dc: 'DC',
  amp: 'AC amplitude',
  hz: 'frequency',
  phase: 'phase',
  amps: 'current',
  closed: 'closed',
  vz: 'zener',
  color: 'color 0-4',
  beta: 'beta',
  vt: 'threshold',
  k: 'k',
  rail: 'rail ±',
  isc: 'out limit',
  wiper: 'wiper 0-1',
  // The unit letter is dropped from these labels on purpose: the value field
  // itself now renders "9 V" / "1 MΩ" through `units.ts`, so repeating it in
  // the label just says the same thing twice.
  volts: 'amplitude ±',
  // A motor's back-EMF constant had NO entry here, so it rendered with its
  // raw serde field name.
  bemf: 'back-EMF K',
  seed: 'seed (whole)',
  // The photocell's CALIBRATION — the two ends of its travel. Its reading is
  // not here and must never be: `light` is world state, written by whoever
  // has a camera on it, and a text box that pretended otherwise would be the
  // first faked number in the project.
  r_dark: 'dark',
  r_lit: 'lit',
  // The logic family's own parameters.
  op: 'function',
  wave: 'waveform',
  ins: 'inputs 1-4',
  edge: 'edge triggered',
  bits: 'bits 2-4',
  modulus: 'counts to',
  sel: 'select lines 1-2',
};
/** Gate functions, in menu order. Mirrors `sim_core::GateOp`. */
const GATE_OPS: GateOp[] = ['And', 'Nand', 'Or', 'Nor', 'Xor', 'Xnor', 'Buf', 'Not'];

/** Source waveforms, in menu order. Mirrors `sim_core::Wave`. */
const WAVES: Wave[] = ['Sine', 'Square', 'Triangle', 'Saw'];

/** The options for an enum-valued property field.
 *
 *  Keyed by FIELD NAME rather than by part, because the field name is what
 *  `buildProps` is iterating and every enum field in the document happens to
 *  have a name unique to its meaning. A field with no entry gets a plain text
 *  box, so adding an enum without touching this is a visible omission rather
 *  than a silent one. */
const ENUM_OPTIONS: Record<string, readonly string[]> = {
  op: GATE_OPS,
  wave: WAVES,
};

/** Parameters that decide how many pins a part has. `SetKind` refuses any
 *  change to the pin count (the footprint would have to move under the
 *  wires), so the panel shows these read-only instead of offering an edit
 *  the server will drop on the floor. */
const WIDTH_FIELDS = new Set(['ins', 'bits', 'sel']);


/** A property field that carries a physical quantity.
 *
 *  This replaces `<input type="number">`, which could not be kept: a number
 *  input rejects "4k7" at the DOM level and hands JavaScript back an empty
 *  string, so no amount of parsing downstream would ever see what the player
 *  typed. Going to `type="text"` costs the native spinner and the numeric
 *  keypad, so both are put back by hand — the arrows here are BETTER than the
 *  ones they replace, because stepping a 100 nF capacitor by 1 (which is what
 *  `step=any` did) is useless and stepping it along a 1-2-5 ladder is not.
 *
 *  The document is edited only when the text actually changed. That is the
 *  second half of the round-trip guarantee: `fmtEntry` makes the string
 *  faithful, and this makes an untouched field a no-op even for the values no
 *  short string can reproduce. */
function valueField(
  q: Quantity,
  initial: number,
  onCommit: (v: number) => void,
): { input: HTMLInputElement; note: HTMLDivElement } {
  const input = document.createElement('input');
  input.type = 'text';
  input.inputMode = 'decimal';
  input.autocomplete = 'off';
  input.spellcheck = false;
  const note = document.createElement('div');
  note.className = 'pnote';

  let value = initial;
  let shown = fmtEntry(initial, q);
  input.value = shown;
  // The legal range IS the field's own constraint, so the player meets it
  // here rather than as a server rejection bouncing back a round trip later.
  const range = rangeText(q);
  input.title = range ? `range ${range}${q.prefixed ? '  ·  prefixes: p n µ m k M G' : ''}` : '';

  const hint = () => {
    input.classList.remove('bad');
    note.className = 'pnote';
    note.textContent = '';
    // ---- PART 2 PROTOTYPE. Off unless the player opted in; see eseries.ts.
    if (stdValuesMode() !== 'hint' || !q.series || !Number.isFinite(value) || value <= 0) return;
    const series = q.series;
    if (isPreferred(value, series)) {
      note.textContent = `✓ ${series} standard value`;
      note.classList.add('good');
      return;
    }
    // Both neighbours, nearest first — the point is that a stock value is a
    // CHOICE between two rungs, not a correction to one.
    const [lo, hi] = preferredNeighbours(value, series);
    const near = nearestPreferred(value, series);
    const cands = lo === hi ? [near] : near === hi ? [hi, lo] : [lo, hi];
    note.append(`not stocked · nearest ${series}: `);
    cands.forEach((cand, i) => {
      if (i) note.append(' · ');
      const a = document.createElement('span');
      a.className = 'snap';
      a.textContent = fmtEntry(cand, q);
      a.title = 'use this value';
      // Snapping is an explicit click. Nothing here ever changes a value on
      // its own — that is the difference between a prototype and a decision.
      a.onclick = () => {
        input.value = fmtEntry(cand, q);
        commit();
      };
      note.append(a);
    });
    const why = document.createElement('span');
    why.className = 'why';
    why.textContent = ' ⓘ';
    why.title = 'why do parts come in fixed values?';
    why.onclick = () => {
      const open = note.querySelector('.explain');
      if (open) {
        open.remove();
        return;
      }
      const box = document.createElement('div');
      box.className = 'explain';
      for (const para of seriesExplainer(series)) {
        const p = document.createElement('p');
        p.textContent = para;
        box.appendChild(p);
      }
      note.appendChild(box);
    };
    note.append(why);
  };

  const commit = () => {
    const text = input.value;
    // Unchanged text is never an edit. This is what makes "open the dialog
    // and press enter" provably a no-op on every value in every saved room.
    if (text === shown) {
      hint();
      return;
    }
    const r = parseField(text, q);
    if (!r.ok) {
      input.classList.add('bad');
      note.className = 'pnote err';
      note.textContent = r.err;
      return;
    }
    value = r.value;
    shown = fmtEntry(r.value, q);
    input.value = shown;
    hint();
    onCommit(r.value);
  };

  input.addEventListener('change', commit);
  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      commit();
    } else if (ev.key === 'Escape') {
      input.value = shown;
      hint();
      input.blur();
    } else if (ev.key === 'ArrowUp' || ev.key === 'ArrowDown') {
      ev.preventDefault();
      const cur = parseEng(input.value, q);
      const base = cur.ok ? cur.value : value;
      // With the Part 2 opt-in on, the arrows walk the stock ladder instead
      // of 1-2-5 — the detents ARE the lesson, felt in the fingers.
      const mants =
        stdValuesMode() === 'hint' && q.series ? seriesLadder(q.series) : undefined;
      input.value = fmtEntry(stepLadder(base, ev.key === 'ArrowUp' ? 1 : -1, q, mants), q);
      commit();
    }
  });

  hint();
  return { input, note };
}

/** The property editor, rendered into any host box. Used twice: docked
 *  (single-click selection) and floating next to the part (double-click).
 *  `onClose` — when given — adds a × button and is called after a delete. */
function buildProps(host: HTMLElement, target: ElementSpec, onClose?: () => void) {
  const mark = (kind: ElementSpec['kind']) => {
    host.dataset.key = JSON.stringify([target.id, kind]);
  };
  mark(target.kind);
  host.innerHTML = '';
  const h = document.createElement('h3');
  // The tier is the part's RATING, not its behaviour: two resistors at
  // different tiers solve identically and only differ in what they can take,
  // so it belongs in the header next to the id rather than in the field list
  // with the electrical parameters. The authority on what a tier can take is
  // `crates/damage`; the client deliberately does not keep a second copy of
  // that table to disagree with.
  const tier = target.tier ?? 0;
  h.textContent = `${target.kind.t}  #${target.id}${tier > 0 ? `  ·  tier ${tier}` : ''}`;
  host.appendChild(h);

  // The NAME, first, above the electrical parameters — it is what a player
  // reads on the control panel, and it is the only field here that is purely
  // for them. Naming a knob CUTOFF is why the synth no longer needs a panel
  // region per switch just to borrow the region's name.
  {
    const label = document.createElement('label');
    const span = document.createElement('span');
    span.textContent = 'name';
    label.appendChild(span);
    const input = document.createElement('input');
    input.type = 'text';
    input.maxLength = MAX_NAME;
    input.placeholder = `${target.kind.t} #${target.id}`;
    input.value = target.name ?? '';
    input.autocomplete = 'off';
    input.spellcheck = false;
    const commit = () => {
      const name = input.value.trim().slice(0, MAX_NAME);
      // No op when nothing changed: tabbing through a field must not fill a
      // player's undo stack, and must not broadcast to the room.
      if (name === (target.name ?? '')) return;
      editDoc({ t: 'SetName', id: target.id, name });
    };
    input.onchange = commit;
    input.onblur = commit;
    input.onkeydown = (ev) => {
      if (ev.key === 'Enter') {
        ev.preventDefault();
        input.blur();
      }
    };
    label.appendChild(input);
    host.appendChild(label);
  }
  if (onClose) {
    const x = document.createElement('button');
    x.className = 'xbtn';
    x.textContent = '×';
    x.title = 'close (Esc)';
    x.onclick = onClose;
    h.appendChild(x);
  }

  // A source written before waveforms existed has NO `wave` key, and serde
  // defaults it to sine on the way in. Iterating the object alone would then
  // show no picker at all on exactly the sources a player most wants to
  // change, so the default is materialised here: choosing a shape writes the
  // field, and leaving it alone writes nothing.
  const entries = Object.entries(target.kind);
  if (
    (target.kind.t === 'VoltageSource' || target.kind.t === 'Rail') &&
    !entries.some(([f]) => f === 'wave')
  ) {
    entries.push(['wave', 'Sine']);
  }
  for (const [field, value] of entries) {
    if (field === 't') continue;
    const label = document.createElement('label');
    const span = document.createElement('span');
    span.textContent = FIELD_LABELS[field] ?? field;
    label.appendChild(span);
    if (typeof value === 'boolean') {
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.checked = value;
      input.onchange = () => {
        const kind = { ...target.kind, [field]: input.checked } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        mark(kind);
      };
      label.appendChild(input);
      host.appendChild(label);
    } else if (typeof value === 'string') {
      // A string field is an enum (only `Gate.op` today), so it gets a menu
      // rather than a text box — and the menu lists ONLY the options that
      // keep the part's pin count, because `SetKind` refuses a width change
      // and a control that silently does nothing is worse than no control.
      // Swapping a NAND for an inverter is placing a different part, not
      // editing this one.
      const sel = document.createElement('select');
      const width = pinCount(target.kind);
      for (const opt of ENUM_OPTIONS[field] ?? []) {
        const cand = { ...target.kind, [field]: opt } as ElementSpec['kind'];
        if (pinCount(cand) !== width) continue;
        const o = document.createElement('option');
        o.value = opt;
        o.textContent = opt.toUpperCase();
        o.selected = opt === value;
        sel.appendChild(o);
      }
      sel.onchange = () => {
        const kind = { ...target.kind, [field]: sel.value } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        mark(kind);
      };
      label.appendChild(sel);
      host.appendChild(label);
    } else if (WIDTH_FIELDS.has(field)) {
      // Fields that decide a PIN COUNT cannot be retargeted in place: the
      // footprint would have to change under the wires. Shown, so the part
      // stays legible, and disabled with the reason, rather than offered and
      // then refused by the server.
      const input = document.createElement('input');
      input.type = 'number';
      input.value = String(value);
      input.disabled = true;
      input.title = 'changes the pin count - place a different part instead';
      label.appendChild(input);
      host.appendChild(label);
    } else {
      const q = quantityOf(target.kind.t, field);
      const f = valueField(q, value as number, (v) => {
        const kind = { ...target.kind, [field]: v } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        mark(kind);
      });
      label.appendChild(f.input);
      host.appendChild(label);
      host.appendChild(f.note);
    }
  }

  const row = document.createElement('div');
  row.className = 'row';
  const rot = document.createElement('button');
  rot.textContent = '⟳ rotate (Q)';
  rot.onclick = () => rotateElements(groupOf(target));
  const del = document.createElement('button');
  del.textContent = '✕ delete';
  del.onclick = () => {
    deleteElements(groupOf(target));
    onClose?.();
  };
  row.appendChild(rot);
  row.appendChild(del);
  host.appendChild(row);
}

/** Docked panel: mirrors a single-part selection. */
function syncPropsPanel() {
  const target =
    selectedIds.size === 1 ? elemById([...selectedIds][0]!) : undefined;
  if (!target) {
    propsDiv.style.display = 'none';
    propsDiv.dataset.key = '';
    return;
  }
  const key = JSON.stringify([target.id, target.kind]);
  if (key === propsDiv.dataset.key) return;
  if (
    propsDiv.contains(document.activeElement) &&
    (propsDiv.dataset.key ?? '').startsWith(`[${target.id},`)
  ) {
    return;
  }
  propsDiv.style.display = 'block';
  buildProps(propsDiv, target);
}

/** Bounding box of a part in screen pixels. */
function boundsPx(e: ElementSpec): [number, number, number, number] {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const p of e.pins) {
    const [x, y] = toPx(p);
    x0 = Math.min(x0, x);
    y0 = Math.min(y0, y);
    x1 = Math.max(x1, x);
    y1 = Math.max(y1, y);
  }
  return [x0, y0, x1, y1];
}

function openPropsDialog(target: ElementSpec) {
  dlgFor = target.id;
  selectedIds = new Set([target.id]);
  selectedProbe = null;
  selectedMachine = false;
  propsDlg.style.display = 'block';
  propsDlg.style.left = '0px';
  propsDlg.style.top = '0px';
  buildProps(propsDlg, target, closePropsDialog);
  placeDialogNear(target);
  const first = propsDlg.querySelector('input');
  if (first instanceof HTMLInputElement) first.focus();
}

/** Park the floating editor just clear of the part, inside the viewport. */
function placeDialogNear(target: ElementSpec) {
  const [x0, y0, x1, y1] = boundsPx(target);
  const r = propsDlg.getBoundingClientRect();
  let x = x1 + 18;
  if (x + r.width + 8 > window.innerWidth) x = x0 - r.width - 18;
  const y = (y0 + y1) / 2 - r.height / 2;
  propsDlg.style.left = `${Math.round(Math.min(Math.max(8, x), Math.max(8, window.innerWidth - r.width - 8)))}px`;
  propsDlg.style.top = `${Math.round(Math.min(Math.max(8, y), Math.max(8, window.innerHeight - r.height - 8)))}px`;
}

function closePropsDialog() {
  dlgFor = null;
  propsDlg.style.display = 'none';
  propsDlg.dataset.key = '';
  propsDlg.innerHTML = '';
}

/** Keep the floating editor honest about server/peer edits; close if the
 *  part is gone. Never rebuilds while the player is typing in it. */
function syncPropsDialog() {
  if (dlgFor === null) return;
  const target = elemById(dlgFor);
  if (!target) {
    closePropsDialog();
    return;
  }
  const key = JSON.stringify([target.id, target.kind]);
  if (key === propsDlg.dataset.key) return;
  if (propsDlg.contains(document.activeElement)) return;
  buildProps(propsDlg, target, closePropsDialog);
}

/** What a part-targeted command acts on: the selection when the part is part
 *  of a multi-selection, otherwise just the part. */
function groupOf(e: ElementSpec): ElementSpec[] {
  if (selectedIds.has(e.id) && selectedIds.size > 1) {
    return elements.filter((x) => selectedIds.has(x.id));
  }
  return [e];
}

/** Integer centroid of a set of parts, in grid units. */
function centroidOf(sel: ElementSpec[]): Point {
  let sx = 0;
  let sy = 0;
  let n = 0;
  for (const e of sel) {
    for (const p of e.pins) {
      sx += p[0];
      sy += p[1];
      n++;
    }
  }
  return [Math.round(sx / n), Math.round(sy / n)];
}

/** Rotate parts 90° clockwise about their shared centroid.
 *
 *  Two things turn, and a part usually only has one of them. PINS carry the
 *  orientation of every multi-pin part: rotate them and the symbol follows,
 *  because the renderer derives the body from the pin geometry. A ONE-PIN
 *  part (Ground, Rail) has nothing to rotate — its single pin maps to itself
 *  about its own centre — so its orientation is a separate quarter-turn
 *  count carried in the shared document, and this is where it advances.
 *
 *  Both ride the same `Move`, so a rotation is one op, one undo entry and
 *  one broadcast whichever kind of part it lands on. And because `rot` never
 *  reaches the netlist, turning a ground symbol cannot change one number in
 *  the circuit — which is the whole point: it is a drawing decision. */
function rotateElements(sel: ElementSpec[]) {
  if (sel.length === 0) return;
  const [cx, cy] = centroidOf(sel);
  history.begin(sel, sel.length > 1 ? `rotate ${sel.length} parts` : 'rotate part');
  for (const e of sel) {
    const pins = e.pins.map(([x, y]) => [cx - (y - cy), cy + (x - cx)] as Point);
    editDoc({ t: 'Move', id: e.id, pins, rot: ((e.rot ?? 0) + 1) & 3 });
  }
  history.end();
}

const rotateSelection = () => rotateElements(elements.filter((e) => selectedIds.has(e.id)));

/** Mirror parts about their bounding box: 'x' flips left-right (about the
 *  vertical axis), 'y' flips top-bottom — the KiCad convention. Pin ORDER is
 *  untouched, so terminal identity survives: a mirrored op-amp keeps in+ as
 *  pin 0, it just sits on the other side.
 *
 *  The mirror is about the bounding box rather than the centroid because
 *  `min + max - v` is an exact involution on the integer grid: the box maps
 *  onto itself, so flipping twice lands exactly where you started. A rounded
 *  centroid does not — a selection whose mean falls on a half unit would
 *  walk sideways every time you flipped it. */
function flipElements(sel: ElementSpec[], axis: 'x' | 'y') {
  if (sel.length === 0) return;
  const i = axis === 'x' ? 0 : 1;
  let lo = Infinity;
  let hi = -Infinity;
  for (const e of sel) {
    for (const p of e.pins) {
      if (p[i] < lo) lo = p[i];
      if (p[i] > hi) hi = p[i];
    }
  }
  const sum = lo + hi;
  const many = sel.length > 1 ? ` ${sel.length} parts` : ' part';
  history.begin(sel, `flip${many} ${axis === 'x' ? 'horizontally' : 'vertically'}`);
  for (const e of sel) {
    // Mirror about the SELECTION's box, not each part's own, so a group
    // keeps its arrangement instead of every part flipping in place.
    const pins = e.pins.map(
      ([x, y]) => (axis === 'x' ? [sum - x, y] : [x, sum - y]) as Point,
    );
    editDoc({ t: 'Move', id: e.id, pins });
  }
  history.end();
}

const flipSelection = (axis: 'x' | 'y') =>
  flipElements(
    elements.filter((e) => selectedIds.has(e.id)),
    axis,
  );

/** Bulk-delete threshold: above this, rebuild the array (and the local
 * netlist) once instead of once per element — a marquee over a district can
 * hold thousands of parts and per-op work is quadratic. */
const BULK_DELETE = 32;

function deleteIds(all: number[]) {
  // Server-owned machine fixtures survive every delete path (the bulk branch
  // below bypasses editDoc, so filtering has to happen here).
  const ids = all.filter((id) => !isFixtureId(id));
  if (ids.length === 0) return;
  // One undo entry per deletion, wherever it was triggered from (key, menu,
  // properties dialog), however many parts it removes.
  history.begin(
    ids.map((id) => elemById(id)),
    ids.length > 1 ? `delete ${ids.length} parts` : 'delete part',
  );
  if (ids.length <= BULK_DELETE) {
    for (const id of ids) editDoc({ t: 'Remove', id });
    history.end();
    return;
  }
  const gone = new Set(ids);
  for (const id of gone) {
    // Record before mutating: the bulk path bypasses editDoc for speed, so
    // history would otherwise miss the whole deletion.
    history.record({ t: 'Remove', id }, elements);
    space.remove(id);
    selectedIds.delete(id);
    if (online) net.sendEdit({ t: 'Remove', id }); // the server still sees every op
  }
  elements = elements.filter((e) => !gone.has(e.id));
  docVersion++; // the bulk path bypasses applyDoc, so bump it by hand
  pushApertures(); // ...and its aperture re-derive with it
  if (!online) localSim.setElements(elements);
  history.end();
}

const deleteElements = (sel: ElementSpec[]) => deleteIds(sel.map((e) => e.id));

function copyElements(sel: ElementSpec[]) {
  if (sel.length === 0) return;
  const [cx, cy] = centroidOf(sel);
  clipboard = sel.map((e) => ({
    kind: JSON.parse(JSON.stringify(e.kind)) as ElementKind,
    // Copied geometry is STRAIGHTENED. A paste is an `Add`, and an added
    // part must be in formation — there is no grandfather clause for a part
    // that did not exist a moment ago. Doing it here rather than at the
    // paste means the ghost under the cursor shows what will actually land,
    // and copying a legacy op-amp out of an old room is how you get a good
    // one. Nothing is connected to a part that has not been pasted yet, so
    // the snap cannot pull a terminal off a junction.
    pins: straightenPins(e.kind, e.pins).map(([x, y]) => [x - cx, y - cy] as Point),
    tier: e.tier ?? 0,
    rot: e.rot ?? 0,
  }));
}

const copySelection = () => copyElements(elements.filter((e) => selectedIds.has(e.id)));

function selectAll() {
  selectedIds = new Set(elements.map((e) => e.id));
  selectedProbe = null;
  selectedMachine = false;
}

function commitPaste(at: Point) {
  if (!pasting) return;
  const ids: number[] = [];
  history.begin(); // the whole paste undoes as one step
  for (const item of pasting) {
    const id = newId();
    ids.push(id);
    editDoc({
      t: 'Add',
      spec: {
        id,
        kind: JSON.parse(JSON.stringify(item.kind)) as ElementKind,
        pins: item.pins.map(([x, y]) => [x + at[0], y + at[1]] as Point),
        tier: item.tier ?? 0,
        rot: item.rot ?? 0,
      },
    });
  }
  history.end();
  selectedIds = new Set(ids);
  selectedProbe = null;
  selectedMachine = false;
  pasting = null;
}

/** Paste the clipboard straight down at `at` (context-menu Paste). */
function pasteAt(at: Point) {
  if (clipboard.length === 0) return;
  pasting = clipboard.map((c) => ({ ...c }));
  placing = null;
  commitPaste(at);
}

/** Arm the cursor-bound paste ghost (⌘/Ctrl+V). */
function armPaste() {
  if (clipboard.length === 0) return;
  pasting = clipboard.map((c) => ({ ...c }));
  placing = null;
  canvas.style.cursor = 'crosshair';
}

// ---------------------------------------------------------------- input
/** Pointer hit-test slack in px (matches elementAt's threshold). */
const HIT_PX = 14;

/** Elements whose bbox could reach a cursor point: one bucket query, never
 * a document scan. `padPx` is the hit slack, `padGrid` covers the body
 * distance hitTest allows past the pins. */
const hitScratch: ElementSpec[] = [];
function nearCursor(x: number, y: number, padPx: number, padGrid = 1): ElementSpec[] {
  const [gx, gy] = toGrid(x, y);
  const pad = padPx / cam.scale + padGrid;
  return space.query(gx - pad, gy - pad, gx + pad, gy + pad, hitScratch);
}

function elementAt(x: number, y: number): ElementSpec | undefined {
  let best: ElementSpec | undefined;
  let bestD = HIT_PX;
  let bestSeq = Infinity;
  for (const e of nearCursor(x, y, HIT_PX)) {
    const d = hitTest(cam, e, x, y);
    const seq = space.seqOf(e.id);
    // Bucket order is not document order: break exact ties the way the old
    // document-order scan did (first spec wins) so picking stays stable.
    if (d < bestD || (d === bestD && seq < bestSeq)) {
      bestD = d;
      bestSeq = seq;
      best = e;
    }
  }
  return best;
}

/** Grid point of the nearest element pin, if the cursor is on one. */
/** The element that OWNS the pin under the cursor (and which pin), for
 * reshape-dragging. At a junction several parts share the point: a selected
 * part wins, so the pin you see highlighted is the pin you grab. Fixture
 * children are bolted down and never reshape. */
function pinOwnerAt(x: number, y: number): { e: ElementSpec; k: number } | null {
  const r = Math.min(HIT_PX, cam.scale * 0.4);
  let best: { e: ElementSpec; k: number } | null = null;
  let bestScore = r;
  for (const e of nearCursor(x, y, r, 0)) {
    if (isFixtureId(e.id)) continue;
    for (let k = 0; k < e.pins.length; k++) {
      const [px, py] = toPx(e.pins[k]!);
      const d = Math.hypot(px - x, py - y);
      const score = selectedIds.has(e.id) ? d - HIT_PX : d; // selected wins ties
      if (d < r && score < bestScore) {
        bestScore = score;
        best = { e, k };
      }
    }
  }
  return best;
}

function pinAt(x: number, y: number): Point | null {
  const r = Math.min(HIT_PX, cam.scale * 0.4);
  let best: Point | null = null;
  let bestD = r;
  for (const e of nearCursor(x, y, r, 0)) {
    for (const p of e.pins) {
      const [px, py] = toPx(p);
      const d = Math.hypot(px - x, py - y);
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
  }
  return best;
}

/** Does any element have a pin exactly at grid point `p`? */
function pinExistsAt(p: Point, excludeId = -1): boolean {
  for (const e of space.query(p[0] - 1, p[1] - 1, p[0] + 1, p[1] + 1)) {
    if (e.id === excludeId) continue;
    if (e.pins.some((q) => q[0] === p[0] && q[1] === p[1])) return true;
  }
  return false;
}

function probeFlagPx(p: Probe): [number, number] | null {
  const e = elemById(p.elem);
  if (!e) return null;
  const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
  const [x, y] = toPx(pin);
  return [x + 14, y - 18];
}

function probeAt(x: number, y: number): Probe | undefined {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (c && Math.hypot(x - c[0], y - c[1]) < 9) return p;
  }
  return undefined;
}

const SCOPE_TITLE_PX = 18;
type ScopeZone =
  | { s: FloatScope; zone: 'title' | 'body' | 'close' | 'resize' }
  | { s: FloatScope; zone: 'chan'; pid: number }
  | { s: FloatScope; zone: 'ctrl'; id: ScopeControlId };

function scopeRectPx(s: FloatScope): [number, number, number, number] {
  return [cam.ox + s.x * cam.scale, cam.oy + s.y * cam.scale, s.w * cam.scale, s.h * cam.scale];
}

/** The trace area handed to renderScopeInto — one definition so the on-canvas
 * control row is hit-tested exactly where scope.ts drew it. */
function scopeBodyPx(s: FloatScope): [number, number, number, number] {
  const [X, Y, W, H] = scopeRectPx(s);
  return [X + 1, Y + SCOPE_TITLE_PX, W - 2, H - SCOPE_TITLE_PX - 1];
}

/** The control panel this scope belongs to (geometry only, re-derived per
 * frame): while owned, the instrument is drawn in that panel's window. */
const scopeOwnerOf = (s: FloatScope): Panel | null => scopeOwner(panels, s);

const SCOPE_BADGE_H = 22;
const SCOPE_BADGE_FONT = '11px ui-monospace, monospace';
/** ui-monospace advance at 11px — the badge is drawn and hit-tested from this
 * one width, exactly like the panel name tabs. */
const SCOPE_BADGE_CHAR_W = 6.7;
const scopeBadgeLabel = (s: FloatScope, owner: Panel) => `scope ${s.sid} → ${owner.name}`;

/** The placeholder a panel-owned scope leaves on the schematic, anchored at the
 * scope's top-left corner (screen-space size: it is chrome, not circuitry). */
function scopeBadgePx(s: FloatScope, owner: Panel): [number, number, number, number] {
  const [X, Y] = scopeRectPx(s);
  return [X, Y, 35 + scopeBadgeLabel(s, owner).length * SCOPE_BADGE_CHAR_W, SCOPE_BADGE_H];
}

function scopeZoneAt(x: number, y: number): ScopeZone | null {
  for (let k = floatScopes.length - 1; k >= 0; k--) {
    const s = floatScopes[k]!;
    const owner = scopeOwnerOf(s);
    if (owner) {
      // Panel-owned: the body (title bar, controls, resize corner) lives in the
      // panel window, so on canvas the badge is one drag zone and the rest of
      // the rect is click-through — canvas input never fights the widget.
      const [bx, by, bw, bh] = scopeBadgePx(s, owner);
      if (x >= bx && x <= bx + bw && y >= by && y <= by + bh) return { s, zone: 'title' };
      continue;
    }
    const [X, Y, W, H] = scopeRectPx(s);
    if (x < X || x > X + W || y < Y || y > Y + H) continue;
    if (y <= Y + SCOPE_TITLE_PX) {
      if (x >= X + W - 18) return { s, zone: 'close' };
      const dotStart = X + 64;
      const k2 = Math.floor((x - dotStart) / 16);
      if (x >= dotStart && k2 >= 0 && k2 < probes.length) {
        return { s, zone: 'chan', pid: probes[k2]!.pid };
      }
      return { s, zone: 'title' };
    }
    if (x >= X + W - 14 && y >= Y + H - 14) return { s, zone: 'resize' };
    const [bx, by, bw, bh] = scopeBodyPx(s);
    const id = scopeControlAt(bw, bh, x - bx, y - by, s.set, scopeProbes(s).length);
    if (id) return { s, zone: 'ctrl', id };
    return { s, zone: 'body' };
  }
  return null;
}

const scopeProbes = (s: FloatScope): Probe[] => scopeChannels(s, probes);

// ------------------------------------------------------------ context menu
// A cascading menu: #ctxmenu is a transparent full-viewport layer holding one
// `.ctxpanel` per open level, so click-away detection stays a single
// `ctxMenu.contains(target)` check no matter how deep the player has gone.
const ctxMenu = document.getElementById('ctxmenu') as HTMLDivElement;
type MenuItem =
  | { label: string; hint?: string; run: () => void }
  | { label: string; sub: () => MenuItem[] }
  | { sep: true }
  | { head: string };
const ctxIsOpen = () => ctxMenu.style.display === 'block';
/** Set when a click-away closed the menu, so the canvas ignores that click. */
let swallowPointer = false;
/** One entry per open level; index 0 is the root panel. */
let ctxPanels: HTMLDivElement[] = [];

function closeCtxMenu() {
  ctxMenu.style.display = 'none';
  ctxMenu.innerHTML = '';
  ctxPanels = [];
}

/** Drop every panel deeper than `depth` (a sibling row was entered). */
function closeCtxBelow(depth: number) {
  for (const p of ctxPanels.splice(depth + 1)) p.remove();
}

/** Build one panel of the cascade. `depth` 0 = root; anchor is where its
 * top-left should sit, flipping left/up when it would leave the viewport. */
function openCtxPanel(depth: number, items: MenuItem[], ax: number, ay: number, flipFrom?: number) {
  closeCtxBelow(depth - 1);
  const panel = document.createElement('div');
  panel.className = 'ctxpanel';
  for (const it of items) {
    const row = document.createElement('div');
    if ('sep' in it) {
      row.className = 'sep';
    } else if ('head' in it) {
      row.className = 'hd';
      row.textContent = it.head;
    } else if ('sub' in it) {
      row.className = 'mi sub';
      row.textContent = it.label;
      const openChild = () => {
        // A sibling (leaf or sub) may still carry the sticky highlight from
        // its own submenu: this row owns the cascade now, so clear them all
        // first or both rows stay lit.
        for (const el of panel.querySelectorAll('.open')) el.classList.remove('open');
        const r = row.getBoundingClientRect();
        openCtxPanel(depth + 1, it.sub(), r.right - 3, r.top - 4, r.left);
        row.classList.add('open');
      };
      row.onpointerenter = openChild;
      row.onclick = openChild;
    } else {
      row.className = 'mi';
      const text = document.createElement('span');
      text.textContent = it.label;
      row.appendChild(text);
      if (it.hint) {
        const k = document.createElement('span');
        k.className = 'kb';
        k.textContent = it.hint;
        row.appendChild(k);
      }
      // Entering a leaf closes any sibling's submenu.
      row.onpointerenter = () => {
        closeCtxBelow(depth);
        for (const el of panel.querySelectorAll('.open')) el.classList.remove('open');
      };
      row.onclick = () => {
        closeCtxMenu();
        it.run();
      };
    }
    panel.appendChild(row);
  }
  ctxMenu.appendChild(panel);
  ctxMenu.style.display = 'block';
  ctxPanels[depth] = panel;

  // Position after measuring; submenus flip to the parent's left edge.
  const r = panel.getBoundingClientRect();
  let x = ax;
  if (x + r.width > window.innerWidth - 6) {
    x = flipFrom !== undefined ? flipFrom - r.width + 3 : window.innerWidth - r.width - 6;
  }
  const y = Math.min(ay, window.innerHeight - r.height - 6);
  panel.style.left = `${Math.round(Math.max(4, x))}px`;
  panel.style.top = `${Math.round(Math.max(4, y))}px`;
}

function openCtxMenu(x: number, y: number, items: MenuItem[]) {
  closeCtxMenu();
  openCtxPanel(0, items, x, y);
}

/** Drop an in-place oscilloscope with its top-left at a grid point. It
 * appears when the room says it did — the sid is the server's to mint, the
 * same way a panel's plid is. */
function addFloatScope(at: Point) {
  scopeOp({
    t: 'add',
    x: at[0],
    y: at[1],
    w: 12,
    h: 6,
    set: wireScopeSet(defaultScopeSettings(5)),
    pids: null,
  });
}

/** "Add part" cascade: categories, then the parts in each. */
function partsMenu(): MenuItem[] {
  return CATEGORIES.map((cat) => ({
    label: cat,
    sub: () =>
      partsInCategory(cat).map((p) => ({
        label: p.name,
        hint: p.key,
        run: () => choosePart(p),
      })),
  }));
}

function partMenu(e: ElementSpec, x: number, y: number): MenuItem[] {
  const n = groupOf(e).length;
  const many = n > 1 ? ` (${n})` : '';
  const items: MenuItem[] = [
    { head: `${e.kind.t} #${e.id}` },
  ];
  if (isBroken(e.id)) {
    // Top of the menu on a dead part: it is the only thing you want here.
    items.push({ label: 'Repair this part', hint: 'K', run: () => repair(e.id) }, { sep: true });
  }
  items.push(
    { label: 'Edit…', run: () => openPropsDialog(e) },
    { label: `Rotate${many}`, hint: 'Q', run: () => rotateElements(groupOf(e)) },
    { label: `Delete${many}`, hint: 'Del', run: () => deleteElements(groupOf(e)) },
  );
  if (e.kind.t !== 'Ground') {
    items.push(
      { sep: true },
      { label: 'Probe voltage', hint: '1', run: () => toggleProbe(e.id, nearestPin(e, x, y), 'v') },
      { label: 'Probe current', hint: '2', run: () => toggleProbe(e.id, 0, 'i') },
      { label: 'Listen', hint: '3', run: () => toggleListen(e.id, nearestPin(e, x, y)) },
    );
  }
  if (e.kind.t === 'Speaker' && audio.speakerStreamed(e.id)) {
    // A speaker needs no probe to be heard, so its own mixer controls live
    // here rather than on a probe flag.
    // "Muted" covers being silenced by another speaker's solo, and unmuting
    // then ends that solo — which is what the player means by the click.
    const muted = audio.speakerMuted(e.id);
    items.push(
      { sep: true },
      {
        label: muted ? 'Unmute this speaker' : 'Mute this speaker',
        run: () => audio.muteSpeaker(e.id, !muted),
      },
      {
        label: audio.isSoloed(e.id) ? 'Unsolo' : 'Solo this speaker',
        run: () => audio.soloSpeaker(e.id),
      },
    );
  }
  items.push(
    { sep: true },
    { label: `Copy${many}`, hint: '⌘C', run: () => copyElements(groupOf(e)) },
    { label: 'Add part', sub: partsMenu },
  );
  return items;
}

/** Right-click on a probe flag. The flag floats above its pin, so this menu
 * takes priority over the part underneath it. */
function probeMenu(p: Probe): MenuItem[] {
  const items: MenuItem[] = [
    { head: `${p.kind === 'v' ? 'Voltage probe' : 'Current clamp'} ${p.pid}` },
    { label: 'Delete probe', hint: 'Del', run: () => deleteProbe(p) },
  ];
  if (p.kind === 'v') {
    const r = p.r;
    items.push(
      { sep: true },
      r
        ? // Re-sending the same point clears the reference (back to ground).
          { label: 'Clear reference', hint: '0', run: () => setProbeRef(p.pid, r[0], r[1]) }
        : {
            label: 'Set reference…',
            hint: '0',
            // '0' references whatever the cursor is over, for the selected probe.
            run: () => {
              selectedProbe = p.pid;
              selectedIds.clear();
              selectedMachine = false;
            },
          },
      {
        label: audio.pid === p.pid ? 'Stop listening' : 'Listen',
        hint: '3',
        run: () => toggleListen(p.elem, p.pin),
      },
    );
  }
  return items;
}

function canvasMenu(x: number, y: number): MenuItem[] {
  const items: MenuItem[] = [{ label: 'Add part', sub: partsMenu }];
  if (clipboard.length > 0) {
    const n = clipboard.length;
    items.push({ label: `Paste${n > 1 ? ` (${n})` : ''}`, run: () => pasteAt(snap(x, y)) });
  }
  items.push(
    { sep: true },
    { label: 'Oscilloscope here', hint: 'O', run: () => addFloatScope(snap(x, y)) },
    { label: 'Control panel here', hint: 'J', run: () => (panelTool = true) },
    { label: 'Label box here', hint: '⇧J', run: () => (labelBoxTool = true) },
    { label: 'Name this net', hint: '⇧W', run: () => (netLabelTool = true) },
    {
      label: 'Camera layer here',
      hint: '⇧Y',
      run: () => {
        layerTool = true;
        canvas.style.cursor = 'crosshair';
      },
    },
    { sep: true },
    { label: 'Select all', run: selectAll },
    { sep: true },
    { label: 'Rooms', sub: roomsMenu },
  );
  return items;
}

/** The room cascade. The chip in the corner is the always-visible answer to
 * "where am I"; this is the same three doors from where the player's hand
 * already is. */
function roomsMenu(): MenuItem[] {
  const here = roomsUI.current();
  return [
    { head: here ? `${here.name} · ${here.id}` : 'no room list on this server' },
    { label: 'Switch room…', hint: '⇧R', run: () => roomsUI.open('rooms') },
    { label: 'New room from a template…', run: () => roomsUI.open('new') },
    { sep: true },
    { label: 'Save this room as a template…', run: () => roomsUI.open('save') },
  ];
}

// ------------------------------------------------------------------ drags
let panDrag: { x: number; y: number; ox: number; oy: number } | null = null;
/** Dragging one pin of one part: `k` is the pin index being carried.
 *  `startPins` is the shape as the drag found it (after any straightening),
 *  which is what an interrupted gesture has to be put back to. */
let pinDrag: {
  id: number;
  k: number;
  moved: boolean;
  lastSent: number;
  startPins: Point[];
} | null = null;
let placeDrag: { a: Point; b: Point } | null = null;
/** How a click or sweep combines with what is already selected. Ctrl is taken
 *  by map panning, so SUBTRACT is Alt — the CAD convention (shift adds, alt
 *  removes) and the only free modifier. Shift is strictly ADDITIVE: a careful
 *  multi-select must never evaporate because one shift-click landed wrong. */
type SelectMode = 'replace' | 'add' | 'remove';
const selectModeOf = (ev: { shiftKey: boolean; altKey: boolean }): SelectMode =>
  ev.altKey ? 'remove' : ev.shiftKey ? 'add' : 'replace';
const modifiedSelect = (ev: { shiftKey: boolean; altKey: boolean }) =>
  ev.shiftKey || ev.altKey;

let marquee: { x0: number; y0: number; x1: number; y1: number; mode: SelectMode } | null = null;
let moveDrag: {
  items: { id: number; startPins: Point[] }[];
  start: Point;
  lastSent: number;
  moved: boolean;
  clickTarget: number;
} | null = null;
let scopeDrag: { s: FloatScope; dx: number; dy: number; lastSent: number } | null = null;
let scopeResize: { s: FloatScope; lastSent: number } | null = null;
/** Dragging the whole machine assembly. Its own gesture, not moveDrag: the
 * machine is not an element, and its children are locked against the document
 * Move op that moveDrag issues. */
interface MachineDrag {
  /** Grid point where the package was grabbed. */
  start: Point;
  /** Footprint and children as they were at that moment: the drag places from
   * this snapshot, so 300 pointer moves cannot drift from 1 move of 300. */
  rect0: MachineRect;
  items: { id: number; startPins: Point[] }[];
  dx: number;
  dy: number;
  lastSent: number;
  /** Delta already sent; ops carry increments, so this is the high-water mark. */
  sentX: number;
  sentY: number;
}
let machineDrag: MachineDrag | null = null;
/** Momentary pushbutton held down by the pointer (closed until release). */
let buttonHeld: ElementSpec | null = null;
/** J tool: dragging out a new control-panel region. */
let panelTool = false;
let panelDrag: { a: Point; b: Point } | null = null;

/** ⇧Y: drag out a camera layer. */
let layerTool = false;
let layerDrag: { a: Point; b: Point } | null = null;
/** Dragging an existing region by its name tab. */
let panelMove: {
  plid: number;
  dx: number;
  dy: number;
  w: number;
  h: number;
  lastSent: number;
} | null = null;
/** Dragging one of a region's eight resize grips. `base` is the rect as it
 * was when the grip was grabbed, so a flip past the opposite edge stays
 * anchored to the edge the player is not holding. */
let panelResize: {
  plid: number;
  handle: PanelHandle;
  base: PanelRect;
  lastSent: number;
} | null = null;

// ---- ANNOTATION GESTURES. Deliberately the same shape as the panel ones —
// drag out a rect, drag the title to move, drag a grip to resize, 60 ms
// throttle with a final absolute op on release — because a player who has
// drawn a control panel already knows how to draw a label box.
/** ⇧J: drag out a new label box. */
let labelBoxTool = false;
let labelBoxDrag: { a: Point; b: Point } | null = null;
let labelBoxMove: {
  blid: number;
  dx: number;
  dy: number;
  w: number;
  h: number;
  lastSent: number;
} | null = null;
let labelBoxResize: {
  blid: number;
  handle: PanelHandle;
  base: PanelRect;
  lastSent: number;
} | null = null;
/** ⇧W: the next click names the net at that grid point. */
let netLabelTool = false;
/** Dragging a net label onto a different point — which changes WHICH net it
 * names, because the anchor is the whole of its identity.
 *
 * `dx`/`dy` are the grab offset in grid units. Without them the label would
 * jump to the point under the cursor the instant it was touched, and the
 * plate floats a dozen pixels ABOVE its anchor — so merely picking one up
 * would silently re-anchor it a grid unit or two north, onto another net. */
let netLabelMove: {
  nlid: number;
  lastSent: number;
  x: number;
  y: number;
  dx: number;
  dy: number;
} | null = null;

let spaceHeld = false;
let lastCursorSent = 0;
/** The control-hint block. Collapsed by default: it is wide enough to sit in
 * front of world objects (the hoist, panels, cards). '?' or '/' toggles it,
 * and the choice is remembered. */
let hintsOpen = (() => {
  try {
    return localStorage.getItem('ee.hints') === '1';
  } catch {
    return false;
  }
})();

/** True while an arrow-key nudge gesture is open: every repeat while the key
 * is held joins ONE undo entry (history coalesces the Moves per id), and the
 * arrow keyup closes it. */
let nudging = false;

/** Move the whole selection by one grid step (arrow keys). Locked fixture
 * children stay put, exactly as in a drag. */
function nudgeSelection(dx: number, dy: number) {
  const ids = [...selectedIds].filter((id) => !isFixtureId(id));
  if (ids.length === 0) return;
  if (!nudging) {
    history.begin(undefined, ids.length > 1 ? `move ${ids.length} parts` : 'move part');
    nudging = true;
  }
  for (const id of ids) {
    const e = elemById(id);
    if (!e) continue;
    editDoc({ t: 'Move', id, pins: e.pins.map(([px, py]) => [px + dx, py + dy] as Point) });
  }
}

function endNudge() {
  if (nudging) {
    nudging = false;
    history.end();
  }
}

/** Grab the machine: select it and snapshot what has to travel together. */
function startMachineDrag(x: number, y: number) {
  const r = hoist.rect();
  if (!r) return;
  selectedIds.clear();
  selectedProbe = null;
  selectedMachine = true;
  hoist.setHot(true);
  canvas.style.cursor = 'grabbing';
  machineDrag = {
    start: snap(x, y),
    rect0: r,
    items: machineChildren().map((c) => ({
      id: c.id,
      startPins: c.pins.map((p) => [...p] as Point),
    })),
    dx: 0,
    dy: 0,
    lastSent: 0,
    sentX: 0,
    sentY: 0,
  };
}

/** Put the whole assembly at the drag's current delta, THIS frame: footprint
 * and all four children from the same snapshot, so the package and the terminals
 * can never rubber-band against each other. */
function placeMachineDrag(d: MachineDrag) {
  hoist.setLocalRect([
    d.rect0[0] + d.dx,
    d.rect0[1] + d.dy,
    d.rect0[2] + d.dx,
    d.rect0[3] + d.dy,
  ]);
  for (const it of d.items) {
    const c = elemById(it.id);
    if (!c) continue;
    c.pins = it.startPins.map(([x, y]) => [x + d.dx, y + d.dy] as Point);
    space.update(c); // the drag edits pins in place: re-bucket as it goes
  }
  // Deliberately NOT bumping docVersion: pins move, kinds do not, so the
  // speaker set cannot have changed — and a 60 Hz rescan of a 50k-part
  // document would be the most expensive thing in the drag (same reason the
  // part drag above leaves it alone).
}

/** Send what the server has not seen yet, as an increment. */
function flushMachineDrag(d: MachineDrag) {
  const ix = d.dx - d.sentX;
  const iy = d.dy - d.sentY;
  if (ix === 0 && iy === 0) return;
  d.sentX = d.dx;
  d.sentY = d.dy;
  if (online) net.sendMachineMove(ix, iy);
  else localSim.setElements(elements);
}

/** Release (or lose) the machine: the final op always goes out, and the whole
 * gesture becomes ONE undo entry — exactly like moving a group of parts. */
function endMachineDrag() {
  const d = machineDrag;
  if (!d) return;
  machineDrag = null;
  hoist.setHot(false);
  canvas.style.cursor = 'default';
  flushMachineDrag(d); // never let the 60 ms throttle eat the last move
  hoist.endLocalDrag(); // hold the placement until the server's answer lands
  if (d.dx === 0 && d.dy === 0) return; // a click, not a drag: just selected
  const [dx, dy] = [d.dx, d.dy];
  history.pushAction({
    label: 'move machine',
    undo: () => moveMachineBy(-dx, -dy),
    redo: () => moveMachineBy(dx, dy),
  });
}

/**
 * TEAR DOWN EVERY IN-FLIGHT CANVAS GESTURE.
 *
 * `pointerup` is not the only way a drag ends. A pointer can be taken away —
 * capture lost, the OS interrupting, a touch cancelled — and until now that
 * path cleaned up exactly three gestures (the held button, the machine drag,
 * the pin drag) and silently abandoned the rest: `moveDrag` left its
 * `history.begin()` open, so the NEXT edit joined an undo entry labelled "move
 * part", and `marquee`/`panDrag`/`placeDrag`/`scopeDrag`/the panel and
 * annotation drags were left dangling. That is a desktop bug — an interrupted
 * mouse drag leaks an unclosed undo transaction — it simply bites touch first,
 * because touch is where pointers get taken away.
 *
 * Two ways to end, because "the pointer vanished" and "the camera is taking
 * this gesture over" want opposite things:
 *
 *   'commit'   — a lost pointer. Whatever the drag had already done locally
 *                (and half-sent to the server) is stated outright so the two
 *                agree. This is what `pointercancel` asks for.
 *   'rollback' — a second finger landed. The document goes back exactly to
 *                where the first finger found it and nothing reaches the undo
 *                stack: a pinch that half-places a resistor is worse than no
 *                pinch at all.
 *
 * Gestures that only ever CREATE something on release (the rect drag-outs, a
 * part placement) are discarded in both modes — release is the only thing
 * allowed to add to the document.
 */
function endCanvasGestures(mode: 'commit' | 'rollback') {
  // A momentary pushbutton must never be left stuck closed in a shared room.
  if (buttonHeld) {
    interact(buttonHeld, { t: 'SetSwitch', closed: false });
    buttonHeld = null;
  }
  // Nothing here has touched the document yet, so both modes just drop it.
  panDrag = null;
  marquee = null;
  placeDrag = null;
  layerDrag = null;
  panelDrag = null;
  labelBoxDrag = null;
  // The live-rect gestures below have ALREADY edited their rect locally and
  // sent throttled increments, so both modes state the final rect: the
  // reconciling op is the only thing that stops client and server drifting.
  // (A scope or a label box a few pixels out is not the "half-placed
  // resistor" the rollback mode exists for.)
  if (scopeDrag || scopeResize) {
    const s = scopeDrag?.s ?? scopeResize!.s;
    scopeDrag = null;
    scopeResize = null;
    scopeOp(scopeRectOp(s));
  }
  if (panelMove || panelResize) {
    const plid = panelMove?.plid ?? panelResize!.plid;
    const p = panels.find((q) => q.plid === plid);
    if (p) panelOp({ t: 'rect', plid: p.plid, x0: p.x0, y0: p.y0, x1: p.x1, y1: p.y1 });
    panelMove = null;
    panelResize = null;
  }
  if (labelBoxMove || labelBoxResize) {
    const blid = labelBoxMove?.blid ?? labelBoxResize!.blid;
    const b = labelBoxes.find((q) => q.blid === blid);
    if (b) labelBoxOp({ t: 'rect', blid: b.blid, x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1 });
    labelBoxMove = null;
    labelBoxResize = null;
  }
  if (netLabelMove) {
    const l = netLabels.find((q) => q.nlid === netLabelMove!.nlid);
    if (l) netLabelOp({ t: 'move', nlid: l.nlid, x: l.x, y: l.y });
    netLabelMove = null;
  }
  if (machineDrag) {
    if (mode === 'rollback') {
      const d = machineDrag;
      d.dx = 0;
      d.dy = 0;
      placeMachineDrag(d); // package and children back to the snapshot
      flushMachineDrag(d); // and tell the server, as the inverse increment
      machineDrag = null;
      hoist.setHot(false);
      canvas.style.cursor = 'default';
      hoist.endLocalDrag();
    } else {
      // A lost pointer must not leave the machine half-moved and un-undoable:
      // commit where it actually got to, as one entry.
      endMachineDrag();
    }
  }
  if (pinDrag) {
    const e = elemById(pinDrag.id);
    if (mode === 'rollback') {
      if (pinDrag.moved && e) {
        e.pins = pinDrag.startPins.map((p) => [...p] as Point);
        space.update(e);
        editDoc({ t: 'Move', id: e.id, pins: e.pins }); // inside the open group
      }
      history.abort();
    } else {
      if (pinDrag.moved && e) editDoc({ t: 'Move', id: e.id, pins: e.pins });
      history.end();
    }
    pinDrag = null;
  }
  if (moveDrag) {
    if (mode === 'rollback') {
      if (moveDrag.moved) {
        for (const item of moveDrag.items) {
          const e = elemById(item.id);
          if (!e) continue;
          e.pins = item.startPins.map(([x, y]) => [x, y] as Point);
          space.update(e);
          editDoc({ t: 'Move', id: e.id, pins: e.pins });
        }
      }
      history.abort();
    } else {
      if (moveDrag.moved) {
        for (const item of moveDrag.items) {
          const e = elemById(item.id);
          if (e) editDoc({ t: 'Move', id: e.id, pins: e.pins });
        }
      }
      // A cancelled drag that never moved is not a click: it selects nothing
      // and flips no switch. Only `pointerup` may mean "click".
      history.end();
    }
    moveDrag = null;
  }
}

// ------------------------------------------------------------------ touch
// THE CAMERA IS ALWAYS TWO FINGERS. Everything below is reached only from an
// `ev.pointerType === 'touch'` branch at the top of the four canvas pointer
// handlers; a mouse or a pen never enters any of it.
//
// Pan and zoom are ONE transform, not two gestures: a pinch whose centroid
// drifts is simultaneously a pan, which is how every canvas app on glass
// behaves, and splitting them makes the world slide out from under the hand.
/** Every finger currently down on the canvas, in screen pixels. */
const touchPts = new Map<number, { x: number; y: number }>();
/** The two fingers currently driving the camera, plus the centroid and span
 *  they were last seen at. */
let touchNav: { a: number; b: number; cx: number; cy: number; d: number } | null = null;
/** True from the moment two fingers claim the camera until the LAST of them
 *  lifts. The finger left over when a pinch ends never got a `pointerdown` of
 *  its own, so it must not be handed to the one-finger dispatch mid-air. */
let touchNavTail = false;

/** One armed instrument, aimed at the part under the finger that just lifted.
 *  A miss leaves the tool armed: a mis-tap on empty canvas should cost one
 *  more tap, not a trip back to the palette. */
function fireTouchTool(x: number, y: number) {
  const t = touchTool;
  if (!t) return;
  const e = elementAt(x, y);
  if (!e || e.kind.t === 'Ground') {
    toast(t === 'listen' ? 'tap a part to listen to it' : 'tap a part to probe it');
    return;
  }
  if (t === 'v') toggleProbe(e.id, nearestPin(e, x, y), 'v');
  else if (t === 'i') toggleProbe(e.id, 0, 'i');
  else toggleListen(e.id, nearestPin(e, x, y));
  touchTool = null; // single shot, like the key it replaces
  canvas.style.cursor = 'default';
}

/** Centroid and span of two tracked fingers; null if either has gone. */
function touchSpan(a: number, b: number) {
  const p = touchPts.get(a);
  const q = touchPts.get(b);
  if (!p || !q) return null;
  return { a, b, cx: (p.x + q.x) / 2, cy: (p.y + q.y) / 2, d: Math.hypot(p.x - q.x, p.y - q.y) };
}

/** @returns true when touch has claimed this event and the canvas dispatch
 *  below must not see it. */
function touchDown(ev: PointerEvent): boolean {
  touchPts.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
  if (touchShot) touchShot.live = false; // a second finger is never a tap
  if (touchPts.size < 2 && !touchNavTail) {
    if (touchTool) {
      // The instrument owns this finger: no marquee, no part drag, no
      // selection change underneath it. It fires (or does not) on the lift.
      touchShot = { id: ev.pointerId, x: ev.clientX, y: ev.clientY, live: true };
      return true;
    }
    return false; // one finger: today's dispatch
  }
  try { canvas.setPointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  if (touchPts.size >= 2) {
    // THE SECOND FINGER CANCELS THE FIRST. Whatever one finger had begun —
    // carrying a pin, dragging a part, sweeping a marquee, stretching a new
    // resistor — is put back before the camera takes over.
    if (!touchNav) endCanvasGestures('rollback');
    const ids = [...touchPts.keys()];
    touchNav = touchSpan(ids[ids.length - 2]!, ids[ids.length - 1]!);
    touchNavTail = true;
  }
  return true;
}

function touchMove(ev: PointerEvent): boolean {
  const p = touchPts.get(ev.pointerId);
  if (p) {
    p.x = ev.clientX;
    p.y = ev.clientY;
  }
  if (touchShot && touchShot.id === ev.pointerId) {
    // A travelling finger is a pan attempt, not an aim. The instrument stays
    // armed for the next tap; this finger is spent either way, so the canvas
    // dispatch below must not get it and start a drag halfway through.
    if (Math.hypot(ev.clientX - touchShot.x, ev.clientY - touchShot.y) > TOUCH_TAP_SLOP) {
      touchShot.live = false;
    }
    return true;
  }
  if (!touchNav) return touchNavTail;
  if (ev.pointerId !== touchNav.a && ev.pointerId !== touchNav.b) return true; // a third finger
  const n = touchSpan(touchNav.a, touchNav.b);
  if (!n) return true;
  const k = touchNav.d > 0 && n.d > 0 ? n.d / touchNav.d : 1;
  const s2 = Math.min(MAX_SCALE, Math.max(MIN_SCALE, cam.scale * k));
  // Zoom about the centroid the fingers were on (the same anchor arithmetic
  // the wheel uses), then carry the camera by however far that centroid
  // travelled. Reading the ratio back off `s2` rather than off `k` is what
  // keeps a pinch against the MIN/MAX_SCALE clamp from also drifting.
  cam.ox = touchNav.cx - (touchNav.cx - cam.ox) * (s2 / cam.scale);
  cam.oy = touchNav.cy - (touchNav.cy - cam.oy) * (s2 / cam.scale);
  cam.scale = s2;
  cam.ox += n.cx - touchNav.cx;
  cam.oy += n.cy - touchNav.cy;
  touchNav = n;
  return true;
}

/** Shared by `pointerup` and `pointercancel`: a finger leaving is the same
 *  event to the camera either way. `tap` is false for a CANCELLED finger —
 *  the camera does not care, but an armed instrument must not fire on a
 *  gesture the browser tore up. */
function touchUp(ev: PointerEvent, tap = true): boolean {
  touchPts.delete(ev.pointerId);
  let spent = false;
  if (touchShot && touchShot.id === ev.pointerId) {
    if (tap && touchShot.live) fireTouchTool(touchShot.x, touchShot.y);
    touchShot = null;
    spent = true;
  }
  if (touchNav && (ev.pointerId === touchNav.a || ev.pointerId === touchNav.b)) {
    // Three fingers down and one lifts: the camera re-anchors on the two that
    // are left rather than jumping.
    const ids = [...touchPts.keys()];
    touchNav = ids.length >= 2 ? touchSpan(ids[0]!, ids[1]!) : null;
  }
  const owned = touchNavTail;
  if (touchPts.size === 0) touchNavTail = false;
  if (owned) {
    try { canvas.releasePointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  }
  return owned || spent;
}

// ------------------------------------------------------------- the palette
//
// THE HOTKEY IS THE PALETTE in this game — a part is armed by pressing its
// letter — so a phone, which has no letters, could look at the world and
// rearrange it but never add anything to it. This is that keyboard, drawn.
//
// It arms through the SAME calls the keys use (`choosePart`, `armRepair`, the
// net-label tool), so there is one placement path in the client and not two
// that drift. It is built from `CATALOG` and `CATEGORIES`, so a new part is a
// new button with no edit anywhere. And it does not exist at all until a real
// finger has touched the screen: see palette.ts.
const palette = createPalette(document.body, {
  categories: CATEGORIES,
  parts: CATALOG,
  choosePart,
  armTool: (t: ToolId) => {
    if (t === 'repair') {
      armRepair();
      return;
    }
    if (t === 'netname') {
      disarmTools();
      netLabelTool = true;
      placing = null;
      pasting = null;
      canvas.style.cursor = 'crosshair';
      toast('tap a point on the net to name it — a name joins nothing');
      return;
    }
    disarmTools(); // arming is exclusive, exactly as it is for the keys
    placing = null;
    pasting = null;
    closeCtxMenu();
    touchTool = t === 'probe-v' ? 'v' : t === 'probe-i' ? 'i' : 'listen';
    toast(
      t === 'listen' ? 'tap a part to listen to that node' : 'tap a part to put a probe on it',
    );
  },
  // The × on the chip. This is Esc's tool layer, and on glass it is the only
  // way out: there is no Escape key, and the browser's back-swipe leaves the
  // room instead of the tool.
  disarm: () => {
    placing = null;
    pasting = null;
    disarmTools();
    pendingBoxName = null;
    pendingNetName = null;
    canvas.style.cursor = 'default';
  },
  // Q and X/Y, for a hand that cannot reach them. Armed-only, like the keys:
  // with nothing being placed the chip is not on screen to be pressed.
  rotate: () => {
    if (placing) placeRot = (placeRot + 1) % 4;
  },
  flip: (axis: 'x' | 'y') => {
    if (!placing) return;
    if (axis === 'x') placeFlipX = !placeFlipX;
    else placeFlipY = !placeFlipY;
  },
});

/** What the armed chip should be showing this frame — the editor's own state,
 *  read fresh, so a tool dropped by Esc or armed by a hotkey shows up here
 *  too. Null means nothing is armed and the chip goes away. */
function armedForPalette(): Armed | null {
  const base = { orient: false, rot: placeRot, flipX: placeFlipX, flipY: placeFlipY };
  if (placing) return { ...base, label: placing.name, orient: true, part: placing.name };
  if (pasting) return { ...base, label: `paste ${pasting.length}` };
  if (touchTool === 'v') return { ...base, label: 'probe V', tool: 'probe-v' };
  if (touchTool === 'i') return { ...base, label: 'probe I', tool: 'probe-i' };
  if (touchTool === 'listen') return { ...base, label: 'listen', tool: 'listen' };
  if (repairing) return { ...base, label: 'repair', tool: 'repair' };
  if (netLabelTool) return { ...base, label: 'net name', tool: 'netname' };
  if (panelTool) return { ...base, label: 'panel region' };
  if (labelBoxTool) return { ...base, label: 'label box' };
  if (layerTool) return { ...base, label: 'camera layer' };
  return null;
}

canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  if (z) {
    const tb = z.s.set.timebase;
    z.s.set.timebase = Math.min(60, Math.max(0.001, tb * Math.exp(ev.deltaY * 0.001)));
    scopeRetuned(z.s);
    return;
  }
  const k = Math.exp(-ev.deltaY * 0.0015);
  const s2 = Math.min(MAX_SCALE, Math.max(MIN_SCALE, cam.scale * k));
  cam.ox = ev.clientX - (ev.clientX - cam.ox) * (s2 / cam.scale);
  cam.oy = ev.clientY - (ev.clientY - cam.oy) * (s2 / cam.scale);
  cam.scale = s2;
}, { passive: false });
// Right-click is ours: suppress the browser menu and raise a Falstad-style one.
canvas.addEventListener('contextmenu', (ev) => {
  ev.preventDefault();
  // macOS ctrl+click arrives as a left button *and* a contextmenu event — if it
  // already started a pan, do not also pop the menu.
  if (panDrag || placing || pasting) return;
  if (scopeZoneAt(ev.clientX, ev.clientY)) return;
  const pr = probeAt(ev.clientX, ev.clientY);
  const e = pr ? undefined : elementAt(ev.clientX, ev.clientY);
  openCtxMenu(
    ev.clientX,
    ev.clientY,
    pr
      ? probeMenu(pr)
      : e
        ? partMenu(e, ev.clientX, ev.clientY)
        : canvasMenu(ev.clientX, ev.clientY),
  );
});

// Click-away closes the menu (and that click does nothing else).
window.addEventListener(
  'pointerdown',
  (ev) => {
    if (!ctxIsOpen()) return;
    if (ev.target instanceof Node && ctxMenu.contains(ev.target)) return;
    closeCtxMenu();
    swallowPointer = ev.button === 0 && ev.target === canvas;
  },
  true,
);

canvas.addEventListener('pointerdown', (ev) => {
  if (ev.pointerType === 'touch' && touchDown(ev)) return; // the camera claimed it
  if (ev.button === 2) return; // right button only ever opens the menu
  if (swallowPointer) {
    swallowPointer = false;
    return;
  }
  try { canvas.setPointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  if (ev.button === 1 || spaceHeld || ev.ctrlKey) {
    panDrag = { x: ev.clientX, y: ev.clientY, ox: cam.ox, oy: cam.oy };
    return;
  }
  if (repairing) {
    // One click, one repair; the tool stays armed so a district full of dead
    // parts can be walked through without re-arming.
    const e = elementAt(ev.clientX, ev.clientY);
    if (e) repair(e.id);
    return;
  }
  if (layerTool) {
    const p = snap(ev.clientX, ev.clientY);
    layerDrag = { a: p, b: p };
    return;
  }
  {
    // The layer's name plate is the ONLY thing that opens a camera, and this
    // click is the user gesture the browser requires. Never on load, never on
    // join, never restored from storage.
    const l = layerPlateAt(cam, layers, ev.clientX, ev.clientY, ...plateView());
    if (l) {
      if (ev.shiftKey) layerOp({ t: 'remove', lid: l.lid });
      else void claimLayer(l);
      return;
    }
  }
  if (panelTool) {
    const p = snap(ev.clientX, ev.clientY);
    panelDrag = { a: p, b: p };
    return;
  }
  if (labelBoxTool) {
    const p = snap(ev.clientX, ev.clientY);
    labelBoxDrag = { a: p, b: p };
    return;
  }
  if (netLabelTool) {
    // ONE CLICK, ONE LABEL, on the grid point under the pointer. The point is
    // the anchor and the anchor is the whole of the label's identity — see
    // `NetLabel` in crates/server/src/main.rs for why it is a point and not a
    // wire or a pin, and for what happens to it when things get deleted.
    const [gx, gy] = snap(ev.clientX, ev.clientY);
    netLabelTool = false;
    canvas.style.cursor = 'default';
    if (netLabels.some((l) => l.x === gx && l.y === gy)) {
      toast('there is already a net label on that point');
      return;
    }
    netLabelOp({ t: 'add', x: gx, y: gy });
    // Name it straight away: a label called "NET 4" is not a label. The
    // rename box opens over where the plate will land, and the op it commits
    // names whichever label just appeared on that point — resolved at commit
    // time, because online the nlid is the server's to allocate.
    pendingNetName = { x: gx, y: gy, deadline: performance.now() + PENDING_NAME_MS };
    return;
  }
  if (pasting) {
    commitPaste(snap(ev.clientX, ev.clientY));
    return;
  }
  if (placing) {
    const p = snap(ev.clientX, ev.clientY);
    placeDrag = { a: p, b: p };
    return;
  }
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  if (z) {
    if (z.zone === 'close') {
      scopeOp({ t: 'remove', sid: z.s.sid });
    } else if (z.zone === 'chan') {
      // Applied locally FIRST (the dot has to light under the click) and then
      // sent; the broadcast confirms it, exactly as a panel drag does.
      const s = z.s;
      if (s.pids === null) s.pids = probes.map((p) => p.pid);
      s.pids = s.pids.includes(z.pid) ? s.pids.filter((x) => x !== z.pid) : [...s.pids, z.pid];
      scopeRetuned(s);
    } else if (z.zone === 'ctrl') {
      applyScopeControl(z.s.set, z.id, scopeProbes(z.s).length);
      scopeRetuned(z.s);
    } else if (z.zone === 'title') {
      const [gx, gy] = toGrid(ev.clientX, ev.clientY);
      scopeDrag = { s: z.s, dx: gx - z.s.x, dy: gy - z.s.y, lastSent: 0 };
    } else if (z.zone === 'resize') {
      scopeResize = { s: z.s, lastSent: 0 };
    }
    return;
  }
  // LABEL BOX titles: × deletes it, the title drags it, the eight grips on
  // its border resize it. Identical to the panel gesture below on purpose —
  // and the ONLY thing that happens is that a rectangle and a word move.
  const bz = labelBoxZoneAt(cam, labelBoxes, ev.clientX, ev.clientY);
  if (bz) {
    if (bz.zone === 'close') {
      labelBoxOp({ t: 'remove', blid: bz.box.blid });
    } else if (bz.zone === 'resize') {
      const { x0, y0, x1, y1 } = bz.box;
      labelBoxResize = {
        blid: bz.box.blid,
        handle: bz.handle,
        base: { x0, y0, x1, y1 },
        lastSent: 0,
      };
    } else {
      const [gx, gy] = toGrid(ev.clientX, ev.clientY);
      labelBoxMove = {
        blid: bz.box.blid,
        dx: gx - bz.box.x0,
        dy: gy - bz.box.y0,
        w: bz.box.x1 - bz.box.x0,
        h: bz.box.y1 - bz.box.y0,
        lastSent: 0,
      };
    }
    return;
  }
  // Panel name tabs: × deletes the region, the tab itself drags it; the
  // eight grips on its border resize it.
  const pz = panelZoneAt(cam, panels, ev.clientX, ev.clientY);
  if (pz) {
    if (pz.zone === 'close') {
      panelOp({ t: 'remove', plid: pz.panel.plid });
    } else if (pz.zone === 'resize') {
      const { x0, y0, x1, y1 } = pz.panel;
      panelResize = { plid: pz.panel.plid, handle: pz.handle, base: { x0, y0, x1, y1 }, lastSent: 0 };
    } else {
      const [gx, gy] = toGrid(ev.clientX, ev.clientY);
      panelMove = {
        plid: pz.panel.plid,
        dx: gx - pz.panel.x0,
        dy: gy - pz.panel.y0,
        w: pz.panel.x1 - pz.panel.x0,
        h: pz.panel.y1 - pz.panel.y0,
        lastSent: 0,
      };
    }
    return;
  }
  const pr = probeAt(ev.clientX, ev.clientY);
  if (pr) {
    selectedProbe = pr.pid;
    selectedIds.clear();
    selectedMachine = false;
    return;
  }
  // NET LABEL PLATES, AFTER the probe flags. A net label is drawn on the very
  // wire a probe is clipped to, so the two overlap — and an INSTRUMENT beats an
  // annotation for a click every time. Everything else the plate sits over is
  // a part body, which it does win against: the plate floats clear above the
  // conductor, so a pixel inside it was never a pixel of the schematic.
  //
  // Shift+click deletes; a plain drag re-anchors it to another point (which is
  // how you move it to a different net — the anchor IS which net it names).
  {
    const l = netLabelAt(cam, netLabels, ev.clientX, ev.clientY);
    if (l) {
      if (ev.shiftKey) {
        netLabelOp({ t: 'remove', nlid: l.nlid });
      } else {
        const [gx, gy] = toGrid(ev.clientX, ev.clientY);
        netLabelMove = {
          nlid: l.nlid,
          lastSent: 0,
          x: l.x,
          y: l.y,
          dx: gx - l.x,
          dy: gy - l.y,
        };
      }
      return;
    }
  }
  // Pins take priority: dragging a terminal reshapes ITS part — stretch,
  // shrink, reorient. Wires are placed with W (or the right-click menu),
  // never by pin-dragging.
  if (!modifiedSelect(ev)) {
    const own = pinOwnerAt(ev.clientX, ev.clientY);
    if (own && straightenBeforeDrag(own.e)) {
      history.begin([own.e], GESTURE_LABEL[pinGesture(own.e.kind, own.k)]); // pins mutate in place
      pinDrag = {
        id: own.e.id,
        k: own.k,
        moved: false,
        lastSent: 0,
        startPins: own.e.pins.map((p) => [...p] as Point),
      };
      selectedIds = new Set([own.e.id]);
      selectedProbe = null;
      selectedMachine = false;
      return;
    }
  }
  // The machine's ⓘ badge, after pins and before parts. Two rules meet here:
  // a terminal always wins over a glyph (wiring is the primary action), and
  // panel.ts's tab discipline — only ever hit-test a glyph at the zoom where
  // it is actually painted, or an invisible button eats clicks when zoomed
  // out. `zoneAt` reports 'info' under exactly that condition.
  if (!modifiedSelect(ev) && hoist.zoneAt(cam, ev.clientX, ev.clientY) === 'info') {
    hoist.openPinout();
    selectedMachine = true;
    selectedIds.clear();
    selectedProbe = null;
    return;
  }
  const e = elementAt(ev.clientX, ev.clientY);
  // The machine assembly comes LAST, after pins and after parts: a pointer on
  // a fixture terminal or child part still selects that part (fixture pins
  // are bolted down, so they never reshape-drag). The package's FACE picks up
  // the whole machine — its legs are outside the body box, so a terminal can
  // never be swallowed by the chip. Shift+drag stays a marquee, so a
  // selection can still be swept across the package.
  if (!e && !modifiedSelect(ev) && hoist.zoneAt(cam, ev.clientX, ev.clientY) === 'body') {
    startMachineDrag(ev.clientX, ev.clientY);
    return;
  }
  if (!e) {
    marquee = {
      x0: ev.clientX,
      y0: ev.clientY,
      x1: ev.clientX,
      y1: ev.clientY,
      mode: selectModeOf(ev),
    };
    return;
  }
  const mode = selectModeOf(ev);
  if (mode !== 'replace') {
    // Deliberately NOT a toggle: shift only ever adds, alt only ever removes.
    // A toggle means a mis-aimed shift-click silently drops the part you meant
    // to keep, which is the failure this system is built to avoid.
    if (mode === 'add') selectedIds.add(e.id);
    else selectedIds.delete(e.id);
    selectedProbe = null;
    return;
  }
  const startMove = (all: number[]) => {
    selectedMachine = false; // grabbing a part is not grabbing the machine
    // Machine fixtures are selectable and probe-able but bolted down: leaving
    // them out of the drag keeps a mixed selection dragging everything else.
    // (They move only with their whole assembly — see startMachineDrag.)
    const ids = all.filter((id) => !isFixtureId(id));
    // Seed the pre-drag specs: pins are mutated in place during the drag, so
    // by the time the final Move reaches editDoc the live "before" is stale.
    history.begin(ids.map((id) => elements.find((x) => x.id === id)));
    moveDrag = {
      items: ids
        .map((id) => elemById(id))
        .filter((x): x is ElementSpec => !!x)
        .map((x) => ({ id: x.id, startPins: x.pins.map((p) => [...p] as Point) })),
      start: snap(ev.clientX, ev.clientY),
      lastSent: 0,
      moved: false,
      clickTarget: e.id,
    };
  };
  if (e.kind.t === 'Button') {
    // Momentary: closed while held, released on pointerup. Pressing wins
    // over move-dragging on the body.
    interact(e, { t: 'SetSwitch', closed: true });
    buttonHeld = e;
    selectedIds = new Set([e.id]);
    selectedProbe = null;
    selectedMachine = false;
    return;
  }
  // Dragging any part body moves it; a member of a multi-selection drags the
  // whole group. Values are edited in the properties editor, never by dragging.
  if (selectedIds.has(e.id) && selectedIds.size > 1) startMove([...selectedIds]);
  else startMove([e.id]);
});

// Double-click a part: floating property editor, parked next to it.

// DOUBLE-CLICK RENAMES AN ANNOTATION. A panel renames through its window
// header; these two have no window, so the title itself is the control. The
// preceding pointerdowns will have opened (and closed) a move drag, which is
// harmless: a drag that ends where it started re-sends the rect it already
// had.
canvas.addEventListener('dblclick', (ev) => {
  const l = netLabelAt(cam, netLabels, ev.clientX, ev.clientY);
  if (l) {
    ev.preventDefault();
    renameNetLabel(l);
    return;
  }
  const bz = labelBoxZoneAt(cam, labelBoxes, ev.clientX, ev.clientY);
  if (bz && bz.zone === 'title') {
    ev.preventDefault();
    renameLabelBox(bz.box);
  }
});

canvas.addEventListener('pointermove', (ev) => {
  if (ev.pointerType === 'touch' && touchMove(ev)) return; // the camera owns it
  mouse = { x: ev.clientX, y: ev.clientY };
  const now = performance.now();
  if (online && now - lastCursorSent > 50) {
    lastCursorSent = now;
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    net.sendCursor(gx, gy);
  }
  if (panDrag) {
    cam.ox = panDrag.ox + (ev.clientX - panDrag.x);
    cam.oy = panDrag.oy + (ev.clientY - panDrag.y);
    return;
  }
  if (scopeDrag) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    scopeDrag.s.x = Math.round(gx - scopeDrag.dx);
    scopeDrag.s.y = Math.round(gy - scopeDrag.dy);
    // Optimistic; the broadcast confirms — and while this drag is live
    // `applyScopes` leaves the rect alone, so the confirmation cannot arrive
    // late and drag the instrument back out from under the pointer.
    if (now - scopeDrag.lastSent > 60) {
      scopeDrag.lastSent = now;
      scopeOp(scopeRectOp(scopeDrag.s));
    }
    return;
  }
  if (scopeResize) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    scopeResize.s.w = Math.max(6, Math.round(gx - scopeResize.s.x));
    scopeResize.s.h = Math.max(4, Math.round(gy - scopeResize.s.y));
    if (now - scopeResize.lastSent > 60) {
      scopeResize.lastSent = now;
      scopeOp(scopeRectOp(scopeResize.s));
    }
    return;
  }
  if (layerDrag) {
    layerDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (panelDrag) {
    panelDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (panelMove) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    const x0 = Math.round(gx - panelMove.dx);
    const y0 = Math.round(gy - panelMove.dy);
    const rect = { x0, y0, x1: x0 + panelMove.w, y1: y0 + panelMove.h };
    const p = panels.find((q) => q.plid === panelMove!.plid);
    if (p) Object.assign(p, rect); // optimistic; the broadcast confirms
    if (now - panelMove.lastSent > 60) {
      panelMove.lastSent = now;
      panelOp({ t: 'rect', plid: panelMove.plid, ...rect });
    }
    return;
  }
  if (panelResize) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    const rect = resizePanelRect(panelResize.base, panelResize.handle, gx, gy);
    const p = panels.find((q) => q.plid === panelResize!.plid);
    if (p) Object.assign(p, rect); // optimistic; the broadcast confirms
    if (now - panelResize.lastSent > 60) {
      panelResize.lastSent = now;
      panelOp({ t: 'rect', plid: panelResize.plid, ...rect });
    }
    return;
  }
  if (labelBoxDrag) {
    labelBoxDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (labelBoxMove) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    const x0 = Math.round(gx - labelBoxMove.dx);
    const y0 = Math.round(gy - labelBoxMove.dy);
    const rect = { x0, y0, x1: x0 + labelBoxMove.w, y1: y0 + labelBoxMove.h };
    const b = labelBoxes.find((q) => q.blid === labelBoxMove!.blid);
    if (b) Object.assign(b, rect); // optimistic; the broadcast confirms
    if (now - labelBoxMove.lastSent > 60) {
      labelBoxMove.lastSent = now;
      labelBoxOp({ t: 'rect', blid: labelBoxMove.blid, ...rect });
    }
    return;
  }
  if (labelBoxResize) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    // The panel's resize maths, unchanged: a rectangle is a rectangle, and
    // both have the same one-grid-unit minimum span.
    const rect = resizePanelRect(labelBoxResize.base, labelBoxResize.handle, gx, gy);
    const b = labelBoxes.find((q) => q.blid === labelBoxResize!.blid);
    if (b) Object.assign(b, rect);
    if (now - labelBoxResize.lastSent > 60) {
      labelBoxResize.lastSent = now;
      labelBoxOp({ t: 'rect', blid: labelBoxResize.blid, ...rect });
    }
    return;
  }
  if (netLabelMove) {
    const [px, py] = toGrid(ev.clientX, ev.clientY);
    const gx = Math.round(px - netLabelMove.dx);
    const gy = Math.round(py - netLabelMove.dy);
    if (gx !== netLabelMove.x || gy !== netLabelMove.y) {
      netLabelMove.x = gx;
      netLabelMove.y = gy;
      const l = netLabels.find((q) => q.nlid === netLabelMove!.nlid);
      if (l) {
        l.x = gx;
        l.y = gy;
      }
      if (now - netLabelMove.lastSent > 60) {
        netLabelMove.lastSent = now;
        netLabelOp({ t: 'move', nlid: netLabelMove.nlid, x: gx, y: gy });
      }
    }
    return;
  }
  if (marquee) {
    marquee.x1 = ev.clientX;
    marquee.y1 = ev.clientY;
    return;
  }
  if (placeDrag) {
    placeDrag.b = snap(ev.clientX, ev.clientY);
    return;
  }
  if (pinDrag) {
    const here = snap(ev.clientX, ev.clientY);
    const e = elemById(pinDrag.id);
    if (e) {
      // TWO TERMINALS ARE A PART; THREE ARE A SYMBOL.
      //
      // A resistor IS its two endpoints, so dragging one is how you draw it
      // and `here` goes straight in. Anything the catalogue draws as an
      // object — an op-amp triangle, a DIP, a transistor — has one canonical
      // layout, and moving ONE of its terminals is not a smaller version of
      // reshaping it, it is a different and meaningless thing. So the drag
      // asks sim-core what the WHOLE part looks like now: swing and stretch
      // it about its far end, or carry it by a leg that has no say in the
      // axis. Neither branch can produce a shape the gate would refuse,
      // which is why the throttled preview below can go out unchecked.
      const rigid = pinCount(e.kind) > 2;
      const next = rigid
        ? reshapePins(e.kind, e.pins, pinDrag.k, here)
        : e.pins.some((p, i) => i !== pinDrag!.k && p[0] === here[0] && p[1] === here[1])
          ? // A two-pin part must not collapse onto itself: both terminals
            // on one grid point would merge its nodes.
            null
          : e.pins.map((p, i) => (i === pinDrag!.k ? here : p));
      if (next && next.some((p, i) => p[0] !== e.pins[i]![0] || p[1] !== e.pins[i]![1])) {
        e.pins = next;
        space.update(e);
        pinDrag.moved = true;
      }
      if (pinDrag.moved && online && now - pinDrag.lastSent > 60) {
        pinDrag.lastSent = now;
        net.sendEdit({ t: 'Move', id: e.id, pins: e.pins });
      }
    }
    return;
  }
  if (machineDrag) {
    const here = snap(ev.clientX, ev.clientY);
    const [dx, dy] = clampMachineDelta(
      machineDrag.rect0,
      here[0] - machineDrag.start[0],
      here[1] - machineDrag.start[1],
    );
    if (dx !== machineDrag.dx || dy !== machineDrag.dy) {
      machineDrag.dx = dx;
      machineDrag.dy = dy;
      placeMachineDrag(machineDrag); // live, snapped, package + children as one
    }
    // Same ~60 ms cadence as a part drag: the pointer never waits on the wire.
    if (now - machineDrag.lastSent > 60) {
      machineDrag.lastSent = now;
      flushMachineDrag(machineDrag);
    }
    return;
  }
  if (moveDrag) {
    const here = snap(ev.clientX, ev.clientY);
    const dx = here[0] - moveDrag.start[0];
    const dy = here[1] - moveDrag.start[1];
    if (dx !== 0 || dy !== 0) moveDrag.moved = true;
    if (!moveDrag.moved) return;
    for (const item of moveDrag.items) {
      const e = elemById(item.id);
      if (e) {
        e.pins = item.startPins.map(([x, y]) => [x + dx, y + dy] as Point);
        space.update(e); // the drag edits pins in place: re-bucket as it goes
      }
    }
    if (now - moveDrag.lastSent > 60) {
      moveDrag.lastSent = now;
      for (const item of moveDrag.items) {
        const e = elemById(item.id);
        if (e && online) net.sendEdit({ t: 'Move', id: e.id, pins: e.pins });
      }
      if (!online) localSim.setElements(elements);
    }
    return;
  }
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  // Panel tabs and grips are hit-tested before pins/parts on pointerdown, so
  // the cursor has to agree with that order.
  const pz = z ? null : panelZoneAt(cam, panels, ev.clientX, ev.clientY);
  const over = z || pz ? undefined : elementAt(ev.clientX, ev.clientY);
  const onPin = !z && !pz && !!pinAt(ev.clientX, ev.clientY);
  // What grabbing that terminal would DO. Three different gestures share one
  // hit test, so the cursor has to distinguish them or a rigid part reads as
  // "the drag stopped working": 'grab' for the terminals that swing the whole
  // part round, 'move' for the legs that carry it and for the free endpoints
  // of a two-pin part.
  const pinOwner = onPin ? pinOwnerAt(ev.clientX, ev.clientY) : null;
  const pinCur =
    pinOwner && pinGesture(pinOwner.e.kind, pinOwner.k) === 'swing' ? 'grab' : 'move';
  // The machine is hit-tested last here too, so the cursor promises exactly
  // what pointerdown will do.
  const mz =
    z || pz || over || onPin ? null : hoist.zoneAt(cam, ev.clientX, ev.clientY);
  hoist.setHot(mz === 'body' || !!machineDrag);
  // The annotation chrome, in the order pointerdown hit-tests it: the label
  // box's title/grips sit with the panel tabs, and the net-label plate comes
  // after them. The cursor has to agree with that order or it promises
  // something the click will not do.
  const bz2 = z || pz ? null : labelBoxZoneAt(cam, labelBoxes, ev.clientX, ev.clientY);
  const nl2 = z || pz || bz2 ? null : netLabelAt(cam, netLabels, ev.clientX, ev.clientY);
  canvas.style.cursor = repairing
    ? REPAIR_CURSOR
    : placing || pasting || panelTool || labelBoxTool || netLabelTool || layerTool
    ? 'crosshair'
    : spaceHeld
      ? 'grab'
      : z
        ? z.zone === 'title'
          ? 'move'
          : z.zone === 'resize'
            ? 'nwse-resize'
            : z.zone === 'ctrl' || z.zone === 'chan' || z.zone === 'close'
              ? 'pointer'
              : 'default'
        : bz2
          ? bz2.zone === 'resize'
            ? PANEL_HANDLE_CURSOR[bz2.handle]
            : bz2.zone === 'close'
              ? 'pointer'
              : 'move'
          : nl2
            ? 'move'
          : pz
          ? pz.zone === 'resize'
            ? PANEL_HANDLE_CURSOR[pz.handle]
            : pz.zone === 'close'
              ? 'pointer'
              : 'move'
          : onPin
            ? pinCur // reshape a free part, or swing/carry a rigid one
            : over?.kind.t === 'Switch' || over?.kind.t === 'Button'
              ? 'pointer'
              : over
                ? 'move' // plain drag moves any part
                : mz === 'info'
                  ? 'pointer' // the package's ⓘ badge: open the datasheet
                  : mz
                    ? 'move' // the package's face drags the whole assembly
                    : 'default';
});

canvas.addEventListener('pointerup', (ev) => {
  if (ev.pointerType === 'touch' && touchUp(ev)) return; // a camera finger lifting
  try { canvas.releasePointerCapture(ev.pointerId); } catch { /* synthetic pointers */ }
  if (panDrag) {
    panDrag = null;
    return;
  }
  if (buttonHeld) {
    interact(buttonHeld, { t: 'SetSwitch', closed: false });
    buttonHeld = null;
    return;
  }
  if (scopeDrag || scopeResize) {
    // The op that reconciles: the throttle can have swallowed the last
    // pointer sample, and this client has been ignoring the echo for the
    // whole drag, so the final rect has to be stated outright.
    const s = scopeDrag?.s ?? scopeResize!.s;
    scopeDrag = null;
    scopeResize = null;
    scopeOp(scopeRectOp(s));
    return;
  }
  if (layerDrag) {
    const r = normLayerRect(layerDrag.a, layerDrag.b);
    layerDrag = null;
    if (r) {
      layerTool = false;
      canvas.style.cursor = 'default';
      layerOp({ t: 'add', x0: r[0], y0: r[1], x1: r[2], y1: r[3] });
      toast('camera layer placed — click its plate to point a camera at it');
    }
    return;
  }
  if (panelDrag) {
    const r = normPanelRect(panelDrag.a, panelDrag.b);
    panelDrag = null;
    if (r) {
      // One region per arming; a stray click leaves the tool armed.
      panelTool = false;
      canvas.style.cursor = 'default';
      panelOp({ t: 'add', x0: r[0], y0: r[1], x1: r[2], y1: r[3] });
    }
    return;
  }
  if (panelMove || panelResize) {
    const plid = panelMove?.plid ?? panelResize!.plid;
    const p = panels.find((q) => q.plid === plid);
    if (p) panelOp({ t: 'rect', plid: p.plid, x0: p.x0, y0: p.y0, x1: p.x1, y1: p.y1 });
    panelMove = null;
    panelResize = null;
    return;
  }
  if (labelBoxDrag) {
    const r = normLabelBoxRect(labelBoxDrag.a, labelBoxDrag.b);
    labelBoxDrag = null;
    if (r) {
      // One box per arming; a stray click leaves the tool armed.
      labelBoxTool = false;
      canvas.style.cursor = 'default';
      labelBoxOp({ t: 'add', x0: r[0], y0: r[1], x1: r[2], y1: r[3] });
      // Name it immediately: a box called "LABEL 3" is not a label. Resolved
      // at commit time by rect, because online the blid is the server's.
      pendingBoxName = { x0: r[0], y0: r[1], deadline: performance.now() + PENDING_NAME_MS };
    }
    return;
  }
  // The final op is ABSOLUTE, so it says the same thing as all the throttled
  // increments put together — nothing can be left half-applied.
  if (labelBoxMove || labelBoxResize) {
    const blid = labelBoxMove?.blid ?? labelBoxResize!.blid;
    const b = labelBoxes.find((q) => q.blid === blid);
    if (b) labelBoxOp({ t: 'rect', blid: b.blid, x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1 });
    labelBoxMove = null;
    labelBoxResize = null;
    return;
  }
  if (netLabelMove) {
    const l = netLabels.find((q) => q.nlid === netLabelMove!.nlid);
    if (l) netLabelOp({ t: 'move', nlid: l.nlid, x: l.x, y: l.y });
    netLabelMove = null;
    return;
  }
  if (machineDrag) {
    endMachineDrag();
    return;
  }
  if (marquee) {
    const [gx0, gy0] = toGrid(Math.min(marquee.x0, marquee.x1), Math.min(marquee.y0, marquee.y1));
    const [gx1, gy1] = toGrid(Math.max(marquee.x0, marquee.x1), Math.max(marquee.y0, marquee.y1));
    const dragged = Math.abs(marquee.x1 - marquee.x0) + Math.abs(marquee.y1 - marquee.y0) > 6;
    // A CLICK THAT HIT NOTHING, INSIDE A CAMERA LAYER. This is the click the
    // player means by "I clicked the camera layer and nothing happened": the
    // plate is a 20-pixel strip and the rectangle is the size of a district.
    // It stays a click on empty canvas — opening a camera is a decision, and
    // the labelled plate is where that decision is made — but it says so,
    // because silence was the whole defect.
    if (!dragged) {
      const l = layerAt(cam, layers, marquee.x1, marquee.y1);
      if (l) {
        const holder = claims.get(l.lid);
        toast(
          holder !== undefined && holder !== myId
            ? `${l.name} is driven by player ${holder}`
            : layerPlateRect(cam, l, ...plateView())
              ? holder === myId && camera.isLive()
                ? `${l.name} is live — click its plate (top-left) to stop`
                : `${l.name} — click its plate (top-left) to point a camera at it`
              : `${l.name} — zoom in until its plate is on screen, then click that`,
        );
      }
    }
    // Only an UNMODIFIED click clears. A stray shift- or alt-click on empty
    // space leaves a hard-won selection exactly as it was.
    if (marquee.mode === 'replace') {
      selectedIds.clear();
      selectedProbe = null;
      selectedMachine = false;
    }
    if (dragged) {
      for (const e of space.query(gx0, gy0, gx1, gy1)) {
        if (e.pins.some(([x, y]) => x >= gx0 && x <= gx1 && y >= gy0 && y <= gy1)) {
          if (marquee.mode === 'remove') selectedIds.delete(e.id);
          else selectedIds.add(e.id);
        }
      }
    }
    marquee = null;
    return;
  }
  if (placeDrag && placing) {
    const kind = placing.make();
    const a = placeDrag.a;
    const clicked = placeDrag.b[0] === a[0] && placeDrag.b[1] === a[1];
    const b = clicked ? placeEnd(a) : placeDrag.b;
    const id = newId();
    // `placeRot` is the armed quarter-turn. For a two- or three-pin part it
    // has already done its work through `placeEnd`, which points the drag;
    // for a one-pin part there is no drag to point, so it rides the spec.
    editDoc({
      t: 'Add',
      spec: {
        id,
        kind,
        pins: placePins(kind, a, b),
        tier: placing.tier ?? 0,
        rot: placeRot,
      },
    });
    selectedIds = new Set([id]);
    selectedProbe = null;
    selectedMachine = false;
    placeDrag = null;
    return; // tool stays armed (Falstad-style); Esc exits
  }
  if (pinDrag) {
    const e = elemById(pinDrag.id);
    // The final pin position rides one Move; the drag's throttled sends were
    // best-effort previews. No motion = the click just selected the part.
    if (pinDrag.moved && e) editDoc({ t: 'Move', id: e.id, pins: e.pins });
    history.end(); // one undo entry per reshape gesture
    pinDrag = null;
    return;
  }
  if (moveDrag) {
    if (moveDrag.moved) {
      for (const item of moveDrag.items) {
        const e = elemById(item.id);
        if (e) editDoc({ t: 'Move', id: e.id, pins: e.pins });
      }
    } else {
      // A click that never moved: select, and flip a switch if that is what it is.
      const t = elemById(moveDrag.clickTarget);
      if (t && t.kind.t === 'Switch') interact(t, { t: 'SetSwitch', closed: !t.kind.closed });
      selectedIds = new Set([moveDrag.clickTarget]);
      selectedProbe = null;
      selectedMachine = false;
    }
    history.end(); // one undo entry per drag gesture
    moveDrag = null;
  }
});
canvas.addEventListener('pointerleave', () => (mouse = null));
// A cancelled pointer (touch interrupted, capture lost) must not leave a
// momentary button stuck closed in a shared room — nor, and this is what it
// used to do, an open `history.begin()` for the next edit to fall into. Every
// gesture ends here, through the same teardown `pointerup` reconciles with.
canvas.addEventListener('pointercancel', (ev) => {
  if (ev.pointerType === 'touch' && touchUp(ev, false)) return;
  endCanvasGestures('commit');
});

window.addEventListener('keydown', (ev) => {
  const inEditor =
    ev.target instanceof Node && (propsDiv.contains(ev.target) || propsDlg.contains(ev.target));
  if (inEditor) {
    if (ev.key === 'Escape' && dlgFor !== null) closePropsDialog();
    return;
  }
  if (panelHost.owns(ev.target)) return; // typing in a panel window
  if (roomsUI.owns(ev.target)) return; // typing in the room browser
  if (chatOwns(ev.target)) return; // typing a chat line
  if (lessonUI.owns(ev.target)) return; // a focused lesson-card button
  if (gfxUI.owns(ev.target)) return; // a slider in the graphics dialog

  // Clipboard first: ⌘/Ctrl+C copies, ⌘/Ctrl+V arms pasting at the cursor.
  if (ev.metaKey || ev.ctrlKey) {
    if (ev.key === 'c') {
      copySelection();
      ev.preventDefault();
    } else if (ev.key === 'v') {
      armPaste();
      ev.preventDefault();
    } else if (!isTypingTarget(ev) && (ev.key === 'z' || ev.key === 'Z' || ev.key === 'y')) {
      // ⌘/Ctrl+Z undo, ⌘/Ctrl+Shift+Z or Ctrl+Y redo — MY edits only.
      if (ev.key === 'y' || ev.shiftKey) history.redo(elements);
      else history.undo(elements);
      ev.preventDefault();
    }
    return;
  }
  if (ev.altKey) return;

  if (ev.key === ' ') {
    spaceHeld = true;
    ev.preventDefault();
    return;
  }
  if (ev.key.startsWith('Arrow') && selectedIds.size > 0 && !isTypingTarget(ev)) {
    const [dx, dy] =
      ev.key === 'ArrowLeft'
        ? [-1, 0]
        : ev.key === 'ArrowRight'
          ? [1, 0]
          : ev.key === 'ArrowUp'
            ? [0, -1]
            : [0, 1];
    nudgeSelection(dx, dy);
    ev.preventDefault();
    return;
  }
  if (ev.key === '?' || ev.key === '/') {
    // '/' used to open the parts cascade; the right-click menu is the one
    // route to it now, and both keys toggle the help instead.
    hintsOpen = !hintsOpen;
    try {
      localStorage.setItem('ee.hints', hintsOpen ? '1' : '0');
    } catch {
      /* private mode: the toggle still works for this session */
    }
    ev.preventDefault();
    return;
  }
  if (ev.key === 'Escape') {
    // Peel one layer at a time: browser, then menu, then editor, then
    // tools/selection. The browser is a modal, so it comes off first.
    if (roomsUI.isOpen()) {
      roomsUI.close();
      return;
    }
    if (ctxIsOpen()) {
      closeCtxMenu();
      return;
    }
    if (dlgFor !== null) {
      closePropsDialog();
      return;
    }
    placing = null;
    pasting = null;
    disarmTools();
    pendingBoxName = null;
    pendingNetName = null;
    selectedIds.clear();
    selectedProbe = null;
    selectedMachine = false;
    canvas.style.cursor = 'default';
    return;
  }
  if (ev.key === 'Enter' && !isTypingTarget(ev)) {
    // Enter opens the chat line. Enter, because every bare letter in this
    // app is a part ('t' is the potentiometer, 'c' the capacitor) and Enter
    // is the one key a player already expects to start a message with — it
    // is also the key that will send it.
    chatOpen();
    ev.preventDefault();
    return;
  }
  if (ev.key === 'R') {
    // ⇧R: the room browser. Shift, because every bare letter in this app is
    // a part — 'r' is the resistor and must stay the resistor.
    if (roomsUI.isOpen()) roomsUI.close();
    else roomsUI.open('rooms');
    ev.preventDefault();
    return;
  }
  // Graphics preferences. Shift+G, because plain g is Ground and every
  // unshifted letter is already a part.
  if (ev.key === 'G') {
    gfxUI.toggle();
    return;
  }
  if (ev.key === 'h' || ev.key === 'H') {
    // The world is far bigger than one screen: 'H' comes home to the
    // starter district, shift+H frames everything that exists.
    if (ev.shiftKey) fitAll();
    else fitHome();
    return;
  }
  if (ev.key === 'k' || ev.key === 'K') {
    // The repair tool. Broken parts are found by eye (they are charred, and
    // they keep a marker when zoomed out) and fixed by clicking them.
    armRepair();
    return;
  }
  if (ev.key === 'Y') {
    // ⇧Y: the camera layer, next to Y (the photocell) because they are two
    // halves of one gesture — draw the hole in the world, then put a part
    // over it.
    disarmTools();
    layerTool = true;
    placing = null;
    pasting = null;
    canvas.style.cursor = 'crosshair';
    toast('drag out a camera layer, then click its plate to point a camera at it');
    return;
  }
  if (ev.key === 'j') {
    // Arm the panel tool: drag a region around the parts you want on a
    // control panel. Its window appears as soon as the region exists.
    disarmTools();
    panelTool = true;
    placing = null;
    pasting = null;
    canvas.style.cursor = 'crosshair';
    return;
  }
  if (ev.key === 'J') {
    // ⇧J: a LABEL BOX. Next to J because it is the same gesture — drag out a
    // rectangle round some parts — and shifted because the difference is
    // exactly that this one is only words: no window, no membership, nothing
    // about the parts inside it changes.
    disarmTools();
    labelBoxTool = true;
    placing = null;
    pasting = null;
    canvas.style.cursor = 'crosshair';
    toast('drag out a label box, then type what it is');
    return;
  }
  if (ev.key === 'W') {
    // ⇧W: NAME A NET. Next to W (the wire) because a net is what wires make.
    // The click lands the name on a grid POINT — see `NetLabel` in
    // crates/server/src/main.rs for why the anchor is a point, what happens
    // when the thing under it is deleted, and why two nets may share a name
    // without being joined by it.
    disarmTools();
    netLabelTool = true;
    placing = null;
    pasting = null;
    canvas.style.cursor = 'crosshair';
    toast('click a point on the net to name it — a name joins nothing');
    return;
  }
  if (ev.key === 'Delete' || ev.key === 'Backspace') {
    // Probes win over the part selection: pointing at a flag and pressing
    // Delete must never remove the parts you have selected elsewhere.
    const pr =
      (mouse ? probeAt(mouse.x, mouse.y) : undefined) ??
      (selectedProbe !== null ? probes.find((p) => p.pid === selectedProbe) : undefined);
    if (pr) {
      deleteProbe(pr);
      return;
    }
    // ANNOTATION UNDER THE POINTER, before the selection, for the same reason
    // probes come first: pointing at a thing and pressing Delete must delete
    // that thing, not the parts you selected somewhere else.
    //
    // It is also the guaranteed way out. Both primitives are deleted by their
    // own chrome (the box's ×, shift+click on a net plate), but a box small
    // enough — or a zoom low enough — that its title plate is not drawn has no
    // chrome to click, and a shape a player can make and cannot unmake is a
    // trap. `netLabelAt` and `labelBoxHotAt` are both edge-or-plate hits, so
    // neither can shadow a part inside the box.
    if (mouse) {
      const nl = netLabelAt(cam, netLabels, mouse.x, mouse.y);
      if (nl) {
        netLabelOp({ t: 'remove', nlid: nl.nlid });
        return;
      }
      const lb = labelBoxHotAt(cam, labelBoxes, mouse.x, mouse.y);
      if (lb) {
        labelBoxOp({ t: 'remove', blid: lb.blid });
        return;
      }
      // A CAMERA LAYER, last of the annotations and for the same reason the
      // comment above gives: a shape a player can make and cannot unmake is a
      // trap. The wire has always carried LayerOp Remove, but no gesture
      // reached it except shift+click on the name plate — undocumented, and a
      // mis-aim of the add-to-selection gesture. Delete over the layer works
      // now, like every other thing on the sheet.
      //
      // LAST because `layerAt` is a BODY hit, not an edge-or-plate hit like
      // the two above: a camera layer is a district-sized rectangle, and
      // testing it earlier would shadow every part standing inside it.
      const ly = layerAt(cam, layers, mouse.x, mouse.y);
      if (ly && !elementAt(mouse.x, mouse.y)) {
        layerOp({ t: 'remove', lid: ly.lid });
        return;
      }
    }
    const e = mouse ? elementAt(mouse.x, mouse.y) : undefined;
    if (selectedIds.size > 0) deleteIds([...selectedIds]);
    else if (e) deleteIds([e.id]);
    return;
  }
  if (ev.key === 'q' || ev.key === 'Q') {
    if (placing) {
      placeRot = (placeRot + 1) % 4;
    } else if (pasting) {
      // Rotate the paste ghost 90° clockwise about its centroid (origin).
      pasting = pasting.map((c) => ({
        ...c,
        pins: c.pins.map(([x, y]) => [-y, x] as Point),
        rot: ((c.rot ?? 0) + 1) & 3,
      }));
    } else {
      rotateSelection();
    }
    return;
  }
  // MIRROR — but only when there is something to mirror.
  //
  // `y` is also the Photocell's hotkey (⇧Y draws the camera layer it reads,
  // and the two are meant to be one gesture). The mirror keys landed first,
  // on a different line, so git merged both cleanly and `y` silently stopped
  // placing the part: the flip arm ran `flipSelection('y')` on an empty
  // selection — a no-op — and returned before the part table was consulted.
  // The camera feature's step 3 has been dead ever since.
  //
  // A flip with no target was always a no-op, so falling through when there
  // is no target cannot change what a flip does. It only fills in the case
  // where the key did nothing at all.
  const flipKey = ev.key === 'x' || ev.key === 'X' || ev.key === 'y' || ev.key === 'Y';
  if (flipKey && (pasting || placing || selectedIds.size > 0)) {
    const axis: 'x' | 'y' = ev.key === 'x' || ev.key === 'X' ? 'x' : 'y';
    if (pasting) {
      // Ghost pins are relative to the cursor, so the centroid is the origin.
      pasting = pasting.map((c) => ({
        ...c,
        pins: c.pins.map(([x, y]) => (axis === 'x' ? [-x, y] : [x, -y]) as Point),
      }));
    } else if (placing) {
      // Arm the mirror so the orientation is chosen BEFORE the part lands.
      if (axis === 'x') placeFlipX = !placeFlipX;
      else placeFlipY = !placeFlipY;
    } else {
      flipSelection(axis);
    }
    return;
  }
  if (ev.key === '`' || ev.key === '~') {
    dock.toggle();
    ev.preventDefault();
    return;
  }
  if (ev.key === 'o' && mouse) {
    addFloatScope(snap(mouse.x, mouse.y));
    return;
  }
  if (ev.key === '1' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, nearestPin(e, mouse.x, mouse.y), 'v');
    return;
  }
  if (ev.key === '2' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleProbe(e.id, 0, 'i');
    return;
  }
  if (ev.key === '3' && mouse) {
    const e = elementAt(mouse.x, mouse.y);
    if (e && e.kind.t !== 'Ground') toggleListen(e.id, nearestPin(e, mouse.x, mouse.y));
    return;
  }
  if (ev.key === '0' && mouse) {
    const target =
      selectedProbe !== null
        ? probes.find((p) => p.pid === selectedProbe)
        : [...probes].reverse().find((p) => p.kind === 'v');
    const e = elementAt(mouse.x, mouse.y);
    if (target && target.kind === 'v' && e && e.kind.t !== 'Ground') {
      setProbeRef(target.pid, e.id, nearestPin(e, mouse.x, mouse.y));
    }
    return;
  }
  // Part hotkeys (Falstad-style). 'M' (shift+m) = PMOS.
  const partName = PART_HOTKEYS[ev.key];
  if (partName) {
    const part = CATALOG.find((p) => p.name === partName);
    if (part) choosePart(part);
  }
});
window.addEventListener('keyup', (ev) => {
  if (ev.key === ' ') spaceHeld = false;
  // The nudge gesture ends when the arrow lifts: held-key repeats coalesce
  // into one undo entry, separate presses become separate entries.
  if (ev.key.startsWith('Arrow')) endNudge();
});
// A pointer gesture or a lost window closes any open nudge group — its keyup
// may never arrive, and a stale group would swallow unrelated edits.
window.addEventListener('pointerdown', endNudge, true);
window.addEventListener('blur', endNudge);

// ---------------------------------------------------------------- render
// (A fifth copy of the SI formatter lived here, with no call sites at all —
// deleted rather than ported. hoist.ts's copy even carried a doc comment
// pointing at it as the canonical one.)

const scopeDiv = document.getElementById('scope') as HTMLDivElement;
const scopeCv = document.getElementById('scopecv') as HTMLCanvasElement;
scopeCv.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  ev.stopPropagation();
  const tb = dockScope.timebase;
  dockScope.timebase = Math.min(60, Math.max(0.001, tb * Math.exp(ev.deltaY * 0.001)));
}, { passive: false });
// The docked panel's controls are the same on-canvas row scope.ts draws for
// floating scopes, so the click routing is the same hit-test call.
const dockCtrlAt = (ev: PointerEvent) => {
  const r = scopeCv.getBoundingClientRect();
  return scopeControlAt(
    scopeCv.clientWidth,
    scopeCv.clientHeight,
    ev.clientX - r.left,
    ev.clientY - r.top,
    dockScope,
    probes.length,
  );
};
scopeCv.addEventListener('pointerdown', (ev) => {
  const id = dockCtrlAt(ev);
  if (id) {
    ev.preventDefault();
    applyScopeControl(dockScope, id, probes.length);
  }
});
scopeCv.addEventListener('pointermove', (ev) => {
  scopeCv.style.cursor = dockCtrlAt(ev) ? 'pointer' : 'default';
});
const dock = createDock(scopeDiv, scopeCv, audio);

/** Blue highlight over an element: its pin-chain plus dots on every pin
 * (pins must overlap exactly to connect — make them visible). */
/** A canvas context whose every stroke and fill comes out one colour.
 *
 *  The symbol draw is a 900-line switch that picks its own colours from
 *  voltage, current, damage and audio, and re-implementing any of that to
 *  produce a blue copy would be a second renderer to keep in step with the
 *  first. Forcing the colour at the context instead means the highlight is
 *  drawn by exactly the code that draws the part, so a symbol that changes
 *  shape tomorrow highlights correctly with no further work. */
const NO_FILL = new Set(['fill', 'fillRect', 'fillText']);

function tinted(base: CanvasRenderingContext2D, color: string): CanvasRenderingContext2D {
  return new Proxy(base, {
    get(t, k) {
      // FILLS ARE DROPPED, not recoloured. Forcing fillStyle too painted a
      // chip's body as one solid blue slab, and a Shift Register under the
      // cursor simply vanished -- you could not see its interior, its pin
      // labels, or where its edges were. The part is already drawn underneath
      // this pass; the highlight only has to re-stroke its OUTLINE, so every
      // fill here is a no-op and the symbol shows through.
      if (NO_FILL.has(k as string)) return () => {};
      const v = Reflect.get(t, k) as unknown;
      return typeof v === 'function' ? (v as (...a: unknown[]) => unknown).bind(t) : v;
    },
    set(t, k, v) {
      // Only strokes take the tint. `fillStyle` is still allowed through so
      // the underlying context is left in a sane state for the next caller.
      Reflect.set(t, k, k === 'strokeStyle' ? color : v);
      return true;
    },
  }) as CanvasRenderingContext2D;
}

/** Hover / selection highlight: THE PART ITSELF GOES BLUE.
 *
 *  It used to be a rounded box drawn round the whole part, on the reasoning
 *  that the symbol already draws its own geometry so the highlight only had
 *  to say "this one". In practice a big translucent rectangle over a dense
 *  schematic hides the thing it is pointing at, and on a wide part (a chip,
 *  a stretched wire) it covers everything the part crosses as well.
 *
 *  So the part is simply drawn again, in blue, over itself: same geometry,
 *  same line widths, one colour. `live`, `dmg`, `sound` and `time` are all
 *  deliberately omitted so the second pass draws the SYMBOL and not the
 *  current dots, the heat glow, the smoke or the speaker halo — those are
 *  state the player is reading, and a highlight must not double them up. */
function drawHighlight(e: ElementSpec, strong: boolean) {
  const t = tinted(ctx, strong ? '#7db1ff' : '#5a8cff');
  ctx.save();
  ctx.globalAlpha = strong ? 1 : 0.75;
  // A touch wider than the symbol underneath, so the blue reads as the part
  // rather than as a slightly-off redraw of it.
  const grow = Math.max(1.5, cam.scale * 0.05);
  ctx.lineWidth = Math.max(2, cam.scale * 0.07) + grow;
  drawElement({ ctx: t, cam, dots, dtSec: 0 }, e);
  ctx.restore();
  // Pin dots stay: they are the wire targets, and they are what a player is
  // aiming at when they hover a part at all.
  ctx.save();
  ctx.fillStyle = '#7db1ff';
  ctx.globalAlpha = 0.9;
  for (const [x, y] of e.pins.map(toPx)) {
    ctx.beginPath();
    ctx.arc(x, y, Math.max(3, cam.scale * 0.11), 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
}

/** A control panel is pointing at something: light it up in exactly the
 * language the canvas already uses for "this one" — a docked panel is just
 * another way of pointing at a part. A row gets the strong single-part box,
 * a whole window the weak "these" box over every member. */
function drawPanelHoverHighlight(h: PanelHover, view: ViewRect) {
  const strong = h.kind === 'row';
  for (const id of h.ids) {
    const e = elemById(id);
    if (!e) continue; // the part went away mid-hover: draw nothing
    if (inView(view, id)) drawHighlight(e, strong);
    else if (strong) drawOffscreenPointer(e);
  }
}

/** The part is off-screen — routine once the panel lives in a sidebar, and a
 * highlight nobody can see is a failed feature. Put a chevron on the
 * viewport edge, on the ray to the part, inset past the rails so it is not
 * hidden under the very panel being used. */
function drawOffscreenPointer(e: ElementSpec) {
  const ins = panelHost.railInsets();
  const m = 26;
  // The HUD block owns the top-left corner; keep the chevron out from under
  // it, and off the scope dock at the bottom.
  const top = 62;
  let cx = 0;
  let cy = 0;
  for (const p of e.pins) {
    cx += p[0];
    cy += p[1];
  }
  const n = e.pins.length || 1;
  const sx = cam.ox + (cx / n) * cam.scale;
  const sy = cam.oy + (cy / n) * cam.scale;
  const px = Math.max(ins.left + m, Math.min(window.innerWidth - ins.right - m, sx));
  const py = Math.max(top, Math.min(window.innerHeight - 30 - m, sy));
  const a = Math.atan2(sy - py, sx - px);
  ctx.save();
  ctx.globalAlpha = 0.92;
  ctx.translate(px, py);
  ctx.rotate(a);
  ctx.fillStyle = '#5a8cff';
  ctx.beginPath();
  ctx.moveTo(12, 0);
  ctx.lineTo(-6, 7.5);
  ctx.lineTo(-6, -7.5);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
  // The distance is grid units off the camera, not an invented number.
  ctx.save();
  ctx.globalAlpha = 0.95;
  ctx.fillStyle = '#9dbcff';
  ctx.font = '11px ui-monospace, monospace';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(
    `#${e.id} ${(Math.hypot(sx - px, sy - py) / cam.scale).toFixed(0)}u`,
    px - Math.cos(a) * 22,
    py - Math.sin(a) * 22,
  );
  ctx.restore();
}

/** Little speaker next to the flag of the probe we are listening to; its
 * arcs ride the stream's own amplitude. */
function drawListenGlyph(x: number, y: number, color: string) {
  const lvl = audio.level;
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(x, y - 2);
  ctx.lineTo(x + 3, y - 2);
  ctx.lineTo(x + 6, y - 5);
  ctx.lineTo(x + 6, y + 5);
  ctx.lineTo(x + 3, y + 2);
  ctx.lineTo(x, y + 2);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.2;
  for (let k = 0; k < 2; k++) {
    ctx.globalAlpha = Math.min(1, 0.25 + lvl * 2.5 - k * 0.35);
    ctx.beginPath();
    ctx.arc(x + 6, y, 4 + k * 3.5, -0.9, 0.9);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
}

function drawProbeMarkers() {
  for (const p of probes) {
    const c = probeFlagPx(p);
    if (!c) continue;
    const e = elemById(p.elem)!;
    const pin = e.pins[Math.min(p.pin, e.pins.length - 1)]!;
    const [x, y] = toPx(pin);
    const color = probeColor(p.pid);
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + 10, y - 14);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(c[0], c[1], 6, 0, Math.PI * 2);
    ctx.fill();
    if (selectedProbe === p.pid) {
      ctx.beginPath();
      ctx.arc(c[0], c[1], 9, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.fillStyle = '#101014';
    ctx.font = 'bold 9px ui-monospace';
    ctx.fillText(p.kind === 'v' ? 'V' : 'I', c[0] - 3, c[1] + 3);
    if (audio.pid === p.pid) drawListenGlyph(c[0] + 9, c[1], color);

    if (p.r) {
      const re = elemById(p.r[0]);
      if (re) {
        const rp = re.pins[Math.min(p.r[1], re.pins.length - 1)]!;
        const [rx, ry] = toPx(rp);
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(rx - 6, ry + 8);
        ctx.lineTo(rx + 6, ry + 8);
        ctx.moveTo(rx - 4, ry + 11);
        ctx.lineTo(rx + 4, ry + 11);
        ctx.moveTo(rx - 2, ry + 14);
        ctx.lineTo(rx + 2, ry + 14);
        ctx.stroke();
      }
    }
  }
}

function drawSelectionBoxes(view: ViewRect) {
  if (selectedIds.size === 0) return;
  ctx.strokeStyle = '#5a8cff';
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 4]);
  for (const id of selectedIds) {
    // Select-all on a big world can hold tens of thousands of ids: only the
    // ones on screen cost anything.
    if (!inView(view, id)) continue;
    const e = elemById(id);
    if (!e) continue;
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const p of e.pins) {
      x0 = Math.min(x0, p[0]);
      y0 = Math.min(y0, p[1]);
      x1 = Math.max(x1, p[0]);
      y1 = Math.max(y1, p[1]);
    }
    const pad = 0.55 * cam.scale;
    ctx.strokeRect(
      cam.ox + x0 * cam.scale - pad,
      cam.oy + y0 * cam.scale - pad,
      (x1 - x0) * cam.scale + pad * 2,
      (y1 - y0) * cam.scale + pad * 2,
    );
  }
  ctx.setLineDash([]);
}

/** The machine assembly drawn as a selected object, in exactly the language
 * the schematic uses for a selected part (drawSelectionBoxes' dashed box) —
 * sized to the footprint the server broadcast, so it tracks a drag frame by
 * frame and follows another player's move too. */
function drawMachineSelection() {
  if (!selectedMachine && !machineDrag) return;
  const r = hoist.rect();
  if (!r) return;
  const pad = 0.35 * cam.scale;
  ctx.strokeStyle = '#5a8cff';
  ctx.lineWidth = 1.5;
  ctx.setLineDash([5, 4]);
  ctx.strokeRect(
    cam.ox + r[0] * cam.scale - pad,
    cam.oy + r[1] * cam.scale - pad,
    (r[2] - r[0]) * cam.scale + pad * 2,
    (r[3] - r[1]) * cam.scale + pad * 2,
  );
  ctx.setLineDash([]);
}

/** Little sine-in-a-screen glyph for the placeholder badge. */
function drawScopeGlyph(x: number, y: number, w: number, h: number) {
  ctx.lineWidth = 1;
  ctx.strokeRect(Math.round(x) + 0.5, Math.round(y) + 0.5, w, h);
  ctx.beginPath();
  for (let k = 0; k <= w - 2; k++) {
    const yy = y + h / 2 - Math.sin((k / (w - 2)) * Math.PI * 2) * (h / 2 - 1.5);
    if (k === 0) ctx.moveTo(x + 1 + k, yy);
    else ctx.lineTo(x + 1 + k, yy);
  }
  ctx.stroke();
}

/** A panel-owned scope is displayed in that panel's window, so all the
 * schematic keeps is this badge at the scope's top-left: it says where the
 * instrument went and is its drag handle. Deliberately small — the rest of the
 * scope's rect stays click-through, unlike the body it replaces. */
function drawScopePlaceholder(s: FloatScope, owner: Panel) {
  const [X, Y, W, H] = scopeBadgePx(s, owner);
  ctx.save();
  roundRectPath(ctx, X, Y, W, H, 6);
  ctx.fillStyle = '#12171de6';
  ctx.fill();
  ctx.setLineDash([4, 4]);
  ctx.lineWidth = 1.2;
  ctx.strokeStyle = '#57808f';
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.strokeStyle = '#8ee7ff';
  ctx.fillStyle = '#8ee7ff';
  drawScopeGlyph(X + 7, Y + 6, 14, 10);
  ctx.font = SCOPE_BADGE_FONT;
  ctx.fillText(scopeBadgeLabel(s, owner), X + 27, Y + 15);
  ctx.restore();
}

function drawFloatScopes() {
  for (const s of floatScopes) {
    const owner = scopeOwnerOf(s);
    if (owner) {
      // The placeholder is a handle, not content: it belongs with the other
      // schematic furniture that stands down for the calm zoomed-out view.
      if (cam.scale >= LOD_FULL) drawScopePlaceholder(s, owner);
      continue;
    }
    const [X, Y, W, H] = scopeRectPx(s);
    ctx.fillStyle = '#101016f0';
    ctx.fillRect(X, Y, W, H);
    ctx.strokeStyle = '#3a3a48';
    ctx.lineWidth = 1;
    ctx.strokeRect(X, Y, W, H);
    ctx.fillStyle = '#191922';
    ctx.fillRect(X, Y, W, SCOPE_TITLE_PX);
    ctx.fillStyle = '#8a8a98';
    ctx.font = '11px ui-monospace, monospace';
    ctx.fillText(`scope ${s.sid}`, X + 8, Y + 13);
    const active = scopeProbes(s);
    probes.forEach((p, k) => {
      const cx = X + 64 + k * 16 + 5;
      const cy = Y + SCOPE_TITLE_PX / 2;
      ctx.beginPath();
      ctx.arc(cx, cy, 5, 0, Math.PI * 2);
      if (active.some((a) => a.pid === p.pid)) {
        ctx.fillStyle = probeColor(p.pid);
        ctx.fill();
      } else {
        ctx.strokeStyle = probeColor(p.pid);
        ctx.stroke();
      }
    });
    ctx.fillStyle = '#8a8a98';
    ctx.fillText('×', X + W - 13, Y + 13);
    ctx.strokeStyle = '#3a3a48';
    ctx.beginPath();
    ctx.moveTo(X + W - 12, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 12);
    ctx.moveTo(X + W - 7, Y + H - 3);
    ctx.lineTo(X + W - 3, Y + H - 7);
    ctx.stroke();
    if (H - SCOPE_TITLE_PX > 20) {
      const [bx, by, bw, bh] = scopeBodyPx(s);
      renderScopeInto(ctx, bx, by, bw, bh, traces, active, s.set.timebase, s.set, netNames());
    }
  }
}

function drawCursors(now: number) {
  for (const [who, c] of cursors) {
    if (now - c.seen > 4000) {
      cursors.delete(who);
      continue;
    }
    const x = cam.ox + c.x * cam.scale;
    const y = cam.oy + c.y * cam.scale;
    const hue = (who * 137.5) % 360;
    ctx.fillStyle = `hsl(${hue} 80% 60%)`;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + 12, y + 5);
    ctx.lineTo(x + 5, y + 12);
    ctx.closePath();
    ctx.fill();
    ctx.font = '11px ui-monospace';
    ctx.fillText(`P${who}`, x + 10, y + 22);
  }
}

// ------------------------------------------------------------ viewport cull
/** Padded viewport in grid units: [x0, y0, x1, y1]. */
type ViewRect = [number, number, number, number];

/** The cull window. Padding covers highlight strokes and glow halos that
 * spill past an element's own bbox. */
function viewRect(pad = 2): ViewRect {
  const [x0, y0] = toGrid(0, 0);
  const [x1, y1] = toGrid(window.innerWidth, window.innerHeight);
  return [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
}

function inView(v: ViewRect, id: number): boolean {
  const b = space.bboxOf(id);
  return !!b && b.x1 >= v[0] && b.x0 <= v[2] && b.y1 >= v[1] && b.y0 <= v[3];
}

/** Level of detail, px per grid unit:
 *   >= LOD_FULL  full symbols (dots, glow, text, probe flags, panel tabs)
 *   2..LOD_FULL  conductor chains only, still solver-colored
 *   < 2          one segment per element
 * LOD_FULL lives in render.ts so every switch tied to it happens at once. */
const LOD_CHAIN = 2;

/** Reused draw lists so a steady frame allocates nothing. `visible` is
 * everything on screen; `schematic` is that minus the machine fixtures, whose
 * glyphs belong to their package. */
const visible: ElementSpec[] = [];
const schematic: ElementSpec[] = [];
/** Above this many on-screen elements, skip the document-order sort: at that
 * density the z-order of overlapping symbols is not visible anyway. */
const SORT_LIMIT = 3000;
/** Last frame's cull cost + counts, for the perf line in the HUD. */
const perf = { cull: 0, drawn: 0, total: 0 };

let simDebt = 0;
let lastT = performance.now();

function frame(now: number) {
  // Clamp below at 0: on a cold load Chrome can deliver a first rAF
  // timestamp *earlier* than the module-eval-time lastT. A negative delta
  // would drive `want` negative, which reinterprets as ~2^32 u32 steps at
  // the wasm ABI (Sim.advance(max_steps: u32)) — a multi-minute spin.
  const wallDt = Math.min(0.1, Math.max(0, (now - lastT) / 1000));
  lastT = now;


  // Speakers are sources of sound the moment they exist in the document.
  // No-op unless the document actually changed since the last frame.
  syncSpeakers();

  if (!online) {
    simDebt += wallDt / DT;
    const want = Math.floor(simDebt);
    localSim.advance(Math.min(want, MAX_STEPS_PER_FRAME));
    simDebt -= want;
    live = unpackFrame(localSim.frame());
    simTime = localSim.time();
    for (const p of probes) {
      const l = live.get(p.elem);
      if (!l) continue;
      let v = p.kind === 'v' ? l.v[p.pin] ?? 0 : l.i[p.pin] ?? 0;
      if (p.kind === 'v' && p.r) {
        const rl = live.get(p.r[0]);
        v -= rl?.v[p.r[1]] ?? 0;
      }
      traces.appendPoint(p.pid, simTime, v);
      audio.pushPoint(p.pid, simTime, v);
    }
  }

  ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
  drawGrid(ctx, cam, window.innerWidth, window.innerHeight);

  // Sensor layers sit UNDER everything: they are a hole in the world that
  // the schematic is drawn on top of. The video only ever renders here, in
  // the rectangle the player placed, so there is no configuration in which
  // capture is running and the player cannot see what it sees.
  drawSensorLayers(
    ctx,
    cam,
    layers,
    claims,
    myId,
    camera.previewEl(),
    mouse ? (layerPlateAt(cam, layers, mouse.x, mouse.y, ...plateView())?.lid ?? null) : null,
    ...plateView(),
    camera.isLive() && camera.silentMs() > CAMERA_SILENT_MS,
  );
  if (layerDrag) {
    drawLayerGhost(ctx, cam, layerDrag.a, layerDrag.b, normLayerRect(layerDrag.a, layerDrag.b) !== null);
  }

  // Panel regions sit under the schematic: they frame parts, never hide them.
  // The one under the pointer (or being dragged) shows its resize grips.
  const hotPanel = mouse ? panelHotAt(cam, panels, mouse.x, mouse.y) : null;
  drawPanelRegions(
    ctx,
    cam,
    panels,
    panelResize?.plid ?? panelMove?.plid ?? hotPanel?.plid ?? panelHover?.plid ?? null,
  );
  if (panelDrag) drawPanelGhost(ctx, cam, panelDrag.a, panelDrag.b);

  // Label boxes sit beside the panel regions and under the schematic, for the
  // same reason: they frame parts, they never hide them.
  const hotBox = mouse ? labelBoxHotAt(cam, labelBoxes, mouse.x, mouse.y) : null;
  drawLabelBoxes(
    ctx,
    cam,
    labelBoxes,
    labelBoxResize?.blid ?? labelBoxMove?.blid ?? hotBox?.blid ?? null,
  );
  if (labelBoxDrag) drawLabelBoxGhost(ctx, cam, labelBoxDrag.a, labelBoxDrag.b);

  // Cull to the viewport through the spatial index: a 20k-element world
  // costs what is on screen, not what exists.
  const view = viewRect();
  const cull0 = performance.now();
  space.query(view[0], view[1], view[2], view[3], visible);
  if (visible.length <= SORT_LIMIT) space.sortByDoc(visible);
  perf.cull = performance.now() - cull0;
  perf.drawn = visible.length;
  perf.total = space.count;

  // Machine fixtures are drawn by their package, not by the schematic pass:
  // inside a chip, a device symbol is internal schematic. They stay in
  // `visible` (and in the spatial index, and in `elementAt`) — only their
  // glyph belongs to the machine.
  schematic.length = 0;
  for (const e of visible) if (!isFixtureId(e.id)) schematic.push(e);

  if (cam.scale >= LOD_FULL) {
    for (const e of schematic) {
      // Speakers get their audio state alongside the solver frame: a
      // sounding one glows, a muted or unstreamed one looks plainly idle.
      const sound =
        e.kind.t === 'Speaker' && audio.speakerStreamed(e.id)
          ? { level: audio.speakerLevel(e.id), muted: audio.speakerMuted(e.id) }
          : undefined;
      drawElement(
        { ctx, cam, live: live.get(e.id), dots, dtSec: wallDt, sound, dmg: damage.get(e.id), time: now },
        e,
      );
    }
  } else {
    // Too small for symbols: conductors only, colored by the solver frame —
    // plus heat, a blast ping and a marker on every dead part, because
    // finding them IS the repair. `schematic` excludes the machine's own
    // fixtures — the chip draws those itself, inside its package.
    drawElementsLod(ctx, cam, schematic, live, cam.scale < LOD_CHAIN, damage, now);
  }

  // The machine's package, on top of the wires (a player's wire routed across
  // a chip passes BEHIND its body, which is what a package does) and under
  // the selection halos and probe flags. It draws its own LOD form, so this
  // is one call on both sides of LOD_FULL. The same call refreshes the goal
  // card overlay.
  hoist.draw(ctx, cam, now, wallDt, {
    children: machineChildren(),
    live,
    damage,
    dots,
  });

  // The lesson card's step checks, against this frame's live map (throttled
  // inside; a room that is not a lesson costs one boolean here).
  lessonUI.tick(now);

  // Hover highlight (blue element + pin dots), Falstad-style.
  const zHover = mouse ? scopeZoneAt(mouse.x, mouse.y) : null;
  const md = moveDrag;
  const hover = md
    ? elemById(md.clickTarget)
    : mouse && !placing && !pasting && !panelTool && !labelBoxTool && !netLabelTool && !layerTool && !zHover
      ? elementAt(mouse.x, mouse.y)
      : undefined;
  if (hover) drawHighlight(hover, true);
  for (const id of selectedIds) {
    if (!inView(view, id)) continue;
    const e = elemById(id);
    if (e && e !== hover) drawHighlight(e, false);
  }
  // ...and whatever a control panel is pointing at (row hover / keyboard).
  if (panelHover) drawPanelHoverHighlight(panelHover, view);

  // NET LABELS, on top of the wires they name — a net name belongs ON the
  // conductor, the way it is drawn on paper. Detached ones (the anchor has
  // nothing connected to it) draw dimmed and dashed rather than vanishing:
  // the player wrote that word deliberately, and the label is still exactly
  // where they put it.
  drawNetLabels(
    ctx,
    cam,
    netLabels,
    netMap,
    netLabelMove?.nlid ?? (mouse ? (netLabelAt(cam, netLabels, mouse.x, mouse.y)?.nlid ?? null) : null),
  );

  // Ghost previews for in-progress edits.
  ctx.globalAlpha = 0.45;
  // While you are DRAWING it, you always see it — that is the whole point of
  // dragging a part out. It is only the ghost that idles under the cursor
  // before the drag that is suppressed for few-pinned parts, since those are
  // drawn rather than stamped down as an object.
  if (placeDrag && placing) {
    const kind = placing.make();
    const clicked = placeDrag.b[0] === placeDrag.a[0] && placeDrag.b[1] === placeDrag.a[1];
    const b = clicked ? placeEnd(placeDrag.a) : placeDrag.b;
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: placePins(kind, placeDrag.a, b) });
  } else if (placing && mouse) {
    const kind = placing.make();
    if (pinCount(kind) > 3) {
      const a = snap(mouse.x, mouse.y);
      drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: placePins(kind, a, placeEnd(a)) });
    }
  }
  if (pasting && mouse) {
    const at = snap(mouse.x, mouse.y);
    for (const item of pasting) {
      drawElement(
        { ctx, cam, dots, dtSec: 0 },
        { id: 0, kind: item.kind, pins: item.pins.map(([x, y]) => [x + at[0], y + at[1]] as Point) },
      );
    }
  }
  ctx.globalAlpha = 1;

  // Pin connect indicator on a reshape drag: green when the carried pin
  // lands on another part's pin (overlapping pins = connected), gray when
  // it floats free.
  if (pinDrag && pinDrag.moved) {
    const e = elemById(pinDrag.id);
    const p = e?.pins[pinDrag.k];
    if (e && p) {
      // The pivot, for a part that swings as one body: the far end, which
      // the gesture promises to hold still. Drawn with a spoke to the
      // carried terminal, so what is happening to the WHOLE part is visible
      // while it happens — without it a rigid swing reads as the drag
      // refusing to follow the cursor.
      const pivot = pinPivot(e.kind, e.pins, pinDrag.k);
      if (pivot) {
        const [hx, hy] = toPx(pivot);
        const [px, py] = toPx(p);
        ctx.strokeStyle = '#ffb24d';
        ctx.lineWidth = 1.5;
        ctx.setLineDash([4, 4]);
        ctx.beginPath();
        ctx.moveTo(hx, hy);
        ctx.lineTo(px, py);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.beginPath();
        ctx.arc(hx, hy, Math.max(3, cam.scale * 0.12), 0, Math.PI * 2);
        ctx.stroke();
      }
      const [bx, by] = toPx(p);
      const connects = pinExistsAt(p, pinDrag.id);
      ctx.strokeStyle = connects ? '#4bff6a' : '#8a8a98';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(bx, by, Math.max(5, cam.scale * 0.18), 0, Math.PI * 2);
      ctx.stroke();
    }
  }

  if (marquee) {
    ctx.strokeStyle = '#5a8cff';
    ctx.fillStyle = '#5a8cff18';
    ctx.lineWidth = 1;
    const x = Math.min(marquee.x0, marquee.x1);
    const y = Math.min(marquee.y0, marquee.y1);
    const w = Math.abs(marquee.x1 - marquee.x0);
    const h = Math.abs(marquee.y1 - marquee.y0);
    ctx.fillRect(x, y, w, h);
    ctx.strokeRect(x, y, w, h);
  }

  drawSelectionBoxes(view);
  drawMachineSelection();
  // Probe flags are schematic furniture: in the calm zoomed-out view they
  // are clutter over a picture that is about voltage, not instrumentation.
  if (cam.scale >= LOD_FULL) drawProbeMarkers();
  drawFloatScopes();
  drawCursors(now);
  syncPropsPanel();
  syncPropsDialog();
  // Panel windows are HTML overlays: re-derive members and refresh every
  // widget from this frame's solver values.
  panelHost.tick(panels);
  // The other direction: pointing at a part on the canvas makes its row in
  // whichever panel owns it read hot. Self-guarded on an unchanged id.
  panelHost.setCanvasHover(hover?.id ?? null);

  dock.update(now, probes, traces, dockScope, netNames());

  // The armed chip. Pulled once a frame rather than pushed from the dozen
  // places that arm and disarm, so it cannot go stale — and it is a no-op
  // both while the palette is unmounted (every desktop session) and while
  // the description has not changed.
  palette.sync(armedForPalette());

  // Deliberately NO hover readout: voltages, currents and power are only
  // visible through probes, scopes and panel meters — placing an instrument
  // IS the game. (Heat and breakage already show on the part itself: glow,
  // smoke, scorch.)

  const mode = repairing
    ? 'repair tool: click a charred part to put it back into service (Esc exits)'
    : panelTool
    ? 'control panel: drag a region around the parts you want on it (Esc cancels)'
    : labelBoxTool
    ? 'label box: drag a box round some parts and say what they are — no window, no grouping (Esc cancels)'
    : netLabelTool
    ? 'name a net: click a point on it. The name is a LABEL — the same name on two nets joins nothing (Esc cancels)'
    : pasting
      ? `pasting ${pasting.length} parts (Q rotates, X/Y flips, click places, Esc cancels)`
      : placing
        ? `placing: ${placing.name} (click or drag, Q rotates, X/Y flips, Esc exits)`
        : machineDrag
          ? 'moving the FREIGHT HOIST — release to place it (⌘Z undoes the whole move)'
          : selectedMachine
            ? 'FREIGHT HOIST selected — drag the chip to move the whole machine; its nine terminals come with it (ⓘ opens its pinout)'
            : selectedIds.size > 1
              ? `${selectedIds.size} selected (drag moves, Q rotates, X/Y flips, shift+ adds, alt+ removes, Del deletes)`
              : '';
  const note = history.note();
  // A standing count of the damage, so somebody working at the other end of
  // the world still knows the room has a dead part in it.
  let dead = 0;
  let hottest = 0;
  for (const d of damage.values()) {
    if (d.broken) dead++;
    else if (d.stress > hottest) hottest = d.stress;
  }
  const harm =
    dead > 0
      ? `   ⚠ ${dead} part${dead === 1 ? '' : 's'} broken (K = repair tool)`
      : hottest > 0.7
        ? `   ⚠ something is smoking (${(hottest * 100) | 0}% of its limit)`
        : '';
  // Sound needs a user gesture before the browser will play anything: say so
  // once, only while it is actually blocking audio, and never once running.
  const snd = audio.status();
  const sound = snd.needsGesture
    ? '  ⚠ click to enable sound'
    : speakerIds.length > 0 && !online
      ? `  ${speakerIds.length} speaker${speakerIds.length === 1 ? '' : 's'} silent offline (no substep sampler in the local sim)`
      : '';
  const hints = hintsOpen
    ? `\nparts: R C L W G V D N P M A U 5 S B T Z E F I · ⇧V rail · drag part = move · drag the hoist chip = move the machine · dbl-click = edit values · right-click = menu` +
      `\ndrag pin = reshape part · W then drag = wire · drag empty = select · Q rotate · X/Y flip · ⌘Z undo · ⌘C/⌘V copy/paste · 1/2 probe · 3 listen · 0 ref · O scope · \` dock · J panel · ⇧J label box · ⇧W name a net · [ ] sidebars · K repair · Del delete` +
      `\nH home district · shift+H fit everything · ⇧R rooms · Enter chat · wheel = zoom (0.4–200 px/unit) · pan: middle / ctrl+drag / space+drag · ? hides this`
    : `\n? controls`;
  // Which room, first thing on the status line — the clickable version of
  // the same fact is the chip in the top-right corner.
  const where = roomsUI.hudLabel();
  hud.textContent =
    `EE Game   ${where ? `${where}   ` : ''}sim t = ${simTime.toFixed(2)} s   ` +
    (online ? `● ONLINE — ${population} player${population === 1 ? '' : 's'}` : '○ offline (local sim)') +
    `   ${perf.drawn}/${perf.total} parts drawn @ ${cam.scale.toFixed(1)} px/unit (cull ${perf.cull.toFixed(2)} ms)` +
    (mode ? `   ${mode}` : '') +
    (note ? `   ${note}` : '') +
    harm +
    sound +
    hints;

  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
