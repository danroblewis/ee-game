// Per-client undo/redo for the document edits THIS player made.
//
// The schematic is shared, so there is no global timeline to rewind: undoing
// another player's op would be indistinguishable from vandalism. We therefore
// record ONLY the ops that pass through main.ts's `editDoc` (the single local
// edit choke point) and never the ops arriving from the server in `onDoc`.
// "Undo" means "put back what I just did", replayed as a brand-new edit
// through that same `editDoc` path so the server validates it like any other.
//
// Conflict handling — by the time you hit ⌘Z, someone else may have moved or
// deleted the element. Every op is checked against the live element list
// first, using exactly the server's own acceptance rules (server/main.rs
// `apply_doc_op`: Add needs a FREE id, the others need an EXISTING id):
//   * Move/SetKind whose element is gone  -> that op is skipped. If every op
//     of the entry is skipped the entry is *dropped and we continue to the
//     next entry* rather than being silently consumed — a dead entry must not
//     burn a keystroke and make ⌘Z look broken.
//   * Add whose element is already gone   -> no-op (skipped).
//   * Remove is undone by re-Adding the captured spec WITH ITS ORIGINAL id:
//     the id is free again after the Remove, and the server rejects
//     duplicates, so reusing it is both required and safe.
// Because ops are pre-filtered with the server's rules, an undo can never be
// rejected server-side, which is what would leave the optimistic client
// desynced. Nothing here throws; dropped entries never enter the redo stack.

import type { DocOp, ElementSpec } from './circuit';

/** Undo depth cap (entries, not ops). */
const MAX_ENTRIES = 200;
/** How long the HUD note stays up. */
const NOTE_MS = 2200;
/** A gesture group left open this long (lost pointerup) is force-closed. */
const GROUP_MAX_MS = 30_000;

const now = () => performance.now();

/** Pins and kinds are mutated in place elsewhere, so every capture is deep. */
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

/** The element a DocOp targets. */
export function opTargetId(op: DocOp): number {
  return op.t === 'Add' ? op.spec.id : op.id;
}

/**
 * The op that undoes `op`, given the element as it was BEFORE `op` applied
 * (`null`/`undefined` = it did not exist). Pure; returns null when the op
 * cannot be inverted (e.g. a Remove whose prior spec was never captured).
 */
export function invertOp(op: DocOp, before: ElementSpec | null | undefined): DocOp | null {
  switch (op.t) {
    case 'Add':
      return { t: 'Remove', id: op.spec.id };
    case 'Remove':
      return before ? { t: 'Add', spec: clone(before) } : null;
    case 'Move':
      return before ? { t: 'Move', id: op.id, pins: clone(before.pins) } : null;
    case 'SetKind':
      return before ? { t: 'SetKind', id: op.id, kind: clone(before.kind) } : null;
  }
}

const VERB: Record<DocOp['t'], string> = {
  Add: 'add',
  Remove: 'delete',
  Move: 'move',
  SetKind: 'edit',
};

/** Human label for an entry, e.g. "move 3 parts". Pure. */
export function labelFor(ops: DocOp[]): string {
  const first = ops[0];
  if (!first) return 'edit';
  const verb = ops.every((o) => o.t === first.t) ? VERB[first.t] : 'edit';
  const n = new Set(ops.map(opTargetId)).size;
  return n > 1 ? `${verb} ${n} parts` : `${verb} part`;
}

/** True when a keystroke belongs to a text field (property dialog, panel
 * window, palette search) — undo keys must stay out of the way there. */
export function isTypingTarget(ev: KeyboardEvent): boolean {
  for (const n of [ev.target, document.activeElement]) {
    if (!(n instanceof HTMLElement)) continue;
    if (n.isContentEditable) return true;
    if (n.tagName === 'INPUT' || n.tagName === 'TEXTAREA' || n.tagName === 'SELECT') return true;
  }
  return false;
}

interface Entry {
  label: string;
  /** Forward ops, in application order. */
  redo: DocOp[];
  /** Inverse ops, already in application order (reverse of `redo`). */
  undo: DocOp[];
}

interface Group {
  label?: string;
  at: number;
  ops: DocOp[];
  /** Element state as of the START of the group; null = did not exist. */
  before: Map<number, ElementSpec | null>;
}

export class History {
  private undoStack: Entry[] = [];
  private redoStack: Entry[] = [];
  private group: Group | null = null;
  private replaying = false;
  private msg = '';
  private msgAt = -Infinity;

  /** `apply` must be main.ts's `editDoc` — undo replays are ordinary edits. */
  constructor(private readonly apply: (op: DocOp) => void) {}

  /**
   * Start a gesture: every op recorded until `end()` collapses into ONE undo
   * entry (drag-move, paste, group delete/rotate). `before` seeds the
   * pre-gesture specs, needed for drag-moves because the pins are mutated in
   * place long before the final Move reaches `editDoc`. Never nests: a new
   * group flushes the previous one, so a lost `end()` cannot leak.
   */
  begin(before?: (ElementSpec | undefined)[], label?: string): void {
    this.end();
    const g: Group = { label, at: now(), ops: [], before: new Map() };
    for (const s of before ?? []) if (s) g.before.set(s.id, clone(s));
    this.group = g;
  }

  /** Close the current gesture, committing it as one entry. */
  end(): void {
    const g = this.group;
    this.group = null;
    if (g) this.commit(g);
  }

  /** Record a locally issued op. Call from `editDoc`, before applying it. */
  record(op: DocOp, elements: ElementSpec[]): void {
    if (this.replaying) return;
    if (this.group && now() - this.group.at > GROUP_MAX_MS) this.end();
    const g: Group = this.group ?? { at: now(), ops: [], before: new Map() };
    const copy = clone(op);
    const id = opTargetId(copy);
    if (!g.before.has(id)) g.before.set(id, clone(elements.find((e) => e.id === id) ?? null));
    // Coalesce repeats within one gesture: a drag emits a Move every ~60 ms.
    const k =
      copy.t === 'Move' || copy.t === 'SetKind'
        ? g.ops.findIndex((o) => o.t === copy.t && opTargetId(o) === id)
        : -1;
    if (k >= 0) g.ops[k] = copy;
    else g.ops.push(copy);
    this.redoStack.length = 0; // a new edit invalidates the redo branch
    if (!this.group) this.commit(g);
  }

  undo(elements: ElementSpec[]): void {
    this.end();
    while (this.undoStack.length > 0) {
      const entry = this.undoStack.pop()!;
      const ops = entry.undo.filter((op) => viable(op, elements));
      if (ops.length === 0) continue; // stale: nothing of it survives
      this.run(ops);
      this.redoStack.push(entry);
      this.flash(`undo: ${entry.label}`);
      return;
    }
    this.flash('nothing to undo');
  }

  redo(elements: ElementSpec[]): void {
    this.end();
    while (this.redoStack.length > 0) {
      const entry = this.redoStack.pop()!;
      const ops = entry.redo.filter((op) => viable(op, elements));
      if (ops.length === 0) continue;
      this.run(ops);
      this.undoStack.push(entry);
      this.flash(`redo: ${entry.label}`);
      return;
    }
    this.flash('nothing to redo');
  }

  /** Transient HUD text ('' once it expires). */
  note(): string {
    return now() - this.msgAt < NOTE_MS ? this.msg : '';
  }

  private commit(g: Group): void {
    const undo: DocOp[] = [];
    for (const op of g.ops) {
      const inv = invertOp(op, g.before.get(opTargetId(op)));
      if (inv) undo.push(inv);
    }
    if (undo.length === 0) return;
    undo.reverse();
    this.undoStack.push({ label: g.label ?? labelFor(g.ops), redo: g.ops, undo });
    if (this.undoStack.length > MAX_ENTRIES) this.undoStack.shift();
  }

  private run(ops: DocOp[]): void {
    this.replaying = true;
    try {
      // Clone on the way out too: a re-Added spec is pushed into `elements`
      // by reference and would then be mutated in place by the next drag.
      for (const op of ops) this.apply(clone(op));
    } finally {
      this.replaying = false;
    }
  }

  private flash(msg: string): void {
    this.msg = msg;
    this.msgAt = now();
  }
}

/** Server acceptance rules (server/main.rs `apply_doc_op`), mirrored. */
function viable(op: DocOp, elements: ElementSpec[]): boolean {
  const id = opTargetId(op);
  const exists = elements.some((e) => e.id === id);
  return op.t === 'Add' ? !exists : exists;
}
