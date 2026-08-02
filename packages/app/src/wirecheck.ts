// Dev-only headless check for the socket boundary. NOT shipped: nothing
// imports it, so the bundle never sees it.
//
//   pnpm --filter @ee/app wirecheck
//
// WHY THIS EXISTS. A declared TypeScript field is a promise about a value the
// compiler never sees: `JSON.parse` returns `any`, so a field the server does
// not send reads as `undefined` at runtime and typechecks clean forever. That
// is how `RoomHello.view` and `RoomHello.machine` came to be declared,
// documented, consumed by main.ts — and never once delivered. The server put
// them beside `room` in the hello; the client forwarded `room` alone. Result:
// a template's camera never framed, its scopes never materialized, and its
// goal card never appeared, with nothing anywhere going red.
//
// So the shape is now PARSED at the boundary (net.ts `parseHello`) and pinned
// in a file both halves assert against: src/wire/hello.contract.json. This
// script is the client half. The server half is a Rust test over a real
// hello_msg (crates/server/src/lifecycle_tests.rs). Move a field on either
// side and one of them fails, naming the path.
//
// TEST AT THE LAYER THE DEFECT CAN OCCUR, NOT ONE BELOW IT. The first version
// of this file only exercised `parseHello` — and `parseHello` was never where
// the bug was. The bug was one layer UP, in connect()'s `case 'hello'`, which
// decided what to hand `onHello`; the whole defect was that it handed over
// `m.room` instead of the parsed object. Proof that the distinction is not
// academic: putting that exact line back — `h.onHello(m.you, ..., m.room)` —
// left `tsc --noEmit` clean, `wirecheck` green and all 48 server tests
// passing. Three guards, none of them touching the line the bug was on.
//
// Hence the last section below, which drives the real `connect()` over a stub
// socket and asserts on the object `onHello` ACTUALLY RECEIVES. The acceptance
// test for any guard here is "would this have caught the original bug?" — and
// that section does, by construction, because it is looking at the value the
// original bug got wrong rather than at a function it called on the way.
//
// What it still CANNOT prove: that the camera lands somewhere a player wants
// to be, or that the seeded scope is pointing at an interesting node. That
// needs a browser and a human.

import {
  connect,
  describeDrift,
  parseHello,
  type NetHandlers,
  type ParsedHello,
  type RoomHello,
  type WireDrift,
} from './net';
import type { ElementSpec } from './circuit';
import type { Panel } from './panel';
import type { Probe } from './scope';

// This file runs under node (see package.json), never in the browser, and the
// repo carries no @types/node — one honest declaration beats a dependency.
declare function require(m: string): { readFileSync(p: string, enc: string): string };
declare const process: { cwd(): string; exitCode: number };

// ------------------------------------------------------------------ harness
let failures = 0;
function check(name: string, ok: boolean, detail = '') {
  if (ok) {
    console.log(`  ok   ${name}`);
  } else {
    failures++;
    console.log(`  FAIL ${name}${detail ? `  (${detail})` : ''}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// -------------------------------------------------------------- the contract

interface Contract {
  types: Record<string, string>;
  sample: Record<string, unknown>;
}

function loadContract(): Contract {
  const rel = 'src/wire/hello.contract.json';
  const tries = [rel, `packages/app/${rel}`];
  for (const p of tries) {
    try {
      return JSON.parse(require('fs').readFileSync(p, 'utf8')) as Contract;
    } catch {
      /* try the next root */
    }
  }
  throw new Error(`cannot read ${rel} from ${process.cwd()}`);
}

/** The JSON type of a dotted path, or 'missing'. */
function typeAt(root: unknown, path: string): string {
  let v: unknown = root;
  for (const key of path.split('.')) {
    if (typeof v !== 'object' || v === null || Array.isArray(v)) return 'missing';
    v = (v as Record<string, unknown>)[key];
    if (v === undefined) return 'missing';
  }
  return v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v;
}

const contract = loadContract();
const sample = contract.sample;

console.log('contract — the sample IS the shape the server promises');
for (const [path, want] of Object.entries(contract.types)) {
  const got = typeAt(sample, path);
  check(`sample.${path} is ${want}`, got === want, `got ${got}`);
}

// ------------------------------------------------- the parse, field by field
//
// Every assertion below fails against the code as it shipped: `m.room` alone
// satisfies none of the last three.

console.log('parseHello — a real hoist hello reaches the client whole');
const p: ParsedHello = parseHello(sample);
check('no drift', p.drift.length === 0, describeDrift(p.drift));
check('you', p.you === 1);
check('elements', p.elements.length === 4);
check('probes', p.probes.length === 2);
check('panels', p.panels.length === 1);
check('room.id', p.room?.id === '7AWF4N');
check('room.name', p.room?.name === 'Hoist practice');
check('room.template', p.room?.template === 'hoist');
check('room.players', p.room?.players === 1);
// The three that silently went missing:
check('room.machine is the goal card', p.room?.machine === true, JSON.stringify(p.room?.machine));
check(
  'room.view.home is the camera the template framed',
  eq(p.room?.view?.home, [22, -2, 68, 28]),
  JSON.stringify(p.room?.view?.home),
);
check(
  'room.view.scopes is the instrument the template ships',
  p.room?.view?.scopes?.length === 1 && p.room.view.scopes[0]?.timebase === 0.5,
  JSON.stringify(p.room?.view?.scopes),
);

// A field that is DECLARED but not delivered must never read as absent-and-
// fine. This is the assertion the type system could not make.
console.log('parseHello — a missing field is loud, not undefined');
{
  const stripped = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  delete stripped.view;
  delete stripped.machine;
  const q = parseHello(stripped);
  const fields = q.drift.map((d) => d.field);
  check('drops view -> drift names hello.view', fields.includes('hello.view'), fields.join(','));
  check(
    'drops machine -> drift names hello.machine',
    fields.includes('hello.machine'),
    fields.join(','),
  );
  check('the room still lands', q.room?.id === '7AWF4N');
  check('machine falls back to "no goal here"', q.room?.machine === false);
  check('view falls back to the client default', q.room?.view === null);
}

// The exact regression: a server that puts them INSIDE room. Understood, so a
// version skew costs nobody their camera — but reported, because one half is
// then wrong.
console.log('parseHello — the nested shape is understood AND reported');
{
  const nested = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  const room = nested.room as Record<string, unknown>;
  room.view = nested.view;
  room.machine = nested.machine;
  delete nested.view;
  delete nested.machine;
  const q = parseHello(nested);
  check('view still arrives', eq(q.room?.view?.home, [22, -2, 68, 28]));
  check('machine still arrives', q.room?.machine === true);
  check('but it is drift', q.drift.length === 2, describeDrift(q.drift));
  check(
    'and it says where it found them',
    q.drift.every((d) => d.got.includes('nested')),
    describeDrift(q.drift),
  );
}

// --------------------------------------------------- servers we still accept

console.log('parseHello — a pre-rooms server is not drift');
{
  const old = { t: 'hello', you: 3, elements: [], probes: [], panels: [] };
  const q = parseHello(old);
  check('room is null', q.room === null);
  check('no drift', q.drift.length === 0, describeDrift(q.drift));
  check('you survives', q.you === 3);
}

console.log('parseHello — a room with no opinion about the view');
{
  const bare = {
    t: 'hello',
    you: 1,
    elements: [],
    room: { id: 'ABC123', name: 'Sandbox', template: 'sandbox', players: 1 },
    machine: false,
    view: { scopes: [] },
  };
  const q = parseHello(bare);
  check('no drift', q.drift.length === 0, describeDrift(q.drift));
  check('home absent, not invented', q.room?.view?.home === undefined);
  check('scopes empty', q.room?.view?.scopes?.length === 0);
  check('no machine, no goal card', q.room?.machine === false);
}

console.log('parseHello — a hand-edited template cannot poison the client');
{
  const junk = {
    t: 'hello',
    you: 'seven',
    elements: 'not a list',
    probes: [],
    panels: [],
    room: { id: 'ZZ0000', name: 5, template: 'x', players: 'many' },
    machine: 'yes',
    view: { home: [1, 2, 3], scopes: [{ x: 1 }, 7, null, 'nope'] },
  };
  const q = parseHello(junk);
  check('you defaults', q.you === 0);
  check('elements defaults to empty', q.elements.length === 0);
  check('name defaults to empty', q.room?.name === '');
  check('players defaults to 0', q.room?.players === 0);
  check('machine defaults to false', q.room?.machine === false);
  check('a 3-long home is refused', q.room?.view?.home === undefined);
  check('only object seeds survive', q.room?.view?.scopes?.length === 1);
  const fields = q.drift.map((d) => d.field).sort();
  check(
    'every one of them is named',
    eq(fields, [
      'hello.elements',
      'hello.machine',
      'hello.room.name',
      'hello.room.players',
      'hello.view.home',
      'hello.you',
    ]),
    fields.join(','),
  );
}

console.log('parseHello — garbage in, no throw');
for (const bad of [null, 42, 'hello', [], undefined]) {
  const q = parseHello(bad);
  check(`${JSON.stringify(bad) ?? 'undefined'} yields an empty hello with drift`, q.drift.length > 0 && q.room === null);
}

// ============================================================ THE JOIN ITSELF
//
// Everything above tests a pure function. The bug was not in a pure function.
//
// `connect()` owns the socket, the message switch and the handler call, and
// the shipped defect lived in exactly one line of it: `case 'hello'` forwarded
// the raw `m.room` — {id, name, template, players} and nothing else — as the
// client's `RoomHello`, while `machine` and `view` sat untouched beside it.
// `parseHello` can be perfect and that line can still throw its result away.
//
// So: run the real `connect()` against a stub WebSocket, feed it a real
// `hello` frame, and assert on the object that came out the other end.

class StubSocket {
  static OPEN = 1;
  readyState = StubSocket.OPEN;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  sent: string[] = [];
  closed = false;
  constructor(public url: string) {
    sockets.push(this);
  }
  send(s: string) {
    this.sent.push(s);
  }
  close() {
    this.closed = true;
    this.readyState = 3;
  }
}
let sockets: StubSocket[] = [];

// The browser globals `connect()` reads. Defined, not assigned: node ships its
// own `WebSocket` these days and a plain assignment would be at its mercy.
const def = (name: string, value: unknown) =>
  Object.defineProperty(globalThis, name, { value, writable: true, configurable: true });
def('WebSocket', StubSocket);
def('location', { protocol: 'https:', host: 'game.example' });
def('window', { setTimeout: () => 0, clearTimeout: () => {} });

/** Exactly the five arguments `onHello` is handed — the client's whole idea of
 * the room it just joined. */
interface HelloCall {
  you: number;
  elements: ElementSpec[];
  probes: Probe[];
  panels: Panel[];
  room: RoomHello | null;
}

/** A `connect()` wired to recording handlers, plus its stub socket. */
function dial(code: string | null = null) {
  sockets = [];
  const hellos: HelloCall[] = [];
  const drifts: WireDrift[][] = [];
  const nop = () => {};
  const handlers: NetHandlers = {
    onHello: (you, elements, probes, panels, room) =>
      hellos.push({ you, elements, probes, panels, room }),
    onRoomMeta: nop,
    onRoomGone: nop,
    onFrame: nop,
    onOp: nop,
    onDoc: nop,
    onProbes: nop,
    onPanels: nop,
    onMachine: nop,
    onDamage: nop,
    onSamples: nop,
    onAudio: nop,
    onPresence: nop,
    onCursor: nop,
    onClose: nop,
    onReject: nop,
    onWireDrift: (d) => drifts.push(d),
  };
  const net = connect(handlers, code);
  const deliver = (msg: unknown) => {
    const s = sockets[sockets.length - 1];
    if (!s || !s.onmessage) throw new Error('connect() opened no socket');
    s.onmessage({ data: JSON.stringify(msg) });
  };
  return { net, hellos, drifts, deliver, socket: () => sockets[sockets.length - 1] };
}

console.log('connect — the room object onHello RECEIVES is the parsed one');
{
  const d = dial();
  d.deliver(sample);
  check('onHello fired exactly once', d.hellos.length === 1);
  const got = d.hellos[0];
  const room = got?.room ?? null;
  check('you', got?.you === 1);
  check('elements', got?.elements.length === 4);
  check('probes', got?.probes.length === 2);
  check('panels', got?.panels.length === 1);
  check('room.id', room?.id === '7AWF4N');
  check('room.name', room?.name === 'Hoist practice');
  check('room.template', room?.template === 'hoist');
  check('room.players', room?.players === 1);
  // The three fields the shipped bug dropped. Each of these is a line that
  // goes red the moment `case 'hello'` forwards the wire's `room` again.
  check('room.machine — the goal card arrives', room?.machine === true);
  check(
    'room.view.home — the camera arrives',
    eq(room?.view?.home, [22, -2, 68, 28]),
    JSON.stringify(room?.view?.home),
  );
  check(
    'room.view.scopes — the instrument arrives',
    room?.view?.scopes?.length === 1 && room.view.scopes[0]?.timebase === 0.5,
    JSON.stringify(room?.view?.scopes),
  );
  // And nothing MORE than RoomHello: the raw `m.room` is a different object
  // with a different set of keys, and so is the whole message. Naming the
  // exact key set is what makes "it happens to have an id" not enough.
  check(
    'room is a RoomHello, key for key',
    eq(Object.keys(room ?? {}).sort(), [
      'id',
      'machine',
      'name',
      'players',
      'template',
      'view',
    ]),
    Object.keys(room ?? {}).sort().join(','),
  );
  check('a clean payload is not drift', d.drifts.length === 0);
  // The reconnect loop must come back to the room we landed in, not to the
  // null we dialled — an invite link is followed once.
  check('the socket now wants this room', d.net.code() === '7AWF4N', String(d.net.code()));
}

console.log('connect — a payload the client cannot read is reported to the app');
{
  const d = dial();
  const stripped = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  delete stripped.view;
  delete stripped.machine;
  d.deliver(stripped);
  const room = d.hellos[0]?.room ?? null;
  check('the room still lands', room?.id === '7AWF4N');
  check('no goal card is latched', room?.machine === false);
  check('no camera is invented', room?.view === null);
  check('onWireDrift fired', d.drifts.length === 1);
  check(
    'and it named both fields',
    eq((d.drifts[0] ?? []).map((x) => x.field).sort(), ['hello.machine', 'hello.view']),
    JSON.stringify(d.drifts[0]),
  );
}

console.log('connect — a pre-rooms server still joins');
{
  const d = dial();
  d.deliver({ t: 'hello', you: 3, elements: [], probes: [], panels: [] });
  check('room is null, not fabricated', d.hellos[0]?.room === null);
  check('no drift', d.drifts.length === 0);
  check('and the retry loop keeps aiming at the default room', d.net.code() === null);
}

console.log('connect — the code in the URL is the room you get');
{
  const d = dial('AB12CD');
  check(
    'the socket asks for it',
    d.socket()?.url === 'wss://game.example/ws?room=AB12CD',
    String(d.socket()?.url),
  );
  d.net.join(null);
  check('and join(null) drops it', d.socket()?.url === 'wss://game.example/ws', String(d.socket()?.url));
  check('the old socket was closed', sockets[0]?.closed === true);
}

// Walking next door is the case one hello can never test. A client that
// caches the first room it ever saw passes every single-hello assertion above
// and still hands the player the previous room's chip, goal card and camera
// forever. So: two different hellos down one `connect()`, and the second wins.
console.log('connect — the SECOND room replaces the first');
{
  const d = dial();
  d.deliver(sample);
  const next = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  const room = next.room as Record<string, unknown>;
  room.id = 'BBBBBB';
  room.name = 'Second room';
  room.players = 4;
  next.machine = false;
  (next.view as Record<string, unknown>).home = [100, 100, 140, 130];
  d.net.join('BBBBBB');
  d.deliver(next);

  check('onHello fired twice', d.hellos.length === 2, String(d.hellos.length));
  const got = d.hellos[d.hellos.length - 1]?.room ?? null;
  check('room.id is the new room', got?.id === 'BBBBBB', String(got?.id));
  check('room.name is the new room', got?.name === 'Second room', String(got?.name));
  check('room.players followed', got?.players === 4, String(got?.players));
  // These three are the ones a cached room silently keeps from next door.
  check('room.machine followed', got?.machine === false, String(got?.machine));
  check(
    'room.view.home followed',
    eq(got?.view?.home, [100, 100, 140, 130]),
    JSON.stringify(got?.view?.home),
  );
  check(
    'the first room was not retained',
    got?.id !== '7AWF4N' && got?.name !== 'Hoist practice',
    `${got?.id}/${got?.name}`,
  );
}

console.log(failures === 0 ? '\nwirecheck: all ok' : `\nwirecheck: ${failures} FAILED`);
if (failures > 0) process.exitCode = 1;
