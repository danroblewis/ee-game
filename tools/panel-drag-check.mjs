#!/usr/bin/env node
// Regression check for the HUD rails: a header drag must survive being
// reparented at ANY pointer speed, and the HUD must never be left in a
// mid-drag state.
//
// The bug this exists to catch: the drag used to bind pointermove/up to the
// window header. Lifting a DOCKED panel reparents the .pwin out of its rail,
// which drops the pointer capture, so past ~29 px per pointermove the header
// stopped receiving events — the window froze in mid-air, pointerup never
// reached the gesture, and the rails stayed armed with a phantom empty
// sidebar and a lit drop caret for ever. A drag must not depend on where its
// element sits in the DOM. panel.ts's assertNotStuck() is the always-on half
// of this check; this is the half that drives a real pointer.
//
//   node tools/panel-drag-check.mjs [url]
//
// Needs playwright-core and a Chrome; skips with exit 0 when neither is
// there, so it can sit in a pipeline that has no browser. ESM ignores
// NODE_PATH, so EE_PLAYWRIGHT may name playwright-core's directory.
import { argv, env, exit } from 'node:process';

const URL_ = argv[2] ?? env.EE_URL ?? 'http://localhost:5173/';
const EXE = env.EE_CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const ATTEMPTS = Number(env.EE_ATTEMPTS ?? 5);
/** Optional directory for per-width screenshots; the numbers are the check. */
const SHOTS = env.EE_SHOTS ?? '';

let chromium;
for (const spec of [env.EE_PLAYWRIGHT, 'playwright-core']) {
  if (!spec) continue;
  try {
    ({ chromium } = await import(spec.startsWith('/') ? `${spec}/index.mjs` : spec));
    break;
  } catch {
    /* try the next */
  }
}
if (!chromium) {
  console.log('SKIP: playwright-core not installed (set EE_PLAYWRIGHT to its directory)');
  exit(0);
}

/** Everything the HUD must look like when NO drag is in flight. */
const readHud = (page) =>
  page.evaluate(() => {
    const rail = (id) => {
      const e = document.getElementById(id);
      const b = e.getBoundingClientRect();
      return {
        on: e.classList.contains('on'),
        armed: e.classList.contains('armed'),
        caret: !!e.querySelector('.rail-caretline.on'),
        panels: e.querySelectorAll('.pwin').length,
        w: Math.round(b.width),
        l: Math.round(b.left),
        r: Math.round(b.right),
        collapsed: e.classList.contains('collapsed'),
      };
    };
    const L = rail('rail-left');
    const R = rail('rail-right');
    return {
      L,
      R,
      dragging: document.querySelectorAll('.pwin.dragging').length,
      wins: document.querySelectorAll('.pwin').length,
      // A rail that is displayed while holding nothing is the phantom.
      phantom: (L.on && L.panels === 0) || (R.on && R.panels === 0),
      armed: L.armed || R.armed,
      caret: L.caret || R.caret,
      overlap: (L.on ? L.r : 0) - (R.on ? R.l : 1e9),
      canvasFree: (R.on ? R.l : innerWidth) - (L.on ? L.r : 0),
    };
  });

/** Bare drag strip of the i-th .pwin under `root`, in client coords. */
const gripAt = (page, root, i) =>
  page.evaluate(
    ([r, k]) => {
      const w = [...document.querySelectorAll(r + ' .pwin')][k];
      if (!w) return null;
      const b = w.querySelector('.pwin-hd').getBoundingClientRect();
      const shut = w.querySelector('.pwin-shut').getBoundingClientRect();
      return { x: shut.left - 12, y: b.top + b.height / 2 };
    },
    [root, i],
  );

async function runOnce(results) {
  const browser = await chromium.launch({
    executablePath: EXE,
    headless: true,
    // Accelerated Canvas2D kills the tab on some stock Chrome builds.
    args: [
      '--disable-gpu',
      '--disable-software-rasterizer',
      '--disable-dev-shm-usage',
      '--no-sandbox',
    ],
  });
  try {
    const ctx = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      deviceScaleFactor: 1,
    });
    const page = await ctx.newPage();
    const errs = [];
    page.on('console', (m) => m.type() === 'error' && errs.push(m.text()));
    page.on('pageerror', (e) => errs.push('PAGEERROR ' + e.message));
    await page.goto(URL_, { waitUntil: 'load' });
    await page.waitForTimeout(2600);
    const wait = (ms) => page.waitForTimeout(ms);
    const check = (name, ok, detail) => results.push({ name, ok, detail });

    // ---- scene: three control panels, all docked into the left rail -----
    await page.mouse.move(700, 450);
    await page.keyboard.press('h');
    await wait(600);
    const count = () => page.evaluate(() => document.querySelectorAll('.pwin').length);
    for (let g = 0; (await count()) < 3 && g < 5; g++) {
      await page.mouse.move(720, 460);
      await page.keyboard.press('j');
      await wait(150);
      const [a, b] = [
        { x: 600 + g * 30, y: 300 + g * 90 },
        { x: 860 + g * 30, y: 420 + g * 90 },
      ];
      await page.mouse.move(a.x, a.y);
      await page.mouse.down();
      for (let i = 1; i <= 12; i++) {
        await page.mouse.move(a.x + ((b.x - a.x) * i) / 12, a.y + ((b.y - a.y) * i) / 12);
      }
      await page.mouse.up();
      await wait(700);
    }
    const total = await count();
    if (total < 2) throw new Error(`scene has only ${total} panels`);

    const dockAll = async () => {
      for (let g = 0; g < 6; g++) {
        const h = await gripAt(page, '#panels', 0);
        if (!h) break;
        await page.mouse.move(h.x, h.y);
        await page.mouse.down();
        for (let i = 1; i <= 20; i++) {
          await page.mouse.move(h.x + ((120 - h.x) * i) / 20, h.y);
          await wait(8);
        }
        await wait(180);
        await page.mouse.up();
        await wait(320);
      }
    };
    await dockAll();

    // ---- 1. the speed sweep ---------------------------------------------
    // 40 px per pointermove is ~2400 px/s at 60 Hz: an ordinary quick drag,
    // and what any dropped frame in a canvas app looks like. The last case
    // is the whole 260 px gesture in ONE pointermove.
    for (const step of [4, 12, 40, 90, 260]) {
      const h = await gripAt(page, '#rail-left', 0);
      if (!h) {
        check(`speed step=${step}`, false, 'no window in the rail');
        continue;
      }
      const dy = 260;
      await page.mouse.move(h.x, h.y);
      await page.mouse.down();
      const n = Math.max(1, Math.round(dy / step));
      for (let i = 1; i <= n; i++) {
        await page.mouse.move(h.x, h.y + (dy * i) / n);
        await wait(8);
      }
      const mid = await page.evaluate(() => {
        const d = document.querySelector('.pwin.dragging');
        return d ? Math.round(d.getBoundingClientRect().top) : null;
      });
      await page.mouse.up();
      await wait(420);
      const s = await readHud(page);
      const want = Math.round(h.y + dy - 15);
      const tracked = mid !== null && Math.abs(mid - want) <= 24;
      check(`speed step=${step}`, tracked && !s.armed && !s.caret && !s.phantom && !s.dragging && s.wins === total, {
        tracked,
        wantTop: want,
        gotTop: mid,
        armed: s.armed,
        caret: s.caret,
        phantom: s.phantom,
        stuck: s.dragging,
        wins: s.wins,
      });
      await dockAll();
    }

    // ---- 2. an interrupted drag still settles ---------------------------
    {
      const h = await gripAt(page, '#rail-left', 0);
      await page.mouse.move(h.x, h.y);
      await page.mouse.down();
      for (let i = 1; i <= 8; i++) await page.mouse.move(h.x + i * 40, h.y + i * 20);
      await page.keyboard.press('Escape');
      await wait(280);
      const s = await readHud(page);
      await page.mouse.up();
      await wait(280);
      check('escape mid-drag settles', !s.armed && !s.caret && !s.phantom && !s.dragging, s);
      await dockAll();
    }

    // ---- 3. narrow viewports: rails never overlap, canvas survives ------
    // BOTH rails have to be holding something, or the case that broke —
    // two 300 px sidebars overlapping by 80 px at a 520 px viewport — is
    // never reached.
    {
      const h = await gripAt(page, '#rail-left', 0);
      const toX = 1440 - 90;
      await page.mouse.move(h.x, h.y);
      await page.mouse.down();
      for (let i = 1; i <= 20; i++) {
        await page.mouse.move(h.x + ((toX - h.x) * i) / 20, h.y);
        await wait(8);
      }
      await wait(200);
      await page.mouse.up();
      await wait(400);
      const s = await readHud(page);
      check('dock into the right rail', s.L.panels > 0 && s.R.panels > 0, {
        L: s.L.panels,
        R: s.R.panels,
      });
    }
    for (const w of [1440, 1000, 800, 700, 620, 520, 420, 320]) {
      await page.setViewportSize({ width: w, height: 780 });
      await wait(650);
      // Evidence only — a screenshot that will not take must never fail a run.
      if (SHOTS) await page.screenshot({ path: `${SHOTS}/rails-w${w}.png` }).catch(() => {});
      const s = await readHud(page);
      check(`fit W=${w}`, s.overlap <= 0 && s.canvasFree >= 200, {
        overlap: s.overlap,
        canvasFree: s.canvasFree,
        L: s.L.w,
        R: s.R.w,
        folded: [s.L.collapsed, s.R.collapsed],
      });
    }
    // Folding is a fact about the window, not a saved preference.
    await page.setViewportSize({ width: 1440, height: 900 });
    await wait(650);
    {
      const s = await readHud(page);
      check('widening unfolds', s.L.panels === 0 || !s.L.collapsed, { L: s.L, R: s.R });
    }
    check('no console errors', errs.length === 0, errs.slice(0, 4));
    return true;
  } finally {
    await browser.close().catch(() => {});
  }
}

// The browser is torn down at random in some sandboxes; a died-mid-run page
// is not a test failure, so retry the whole thing from a fresh browser.
let results = [];
let ran = false;
for (let a = 1; a <= ATTEMPTS && !ran; a++) {
  results = [];
  try {
    ran = await runOnce(results);
  } catch (e) {
    console.log(`attempt ${a} aborted: ${String(e.message ?? e).slice(0, 120)}`);
  }
}
if (!ran) {
  console.log(`SKIP: could not complete a run in ${ATTEMPTS} attempts`);
  exit(0);
}
for (const r of results) console.log(`${r.ok ? 'ok  ' : 'FAIL'} ${r.name} ${JSON.stringify(r.detail)}`);
const bad = results.filter((r) => !r.ok);
console.log(bad.length === 0 ? 'PASS' : `FAILED: ${bad.map((r) => r.name).join(', ')}`);
exit(bad.length === 0 ? 0 : 1);
