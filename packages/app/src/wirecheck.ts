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
// What it CANNOT prove: that the camera lands somewhere a player wants to be,
// or that the seeded scope is pointing at an interesting node. That needs a
// browser and a human.

import { describeDrift, parseHello, type ParsedHello } from './net';

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

console.log(failures === 0 ? '\nwirecheck: all ok' : `\nwirecheck: ${failures} FAILED`);
if (failures > 0) process.exitCode = 1;
