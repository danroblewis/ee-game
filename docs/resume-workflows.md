# Six workflows, ready to resume

*Written 2026-08-04, main `bbfa877`. Each of these ran its research phase to
completion and was stopped before its build phase. **The research is cached
and replays for free** — only build and verify actually run.*

## How to resume one

```
Workflow({ scriptPath: "<script>", resumeFromRunId: "<run id>" })
```

Scripts live in
`~/.claude/projects/-Users-daniellewis-ee-game/<session>/workflows/scripts/`.
Full agent results are in
`~/.claude/projects/-Users-daniellewis-ee-game/<session>/subagents/workflows/<run id>/journal.jsonl`
— one `{"type":"result",...}` line per completed agent.

### Three rules, each learned the hard way today

1. **Never change a survey agent's model.** The model is part of the cache
   key. Flipping it invalidates that agent and re-runs the expensive
   research. Build/verify agents are already switched to Opus; the survey
   agents are pinned to whatever they completed on and must stay there.
2. **Open a room in a browser before merging.** A missing `break` from a
   merge froze the entire client while 367 tests, `tsc` and `wirecheck` all
   passed. `noFallthroughCasesInSwitch` closes that one class; the habit
   covers the rest.
3. **Mutation-test any guard you touch.** Plant the violation, watch it go
   red, revert. Three guards in this repo have silently stopped guarding
   while still reporting "ok".

Merging several of these will conflict — they touch `Room`, `SaveFile`,
`hello` and `wirecheck`. Resolve by comparing each side's bracket balance and
only auto-merging where both sides are complete constructs.

---

## 1 · Camera as a part — `wf_6c6b1a9d-575`

**Already fixed and merged** (`575790c`): the ⇧Y arm-then-wait gesture, Delete
removing a layer, and the photocell's dark end (luma was normalised `Y/255`
when cameras emit video range where black is `Y=16`; black now reads its true
1 MΩ instead of 648 kΩ).

**Left:** make the camera an `ElementSpec` rather than bespoke `Layer` room
state. The three fixed faults were all symptoms of that — a primitive every
generic table forgot. As a part it inherits placement, the gate, selection,
Delete, undo, naming and rotation for free. Needs a migration for saved rooms
carrying `Layer`s, and an enable-pin design if a circuit is to switch it.

**Streaming the feed — recommendation is NO for video.** Measured: full
WebRTC 640×480@30 is 60–190 kB/s *per viewer*, and mesh upload scales
×(N−1) on the camera owner. The alternative worth building is a coarse luma
grid, 16×12 @ 10 Hz ≈ **1.5–2 kB/s per camera** — enough to see what the
camera sees, and it keeps the privacy guarantee structural rather than a
policy promise.

---

## 2 · Five games on the machine seam — `wf_72878ac7-ecb`

**Seam recipe is fully documented in the journal.** Server side `MachineSpec`
is `None | Hoist`, `sim_task` destructures a concrete `Hoist`, and
`machine_step`/`machine_msg` hardcode fixture ids 900–903 — generalising
those is the real work. The *client* seam is done and proven (`conveyor.ts`
exists and works). Coupling: `MACHINE_H = 640 µs` against the hoist's 25 ms
mechanical time constant, 39× inside it — derive the equivalent per mechanism
or it rings.

**Blocking dependency:** the curriculum these games must be solvable from has
**no lesson on the motor** — back-EMF, voltage buying speed rather than
position, stall vs running current. That is the hoist's central physics.
Close that hole (extend lesson 9, or add a motor room between 9 and 10)
before the games lean on it.

---

## 3 · Five historical synths — `wf_9e2361c8-129`

**The headline finding: the affordability ordering of history is inverted
here.** Cost ≈ `newton_iterations × devices^1.64` against a 20 µs/substep
budget. Smooth nonlinears (OTA, diode, BJT, MOSFET) buy Newton iterations
globally; discrete nonlinears (OpAmp, 555, and the whole logic family) cost
Newton *nothing*.

- **TR-808 kick** — a bridged-T in an op-amp feedback loop — is one of the
  **cheapest** things this engine can build: linear plus one Newton-free
  op-amp, and its ring frequency is continuous (no substep pitch grid).
- **Moog ladder** is one of the most expensive: a faithful transistor ladder
  is 10+ smooth-nonlinear devices. The honest equivalent is four gm-C stages
  sharing one bias node with global feedback — same law, same self-
  oscillation — and it is affordable in a *small* room (~35–40 devices), not
  in a 77-device one. **Label it honestly**: a one-pole filter wearing a Moog
  name is the kind of lie this project exists not to tell.
- **Hard limit:** audio tap Nyquist is 6.25 kHz, so real hi-hats and cymbals
  (8–12 kHz) are untransmittable. A band-limited hat measured indistinguish-
  able from a snare.

---

## 4 · Control surface — `wf_29881c16-571`

**What exists:** a `Panel` is *only* a rectangle; membership is re-derived
every frame by `panelMembers` (every pin inside the rect). That is why the
synth once needed thirteen regions. Widget kinds today: pot, switch,
indicator (lamp/LED), DC source, probe, scope.

**What to copy:** scopes are the worked example of live sync — `retune`
throttle **keyed by sid**, and the `held(sid, 'rect'|'set')` echo guard where
whoever holds a control owns that half of its state until they let go, with a
trailing op on release. Panels today are cruder: full replace, no hold.

**Not yet room state:** every layout preference is per-player localStorage
(`<plid>:pos`, `:order`, `:shut`, `:mode:<id>`, `rail:<side>`). Explicit
membership and 2-D placement have to become shared, or two players see
different panels.

---

## 5 · Mobile — `wf_1b294ac2-ba9`

**Measured on emulated phones: the game is unusable, not merely awkward.**

- Pan is middle-drag / ctrl+drag / space+drag; zoom is the wheel. **None
  exist on a touchscreen.**
- The canvas has `touch-action: auto`, so the browser seizes every drag:
  one `pointermove` then `pointercancel`. **All drags are dead** — no wiring,
  no part move, no marquee.
- Pinch produces *browser page zoom* (visualViewport 1→5), which on a canvas
  app is the wrong zoom and the only one available.
- Long-press fires no `contextmenu` in emulation; iOS Safari never does. A
  long-press menu must be implemented in-app.

**Already working:** the room browser (~90% — full-width modal, big targets),
and panel widgets, because `.pknob`/`.prow-grip`/`.pscope` already carry
`touch-action: none`.

**Perf:** `sim 0.42x` on a 1001-part room on a phone profile — below 1.0×
detunes the audio, so phones need a smaller room or a different answer.

---

## 6 · Circuit ownership + golf — `wf_eff33c4b-a56`

**The design is written and it is good.** Identity becomes a **seat**: room
state `{n, name, color, token_hash}`, claimed with a bearer token in
localStorage, so ownership survives a reload (per-connection ids do not).
`owner: u8` joins `tier`/`rot`/`name` as a per-instance field that never
reaches a stamp.

**The rule, in one sentence a player would say:** *two circuits may only meet
at ground, unless one of them is shared.* Enforced as a union-find check at
the front of `check_edit`, so ownership refusals are the cheapest refusals in
the gate. Owner-0 parts are the co-op valve — a shared rail stays shared.

**The elegant part, already verified:** `boards_that_share_only_ground_are_
separate_islands` in sim-core proves two circuits meeting only at ground
share no unknown. So "you cannot modify my circuit" can be true
*electrically*, not just socially.

**Watch for:** the placement gate trials the whole document, so one player's
deliberately awful circuit must not be able to make another player's edits
get refused. That attack is in the verify brief.

---

## Also open

- **Voice chat** (`worktree-wf_d85adf77-879-1` @ `5f4dde0`) — DO-NOT-SHIP.
  Architecture validated; three defects block it, all diagnosed in the
  branch's own abandoned WIP: a silent glare death where ~1/3 of joins
  complete SDP, never run ICE, and sit at "0/1 peers" with the mic hot; a
  `.stop()` guard satisfiable without the teardown; and departed peers
  labelled "would need TURN relay". Needs a rebase onto main.
- **Never launched:** a physics builder (chassis/environment with pins on its
  sides), an electronic Rube Goldberg machine, and separate client views (a
  control-panel-only page for phones, a display-only page for a TV).

---

## Resume commands, ready to paste

Substitute `<S>` = `/Users/daniellewis/.claude/projects/-Users-daniellewis-ee-game/8f967417-d251-4ff3-a4ec-99cad13916cd`

| # | script (under `<S>/workflows/scripts/`) | resumeFromRunId |
|---|---|---|
| 1 | `external-inputs-rework-wf_6c6b1a9d-575.js` | `wf_6c6b1a9d-575` |
| 2 | `five-more-games-wf_72878ac7-ecb.js` | `wf_72878ac7-ecb` |
| 3 | `synth-templates-wf_9e2361c8-129.js` | `wf_9e2361c8-129` |
| 4 | `control-surface-wf_29881c16-571.js` | `wf_29881c16-571` |
| 5 | `mobile-wf_1b294ac2-ba9.js` | `wf_1b294ac2-ba9` |
| 6 | `contested-field-wf_eff33c4b-a56.js` | `wf_eff33c4b-a56` |

Suggested order if credits are limited: **3** (synths — self-contained, no
shared files, lowest merge risk), then **1** (camera — three faults already
fixed, the rest is one redesign), then **6** (ownership — worth shipping even
without golf), then **2**, **4**, **5**.
