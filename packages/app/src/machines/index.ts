// The machine registry: chip presentations by the `kind` the server sends.
//
// This is the whole client-side cost of a second machine. Add its ChipSpec
// file next to hoist.ts, add one line here, and the package renders, drags,
// selects, hit-tests, labels its pins, animates its dots, shows its damage
// and degrades across LOD_FULL exactly like the first one — because none of
// that machinery lives in a machine file.

import type { ChipSpec } from '../chip';
import { HOIST_CHIP, type HoistState } from './hoist';

/** Chip presentations by `machine.kind`. */
export const CHIPS: Record<string, ChipSpec<HoistState>> = {
  hoist: HOIST_CHIP,
};

/** The presentation for a machine message, falling back to the hoist so a
 * server from before `kind` existed still draws something real. */
export function chipFor(kind: string | undefined): ChipSpec<HoistState> {
  return CHIPS[kind ?? 'hoist'] ?? HOIST_CHIP;
}
