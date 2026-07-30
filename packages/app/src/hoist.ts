// THE HOIST — client half of the game's first goal.
//
// A freight hoist stands in the world beside the demo vignettes: a crate on a
// platform in a vertical shaft, lifted by a DC motor whose two leads are real
// wire-able terminals. The player has to hold the crate inside a painted green
// band for five continuous seconds. A constant voltage lifts it but cannot
// hold it (voltage buys speed, not position), so the goal is only reachable by
// wiring feedback off the position sensor.
//
// NOTHING in here simulates the machine. The server integrates the crate from
// the SOLVER's motor current and broadcasts one "machine" message per tick:
//
//   {t, id, rect, h, band, y, vel, i, hold, need, impact, landings, win, joules}
//
// Every number this module draws or prints comes out of that message (design
// pillar: no faked electrical behaviour). The two exceptions are purely
// cosmetic and derived from the message's own values: the drum's rotation
// angle (a running integral of the server's `vel`) and the dust particles a
// landing throws (seeded from the server's `impact`).
//
// This module owns two things:
//   * the canvas CHROME (shaft, rails, drum, cable, crate, band, nameplate),
//     drawn BEHIND the schematic pass so the four fixture elements the server
//     owns (ids 900..903) stay visible, selectable, probe-able and wire-able —
//     they are ordinary elements and this file never draws or hides them;
//   * the GOAL CARD, an HTML overlay in the same visual language as the
//     control-panel windows (.pwin) in index.html.
//
// Dev mock: with no server half in reach, `?hoistmock` in the URL (or calling
// window.__hoistMock() from the console) synthesises machine messages from a
// scripted lift/fall so the chrome, the landing event and the win state can be
// exercised locally. It only ever calls the same onMachine() the socket does.

import type { Camera } from './render';
import { roundRectPath } from './panel';

/** Server -> client machine state, once per tick (protocol contract). */
export interface MachineMsg {
  /** Fixture id of the motor, i.e. which machine this is. */
  id: number;
  /** Footprint in GRID units: [x0, y0, x1, y1]. All chrome lives inside it. */
  rect: [number, number, number, number];
  /** Shaft height, metres. */
  h: number;
  /** Goal band [lo, hi], metres. */
  band: [number, number];
  /** Crate height, metres (integral of a solver unknown). */
  y: number;
  /** Crate velocity, m/s. */
  vel: number;
  /** Motor current into pin 0, amps (a solver unknown). */
  i: number;
  /** Accumulated in-band time, seconds. */
  hold: number;
  /** Hold time the goal needs, seconds. */
  need: number;
  /** Landing speed, m/s — non-zero only on the tick the crate hits the floor. */
  impact: number;
  /** Hard landings so far. */
  landings: number;
  win: boolean;
  /** Energy delivered by the player's sources, joules. */
  joules: number;
}

export interface Hoist {
  /** One machine message from the net layer (or the dev mock). */
  onMachine(m: MachineMsg): void;
  /** Once per animation frame, BEFORE the schematic pass: chrome + card. */
  draw(ctx: CanvasRenderingContext2D, cam: Camera, now: number, dtSec: number): void;
  /** Latest state, or null before the first message (tests/debug). */
  state(): MachineMsg | null;
}

// ------------------------------------------------------------------ layout
//
// Everything is a fraction of the server-supplied rect, so the client
// hardcodes no world geometry — only proportions. Vertical fractions are
// measured down from the rect's top edge (grid y grows downward, like px).

/** Shaft opening, as fractions of the rect width. */
const SHAFT_X0 = 0.06;
const SHAFT_X1 = 0.44;
/** Drum centre height. */
const DRUM_CY = 0.115;
/** Platform surface at y = h (top of travel) and y = 0 (floor). */
const TRAVEL_TOP = 0.28;
const TRAVEL_BOT = 0.76;
/** Top edge of the engraved nameplate strip. */
const PLATE_TOP = 0.8;
/** Crate height. */
const CRATE_H = 0.1;
/** Platform slab thickness. */
const SLAB_H = 0.022;

// So the crate's top at y = h sits at 0.18 of the rect height (clear of the
// drum, whose lowest point is 0.165) and the slab's bottom at y = 0 sits at
// 0.782, clear of the 0.80 nameplate. Nothing can reach an edge — and the
// whole pass is clipped to the rect anyway, which makes that structural
// rather than arithmetical.

/** Below this zoom (px per grid unit) the machine is one simplified block —
 * matches main.ts's LOD_FULL band for the schematic itself. */
const LOD_FULL = 6;
/** Smallest font worth drawing; below it, text is skipped entirely. */
const MIN_TEXT_PX = 7.5;

/** Drum radius, metres (machine constant): only used to turn the server's
 * `vel` into a rotation angle for the spokes. Never displayed. */
const DRUM_R = 0.02;
/** Crate weight, newtons (m·g with m = 1.2 kg): the cable goes slack below
 * it. Never displayed, just the cable's look. */
const CRATE_WEIGHT = 1.2 * 9.81;
/** Cable tension from motor torque, newtons: K·i / r. Never displayed. */
const tension = (i: number) => (0.25 * i) / DRUM_R;

/** Landing flash/shake duration, seconds. */
const LAND_S = 0.45;
/** Impact speed that saturates the landing effects, m/s. */
const LAND_FULL = 2.0;
/** Speed that saturates the motion streaks, m/s. */
const VEL_FULL = 0.35;
/** No machine message for this long = the link is gone, not the machine. */
const STALE_MS = 1500;

const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

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

const div = (cls: string, parent?: HTMLElement): HTMLDivElement => {
  const el = document.createElement('div');
  el.className = cls;
  parent?.append(el);
  return el;
};

interface Card {
  /** `stale` = no machine message recently: the numbers are frozen, and the
   * card says so instead of passing old values off as live. */
  onState(m: MachineMsg, now: number, stale: boolean): void;
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
  hint.textContent =
    'M+/M− drive the drum · SENSE-W reads height (12.5 mV/mm) · a constant\n' +
    'voltage cannot hold a position — close the loop.';

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
    onState(m: MachineMsg, now: number, stale: boolean) {
      if (!shown) {
        shown = true;
        el.style.display = 'block';
      }
      const flip = m.win !== lastWin || stale !== lastStale;
      if (!flip && now - lastAt < UPDATE_MS) return;
      lastAt = now;
      lastWin = m.win;
      lastStale = stale;
      el.classList.toggle('stale', stale);

      title.textContent = `CRATE IN BAND — HOLD ${m.need.toFixed(1)} s`;
      const frac = m.need > 0 ? clamp(m.hold / m.need, 0, 1) : 0;
      fill.style.width = `${(frac * 100).toFixed(1)}%`;
      barTxt.textContent = `${m.hold.toFixed(2)} / ${m.need.toFixed(2)} s`;
      const inBand = m.y >= m.band[0] && m.y <= m.band[1];
      badge.textContent = stale ? 'NO LINK' : m.win ? 'HELD' : inBand ? 'IN BAND' : 'OUT';
      badge.className = `goal-badge${stale ? '' : m.win ? ' win' : inBand ? ' in' : ''}`;

      vHeight.textContent = `${(m.y * 1000).toFixed(1)} mm`;
      vVel.textContent = `${(m.vel * 1000).toFixed(0)} mm/s`;
      vCur.textContent = fmtSI(m.i, 'A');
      vJoule.textContent = `${m.joules.toFixed(1)} J`;
      vLand.textContent = String(m.landings);
      vBand.textContent = `${(m.band[0] * 1000).toFixed(0)}–${(m.band[1] * 1000).toFixed(0)} mm`;

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

// ------------------------------------------------------------------- dust
//
// Particles live in shaft-normalised space (u across the shaft, w up from the
// platform surface, both in "shaft widths"), so a zoom or a pan never moves
// them relative to the machine.

interface Dust {
  u: number;
  w: number;
  du: number;
  dw: number;
  age: number;
  life: number;
}

// ------------------------------------------------------------------ module

export function createHoist(root: HTMLElement, opts: { reset: () => void }): Hoist {
  const card = buildCard(root, () => {
    if (mock) mock.reset();
    else opts.reset();
  });

  let m: MachineMsg | null = null;
  /** Cosmetic drum angle: the integral of the server's own `vel`. */
  let spin = 0;
  /** Seconds since the last landing, and how hard it was. */
  let landAge = Infinity;
  let landV = 0;
  let dust: Dust[] = [];
  /** Seconds since `win` flipped true (drives the celebration flash). */
  let winAge = Infinity;
  let mock: Mock | null = null;
  /** Wall clock of the last message: the card must not pass a frozen state
   * off as live if the socket drops. */
  let lastMsgAt = -Infinity;

  function onMachine(next: MachineMsg) {
    lastMsgAt = performance.now();
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

  function draw(ctx: CanvasRenderingContext2D, cam: Camera, now: number, dtSec: number) {
    const mm = m;
    if (!mm) return;
    card.onState(mm, now, performance.now() - lastMsgAt > STALE_MS);
    advance(Math.min(0.1, dtSec));

    // Normalised corners: a rect sent as [x1, y1, x0, y0] still stands up the
    // right way instead of silently drawing nothing.
    const gx0 = Math.min(mm.rect[0], mm.rect[2]);
    const gx1 = Math.max(mm.rect[0], mm.rect[2]);
    const gy0 = Math.min(mm.rect[1], mm.rect[3]);
    const gy1 = Math.max(mm.rect[1], mm.rect[3]);
    const X0 = cam.ox + gx0 * cam.scale;
    const X1 = cam.ox + gx1 * cam.scale;
    const Y0 = cam.oy + gy0 * cam.scale;
    const Y1 = cam.oy + gy1 * cam.scale;
    const W = X1 - X0;
    const H = Y1 - Y0;
    if (!Number.isFinite(W) || !Number.isFinite(H)) return;
    if (!(W > 2 && H > 2)) return; // degenerate or absurdly zoomed out
    if (X1 < 0 || Y1 < 0 || X0 > window.innerWidth || Y0 > window.innerHeight) return;

    ctx.save();
    // Structural guarantee: no chrome pixel can land outside the server's rect.
    roundRectPath(ctx, X0, Y0, W, H, Math.min(14, cam.scale * 0.4));
    ctx.clip();

    const detail = cam.scale >= LOD_FULL;
    const sx0 = X0 + SHAFT_X0 * W;
    const sx1 = X0 + SHAFT_X1 * W;
    const sw = sx1 - sx0;
    const yTop = Y0 + TRAVEL_TOP * H;
    const yBot = Y0 + TRAVEL_BOT * H;
    /** Platform surface in px for a crate height in metres. */
    const pyOf = (y: number) => yBot - (clamp(y, 0, mm.h) / Math.max(1e-9, mm.h)) * (yBot - yTop);

    // ---- cabinet
    const grad = ctx.createLinearGradient(X0, Y0, X0, Y1);
    grad.addColorStop(0, '#232830');
    grad.addColorStop(1, '#15181d');
    ctx.fillStyle = grad;
    ctx.fillRect(X0, Y0, W, H);
    // Inset by half the line width so even the outline is inside the rect.
    const lw = Math.min(Math.max(1, cam.scale * 0.06), Math.min(W, H) / 4);
    ctx.strokeStyle = '#3c4653';
    ctx.lineWidth = lw;
    roundRectPath(ctx, X0 + lw / 2, Y0 + lw / 2, W - lw, H - lw, Math.min(14, cam.scale * 0.4));
    ctx.stroke();

    // ---- shaft recess
    ctx.fillStyle = '#0c0f13';
    ctx.fillRect(sx0, Y0 + 0.05 * H, sw, PLATE_TOP * H - 0.05 * H);

    const landK = landAge < LAND_S ? (1 - landAge / LAND_S) * clamp(landV / LAND_FULL, 0.2, 1) : 0;
    const py = pyOf(mm.y);

    if (!detail) {
      // Far zoom: one legible block — band, crate, no text, no particles.
      drawBand(ctx, mm, sx0, sw, pyOf, now, winAge, false);
      ctx.fillStyle = '#c8a05a';
      const ch = CRATE_H * H;
      ctx.fillRect(sx0 + sw * 0.14, py - ch, sw * 0.72, ch);
      ctx.restore();
      return;
    }

    // ---- guide rails
    const railW = Math.max(1, sw * 0.05);
    ctx.fillStyle = '#2a3240';
    ctx.fillRect(sx0 + sw * 0.06, Y0 + 0.08 * H, railW, PLATE_TOP * H - 0.09 * H);
    ctx.fillRect(sx1 - sw * 0.06 - railW, Y0 + 0.08 * H, railW, PLATE_TOP * H - 0.09 * H);
    ctx.fillStyle = '#4d5b6d';
    ctx.fillRect(sx0 + sw * 0.06, Y0 + 0.08 * H, Math.max(0.5, railW * 0.35), PLATE_TOP * H - 0.09 * H);
    ctx.fillRect(sx1 - sw * 0.06 - railW, Y0 + 0.08 * H, Math.max(0.5, railW * 0.35), PLATE_TOP * H - 0.09 * H);

    // ---- travel scale (0 .. h in mm, from the message)
    const tickFont = Math.round(clamp(H * 0.035, 4, 13));
    if (tickFont >= MIN_TEXT_PX) {
      ctx.font = `${tickFont}px ui-monospace, monospace`;
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.strokeStyle = '#3a4552';
      ctx.fillStyle = '#637a86';
      ctx.lineWidth = 1;
      for (let k = 0; k <= 4; k++) {
        const yv = (mm.h * k) / 4;
        const ty = Math.round(pyOf(yv)) + 0.5;
        ctx.beginPath();
        ctx.moveTo(sx1 + sw * 0.04, ty);
        ctx.lineTo(sx1 + sw * 0.12, ty);
        ctx.stroke();
        ctx.fillText(`${Math.round(yv * 1000)}`, sx1 + sw * 0.15, ty);
      }
    }

    drawBand(ctx, mm, sx0, sw, pyOf, now, winAge, tickFont >= MIN_TEXT_PX);

    // ---- drum + cable
    const drumCY = Y0 + DRUM_CY * H;
    const drumR = Math.min(0.05 * H, 0.1 * W);
    const cx = (sx0 + sx1) / 2;
    const T = tension(mm.i);
    const slack = clamp((CRATE_WEIGHT - T) / CRATE_WEIGHT, 0, 1);
    const cableW = Math.max(1, sw * (0.03 + 0.03 * clamp(T / (CRATE_WEIGHT * 2), 0, 1)));
    const crateTop = py - CRATE_H * H;
    ctx.strokeStyle = slack > 0.5 ? '#6d757f' : '#b9c6d2';
    ctx.lineWidth = cableW;
    ctx.beginPath();
    ctx.moveTo(cx, drumCY);
    if (slack > 0.05) {
      // A cable that is not carrying the crate's weight (motor torque below
      // m·g·r, i.e. the crate is on its way down) bows and dulls; a cable that
      // is carrying it is a taut bright line. Tension comes from the message's
      // own current — it is a cue, not a number.
      const bow = slack * sw * 0.08;
      ctx.quadraticCurveTo(cx + bow, (drumCY + crateTop) / 2, cx, crateTop);
    } else {
      ctx.lineTo(cx, crateTop);
    }
    ctx.stroke();

    ctx.fillStyle = '#2b333d';
    ctx.beginPath();
    ctx.arc(cx, drumCY, drumR, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = '#59677a';
    ctx.lineWidth = Math.max(1, drumR * 0.14);
    ctx.stroke();
    ctx.strokeStyle = '#7f8fa2';
    ctx.lineWidth = Math.max(1, drumR * 0.1);
    for (let k = 0; k < 4; k++) {
      const a = spin + (k * Math.PI) / 4;
      ctx.beginPath();
      ctx.moveTo(cx - Math.cos(a) * drumR * 0.8, drumCY - Math.sin(a) * drumR * 0.8);
      ctx.lineTo(cx + Math.cos(a) * drumR * 0.8, drumCY + Math.sin(a) * drumR * 0.8);
      ctx.stroke();
    }

    // ---- motion streaks (speed straight off the message's `vel`)
    const vk = clamp(Math.abs(mm.vel) / VEL_FULL, 0, 1);
    if (vk > 0.04) {
      const dir = mm.vel > 0 ? 1 : -1; // +vel = rising = up the screen
      ctx.strokeStyle = '#9fd8ff';
      ctx.lineWidth = Math.max(0.7, sw * 0.02);
      ctx.globalAlpha = 0.1 + 0.35 * vk;
      const len = vk * H * 0.09;
      for (let k = 0; k < 4; k++) {
        const sxk = sx0 + sw * (0.2 + 0.2 * k);
        // Streaks trail the crate: below the platform when rising, above the
        // crate's roof when falling (otherwise the crate hides them).
        const y0 = dir > 0 ? py + 2 + k : py - CRATE_H * H - 2 - k;
        ctx.beginPath();
        ctx.moveTo(sxk, y0);
        ctx.lineTo(sxk, y0 + dir * len);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    }

    // ---- crate + platform (with motion blur and the landing shake)
    // Guard the phase, not the amplitude: landAge is Infinity until the first
    // landing, and Math.sin(Infinity) is NaN — which `* landK` does NOT clear,
    // because 0 * NaN is NaN. An NaN here reached the crate's x and threw out
    // of createLinearGradient, killing the whole frame.
    const shake =
      landK > 0 ? landK * Math.min(sw * 0.05, 4) * Math.sin(landAge * 90) : 0;
    const cw = sw * 0.72;
    const ch = CRATE_H * H;
    const slabH = Math.max(2, SLAB_H * H);
    if (vk > 0.15) {
      ctx.globalAlpha = 0.18;
      for (let k = 1; k <= 2; k++) {
        const off = -(mm.vel > 0 ? -1 : 1) * vk * H * 0.02 * k;
        drawCrate(ctx, sx0 + (sw - cw) / 2, py + off, cw, ch, false, 0);
      }
      ctx.globalAlpha = 1;
    }
    // Platform slab
    ctx.fillStyle = landK > 0 ? '#ffd9a0' : '#5b6a7a';
    ctx.fillRect(sx0 + sw * 0.06 + shake, py, sw * 0.88, slabH);
    if (landK > 0) {
      ctx.globalAlpha = landK;
      ctx.fillStyle = '#fff3d0';
      ctx.fillRect(sx0 + sw * 0.04 + shake, py - slabH * 0.4, sw * 0.92, slabH * 1.8);
      ctx.globalAlpha = 1;
    }
    drawCrate(ctx, sx0 + (sw - cw) / 2 + shake, py, cw, ch, tickFont >= MIN_TEXT_PX, landK);

    // ---- dust: puffs stay inside the shaft, never over the faceplate
    if (dust.length > 0) {
      ctx.fillStyle = '#c9b79a';
      const floorY = pyOf(0);
      for (const d of dust) {
        const f = d.age / d.life;
        const a = (1 - f) * 0.5;
        if (a <= 0) continue;
        const r = Math.max(0.8, Math.min(sw * (0.03 + 0.05 * f), H * 0.02));
        if (sw < 2 * r + 1 || floorY - yTop < 2 * r + 1) continue;
        ctx.globalAlpha = a;
        ctx.beginPath();
        ctx.arc(
          clamp(sx0 + d.u * sw, sx0 + r, sx0 + sw - r),
          clamp(floorY - d.w * sw, yTop + r, floorY - r),
          r,
          0,
          Math.PI * 2,
        );
        ctx.fill();
      }
      ctx.globalAlpha = 1;
    }

    // ---- floor
    ctx.fillStyle = '#2a323c';
    ctx.fillRect(sx0, pyOf(0) + Math.max(2, SLAB_H * H), sw, Math.max(1.5, H * 0.012));

    // ---- faceplate nameplate: terminals + printed constants
    drawPlate(ctx, X0, Y0, W, H, mm);

    ctx.restore();
  }

  /** The green band, from the message's own [lo, hi]; flashes on a win. */
  function drawBand(
    ctx: CanvasRenderingContext2D,
    mm: MachineMsg,
    sx0: number,
    sw: number,
    pyOf: (y: number) => number,
    now: number,
    wAge: number,
    withText: boolean,
  ) {
    const top = pyOf(mm.band[1]);
    const bot = pyOf(mm.band[0]);
    const flash = wAge === Infinity ? 0 : (0.55 + 0.45 * Math.sin(now / 90)) * (wAge < 2 ? 1 : 0.45);
    ctx.fillStyle = `rgba(96, 255, 168, ${(0.1 + 0.28 * flash).toFixed(3)})`;
    ctx.fillRect(sx0, top, sw, bot - top);
    ctx.strokeStyle = `rgba(125, 255, 176, ${(0.55 + 0.45 * flash).toFixed(3)})`;
    ctx.lineWidth = Math.max(1, sw * 0.03);
    ctx.beginPath();
    ctx.moveTo(sx0, Math.round(top) + 0.5);
    ctx.lineTo(sx0 + sw, Math.round(top) + 0.5);
    ctx.moveTo(sx0, Math.round(bot) + 0.5);
    ctx.lineTo(sx0 + sw, Math.round(bot) + 0.5);
    ctx.stroke();
    if (withText && bot - top > 9) {
      ctx.fillStyle = '#7dffb0';
      ctx.font = `${Math.round(clamp((bot - top) * 0.5, 6, 12))}px ui-monospace, monospace`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText('BAND', sx0 + sw / 2, (top + bot) / 2);
    }
  }

  function drawCrate(
    ctx: CanvasRenderingContext2D,
    x: number,
    surfaceY: number,
    w: number,
    h: number,
    withText: boolean,
    landK: number,
  ) {
    const y = surfaceY - h;
    const g = ctx.createLinearGradient(x, y, x + w, y + h);
    g.addColorStop(0, landK > 0 ? '#e8b877' : '#c8a05a');
    g.addColorStop(1, '#8a6c38');
    ctx.fillStyle = g;
    ctx.fillRect(x, y, w, h);
    ctx.strokeStyle = '#5d4720';
    ctx.lineWidth = Math.max(1, w * 0.03);
    ctx.strokeRect(x, y, w, h);
    ctx.beginPath(); // slats
    ctx.moveTo(x, y);
    ctx.lineTo(x + w, y + h);
    ctx.moveTo(x + w, y);
    ctx.lineTo(x, y + h);
    ctx.lineWidth = Math.max(0.6, w * 0.02);
    ctx.strokeStyle = '#6f5527';
    ctx.stroke();
    if (withText && h > 12 && w > 26) {
      ctx.fillStyle = '#3a2c12';
      ctx.font = `${Math.round(clamp(h * 0.3, 6, 12))}px ui-monospace, monospace`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText('1.2 kg', x + w / 2, y + h / 2);
    }
  }

  /** The engraved nameplate strip: terminal names and the printed constants a
   * player needs to design against. Text is skipped when it would be mush. */
  function drawPlate(
    ctx: CanvasRenderingContext2D,
    X0: number,
    Y0: number,
    W: number,
    H: number,
    mm: MachineMsg,
  ) {
    const py0 = Y0 + PLATE_TOP * H;
    const ph = H - PLATE_TOP * H;
    const boxX = X0 + W * 0.03;
    const boxW = W * 0.94;
    ctx.fillStyle = '#1b2027';
    ctx.fillRect(boxX, py0, boxW, ph * 0.86);
    ctx.strokeStyle = '#39424f';
    ctx.lineWidth = 1;
    if (boxW > 2) ctx.strokeRect(boxX + 0.5, py0 + 0.5, boxW - 1, ph * 0.86 - 1);

    // Engraved lines, most important first: whatever does not fit the plate at
    // a legible size is dropped rather than drawn as a clipped half-line.
    const lines: [string, string][] = [
      ['#8ea3b1', 'M+  M−   |   SENSE A W B   |   LIM-TOP   |   LIM-BOT'],
      ['#6d7d89', 'R=2Ω  L=1.5mH  K=0.25 V·s/rad'],
      ['#6d7d89', 'SENSE: 12.5 mV/mm'],
    ];
    // Fit vertically (row height) AND horizontally (the longest line inside
    // the plate), then drop the text altogether if that lands below legible.
    const lx = X0 + W * 0.055;
    const textW = boxX + boxW - lx - W * 0.02;
    let font = Math.round(clamp(ph * 0.19, 4, 13));
    ctx.font = `${font}px ui-monospace, monospace`;
    let widest = 0;
    for (const [, text] of lines) widest = Math.max(widest, ctx.measureText(text).width);
    if (widest > textW && widest > 0) font = Math.floor(font * (textW / widest));
    const step = font * 1.32;
    const room = Math.floor((ph * 0.86 - ph * 0.06) / step);
    if (font >= MIN_TEXT_PX && room >= 1) {
      ctx.font = `${font}px ui-monospace, monospace`;
      ctx.textAlign = 'left';
      ctx.textBaseline = 'top';
      let ly = py0 + ph * 0.06;
      for (const [color, text] of lines.slice(0, room)) {
        ctx.fillStyle = color;
        ctx.fillText(text, lx, ly);
        ly += step;
      }
    }

    // Machine name + objective, engraved along the top of the cabinet clear of
    // the shaft; shortened, then dropped, when the rect is too narrow for it.
    const tf = Math.round(clamp(H * 0.045, 4, 18));
    if (tf >= MIN_TEXT_PX) {
      ctx.font = `${tf}px ui-monospace, monospace`;
      ctx.textAlign = 'right';
      ctx.textBaseline = 'top';
      ctx.fillStyle = '#8b9caa';
      const long = `FREIGHT HOIST — CRATE IN BAND · HOLD ${mm.need.toFixed(1)} s`;
      const room2 = W * (1 - SHAFT_X1 - 0.05);
      const text =
        ctx.measureText(long).width <= room2
          ? long
          : ctx.measureText('FREIGHT HOIST').width <= room2
            ? 'FREIGHT HOIST'
            : '';
      if (text) ctx.fillText(text, X0 + W * 0.96, Y0 + H * 0.03);
    }
  }

  const hoist: Hoist = { onMachine, draw, state: () => m };

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

/** Synthesised machine messages, so the chrome can be developed and reviewed
 * before the server half lands. The script is: lift on 9 V, cut to reverse and
 * let it slam into the floor (a hard landing, dust and all), then close a lazy
 * PD loop on the position — which climbs into the band, holds, and wins.
 *
 * The same one-degree-of-freedom model and the same constants as the server
 * spec, integrated per animation frame with the motor current taken from
 * i = (V - K·omega) / R (L is negligible over a 16 ms frame). Development
 * scaffolding only: it is not the sim, it is not authoritative, and nothing
 * starts it unless a reviewer asks for it. */
function startMock(onMachine: (m: MachineMsg) => void, arg: string | null): Mock {
  const rect = parseRect(arg) ?? ([46, 4, 66, 34] as [number, number, number, number]);
  const H = 0.4;
  const BAND: [number, number] = [0.3, 0.34];
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
    onMachine({
      id: 900,
      rect,
      h: H,
      band: BAND,
      y,
      vel: r * omega,
      i,
      hold,
      need: NEED,
      impact,
      landings,
      win,
      joules,
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
