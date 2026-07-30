// The bottom oscilloscope dock. Collapsed by default to a slim bar that
// still reports the probe count and every channel's latest sample (taken
// straight out of the TraceStore — never invented); expanded on demand by
// clicking the header strip or pressing backquote. While collapsed the
// waveform draw is skipped entirely and only the ~10 Hz text summary runs.
// Open/closed state and the expanded height persist in localStorage.
//
// The bar is also where the global sound controls live (mute + volume): it is
// the one strip that is already there, already the "instruments" chrome, and
// already persisted — inventing a second floating widget for two controls
// would be worse. The bar therefore stays visible when audio is live even if
// nothing is probed.

import type { AudioControls } from './audio';
import { lsGet as read, lsSet as write } from './store';
import { probeColor, renderScope, type Probe, type TraceStore, type ScopeSettings } from './scope';

const BAR_PX = 24;
const MIN_H = 120;
const MAX_H = 520;
const DEFAULT_H = 190;
const OPEN_KEY = 'ee.dock.open';
const HEIGHT_KEY = 'ee.dock.h';
const SUMMARY_MS = 100; // ~10 Hz — the collapsed bar does not need 60 fps

const clampH = (h: number) => Math.min(MAX_H, Math.max(MIN_H, h));

const fmtSI = (v: number, unit: string) => {
  const a = Math.abs(v);
  if (a >= 1000) return `${(v / 1000).toFixed(2)} k${unit}`;
  if (a >= 1) return `${v.toFixed(2)} ${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toFixed(1)} m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toFixed(1)} µ${unit}`;
  if (a >= 1e-9) return `${(v * 1e9).toFixed(1)} n${unit}`;
  return `0 ${unit}`;
};

/** Latest sample of a probe, or null when the store has nothing yet. */
function latest(traces: TraceStore, pid: number): number | null {
  const a = traces.samples(pid);
  return a.length >= 2 ? a[a.length - 1]! : null;
}

export interface Dock {
  isOpen(): boolean;
  setOpen(v: boolean): void;
  toggle(): void;
  /** Per-frame: hides the dock when there is nothing to show (no probes and
   * no audio), draws waveforms only if open. */
  update(now: number, probes: Probe[], traces: TraceStore, set: ScopeSettings): void;
}

export function createDock(root: HTMLElement, cv: HTMLCanvasElement, audio: AudioControls): Dock {
  const bar = root.querySelector('#scopebar') as HTMLDivElement;
  const caret = root.querySelector('#scopecaret') as HTMLSpanElement;
  const sumEl = root.querySelector('#scopesum') as HTMLSpanElement;
  const audioEl = root.querySelector('#scopeaudio') as HTMLSpanElement;
  const muteBtn = root.querySelector('#audiomute') as HTMLButtonElement;
  const volEl = root.querySelector('#audiovol') as HTMLInputElement;
  const audioLbl = root.querySelector('#audiolabel') as HTMLSpanElement;

  // The bar's own pointerdown toggles/resizes the dock: the controls must
  // swallow their events or every mute click would also collapse the scope.
  for (const el of [muteBtn, volEl]) {
    el.addEventListener('pointerdown', (ev) => ev.stopPropagation());
    el.addEventListener('pointerup', (ev) => ev.stopPropagation());
  }
  muteBtn.addEventListener('click', (ev) => {
    ev.stopPropagation();
    audio.setMuted(!audio.muted);
    syncAudio(true);
  });
  volEl.addEventListener('input', () => {
    audio.setVolume(Number(volEl.value) / 100);
    syncAudio(true);
  });
  volEl.value = String(Math.round(audio.volume * 100));

  let audioKey = '';
  /** Mirror the player's state into the bar. `force` skips the change check
   * (used right after a click, whose effect is instant). */
  function syncAudio(force = false) {
    const st = audio.status();
    const live = st.speakers > 0 || st.listening;
    const key = `${live}|${st.speakers}|${st.listening}|${audio.muted}|${st.sounding}|${st.needsGesture}`;
    if (!force && key === audioKey) return live;
    audioKey = key;
    audioEl.style.display = live ? 'flex' : 'none';
    if (!live) return live;
    muteBtn.textContent = audio.muted ? 'sound off' : st.sounding ? 'sound ◂))' : 'sound ◂';
    muteBtn.title = audio.muted ? 'unmute (all sound is off)' : 'mute all sound';
    muteBtn.classList.toggle('off', audio.muted);
    const parts: string[] = [];
    if (st.speakers > 0) parts.push(`${st.speakers} speaker${st.speakers === 1 ? '' : 's'}`);
    if (st.listening) parts.push('listening');
    audioLbl.textContent = st.needsGesture ? 'click to enable sound' : parts.join(' · ');
    audioLbl.classList.toggle('warn', st.needsGesture);
    return live;
  }

  let open = read(OPEN_KEY) === '1'; // collapsed unless explicitly opened before
  let height = clampH(Number(read(HEIGHT_KEY)) || DEFAULT_H);
  let lastSumT = -Infinity;
  let sumKey = '';
  /** With nothing probed there is no waveform to expand into, so the bar is
   * audio-only: it stays a strip however the open flag is stored. */
  let hasProbes = false;

  function apply() {
    const showTraces = open && hasProbes;
    root.classList.toggle('collapsed', !showTraces);
    root.style.height = `${showTraces ? height : BAR_PX}px`;
    caret.style.display = hasProbes ? '' : 'none';
    caret.textContent = showTraces ? 'scope ▾' : 'scope ▴';
  }

  function setOpen(v: boolean) {
    open = v;
    write(OPEN_KEY, v ? '1' : '0');
    apply();
  }

  // Click the strip to toggle; drag it vertically to resize when expanded
  // (a drag that starts on the collapsed bar opens the dock first).
  let drag: { y: number; h: number; moved: boolean } | null = null;
  bar.addEventListener('pointerdown', (ev) => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    ev.stopPropagation();
    drag = { y: ev.clientY, h: open ? height : BAR_PX, moved: false };
    bar.setPointerCapture(ev.pointerId);
  });
  bar.addEventListener('pointermove', (ev) => {
    if (!drag) return;
    const dy = drag.y - ev.clientY; // dragging up grows the panel
    if (!drag.moved && Math.abs(dy) <= 4) return;
    drag.moved = true;
    if (!open) setOpen(true);
    height = clampH(drag.h + dy);
    root.style.height = `${height}px`;
  });
  bar.addEventListener('pointerup', () => {
    if (!drag) return;
    const moved = drag.moved;
    drag = null;
    if (moved) write(HEIGHT_KEY, String(Math.round(height)));
    else setOpen(!open);
  });
  bar.addEventListener('pointercancel', () => (drag = null));

  /** Probe count plus each channel's latest value in its probe colour. */
  function updateSummary(probes: Probe[], traces: TraceStore) {
    if (probes.length === 0) {
      if (sumKey !== '') {
        sumKey = '';
        sumEl.textContent = '';
      }
      return;
    }
    const labels = probes.map((p) => {
      const v = latest(traces, p.pid);
      const name = `${p.kind.toUpperCase()}${p.pid}${p.r ? 'Δ' : ''}`;
      return `${name} ${v === null ? '—' : fmtSI(v, p.kind === 'v' ? 'V' : 'A')}`;
    });
    const key = labels.join('|');
    if (key === sumKey) return;
    sumKey = key;
    sumEl.textContent = '';
    const count = document.createElement('span');
    count.textContent = `${probes.length} probe${probes.length === 1 ? '' : 's'}`;
    sumEl.append(count);
    probes.forEach((p, k) => {
      const chip = document.createElement('span');
      chip.style.color = probeColor(p.pid);
      chip.textContent = labels[k]!;
      sumEl.append(chip);
    });
  }

  function update(now: number, probes: Probe[], traces: TraceStore, set: ScopeSettings) {
    // Audio state is cheap to read but not free to reflect into the DOM, so
    // it rides the same ~10 Hz budget as the channel summary.
    const audioLive = now - lastSumT >= SUMMARY_MS ? syncAudio() : audioEl.style.display !== 'none';
    if (hasProbes !== probes.length > 0) {
      hasProbes = probes.length > 0;
      apply();
    }
    if (probes.length === 0 && !audioLive) {
      root.style.display = 'none';
      return;
    }
    root.style.display = 'block';
    if (open && hasProbes) renderScope(cv, traces, probes, set.timebase, set);
    if (now - lastSumT >= SUMMARY_MS) {
      lastSumT = now;
      updateSummary(probes, traces);
    }
  }

  apply();
  syncAudio(true);
  return { isOpen: () => open, setOpen, toggle: () => setOpen(!open), update };
}
