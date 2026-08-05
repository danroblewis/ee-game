// Dev-only headless check for per-client undo/redo. NOT shipped: nothing
// imports it, so the bundle never sees it.
//
//   pnpm --filter @ee/app historycheck
//
// history.ts is pure logic over a document, so it can be driven without a
// browser — and it had to grow a second KIND of entry when the freight hoist
// became draggable (a machine move is not a DocOp: the server owns those
// fixture ids and refuses document edits on them). This asserts both halves:
//
//   * every pre-existing behaviour — inverse ops, one entry per gesture, Move
//     coalescing inside a gesture, Remove undone as a re-Add with the original
//     id, a stale entry skipped rather than silently eating the keystroke,
//     the redo branch invalidated by a new edit;
//   * the new `pushAction` — one drag gesture is one undo step, its thunks run
//     exactly once each, it interleaves with op entries in player order, and
//     an edit issued from inside a thunk is not recorded as a fresh entry.
//
// What it CANNOT prove: that dragging the hoist FEELS like moving a part, or
// that its package reads as one grabbable object. That needs a browser and a
// human.

import { History } from './history';
import type { DocOp, ElementSpec, Point } from './circuit';

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

/** A stand-in for main.ts's document + editDoc pair. */
function doc(initial: ElementSpec[] = []) {
  const elements: ElementSpec[] = JSON.parse(JSON.stringify(initial)) as ElementSpec[];
  const applied: DocOp[] = [];
  const apply = (op: DocOp) => {
    applied.push(op);
    if (op.t === 'Add') elements.push(op.spec);
    else if (op.t === 'Remove') {
      const k = elements.findIndex((e) => e.id === op.id);
      if (k >= 0) elements.splice(k, 1);
    } else if (op.t === 'Move') {
      const e = elements.find((x) => x.id === op.id);
      if (e) e.pins = op.pins;
    } else if (op.t === 'SetKind') {
      const e = elements.find((x) => x.id === op.id);
      if (e) e.kind = op.kind;
    }
  };
  const history = new History(apply);
  /** The local-edit path: record, then apply (exactly main.ts's editDoc). */
  const edit = (op: DocOp) => {
    history.record(op, elements);
    apply(op);
  };
  return { elements, applied, history, edit };
}

const wire = (id: number, a: Point, b: Point): ElementSpec => ({
  id,
  kind: { t: 'Wire' },
  pins: [a, b],
});
const pinsOf = (elements: ElementSpec[], id: number) =>
  JSON.stringify(elements.find((e) => e.id === id)?.pins ?? null);

// ------------------------------------------- existing behaviour: inverse ops
console.log('\ndocument edits still undo as inverse ops');
{
  const d = doc([wire(1, [0, 0], [4, 0])]);
  d.edit({ t: 'Move', id: 1, pins: [[2, 2], [6, 2]] });
  check('the move applied', pinsOf(d.elements, 1) === '[[2,2],[6,2]]');
  d.history.undo(d.elements);
  check('undo restores the pre-move pins', pinsOf(d.elements, 1) === '[[0,0],[4,0]]');
  d.history.redo(d.elements);
  check('redo re-applies it', pinsOf(d.elements, 1) === '[[2,2],[6,2]]');

  d.edit({ t: 'Remove', id: 1 });
  check('the remove applied', d.elements.length === 0);
  d.history.undo(d.elements);
  check('undo re-Adds it with its original id', pinsOf(d.elements, 1) === '[[2,2],[6,2]]');

  d.edit({ t: 'Add', spec: wire(2, [9, 9], [9, 12]) });
  d.history.undo(d.elements);
  check('undo of an Add removes it', !d.elements.some((e) => e.id === 2));
}

console.log('\none gesture is still one undo entry');
{
  const d = doc([wire(1, [0, 0], [4, 0]), wire(2, [0, 4], [4, 4])]);
  d.history.begin([d.elements[0], d.elements[1]], 'move 2 parts');
  // A drag emits a Move every ~60 ms; they must coalesce, not stack up.
  for (let k = 1; k <= 3; k++) {
    d.edit({ t: 'Move', id: 1, pins: [[k, 0], [4 + k, 0]] });
    d.edit({ t: 'Move', id: 2, pins: [[k, 4], [4 + k, 4]] });
  }
  d.history.end();
  d.history.undo(d.elements);
  check(
    'one ⌘Z takes the whole group drag back',
    pinsOf(d.elements, 1) === '[[0,0],[4,0]]' && pinsOf(d.elements, 2) === '[[0,4],[4,4]]',
  );
  check('and it was labelled', d.history.note() === 'undo: move 2 parts', d.history.note());
  d.history.undo(d.elements);
  check('nothing else was recorded', d.history.note() === 'nothing to undo', d.history.note());
}

// ----------------------------------------- new: a gesture thrown away whole
console.log('\nan aborted gesture leaves nothing on the undo stack');
{
  const d = doc([wire(1, [0, 0], [4, 0])]);
  // A one-finger part drag, interrupted by a second finger landing: the
  // gesture's throttled Moves went out, then main.ts put the pins back with a
  // compensating Move and aborted. ⌘Z must find the edit BEFORE the drag, not
  // an entry that appears to do nothing.
  d.edit({ t: 'Move', id: 1, pins: [[9, 9], [13, 9]] }); // an earlier, real edit
  d.history.begin([d.elements[0]], 'move part');
  d.edit({ t: 'Move', id: 1, pins: [[10, 9], [14, 9]] });
  d.edit({ t: 'Move', id: 1, pins: [[11, 9], [15, 9]] });
  d.edit({ t: 'Move', id: 1, pins: [[9, 9], [13, 9]] }); // the rollback itself
  d.history.abort();
  check('the rollback landed', pinsOf(d.elements, 1) === '[[9,9],[13,9]]');
  d.history.undo(d.elements);
  check(
    'one ⌘Z reaches past the aborted gesture to the edit before it',
    pinsOf(d.elements, 1) === '[[0,0],[4,0]]',
    `${pinsOf(d.elements, 1)} / ${d.history.note()}`,
  );
  d.history.undo(d.elements);
  check('and there was only ever the one entry', d.history.note() === 'nothing to undo',
    d.history.note());
}

console.log('\na stale entry is skipped, not swallowed');
{
  const d = doc([wire(1, [0, 0], [4, 0]), wire(2, [0, 4], [4, 4])]);
  d.edit({ t: 'Move', id: 1, pins: [[1, 0], [5, 0]] }); // entry A
  d.edit({ t: 'Move', id: 2, pins: [[1, 4], [5, 4]] }); // entry B
  // A peer deletes element 2 out from under us: entry B cannot be replayed.
  const k = d.elements.findIndex((e) => e.id === 2);
  d.elements.splice(k, 1);
  d.history.undo(d.elements);
  check(
    'the dead entry is dropped and the live one undoes instead',
    pinsOf(d.elements, 1) === '[[0,0],[4,0]]',
    pinsOf(d.elements, 1),
  );
}

// ------------------------------------------ new: a machine move as one entry
console.log('\na machine drag is one undoable action');
{
  const d = doc();
  // Stand in for the hoist: a footprint this "server" owns, moved only by the
  // assembly op (main.ts's moveMachineBy).
  let rect: [number, number] = [46, 2];
  let undos = 0;
  let redos = 0;
  const moveBy = (dx: number, dy: number) => {
    rect = [rect[0] + dx, rect[1] + dy];
  };
  // ONE gesture: the pointer moved 5 right and 3 down over many frames.
  moveBy(5, 3);
  d.history.pushAction({
    label: 'move machine',
    undo: () => {
      undos++;
      moveBy(-5, -3);
    },
    redo: () => {
      redos++;
      moveBy(5, 3);
    },
  });
  check('the drag moved the machine', rect[0] === 51 && rect[1] === 5, String(rect));

  d.history.undo(d.elements);
  check('⌘Z puts it back exactly', rect[0] === 46 && rect[1] === 2, String(rect));
  check('the inverse thunk ran once', undos === 1, `undos=${undos}`);
  check('labelled as a machine move', d.history.note() === 'undo: move machine', d.history.note());

  d.history.redo(d.elements);
  check('redo moves it again', rect[0] === 51 && rect[1] === 5, String(rect));
  check('the replay thunk ran once', redos === 1, `redos=${redos}`);

  d.history.undo(d.elements);
  d.history.undo(d.elements);
  check(
    'the machine entry is not an infinite well',
    d.history.note() === 'nothing to undo' && rect[0] === 46,
    `${d.history.note()} ${rect}`,
  );
}

console.log('\nmachine moves and part edits share one timeline');
{
  const d = doc([wire(1, [0, 0], [4, 0])]);
  let x = 46;
  d.edit({ t: 'Move', id: 1, pins: [[7, 0], [11, 0]] }); // player did this first
  x += 5; // ...then dragged the machine
  d.history.pushAction({
    label: 'move machine',
    undo: () => (x -= 5),
    redo: () => (x += 5),
  });

  d.history.undo(d.elements);
  check(
    'the first ⌘Z takes back the machine move (the newest edit)',
    x === 46 && pinsOf(d.elements, 1) === '[[7,0],[11,0]]',
    `x=${x} pins=${pinsOf(d.elements, 1)}`,
  );
  d.history.undo(d.elements);
  check(
    'the second ⌘Z takes back the part move',
    x === 46 && pinsOf(d.elements, 1) === '[[0,0],[4,0]]',
    `x=${x} pins=${pinsOf(d.elements, 1)}`,
  );
  d.history.redo(d.elements);
  d.history.redo(d.elements);
  check(
    'redo walks forward through both kinds',
    x === 51 && pinsOf(d.elements, 1) === '[[7,0],[11,0]]',
    `x=${x} pins=${pinsOf(d.elements, 1)}`,
  );
}

console.log('\na new edit invalidates the redo branch (both kinds)');
{
  // A part edit after undoing a machine move kills the machine's redo.
  const d = doc([wire(1, [0, 0], [4, 0])]);
  let x = 46;
  x += 5;
  d.history.pushAction({ label: 'move machine', undo: () => (x -= 5), redo: () => (x += 5) });
  d.history.undo(d.elements); // machine back at 46, entry on the redo stack
  d.edit({ t: 'Move', id: 1, pins: [[7, 0], [11, 0]] }); // a fresh edit
  d.history.redo(d.elements);
  check(
    'the stale machine redo is gone and nothing moved',
    x === 46 && d.history.note() === 'nothing to redo',
    `x=${x} note=${d.history.note()}`,
  );

  // ...and the mirror case: a machine drag kills an op entry's redo branch.
  const e = doc([wire(1, [0, 0], [4, 0])]);
  e.edit({ t: 'Move', id: 1, pins: [[7, 0], [11, 0]] });
  e.history.undo(e.elements); // part move on the redo stack
  let moved = false;
  e.history.pushAction({
    label: 'move machine',
    undo: () => (moved = false),
    redo: () => (moved = true),
  });
  e.history.redo(e.elements);
  check(
    'the abandoned part-move redo is gone',
    e.history.note() === 'nothing to redo' && pinsOf(e.elements, 1) === '[[0,0],[4,0]]',
    `${e.history.note()} ${pinsOf(e.elements, 1)}`,
  );
  e.history.undo(e.elements);
  check(
    'and the machine drag is what ⌘Z now takes back',
    e.history.note() === 'undo: move machine' && !moved,
    e.history.note(),
  );
}

console.log('\nan action thunk cannot record itself');
{
  const d = doc([wire(1, [0, 0], [4, 0])]);
  // A thunk that edits the document (the machine op path also touches child
  // elements) must not push a NEW entry while it is replaying, or ⌘Z would
  // never converge.
  d.history.pushAction({
    label: 'move machine',
    undo: () => {
      d.edit({ t: 'Move', id: 1, pins: [[0, 0], [4, 0]] });
      d.history.pushAction({ label: 'nested', undo: () => {}, redo: () => {} });
    },
    redo: () => {},
  });
  d.history.undo(d.elements);
  d.history.undo(d.elements);
  check(
    'the replay guard held: no entry was created by the undo itself',
    d.history.note() === 'nothing to undo',
    d.history.note(),
  );
}

console.log(
  failures === 0
    ? '\nALL CHECKS PASSED (logic only — drag feel still needs a browser and a human)'
    : `\n${failures} CHECK(S) FAILED`,
);
// No @types/node in this package (the bench and audiocheck have the same
// constraint), so the exit code goes out through a narrowly typed handle.
(globalThis as unknown as { process: { exitCode: number } }).process.exitCode =
  failures === 0 ? 0 : 1;
