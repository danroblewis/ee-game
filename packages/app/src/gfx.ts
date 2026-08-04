// Graphics configuration: what the schematic LOOKS like, and nothing else.
//
// Everything here is PER-PLAYER DISPLAY STATE. It never touches the document,
// never reaches the wire, and never reaches the solver — two players in one
// room can disagree about every setting in this file and still be looking at
// the same circuit. That is the whole reason it lives apart from `Panel`,
// `Scope` and the rest of the room's shared furniture: those are things the
// room HAS, these are things a player PREFERS.
//
// Persisted in localStorage under one key, so a preference survives a reload
// and a room switch. Deliberately NOT room-scoped: if you like the current
// dots off, you like them off everywhere.

/** Everything the renderer will read. Add a field, add a row, that is all. */
export interface Gfx {
  /** Draw the yellow current dots at all. */
  dots: boolean;
  /** Multiplier on dot travel speed. 1 = the measured default. */
  dotSpeed: number;
  /** Multiplier on dot radius. */
  dotSize: number;
  /** Dot opacity, 0..1. */
  dotAlpha: number;
  /** Colour wires and pins by voltage. */
  voltageColor: boolean;
}

export const GFX_DEFAULT: Gfx = {
  dots: true,
  dotSpeed: 1,
  dotSize: 1,
  dotAlpha: 1,
  voltageColor: true,
};

const KEY = 'ee.gfx';

/** The live settings object. The renderer holds this exact reference and
 *  reads it every frame, so a change takes effect on the next one with no
 *  plumbing, no event, and no re-render call. */
export const gfx: Gfx = { ...GFX_DEFAULT };

const clamp = (v: number, lo: number, hi: number, d: number) =>
  Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : d;

/** Load from localStorage, field by field, clamping as we go. A hand-edited
 *  or half-written value must not be able to make the schematic unreadable —
 *  the worst a bad key can do is fall back to the default. */
export function loadGfx(): void {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(KEY) ?? '{}');
  } catch {
    raw = {};
  }
  const o = (raw ?? {}) as Partial<Record<keyof Gfx, unknown>>;
  gfx.dots = typeof o.dots === 'boolean' ? o.dots : GFX_DEFAULT.dots;
  gfx.voltageColor =
    typeof o.voltageColor === 'boolean' ? o.voltageColor : GFX_DEFAULT.voltageColor;
  gfx.dotSpeed = clamp(Number(o.dotSpeed), 0.1, 4, GFX_DEFAULT.dotSpeed);
  gfx.dotSize = clamp(Number(o.dotSize), 0.4, 3, GFX_DEFAULT.dotSize);
  gfx.dotAlpha = clamp(Number(o.dotAlpha), 0.1, 1, GFX_DEFAULT.dotAlpha);
}

function saveGfx(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(gfx));
  } catch {
    /* private mode, a full quota — a lost preference is not worth a throw */
  }
}

// ------------------------------------------------------------------ the UI

export interface GfxUI {
  toggle(): void;
  close(): void;
  isOpen(): boolean;
  /** Does this event belong to the dialog? Used to suppress part hotkeys. */
  owns(t: EventTarget | null): boolean;
}

/** The Graphics dialog. Opened with ⇧G, closed with ⇧G, Escape or its ×.
 *
 *  Every control applies IMMEDIATELY and the dialog does not cover the middle
 *  of the sheet, because the only way to judge "is that the right dot speed"
 *  is to watch the dots while you drag the slider. There is no OK button for
 *  the same reason: there is nothing to confirm. */
export function createGfx(host: HTMLElement): GfxUI {
  loadGfx();

  const el = document.createElement('div');
  el.className = 'pwin gfxwin';
  el.style.display = 'none';

  const hd = document.createElement('div');
  hd.className = 'pwin-hd';
  const title = document.createElement('span');
  title.className = 'pwin-title';
  title.textContent = 'GRAPHICS';
  const x = document.createElement('button');
  x.className = 'pwin-x';
  x.textContent = '×';
  x.title = 'close (⇧G or Esc)';
  x.onclick = () => close();
  hd.append(title, x);

  const body = document.createElement('div');
  body.className = 'pwin-body';

  const row = (label: string) => {
    const r = document.createElement('div');
    r.className = 'prow';
    const l = document.createElement('span');
    l.className = 'prow-label';
    l.textContent = label;
    const c = document.createElement('div');
    c.className = 'prow-ctl';
    const v = document.createElement('span');
    v.className = 'prow-val';
    r.append(l, c, v);
    body.appendChild(r);
    return { ctl: c, val: v };
  };

  const check = (label: string, get: () => boolean, set: (b: boolean) => void) => {
    const { ctl, val } = row(label);
    const b = document.createElement('button');
    b.className = 'pswitch';
    const paint = () => {
      const on = get();
      b.textContent = on ? 'ON' : 'OFF';
      b.classList.toggle('on', on);
      val.textContent = '';
    };
    b.onclick = () => {
      set(!get());
      saveGfx();
      paint();
      sync();
    };
    ctl.appendChild(b);
    paint();
    return paint;
  };

  const slider = (
    label: string,
    lo: number,
    hi: number,
    get: () => number,
    set: (n: number) => void,
    fmt: (n: number) => string,
  ) => {
    const { ctl, val } = row(label);
    const s = document.createElement('input');
    s.type = 'range';
    s.min = String(lo);
    s.max = String(hi);
    s.step = '0.05';
    const paint = () => {
      s.value = String(get());
      val.textContent = fmt(get());
    };
    s.oninput = () => {
      set(Number(s.value));
      val.textContent = fmt(get());
    };
    // Persist on release, not on every pixel of the drag.
    s.onchange = () => saveGfx();
    ctl.appendChild(s);
    paint();
    return paint;
  };

  const repaint: Array<() => void> = [];

  repaint.push(
    check(
      'current dots',
      () => gfx.dots,
      (b) => {
        gfx.dots = b;
      },
    ),
  );
  repaint.push(
    slider(
      'dot speed',
      0.1,
      4,
      () => gfx.dotSpeed,
      (n) => {
        gfx.dotSpeed = n;
      },
      (n) => `${n.toFixed(2)}×`,
    ),
  );
  repaint.push(
    slider(
      'dot size',
      0.4,
      3,
      () => gfx.dotSize,
      (n) => {
        gfx.dotSize = n;
      },
      (n) => `${n.toFixed(2)}×`,
    ),
  );
  repaint.push(
    slider(
      'dot opacity',
      0.1,
      1,
      () => gfx.dotAlpha,
      (n) => {
        gfx.dotAlpha = n;
      },
      (n) => `${Math.round(n * 100)}%`,
    ),
  );
  repaint.push(
    check(
      'voltage colour',
      () => gfx.voltageColor,
      (b) => {
        gfx.voltageColor = b;
      },
    ),
  );

  /** Grey the dot controls when the dots are off — they still work, but a
   *  slider that visibly does nothing is a small lie. */
  function sync() {
    const off = !gfx.dots;
    for (const r of [...body.children].slice(1, 4)) {
      (r as HTMLElement).style.opacity = off ? '0.35' : '1';
    }
  }

  const reset = document.createElement('button');
  reset.className = 'gfx-reset';
  reset.textContent = 'reset to defaults';
  reset.onclick = () => {
    Object.assign(gfx, GFX_DEFAULT);
    saveGfx();
    for (const f of repaint) f();
    sync();
  };
  body.appendChild(reset);

  el.append(hd, body);
  host.appendChild(el);
  sync();

  let open = false;
  function close() {
    open = false;
    el.style.display = 'none';
  }
  function toggle() {
    open = !open;
    el.style.display = open ? '' : 'none';
    if (open) for (const f of repaint) f();
  }

  return {
    toggle,
    close,
    isOpen: () => open,
    owns: (t) => t instanceof Node && el.contains(t),
  };
}
