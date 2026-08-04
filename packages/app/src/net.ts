// WebSocket net layer. The client is a renderer of server truth; when no
// server answers, main.ts falls back to the local WASM sim.

import type { DocOp, ElementSpec, InteractOp } from './circuit';
import type { MachineMsg } from './hoist';
import type { Panel, PanelOp } from './panel';
import type { LabelBox, LabelBoxOp, NetLabel, NetLabelOp } from './annotate';
import type { Layer, LayerOp } from './layer';
import type { Probe, ScopeOp, SeedScope, WireScope } from './scope';

/** Where a room wants the camera, and which in-place scopes it SEEDED with.
 *
 * `home` is a client-local concern (where to point the camera on arrival).
 * `scopes` is an opening offer, not live state: the server materializes it
 * into the room's real scope list once, when the room is created, and from
 * then on `hello.scopes` and the `scopes` broadcast are the truth. It stays
 * on the view because "save this room as a template" writes it. */
export interface RoomView {
  /** Grid rect `[x0, y0, x1, y1]` framed on first join; absent = the
   * client's own default district. */
  home?: [number, number, number, number];
  scopes?: SeedScope[];
}

/** Which room this socket landed in. Null against a pre-rooms server, which
 * is the one case where the client genuinely cannot say where it is.
 *
 * This is the CLIENT's shape, not the wire's: `machine` and `view` ride the
 * hello beside `room`, not inside it, and `parseHello` is the one place they
 * are joined up. Build one anywhere else and you are guessing. */
export interface RoomHello {
  /** The 6-char code: immutable, the filename, and the `?room=` value. */
  id: string;
  name: string;
  /** The template it was created from — provenance, never a live link. */
  template: string;
  players: number;
  /** True when the room owns a machine, i.e. its goal card belongs on
   * screen. False means "no goal here": hide it instead of latching it. */
  machine: boolean;
  view: RoomView | null;
}

/** Why a socket is being turned away from a room. */
export type GoneReason = 'deleted' | 'unknown' | string;

export interface ServerFrame {
  time: number;
  /** [id, npins, v0..v5, i0..i5, power] per element. */
  e: number[][];
  /** Sim seconds per wall second, smoothed server-side (absent on servers
   * from before it rode the frame). */
  rt?: number;
}

// ------------------------------------------------------------- hello parse
//
// The boundary. `JSON.parse` hands back `any`, so every field read off a
// socket message is a claim the compiler cannot check — and a declared-but-
// never-delivered field is invisible: it reads as `undefined` at runtime and
// typechecks clean forever. That is not hypothetical. `hello` used to be
// forwarded as `m.room`, which carries {id, name, template, players} and
// NOTHING ELSE, while the server writes `view` and `machine` at the TOP LEVEL
// beside it. `RoomHello` declared all six fields, tsc was happy, and two of
// the four things a template promises — the camera it frames and the scopes
// it ships — silently never arrived.
//
// So the wire is parsed into the declared shape here, once, and anything that
// does not match is REPORTED rather than absorbed. See
// `src/wire/hello.contract.json`, which pins this shape from both ends: the
// server asserts it against a real `hello_msg` (crates/server, `the_hello_a_
// room_sends_is_the_shape_the_client_parses`), the client asserts it against
// `parseHello` (`pnpm --filter @ee/app wirecheck`).

/** One field of `hello` that did not arrive in the shape the client expects.
 * Never fatal — a client that renders four fifths of a room beats a blank
 * page — but never silent either: it means the two halves have drifted. */
export interface WireDrift {
  /** Dotted path as the CLIENT expects it, e.g. `hello.view.home`. */
  field: string;
  /** What that path has to be for the field to reach the player. */
  want: string;
  /** What turned up instead: a type name, `missing`, or where it was found. */
  got: string;
}

/** `hello`, parsed. Exactly the arguments `onHello` takes, plus the list of
 * places the payload disagreed with this client. */
export interface ParsedHello {
  you: number;
  elements: ElementSpec[];
  probes: Probe[];
  panels: Panel[];
  /** The room's in-place oscilloscopes. Room state like `panels` — NOT the
   * `view.scopes` seeds, which are a template's opening offer and are
   * materialized into this list once, server-side, when the room is made. */
  scopes: WireScope[];
  /** The two ANNOTATION primitives: boxes with titles, and names pinned to
   * grid points. Both are room state and both ride the hello beside the
   * panels; neither means anything electrically. */
  labelBoxes: LabelBox[];
  netLabels: NetLabel[];
  room: RoomHello | null;
  drift: WireDrift[];
}

const typeName = (v: unknown): string =>
  v === undefined ? 'missing' : v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * Turn a raw `hello` message into the shape the rest of the client is written
 * against. Never throws: a malformed payload yields defaults plus a `drift`
 * entry naming the field, so the failure is a loud line in the console and a
 * failing check, not a feature that quietly stops existing.
 */
export function parseHello(raw: unknown): ParsedHello {
  const drift: WireDrift[] = [];
  const note = (field: string, want: string, got: unknown) =>
    drift.push({ field, want, got: typeof got === 'string' ? got : typeName(got) });

  if (!isRecord(raw)) {
    note('hello', 'object', raw);
    return {
      you: 0,
      elements: [],
      probes: [],
      panels: [],
      scopes: [],
      labelBoxes: [],
      netLabels: [],
      room: null,
      drift,
    };
  }
  const m = raw;

  let you = 0;
  if (typeof m.you === 'number' && Number.isFinite(m.you)) you = m.you;
  else note('hello.you', 'number', m.you);

  /** Optional-on-the-wire list: absent is fine (old servers), wrong is not. */
  const list = <T>(v: unknown, field: string): T[] => {
    if (v === undefined) return [];
    if (Array.isArray(v)) return v as T[];
    note(field, 'array', v);
    return [];
  };
  const elements = list<ElementSpec>(m.elements, 'hello.elements');
  const probes = list<Probe>(m.probes, 'hello.probes');
  const panels = list<Panel>(m.panels, 'hello.panels');
  const scopes = list<WireScope>(m.scopes, 'hello.scopes');
  const labelBoxes = list<LabelBox>(m.labelboxes, 'hello.labelboxes');
  const netLabels = list<NetLabel>(m.netlabels, 'hello.netlabels');

  // No `room` key at all = a server from before rooms existed. That is a
  // supported server, not drift: the chip says "this server has no room list"
  // and everything else works.
  let room: RoomHello | null = null;
  if (m.room !== undefined && m.room !== null) {
    if (!isRecord(m.room)) {
      note('hello.room', 'object', m.room);
    } else {
      const r = m.room;
      const str = (v: unknown, field: string): string => {
        if (typeof v === 'string') return v;
        note(field, 'string', v);
        return '';
      };
      const id = str(r.id, 'hello.room.id');
      const name = str(r.name, 'hello.room.name');
      const template = str(r.template, 'hello.room.template');
      let players = 0;
      if (typeof r.players === 'number' && Number.isFinite(r.players)) players = r.players;
      else note('hello.room.players', 'number', r.players);

      // THE JOIN. `machine` and `view` are top-level: they are this client's
      // half of a room (a goal card to show, a camera to fly, instruments to
      // materialize), not registry metadata about it. A server that nests
      // them instead is still understood — losing a template's whole camera
      // over a moved brace would be absurd — but it is reported, because one
      // of the two halves is then wrong.
      let machineRaw = m.machine;
      if (machineRaw === undefined && r.machine !== undefined) {
        machineRaw = r.machine;
        note('hello.machine', 'boolean beside `room`', 'nested inside hello.room');
      }
      let machine = false;
      if (typeof machineRaw === 'boolean') machine = machineRaw;
      else note('hello.machine', 'boolean', machineRaw);

      let viewRaw = m.view;
      if (viewRaw === undefined && r.view !== undefined) {
        viewRaw = r.view;
        note('hello.view', 'object beside `room`', 'nested inside hello.room');
      }
      let view: RoomView | null = null;
      if (viewRaw === undefined) {
        // Every rooms-era server sends a view object, even an empty one.
        note('hello.view', 'object', viewRaw);
      } else if (viewRaw === null) {
        view = null; // an explicit "no opinion": the client's own district
      } else if (!isRecord(viewRaw)) {
        note('hello.view', 'object', viewRaw);
      } else {
        view = {};
        const h = viewRaw.home;
        if (h !== undefined && h !== null) {
          if (
            Array.isArray(h) &&
            h.length === 4 &&
            h.every((n) => typeof n === 'number' && Number.isFinite(n))
          ) {
            view.home = [h[0] as number, h[1] as number, h[2] as number, h[3] as number];
          } else {
            note('hello.view.home', 'four finite numbers', h);
          }
        }
        const s = viewRaw.scopes;
        if (s !== undefined) {
          if (Array.isArray(s)) {
            // Seeds are hand-editable (a template is a file): keep the
            // objects, drop anything that is not one. `seedToScope` clamps
            // every field inside them.
            view.scopes = s.filter(isRecord) as unknown as SeedScope[];
          } else {
            note('hello.view.scopes', 'array', s);
          }
        }
      }

      room = { id, name, template, players, machine, view };
    }
  }

  return { you, elements, probes, panels, scopes, labelBoxes, netLabels, room, drift };
}

/** One line per drifted field, for a console or a toast. */
export function describeDrift(drift: WireDrift[]): string {
  return drift.map((d) => `${d.field}: want ${d.want}, got ${d.got}`).join('; ');
}

/** Why the server refused an op. Mirrors `sim_core::Reject` through
 * `server/main.rs`'s `reject_msg`, and is byte-identical in shape to what the
 * client's own `checkDocument` returns — one implementation, two callers, so
 * the two sides can never disagree about what is placeable. */
export interface RejectMsg {
  /** The player whose op was refused. Only that client should react. */
  who: number;
  /** Which path refused it: "edit" | "interact" | "repair" | "machinemove". */
  ctx: string;
  /** Machine-readable reason: "bad_value", "collapsed_pins", "shorted_source",
   * "conflicting_sources", "source_loop", "will_not_converge", "unsolvable",
   * "unsolvable_switched". */
  code: string;
  /** The primary offending element, when the refusal is pinned to one. */
  id: number | null;
  /** EVERY implicated element — both halves of a conflict, the whole of a
   * source loop — so the callout can point at all of them. */
  ids: number[];
  /** A sentence for the player, written as a DRC callout. */
  hint: string;
}

export interface NetHandlers {
  /** The late-join payload. `room` is null only against a server from before
   * rooms existed — everything else about that server still works. */
  onHello(
    you: number,
    elements: ElementSpec[],
    probes: Probe[],
    panels: Panel[],
    scopes: WireScope[],
    labelBoxes: LabelBox[],
    netLabels: NetLabel[],
    room: RoomHello | null,
  ): void;
  /** The room was renamed by somebody (possibly us): every open chip, tab
   * title and browser row updates from this, not from the PATCH's reply. */
  onRoomMeta(id: string, name: string): void;
  /** This socket is not going to get a room: it was deleted under us, or the
   * code is unknown. The server closes right after, so the reconnect loop
   * must be pointed somewhere else before it fires. */
  onRoomGone(id: string, reason: GoneReason): void;
  onFrame(f: ServerFrame): void;
  onOp(id: number, op: InteractOp): void;
  onDoc(op: DocOp): void;
  onProbes(list: Probe[]): void;
  onPanels(list: Panel[]): void;
  /** The room's in-place oscilloscopes, whole list, after any change — the
   * step that used to be missing entirely. Same contract as `onPanels`: the
   * server's list is the truth, including for whoever sent the op. */
  onScopes(list: WireScope[]): void;
  /** Label boxes: words on the sheet. Whole list, never a delta — a lagging
   * subscriber SKIPS messages, and a skipped delta desyncs forever. */
  onLabelBoxes(list: LabelBox[]): void;
  /** Net labels: names pinned to grid points. Same whole-list rule. */
  onNetLabels(list: NetLabel[]): void;
  /** WHICH net is named what, DERIVED by the server from the compiled
   * document. `live` is the nlids whose anchor is a real junction (everything
   * else is detached); `probe` is `[pid, nlid]` for probes on named nets.
   *
   * Derived, so it is never persisted and never sent by the client. It has to
   * come from the server because the client has no netlist: node membership is
   * an equivalence class the solver computes, and a second implementation of
   * it here would be a second answer to disagree with. */
  onNetMap(live: number[], probe: [number, number][]): void;
  /** Sensor layers plus who is driving each, `[[lid, who], ...]`. Rectangles
   * and claims only — there is no field on this message for a device, and
   * there must never be one. */
  onLayers(list: Layer[], claims: [number, number][]): void;
  /** Live sensor readings, `[[element_id, q], ...]` with `q` a u16 over full
   * scale, changed sensors only.
   *
   * This is what every OTHER player receives about somebody's camera: the
   * number a photocell is reading. Not the picture, and there is no way to
   * ask for the picture. */
  onSensors(s: [number, number][]): void;
  /** Machine state (the hoist), broadcast once per tick beside "frame". */
  onMachine(m: MachineMsg): void;
  /** Damage SNAPSHOT: `[id, stress01, broken01]` for every part worth
   * drawing, dead ones first. Authoritative and complete — replace the whole
   * damage map from it — and only sent while something is stressed or
   * broken, plus one empty message when the room goes quiet again. */
  onDamage(parts: [number, number, number][]): void;
  onSamples(t0: number, dts: number, s: Record<string, number[]>): void;
  /** Speaker audio taps, keyed by ELEMENT id. A separate stream from
   * `samples` so scope decimation and speaker audio never fight over a
   * cadence; best-effort, so a dropped chunk is a blip of silence.
   *
   * `rt` is the server's realtime ratio — sim seconds produced per wall
   * second. It rides THIS message rather than `frame` because it is the
   * production rate of these very samples: the client can attribute a
   * dilation to the exact chunk without correlating two streams, and a room
   * with no speakers pays nothing for a number nobody would read. */
  onAudio(t0: number, dts: number, s: Record<string, number[]>, rt: number | null): void;
  onPresence(n: number): void;
  onCursor(who: number, x: number, y: number): void;
  /** One line of room chat. `who` is the SAME per-connection id the cursors
   * carry, so a line and a cursor agree about who is who. Includes the
   * scrollback a joiner is replayed right after `hello` (same message shape,
   * so a joiner and a resident can never disagree about what a line is).
   * The text arrives cleaned and capped BY THE SERVER; render it as text —
   * never markup, never a link. */
  onChat(who: number, text: string): void;
  /** An op was refused by the placement gate. Broadcast to everyone (every
   * client already ignores messages it does not care about), because the
   * SENDER needs it to roll back its optimistic local apply. */
  onReject(r: RejectMsg): void;
  onClose(): void;
  /** The server's `hello` disagreed with the shape this client parses. Always
   * a bug in one half or the other, and always worth being noisy about: the
   * fields that go missing this way (a room's camera, its scopes, whether it
   * has a goal) fail by simply not happening. */
  onWireDrift?(drift: WireDrift[]): void;
}

export interface Net {
  sendInteract(id: number, op: InteractOp): void;
  sendEdit(op: DocOp): void;
  sendProbe(elem: number, pin: number, kind: 'v' | 'i'): void;
  sendProbeRef(pid: number, elem: number, pin: number): void;
  sendPanel(op: PanelOp): void;
  /** Place, move, retune or close an in-place oscilloscope. */
  sendScope(op: ScopeOp): void;
  /** Draw/move/rename/delete a label box. Pure annotation: this op can never
   * change what the solver solves. */
  sendLabelBox(op: LabelBoxOp): void;
  /** Name a net (annotation only — the same name on two nets joins nothing). */
  sendNetLabel(op: NetLabelOp): void;
  sendLayer(op: LayerOp): void;
  /** Take or drop the right to drive a layer with your own device. */
  sendLayerClaim(lid: number, claim: boolean): void;
  /**
   * THE ENTIRE EGRESS SURFACE of the camera/microphone feature: a list of
   * `[element_id, q]` with `q` an integer 0..65535. One message per tick at
   * most, ~12 bytes per moved sensor.
   *
   * Integers, not floats: no printed-double ambiguity, two bytes on the wire,
   * and a canonical value that means the same thing on every machine. The
   * server clamps and re-derives the binding anyway — this is a request to
   * read a value, not an instruction to apply one.
   */
  sendSensor(s: [number, number][]): void;
  /** Lower the crate to the floor, zero the hold and re-arm the goal. */
  sendMachineReset(): void;
  /** Drag the whole machine assembly by an integer GRID delta. The server
   * translates the footprint and its four fixture children together at a tick
   * boundary — it is the only op that moves them, and it never touches the
   * mechanism (height, hold timer, landings survive a move). */
  sendMachineMove(dx: number, dy: number): void;
  /** The repair tool: put a broken part back into service. Not a document
   * edit — it is a world event, so it never enters the undo history and it
   * is allowed on the server-owned hoist fixture. */
  sendRepair(id: number): void;
  sendCursor(x: number, y: number): void;
  /** Say a line to the room. Trimmed and capped client-side for the typist's
   * benefit; the server cleans, caps and rate-limits it regardless. */
  sendChat(text: string): void;
  /** Switch rooms in place: drop this socket and open one on `code` (null =
   * the server's default room). No page navigation — see `resetForRoom` in
   * main.ts for the state that has to be dropped with the old room. */
  join(code: string | null): void;
  /** The code this socket is in, or trying to be in. */
  code(): string | null;
}

/** Longest chat line, in characters. Mirrors the server's `MAX_CHAT_LEN` —
 * the input's maxlength and `sendChat`'s slice are a courtesy so a player
 * sees the cap while typing; the server enforces it regardless. */
export const MAX_CHAT_LEN = 240;

const RECONNECT_MS = 2500;

/** Connect with automatic reconnection: a server restart mid-session
 * drops the client to the local sim, then transparently rejoins — the same
 * room it was in, because the code is what the retry loop is pointed at. */
export function connect(h: NetHandlers, room: string | null = null): Net {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  let ws: WebSocket | null = null;
  let wasOpen = false;
  /** The room we want to be in. Null = "whatever the server calls default",
   * which is exactly how a bare `/ws` behaved before rooms existed. */
  let want = room;
  let retry = 0;

  const open = () => {
    retry = 0;
    const q = want ? `?room=${encodeURIComponent(want)}` : '';
    const sock = new WebSocket(`${proto}://${location.host}/ws${q}`);
    ws = sock;
    sock.onopen = () => (wasOpen = true);
    sock.onclose = () => {
      // A socket we deliberately abandoned in join() is no longer `ws`: its
      // death is not an outage and must not drop the client to the local sim
      // or schedule a retry against a room we have already left.
      if (sock !== ws) return;
      if (wasOpen) h.onClose();
      wasOpen = false;
      retry = window.setTimeout(open, RECONNECT_MS);
    };
    sock.onerror = () => sock.close();
    sock.onmessage = onMessage;
  };

  const join = (code: string | null) => {
    want = code;
    if (retry) {
      clearTimeout(retry);
      retry = 0;
    }
    const old = ws;
    ws = null; // marks `old` as superseded for its own onclose
    if (old) {
      old.onmessage = null;
      old.onerror = null;
      old.close();
    }
    wasOpen = false;
    open();
  };

  const onMessage = (ev: MessageEvent) => {
    const m = JSON.parse(ev.data as string);
    switch (m.t) {
      case 'hello': {
        // Parsed, not trusted — see `parseHello`. A server that predates
        // rooms sends no `room` key: null, not a fabricated one, because "I
        // don't know which room" is honest and the chip says so rather than
        // showing a made-up code.
        //
        // THIS IS THE LINE THE BUG WAS ON. `parseHello` being correct never
        // saved anybody: the shipped defect was handing `m.room` to onHello
        // instead of `p.room`, and a guard aimed at the parser cannot see the
        // difference. So wirecheck drives this switch over a stub socket and
        // asserts on what onHello RECEIVES — put `m.room` back and it goes
        // red on the exact three fields the players lost.
        const p = parseHello(m);
        if (p.drift.length > 0) {
          console.error(`hello: wire drift — ${describeDrift(p.drift)}`);
          h.onWireDrift?.(p.drift);
        }
        h.onHello(
          p.you,
          p.elements,
          p.probes,
          p.panels,
          p.scopes,
          p.labelBoxes,
          p.netLabels,
          p.room,
        );
        // Sensor layers ride the hello beside `panels`. Deliberately routed
        // through the SAME handler the live broadcast uses, so a late joiner
        // and a running client can never disagree about what a layer is.
        h.onLayers(
          Array.isArray(m.layers) ? m.layers : [],
          Array.isArray(m.claims) ? m.claims : [],
        );
        if (p.room && p.room.id) want = p.room.id;
        break;
      }
      case 'roommeta':
        h.onRoomMeta(m.id, m.name);
        break;
      case 'roomgone':
        h.onRoomGone(m.id ?? '', m.reason ?? 'unknown');
        break;
      case 'frame':
        h.onFrame(m);
        break;
      case 'op':
        h.onOp(m.id, m.op);
        break;
      case 'doc':
        h.onDoc(m.op);
        break;
      case 'probes':
        h.onProbes(m.list);
        break;
      case 'panels':
        h.onPanels(m.list ?? []);
        break;
      case 'scopes':
        h.onScopes(m.list ?? []);
        break;
      case 'labelboxes':
        h.onLabelBoxes(m.list ?? []);
        break;
      case 'netlabels':
        h.onNetLabels(m.list ?? []);
        break;
      case 'netmap':
        h.onNetMap(
          Array.isArray(m.live) ? m.live : [],
          Array.isArray(m.probes) ? m.probes : [],
        );
        break;
      case 'layers':
        h.onLayers(m.list ?? [], m.claims ?? []);
        break;
      case 'sensors':
        h.onSensors(m.s ?? []);
        break;
      case 'machine':
        h.onMachine(m as MachineMsg);
        break;
      case 'damage':
        h.onDamage(m.parts ?? []);
        break;
      case 'samples':
        h.onSamples(m.t0, m.dts, m.s);
        break;
      case 'audio':
        h.onAudio(m.t0, m.dts, m.s ?? {}, typeof m.rt === 'number' ? m.rt : null);
        break;
      case 'presence':
        h.onPresence(m.n);
        break;
      case 'cursor':
        h.onCursor(m.who, m.x, m.y);
        break;
      case 'chat':
        if (typeof m.who === 'number' && typeof m.text === 'string' && m.text.length > 0) {
          h.onChat(m.who, m.text);
        }
        break;
      case 'reject':
        h.onReject({
          who: m.who,
          ctx: m.ctx ?? '',
          code: m.code ?? 'unsolvable',
          id: typeof m.id === 'number' ? m.id : null,
          // `ids` is newer than `id`; fall back so an older server still
          // produces a usable callout.
          ids: Array.isArray(m.ids) ? m.ids : typeof m.id === 'number' ? [m.id] : [],
          hint: m.hint ?? '',
        });
        break;
    }
  };

  open();

  const send = (o: unknown) => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(o));
  };
  return {
    sendInteract: (id, op) => send({ t: 'interact', id, op }),
    sendEdit: (op) => send({ t: 'edit', op }),
    sendProbe: (elem, pin, kind) => send({ t: 'probe', elem, pin, kind }),
    sendProbeRef: (pid, elem, pin) => send({ t: 'proberef', pid, elem, pin }),
    sendPanel: (op) => send({ t: 'panel', op }),
    sendScope: (op) => send({ t: 'scope', op }),
    sendLabelBox: (op) => send({ t: 'labelbox', op }),
    // Integers only, enforced HERE: the anchor is a grid POINT and the server
    // deserializes it into an i32 pair, so a fractional coordinate would be a
    // dropped message rather than a label.
    sendNetLabel: (op) =>
      send({
        t: 'netlabel',
        op:
          op.t === 'add' || op.t === 'move'
            ? { ...op, x: Math.round(op.x), y: Math.round(op.y) }
            : op,
      }),
    sendLayer: (op) => send({ t: 'layer', op }),
    sendLayerClaim: (lid, claim) => send({ t: 'layerclaim', lid, claim }),
    // Integers, enforced HERE and not merely expected: the one message a
    // camera can produce is normalized to `[int, int]` pairs before it can
    // reach the socket, so there is no shape a caller could hand this that
    // would put anything else on the wire.
    sendSensor: (list) => {
      if (list.length === 0) return;
      const s: [number, number][] = [];
      for (const [id, q] of list) {
        s.push([id | 0, Math.max(0, Math.min(65535, Math.round(q)))]);
      }
      send({ t: 'sensor', s });
    },
    sendMachineReset: () => send({ t: 'machinereset' }),
    // Integers only: the server takes i32 grid units and drops the message
    // outright if it cannot parse them.
    sendMachineMove: (dx, dy) =>
      send({ t: 'machinemove', dx: Math.round(dx), dy: Math.round(dy) }),
    sendRepair: (id) => send({ t: 'repair', id }),
    sendCursor: (x, y) => send({ t: 'cursor', x, y }),
    // Trimmed and capped HERE as well as on the server: the client-side cap
    // is in CHARACTERS (code points, matching the server's `.chars()`), so
    // an emoji is one unit on both ends, not a surrogate pair split in two.
    sendChat: (text) => {
      const t = [...text.trim()].slice(0, MAX_CHAT_LEN).join('');
      if (t.length > 0) send({ t: 'chat', text: t });
    },
    join,
    code: () => want,
  };
}
