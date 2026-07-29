// WebSocket net layer. The client is a renderer of server truth; when no
// server answers, main.ts falls back to the local WASM sim.

import type { DocOp, ElementSpec, InteractOp } from './circuit';
import type { Probe } from './scope';

export interface ServerFrame {
  time: number;
  /** [id, npins, v0, v1, v2, i0, i1, i2, power] per element. */
  e: [number, number, number, number, number, number, number, number, number][];
}

export interface NetHandlers {
  onHello(you: number, elements: ElementSpec[], probes: Probe[]): void;
  onFrame(f: ServerFrame): void;
  onOp(id: number, op: InteractOp): void;
  onDoc(op: DocOp): void;
  onProbes(list: Probe[]): void;
  onSamples(t0: number, dts: number, s: Record<string, number[]>): void;
  onPresence(n: number): void;
  onCursor(who: number, x: number, y: number): void;
  onClose(): void;
}

export interface Net {
  sendInteract(id: number, op: InteractOp): void;
  sendEdit(op: DocOp): void;
  sendProbe(elem: number, pin: number, kind: 'v' | 'i'): void;
  sendCursor(x: number, y: number): void;
}

export function connect(h: NetHandlers): Net {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data as string);
    switch (m.t) {
      case 'hello':
        h.onHello(m.you, m.elements, m.probes ?? []);
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
      case 'samples':
        h.onSamples(m.t0, m.dts, m.s);
        break;
      case 'presence':
        h.onPresence(m.n);
        break;
      case 'cursor':
        h.onCursor(m.who, m.x, m.y);
        break;
    }
  };
  ws.onclose = () => h.onClose();
  ws.onerror = () => ws.close();

  const send = (o: unknown) => {
    if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(o));
  };
  return {
    sendInteract: (id, op) => send({ t: 'interact', id, op }),
    sendEdit: (op) => send({ t: 'edit', op }),
    sendProbe: (elem, pin, kind) => send({ t: 'probe', elem, pin, kind }),
    sendCursor: (x, y) => send({ t: 'cursor', x, y }),
  };
}
