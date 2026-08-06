//! PARTS ON THE SYSTEM CLIPBOARD, as text.
//!
//! The in-memory clipboard only ever existed inside one tab, so copying a
//! block out of one room and pasting it into another — two windows, the
//! obvious thing to want — did nothing at all. Text is what crosses that
//! gap: the OS clipboard carries it between tabs, between browsers, and out
//! into a chat message or a gist, which means a circuit becomes something
//! you can send someone.
//!
//! THE FORMAT IS DELIBERATELY BORING. One JSON object, a magic string, a
//! version, and the same `{kind, pins, tier, rot}` the internal clipboard
//! already held, with pins already centred on the selection's centroid.
//! Nothing here knows about the netlist; a paste still goes through the
//! ordinary `Add` path and the ordinary placement gate, which is what
//! decides whether the document that would result is legal.
//!
//! WHAT ARRIVES IS UNTRUSTED. It came off the system clipboard: it may be a
//! shopping list, a half-copied string, or a hand-edited file from someone
//! who thought a 900-pin resistor would be funny. Everything is checked
//! structurally here — shape, finiteness, integer grid points, a sane count
//! — and everything that survives is still put through the same gate a
//! hand-drawn part faces. Bad input returns null and the paste falls back to
//! whatever was in memory; it never throws into a keyboard handler.

import type { ElementKind, Point } from './circuit';

/** What we put in the envelope, so a stray JSON blob is not mistaken for a
 *  circuit. Version it from the start: this string is going to end up in
 *  chat logs and gists that outlive any particular build. */
const MAGIC = 'ee-game/parts';
const VERSION = 1;

/** A paste is one gesture; nobody means to drop ten thousand parts with it,
 *  and a document that large has other problems. Bounded so a malformed or
 *  malicious blob cannot make the client chew through it before the gate
 *  gets a say. */
const MAX_PARTS = 512;
/** Pins per part. The real per-kind counts are smaller and are enforced by
 *  the gate; this is only here so a garbage array cannot be enormous. */
const MAX_PINS = 16;
/** Grid coordinates are i32 on the wire. Anything beyond this is not a
 *  circuit somebody drew. */
const MAX_COORD = 1e7;

export type ClipPart = { kind: ElementKind; pins: Point[]; tier?: number; rot?: number };

/** The text that goes on the clipboard. Pretty-printed on purpose — this is
 *  a thing people will paste into a message and read. */
export function serializeParts(parts: ClipPart[]): string {
  return JSON.stringify({ ee: MAGIC, v: VERSION, parts }, null, 1);
}

const isInt = (n: unknown): n is number =>
  typeof n === 'number' && Number.isFinite(n) && Number.isInteger(n) && Math.abs(n) <= MAX_COORD;

function parsePins(v: unknown): Point[] | null {
  if (!Array.isArray(v) || v.length < 1 || v.length > MAX_PINS) return null;
  const out: Point[] = [];
  for (const p of v) {
    if (!Array.isArray(p) || p.length !== 2 || !isInt(p[0]) || !isInt(p[1])) return null;
    out.push([p[0], p[1]]);
  }
  return out;
}

/** Read parts off clipboard text, or null if that text is not ours.
 *
 *  Null is the ordinary answer, not an error: most of the time the clipboard
 *  holds a URL or a sentence, and the caller simply carries on with whatever
 *  it had. */
export function parseParts(text: string): ClipPart[] | null {
  // A cheap reject before handing several megabytes of someone's document to
  // JSON.parse.
  if (!text || text.length > 4_000_000 || !text.includes(MAGIC)) return null;
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof doc !== 'object' || doc === null) return null;
  const o = doc as Record<string, unknown>;
  if (o.ee !== MAGIC || o.v !== VERSION) return null;
  if (!Array.isArray(o.parts) || o.parts.length < 1 || o.parts.length > MAX_PARTS) return null;

  const out: ClipPart[] = [];
  for (const raw of o.parts) {
    if (typeof raw !== 'object' || raw === null) return null;
    const r = raw as Record<string, unknown>;
    // The kind is passed through as-is apart from requiring an object with a
    // string tag: this module has no business knowing the catalogue, and the
    // gate refuses a kind nothing recognises anyway. What it must NOT be is
    // a string, a number, or an array pretending to be one.
    const kind = r.kind;
    if (
      typeof kind !== 'object' ||
      kind === null ||
      Array.isArray(kind) ||
      typeof (kind as Record<string, unknown>).t !== 'string'
    ) {
      return null;
    }
    const pins = parsePins(r.pins);
    if (!pins) return null;
    const tier = r.tier === undefined ? 0 : r.tier;
    const rot = r.rot === undefined ? 0 : r.rot;
    if (!isInt(tier) || tier < 0 || tier > 8) return null;
    if (!isInt(rot)) return null;
    out.push({ kind: kind as ElementKind, pins, tier, rot: ((rot % 4) + 4) % 4 });
  }
  return out;
}
