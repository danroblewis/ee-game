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
//   probe flag            X deletes it; right-click = delete/reference/listen
//   drag a part body      move it (whole selection if it is in one)
//   drag a machine        the freight hoist moves as one assembly — grab the
//                         bar across the top of its cabinet (or any cabinet
//                         chrome clear of its terminals); its four fixtures
//                         travel with it and ⌘Z undoes the whole gesture.
//                         Its pins still draw wires and its children still
//                         select individually; shift+drag still marquees.
//   double-click a part   floating property editor next to it
//   right-click           cascading context menu — on a part:
//                         edit/rotate/delete/probe/listen/copy; on empty
//                         canvas: Add part ▸ category ▸ part, paste, scope,
//                         panel, select all.  There is no side palette.
//   ? or /                toggle the help block in the HUD
//   drag from a pin       reshape that part: stretch/shrink/reorient it by
//                         carrying the pin (wires come from W + drag; pins
//                         must overlap to connect)
//   ⌘/Ctrl+C, ⌘/Ctrl+V    copy selection / paste bound to cursor
//   ⌘/Ctrl+Z, +Shift, ^Y  undo / redo — this player's own edits only
//   Q                     rotate placement ghost, paste ghost, or selection
//   1 / 2                 voltage probe / current clamp at hover
//   3                     listen: play that node's waveform (WebAudio)
//                         — Speakers need no probe: every Speaker element in
//                         the document is streamed to the mixer on its own
//                         12.5 kHz tap, muted/soloed from its right-click
//                         menu, with global mute + volume in the scope bar
//   0                     set selected V-probe's reference (differential)
//   O                     drop an in-place oscilloscope;  X delete
//   ` (backquote)         collapse/expand the bottom scope dock (starts collapsed)
//   K                     repair tool: the wrench cursor; click a charred
//                         part to put it back into service (parts break when
//                         you overload them — the server decides, from the
//                         solver, and says so with a toast)
//   J                     drag a control-panel region around some parts
//                         (its floating instrument window follows) — a scope
//                         parked inside a region becomes a widget in that
//                         panel's window; drag it out to detach it again
//   H / shift+H           frame the home district / the whole document
//   wheel zoom (over a scope: timebase) · pan: middle-drag, ctrl+drag, space+drag
//
// The world is large (tens of thousands of parts): the draw loop and the
// pointer hit-tests go through the grid-space spatial index in spatial.ts,
// and the zoom band 0.4..200 px/unit drops symbol detail below ~6 px/unit.

import init, { Sim } from './wasm/sim_wasm';
import {
  demoCircuit,
  MAX_PINS,
  unpackFrame,
  type DocOp,
  type ElementKind,
  type ElementSpec,
  type ElemLive,
  type InteractOp,
  type Point,
} from './circuit';
import { AudioPlayer } from './audio';
import { CATALOG, CATEGORIES, makePins, partsInCategory, type PartDef } from './catalog';
import { History, isTypingTarget } from './history';
import { createHoist, type MachineRect } from './hoist';
import { connect } from './net';
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
  type Panel,
  type PanelHandle,
  type PanelOp,
  type PanelRect,
} from './panel';
import {
  DotFlow,
  drawElement,
  drawElementsLod,
  drawGrid,
  hitTest,
  type Camera,
  type DamageState,
} from './render';
import { SpatialIndex } from './spatial';
import {
  applyScopeControl,
  defaultScopeSettings,
  probeColor,
  renderScopeInto,
  scopeChannels,
  scopeControlAt,
  TraceStore,
  type FloatScope,
  type Probe,
  type ScopeControlId,
} from './scope';
import { createDock } from './dock';

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
};

await init();

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

/** Copy/paste: kinds + pins relative to the selection centroid. */
type ClipItem = { kind: ElementKind; pins: Point[] };
let clipboard: ClipItem[] = [];
let pasting: ClipItem[] | null = null;

/** In-place oscilloscopes: world-anchored, per-player instruments (the shape
 * lives in scope.ts because panel.ts renders the ones a region encloses). */
let floatScopes: FloatScope[] = [];
let sidCounter = 1;

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
      space.update(e);
    }
  } else if (op.t === 'SetKind') {
    const e = elemById(op.id);
    if (e) {
      e.kind = op.kind;
      space.update(e); // pin count can change with the kind
    }
  }
}

let idCounter = 1;
const newId = () => (myId > 0 ? myId : 999) * 1_000_000 + idCounter++;

/** THE HOIST: machine chrome on the canvas plus the goal card. hoist.ts owns
 * every pixel and every DOM node of it; the four fixture parts it stands on
 * (ids 900..903) are ordinary elements the normal renderer draws on top. */
const hoist = createHoist(document.body, { reset: () => net.sendMachineReset() });

/** Ids 900..999 are the server's machine fixtures: players wire to them but
 * cannot move, rotate, retype or delete them INDIVIDUALLY. The server rejects
 * those ops, so applying one locally would only desync this client. The whole
 * assembly moves together through the machine op below. */
const isFixtureId = (id: number) => id >= 900 && id <= 999;

// ------------------------------------------------------ the machine assembly
//
// The freight hoist behaves like a part: click its cabinet to select it, drag
// the cabinet (or the grab bar across its top) to move it. It is NOT an
// element — the four fixtures bolted inside it are, and the server owns them —
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

/** Translate the assembly by an integer grid delta: footprint, chrome and
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

let firstHello = true;
const net = connect({
  onHello(you, serverElements, serverProbes, serverPanels) {
    online = true;
    myId = you;
    elements = serverElements;
    docVersion++;
    space.rebuild(elements);
    probes = serverProbes;
    panels = serverPanels;
    live = new Map();
    // Damage is room state: forget the old room's, and treat whatever the
    // first snapshot carries as history, not as news (no toast, no pop for
    // parts that were already dead when we joined).
    damage = new Map();
    damageSeen = false;
    if (firstHello) {
      firstHello = false;
      fitHome();
    }
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
    // and applying it would rubber-band the terminals against the chrome.
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
  },
  onCursor(who, x, y) {
    if (who !== myId) cursors.set(who, { x, y, seen: performance.now() });
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
    }
  },
});

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

/** The oldest joke in electronics, and the clearest possible failure notice. */
function magicSmoke(id: number) {
  const e = elemById(id);
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
  repairing = true;
  placing = null;
  pasting = null;
  panelTool = false;
  canvas.style.cursor = REPAIR_CURSOR;
  // Offline there is no damage model at all, so there is never anything to
  // repair — say so rather than leaving the wrench clicking on nothing.
  if (!online) toast('offline: the local sim has no damage model — nothing can break, or be fixed');
}

function interact(e: ElementSpec, op: InteractOp) {
  applyOp(e, op); // optimistic; server echo confirms
  if (online) net.sendInteract(e.id, op);
  else localSim.interact(e.id, op);
}

/** Local undo/redo: every edit this player makes funnels through editDoc. */
const history = new History(editDoc);

function editDoc(op: DocOp) {
  if (isFixtureId(op.t === 'Add' ? op.spec.id : op.id)) return; // locked fixture
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
  removeScope: (sid) => {
    floatScopes = floatScopes.filter((s) => s.sid !== sid);
  },
  interact: (e, op) => interact(e, op),
  op: panelOp,
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

const cam: Camera = { scale: 48, ox: 60, oy: 60 };
// Exposed for end-to-end tests.
(window as unknown as { __cam: Camera }).__cam = cam;
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
 * demo bench. 'H' returns here, shift+H frames everything. */
const HOME_RECT = { x0: -10, y0: -10, x1: 60, y1: 60 };

/** Frame a grid-space rect, with margin, clamped to a usable zoom band. */
function fitRect(x0: number, y0: number, x1: number, y1: number, loScale = 4, hiScale = 60) {
  const w = Math.max(1, x1 - x0 + 4);
  const ht = Math.max(1, y1 - y0 + 4);
  const fit = Math.min(window.innerWidth / w, window.innerHeight / ht);
  cam.scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, Math.max(loScale, Math.min(hiScale, fit))));
  cam.ox = (window.innerWidth - (x0 + x1) * cam.scale) / 2;
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

/** 'H' / join / reset: frame what is in the home district (or the district
 * itself when it is still empty). */
function fitHome() {
  const inHome = space
    .query(HOME_RECT.x0, HOME_RECT.y0, HOME_RECT.x1, HOME_RECT.y1)
    .filter((e) =>
      e.pins.every(
        ([x, y]) =>
          x >= HOME_RECT.x0 && x <= HOME_RECT.x1 && y >= HOME_RECT.y0 && y <= HOME_RECT.y1,
      ),
    );
  const b = pinBounds(inHome);
  if (b) fitRect(...b, 8, 60);
  else fitRect(HOME_RECT.x0, HOME_RECT.y0, HOME_RECT.x1, HOME_RECT.y1, MIN_SCALE, 60);
}

/** shift+H: frame the whole document, however far it sprawls. */
function fitAll() {
  const b = pinBounds(elements);
  if (b) fitRect(...b, MIN_SCALE, 60);
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

function choosePart(p: PartDef) {
  placing = p;
  pasting = null;
  repairing = false;
  closeCtxMenu();
  canvas.style.cursor = 'crosshair';
}

// ---------------------------------------------------------------- props
const propsDiv = document.getElementById('props') as HTMLDivElement;
const propsDlg = document.getElementById('propsdlg') as HTMLDivElement;
/** Element the floating editor is open for (double-click / context menu). */
let dlgFor: number | null = null;

const FIELD_LABELS: Record<string, string> = {
  ohms: 'resistance Ω',
  rated_watts: 'rated W',
  farads: 'capacitance F',
  henries: 'inductance H',
  dc: 'DC volts',
  amp: 'AC amplitude V',
  hz: 'frequency Hz',
  phase: 'phase rad',
  amps: 'current A',
  closed: 'closed',
  vz: 'zener V',
  color: 'color 0-4',
  beta: 'beta',
  vt: 'threshold V',
  k: 'k A/V²',
  rail: 'rail ±V',
  wiper: 'wiper 0-1',
};

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
  h.textContent = `${target.kind.t}  #${target.id}`;
  host.appendChild(h);
  if (onClose) {
    const x = document.createElement('button');
    x.className = 'xbtn';
    x.textContent = '×';
    x.title = 'close (Esc)';
    x.onclick = onClose;
    h.appendChild(x);
  }

  for (const [field, value] of Object.entries(target.kind)) {
    if (field === 't') continue;
    const label = document.createElement('label');
    const span = document.createElement('span');
    span.textContent = FIELD_LABELS[field] ?? field;
    label.appendChild(span);
    const input = document.createElement('input');
    if (typeof value === 'boolean') {
      input.type = 'checkbox';
      input.checked = value;
      input.onchange = () => {
        const kind = { ...target.kind, [field]: input.checked } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        mark(kind);
      };
    } else {
      input.type = 'number';
      input.step = 'any';
      input.value = String(value);
      input.onchange = () => {
        const num = Number(input.value);
        if (!Number.isFinite(num)) return;
        const kind = { ...target.kind, [field]: num } as ElementSpec['kind'];
        editDoc({ t: 'SetKind', id: target.id, kind });
        mark(kind);
      };
    }
    label.appendChild(input);
    host.appendChild(label);
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

/** Rotate parts 90° clockwise about their shared centroid. */
function rotateElements(sel: ElementSpec[]) {
  if (sel.length === 0) return;
  const [cx, cy] = centroidOf(sel);
  history.begin(sel, sel.length > 1 ? `rotate ${sel.length} parts` : 'rotate part');
  for (const e of sel) {
    const pins = e.pins.map(([x, y]) => [cx - (y - cy), cy + (x - cx)] as Point);
    editDoc({ t: 'Move', id: e.id, pins });
  }
  history.end();
}

const rotateSelection = () => rotateElements(elements.filter((e) => selectedIds.has(e.id)));

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
  if (!online) localSim.setElements(elements);
  history.end();
}

const deleteElements = (sel: ElementSpec[]) => deleteIds(sel.map((e) => e.id));

function copyElements(sel: ElementSpec[]) {
  if (sel.length === 0) return;
  const [cx, cy] = centroidOf(sel);
  clipboard = sel.map((e) => ({
    kind: JSON.parse(JSON.stringify(e.kind)) as ElementKind,
    pins: e.pins.map(([x, y]) => [x - cx, y - cy] as Point),
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
  pasting = clipboard.map((c) => ({ kind: c.kind, pins: c.pins }));
  placing = null;
  commitPaste(at);
}

/** Arm the cursor-bound paste ghost (⌘/Ctrl+V). */
function armPaste() {
  if (clipboard.length === 0) return;
  pasting = clipboard.map((c) => ({ kind: c.kind, pins: c.pins }));
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

/** Drop an in-place oscilloscope with its top-left at a grid point. */
function addFloatScope(at: Point) {
  floatScopes.push({
    sid: sidCounter++,
    x: at[0],
    y: at[1],
    w: 12,
    h: 6,
    set: defaultScopeSettings(5),
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
    { label: `Delete${many}`, hint: 'X', run: () => deleteElements(groupOf(e)) },
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
    { label: 'Delete probe', hint: 'X', run: () => deleteProbe(p) },
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
    { sep: true },
    { label: 'Select all', run: selectAll },
  );
  return items;
}

// ------------------------------------------------------------------ drags
let panDrag: { x: number; y: number; ox: number; oy: number } | null = null;
/** Dragging one pin of one part: `k` is the pin index being carried. */
let pinDrag: { id: number; k: number; moved: boolean; lastSent: number } | null = null;
let placeDrag: { a: Point; b: Point } | null = null;
let marquee: { x0: number; y0: number; x1: number; y1: number; add: boolean } | null = null;
let moveDrag: {
  items: { id: number; startPins: Point[] }[];
  start: Point;
  lastSent: number;
  moved: boolean;
  clickTarget: number;
} | null = null;
let scopeDrag: { s: FloatScope; dx: number; dy: number } | null = null;
let scopeResize: { s: FloatScope } | null = null;
/** Dragging the whole machine assembly. Its own gesture, not moveDrag: the
 * machine is not an element, and its children are locked against the document
 * Move op that moveDrag issues. */
interface MachineDrag {
  /** Grid point where the cabinet was grabbed. */
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
 * and all four children from the same snapshot, so the chrome and the terminals
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

canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  const z = scopeZoneAt(ev.clientX, ev.clientY);
  if (z) {
    const tb = z.s.set.timebase;
    z.s.set.timebase = Math.min(60, Math.max(0.001, tb * Math.exp(ev.deltaY * 0.001)));
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
  if (panelTool) {
    const p = snap(ev.clientX, ev.clientY);
    panelDrag = { a: p, b: p };
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
      floatScopes = floatScopes.filter((s) => s.sid !== z.s.sid);
    } else if (z.zone === 'chan') {
      const s = z.s;
      if (s.pids === null) s.pids = probes.map((p) => p.pid);
      s.pids = s.pids.includes(z.pid) ? s.pids.filter((x) => x !== z.pid) : [...s.pids, z.pid];
    } else if (z.zone === 'ctrl') {
      applyScopeControl(z.s.set, z.id, scopeProbes(z.s).length);
    } else if (z.zone === 'title') {
      const [gx, gy] = toGrid(ev.clientX, ev.clientY);
      scopeDrag = { s: z.s, dx: gx - z.s.x, dy: gy - z.s.y };
    } else if (z.zone === 'resize') {
      scopeResize = { s: z.s };
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
  // Pins take priority: dragging a terminal reshapes ITS part — stretch,
  // shrink, reorient. Wires are placed with W (or the right-click menu),
  // never by pin-dragging.
  if (!ev.shiftKey) {
    const own = pinOwnerAt(ev.clientX, ev.clientY);
    if (own) {
      history.begin([own.e], 'reshape part'); // pins mutate in place mid-drag
      pinDrag = { id: own.e.id, k: own.k, moved: false, lastSent: 0 };
      selectedIds = new Set([own.e.id]);
      selectedProbe = null;
      selectedMachine = false;
      return;
    }
  }
  const e = elementAt(ev.clientX, ev.clientY);
  // The machine assembly comes LAST, after pins and after parts: a pointer on
  // a fixture terminal or child part still selects that part (fixture pins
  // are bolted down, so they never reshape-drag). Only the cabinet's own chrome — the
  // grab bar, the frame, empty faceplate away from the children — picks up the
  // whole machine. Shift+drag stays a marquee, so a selection can still be
  // swept across the cabinet.
  if (!e && !ev.shiftKey && hoist.zoneAt(cam, ev.clientX, ev.clientY)) {
    startMachineDrag(ev.clientX, ev.clientY);
    return;
  }
  if (!e) {
    marquee = {
      x0: ev.clientX,
      y0: ev.clientY,
      x1: ev.clientX,
      y1: ev.clientY,
      add: ev.shiftKey,
    };
    return;
  }
  if (ev.shiftKey) {
    // Shift+click toggles membership in the selection.
    if (selectedIds.has(e.id)) selectedIds.delete(e.id);
    else selectedIds.add(e.id);
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
canvas.addEventListener('dblclick', (ev) => {
  if (placing || pasting) return;
  if (scopeZoneAt(ev.clientX, ev.clientY)) return;
  const e = elementAt(ev.clientX, ev.clientY);
  if (e) openPropsDialog(e);
});

canvas.addEventListener('pointermove', (ev) => {
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
    return;
  }
  if (scopeResize) {
    const [gx, gy] = toGrid(ev.clientX, ev.clientY);
    scopeResize.s.w = Math.max(6, Math.round(gx - scopeResize.s.x));
    scopeResize.s.h = Math.max(4, Math.round(gy - scopeResize.s.y));
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
      const cur = e.pins[pinDrag.k]!;
      const collides = e.pins.some(
        (p, i) => i !== pinDrag!.k && p[0] === here[0] && p[1] === here[1],
      );
      // A part must not collapse onto itself: two of its own terminals on
      // one grid point would merge its nodes.
      if (!collides && (cur[0] !== here[0] || cur[1] !== here[1])) {
        e.pins[pinDrag.k] = here;
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
      placeMachineDrag(machineDrag); // live, snapped, chrome + children as one
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
  // The machine is hit-tested last here too, so the cursor promises exactly
  // what pointerdown will do.
  const mz =
    z || pz || over || onPin ? null : hoist.zoneAt(cam, ev.clientX, ev.clientY);
  hoist.setHot(mz === 'grab');
  canvas.style.cursor = repairing
    ? REPAIR_CURSOR
    : placing || pasting || panelTool
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
        : pz
          ? pz.zone === 'resize'
            ? PANEL_HANDLE_CURSOR[pz.handle]
            : pz.zone === 'close'
              ? 'pointer'
              : 'move'
          : onPin
            ? 'move' // drag from a terminal carries that pin (reshape)
            : over?.kind.t === 'Switch' || over?.kind.t === 'Button'
              ? 'pointer'
              : over
                ? 'move' // plain drag moves any part
                : mz === 'grab'
                  ? 'grab' // the machine's title strip: pick the whole thing up
                  : mz
                    ? 'move' // its cabinet drags the assembly too
                    : 'default';
});

canvas.addEventListener('pointerup', (ev) => {
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
    scopeDrag = null;
    scopeResize = null;
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
  if (machineDrag) {
    endMachineDrag();
    return;
  }
  if (marquee) {
    const [gx0, gy0] = toGrid(Math.min(marquee.x0, marquee.x1), Math.min(marquee.y0, marquee.y1));
    const [gx1, gy1] = toGrid(Math.max(marquee.x0, marquee.x1), Math.max(marquee.y0, marquee.y1));
    const dragged = Math.abs(marquee.x1 - marquee.x0) + Math.abs(marquee.y1 - marquee.y0) > 6;
    if (!marquee.add) {
      selectedIds.clear();
      selectedProbe = null;
      selectedMachine = false;
    }
    if (dragged) {
      for (const e of space.query(gx0, gy0, gx1, gy1)) {
        if (e.pins.some(([x, y]) => x >= gx0 && x <= gx1 && y >= gy0 && y <= gy1)) {
          selectedIds.add(e.id);
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
    editDoc({ t: 'Add', spec: { id, kind, pins: makePins(kind, a, b) } });
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
// momentary button stuck closed in a shared room.
canvas.addEventListener('pointercancel', () => {
  if (buttonHeld) {
    interact(buttonHeld, { t: 'SetSwitch', closed: false });
    buttonHeld = null;
  }
  // A lost pointer must not leave the machine half-moved and un-undoable:
  // commit where it actually got to, as one entry.
  endMachineDrag();
  // Same rule for a half-carried pin: commit where it landed, one entry.
  if (pinDrag) {
    const e = elemById(pinDrag.id);
    if (pinDrag.moved && e) editDoc({ t: 'Move', id: e.id, pins: e.pins });
    history.end();
    pinDrag = null;
  }
});

window.addEventListener('keydown', (ev) => {
  const inEditor =
    ev.target instanceof Node && (propsDiv.contains(ev.target) || propsDlg.contains(ev.target));
  if (inEditor) {
    if (ev.key === 'Escape' && dlgFor !== null) closePropsDialog();
    return;
  }
  if (panelHost.owns(ev.target)) return; // typing in a panel window

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
    // Peel one layer at a time: menu, then editor, then tools/selection.
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
    panelTool = false;
    panelDrag = null;
    repairing = false;
    selectedIds.clear();
    selectedProbe = null;
    selectedMachine = false;
    canvas.style.cursor = 'default';
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
  if (ev.key === 'j' || ev.key === 'J') {
    // Arm the panel tool: drag a region around the parts you want on a
    // control panel. Its window appears as soon as the region exists.
    panelTool = true;
    placing = null;
    pasting = null;
    repairing = false;
    canvas.style.cursor = 'crosshair';
    return;
  }
  if (ev.key === 'Delete' || ev.key === 'Backspace' || ev.key === 'x') {
    // Probes win over the part selection: pointing at a flag and pressing X
    // must never delete the parts you happen to have selected elsewhere.
    const pr =
      (mouse ? probeAt(mouse.x, mouse.y) : undefined) ??
      (selectedProbe !== null ? probes.find((p) => p.pid === selectedProbe) : undefined);
    if (pr) {
      deleteProbe(pr);
      return;
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
        kind: c.kind,
        pins: c.pins.map(([x, y]) => [-y, x] as Point),
      }));
    } else {
      rotateSelection();
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
const fmt = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(2)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(2)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(2)} µ${unit}`;
  if (a >= 1e-9) return `${(v * 1e9).toFixed(2)} n${unit}`;
  return `0 ${unit}`;
};


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
function drawHighlight(e: ElementSpec, strong: boolean) {
  // One soft rounded box over the WHOLE part — no pin-to-pin "skeleton"
  // lines: the symbol already draws its own geometry, so the highlight only
  // has to say "this one", not re-trace it.
  const P = e.pins.map(toPx);
  const pad = Math.max(6, cam.scale * 0.5);
  let x0 = Math.min(...P.map((p) => p[0])) - pad;
  let y0 = Math.min(...P.map((p) => p[1])) - pad;
  let x1 = Math.max(...P.map((p) => p[0])) + pad;
  let y1 = Math.max(...P.map((p) => p[1])) + pad;
  // One-pin parts draw their body away from the pin: stretch to cover it.
  if (e.kind.t === 'Rail') y0 -= cam.scale * 0.85;
  if (e.kind.t === 'Ground') y1 += cam.scale * 0.72;
  ctx.fillStyle = strong ? '#5a8cff' : '#4a7de0';
  ctx.globalAlpha = strong ? 0.22 : 0.14;
  roundRectPath(ctx, x0, y0, x1 - x0, y1 - y0, Math.min(8, pad * 0.6));
  ctx.fill();
  ctx.globalAlpha = strong ? 0.55 : 0.35;
  ctx.strokeStyle = strong ? '#5a8cff' : '#4a7de0';
  ctx.lineWidth = 1.5;
  ctx.stroke();
  // Pin dots stay: they are the wire targets, not skeleton.
  ctx.fillStyle = '#7db1ff';
  ctx.globalAlpha = 0.9;
  for (const [x, y] of P) {
    ctx.beginPath();
    ctx.arc(x, y, Math.max(3, cam.scale * 0.11), 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
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
      drawScopePlaceholder(s, owner);
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
      renderScopeInto(ctx, bx, by, bw, bh, traces, active, s.set.timebase, s.set);
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
 *   >= 6  full symbols (dots, glow, text)
 *   2..6  conductor chains only, still solver-colored
 *   < 2   one segment per element */
const LOD_FULL = 6;
const LOD_CHAIN = 2;

/** Reused draw list so a steady frame allocates nothing. */
const visible: ElementSpec[] = [];
/** Above this many on-screen elements, skip the document-order sort: at that
 * density the z-order of overlapping symbols is not visible anyway. */
const SORT_LIMIT = 3000;
/** Last frame's cull cost + counts, for the perf line in the HUD. */
const perf = { cull: 0, drawn: 0, total: 0 };

let simDebt = 0;
let lastT = performance.now();

function frame(now: number) {
  const wallDt = Math.min(0.1, (now - lastT) / 1000);
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

  // Machine chrome (the hoist) is scenery: it goes down before the panel
  // regions and the schematic so its fixture parts stay visible and wire-able.
  // The same call refreshes the goal card overlay.
  // The locked fixture parts, so the hoist can name its own terminals.
  hoist.draw(
    ctx,
    cam,
    now,
    wallDt,
    elements
      .filter((e) => e.id >= 900 && e.id <= 903)
      .map((e) => ({ id: e.id, pins: e.pins as [number, number][] })),
  );

  // Panel regions sit under the schematic: they frame parts, never hide them.
  // The one under the pointer (or being dragged) shows its resize grips.
  const hotPanel = mouse ? panelHotAt(cam, panels, mouse.x, mouse.y) : null;
  drawPanelRegions(
    ctx,
    cam,
    panels,
    panelResize?.plid ?? panelMove?.plid ?? hotPanel?.plid ?? null,
  );
  if (panelDrag) drawPanelGhost(ctx, cam, panelDrag.a, panelDrag.b);

  // Cull to the viewport through the spatial index: a 20k-element world
  // costs what is on screen, not what exists.
  const view = viewRect();
  const cull0 = performance.now();
  space.query(view[0], view[1], view[2], view[3], visible);
  if (visible.length <= SORT_LIMIT) space.sortByDoc(visible);
  perf.cull = performance.now() - cull0;
  perf.drawn = visible.length;
  perf.total = space.count;

  if (cam.scale >= LOD_FULL) {
    for (const e of visible) {
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
    // plus a marker on every dead part, because finding them IS the repair.
    drawElementsLod(ctx, cam, visible, live, cam.scale < LOD_CHAIN, damage);
  }

  // Hover highlight (blue element + pin dots), Falstad-style.
  const zHover = mouse ? scopeZoneAt(mouse.x, mouse.y) : null;
  const md = moveDrag;
  const hover = md
    ? elemById(md.clickTarget)
    : mouse && !placing && !pasting && !panelTool && !zHover
      ? elementAt(mouse.x, mouse.y)
      : undefined;
  if (hover) drawHighlight(hover, true);
  for (const id of selectedIds) {
    if (!inView(view, id)) continue;
    const e = elemById(id);
    if (e && e !== hover) drawHighlight(e, false);
  }

  // Ghost previews for in-progress edits.
  ctx.globalAlpha = 0.45;
  if (placeDrag && placing) {
    const kind = placing.make();
    const clicked = placeDrag.b[0] === placeDrag.a[0] && placeDrag.b[1] === placeDrag.a[1];
    const b = clicked ? placeEnd(placeDrag.a) : placeDrag.b;
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, placeDrag.a, b) });
  } else if (placing && mouse) {
    const kind = placing.make();
    const a = snap(mouse.x, mouse.y);
    drawElement({ ctx, cam, dots, dtSec: 0 }, { id: 0, kind, pins: makePins(kind, a, placeEnd(a)) });
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
    if (p) {
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
  drawProbeMarkers();
  drawFloatScopes();
  drawCursors(now);
  syncPropsPanel();
  syncPropsDialog();
  // Panel windows are HTML overlays: re-derive members and refresh every
  // widget from this frame's solver values.
  panelHost.tick(panels);

  dock.update(now, probes, traces, dockScope);

  // Deliberately NO hover readout: voltages, currents and power are only
  // visible through probes, scopes and panel meters — placing an instrument
  // IS the game. (Heat and breakage already show on the part itself: glow,
  // smoke, scorch.)

  const mode = repairing
    ? 'repair tool: click a charred part to put it back into service (Esc exits)'
    : panelTool
    ? 'control panel: drag a region around the parts you want on it (Esc cancels)'
    : pasting
      ? `pasting ${pasting.length} parts (Q rotates, click places, Esc cancels)`
      : placing
        ? `placing: ${placing.name} (click or drag, Q rotates, Esc exits)`
        : machineDrag
          ? 'moving the FREIGHT HOIST — release to place it (⌘Z undoes the whole move)'
          : selectedMachine
            ? 'FREIGHT HOIST selected — drag its top bar (or its cabinet) to move the whole machine; its terminals come with it'
            : selectedIds.size > 1
              ? `${selectedIds.size} selected (drag moves, Q rotates, ⌘C copies, X deletes)`
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
    ? `\nparts: R C L W G V D N P M A U 5 S B T Z E F I · ⇧V rail · drag part = move · drag the hoist cabinet = move the machine · dbl-click = edit values · right-click = menu` +
      `\ndrag pin = reshape part · W then drag = wire · drag empty = select · Q rotate · ⌘Z undo · ⌘C/⌘V copy/paste · 1/2 probe · 3 listen · 0 ref · O scope · \` dock · J panel · K repair · X delete` +
      `\nH home district · shift+H fit everything · wheel = zoom (0.4–200 px/unit) · pan: middle / ctrl+drag / space+drag · ? hides this`
    : `\n? controls`;
  hud.textContent =
    `EE Game   sim t = ${simTime.toFixed(2)} s   ` +
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
