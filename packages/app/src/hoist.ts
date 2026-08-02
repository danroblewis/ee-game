// THE HOIST — client half of the game's first goal.
//
// A freight hoist stands in the world beside the demo vignettes: a crate on a
// platform in a vertical shaft, lifted by a DC motor whose two leads are real
// wire-able terminals. The player has to hold the crate inside a painted green
// band for five continuous seconds. A constant voltage lifts it but cannot
// hold it (voltage buys speed, not position), so the goal is only reachable by
// wiring feedback off the position sensor — and since parts have safety limits
// now, a bare supply across M+/M− does not merely fail, it burns the motor out
// (a stalled armature draws V/R). The nameplate and the goal card say so, and
// the rating they print is the server's own.
//
// THE MACHINE IS A CHIP. It is drawn as a package with nine pins on legs
// OUTSIDE the body, exactly the 555's grammar, with the live physics inside.
// That is not a skin: playing the old cabinet made it obvious that a player
// wires OUT from a machine no matter what it looks like, so the symbol should
// present its terminals the way every other multi-pin part does. chip.ts owns
// the package and machines/hoist.ts owns what goes inside it; THIS file owns
// the machine as a live object in the room:
//
//   * the STATE — the server's message plus the few quantities the client
//     integrates from it (drum angle from `vel`, dust from `impact`, flash
//     ages), handed to the chip renderer each frame;
//   * the GOAL CARD, an HTML overlay in the same visual language as the
//     control-panel windows (.pwin) in index.html, now with a PINOUT tab that
//     the package's own ⓘ badge opens;
//   * the machine's FOOTPRINT as a hit-testable object: `rect()` is the box in
//     grid units, `zoneAt()` says whether a pointer is on the package or its
//     badge, and `setLocalRect()` lets a drag in main.ts place the whole
//     assembly optimistically at 60 fps while the server catches up. That is
//     the entire client-side seam for "this machine is a draggable part" —
//     main.ts owns the pointer gesture, this file owns the box.
//
// NOTHING in here simulates the machine. The server integrates the crate from
// the SOLVER's motor current and broadcasts one "machine" message per tick;
// every number drawn or printed comes out of that message (design pillar: no
// faked electrical behaviour).
//
// Dev mock: with no server half in reach, `?hoistmock` in the URL (or calling
// window.__hoistMock() from the console) synthesises machine messages from a
// scripted lift/fall so the package, the landing event and the win state can
// be exercised locally. It only ever calls the same onMachine() the socket
// does.

import { chipZoneAt, renderChip, type ChipSpec } from './chip';
import { chipFor } from './machines';
import { ratedA, type Dust, type HoistState, type MachineMsg } from './machines/hoist';
import type { ElemLive, ElementSpec } from './circuit';
import type { Camera, DamageState, DotFlow } from './render';

export type { MachineMsg } from './machines/hoist';

/** Footprint in grid units, corners normalized: [x0, y0, x1, y1]. */
export type MachineRect = [number, number, number, number];

/**
 * What a pointer inside the machine is over:
 *   'info' — the ⓘ badge on the package's title band: opens the datasheet.
 *            Only ever reported when the badge is actually painted.
 *   'body' — the package face, which is the whole handle (render.ts's
 *            `hitTest` already treats any package's face that way, and the
 *            555 has no title bar either). Note this is the BODY box, not the
 *            footprint: the leg corridors are NOT machine zones, so the
 *            package can never swallow a click aimed at a terminal.
 * Callers must still hit-test PINS and CHILD ELEMENTS first: a terminal starts
 * a wire and a child part selects on its own.
 */
export type MachineZone = 'body' | 'info';

/** What the machine needs from the document to draw its own children. */
export interface MachineView {
  /** The locked fixture elements (ids 900..999). */
  children: ElementSpec[];
  live: Map<number, ElemLive>;
  damage: Map<number, DamageState>;
  dots: DotFlow;
}

export interface Hoist {
  /** One machine message from the net layer (or the dev mock). */
  onMachine(m: MachineMsg): void;
  /** Once per animation frame, AFTER the schematic pass (a player's wire
   * routed across the package passes BEHIND the body, which is what a
   * package does) and before selection halos and probe flags. */
  draw(
    ctx: CanvasRenderingContext2D,
    cam: Camera,
    now: number,
    dtSec: number,
    view: MachineView,
  ): void;
  /** Latest state, or null before the first message (tests/debug). */
  state(): MachineMsg | null;
  /** The footprint being drawn right now — the server's, or this client's
   * optimistic one mid-drag. Null before the first machine message (offline:
   * there is no machine, so there is nothing to hit-test or drag). */
  rect(): MachineRect | null;
  /** Which part of the machine a screen point is over; null = not on it. */
  zoneAt(cam: Camera, x: number, y: number): MachineZone | null;
  /** Open the datasheet on its pinout. A machine has no editable values, so
   * it has no property editor — it has a datasheet, and the ⓘ badge on its
   * title band is the way in. */
  openPinout(): void;
  /** Place the assembly locally, ahead of the server, while dragging. Held
   * until `endLocalDrag` plus one round trip, then the server's rect wins
   * again — so a peer moving the same machine mid-drag can never leave this
   * client stuck on a placement the room does not share. */
  setLocalRect(r: MachineRect): void;
  /** The pointer let go: keep the local placement just long enough for the
   * server's answer to arrive, then defer to it. */
  endLocalDrag(): void;
  /** Light up the package outline (pointer over it, or a drag in progress). */
  setHot(hot: boolean): void;
  /**
   * Forget the machine entirely: no state, no footprint, no goal card.
   *
   * Called when the client changes ROOM. Not every room has a hoist — a
   * template declares whether it owns a machine — and the card used to latch
   * visible forever after the first message, so joining a sandbox from the
   * hoist would have left a frozen objective on screen for a machine that is
   * no longer anywhere in the world.
   */
  clear(): void;
}

/** Drum radius, metres (machine constant): only used to turn the server's
 * `vel` into a rotation angle. Never displayed. */
const DRUM_R = 0.02;
/** Impact speed that saturates the landing effects, m/s. */
const LAND_FULL = 2.0;
/** No machine message for this long = the link is gone, not the machine. */
const STALE_MS = 1500;
/** How long a released drag's placement outlives the gesture, waiting for the
 * server's answer. One server tick is 33 ms; this is a generous ceiling that
 * still guarantees the authoritative rect wins in bounded time. */
const LOCAL_GRACE_MS = 1000;

const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

/** Corners in min/max order: a rect sent as [x1, y1, x0, y0] still stands up
 * the right way instead of silently drawing (and hit-testing) nothing. */
const normRect = (r: MachineRect): MachineRect => [
  Math.min(r[0], r[2]),
  Math.min(r[1], r[3]),
  Math.max(r[0], r[2]),
  Math.max(r[1], r[3]),
];

const sameRect = (a: MachineRect, b: MachineRect) =>
  a[0] === b[0] && a[1] === b[1] && a[2] === b[2] && a[3] === b[3];

// -------------------------------------------------------------- goal card

const OPEN_KEY = 'ee.hoist.open';

// localStorage throws in private/blocked contexts; the card must still work.
const readLS = (k: string): string | null => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const writeLS = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v);
  } catch {
    /* ignore */
  }
};

/** SI formatting, same shape as the HUD's `fmt` in main.ts. */
function fmtSI(v: number, unit: string): string {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(2)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(2)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(2)} µ${unit}`;
  return `0 ${unit}`;
}

/** The instruction on the goal card.
 *
 * It used to read "wire a constant voltage to M+/M−", which is now the way to
 * destroy the machine: a stalled armature draws V/R (12 V across 2 Ω is 6 A),
 * so a bare supply cooks the motor within seconds of the crate reaching the
 * head stop. The card therefore asks for a CONTROLLED drive and names the two
 * numbers that make the point — the nameplate current, and where the stall
 * current comes from. Both are the machine's own constants; the live current
 * beside it is the solver's. */
function hintText(m: MachineMsg | null): string {
  return (
    `M+/M− drive the drum — ${ratedA(m)} max, and a stalled rotor draws V/R\n` +
    'SNS W reads height (12.5 mV/mm) · TOP / BOT are the end stops\n' +
    'A constant voltage buys speed, not position: it cannot hold the band,\n' +
    'and parked against a stop it burns the motor out. Close the loop and\n' +
    'keep the current inside the nameplate.\n' +
    'drag the chip to move the whole machine · ⌘Z undoes it · ⓘ for the pinout.'
  );
}

const div = (cls: string, parent?: HTMLElement): HTMLDivElement => {
  const el = document.createElement('div');
  el.className = cls;
  parent?.append(el);
  return el;
};

interface Card {
  /** `stale` = no machine message recently: the numbers are frozen, and the
   * card says so instead of passing old values off as live. */
  onState(s: HoistState, spec: ChipSpec<HoistState>): void;
  /** Take the card off screen and re-arm it, so the next room's first
   * machine message (if it ever has one) brings it back fresh. */
  hide(): void;
  /** Expand the card and select the PINOUT tab (the ⓘ badge). */
  showPinout(): void;
}

function buildCard(root: HTMLElement, reset: () => void): Card {
  const el = div('goalcard');
  el.id = 'goal';
  el.style.display = 'none';

  const hd = div('goal-hd', el);
  const caret = document.createElement('span');
  caret.className = 'goal-caret';
  const title = document.createElement('span');
  title.className = 'goal-title';
  title.textContent = 'CRATE IN BAND';
  const badge = document.createElement('span');
  badge.className = 'goal-badge';
  badge.textContent = '—';
  hd.append(caret, title, badge);

  // Two tabs: the goal, and the package's datasheet. The chip's ⓘ badge
  // opens the second one.
  const tabs = div('goal-tabs', el);
  const tabGoal = document.createElement('button');
  tabGoal.className = 'goal-tab on';
  tabGoal.textContent = 'GOAL';
  const tabPins = document.createElement('button');
  tabPins.className = 'goal-tab';
  tabPins.textContent = 'PINOUT';
  tabs.append(tabGoal, tabPins);

  const body = div('goal-body', el);
  const bar = div('goal-bar', body);
  const fill = div('goal-fill', bar);
  const barTxt = div('goal-bartext', bar);
  const grid = div('goal-grid', body);

  const cell = (label: string) => {
    const c = div('goal-cell', grid);
    const k = document.createElement('b');
    k.textContent = label;
    const v = document.createElement('span');
    v.textContent = '—';
    c.append(k, v);
    return v;
  };
  const vHeight = cell('height');
  const vVel = cell('velocity');
  const vCur = cell('motor I');
  const vJoule = cell('delivered');
  const vLand = cell('hard landings');
  const vBand = cell('band window');

  const score = div('goal-score', body);
  score.style.display = 'none';

  const row = div('goal-row', body);
  const btn = document.createElement('button');
  btn.className = 'goal-reset';
  btn.textContent = 'reset hoist';
  btn.title = 'lower the crate to the floor and re-arm the goal';
  btn.onclick = () => {
    reset();
    btn.blur(); // the canvas hotkeys listen on window; do not hold focus
  };
  row.append(btn);

  const hint = div('goal-hint', body);
  hint.textContent = hintText(null);
  /** The `imax` the hint text was last written for. */
  let hintFor: number | undefined;

  // ---- pinout pane: one row per leg, in package order.
  const pins = div('goal-pins', el);
  pins.style.display = 'none';
  const pinRows: { name: HTMLElement; what: HTMLElement; num: HTMLElement }[] = [];

  let tab: 'goal' | 'pins' = 'goal';
  const applyTab = () => {
    tabGoal.classList.toggle('on', tab === 'goal');
    tabPins.classList.toggle('on', tab === 'pins');
    body.style.display = tab === 'goal' ? '' : 'none';
    pins.style.display = tab === 'pins' ? '' : 'none';
  };
  tabGoal.onclick = (ev) => {
    ev.stopPropagation();
    tab = 'goal';
    applyTab();
  };
  tabPins.onclick = (ev) => {
    ev.stopPropagation();
    tab = 'pins';
    applyTab();
  };
  applyTab();

  let open = readLS(OPEN_KEY) !== '0'; // starts expanded; choice is remembered
  const apply = () => {
    el.classList.toggle('collapsed', !open);
    caret.textContent = open ? '▾' : '▸';
  };
  hd.onclick = () => {
    open = !open;
    writeLS(OPEN_KEY, open ? '1' : '0');
    apply();
  };
  apply();
  root.append(el);

  const UPDATE_MS = 60; // ~16 Hz: the bar reads smooth, the DOM stays cheap
  let lastAt = -Infinity;
  let lastWin = false;
  let lastStale = false;
  let shown = false;

  return {
    hide() {
      shown = false;
      el.style.display = 'none';
      el.classList.remove('win');
      lastAt = -Infinity;
      lastWin = false;
      lastStale = false;
    },
    showPinout() {
      if (!open) {
        open = true;
        writeLS(OPEN_KEY, '1');
        apply();
      }
      tab = 'pins';
      applyTab();
    },
    onState(s: HoistState, spec: ChipSpec<HoistState>) {
      const m = s.m;
      if (!shown) {
        shown = true;
        el.style.display = 'block';
      }
      const flip = m.win !== lastWin || s.stale !== lastStale;
      if (!flip && s.now - lastAt < UPDATE_MS) return;
      lastAt = s.now;
      lastWin = m.win;
      lastStale = s.stale;
      el.classList.toggle('stale', s.stale);

      title.textContent = `CRATE IN BAND — HOLD ${m.need.toFixed(1)} s`;
      const frac = m.need > 0 ? clamp(m.hold / m.need, 0, 1) : 0;
      fill.style.width = `${(frac * 100).toFixed(1)}%`;
      barTxt.textContent = `${m.hold.toFixed(2)} / ${m.need.toFixed(2)} s`;
      const inBand = m.y >= m.band[0] && m.y <= m.band[1];
      badge.textContent = s.stale ? 'NO LINK' : m.win ? 'HELD' : inBand ? 'IN BAND' : 'OUT';
      badge.className = `goal-badge${s.stale ? '' : m.win ? ' win' : inBand ? ' in' : ''}`;

      vHeight.textContent = `${(m.y * 1000).toFixed(1)} mm`;
      vVel.textContent = `${(m.vel * 1000).toFixed(0)} mm/s`;
      vCur.textContent = fmtSI(m.i, 'A');
      // Over the nameplate current is exactly the moment worth flagging:
      // both numbers come from the message, and the comparison is the whole
      // lesson (the motor is now cooking).
      const over = m.imax !== undefined && m.imax > 0 && Math.abs(m.i) > m.imax;
      vCur.className = over ? 'over' : '';
      // The hint names the nameplate current, so it is rewritten only when
      // that number actually arrives (or changes) — not 16 times a second.
      if (m.imax !== hintFor) {
        hintFor = m.imax;
        hint.textContent = hintText(m);
      }
      vJoule.textContent = `${m.joules.toFixed(1)} J`;
      vLand.textContent = String(m.landings);
      vBand.textContent = `${(m.band[0] * 1000).toFixed(0)}–${(m.band[1] * 1000).toFixed(0)} mm`;

      // The pinout is the chip's own table, so it can never disagree with
      // the labels engraved on the package.
      if (tab === 'pins') {
        const rows = spec.pinout(s);
        while (pinRows.length < rows.length) {
          const r = div('goal-pin', pins);
          const name = document.createElement('b');
          const what = document.createElement('i');
          const num = document.createElement('u');
          r.append(name, what, num);
          pinRows.push({ name, what, num });
        }
        rows.forEach((r, k) => {
          const cells = pinRows[k]!;
          cells.name.textContent = r[0];
          cells.what.textContent = r[1];
          cells.num.textContent = r[2];
        });
      }

      el.classList.toggle('win', m.win);
      score.style.display = m.win ? 'block' : 'none';
      if (m.win) {
        score.textContent =
          `HELD ${m.need.toFixed(1)} s — score: ${m.joules.toFixed(1)} J delivered, ` +
          `${m.landings} hard landing${m.landings === 1 ? '' : 's'}`;
      }
    },
  };
}

// ------------------------------------------------------------------ module

export function createHoist(root: HTMLElement, opts: { reset: () => void }): Hoist {
  const card = buildCard(root, () => {
    if (mock) mock.reset();
    else opts.reset();
  });

  let m: MachineMsg | null = null;
  let spin = 0;
  let landAge = Infinity;
  let landV = 0;
  let dust: Dust[] = [];
  let winAge = Infinity;
  let mock: Mock | null = null;
  /** Wall clock of the last message: the card must not pass a frozen state
   * off as live if the socket drops. */
  let lastMsgAt = -Infinity;
  /** This client's optimistic footprint while it drags the assembly. The
   * server's rect is the truth; this only covers the round trip so the machine
   * tracks the pointer instead of the network. */
  let localRect: MachineRect | null = null;
  /** True while the pointer owns the placement (between setLocalRect and
   * endLocalDrag). */
  let localHeld = false;
  /** When the placement was last claimed, for the post-release grace window. */
  let localAt = 0;
  /** Package highlight (hover or active drag). */
  let hot = false;

  function onMachine(next: MachineMsg) {
    lastMsgAt = performance.now();
    // The server has caught up with our optimistic placement: hand the
    // footprint straight back to it, with no wait for the grace window.
    // Comparing rects rather than counting acknowledgements means a move by
    // ANOTHER player also lands cleanly.
    if (!localHeld && localRect && sameRect(localRect, normRect(next.rect))) localRect = null;
    if (m && !m.win && next.win) winAge = 0;
    if (!m) winAge = next.win ? 0 : Infinity;
    if (next.impact > 0) {
      landAge = 0;
      landV = next.impact;
      spawnDust(next.impact);
    }
    m = next;
  }

  function spawnDust(impact: number) {
    const n = Math.round(clamp(impact / LAND_FULL, 0.15, 1) * 16);
    for (let k = 0; k < n; k++) {
      const side = k % 2 === 0 ? -1 : 1;
      const r = (k * 0.618) % 1; // cheap deterministic spread, no RNG needed
      dust.push({
        u: 0.5 + side * (0.12 + 0.3 * r),
        w: 0.01 + 0.03 * r,
        du: side * (0.25 + 0.55 * r) * clamp(impact / LAND_FULL, 0.3, 1.4),
        dw: (0.35 + 0.5 * r) * clamp(impact / LAND_FULL, 0.3, 1.4),
        age: 0,
        life: 0.5 + 0.5 * r,
      });
    }
    if (dust.length > 80) dust = dust.slice(dust.length - 80);
  }

  function advance(dtSec: number) {
    const mm = m;
    if (!mm) return;
    // Drum spin: omega = vel / r. Visual only — see DRUM_R.
    spin = (spin + (mm.vel / DRUM_R) * dtSec) % (Math.PI * 2);
    if (landAge !== Infinity) landAge += dtSec;
    if (winAge !== Infinity) winAge += dtSec;
    for (const d of dust) {
      d.age += dtSec;
      d.u += d.du * dtSec;
      d.w += d.dw * dtSec;
      d.dw -= 1.2 * dtSec; // settle back down
      d.du *= 1 - Math.min(1, 2.5 * dtSec);
    }
    if (dust.length > 0) dust = dust.filter((d) => d.age < d.life);
  }

  /** The footprint in play: this client's optimistic one while it drags the
   * assembly (and for one round trip after), the server's otherwise.
   *
   * The grace window is what makes the optimism safe. Without it a release
   * would rubber-band for a tick; with it *unbounded*, a peer moving the same
   * machine mid-drag would leave this client stuck on a placement the room
   * never agreed to, since the awaited rect would never arrive. */
  function rect(): MachineRect | null {
    if (localRect && (localHeld || performance.now() - localAt < LOCAL_GRACE_MS)) {
      return localRect;
    }
    localRect = null;
    return m ? normRect(m.rect) : null;
  }

  /** The chip presentation this machine is drawn with. */
  const spec = (): ChipSpec<HoistState> => chipFor(m?.kind);

  function stateAt(now: number): HoistState | null {
    if (!m) return null;
    return {
      m,
      now,
      spin,
      landAge,
      landV,
      winAge,
      dust,
      stale: performance.now() - lastMsgAt > STALE_MS,
    };
  }

  function zoneAt(cam: Camera, x: number, y: number): MachineZone | null {
    const r = rect();
    if (!r) return null;
    return chipZoneAt(cam, spec(), r, x, y);
  }

  function draw(
    ctx: CanvasRenderingContext2D,
    cam: Camera,
    now: number,
    dtSec: number,
    view: MachineView,
  ) {
    const r = rect();
    if (!m || !r) return;
    advance(Math.min(0.1, dtSec));
    const st = stateAt(now);
    if (!st) return;
    const sp = spec();
    card.onState(st, sp);

    const children = new Map<number, ElementSpec>();
    for (const c of view.children) children.set(c.id, c);
    renderChip({
      ctx,
      cam,
      spec: sp,
      rect: r,
      state: st,
      children,
      live: view.live,
      damage: view.damage,
      dots: view.dots,
      dtSec,
      hot,
    });
  }

  const hoist: Hoist = {
    onMachine,
    draw,
    state: () => m,
    rect,
    zoneAt,
    openPinout: () => card.showPinout(),
    setLocalRect: (r) => {
      localRect = normRect(r);
      localHeld = true;
      localAt = performance.now();
    },
    endLocalDrag: () => {
      localHeld = false;
      localAt = performance.now();
    },
    setHot: (v) => (hot = v),
    clear: () => {
      // The dev mock is a machine too: leaving it running would keep feeding
      // a hoist into a room that has none.
      mock?.stop();
      mock = null;
      m = null;
      localRect = null;
      localHeld = false;
      dust = [];
      spin = 0;
      landAge = Infinity;
      winAge = Infinity;
      lastMsgAt = -Infinity;
      card.hide();
    },
  };

  // ---- dev mock (see the file header): URL flag or console call.
  const params = new URLSearchParams(location.search);
  if (params.has('hoistmock')) mock = startMock(onMachine, params.get('hoistmock'));
  (window as unknown as { __hoistMock: (rect?: string | null) => void }).__hoistMock = (rect) => {
    mock?.stop();
    mock = startMock(onMachine, rect ?? null);
  };
  return hoist;
}

// --------------------------------------------------------------- dev mock

interface Mock {
  reset(): void;
  stop(): void;
}

/** Synthesised machine messages, so the package can be developed and reviewed
 * before (or without) the server half. The script is: lift on 9 V, cut to
 * reverse and let it slam into the floor (a hard landing, dust and all), then
 * close a lazy PD loop on the position — which climbs into the band, holds,
 * and wins.
 *
 * The same one-degree-of-freedom model and the same constants as the server
 * spec, integrated per animation frame with the motor current taken from
 * i = (V - K·omega) / R (L is negligible over a 16 ms frame). Development
 * scaffolding only: it is not the sim, it is not authoritative, and nothing
 * starts it unless a reviewer asks for it. */
function startMock(onMachine: (m: MachineMsg) => void, arg: string | null): Mock {
  // The server's own default footprint, so a reviewer running ?hoistmock
  // reviews the machine the server actually broadcasts.
  const rect = parseRect(arg) ?? ([46, 2, 62, 17] as [number, number, number, number]);
  const H = 0.4;
  const BAND: [number, number] = [0.3, 0.34];
  const LIM: [number, number] = [0.04, 0.36];
  const NEED = 5;
  const R = 2;
  const K = 0.25;
  const r = 0.02;
  const J = 7.8e-4;
  const b = 2e-4;
  const LOAD = 1.2 * 9.81 * r; // gravity torque at the drum
  const MID = (BAND[0] + BAND[1]) / 2;
  /** Mock integration substep, seconds. */
  const HM = 1 / 2000;

  let omega = 0;
  let y = 0;
  let hold = 0;
  let joules = 0;
  let landings = 0;
  let win = false;
  let t = 0;
  let last = performance.now();
  let raf = 0;
  let limTop = false;
  let limBot = true;

  const reset = () => {
    omega = 0;
    y = 0;
    hold = 0;
    landings = 0;
    joules = 0;
    win = false;
    t = 0;
  };

  const step = (now: number) => {
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    // Fixed 0.5 ms substeps: at one explicit-Euler step per animation frame
    // the PD loop below is unstable on a slow (or backgrounded) tab, and the
    // mock has to look the same at 30 fps as at 144.
    const steps = clamp(Math.round(dt / HM), 1, 400);
    let impact = 0;
    let i = 0;
    for (let s = 0; s < steps; s++) {
      t += HM;
      // Scripted drive, in volts across the motor.
      const volts =
        t < 1.6
          ? 9 // wide open: it flies up
          : t < 3.2
            ? -9 // cut and reverse: it comes down hard
            : clamp(240 * (MID - y) - 22 * r * omega, -9, 9); // lazy PD hold
      i = (volts - K * omega) / R; // L is negligible at 0.5 ms
      omega += (HM * (K * i - LOAD - b * omega)) / J;
      y += HM * r * omega;
      if (y < 0) {
        const hit = Math.abs(r * omega);
        impact = Math.max(impact, hit);
        if (hit > 0.8) landings++;
        y = 0;
        omega = 0;
      }
      if (y > H) {
        y = H;
        omega = Math.min(omega, 0);
      }
      const inBand = y >= BAND[0] && y <= BAND[1];
      hold = clamp(hold + (inBand ? HM : -3 * HM), 0, NEED);
      if (hold >= NEED) win = true;
      joules += Math.max(0, volts * i) * HM;
    }
    limTop = limTop ? y >= LIM[1] - 0.002 : y >= LIM[1];
    limBot = limBot ? y <= LIM[0] + 0.002 : y <= LIM[0];
    onMachine({
      id: 900,
      kind: 'hoist',
      rect,
      h: H,
      band: BAND,
      imax: 3.0, // mirrors the server's damage table, for review only
      y,
      vel: r * omega,
      i,
      hold,
      need: NEED,
      impact,
      landings,
      win,
      joules,
      wiper: clamp(1 - y / H, 0.02, 0.98),
      limt: limTop,
      limb: limBot,
      lim: LIM,
    });
    raf = requestAnimationFrame(step);
  };
  raf = requestAnimationFrame(step);
  return { reset, stop: () => cancelAnimationFrame(raf) };
}

/** "x0,y0,x1,y1" from the mock flag, when a reviewer wants it elsewhere. */
function parseRect(s: string | null): [number, number, number, number] | null {
  if (!s) return null;
  const n = s.split(',').map(Number);
  if (n.length !== 4 || n.some((v) => !Number.isFinite(v))) return null;
  return [n[0]!, n[1]!, n[2]!, n[3]!];
}
