// WebSocket net layer. The client is a renderer of server truth; when no
// server answers, main.ts falls back to the local WASM sim.

import type { DocOp, ElementSpec, InteractOp } from './circuit';
import type { MachineMsg } from './hoist';
import type { Panel, PanelOp } from './panel';
import type { Probe, SeedScope } from './scope';

/** Where a room wants the camera, and which in-place scopes it ships with.
 * Both are client-local concerns, which is why they ride the hello rather
 * than being replicated room state. */
export interface RoomView {
  /** Grid rect `[x0, y0, x1, y1]` framed on first join; absent = the
   * client's own default district. */
  home?: [number, number, number, number];
  scopes?: SeedScope[];
}

/** Which room this socket landed in. Null against a pre-rooms server, which
 * is the one case where the client genuinely cannot say where it is. */
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

export interface NetHandlers {
  /** The late-join payload. `room` is null only against a server from before
   * rooms existed — everything else about that server still works. */
  onHello(
    you: number,
    elements: ElementSpec[],
    probes: Probe[],
    panels: Panel[],
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
  onClose(): void;
}

export interface Net {
  sendInteract(id: number, op: InteractOp): void;
  sendEdit(op: DocOp): void;
  sendProbe(elem: number, pin: number, kind: 'v' | 'i'): void;
  sendProbeRef(pid: number, elem: number, pin: number): void;
  sendPanel(op: PanelOp): void;
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
  /** Switch rooms in place: drop this socket and open one on `code` (null =
   * the server's default room). No page navigation — see `resetForRoom` in
   * main.ts for the state that has to be dropped with the old room. */
  join(code: string | null): void;
  /** The code this socket is in, or trying to be in. */
  code(): string | null;
}

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
      case 'hello':
        // A server that predates rooms sends no `room` key: null, not a
        // fabricated one — "I don't know which room" is honest, and the chip
        // says so rather than showing a made-up code.
        h.onHello(m.you, m.elements, m.probes ?? [], m.panels ?? [], m.room ?? null);
        if (m.room && typeof m.room.id === 'string') want = m.room.id;
        break;
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
    sendMachineReset: () => send({ t: 'machinereset' }),
    // Integers only: the server takes i32 grid units and drops the message
    // outright if it cannot parse them.
    sendMachineMove: (dx, dy) =>
      send({ t: 'machinemove', dx: Math.round(dx), dy: Math.round(dy) }),
    sendRepair: (id) => send({ t: 'repair', id }),
    sendCursor: (x, y) => send({ t: 'cursor', x, y }),
    join,
    code: () => want,
  };
}
