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
import type { Layer } from './layer';
import type { Probe, WireScope } from './scope';
import type { LabelBox, NetLabel } from './annotate';

// This file runs under node (see package.json), never in the browser, and the
// repo carries no @types/node — one honest declaration beats a dependency.
declare function require(m: string): {
  readFileSync(p: string, enc: string): string;
  readdirSync(p: string): string[];
  statSync(p: string): { isDirectory(): boolean };
};
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

/** Exactly the six arguments `onHello` is handed — the client's whole idea of
 * the room it just joined. */
/** Exactly the arguments `onHello` is handed — the client's whole idea of the
 * room it just joined. */
interface HelloCall {
  you: number;
  elements: ElementSpec[];
  probes: Probe[];
  panels: Panel[];
  scopes: WireScope[];
  labelBoxes: LabelBox[];
  netLabels: NetLabel[];
  room: RoomHello | null;
}

/** A `connect()` wired to recording handlers, plus its stub socket. */
function dial(code: string | null = null) {
  sockets = [];
  const hellos: HelloCall[] = [];
  const drifts: WireDrift[][] = [];
  const layerCalls: { list: Layer[]; claims: [number, number][] }[] = [];
  const boxCalls: LabelBox[][] = [];
  const netLabelCalls: NetLabel[][] = [];
  const netMapCalls: { live: number[]; probe: [number, number][] }[] = [];
  const sensorCalls: [number, number][][] = [];
  const nop = () => {};
  const handlers: NetHandlers = {
    onHello: (you, elements, probes, panels, scopes, labelBoxes, netLabels, room) =>
      hellos.push({ you, elements, probes, panels, scopes, labelBoxes, netLabels, room }),
    onRoomMeta: nop,
    onRoomGone: nop,
    onFrame: nop,
    onOp: nop,
    onDoc: nop,
    onProbes: nop,
    onPanels: nop,
    onScopes: nop,
    onLabelBoxes: (list) => boxCalls.push(list),
    onNetLabels: (list) => netLabelCalls.push(list),
    onNetMap: (live, probe) => netMapCalls.push({ live, probe }),
    onLayers: (list, cl) => layerCalls.push({ list, claims: cl }),
    onSensors: (list) => sensorCalls.push(list),
    onMachine: nop,
    onDamage: nop,
    onSamples: nop,
    onAudio: nop,
    onPresence: nop,
    onCursor: nop,
    onChat: nop,
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
  return {
    net,
    hellos,
    drifts,
    layerCalls,
    boxCalls,
    netLabelCalls,
    netMapCalls,
    sensorCalls,
    deliver,
    socket: () => sockets[sockets.length - 1],
  };
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
  // The room's INSTRUMENTS, beside its panels. Room state — the seed under
  // `view.scopes` is what the template offered, this is what the room has.
  check(
    'scopes — the bench arrives as room state',
    got?.scopes.length === 1 && got.scopes[0]?.set?.timebase === 0.5,
    JSON.stringify(got?.scopes),
  );
  check('scopes carry a server-minted sid', got?.scopes[0]?.sid === 1);
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
  check('and it brought no annotation, quietly', d.hellos[0]?.labelBoxes.length === 0);
  check('nor any net labels', d.hellos[0]?.netLabels.length === 0);
  check('and the retry loop keeps aiming at the default room', d.net.code() === null);
}

// ANNOTATION ON THE WIRE. Both primitives are room state, so both have to
// arrive on the hello AND on their own broadcast, through the same handler —
// a late joiner and a running client must never disagree about what the sheet
// says. The `netmap` is the third message and the only DERIVED one: which net
// each label is on, computed server-side because the client has no netlist.
console.log('annotate — label boxes and net labels reach the client');
{
  const d = dial();
  const withAnn = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  withAnn.labelboxes = [{ blid: 1, x0: 2, y0: 3, x1: 12, y1: 9, name: 'VCO 1V/OCT' }];
  withAnn.netlabels = [{ nlid: 1, x: 7, y: 4, name: '5V RAIL' }];
  d.deliver(withAnn);
  check('hello carried the label box', d.hellos[0]?.labelBoxes.length === 1);
  check('with its title', d.hellos[0]?.labelBoxes[0]?.name === 'VCO 1V/OCT');
  check('hello carried the net label', d.hellos[0]?.netLabels.length === 1);
  // The anchor is a POINT — integers, the same lattice pins live in. A
  // fractional anchor could never intern-match a junction.
  check('anchored to a grid point', d.hellos[0]?.netLabels[0]?.x === 7);
  check('no drift', d.drifts.length === 0, describeDrift(d.drifts[0] ?? []));

  d.deliver({ t: 'labelboxes', list: [{ blid: 2, x0: 0, y0: 0, x1: 4, y1: 4, name: 'PSU' }] });
  check('the broadcast uses the same shape', d.boxCalls.length === 1);
  check('whole list, never a delta', d.boxCalls[0]?.length === 1);
  d.deliver({ t: 'netlabels', list: [{ nlid: 2, x: 1, y: 1, name: 'GND' }] });
  check('net labels broadcast too', d.netLabelCalls[0]?.[0]?.name === 'GND');

  d.deliver({ t: 'netmap', live: [1, 2], probes: [[1, 2]] });
  check('netmap says which labels are attached', eq(d.netMapCalls[0]?.live, [1, 2]));
  check('and which net each probe is on', eq(d.netMapCalls[0]?.probe, [[1, 2]]));
}

console.log('annotate — the client cannot put a fractional anchor on the wire');
{
  const d = dial();
  d.deliver(sample);
  const before = d.socket()?.sent.length ?? 0;
  d.net.sendNetLabel({ t: 'add', x: 3.7, y: -2.2, name: 'BUS' });
  const sent = JSON.parse(d.socket()?.sent[before] ?? '{}') as {
    t: string;
    op: { x: number; y: number };
  };
  check('it is a netlabel message', sent.t === 'netlabel');
  check('x is rounded to the grid', Number.isInteger(sent.op.x), String(sent.op.x));
  check('y is rounded to the grid', Number.isInteger(sent.op.y), String(sent.op.y));
  // A rename carries no coordinates at all and must not grow any.
  d.net.sendNetLabel({ t: 'rename', nlid: 1, name: 'BUS' });
  const ren = JSON.parse(d.socket()?.sent[before + 1] ?? '{}') as { op: Record<string, unknown> };
  check('a rename is nlid + name and nothing else', eq(Object.keys(ren.op).sort(), ['name', 'nlid', 't']));
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

// ------------------------------------------------- external inputs (privacy)
//
// TEST AT THE LAYER THE DEFECT CAN OCCUR. The failure this whole feature has
// to be incapable of is "a frame, a buffer or a device identity ends up on
// the socket". A reviewer reading the code cannot prove that stays true after
// the next edit; these two checks can.
//
// The first drives the REAL `connect()` with the real sender and asserts on
// the bytes that reach the socket. The second is a static scan of the whole
// client for the APIs that could turn media into something transmissible —
// currently zero hits, and this pins it at zero.

console.log('sensor — the only thing a camera puts on the wire is [[int,int]]');
{
  const d = dial();
  d.deliver(sample);
  const before = d.socket()?.sent.length ?? 0;
  // Deliberately hostile input: fractional, out of range, negative, huge.
  d.net.sendSensor([
    [42, 20841.7],
    [57, -5],
    [58, 999999],
  ]);
  const sent = (d.socket()?.sent ?? []).slice(before);
  check('exactly one message', sent.length === 1, String(sent.length));
  const m = JSON.parse(sent[0] ?? '{}') as Record<string, unknown>;
  check('t is "sensor"', m.t === 'sensor');
  check('it has exactly two keys', Object.keys(m).sort().join(',') === 's,t', Object.keys(m).join(','));
  const list = m.s as unknown[];
  check('s is a list of pairs', Array.isArray(list) && list.length === 3);
  let shapeOk = true;
  let rangeOk = true;
  for (const row of list) {
    const pair = row as unknown[];
    if (!Array.isArray(pair) || pair.length !== 2) shapeOk = false;
    else {
      for (const n of pair) {
        if (typeof n !== 'number' || !Number.isInteger(n)) shapeOk = false;
      }
      const q = pair[1] as number;
      if (q < 0 || q > 65535) rangeOk = false;
    }
  }
  check('every entry is [int, int]', shapeOk, JSON.stringify(list));
  check('every q is inside 0..65535', rangeOk, JSON.stringify(list));
  check('fractions are rounded, not printed', eq(list[0], [42, 20842]), JSON.stringify(list[0]));
  check('negatives clamp to dark', eq(list[1], [57, 0]), JSON.stringify(list[1]));
  check('overflow clamps to full scale', eq(list[2], [58, 65535]), JSON.stringify(list[2]));
  // The size of the promise, stated as a number: a frame is ~100 kB and this
  // is not that. Twelve-ish bytes per moved sensor per tick.
  check('the whole message is tiny', (sent[0] ?? '').length < 80, String((sent[0] ?? '').length));
  // And an empty batch sends nothing at all — a camera pointed at a still
  // wall is silent on the wire.
  const quiet = d.socket()?.sent.length ?? 0;
  d.net.sendSensor([]);
  check('an empty batch is not a message', (d.socket()?.sent.length ?? 0) === quiet);
}

console.log('sensor — layers arrive through one handler, hello and broadcast alike');
{
  const d = dial();
  const withLayers = JSON.parse(JSON.stringify(sample)) as Record<string, unknown>;
  withLayers.layers = [{ lid: 1, x0: 0, y0: 0, x1: 20, y1: 15, name: 'CAMERA 1' }];
  withLayers.claims = [[1, 7]];
  d.deliver(withLayers);
  check('hello delivered the layers', d.layerCalls[0]?.list.length === 1);
  check('and who is driving them', eq(d.layerCalls[0]?.claims, [[1, 7]]));
  // A hello with no layers is not an error: every save written before this
  // feature existed has none.
  const d2 = dial();
  d2.deliver(sample);
  check('an older room has no layers and no drift', d2.layerCalls[0]?.list.length === 0);
  check('and it is still a clean payload', d2.drifts.length === 0);
  // The live broadcast lands in the same place.
  d2.deliver({ t: 'layers', list: [{ lid: 4, x0: 1, y0: 1, x1: 9, y1: 7, name: 'X' }], claims: [] });
  check('the broadcast uses the same handler', d2.layerCalls.length === 2);
  d2.deliver({ t: 'sensors', s: [[42, 32768]] });
  check('a reading reaches onSensors', eq(d2.sensorCalls[0], [[42, 32768]]));
}

console.log('privacy — the client has no way to transmit media, and cannot grow one');
{
  // Media only becomes transmissible through a small, nameable set of APIs.
  // None of them appear anywhere in this client, and this is the guard that
  // notices the day one does.
  const banned = [
    'RTCPeerConnection',
    'MediaRecorder',
    'getDisplayMedia',
    'toDataURL',
    'toBlob',
    'FileReader',
  ];
  const fs = require('fs');
  // WALK THE DIRECTORY — never a hand-written list. This scan is the guard
  // that camera pixels never leave the machine, and it was naming 18 files
  // while `src/` held 25. The two it missed were `layer.ts`, the file that
  // actually draws camera frames onto the shared canvas, and `store.ts`. A
  // `toDataURL` added in exactly the file that has the pixels would have
  // sailed past the check written to catch it. A guard with a manual
  // inventory silently stops guarding the moment someone adds a file.
  const srcDir = ['src/', 'packages/app/src/'].find((d) => {
    try {
      return fs.statSync(d).isDirectory();
    } catch {
      return false;
    }
  });
  // The ONE exclusion, and it is not a shipped file: this checker is compiled
  // by `tsc` and run under node, never bundled and never loaded by a browser,
  // so the banned names in its own `banned` array are data, not call sites.
  // Every file that CAN reach a browser is scanned.
  const files: string[] = srcDir
    ? fs
        .readdirSync(srcDir)
        .filter((f: string) => f.endsWith('.ts') && !f.endsWith('.d.ts') && f !== 'wirecheck.ts')
    : [];
  if (files.length < 20) throw new Error(`wirecheck: only found ${files.length} sources — refusing to certify a scan that may have missed files`);
  const hits: string[] = [];
  let read = 0;
  for (const f of files) {
    let src = '';
    for (const base of ['src/', 'packages/app/src/']) {
      try {
        src = fs.readFileSync(base + f, 'utf8');
        break;
      } catch {
        /* try the next root */
      }
    }
    if (!src) continue;
    read++;
    for (const b of banned) {
      // Skip the comment lines that NAME the ban (this file's own prose, and
      // sensor.ts's header): a mention is not a call site.
      for (const line of src.split('\n')) {
        const t = line.trim();
        if (t.startsWith('//') || t.startsWith('*') || t.startsWith('/*')) continue;
        if (line.includes(b)) hits.push(`${f}: ${t.slice(0, 60)}`);
      }
    }
  }
  check('every client source was scanned', read >= 15, `${read} files`);
  check('no media-egress API anywhere in the client', hits.length === 0, hits.join(' | '));

  // getUserMedia exists exactly once, in the sampler, and only inside the
  // click that claims a layer — never at module scope.
  let sensorSrc = '';
  for (const base of ['src/', 'packages/app/src/']) {
    try {
      sensorSrc = fs.readFileSync(base + 'sensor.ts', 'utf8');
      break;
    } catch {
      /* try the next root */
    }
  }
  const calls = (sensorSrc.match(/getUserMedia\(/g) ?? []).length;
  check('exactly one getUserMedia call site', calls === 1, String(calls));
  // A GUARD THAT CANNOT FAIL IS NOT A GUARD. This used to read
  // `indexOf('async start(') < indexOf('getUserMedia(')`, and the day
  // `async start()` became `start()` + `private async begin()` the left side
  // turned into -1 — which is less than every index there is, so the
  // assertion passed no matter where the call had moved, including module
  // scope. Anchor on something that must exist, and PROVE it must exist.
  const opener = /\n  (?:private )?(?:async )?(start|begin)\(/.exec(sensorSrc);
  check('the camera opener is a method, and findable', opener !== null, String(opener?.[1]));
  const openerAt = opener ? opener.index : -1;
  check(
    'and getUserMedia is inside it, not at module scope',
    openerAt >= 0 && openerAt < sensorSrc.indexOf('getUserMedia('),
  );
  // The call is reached only from a method, never from the top level: no
  // line that calls it may start at column zero.
  check(
    'no getUserMedia call at top-level indentation',
    !/^getUserMedia\(|^\s{0,3}(?:await )?navigator\.mediaDevices/m.test(sensorSrc),
  );
  // Comment-aware, like the scan above: sensor.ts's own prose says "never
  // `enabled = false`", and a mention is not a call site.
  const sensorCode = sensorSrc
    .split('\n')
    .filter((l) => {
      const t = l.trim();
      return !(t.startsWith('//') || t.startsWith('*') || t.startsWith('/*'));
    })
    .join('\n');
  check(
    'the sampler stops the hardware, not just the frames',
    sensorCode.includes('.stop()') && !sensorCode.includes('enabled = false'),
  );
  check('enumerateDevices is never called', !sensorSrc.includes('enumerateDevices'));
}

console.log(failures === 0 ? '\nwirecheck: all ok' : `\nwirecheck: ${failures} FAILED`);
if (failures > 0) process.exitCode = 1;
