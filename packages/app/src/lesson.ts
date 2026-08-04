// The intro series, client half: the LESSON CARD.
//
// A lesson is a ROOM made from an `intro-*` template (see the server's
// `lessons.rs`): a small circuit, pre-armed probes, a seeded scope, and this
// card — the words. The card is keyed by the room's TEMPLATE id (provenance
// in the hello), shows one short step at a time, and checks each step
// against LIVE SOLVER OUTPUT: a step is done when the circuit says so, never
// when the player clicks "ok". That is the design pillar applied to
// teaching — the lesson cannot claim what the solver does not show.
//
// Element ids inside the checks are a CONTRACT with the template JSON in
// crates/server/templates/ — renumber there, renumber here.
//
// Steps LATCH: once the solver has shown the thing, un-showing it does not
// un-teach it. Progress persists per room in localStorage, so a reload
// resumes mid-lesson. When every step has latched the card offers the next
// lesson — it finds an existing room made from that template (yours from a
// previous run) or creates one, through the same /api the room browser uses.
//
// The card deliberately reuses the goal card's visual language (.goalcard
// CSS) and sits top-left, so on lesson 10 — where the hoist's own goal card
// is on screen bottom-right — the two read as two instruments, not a clash.

import type { ElementSpec, ElemLive } from './circuit';
import type { MachineMsg } from './machines/seam';

export interface LessonDeps {
  elements(): ElementSpec[];
  live(): Map<number, ElemLive>;
  /** Server damage truth: is this part currently broken? */
  isBroken(id: number): boolean;
  /** Latest machine message (lesson 10 reads the hoist's own win). */
  machine(): MachineMsg | null;
  /** Switch this client to a room (same path the room browser uses). */
  join(code: string): void;
  toast(msg: string): void;
}

export interface LessonUI {
  /** A hello landed: `template` decides whether this room is a lesson. */
  onRoom(room: { id: string; template: string } | null): void;
  /** Once per animation frame; internally throttled. */
  tick(now: number): void;
  /** Typing guard for main.ts hotkeys (the card only has buttons, but a
   * focused button must not eat the canvas keys). */
  owns(t: EventTarget | null): boolean;
}

// ------------------------------------------------------------- the checks

/** What a step check may read. Everything here is solver output or shared
 * document state — a check can never invent a number. */
interface Ctx {
  byId: Map<number, ElementSpec>;
  live: Map<number, ElemLive>;
  broken(id: number): boolean;
  machine: MachineMsg | null;
}

/** |current| at a pin (A). 0 when the part has no frame yet. */
const ia = (c: Ctx, id: number, pin = 0) => Math.abs(c.live.get(id)?.i[pin] ?? 0);
/** Voltage at a pin, to ground (V). */
const va = (c: Ctx, id: number, pin = 0) => c.live.get(id)?.v[pin] ?? 0;
/** Voltage ACROSS a two-pin part, pin0 - pin1 (V). */
const vd = (c: Ctx, id: number) => {
  const l = c.live.get(id);
  return l ? (l.v[0] ?? 0) - (l.v[1] ?? 0) : 0;
};
/** A Switch/Button's shared closed state. */
const closed = (c: Ctx, id: number) => {
  const k = c.byId.get(id)?.kind;
  return (k?.t === 'Switch' || k?.t === 'Button') && k.closed;
};
/** A DC source's dialled volts. */
const dc = (c: Ctx, id: number) => {
  const k = c.byId.get(id)?.kind;
  return k?.t === 'VoltageSource' ? k.dc : 0;
};

type Check = (c: Ctx) => boolean;

/** Latches when a toast containing `text` has been on screen. Used exactly
 * once, for the placement-gate refusal in lesson 4: the refusal IS the gate's
 * real output, and it is transient, so the check has to remember seeing it. */
function sawToast(text: string): Check {
  let seen = false;
  return () => {
    if (!seen) {
      const t = document.getElementById('toasts')?.textContent ?? '';
      seen = t.includes(text);
    }
    return seen;
  };
}

/** The condition must hold continuously for `ms` — for "dial it to 5 mA"
 * checks, so sweeping the slider through the target does not count. */
function held(ms: number, f: Check): Check {
  let since: number | null = null;
  return (c) => {
    if (!f(c)) {
      since = null;
      return false;
    }
    const now = performance.now();
    since ??= now;
    return now - since >= ms;
  };
}

interface Step {
  /** Short imperative text; the thing to DO and the thing it shows. */
  text: string;
  check: Check;
}

interface LessonSpec {
  /** 1-based position, for "LESSON 4 / 10". */
  n: number;
  title: string;
  /** Fresh step list (checks are stateful — `held` — so they are rebuilt
   * on every room join). */
  steps(): Step[];
  /** One quiet line under the steps: the idea, not an instruction. */
  hint: string;
  /** Template id the NEXT button leads to; null ends the course. */
  next: string | null;
  nextLabel: string;
}

// The course. Texts stay under ~200 chars per step: a lesson that needs a
// paragraph is the wrong lesson.
const LESSONS: Record<string, LessonSpec> = {
  'intro-01-loop': {
    n: 1,
    title: 'THE LOOP',
    steps: () => [
      {
        text: 'The bottom wire is missing. Press W, then drag across the gap. The lamp lights the moment the loop closes.',
        check: (c) => ia(c, 3) > 0.05,
      },
      {
        text: 'Click any wire — even one the lamp is “after” — and press Delete. Everything stops at once: there is no “used up along the way”.',
        check: (c) => ia(c, 3) < 0.001,
      },
      {
        text: 'Draw it back. Both meters read the same 100 mA = 9 V ÷ 90 Ω: one current, everywhere in the loop.',
        check: (c) => ia(c, 3) > 0.05,
      },
    ],
    hint: 'A circuit is a loop. Current flows around it — it is not consumed.',
    next: 'intro-02-divider',
    nextLabel: '2 · Volts, Ohms, the Divider',
  },

  'intro-02-divider': {
    n: 2,
    title: 'VOLTS, OHMS, THE DIVIDER',
    steps: () => [
      {
        text: 'Left bench: drag the SRC slider in the OHM BENCH window until the meter holds 5.0 mA. One ohm law: I = V ÷ R, and R is 1 kΩ.',
        check: held(700, (c) => Math.abs(ia(c, 3) - 0.005) < 0.0004),
      },
      {
        text: 'Right bench: drag the POT until the wiper meter holds 1.00 V. The wiper takes a fraction of 4 V by position — a dialable voltage.',
        check: held(700, (c) => Math.abs(va(c, 7, 1) - 1.0) < 0.06),
      },
    ],
    hint: 'The divider is every sensor and every setpoint you will ever wire.',
    next: 'intro-03-kirchhoff',
    nextLabel: '3 · Kirchhoff',
  },

  'intro-03-kirchhoff': {
    n: 3,
    title: 'KIRCHHOFF',
    steps: () => [
      {
        text: 'Left: 150 mA leaves the battery and splits, 100 + 50, over two lamps. Click the switch OPEN — the battery meter drops by exactly the branch you cut.',
        check: (c) =>
          !closed(c, 5) && ia(c, 3) > 0.06 && Math.abs(ia(c, 1) - ia(c, 3)) < 0.005,
      },
      {
        text: 'Close it again: the meters re-add. In equals out at every junction — charge has nowhere else to go.',
        check: (c) =>
          closed(c, 5) && ia(c, 6) > 0.03 && Math.abs(ia(c, 1) - (ia(c, 3) + ia(c, 6))) < 0.01,
      },
      {
        text: 'Right: two resistors share 9 V, 6 + 3. Slide the battery in its window — the drops move together and ALWAYS sum to the source.',
        check: (c) =>
          Math.abs(Math.abs(vd(c, 9)) - 9) > 1.5 &&
          Math.abs(vd(c, 11) + vd(c, 12) - vd(c, 9)) < 0.3 &&
          Math.abs(vd(c, 9)) > 1.0,
      },
    ],
    hint: 'Two bookkeeping laws: currents at a point sum to zero, volts around a loop sum to zero.',
    next: 'intro-04-smoke',
    nextLabel: '4 · Smoke',
  },

  'intro-04-smoke': {
    n: 4,
    title: 'SMOKE',
    steps: () => [
      {
        text: 'Top left: an LED straight across 9 V, one gap. TRY to close it with a wire (W) — the game refuses. With nothing to limit current there is no valid answer at all. Read the message.',
        check: sawToast('no operating point'),
      },
      {
        text: 'Top right: bridge that gap with a RESISTOR instead (press R, drag across it). Now (9 − 2) V ÷ 1 kΩ ≈ 7 mA flows and the LED lives: the resistor takes the rest of the push and sets the flow.',
        check: (c) => !c.broken(6) && ia(c, 6) > 0.004 && ia(c, 6) < 0.038,
      },
      {
        text: 'Bottom: someone decided 10 Ω was “close enough”. Click its switch and watch the LED: 0.65 A into a part rated 0.04. Watts = volts × amps became heat, and heat has a budget.',
        check: (c) => c.broken(12),
      },
      {
        text: 'Open that switch, press K, and click the charred LED to repair it. It stays alive exactly as long as nobody closes the trap again — fix circuits, not just parts.',
        check: (c) => !c.broken(12) && !closed(c, 10),
      },
    ],
    hint: '20 mA × 2 V = 0.04 W is fine. 0.65 A × 7 V is 4.5 W in a 5 mm package: 0.35 s.',
    next: 'intro-05-time',
    nextLabel: '5 · Time',
  },

  'intro-05-time': {
    n: 5,
    title: 'TIME',
    steps: () => [
      {
        text: 'Click and HOLD the button beside the battery. The meter climbs a CURVE — fast, then slower. τ = R·C = 1 kΩ × 1000 µF = 1 s; near-full takes three of them.',
        check: (c) => va(c, 6, 0) > 4.2,
      },
      {
        text: 'Let go. The voltage stays: a capacitor STORES charge, like a bucket holds water.',
        check: (c) => !closed(c, 3) && va(c, 6, 0) > 3.5,
      },
      {
        text: 'Now hold the button beside the lamp: the bucket dumps through it in a flash — same law, smaller R, faster τ.',
        check: (c) => va(c, 6, 0) < 0.8,
      },
    ],
    hint: 'Nothing electrical is instant. R times C is the clock everything analog keeps.',
    next: 'intro-06-diode',
    nextLabel: '6 · One Way',
  },

  'intro-06-diode': {
    n: 6,
    title: 'ONE WAY',
    steps: () => [
      {
        text: 'The lamp is dark: the source is pushing −5 V into a diode. Slide SRC positive (its window is on the left) — now it lights.',
        check: (c) => ia(c, 4) > 0.05,
      },
      {
        text: 'Slide it negative again: dark at ANY negative push. A diode is a one-way valve.',
        check: (c) => dc(c, 1) < -2 && ia(c, 4) < 0.001,
      },
      {
        text: 'Back to +5 V, and read the meters: the diode keeps only ~0.7 V — its fixed toll — and the lamp gets the rest. The right bench shows the same valve halving a wave.',
        check: (c) => dc(c, 1) > 3 && ia(c, 4) > 0.05 && Math.abs(vd(c, 3) - 0.8) < 0.25,
      },
    ],
    hint: 'Forward: a fixed ~0.7–2 V drop. Reverse: nothing. This is why an LED needs a resistor to take the rest.',
    next: 'intro-07-opamp',
    nextLabel: '7 · The Decider',
  },

  'intro-07-opamp': {
    n: 7,
    title: 'THE DECIDER',
    steps: () => [
      {
        text: 'The op-amp compares the knob’s voltage against a 2 V reference. Turn SETPOINT KNOB up: the instant the wiper passes 2 V the output slams to +5 and the LED snaps ON.',
        check: (c) => va(c, 2, 1) > 2.25 && va(c, 9, 2) > 4,
      },
      {
        text: 'Ease it back under 2 V: it snaps OFF. No dimming, no in-between — open loop, an op-amp is a DECISION.',
        check: (c) => va(c, 2, 1) < 1.75 && va(c, 9, 2) < -4,
      },
      {
        text: 'Right bench, MUSCLE TEST: an op-amp wired straight at a lamp. Close the switch and read the meter — 25 mA, and the lamp barely warms. That is all any op-amp can push.',
        check: (c) => closed(c, 17) && ia(c, 16, 2) > 0.015 && ia(c, 16, 2) < 0.035,
      },
    ],
    hint: 'A brain, not a muscle: it decides with volts and must command something else to carry amps.',
    next: 'intro-08-mosfet',
    nextLabel: '8 · The Gate',
  },

  'intro-08-mosfet': {
    n: 8,
    title: 'THE GATE',
    steps: () => [
      {
        text: 'Left: a MOSFET sits under the lamp, its gate fed by the little switch. Close it — the lamp lights through the FET’s channel.',
        check: (c) => ia(c, 3) > 0.1,
      },
      {
        text: 'Read the GATE meter: 0.000 A. The gate is a field, not a path — no current in, real current commanded. This is what an op-amp’s 25 mA is FOR.',
        check: (c) => closed(c, 8) && ia(c, 4, 0) < 1e-6 && ia(c, 3) > 0.1,
      },
      {
        text: 'Right: the same parts with the FET on TOP of the lamp. Close its switch — barely a glow. Vgs is gate-to-SOURCE, and the lamp under the source steals it. Switch the LOW side.',
        check: (c) => closed(c, 18) && ia(c, 14) > 0.02 && ia(c, 14) < 0.095,
      },
    ],
    hint: 'Gate over threshold → channel on. Free to command, and the source must sit at ground.',
    next: 'intro-09-muscle',
    nextLabel: '9 · Muscle',
  },

  'intro-09-muscle': {
    n: 9,
    title: 'MUSCLE',
    steps: () => [
      {
        text: 'Left: a TO-92 small-signal FET asked to carry half an amp. Close the switch and watch its meter — it must drop ~3 V at 0.2 A: 0.7 W in a 0.35 W package. It chars.',
        check: (c) => c.broken(4),
      },
      {
        text: 'Right: the SAME 5 V command into a power FET (TO-220, tier 1). Close its switch: full brightness, and the FET drops millivolts — cold.',
        check: held(2000, (c) => ia(c, 13) > 0.4 && !c.broken(14)),
      },
      {
        text: 'Open the left switch, press K, and repair the little FET. Small parts for signals, packages with headroom for power.',
        check: (c) => !c.broken(4) && !closed(c, 8),
      },
    ],
    hint: 'Watts land in the package: I·V across the part is heat it must survive. Ratings are the tech tree.',
    next: 'intro-10-close-the-loop',
    nextLabel: '10 · Close the Loop',
  },

  'intro-10-close-the-loop': {
    n: 10,
    title: 'CLOSE THE LOOP',
    steps: () => [
      {
        text: 'Three benches: SENSE (4 V through the machine’s pot — height as volts), COMPARE (op-amp: wiper vs 3.2 V), DRIVE (6 V, power FET, freewheel diode). ONE wire is missing: connect the op-amp OUT to the FET GATE (W, two strokes).',
        check: (c) => ia(c, 900, 0) > 0.15,
      },
      {
        text: 'It climbs, overshoots, chatters — bang-bang feedback, holding without you. Keep the crate in the band 5 s: the goal card bottom-right is the hoist’s own.',
        check: (c) => c.machine?.win === true,
      },
      {
        text: 'Double-click the 3.2 V battery on the COMPARE bench and nudge it — the crate FOLLOWS. 3.2 V is 0.32 m through the sensor’s 4 V / 0.40 m scale: a setpoint is a target written in volts.',
        check: (c) => Math.abs(dc(c, 9) - 3.2) > 0.15,
      },
    ],
    hint: 'Sense → compare → drive. You are no longer the feedback: the circuit is.',
    next: 'hoist',
    nextLabel: 'THE HOIST — the real one, bare',
  },
};

/** The course in order, for the header (“LESSON 4 / 10”). */
const COURSE_LEN = 10;

// ------------------------------------------------------------ persistence

const doneKey = (room: string) => `ee.lesson.${room}.done`;
const lsGet = (k: string): string | null => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const lsSet = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v);
  } catch {
    /* private mode */
  }
};

// ---------------------------------------------------------------- the card

const div = (cls: string, parent?: HTMLElement): HTMLDivElement => {
  const el = document.createElement('div');
  el.className = cls;
  parent?.append(el);
  return el;
};

export function createLesson(root: HTMLElement, deps: LessonDeps): LessonUI {
  const el = div('goalcard lessoncard');
  el.id = 'lesson';
  el.style.display = 'none';

  const hd = div('goal-hd', el);
  const caret = document.createElement('span');
  caret.className = 'goal-caret';
  const title = document.createElement('span');
  title.className = 'goal-title';
  const badge = document.createElement('span');
  badge.className = 'goal-badge';
  hd.append(caret, title, badge);

  const body = div('goal-body', el);
  const stepsEl = div('lesson-steps', body);
  const hint = div('goal-hint', body);
  const row = div('goal-row', body);
  const nextBtn = document.createElement('button');
  nextBtn.className = 'goal-reset lesson-next';
  nextBtn.style.display = 'none';
  const restartBtn = document.createElement('button');
  restartBtn.className = 'goal-reset';
  restartBtn.textContent = 'restart steps';
  restartBtn.title = 'forget this room’s step progress (the circuit stays as it is)';
  row.append(nextBtn, restartBtn);

  // Collapse state is per-course, not per-room: fold it once, it stays folded.
  const OPEN_KEY = 'ee.lesson.open';
  let open = lsGet(OPEN_KEY) !== '0';
  const applyOpen = () => {
    el.classList.toggle('collapsed', !open);
    caret.textContent = open ? '▾' : '▸';
  };
  hd.onclick = () => {
    open = !open;
    lsSet(OPEN_KEY, open ? '1' : '0');
    applyOpen();
  };
  applyOpen();
  root.append(el);

  // ------------------------------------------------------------ room state

  let roomId: string | null = null;
  let spec: LessonSpec | null = null;
  let steps: Step[] = [];
  let latched = 0;
  /** Steps only advance once live frames have arrived (a fresh join briefly
   * has an empty live map, which would satisfy “current < 1 mA” checks). */
  let sawLive = false;
  let lastTick = -Infinity;
  /** DOM rebuild signature, so tick() only touches the DOM on change. */
  let sig = '';

  function saveProgress() {
    if (roomId) lsSet(doneKey(roomId), String(latched));
  }

  function loadProgress(): number {
    if (!roomId) return 0;
    const n = Number(lsGet(doneKey(roomId)) ?? '0');
    return Number.isFinite(n) ? Math.max(0, Math.min(steps.length, Math.floor(n))) : 0;
  }

  function arm(room: { id: string; template: string } | null) {
    roomId = room?.id ?? null;
    spec = room ? (LESSONS[room.template] ?? null) : null;
    sawLive = false;
    sig = '';
    if (!spec) {
      el.style.display = 'none';
      steps = [];
      latched = 0;
      return;
    }
    steps = spec.steps();
    latched = loadProgress();
    el.style.display = 'block';
    title.textContent = `LESSON ${spec.n} / ${COURSE_LEN} — ${spec.title}`;
    hint.textContent = spec.hint;
    nextBtn.textContent = `next: ${spec.nextLabel} →`;
    paint();
  }

  restartBtn.onclick = () => {
    if (!spec) return;
    steps = spec.steps();
    latched = 0;
    saveProgress();
    sig = '';
    paint();
    restartBtn.blur();
  };

  // ------------------------------------------------- next-lesson plumbing

  /** Join a room made from `template`, creating one if none exists. The same
   * /api the room browser talks to; nothing here is a second protocol. */
  async function gotoTemplate(template: string) {
    nextBtn.disabled = true;
    try {
      const list = (await (await fetch('/api/rooms')).json()) as {
        rooms?: { id: string; template: string; players: number }[];
      };
      // Rejoin an existing room from this template — an EMPTY one, so
      // "next" never drops the player into someone's half-done lesson.
      const have = list.rooms?.find((r) => r.template === template && r.players === 0);
      if (have) {
        deps.join(have.id);
        return;
      }
      const res = await fetch('/api/rooms', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: '', template }),
      });
      const made = (await res.json()) as { room?: { id: string }; error?: string };
      if (!res.ok || !made.room) {
        deps.toast(`could not start the next lesson: ${made.error ?? 'no answer'}`);
        return;
      }
      deps.join(made.room.id);
    } catch {
      deps.toast('could not reach the server for the next lesson');
    } finally {
      nextBtn.disabled = false;
    }
  }

  nextBtn.onclick = () => {
    if (spec?.next) void gotoTemplate(spec.next);
    nextBtn.blur();
  };

  // ------------------------------------------------------------- painting

  function paint() {
    if (!spec) return;
    const won = latched >= steps.length;
    const key = `${spec.n}|${latched}|${won}`;
    if (key === sig) return;
    sig = key;
    badge.textContent = won ? 'DONE' : `${latched} / ${steps.length}`;
    badge.className = `goal-badge${won ? ' win' : latched > 0 ? ' in' : ''}`;
    el.classList.toggle('win', won);
    stepsEl.replaceChildren(
      ...steps.map((s, k) => {
        const r = div('lesson-step');
        const mark = document.createElement('b');
        const text = document.createElement('span');
        if (k < latched) {
          r.className = 'lesson-step done';
          mark.textContent = '✓';
          text.textContent = s.text;
        } else if (k === latched) {
          r.className = 'lesson-step now';
          mark.textContent = '▸';
          text.textContent = s.text;
        } else {
          r.className = 'lesson-step later';
          mark.textContent = '·';
          text.textContent = '…';
        }
        r.append(mark, text);
        return r;
      }),
    );
    nextBtn.style.display = won && spec.next ? '' : 'none';
  }

  // ------------------------------------------------------------ the clock

  const TICK_MS = 150;

  return {
    onRoom: arm,
    owns: (t) => t instanceof Node && el.contains(t),
    tick(now: number) {
      if (!spec || now - lastTick < TICK_MS) return;
      lastTick = now;
      const live = deps.live();
      if (!sawLive) {
        // One real frame with data in it arms the checks.
        sawLive = live.size > 0;
        if (!sawLive) return;
      }
      if (latched < steps.length) {
        const byId = new Map<number, ElementSpec>();
        for (const e of deps.elements()) byId.set(e.id, e);
        const ctx: Ctx = {
          byId,
          live,
          broken: (id) => deps.isBroken(id),
          machine: deps.machine(),
        };
        // Only the CURRENT step is evaluated: later checks must not latch
        // early off a state the player has not been asked to notice yet.
        const step = steps[latched];
        if (step && step.check(ctx)) {
          latched++;
          saveProgress();
          if (latched >= steps.length) {
            deps.toast(
              spec.next
                ? `lesson ${spec.n} done — the card offers the next one`
                : `lesson ${spec.n} done`,
            );
          }
        }
      }
      paint();
    },
  };
}
