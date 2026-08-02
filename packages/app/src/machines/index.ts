// The machine registry: machines by the `kind` the server sends.
//
// This is the whole client-side cost of a second machine. Write its module
// next to hoist.ts — a `ChipSpec` and an animator, i.e. one `MachineDef` —
// add one line to `MACHINES` below, and the package renders, drags, selects,
// hit-tests, labels its pins, animates its dots, shows its damage, fills in
// its datasheet and degrades across LOD_FULL exactly like the first one,
// because none of that machinery lives in a machine module.
//
// WHY THIS FILE HAS A `bind` IN IT. A machine's state type is its own: the
// hoist integrates a drum angle and a dust cloud, a conveyor integrates a
// belt phase and a run of parcels, and neither type is assignable to the
// other. A registry typed `Record<string, ChipSpec<HoistState>>` therefore
// rejected the second machine outright — the seam stopped one file short of
// the thing it claimed to enable. It cannot be fixed by widening `S`:
// `ChipSpec<S>` consumes S and a machine's animator produces it, so S is
// invariant and no supertype accepts both (and `any` would merely delete the
// pairing check that keeps a spec drawing its OWN state).
//
// The fix is to store machines already erased. `bind` is generic, so each
// entry below calls it at ITS OWN state type and hands back a `Machine` — a
// value with no state type at all, whose spec and animator are sealed
// together in one closure and can only ever meet each other. Unrelated
// machines then share one table with no cast anywhere.

import {
  chipLegs,
  chipMeas,
  chipZoneAt,
  renderChip,
  type ChipLeg,
  type PinoutRow,
} from '../chip';
import type { ElemLive } from '../circuit';
import type { Camera } from '../render';
import { CONVEYOR } from './conveyor';
import { HOIST } from './hoist';
import type { MachineDef, MachineDraw, MachineFrame, MachineMsg } from './seam';

export type { MachineDraw, MachineFrame, MachineMsg } from './seam';

/** One live machine, with its state type sealed inside. Everything the room
 * does to a machine, it does through this. */
export interface Machine {
  /** The `kind` this was built for. */
  readonly kind: string;
  /** One machine message from the net layer (or the dev mock). */
  onMessage(m: MachineMsg): void;
  /** Advance whatever this machine integrates on the client. */
  advance(dtSec: number): void;
  /** Draw the package, and remember the legs it stood on. */
  draw(d: MachineDraw): void;
  /** Which part of the package a screen point is over; null = not on it.
   * Hit-tests the legs the last `draw` resolved, so the box you can grab is
   * the box you can see. */
  zoneAt(
    cam: Camera,
    rect: [number, number, number, number],
    x: number,
    y: number,
  ): 'body' | 'info' | null;
  /** The datasheet's pinout table, with this frame's solver readings in it. */
  pinout(at: MachineFrame, live: Map<number, ElemLive>): PinoutRow[];
}

/** Seal one machine's spec and animator together and forget its state type.
 * Generic, and called once per registry entry at that entry's own `S`. */
function bind<S>(def: MachineDef<S>): Machine {
  const anim = def.create();
  /** The legs the last draw resolved: what `zoneAt` and `pinout` read. */
  let legs: ChipLeg[] = [];
  return {
    kind: def.spec.kind,
    onMessage: (m) => anim.onMessage(m),
    advance: (dtSec) => anim.advance(dtSec),
    draw(d) {
      legs = chipLegs(def.spec, d.rect, d.children);
      renderChip({
        ctx: d.ctx,
        cam: d.cam,
        spec: def.spec,
        rect: d.rect,
        state: anim.frame(d.at),
        children: d.children,
        live: d.live,
        damage: d.damage,
        dots: d.dots,
        dtSec: d.dtSec,
        hot: d.hot,
      });
    },
    zoneAt: (cam, rect, x, y) => chipZoneAt(cam, legs, rect, x, y),
    pinout: (at, live) => def.spec.pinout(anim.frame(at), chipMeas(legs, live)),
  };
}

/** Machine constructors by `machine.kind`. One line per machine. */
export const MACHINES: Record<string, () => Machine> = {
  hoist: () => bind(HOIST),
  conveyor: () => bind(CONVEYOR),
};

/** Stand up the machine for a `kind`, falling back to the hoist so a server
 * from before `kind` existed still gets something real. */
export function machineFor(kind: string | undefined): Machine {
  return (MACHINES[kind ?? 'hoist'] ?? MACHINES['hoist']!)();
}
