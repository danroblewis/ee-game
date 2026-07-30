// Uniform spatial hash over the document, in GRID units.
//
// The world is now huge (50k elements server-side), so nothing on the hot
// path may scan the whole document: the render loop culls to the viewport
// through this index, and hit-testing only ever looks at the buckets near
// the cursor. It is a plain hash grid — the R-tree interest management in
// the plan (M5) is a server-side concern; this is the client's cull.
//
// Invariants:
//   - one cached bbox per element id, invalidated by insert/update/remove
//     (main.ts funnels every geometry change through applyDoc + the move
//     drag, which call update);
//   - `seq` preserves document order so a culled draw list can be restored
//     to the same z-order the un-culled loop had;
//   - inserts are bounded: an element covering more than MAX_CELLS buckets
//     goes on the `big` list, which every query includes. A 100k-unit wire
//     therefore costs O(1) to index instead of 3k bucket pushes.

import type { ElementSpec } from './circuit';

/** Bucket edge in grid units. 32 keeps a screenful at 12 px/unit down to a
 * few dozen buckets while a typical part lands in exactly one. */
export const BUCKET = 32;

/** Symbol overhang past the pins, in grid units (op-amp bodies, ground
 * combs, lamp circles all sit within ~0.8). Bboxes are padded by it so a
 * viewport cull never clips a symbol that pokes into view. */
export const SYMBOL_PAD = 0.9;

/** Cap on buckets one element may occupy before it becomes "big". */
const MAX_CELLS = 256;

const CELL_OFF = 1 << 20;
const CELL_SPAN = 1 << 21;
/** Pack signed bucket coords into one safe integer (no string keys on the
 * hot path). |bx|,|by| < 2^20 buckets = ±33M grid units. */
const cellKey = (bx: number, by: number) => (bx + CELL_OFF) * CELL_SPAN + (by + CELL_OFF);

export interface Bbox {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** Padded grid-space bounding box of an element's pins. */
export function elemBbox(e: ElementSpec): Bbox {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const p of e.pins) {
    if (p[0] < x0) x0 = p[0];
    if (p[0] > x1) x1 = p[0];
    if (p[1] < y0) y0 = p[1];
    if (p[1] > y1) y1 = p[1];
  }
  if (!Number.isFinite(x0)) return { x0: 0, y0: 0, x1: 0, y1: 0 };
  return { x0: x0 - SYMBOL_PAD, y0: y0 - SYMBOL_PAD, x1: x1 + SYMBOL_PAD, y1: y1 + SYMBOL_PAD };
}

export interface IndexStats {
  elements: number;
  cells: number;
  big: number;
  /** Mean ids per occupied bucket. */
  load: number;
}

export class SpatialIndex {
  private cells = new Map<number, number[]>();
  private boxes = new Map<number, Bbox>();
  private specs = new Map<number, ElementSpec>();
  private seq = new Map<number, number>();
  private big: number[] = [];
  private nextSeq = 0;
  /** Query dedup stamps: id -> epoch, so no Set is allocated per query. */
  private stamp = new Map<number, number>();
  private epoch = 0;

  get count(): number {
    return this.specs.size;
  }

  get(id: number): ElementSpec | undefined {
    return this.specs.get(id);
  }

  bboxOf(id: number): Bbox | undefined {
    return this.boxes.get(id);
  }

  /** Document order of an element (ascending = drawn later). */
  seqOf(id: number): number {
    return this.seq.get(id) ?? 0;
  }

  stats(): IndexStats {
    let ids = 0;
    for (const c of this.cells.values()) ids += c.length;
    return {
      elements: this.specs.size,
      cells: this.cells.size,
      big: this.big.length,
      load: this.cells.size === 0 ? 0 : ids / this.cells.size,
    };
  }

  /** Full rebuild — used on `hello` and whenever the array is replaced. */
  rebuild(elements: ElementSpec[]): void {
    this.cells.clear();
    this.boxes.clear();
    this.specs.clear();
    this.seq.clear();
    this.stamp.clear();
    this.big = [];
    this.nextSeq = 0;
    for (const e of elements) this.insert(e);
  }

  insert(e: ElementSpec): void {
    if (this.specs.has(e.id)) {
      this.update(e);
      return;
    }
    this.specs.set(e.id, e);
    this.seq.set(e.id, this.nextSeq++);
    this.link(e);
  }

  /** Geometry changed (Move / SetKind / pin drag): re-link only this id. */
  update(e: ElementSpec): void {
    const known = this.specs.get(e.id);
    if (!known) {
      this.insert(e);
      return;
    }
    if (known !== e) this.specs.set(e.id, e);
    const old = this.boxes.get(e.id);
    const box = elemBbox(e);
    if (old && old.x0 === box.x0 && old.y0 === box.y0 && old.x1 === box.x1 && old.y1 === box.y1) {
      return; // same cells; nothing to re-link
    }
    this.unlink(e.id);
    this.link(e, box);
  }

  remove(id: number): void {
    if (!this.specs.has(id)) return;
    this.unlink(id);
    this.specs.delete(id);
    this.boxes.delete(id);
    this.seq.delete(id);
    this.stamp.delete(id);
  }

  /** Bucket span of a bbox. `big` = do not bucket it at all: either it
   * covers too many cells, or it sits outside the packable key range (a
   * peer could send absurd coordinates; a wrong key must not alias). */
  private static range(box: Bbox) {
    const bx0 = Math.floor(box.x0 / BUCKET);
    const bx1 = Math.floor(box.x1 / BUCKET);
    const by0 = Math.floor(box.y0 / BUCKET);
    const by1 = Math.floor(box.y1 / BUCKET);
    const inRange =
      Math.abs(bx0) < CELL_OFF &&
      Math.abs(bx1) < CELL_OFF &&
      Math.abs(by0) < CELL_OFF &&
      Math.abs(by1) < CELL_OFF;
    const big = !inRange || (bx1 - bx0 + 1) * (by1 - by0 + 1) > MAX_CELLS;
    return { bx0, bx1, by0, by1, big };
  }

  private link(e: ElementSpec, box = elemBbox(e)): void {
    this.boxes.set(e.id, box);
    const { bx0, bx1, by0, by1, big } = SpatialIndex.range(box);
    if (big) {
      this.big.push(e.id);
      return;
    }
    for (let bx = bx0; bx <= bx1; bx++) {
      for (let by = by0; by <= by1; by++) {
        const k = cellKey(bx, by);
        const c = this.cells.get(k);
        if (c) c.push(e.id);
        else this.cells.set(k, [e.id]);
      }
    }
  }

  private unlink(id: number): void {
    const box = this.boxes.get(id);
    if (!box) return;
    const { bx0, bx1, by0, by1, big } = SpatialIndex.range(box);
    if (big) {
      const k = this.big.indexOf(id);
      if (k >= 0) this.big.splice(k, 1);
      return;
    }
    for (let bx = bx0; bx <= bx1; bx++) {
      for (let by = by0; by <= by1; by++) {
        const k = cellKey(bx, by);
        const c = this.cells.get(k);
        if (!c) continue;
        const at = c.indexOf(id);
        if (at >= 0) c.splice(at, 1);
        if (c.length === 0) this.cells.delete(k);
      }
    }
  }

  /** Every element whose padded bbox intersects the rect, in bucket order.
   * `out` is reused by the caller so a steady frame allocates nothing. */
  query(x0: number, y0: number, x1: number, y1: number, out: ElementSpec[] = []): ElementSpec[] {
    out.length = 0;
    const ep = ++this.epoch;
    // Clamp the swept bucket range: a camera parked absurdly far out must not
    // turn one query into millions of misses.
    const bx0 = Math.max(-CELL_OFF + 1, Math.floor(x0 / BUCKET));
    const bx1 = Math.min(CELL_OFF - 1, Math.floor(x1 / BUCKET));
    const by0 = Math.max(-CELL_OFF + 1, Math.floor(y0 / BUCKET));
    const by1 = Math.min(CELL_OFF - 1, Math.floor(y1 / BUCKET));
    for (let bx = bx0; bx <= bx1; bx++) {
      for (let by = by0; by <= by1; by++) {
        const c = this.cells.get(cellKey(bx, by));
        if (!c) continue;
        for (const id of c) {
          if (this.stamp.get(id) === ep) continue;
          this.stamp.set(id, ep);
          const b = this.boxes.get(id)!;
          if (b.x1 < x0 || b.x0 > x1 || b.y1 < y0 || b.y0 > y1) continue;
          const e = this.specs.get(id);
          if (e) out.push(e);
        }
      }
    }
    for (const id of this.big) {
      if (this.stamp.get(id) === ep) continue;
      this.stamp.set(id, ep);
      const b = this.boxes.get(id)!;
      if (b.x1 < x0 || b.x0 > x1 || b.y1 < y0 || b.y0 > y1) continue;
      const e = this.specs.get(id);
      if (e) out.push(e);
    }
    return out;
  }

  /** Restore document order in place (draw z-order must not depend on which
   * bucket an element happened to land in). */
  sortByDoc(list: ElementSpec[]): ElementSpec[] {
    return list.sort((a, b) => this.seqOf(a.id) - this.seqOf(b.id));
  }
}
