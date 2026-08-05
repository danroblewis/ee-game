// THE SELECTION HUD — the modifier keys and the Delete key, drawn.
//
// The palette (palette.ts) gave a finger a way to ARM a part. This file gives
// it a way to act on what is already on the sheet. Everything here is a verb
// that lives on a key or a modifier today and is therefore unreachable on
// glass:
//
//   q          rotate            →  ⟳
//   x / y      mirror            →  ⇋ / ⇅
//   ⌘C         copy              →  ⧉
//   Delete     delete            →  del
//   ← ↑ ↓ →    nudge             →  the four arrows
//   shift/alt  add / remove      →  ONE sticky mode button (see below)
//   ⌘Z / ⌘⇧Z   undo / redo       →  ↶ / ↷
//   right-click part menu        →  ⋯  (the whole cascade, for a selection)
//
// TWO RULES.
//
// 1. IT DOES NOT MOUNT UNTIL A FINGER LANDS — the same rule, and the same
//    reason, as the palette: a touchscreen laptop is a mouse machine until
//    somebody actually touches the screen, and new chrome under a desktop
//    player who owns a keyboard is a regression.
//
// 2. UNDO AND REDO ARE ALWAYS THERE. Everything else on the strip appears
//    with a selection and goes away with it, but a player who has just done
//    something wrong must never have to first select something in order to
//    take it back. They are the only two buttons that stand alone.
//
// The strip floats just above the palette bar, and it measures where that bar
// actually is rather than assuming a height — the bar wraps to two rows on a
// 390 px phone as soon as the armed chip appears, and a hard-coded offset
// would put this strip through the middle of it.

/** Sticky stand-in for shift (add) and alt (remove) while clicking or
 *  marquee-ing. Null is the plain, replacing click. */
export type TouchSelectMode = 'add' | 'remove' | null;

export interface HudHooks {
  undo(): void;
  redo(): void;
  rotate(): void;
  flip(axis: 'x' | 'y'): void;
  copy(): void;
  del(): void;
  nudge(dx: number, dy: number): void;
  /** The part menu for the current selection, anchored at these screen px. */
  more(x: number, y: number): void;
  setMode(m: TouchSelectMode): void;
}

export interface HudState {
  /** How many parts are selected. 0 hides everything but undo/redo. */
  count: number;
  mode: TouchSelectMode;
  /** Hide the whole strip: the parts sheet is open over it, or a part is
   *  being placed and the player is building, not editing. */
  hidden: boolean;
}

export interface HudUI {
  sync(s: HudState): void;
  isMounted(): boolean;
  owns(t: EventTarget | null): boolean;
  /** Test seam: mount without waiting for a finger. */
  mountNow(): void;
}

const MODE_LABEL: Record<string, string> = {
  null: 'tap: set',
  add: 'tap: add',
  remove: 'tap: drop',
};

export function createTouchHud(host: HTMLElement, hooks: HudHooks): HudUI {
  let root: HTMLElement | null = null;
  let sel: HTMLElement | null = null;
  let countEl: HTMLElement | null = null;
  let modeBtn: HTMLButtonElement | null = null;
  let moreBtn: HTMLButtonElement | null = null;
  let last = '?'; // signature of the last state painted ('?' matches none)
  let lastBottom = -1;

  /** A button that never keeps focus — a stuck :focus ring on glass reads as
   *  "still armed", and focus parked on a button aims the on-screen
   *  keyboard's Space (the pan modifier) at it. */
  const button = (cls: string, text: string, title: string, onTap: () => void) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = cls;
    b.textContent = text;
    b.title = title;
    b.onclick = () => {
      b.blur();
      onTap();
    };
    return b;
  };

  function build() {
    root = document.createElement('div');
    root.id = 'thud';

    // ---- always: undo, redo, and the sticky selection mode.
    const nav = document.createElement('div');
    nav.className = 'thud-grp';
    // Spelled out, not ↶ / ↷: in the mono face this page is set in, those two
    // arrows render as a squiggle and a squiggle, and undo is the one button
    // that must never be guessed at.
    nav.append(
      button('thud-b thud-word', 'undo', 'undo (⌘Z)', hooks.undo),
      button('thud-b thud-word', 'redo', 'redo (⌘⇧Z)', hooks.redo),
    );
    // The mode button lives with undo/redo rather than with the selection
    // verbs BECAUSE it has to be settable before there is a selection: it is
    // how a long-press-drag marquee is told to add rather than replace.
    modeBtn = button('thud-b thud-mode', MODE_LABEL.null!, 'shift / alt, for a hand with neither', () => {
      const now = modeBtn!.dataset.mode ?? 'null';
      hooks.setMode(now === 'null' ? 'add' : now === 'add' ? 'remove' : null);
    });
    nav.appendChild(modeBtn);

    // ---- with a selection: everything the keyboard did to it.
    sel = document.createElement('div');
    sel.className = 'thud-grp thud-sel';
    countEl = document.createElement('span');
    countEl.className = 'thud-n';
    moreBtn = button('thud-b', '⋯', 'the part menu (right-click)', () => {
      const r = moreBtn!.getBoundingClientRect();
      hooks.more(r.left, r.top - 8);
    });
    const nudge = document.createElement('div');
    nudge.className = 'thud-nudge';
    nudge.append(
      button('thud-b thud-arrow', '◀', 'nudge left (←)', () => hooks.nudge(-1, 0)),
      button('thud-b thud-arrow', '▲', 'nudge up (↑)', () => hooks.nudge(0, -1)),
      button('thud-b thud-arrow', '▼', 'nudge down (↓)', () => hooks.nudge(0, 1)),
      button('thud-b thud-arrow', '▶', 'nudge right (→)', () => hooks.nudge(1, 0)),
    );
    sel.append(
      countEl,
      button('thud-b', '⟳', 'rotate (Q)', hooks.rotate),
      button('thud-b', '⇋', 'mirror left/right (X)', () => hooks.flip('x')),
      button('thud-b', '⇅', 'mirror up/down (Y)', () => hooks.flip('y')),
      button('thud-b', '⧉', 'copy (⌘C)', hooks.copy),
      button('thud-b thud-del', 'del', 'delete (Del)', hooks.del),
      moreBtn,
      nudge,
    );

    root.append(sel, nav);
    host.appendChild(root);
  }

  function mountNow() {
    if (!root) build();
  }

  const onFirstTouch = (ev: PointerEvent) => {
    if (ev.pointerType !== 'touch') return;
    window.removeEventListener('pointerdown', onFirstTouch, true);
    mountNow();
  };
  window.addEventListener('pointerdown', onFirstTouch, true);

  /** Park the strip immediately above the palette bar, wherever that has
   *  ended up this frame. Measured, not assumed — the bar wraps. */
  function reposition() {
    if (!root) return;
    const bar = document.querySelector('.tpal-bar');
    const bottom = bar
      ? Math.round(window.innerHeight - bar.getBoundingClientRect().top + 6)
      : 8;
    if (bottom === lastBottom) return;
    lastBottom = bottom;
    root.style.bottom = `${bottom}px`;
  }

  function sync(s: HudState) {
    if (!root || !sel || !countEl || !modeBtn) return;
    const sig = `${s.count}|${s.mode}|${s.hidden}`;
    if (sig !== last) {
      last = sig;
      root.style.display = s.hidden ? 'none' : '';
      sel.style.display = s.count > 0 ? '' : 'none';
      countEl.textContent = s.count > 1 ? `${s.count} sel` : '';
      countEl.style.display = s.count > 1 ? '' : 'none';
      const key = s.mode ?? 'null';
      modeBtn.dataset.mode = key;
      modeBtn.textContent = MODE_LABEL[key]!;
      modeBtn.classList.toggle('on', s.mode !== null);
    }
    if (!s.hidden) reposition();
  }

  return {
    sync,
    isMounted: () => root !== null,
    owns: (t) => t instanceof Node && root !== null && root.contains(t),
    mountNow,
  };
}
