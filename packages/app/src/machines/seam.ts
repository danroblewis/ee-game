// THE MACHINE SEAM: the protocol message a server sends about a machine, and
// the contract a machine module implements to be one.
//
// Its own file because BOTH the registry and every machine module need these,
// and a machine importing them from the registry (which imports every
// machine) would be a cycle.
//
// One message shape serves every machine. Today its payload is a
// one-degree-of-freedom mechanism — a carriage on a track with a travel
// limit, a goal band, an end-stop pair and a position sensor — which is what
// the server's `machine` crate integrates. A machine module decides what that
// degree of freedom LOOKS like (a hoist's crate up a shaft, a conveyor's
// carriage along a belt); it does not get to invent numbers for it.

import type { ChipDraw, ChipSpec } from '../chip';

/** Server -> client machine state, once per tick (protocol contract). */
export interface MachineMsg {
  /** Fixture id of the motor, i.e. which machine this is. */
  id: number;
  /** Which chip presentation to draw it with. Optional: a server from before
   * the chip presentation existed still resolves to the hoist. */
  kind?: string;
  /** Footprint in GRID units: [x0, y0, x1, y1] — the machine's CELL. The
   * package body is inset inside it and the legs point inward, so every pin
   * is inside the box the server validated. */
  rect: [number, number, number, number];
  /** Travel, metres. */
  h: number;
  /** Goal band [lo, hi], metres. */
  band: [number, number];
  /** Carriage position, metres (integral of a solver unknown). */
  y: number;
  /** Carriage velocity, m/s. */
  vel: number;
  /** Motor current into pin 0, amps (a solver unknown). */
  i: number;
  /** Accumulated in-band time, seconds. */
  hold: number;
  /** Hold time the goal needs, seconds. */
  need: number;
  /** Landing speed, m/s — non-zero only on the tick the carriage hits the
   * bottom stop. */
  impact: number;
  /** Hard landings so far. */
  landings: number;
  win: boolean;
  /** Energy delivered by the player's sources, joules. */
  joules: number;
  /** The motor's nameplate current, amps — its safety limit, straight from
   * the server's damage table. Optional so a server from before parts could
   * break still renders (the faceplate then just omits the rating). */
  imax?: number;
  /** Position-sensor wiper, 0 at the far end of travel to 1 at the near end:
   * EXACTLY the number written into the solver. The client must not derive it
   * from `y`, because the two differ at the stops (the wiper clamps at
   * 0.02/0.98) and drawing the tap from `y` would be a small lie about what
   * the pot sees. Optional: an older server just gets the derived value. */
  wiper?: number;
  /** Limit-switch positions, as written into the solver. The document's copy
   * of these is broadcast once at `hello` and never again, so a package that
   * drew its switches from `ElementKind` would show state frozen at join. */
  limt?: boolean;
  limb?: boolean;
  /** Limit trip positions [near, far], metres — where the blocks are bolted. */
  lim?: [number, number];
}

// ------------------------------------------------------------- the contract

/** The client half of ONE machine, at that machine's own state type `S`.
 *
 * `S` is private to the machine: only its own `create()` produces it and only
 * its own `ChipSpec` consumes it. That is what lets the registry hold
 * machines whose states have nothing whatever in common — see
 * machines/index.ts, where each entry is erased to a `Machine` at its own S. */
export interface MachineDef<S> {
  spec: ChipSpec<S>;
  /** A fresh animator for one machine instance. */
  create(): MachineAnim<S>;
}

/** Everything a machine needs to know about the moment being drawn. */
export interface MachineFrame {
  m: MachineMsg;
  /** performance.now(), for animation phases. */
  now: number;
  /** No message recently: the picture is frozen and must say so rather than
   * pass stale numbers off as live. */
  stale: boolean;
}

/** Per-instance client state: whatever this machine INTEGRATES from the
 * server's messages — a drum angle from `vel`, a dust cloud from `impact`, a
 * belt phase. It may not invent physics: everything it produces has to be a
 * function of the messages it was handed and the frame clock, because the
 * design pillar is that every number a player sees came out of the solver. */
export interface MachineAnim<S> {
  onMessage(m: MachineMsg): void;
  advance(dtSec: number): void;
  /** This frame's state for the chip. Must be idempotent: it is called once
   * to draw the package and again to fill in the datasheet. */
  frame(at: MachineFrame): S;
}

/** What the room hands a machine to draw it: a `ChipDraw` minus the two
 * fields only the machine can supply (its spec and its state), plus the
 * moment being drawn. */
export type MachineDraw = Omit<ChipDraw<never>, 'spec' | 'state'> & { at: MachineFrame };
