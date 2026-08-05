// WHEN IS A NARROW WINDOW A PHONE?
//
// The chrome takes a different posture on a phone: cards that would blanket
// a 390 px world start folded, and a rail is allowed to take the screen
// because the alternative was a rail that could never open at all. Every one
// of those is the right answer for a phone and the WRONG answer for a mouse.
//
// The first cut of this asked `window.innerWidth < 500` and nothing else, so
// a desktop player who narrowed a window changed what the app did: measured
// at 480 px wide with a mouse and no touch pointer in the session at all, the
// status plate moved from [8,8,180] to [8,44,464], the lesson card and the
// goal card both came up collapsed instead of expanded, and the schematic's
// guaranteed 200 px share dropped to 180. None of that is a desktop
// regression anybody would call a bug, and all of it is a desktop regression:
// the brief for this work says the mouse path is byte-identical, and "the
// difference is harmless" is not the same sentence.
//
// So width is necessary and not sufficient. `pointer: coarse` says the
// PRIMARY input cannot aim precisely and `hover: none` says it cannot hover;
// a phone and a tablet answer yes to both, and a desktop, a laptop, and a
// TOUCHSCREEN laptop with a trackpad all answer no to at least one — which is
// the case the width test got wrong. A window is not a device.
export const PHONE_PX = 500;

/** Both halves, asked fresh: a window can be resized and a device can be
 *  docked. Guarded for the odd embedding with no matchMedia. */
export const phonePosture = (): boolean =>
  window.innerWidth < PHONE_PX &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(pointer: coarse) and (hover: none)').matches;
