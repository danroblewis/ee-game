// WebSocket net layer. The client is a renderer of server truth; when no
// server answers, main.ts falls back to the local WASM sim.

import type { ElementSpec, InteractOp } from './circuit';

export interface ServerFrame {
  time: number;
  e: [number, number, number, number, number][]; // id, va, vb, i, p
}

export interface NetHandlers {
  onHello(you: number, elements: ElementSpec[]): void;
  onFrame(f: ServerFrame): void;
  onOp(id: number, op: InteractOp): void;
  onPresence(n: number): void;
  onCursor(who: number, x: number, y: number): void;
  onClose(): void;
}

export interface Net {
  sendInteract(id: number, op: InteractOp): void;
  sendCursor(x: number, y: number): void;
}

export function connect(h: NetHandlers): Net {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data as string);
    switch (m.t) {
      case 'hello':
        h.onHello(m.you, m.elements);
        break;
      case 'frame':
        h.onFrame(m);
        break;
      case 'op':
        h.onOp(m.id, m.op);
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
    sendCursor: (x, y) => send({ t: 'cursor', x, y }),
  };
}
