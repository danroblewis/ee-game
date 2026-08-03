# External inputs: gamepad and microphone — the half that was designed and not built

**Status: design, recovered.** The camera half of this work shipped in
`966ccfa` ("the webcam is a layer, a photocell reads it"). The gamepad and
microphone halves were designed and measured in the same pass, deliberately
cut from that branch's scope, and then existed only as workflow transcript —
four passing comments in the source (`crates/server/src/main.rs`,
`crates/sim-core/src/netlist.rs`, `packages/app/src/layer.ts`,
`packages/app/src/machines/seam.ts`) were the entire surviving trace. This
file is that research written down. Numbers below are measured, in stock
Chrome, on an M3 Ultra, with fake devices; provenance is at the bottom.

The one line the whole feature rests on, and the reason the gamepad is nearly
free once the camera exists: **the sim never learns that a webcam — or a
gamepad, or a microphone — exists.** It learns that element #42's resistance
is now 4700 Ω. `sim-core` has no external-input concept at all.

---

## 1. Why the gamepad is third and not first

| source | value to the player | risk + effort |
|---|---|---|
| Microphone | high — clap-to-circuit is instantly legible; a spectrum strip is a genuinely new toy | low–medium |
| Camera | highest — the world-layer mechanic, hand-distance-as-light | highest |
| **Gamepad** | **lowest — it is a knob, and the room already has pots and sliders** | **lowest** |

The decision was to spike the **camera** first even though the microphone had
the better ratio, because the camera is the only one that forces the whole
architecture into existence: the world layer with a coordinate mapping,
aperture→field sampling, the fail-safe, the privacy chrome, `ParamWrite::Light`
and the placement-time range trial. Spiking the gamepad first would have
validated the cheap half, left the hard half unproven, and very likely produced
a **binding-configuration dialog that the layer mechanic then throws away**.

So the order is camera → microphone → gamepad, and the gamepad was cut from
the shipped branch for that reason alone. Nothing about it was found to be
hard or unwise.

---

## 2. The gamepad, measured

- `navigator.getGamepads()` is a **snapshot poller, not an event source**.
  Real implementation, zero pads connected: **0.47 µs per call**. Free at any
  rate. Poll it in the rAF loop that already exists.
- Axes are floats in −1..+1. Buttons are `{pressed, touched, value: 0..1}`, so
  triggers are analogue. `mapping: "standard"` gives the 17-button / 4-axis
  layout; **anything else is a raw HID map and must be treated as unlabelled** —
  do not name buttons on a pad that did not claim the standard mapping.
- **The gotcha, and it shapes the UI:** per spec and per Chrome,
  `getGamepads()` returns an empty list until a *gamepad user gesture* — a
  button press or an axis move on the pad, while the page has focus. This is
  anti-fingerprinting and must not be worked around. The consequence is that
  **you cannot offer contextual "press A to bind" copy, because until they
  press something you do not know a pad exists.** The flow has to be: player
  acts in the app first → app says *"press any button on your controller"* →
  `gamepadconnected` fires → bind. Secure-context only, like everything else
  here.
- **Deadzone:** ~0.08–0.15 with radial rescale, `(|a| − dz) / (1 − dz)`. Then
  quantize to the u16, *then* apply hysteresis on the quantized value, so a
  resting stick sends nothing at all.
- **Testing without hardware:** `navigator.getGamepads` is a plain property and
  can be redefined from the page. A synthetic pad polled that way measured
  0.074 µs and exercises the whole bind → deadzone → quantize → send path
  deterministically in CI. Real-hardware verification is a separate one-off.
- Privacy: a gamepad needs **no permission prompt and no live indicator** — it
  is not a capture device. It is still a fingerprinting surface, which is
  exactly why the browser gates it behind the gamepad gesture above.

### The mapping story

A gamepad rides the **same mechanic as the camera**, and that is the whole
point: only the *field provider* differs. The part, the op, the gate, the
fail-safe and the privacy story are identical. A `FieldSource` is anything
that answers

```ts
interface FieldSource { sample(u0: number, v0: number, u1: number, v1: number): number } // → 0..1
```

- **Pad layer** — a drawn diagram of the controller placed in the world, the
  same rectangle a camera layer is, with a different provider behind it. Each
  axis is a strip, each button a pad. `sample` returns `(axis + 1) / 2` for an
  axis, `value` for a trigger, `pressed ? 1 : 0` for a button. A photocell
  dropped on the left stick's X strip reads that axis; drag it onto a button
  and it reads that button.
- **Bipolar drive is a circuit problem, not an API problem.** An axis is
  −1..+1 and a photocell reads 0..1; getting a signed drive out of it wants two
  cells and a bridge. That is the game, and it is a better puzzle than an
  option box.
- **There is no binding-configuration UI anywhere, and there must never be
  one.** You place a part on a thing. Which part reads which region is
  re-derived from element geometry every frame, exactly as `panelMembers` and
  the shipped photocell already work — drag it off and it unbinds with no op
  at all.

### What it costs to build

Everything downstream already exists. A gamepad needs:

1. a `FieldSource` implementation polling `getGamepads()` in the rAF loop;
2. a `LayerKind::Pad` variant on the existing `Layer` room state (the design
   always had `kind: Camera | Mic | Pad`; the shipped struct dropped it) and a
   diagram renderer for it;
3. nothing else. Same `Cmd::Sensor`, same u16 quantization, same tick-boundary
   `ParamWrite`, same claim handshake, same fail-safe.

Estimated at roughly a day once the camera exists. That estimate is why it was
cut rather than deferred indefinitely.

---

## 3. The microphone, measured

Designed in the same breath and cut for the same reason; recorded here because
the two were never separable.

- **Mic layer — a spectrum strip.** `u` maps log-frequency 40 Hz…8 kHz, `v` is
  unused. `sample` returns mean normalized magnitude over the band under the
  aperture, taken from the same `Float32Array` the strip is *drawn* from —
  sample the data, never the rendering. A photocell on the left of the strip is
  a kick-drum sensor; on the right it is a hiss sensor.
- Ground truth fed to Chrome: 440 Hz sine, amplitude 0.3, true RMS 0.21213.

  | method | window | cost/call | measured RMS (p50 / p95) |
  |---|---|---|---|
  | `AnalyserNode` fft256 | 5.33 ms | 0.0008 ms | 0.2125 / **0.2179** |
  | `AnalyserNode` fft1024 | 21.3 ms | 0.0022 ms | 0.2123 / 0.2133 |
  | `AnalyserNode` fft2048 | 42.7 ms | 0.0027 ms | 0.2123 / 0.2130 |
  | AudioWorklet, every 16 blocks | 42.7 ms | — | 0.2121 / 0.2130 |

  **RMS extraction is exact and free either way. The window is what matters:**
  at 5.33 ms the RMS ripples ±2.6% at the signal's own frequency. Use ≥20 ms.
- **Prefer the `AudioWorklet` to the `AnalyserNode`** even though the Analyser
  is simpler and adequate: the worklet decimates on the audio thread, attaches
  an honest `currentTime`, and cannot silently under-sample when a rAF is
  dropped. `every=12` at 48 kHz gives 31.25 Hz, closest to the tick.
- **`every=1` (375 Hz) is a trap.** The audio thread's timing stays exact but
  delivery to the main thread batches: measured message deltas p50 **0.1 ms**,
  max **42.8 ms**. That is `audio-worklet.ts`'s two-clocks lesson running in
  reverse — *the audio thread's clock is trustworthy, the main thread's arrival
  time is not*. Stamp every measurement in the worklet; never infer time from
  when the message landed.
- `echoCancellation`, `noiseSuppression` and `autoGainControl` must be **off**
  (verified they can be). They are speech processors and will mangle a level
  measurement — the same argument as the camera's auto-exposure.
- Pitch is viable if ever wanted: an 8192-point FFT at 48 kHz (5.86 Hz bins)
  found 440 Hz at **439.45 Hz**, ~0.004 ms/call. Not needed for a knob.
- Privacy: the mic gets the camera's whole treatment, plus the specific
  promise that it is reduced to one number per tick **on the audio thread** —
  no audio is buffered, recorded or transmitted.

---

## 4. The rate, and why it is the same for all three

The tick is 30 Hz and the server's `supersede` coalesces one write per part per
tick, so an input arriving faster is discarded regardless of the hardware.
**Every external input is a 30 Hz signal with 15 Hz of usable bandwidth.** You
can make loudness dim a lamp; you cannot whistle a tone into a circuit. The UI
must say so rather than letting players discover it.

This is a deliberate ceiling, not a limitation to engineer around:

- Audio at 12.5 kHz through the op pipeline would be 50 kB/s per source into
  the log and 12 500 `write_param` calls per tick — the machine co-sim does 45.
  Dead on both counts.
- A genuine waveform input is a **different device**: a buffered
  arbitrary-waveform source whose sample block is document data, sent one tick
  ahead and played out across substeps by the engine. Decimated to ~4 kHz mono
  that is 8 kB/s and it stays deterministic, because the buffer is in the log.
  Real, buildable, and not this feature. Do not conflate them.

Why the write is `ParamWrite` at the tick boundary and never
`InteractOp::SetValue`: the knob path clones the document and runs the
placement gate, which is **562 µs on the shipped 147-element room and 10.5 ms
at 797 elements** — 99% of it the gate's two whole-document LU factorizations.
`eng.interact` itself is 8–65 µs and the `ParamWrite` is unmeasurable. The
range is gated **once, at placement**, exactly as the hoist's 1.5 kHz ungated
writes are; thereafter a bounded-conductance write cannot make the matrix
singular and — critically — does not clear `quarantined` or re-arm `be_steps`,
which is what stops a sensor stream resurrecting a diverged room 30×/s.

---

## 5. Explicitly rejected, with the numbers

**MediaPipe hand tracking — no.** Measured with `@mediapipe/tasks-vision`
1.0.1 against a fake camera: **~9.4 MB gzipped** (wasm 3.43 MB gz + glue
78 KB gz + API 45 KB gz + `hand_landmarker.task` float16 5.85 MB gz) against
the app's entire current sim WASM of 258 KB; `HandLandmarker` create 121 ms
GPU / 58 ms CPU; per frame on an M3 Ultra p50 **5.5 ms** GPU — **but the first
call took 4548 ms** compiling shaders, freezing rAF for four and a half
seconds. CPU delegate p50 18.8 ms with a visible main-thread stutter. And no
hand was in frame, so only the palm detector ran.

If it is ever wanted it must be lazily downloaded on bind, run in a worker with
`OffscreenCanvas`, warm up behind an explicit loading state, and produce the
**same 0..1 scalar the cheap path produces** so nothing downstream knows the
difference. "Move your hand closer" should work because *less light reaches the
cell*, not because anything detected a hand — which is also the more honest
instrument, and why calling it a photoresistor rather than a distance sensor is
the correct framing.

Size-as-distance on the cheap path does work, weakly and honestly: a white
square swept 240 → 42 px tracked to ±1 px, giving **21 distinct levels at a
64-wide buffer and 46 at 128-wide** across a 5.7× size range — but ±1 px
quantization made the raw signal non-monotonic in 54 of 182 size steps, so it
needs smoothing and hysteresis. A bright blob on black is not a hand in a lit
room; it works with a torch, a white card or a dark room, which is arguably
better game design.

**Broadcasting sensor values out-of-band like `cursor` does — no.** Cursors
bypass `Cmd` entirely and never touch the netlist. A sensor must land at a tick
boundary and be in the op log, so it cannot take that path.

**A "trusted sensor" concept — no.** There is no way to prove a camera points
at the world rather than at a script, and pretending otherwise would be the
project's first faked number. The shipped answer is the right one: badge the
run `externally driven — unscored` from the first external write, and let a
reset clear it.

---

## 6. Provenance

Recovered from the `external-inputs` workflow transcripts under
`~/.claude/projects/-Users-daniellewis-ee-game/8f967417-d251-4ff3-a4ec-99cad13916cd/subagents/workflows/wf_b6d148a4-8bd/`
(survey report and design journal), which were the only place this work existed
after `966ccfa` shipped camera-only. Browser measurements are stock Chrome 150
headless via playwright-core on an M3 Ultra with Chrome's fake devices fed from
generated WAV/Y4M files of known content; server numbers are from a release
`cargo test -p server` micro-benchmark run in a scratch worktree.

Sources consulted at the time: [Gamepad API —
MDN](https://developer.mozilla.org/en-US/docs/Web/API/Gamepad_API), [Jumping
the hurdles with the Gamepad API](https://web.dev/articles/doodles-gamepad),
[MediaStreamTrackProcessor —
MDN](https://developer.mozilla.org/en-US/docs/Web/API/MediaStreamTrackProcessor),
[MediaStreamTrack Insertable Media Processing
(W3C)](https://www.w3.org/TR/mediacapture-transform/), [MediaPipe hand
landmarker for
web](https://developers.google.com/mediapipe/solutions/vision/hand_landmarker/web_js).
