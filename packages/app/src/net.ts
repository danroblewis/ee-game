// WebSocket net layer. The client is a renderer of server truth; when no
// server answers, main.ts falls back to the local WASM sim.

import type { DocOp, ElementSpec, InteractOp } from './circuit';
import type { MachineMsg } from './hoist';
import type { Panel, PanelOp } from './panel';
import type { Probe } from './scope';

export interface ServerFrame {
  time: number;
  /** [id, npins, v0..v5, i0..i5, power] per element. */
  e: number[][];
  /** Sim seconds per wall second, smoothed server-side (absent on servers
   * from before it rode the frame). */
  rt?: number;
}

export interface NetHandlers {
  onHello(you: number, elements: ElementSpec[], probes: Probe[], panels: Panel[]): void;
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
}

const RECONNECT_MS = 2500;

/** Connect with automatic reconnection: a server restart mid-session
 * drops the client to the local sim, then transparently rejoins. */
export function connect(h: NetHandlers): Net {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  let ws: WebSocket | null = null;
  let wasOpen = false;

  const open = () => {
    ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.onopen = () => (wasOpen = true);
    ws.onclose = () => {
      if (wasOpen) h.onClose();
      wasOpen = false;
      setTimeout(open, RECONNECT_MS);
    };
    ws.onerror = () => ws?.close();
    ws.onmessage = onMessage;
  };

  const onMessage = (ev: MessageEvent) => {
    const m = JSON.parse(ev.data as string);
    switch (m.t) {
      case 'hello':
        h.onHello(m.you, m.elements, m.probes ?? [], m.panels ?? []);
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
  };
}
