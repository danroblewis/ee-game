# Architecture for 100,000+ concurrent circuits

A decision document for the layers **above** the solver. It assumes the
per-island restructuring is landing (islands, quiescent-island freeze,
per-island `local_dt`, rayon over islands) and designs what has to sit on top
of it for a world of ~100,000 circuits / ~2,000,000 elements.

It does not re-derive anything already measured. Read first:

- `docs/scale-baseline.md` — where solver time goes, and the per-element floor.
- `docs/scale-parallelism.md` — islands, rayon, quiescence, multirate, GPU=no.
- `docs/plan.md` resolutions 1–8; `docs/arch_backend-netcode.md` §3, §6, §9;
  `docs/arch_sim-engine.md` §2–§5.

**Notation for every number below:**
`[M]` measured in this repo (source cited) · `[D]` derived by arithmetic from
measured inputs · `[E]` estimate with its basis stated.

---

## 0. Verdict in one page

The measured floor is **58.6 ns per element per substep** for a hot island with
a realistic nonlinear mix `[M: scale-baseline "Structural win 1", 50 engines,
5,122 elements, 300 µs/substep]`. At dt = 20 µs that is **341 hot elements per
core**, ~1,500 per 10-core box `[D]`. A 2,000,000-element world that was
uniformly hot would need **~1,300 boxes**.

So the architecture is not about making the solver faster. It is about the
ratio

```
    hot elements  ≈  (elements a player sees in detail)
                     × (fraction of those that are electrically moving)
                     ÷ (average local-dt multiple k of those islands)
                     × (concurrent players)
```

and every layer in this document exists to push one of those four terms.

Three consequences drive everything:

1. **The binding resource is concurrent *detailed observation*, not stored
   circuits.** Storing 100k circuits costs ~60 MB cold `[D]` and ~2,000 doc-ops
   per second of write traffic `[E]`. Both are trivial. Stepping the ~2% of
   them that someone is watching closely is the entire cost.
2. **Per-substep cost must be O(awake), never O(world).** Not "cheap per
   frozen island" — *zero*. Polling 100,000 frozen islands at even 1 ns each
   costs 100 µs per substep, 5× the entire 20 µs budget `[D]`. Frozen entities
   must be woken by push from an awake neighbour, never discovered by a scan.
3. **Regions are separated by transmission-line delay, and nothing else.** A
   Bergeron corridor with delay τ lets the two sides be solved independently
   for τ of sim time — that is the only decoupling primitive in the design that
   is *physics*, not a fudge. Everything about sharding follows from where you
   are willing to put a τ.

Honest capacity, worked in §1.4: with today's measured constants and a
plausible LOD policy, **one 10-core box supports 20–240 concurrently building
players** on a world of arbitrary stored size. 2,000 concurrent players is
~25 boxes. 100,000 circuits *simultaneously observed at schematic band* is not
reachable and §6 says what would have to change.

---

## 1. The budget model

Everything downstream references this section. If you change one of these
constants, re-derive the tables rather than arguing about them.

### 1.1 Planning constants

| symbol | value | source |
|---|---|---|
| `C_hot` | **58.6 ns** per element per substep, all-in (stamp + solve + accept) | `[M]` scale-baseline §"Structural win 1": 50 independent engines, 5,122 elements, 300 µs/substep. Cross-checked: `[M]` scale-parallelism §1.3 case B gives 62–75 ns/element/substep across 113–4,520 elements |
| `C_floor` | 14–68 ns per element per substep, bookkeeping only (no factorization at all) | `[M]` scale-baseline §7 |
| substeps/s | 50,000 at dt = 20 µs; **100,000 at the plan's nominal dt = 10 µs** | `[M]` server `DT`; `[M]` plan resolution 1 |
| substep budget | 20 µs (dt = 20 µs) / 10 µs (dt = 10 µs) | definition of real time |
| rayon over islands, per-tick joins | **3.5–4.9×** on 10 cores, a lower bound (measured on a contended box) | `[M]` scale-parallelism §4.1 |
| rayon per *substep* joins | **0.24×–1.72×** — a slowdown at demo scale | `[M]` scale-parallelism §4.1. Do not do this. |
| unrolled n ≤ 8 kernels | 1.5–3× on the solver component of small islands | `[E]` scale-parallelism §4.3 — **not measured** |
| quiescent share, 20% "active" builds | 75% of districts fully static within 250 ms of sim time, 0 exceptions | `[M]` scale-baseline §"Structural win 2" |
| multirate tolerance | 12 of 17 golden islands bit-quiet or sub-nV at k = 100 | `[M]` scale-parallelism §5.3 |

### 1.2 Derived per-box capacity

```
hot elements per core   = substep_budget / C_hot
hot elements per box    = that × rayon_speedup
```

| dt | per core `[D]` | per 10-core box, measured constants only `[D]` | + unrolled kernels `[E]` |
|---|---:|---:|---:|
| 20 µs | 341 | **1,536** | ~2,300 |
| 10 µs (plan nominal) | 171 | **768** | ~1,150 |

Use **1,500 hot elements per 10-core box at dt = 20 µs** as the planning
number. It is measured-only; the 2,300 requires work nobody has prototyped.

### 1.3 Tier cost algebra

Define an island's tier by how often it is visited:

| tier | visits per room substep | cost per element per room substep |
|---|---|---|
| HOT | every substep | `C_hot` = 58.6 ns |
| WARM(k) | every k-th substep, `local_dt = k·dt` | `C_hot / k` |
| FROZEN | never | **0** (see §2.6 — this must be literally zero, not small) |
| COLD | not resident | 0 |

Box budget equation, per box:

```
    Σ_hot E_i  +  Σ_warm (E_i / k_i)   ≤   1,500        (dt = 20 µs)
```

The whole architecture is a machine for keeping the left-hand side small while
players believe they are looking at a live world of two million elements.

### 1.4 What that buys, as a function of the LOD policy

Let `V` = elements inside one player's *schematic-band* viewport, `a` = the
fraction of those that are electrically moving (`1 − quiescent`), `k̄` = the
mean local-dt multiple those islands run at. Then players per box =
`1,500 / (V·a/k̄)` `[D]`.

**Players per 10-core box, dt = 20 µs** `[D]`:

| V (elements in detail view) | a=0.25, k̄=1 | a=0.25, k̄=4 | a=0.25, k̄=8 | a=0.50, k̄=4 |
|---:|---:|---:|---:|---:|
| 100 | 60 | 240 | 480 | 120 |
| 300 | 20 | 80 | 160 | 40 |
| 1,000 | 6 | 24 | 48 | 12 |
| 3,000 | 2 | 8 | 16 | 4 |

Halve every cell at dt = 10 µs.

Read this table as the design brief for three other teams:

- **Rendering/LOD owns `V`.** The difference between "schematic band shows the
  700 elements on screen" and "schematic band shows the 150 elements in the
  focused plot, everything else is block-band aggregates" is a **4–5× server
  capacity difference**. This is a *client design decision with a server
  price tag*, and it should be decided with this table in hand.
- **Quiescence owns `a`.** Measured at 25% for a 20%-active district mix
  `[M]`. A world full of 555 astables has `a → 1`. Contracts and level design
  that reward steady-state DC delivery keep `a` low; a metagame of blinkenlights
  raises it. Worth stating in the game-design doc.
- **The local-error controller owns `k̄`.** Measured headroom is enormous
  (12/17 islands tolerate k = 100 `[M]`), but only a *state-derived* controller
  may set it (§5.4). A conservative shipped `k̄ = 4` triples capacity.

**Sanity check against the goal.** 2,000 concurrent players at V=300, a=0.25,
k̄=4 → 80 players/box → **25 boxes**, holding a world of any stored size. The
same 2,000 players at V=1,000, k̄=1 → 333 boxes. The LOD policy is worth an
order of magnitude more than any solver work left on the table.

---

## 2. Activity tiering and interest management

### 2.1 The tiers

Five states. The distinction between FROZEN and SUSPENDED is the load-bearing
one and is *not* a performance distinction — it is a truth distinction.

| tier | definition | truth on resume | cost/substep |
|---|---|---|---|
| **HOT** | stepped at room dt | — | `C_hot`/element |
| **WARM(k)** | stepped at `k·dt`, k ∈ {2,4,8,…,128} | true trajectory of the same netlist at a coarser dt; error bounded by the measured k-table `[M: scale-parallelism §5.3]` | `C_hot/k` |
| **FROZEN** | not stepped. **Provably at a fixed point of a time-invariant system** (§2.4) | *exact*: x(t+T) = x(t) for all T | 0 |
| **SUSPENDED** | not stepped, but the island has live dynamics. Its **local sim clock stops** | exact continuation of the true trajectory, shifted in time. The shift is surfaced, never hidden | 0 |
| **COLD** | evicted from RAM to the store (§4.5) | as FROZEN or SUSPENDED, plus a materialization cost | 0 |

SUSPENDED is licensed by an existing design decision, not invented here: the
game design already specifies **"no offline automation (sim pauses in empty
rooms)"** `[docs/plan.md` game-design summary, `docs/game_design.md` §3]`. An
unobserved plot whose clock is stopped is that rule, applied at plot
granularity instead of room granularity.

### 2.2 What must run HOT (the complete list)

The server does *not* need to step the world at full rate. It needs to step
exactly these:

1. Islands with an active **probe or scope subscription** — a scope is a
   contract with the player about bandwidth, so a subscription pins `k = 1`.
2. Islands with an **audio tap** (speaker). At `AUDIO_EVERY = 4` and dt = 20 µs
   the audio stream is 12.5 kHz; `k > 1` aliases it. Pins `k = 1`.
3. Islands running a **server-side measurement chip**: contract verifiers,
   energy meters (∫V·I at a service entrance), fuse i²t budgets. Anything whose
   *integral* is a game value cannot be suspended without changing the outcome.
4. Islands under **co-simulation with a mechanism** (`crates/machine`): the
   explicit mechanical integrator's stability is tied to its `h`.
5. Islands **intersecting a schematic-band interest set** *and* not quiescent.
6. Islands **edited or interacted with** in the last `T_cool` (§2.3).
7. Islands whose **corridor far-end is hot and moving** (§3.3).

Everything else is WARM, FROZEN or SUSPENDED. Note that (1)–(4) are *player-
purchased* hot slots and are therefore the anti-abuse surface (§2.8).

### 2.3 Promotion and demotion

Built on the M5 R-tree viewport interest `[plan.md M5;
arch_backend-netcode.md §6]`, with one change: **index islands, not
elements.** 100,000 island AABBs is a ~10 MB `rstar` index `[E: ~100 B/entry]`;
2,000,000 element AABBs is ~200 MB and 20× the query cost. Element-level
filtering is a linear scan of the ~20 elements in a hit island.

Per client, per `ViewportUpdate` (class 2, ~5 Hz `[arch_backend §6]`):

```
rect + 30% margin  →  R-tree query  →  island set
zoom band decides the required tier:
    schematic band  →  HOT (or WARM with k capped by the probe/audio rules)
    block band      →  WARM, k ≤ 16   (aggregates update at ≥ 10 Hz)
    world band      →  no requirement; region aggregates only
```

Rules, with numbers:

| event | action | value | why |
|---|---|---|---|
| island enters any interest set at required tier > current | **promote immediately**, same tick | — | a player must never wait on a tier change |
| island leaves every interest set | demote HOT→WARM after **`T_cool` = 2 s** (60 ticks @ 30 Hz) | 2 s | a panning camera must not thrash. 2 s ≈ one deliberate look-away |
| island stays out of every interest set | WARM→FROZEN as soon as §2.4 passes | — | free; freezing is exact |
| island stays out of every interest set and cannot freeze | WARM(k)→WARM(2k) every 5 s up to k=128, then SUSPENDED after **`T_susp` = 30 s** | 30 s | a player who steps away and comes back sees continuity |
| region has no client within 2 world-band viewports | whole region → COLD after **`T_cold` = 5 min** | 5 min | §4.5 |
| **promotion admission control** | at most **200 islands promoted per region per tick** | `[D]` 200 islands × 20 elements × ~0.3 µs/element of per-island compile ≈ 1.2 ms/tick | a player teleporting across a frozen district must not stall the tick. Un-promoted islands render from aggregates with an explicit "warming" treatment |

### 2.4 When freezing is exact (the correctness argument)

This is the part that must not be got wrong, because "resume must produce
solver-truth, never a plausible-looking fake" is a pillar.

The measured quiescence criterion is *a detector*, not a *licence*:

> no unknown moves more than 1 µV in a 20 µs substep for 500 consecutive
> substeps `[M: scale-baseline "Structural win 2"]`

Taken literally that threshold permits 0.05 V/s of true drift. Freeze for
60 s and you would be 3 V wrong. That would be a fake. The measurement itself
says the real behaviour is much better ("reaches a fully static DC state and
*stays* there, 0 exceptions"), and `[M: scale-parallelism §5.2]` measured
`max|Δv| < 1e-9 V` per substep on 13 of 17 goldens — but a threshold is not an
argument.

**The argument that licences an unbounded freeze is structural:**

> If an island's netlist is **time-invariant** (no source with an explicit `t`
> dependence, no external write, no time-varying corridor far-end) and its
> state is a **fixed point** of the discrete update, then `x(t+T) = x(t)`
> exactly, for every T. Freezing introduces no error at all.

So the freeze gate has two halves, and both are required:

**Structural half** — every element in the island satisfies:
- `VoltageSource`/`Rail` with `amp == 0.0` (pure DC), or no source at all;
- no arbitrary/sketched source, no MCU or event-layer block;
- no `ParamWrite` producer attached (no `machine` co-sim, no hoist);
- every Bergeron port's far end is itself FROZEN or driving a constant history
  term.

**Numerical half** — `max|Δx| < ε_freeze = 1e-9 V` (and the branch-current
analogue) for `N = 500` consecutive substeps. Worst-case accumulated error over
the detection window is `N·ε = 5e-7 V` `[D]`, three orders below the 16-bit
display quantization.

**An island that fails the structural half may never be FROZEN. It is
SUSPENDED, and its local clock stops.** That is the honest bookkeeping: a
suspended oscillator has not been "left running", it has been paused, and the
pause is a fact the client is told about.

**The correctness gate (make this a CI test).** For every island the freezer
freezes: step it continuously for 10 s of sim time and compare
`state_hash()` to the frozen state hash. **They must be bit-identical.** If
they are not, the freeze criterion is wrong, not the test. This is cheap, it
is deterministic, it runs in the existing golden harness, and it converts the
entire "is freezing a lie?" question into a hash comparison.

### 2.5 Arrival: what happens when a player reaches a frozen region

Decision table. **Never DC-solve.**

| stored tier | on arrival | cost | truth |
|---|---|---|---|
| FROZEN | **resume from stored state.** Publish it as the first snapshot immediately, stamped with the island's local sim time | 0 | exact (§2.4) |
| WARM(k) | promote to the required k; the trajectory continues | 0 | true trajectory, accuracy improves |
| SUSPENDED | **resume at the island's local time.** Client is told `t_local` and `skew = t_region − t_local` | 0 | exact continuation of the true trajectory of that netlist, shifted in time |
| COLD | materialize (§4.5), then as above | `[E]` ~1 ms per 20-element island (postcard decode + island compile) | unchanged |
| quarantined | stays quarantined; diagnostic surfaced; retry on backoff (§4.8) | 0 | honest failure |

**Why not DC-solve.** `arch_sim-engine.md` §3 already forbids it: *"No DC
operating-point solve ever — simulation starts from zero state; the startup
transient is the OP."* At MMO scale the reason gets sharper: hysteretic
circuits — latches, relay seal-in, Schmitt/relaxation oscillators, the 555's RS
latch (`ElemState::region` doubles as it) — have **multiple** DC solutions. A
DC solve picks one. That is manufacturing a state the player's circuit was not
in, which is precisely the forbidden thing. It would also silently un-latch
every alarm and every islanding relay in a district the moment a player walked
past.

**Why fast-forward is not a general answer, with the arithmetic.** Closing a
skew of `T` seconds on an island of `E` elements costs
`T × 50,000 × E × 58.6 ns` `[D]`:

| island | 1 s of skew | 60 s of skew | 1 h of skew |
|---|---:|---:|---:|
| 20 elements (one circuit) | 59 ms | 3.5 s | 3.5 min |
| 100 elements (one plot) | 293 ms | 17.6 s | 17.6 min |
| 2,000 elements (a district) | 5.9 s | 5.9 min | 5.9 h |

So fast-forward is viable **only for sub-second skews**, which in practice
means only for the WARM→HOT case where an island has been running at k=128 and
is a few hundred µs behind a corridor peer. Policy:

- **Fast-forward is allowed up to a hard cap of 200 ms of skew and 5 ms of
  wall per island per tick.** Above that, the island resumes with declared
  skew.
- If the game later demands wall-clock continuity for a specific class of
  object (a district clock, a scheduled contract), the *only* honest mechanism
  is a catch-up rate cap — e.g. run at 2× real time until caught up, with a
  visible "resynchronising" state — and it must be budgeted as above. Never an
  extrapolation, never an interpolation, never a re-settle.

**What the player actually sees on arrival at a suspended plot.** The plot's
LEDs are exactly as they were, its oscillator continues mid-cycle, its meter
reads what it read. Diegetically this is a plot that was powered down and came
back; the world band can render suspended plots dimmed and the block band can
show "idle, resumed" — but note this leaks information about rival plots and is
therefore subject to §2.9.

### 2.6 Hard invariant: per-substep cost is O(awake)

This is the invariant that makes the whole thing work and it is easy to break
by accident.

> **No per-substep loop, anywhere in the server, may be proportional to the
> number of islands, elements or corridors in the world. Only to the number
> that are awake.**

Arithmetic that forces it `[D]`: 100,000 islands polled at 1 ns each = 100 µs
per substep = **5× the entire 20 µs budget**, before any circuit is solved.
Even a 1-ns-per-island liveness check is fatal.

Consequences, each of which is an implementation constraint:

- **Frozen islands are woken by push, not by poll.** The wake list is written
  by awake neighbours (§2.7), by the op pipeline, and by the interest manager
  at tick boundaries. Nothing scans.
- **A corridor is stepped only if at least one end is awake.** If both ends
  are frozen, the corridor is frozen. Corridor bookkeeping therefore also
  costs O(awake).
- **The scheduler is a bucketed due-list (timing wheel), not a sorted scan.**
  HOT islands live in one contiguous SoA array walked every substep; WARM(k)
  islands live in bucket `k` and the scheduler pops bucket `k` when
  `substep_index % k == 0`. Cost per substep = `|hot| + Σ_{k | i} |bucket_k|`.
- **Per-tick work may be O(awake + Σ interest sets)**, never O(world). The
  interest R-tree query is per client, not per island.

Add a debug-build assertion and a benchmark that *proves* this: hold 500 hot
elements constant and vary the frozen world from 5,000 to 500,000 elements;
substep wall time must stay within 10%. That is the decisive Stage-2 gate
(§5).

### 2.7 The complete wake list

A FROZEN or SUSPENDED island wakes on exactly these, and nothing else:

1. Any `DocOp` or `InteractOp` addressed to one of its elements.
2. Any `ParamWrite` from a co-simulated mechanism.
3. **A Bergeron port's incoming history term changing by more than
   `ε_wake`** — checked by the *awake* far end when it writes the shared
   history word, never by the frozen side. `ε_wake` = `ε_freeze` = 1e-9 V.
4. A probe, audio tap, contract verifier or meter attaching.
5. Interest-set promotion (§2.3), subject to admission control.
6. The damage model crossing a threshold. While frozen, an island's dissipated
   power is constant by construction, so its first-order thermal state has a
   **closed-form** solution over the frozen interval — integrate it exactly on
   wake rather than stepping it. This is not "faked electricity": the
   electrical numbers are untouched and the thermal ODE is the *same* model,
   integrated exactly instead of by Euler. It is more accurate, not less. Add
   a golden test comparing closed-form vs stepped decay.
7. Quarantine backoff expiry (§4.8).

### 2.8 Per-player observation budget (the compute fuse)

At MMO scale CPU is a contested shared resource, exactly like the grid. The
game already has the mechanism: **the service entrance's unbypassable
auto-resetting main fuse.** Lift it.

- Every island has an owner (its plot). The scheduler maintains a per-island
  cost EMA (`Island.cost_ema` already exists in the `arch_sim-engine.md` §2
  struct) and charges it to the owner.
- Every connected player has a budget in **hot-element-substeps per second**.
  Suggested starting value: `1,500 × 50,000 / players_on_box` — i.e. an equal
  share of the box, with a floor.
- Overrun response, in order: (a) raise `k` on that player's non-pinned
  islands; (b) refuse new probe/audio subscriptions with a diegetic message;
  (c) dilate the player's own sync group and show the existing "running at
  0.4×" badge `[arch_backend §3]`.
- **Overrun never degrades another player's islands.** That is the whole point
  of per-island partitioning, and it is the anti-DoS story: an adversarial
  circuit is a fuse that trips on its own author.

The existing `MAX_PROBES = 8` cap is the prototype of this rule; generalize it
rather than adding more ad-hoc caps.

### 2.9 Visibility masks, and tier as a side channel

Interest filtering is the anti-wallhack layer `[arch_backend §6]`. Tiering adds
a new leak: **an island's tier is information about its contents.** "That plot
is FROZEN" means "it contains no oscillator, no AC source, no MCU" — a free
reconnaissance result that a real engineer would have to *measure*.

Rules:

- Never publish tier, `k`, `local_dt`, `skew` or `cost_ema` for an island the
  client does not have edit or ally rights to.
- For rival plots, publish only quantities observable **at the corridor port**:
  port voltage, port current, power flow, and the trunk's own thermal state.
  Those are always at least WARM (the corridor's near half belongs to the
  region, §3.1), so they are always available.
- The world-band rendering of a suspended rival plot must be indistinguishable
  from a quiescent one. Both render as "dark".

### 2.10 The client is a scaling resource — and its own wall

The WASM preview `[arch_backend §8]` is the reason a player can perceive a
smooth 60 fps world while the server steps their islands at k = 4. Presentation
may be client-solved; **consequence must be server-solved** (payouts, damage,
meters, trips — all from `sim-core::measure` on the sim thread, which is the
existing anti-cheat property).

But the client hits the same floor. A client with a 4 ms per-frame sim budget
covering 16.7 ms of sim time at dt = 20 µs runs 835 substeps, so it sustains
`4 ms / (835 × 58.6 ns)` = **82 elements** `[D]`, and less in WASM `[E: 40–55
elements at a 1.5–2× WASM penalty]`.

Therefore: **the client preview must be multirate too.** It runs the focused
island at k = 1 and everything else in its interest set at k = 8–32. Its
reduced fidelity is acceptable *precisely because it is not authoritative* —
every reseed (20 Hz) corrects it from server truth `[plan resolution 5]`. This
should be stated in the frontend plan, because "the client can just simulate
what it sees" is otherwise a natural and wrong assumption.

---

## 3. Regions and sharding

### 3.1 What a region owns

A region is a **quadtree cell of the world plane** plus everything geometrically
inside it:

- every element whose pins are inside the cell;
- every island wholly inside the cell (islands never span regions — §3.2
  guarantees it);
- the **near half of every corridor crossing its boundary**: each side owns its
  own Bergeron half-line, its own history buffer, and its own end stamp;
- one op-log + checkpoint store (one SQLite file, WAL, one writer thread
  `[arch_backend §9]`);
- one island R-tree, one scheduler, one sim clock, one tick loop;
- the plots, contracts, meters and damage state of everything above.

Sizing targets `[D]` from §1.2 and §4.5:

| bound | value | reason |
|---|---:|---|
| stored elements per region | ≤ **50,000** | keeps a region's cold form ~1.5 MB and its compiled form ~15 MB; 2M elements = ~40 regions |
| hot elements per region | ≤ **1,500** | one 10-core box at dt = 20 µs |
| regions per box | 1 sync group (§3.5); several if they are small | |

### 3.2 The seam rule

> **No electrical connection may cross a region boundary except through a
> corridor with propagation delay τ ≥ dt.**

This is the load-bearing constraint of the whole sharding design and it has a
design cost that must be paid honestly: **the plane is not homogeneous.** The
editor must refuse a plain wire drawn across a seam, and refuse a paste or a
move that would straddle one.

Mitigations that make this liveable rather than annoying:

- Seams are cut through **neutral-ground corridors**, which the game design
  already places between plots `[game_design.md §2]`. A player never draws a
  wire across neutral ground casually; they run a rated feeder, which is the
  corridor part.
- The refusal is diegetic and matches an existing rule: *"anyone may run wires
  across neutral ground"* becomes *"a run that crosses the trunk right-of-way
  must land on the trunk"*, with the DRC hint saying so.
- Region boundaries are chosen at world-generation time to follow the corridor
  grid, so seams and trunks coincide by construction.

**Corollary: ops are atomic within a region only.** A multi-mutation op (paste,
Create Block) that spans a seam is rejected, not split. This removes
distributed transactions from the design entirely.

### 3.3 Intra-host corridors (the normal case)

Two regions on the same box are scheduler partitions in one process. Their
corridors are ordinary Bergeron lines at `τ = k_c · dt`, k_c small (1–8).

**Implementation constraint, from the O(awake) invariant:** the corridor must
be **two ordinary devices** — one stamped inside each island, exactly like any
other element — plus a **shared 2-word history slot** written by whichever end
is awake. It must *not* be a scheduler-level exchange phase, because a phase
that iterates corridors is O(corridors), and corridors are as numerous as
plots.

Cost check `[D]`: with the device formulation, each corridor end costs one
element-visit (already inside `C_hot`). The only extra is the 2×f64 history
write, ~2 ns. At 500 awake corridors that is 1 µs per substep, **5% of the
budget**. With a scheduler-phase formulation over 100,000 corridors it would be
200 µs, **10× the budget**. The two designs differ by 200×.

`scale-parallelism.md` Appendix B flags corridor coupling cost as **not
measured**. It is the main risk to the Stage-4 gate (§5) and must be measured
before regions ship.

### 3.4 Inter-host interties (the long-haul case)

A corridor between two *hosts* cannot exchange a history word per substep:
50,000 packets/s per corridor is not a network protocol. The exchange must be
**once per tick**, i.e. `τ ≥ 1 tick` = 33.3 ms at 30 Hz, 16.7 ms at 60 Hz.

A Bergeron line with τ = 33 ms is a 6,700 km transmission line `[D: v = 2e8
m/s]`. Not credible as copper on a map where TDR locates a splice "at 210 m"
`[game_design.md §4]`. Two ways to be honest about it:

**Option A — band-limited long trunk (physics-pure, expensive).** Keep the
Bergeron line, declare the trunk's own low-pass corner (series L, shunt C give
`f_c`), and transmit the history stream at its Nyquist rate rather than at dt.
At `f_c` = 1 kHz that is 67 samples/tick = 536 B/tick = **16 kB/s per trunk**
`[D]`; 100 inter-host trunks = 1.6 MB/s. Requires a documented reconstruction
(ZOH or linear) applied *identically at both ends* and an error budget measured
against a single-process reference.

**Option B — converter intertie (recommended).** Model the inter-host link as
what a real long-haul link is: a **pair of power-electronic converters** (an
HVDC-style intertie) for power, and a **store-and-forward modem** for data.
Each end is a real stamped device — a controlled source with a real control
loop of ~100 Hz bandwidth, a real output impedance, real losses, and a real
current limit. The only thing crossing the network is the other end's measured
`(V, I, setpoint, status)`, one tick old: **4 × f64 = 32 B per trunk per tick**,
i.e. 96 kB/s for 100 trunks at 30 Hz `[D]`.

Option B is recommended because:
- it is a **real device**, so every number is still solver-truth — each end
  solves a real converter driven by a real measurement from the other side;
- one tick of control-loop latency is physically ordinary for an intertie, so
  nothing is fudged;
- it gives the game a legible object: interties are visible, buildable,
  rate-limited, trippable, and a natural site for conflict;
- it is 170× cheaper on bandwidth and needs no reconstruction filter.

This is the "regions separated and connected by some other mechanism" the owner
already accepts, arrived at from the physics rather than from convenience.

**Host boundary = intertie boundary.** Regions connected by intra-host
corridors (τ < 1 tick) form a **sync group** and must live on the same host.
Regions connected only by interties need no barrier and may live anywhere.

### 3.5 Consistency model

| scope | model | guarantee |
|---|---|---|
| within a region | **strict / linearizable** — one clock, one op-log, serialized ops, exactly as `crates/server` does today | every client sees the same op order and the same sim state |
| within a sync group (corridor-coupled, same host) | **lockstep with τ decoupling** — all regions advance the same global t; each consumes its neighbours' history from t−τ | exact: it is the physics of the line, not a compensation |
| across an intertie | **bounded staleness of exactly one tick** | the far end's `(V,I)` is one tick old, by construction of the device |
| across the world | **no global clock, no global order** | regions in different sync groups have independent sim clocks |

**There is no rollback anywhere.** The delay is physical, not a
lag-compensation mechanism. This preserves plan resolution 5 ("hard reseed,
never physics rollback") at region scale.

**Dilation across a sync group.** If region A holds real time and neighbour B
is at 0.3×, A consumes B's corridor history faster than B produces it. Two
options and one decision:

- *Rejected:* A extrapolates B's history. That is fabricating electricity.
- *Rejected as a default:* A holds the last sample (ZOH) and marks the corridor
  stale. Electrically this is a constant-source boundary — crude but not a lie
  — and it is the correct **fallback**, not the policy.
- **Decision: A dilates to match the slowest member of its sync group.** Sim
  time is a per-sync-group quantity, exactly as `arch_backend §3` and
  `arch_sim-engine §4` already specify per island. The existing "plot running
  at 0.3×" badge becomes a district-wide badge.
- **Floor: if the group would dilate below 0.25×, the corridor trips instead.**
  This is the game's own islanding relay — an undervoltage/underfrequency tie
  opening `[game_design.md §6 "Islanding"]`. The overloaded region islands
  itself, the healthy region keeps real time, and the player-visible event is a
  breaker opening, which is a *thing that happens in this game*. Graceful
  degradation of the shard fabric is expressed as gameplay rather than as an
  error state. Reclose is the existing auto-reclose logic.

### 3.6 Crossing a boundary: what the player sees

**No entity ever migrates.** Elements are pinned to a region by geometry; only
*clients* move. This deletes live object migration — the hardest problem in
sharded MMOs — from the design.

1. The client's viewport straddles the seam. It subscribes to interest sets in
   **both** regions. One WebSocket; an edge router multiplexes to both region
   hosts (class tags already exist `[arch_backend §5]`).
2. Each region publishes its own snapshots with its own `sim_time`. The client
   renders each with its own clock. Nothing is drawn as a continuous quantity
   across the seam except the trunk itself, and the trunk is drawn from the
   **near end's** port values on each side — so the τ of delay appears exactly
   where the physics puts it, in the trunk's glow.
3. Edit authority follows the element's region. Ops route to the owning region.
4. When the viewport centre crosses, the client's "home" region changes; op
   routing follows. No state moves, no handshake, no freeze.
5. Latency: a player standing on a seam has two ops-pipelines. Their own edits
   are optimistic-local as always `[arch_backend §4]`, so the seam is invisible
   at human timescales.

**Failure mode to design for:** a region host dies. Its half of every corridor
stops producing history. The surviving side sees the corridor go stale → §3.5's
dilation floor → the tie trips → the surviving region islands and keeps
running. Players near the seam see a blackout on the far side and a breaker
open on theirs. Recovery is region resume from checkpoint (§4.4), then reclose.
**No player loses work**, because their work is in the dead region's op-log,
which is on disk.

### 3.7 The S7 island-merge threshold

S7 asks: when should two corridor-coupled islands be merged into one matrix
instead of decoupled by a Bergeron line? There are two answers and they must
not be confused.

**(a) The fidelity rule — when is a corridor's τ a lie?**

Bergeron requires `τ = max(dt, ℓ/v)`. When `ℓ/v < dt` the extra delay is
manufactured, and manufacturing it changes the physics visibly:

- at dt = 20 µs, `ℓ/v = dt` needs **ℓ = 4,000 m** `[D: v = 2e8 m/s]`. Every
  run on the map is shorter. So *geometrically*, everything merges.
- Forcing `τ = 20 µs` on a short run implies `L = Z₀τ` and `C = τ/Z₀` — at
  Z₀ = 50 Ω, that is **1 mH and 0.4 µF** for what should be a ~1 µH / ~40 pF
  jumper `[D]`. That is a huge lumped reactance, a 25 kHz round-trip
  resonance, and it *will* show up on a scope and destabilise a fast feedback
  loop routed through the corridor.

**Resolution: decouple by declaration, not by geometry.** A corridor trunk is a
distinct part with a **published datasheet**: `Z₀`, `τ`, `R/m`, ampacity. The
editor only offers it for runs above `L_min` (set at the service-entrance /
plot-boundary scale). Plain wire runs below `L_min` remain lumped R-L-C
elements and **merge** their islands. The manufactured delay stops being a
hidden fudge and becomes a property of a part the player chose — with a visible
25 kHz notch that a scope can find, which is a feature, not a bug.

**(b) The performance cap — how big may a merged island get?**

Merging is not free: dense LU is superlinear. From the measured MNA kernel
column `[M: scale-baseline §5]` — 27 µs at n=78, 120 µs at n=161, i.e. ≈ n^2.5
in this range:

| merged n | one MNA factor `[D]` | nonlinear (×2 NR iters) | % of a 20 µs substep |
|---:|---:|---:|---:|
| 32 | 2.9 µs | 5.8 µs | 29% |
| 40 | 5.1 µs | 10.2 µs | 51% |
| 48 | 8.0 µs | 16.1 µs | 80% |
| 64 | 16.4 µs | 32.8 µs | **164% — over budget** |

Linear islands only pay the triangular solve per substep (11.5 µs at n=136,
O(n²) `[M]`): n=64 → 2.5 µs, n=96 → 5.7 µs, n=128 → 10.2 µs.

**Merge caps (dense kernel):**

- **nonlinear merged island: n ≤ 40** (51% of the substep budget for the
  factor; leaves room for stamping and the rest of the world);
- **linear merged island: n ≤ 96** (29% for the solve);
- these rise once the fixed-pattern sparse LU (S3) lands — that is exactly the
  "one large connected island" case `[scale-parallelism §5.4]` where sparse
  wins 39–309×;
- **a corridor that crosses a region seam may never be merged**, regardless of
  either rule. Seams outrank fidelity.

**S7's tuning task, restated:** the spike should measure *feel*, not
performance — does a 20 µs manufactured delay on a plot service entrance feel
laggy (it should not: 20 µs is 1/1600 of a frame), and does the 25 kHz
resonance appear anywhere a player will trip over it. The performance question
is answered by the table above.

### 3.8 Region split and merge

A region that grows past 50,000 stored elements or persistently past 1,500 hot
elements must split. Splitting relocates the seam, which changes which
connections are legal, so it is **not** an online operation.

- **Split/merge is offline**, using the existing park/resume primitive
  `[arch_backend §2, §9]`: park the region (checkpoint = the migration format,
  for free), re-cut the quadtree cell along a corridor line, resume as two
  regions.
- Trigger it at low occupancy, on the same schedule as maintenance.
- If a cell cannot be cut along a corridor (a player has built one enormous
  connected mega-plot), **it cannot be split**, and that region's ceiling is one
  box. This is a real limit and should be surfaced as a build-size cap at the
  plot level rather than discovered as a production incident.

---

## 4. The non-solver costs

`scale-baseline.md` §6 and `scale-parallelism.md` §1.5 both warn that these
dominate before solver work matters. At 2M elements they do not "dominate" —
they are catastrophic by four orders of magnitude. Every one of them is a
data-structure problem, not an algorithm problem.

### 4.1 `compile()` — O(elements²) today

Measured `set_elements` `[M: scale-baseline §6]`: 0.14 ms at 516 elements →
10.17 ms at 5,122 → 168.81 ms at 20,495 → **1,261 ms at 51,252**. That is
E^2.09 over the top decade. Extrapolating to 2,000,000 elements:
**~32 minutes for a single edit** `[D]`.

The cause is a linear scan: `junctions: Vec<(Point, usize)>` interned by
`points.iter().position(...)`, plus the same shape in `solve_wire_currents`.

**Requirement.** An edit's cost must be `O(size of the affected island) +
O(log W)`, never `O(W)`.

- per-island junction interning via a hash map scoped to the island;
- a **global point → island** index (grid hash of cell → island list, or the
  island R-tree) so an edit finds its island in O(log W). At 2M elements ×
  ~2 pins the direct map is 4M entries × 16 B = **64 MB** `[D]`; a coarse grid
  hash is smaller;
- an add that joins two islands merges them (subject to the §3.7 caps); an
  add that splits one splits them. Both are scoped to the touched islands.

**Gate:** add/remove one element in a 2,000,000-element world at **p99 ≤ 100 µs**.

### 4.2 `frame()` and display extraction — O(elements) per tick

Measured `[M: scale-baseline]`: 8.22 ms at 5,122 elements = **1.60 µs per
element**. At 2M elements one call is **3.2 seconds** `[D]`. The current server
calls it once per tick unconditionally and then serializes the whole thing to
JSON.

The JSON is the larger crime. `crates/server/src/main.rs` builds
`Vec<[f64;15]>` for every element and broadcasts it to every client every tick
at 30 Hz — a full-world, uncompressed, O(world) message. At an estimated
120–250 B/element/tick `[E: 15 serde_json f64 fields]` that is **~1 MB/s per
client for today's 150-element demo**, and **~400 MB per tick per client** at
2M elements `[D]`.

**Requirements.**

- `frame()` becomes **pull-based and interest-scoped**: `frame_for(&[IslandId])`
  writing into a caller-owned buffer. No allocation proportional to world size,
  ever, on any path.
- Wire-current reconstruction (`solve_wire_currents`) runs only for wires that
  are in someone's interest set or probed — it is already the O(E²) half.
- Per-client payload is the binary Tier-B delta of `arch_backend §5`
  (interest-table index, changed-field bitmask, 16-bit quantized values,
  32-deep ack ring).

**Gate:** per-tick extraction + encode ≤ **2 ms** for the union of all clients'
interest sets on a 2,000,000-element region-set, with a per-client cap of 250
changed entities per snapshot.

### 4.3 Snapshot and delta bandwidth

Per-client, per-snapshot budget at 20 Hz:

```
250 changed entities × ~13 B (idx + mask + 2–4 quantized fields) = 3.25 kB
× 20 Hz = 65 kB/s per client
```

That is the design cap and it must be **enforced**, not hoped for: if a
client's interest set has more than 250 changing entities, the excess collapses
to block-band aggregates. Otherwise a player who parks over a district of
oscillators gets 500 kB/s.

Fleet arithmetic `[D]`:

| quantity | value |
|---|---|
| 2,000 clients × 65 kB/s | 130 MB/s across the fleet, **5 MB/s per box** at 25 boxes |
| encoding CPU: 2,000 × 250 × 20 Hz = 10 M entity-encodes/s at ~50 ns | **0.5 core across the fleet** |
| region aggregates (world band): 40 regions × ~64 B × 30 Hz, broadcast | 77 kB/s total |

**Conclusion, stated so nobody over-engineers it:** once extraction is
interest-scoped and the encoding is binary, snapshot bandwidth and snapshot CPU
are **not** bottlenecks. Per-tile shared encoding (encode once per spatial tile,
route tiles to clients) is a valid optimization but is **not needed for Stage 1
or 2** — defer it. The thing that *is* a bottleneck is today's O(world) JSON
broadcast, and the fix is small.

### 4.4 Persistence and the op-log

Today `checkpoint()` clones the entire element vector and rewrites one JSON
file every 5 s. At 2M elements × ~120 B of JSON that is **240 MB rewritten
every 5 s = 48 MB/s of sustained disk write** `[D]`, for a world nobody edited.

**Rule: no periodic full-world serialization, ever.**

Design (mostly already specified in `arch_backend §9`, with the scale numbers
attached):

| item | design | numbers `[D]/[E]` |
|---|---|---|
| op-log | append-only per region, `(seq, author, ts, postcard(mutations))`, SQLite WAL, one writer thread per region | 2,000 players × ~1 edit/s = 2,000 ops/s world-wide = **50 ops/s per region** at 40 regions. SQLite WAL sustains 10k+ inserts/s per file |
| log growth | ~60 B/op | 2,000 ops/s × 60 B = 120 kB/s = **10 GB/day world-wide** → compaction is mandatory |
| checkpoints | **per island, not per world**; written on demotion to COLD and on a dirty-island timer | one 20-element island ≈ 600 B postcard |
| compaction | truncate the log before the second-most-recent checkpoint of every island it touches | keeps steady-state on-disk size ≈ world cold size + a few hours of tail |
| continuous state | only ~18% of elements carry state (caps 9.1% + inductors 4.0% + nonlinear ~4.7% `[M: scale-baseline device mix]`) at ~16 B | **5.8 MB for the whole 2M-element world** |
| gameplay state | thermal integrators, i²t budgets, meter totals, contract progress, credits — sparse, only non-zero accumulators | small; already the `arch_backend §9` device-state snapshot |
| damage | `crates/damage` `DamageModel` is already separate from the solver and already serializable | unchanged |

Persistence is bounded by **player edit rate**, not by world size — provided
nothing periodically touches the whole world. Guard that with a test: idle a
2M-element world for 10 minutes with zero ops and assert **zero bytes written**.

### 4.5 Memory per island, and eviction

Computed from the struct layouts in `crates/sim-core/src/engine.rs` `[D]`:

| item | bytes |
|---|---:|
| `ElemState` (v_prev, i_prev, vg1, vg2, region, lastv[6], pin_i[6]) | 136 |
| `[usize; MAX_PINS]` node map | 48 |
| `Option<usize>` branch | 16 |
| `ElementSpec` (id + kind + Vec header) | 72 |
| pins heap (2 × `Point`) | 16 |
| **`CompiledElem`, total** | **~290 B** |
| document copy in `Room::elements` | ~90 B |
| matrix, at n ≈ 0.26 × elements `[M: 5,122 el → n=1,348 over 50 districts]`: n=5 for a 20-element island → `a` + `lu` + x/b | ~500 B per island |

| world form | bytes/element | 2,000,000 elements |
|---|---:|---:|
| compiled + resident (HOT/WARM/FROZEN) | ~380 B | **760 MB** |
| cold (postcard doc ~25 B + sparse state ~3 B) `[E]` | ~30 B | **60 MB** |

**Two conclusions.**

1. **Memory is not the wall.** A 32 GB box can hold 2M compiled elements. Do
   not design around a memory constraint that does not exist; design around the
   1,500-hot-element constraint that does.
2. **But per-island *overhead* is real.** 100,000 `Engine` structs, each with
   ~8 `Vec`s, is ~800,000 allocations and ~100 MB of headers and slack `[E]`.
   Use an **arena / SoA layout per region**: one contiguous element array, one
   contiguous state array, islands as index ranges, matrices from a pooled
   allocator sized by n-class. This also gives the scheduler its contiguous hot
   list (§2.6) for free.

**Eviction (COLD).** Regions with no client within two world-band viewports for
`T_cold` = 5 min serialize their islands to the store and drop the compiled
form. Materialization on arrival costs `[E]` ~1 ms per 20-element island
(postcard decode + island compile), which fits inside the 200-islands-per-tick
promotion budget (§2.3). Eviction is what makes "100k circuits" a storage
statement rather than a RAM statement — 60 MB, not 760 MB.

### 4.6 Late join

Today `hello` ships the whole element list as JSON. At 2M elements that is
**~240 MB** `[D]`. Fatal, and it is the single most visible failure at scale.

**Requirement: the join payload must not depend on world size.**

```
Welcome {
  selfId, members,
  region_directory,          // ~40 rows: cell bounds → host
  region_aggregates,         // world-band render, ~64 B × 40
  initial_tile_set,          // the viewport's islands only, postcard
  sim_time per region, opTail since the client's last acked seq
}
```

Budget: initial payload ≤ **256 kB**, which at ~30 B/element is ~8,000 elements
`[D]` — comfortably a full schematic-band screen. Everything else streams as the
viewport moves, through the same interest pipeline as any other promotion.

**Gate:** time-to-first-frame ≤ **1 s on a 5 Mbit link** for any viewport in a
2,000,000-element world, and the measured payload must be within 10% of the
same measurement in a 50,000-element world.

### 4.7 Indexing and scheduler structures

| structure | scale | cost |
|---|---|---|
| island R-tree (`rstar`) | 100,000 entries | ~10 MB `[E]`; query ~10 µs `[E]` |
| viewport queries | 2,000 clients × 5 Hz = 10,000/s | **0.1 core** `[D]` |
| point → island grid hash | 4M pin entries | 64 MB `[D]`, or less with cell granularity |
| scheduler due-list | 8 buckets (k = 1..128) + one contiguous hot array | O(awake) per substep by construction (§2.6) |
| per-island scheduler record (id, tier, k, due, aabb, owner, cost_ema) | 100,000 × 64 B | 6.4 MB `[D]` |

None of these are problems. They are listed so that nobody builds the
element-level R-tree by default.

### 4.8 Quarantine at scale

Per-island quarantine is already required (`CLAUDE.md`: NR failure ends in
quarantine, never a panic; one bad island must not quarantine the world). Two
additions that only matter at scale:

1. **A quarantined island must not be retried every substep.** Otherwise an
   adversarial player builds 1,000 quarantining islands and each one burns
   `NR_MAX_ITERS` worth of factorizations forever. Rule: a quarantined island
   drops to FROZEN with exponential backoff — retry at 1 s, 2 s, 4 s, … capped
   at 60 s of sim time — and wakes immediately on an edit to it (the player
   fixing their circuit must get instant feedback).
2. **Rescue-ladder work is charged to the owner's budget** (§2.8). The rescue
   ladder can multiply a substep's cost by up to 32 `[M: scale-parallelism
   §1.1, RESCUE_DEPTH=4]`; that cost belongs to whoever built the circuit.

---

## 5. Staged roadmap with gates

Each stage names its circuit-count target, its lever, and the **specific
measurement** that proves it is done. Stage 0 is the work already in flight.

### Stage 0 — islands, quiescence, local dt, rayon *(in flight, not this doc)*

Gates are already written in `scale-parallelism.md` §6. Do not restate them;
this document assumes they pass.

---

### Stage 1 — Interest-scoped extraction and a binary wire protocol

| | |
|---|---|
| **Target** | 1 region, ~5,000 elements (250 circuits), 16 players |
| **Lever** | kill the O(world) `frame()` + JSON broadcast (§4.2, §4.3) |
| **Effort** | `[E]` 5–8 days |

**Gates.**
1. Per-tick display extraction + encode ≤ **2 ms** with 16 interest sets on a
   5,000-element world (today: 8.22 ms for extraction alone at 5,122 `[M]`).
2. Per-client class-2 traffic ≤ **65 kB/s** at steady state and ≤ 200 kB/s on
   a full keyframe.
3. **Zero heap allocation proportional to world size on the per-tick path** —
   assert with an allocation counter in a debug build, not by inspection.
4. Late-join payload ≤ 256 kB and independent of world size (§4.6).

---

### Stage 2 — Activity tiering with event-driven wake

| | |
|---|---|
| **Target** | 50,000 elements (2,500 circuits) stored, ~500 hot, 16–32 players, one box |
| **Lever** | HOT/WARM/FROZEN/SUSPENDED (§2), O(awake) scheduler (§2.6) |
| **Effort** | `[E]` 10–15 days |

**Gates.**
1. **The decisive one.** Hold 500 hot elements fixed and vary the frozen world
   from 5,000 to 500,000 elements. Substep wall time must stay **within 10%**.
   This is the direct measurement that world size has left the substep cost.
2. **Freeze-truth gate (CI).** For every island the freezer freezes, stepping
   it continuously for 10 s of sim time yields a `state_hash()` **bit-identical**
   to the frozen hash. Zero exceptions permitted; a failure means the freeze
   criterion (§2.4) is wrong.
3. **Suspend-truth gate (CI).** Suspend an island for 60 s of region time,
   resume, and compare its trajectory to the same island run continuously from
   the same local time: bit-identical, because it is the same netlist evaluated
   at the same local times.
4. Promotion latency: an island entering a schematic-band interest set is
   publishing at the required tier **within 2 ticks**, p99, at up to 200
   promotions/tick.
5. No demotion thrash: a client panning at 2 screens/s across a district
   causes ≤ 1 tier transition per island per 2 s.

---

### Stage 3 — O(1)-per-edit compile, cold storage, and honest persistence

| | |
|---|---|
| **Target** | 500,000 elements (25,000 circuits) stored on one box, ~1,500 hot |
| **Lever** | hashed junction interning + point→island index (§4.1); COLD eviction (§4.5); per-island checkpoints (§4.4) |
| **Effort** | `[E]` 8–12 days |

**Gates.**
1. Add/remove one element in a 500,000-element world: **p99 ≤ 100 µs**
   (today, extrapolated: ~2 minutes `[D]`).
2. Idle a 500,000-element world for 10 minutes with zero ops: **zero bytes
   written to disk**.
3. Resident memory ≤ **4 GB** with 500k elements stored and ≤ 50k resident-hot.
4. Cold→hot materialization ≤ **1 ms per 20-element island**, p99.
5. Determinism harness green (`tools/determinism.sh`) on the re-baselined
   per-island hashes, with tier transitions replayed from the op-log (§5.4).

---

### Stage 4 — Regions inside one process, intra-host corridors

| | |
|---|---|
| **Target** | 2,000,000 elements (100,000 circuits) stored, ~40 regions, 20–240 players/box per §1.4 |
| **Lever** | region partitioning (§3.1), seam rule (§3.2), corridor-as-device (§3.3), per-sync-group dilation (§3.5) |
| **Effort** | `[E]` 15–25 days |

**Gates.**
1. **Corridor overhead ≤ 10% of the substep budget** at 500 awake corridors —
   this is the item `scale-parallelism.md` Appendix B flags as unmeasured, and
   it is the main risk to this stage. Measure the device formulation *and* the
   naive scheduler-phase formulation, to confirm the 200× gap `[D: §3.3]`.
2. **Dilation isolation:** drive one region to 0.3× with a deliberately
   pathological circuit; every region not in its sync group must stay at
   ≥ 0.99× real time, and the sync-group members must dilate together (not
   independently, which would break the corridor contract).
3. **Islanding fallback:** force a sync group below the 0.25× floor; the tie
   must trip, the healthy region must return to 1.0×, and the event must appear
   as a breaker opening in the client, not as an error.
4. Edit an element in region A while region B is at 0.3×: A's op-apply latency
   is unaffected (p99 ≤ 1 tick).
5. Determinism harness green **per region**.
6. Merge caps enforced: no merged island exceeds n = 40 nonlinear / n = 96
   linear (§3.7); assert in debug builds.

---

### Stage 5 — Multi-host regions and long-haul interties

| | |
|---|---|
| **Target** | unbounded stored world; players-per-box unchanged; fleet scales linearly |
| **Lever** | converter intertie (§3.4 option B), edge router, region park/resume as the migration primitive (§3.8) |
| **Effort** | `[E]` 20–30 days |

**Gates.**
1. **Energy conservation across an intertie:** ∫V·I measured at both ends over
   60 s of sim time agrees within **0.1%**, and agrees with a single-process
   reference run of the same two regions within the same bound.
2. Intertie bandwidth ≤ **32 B per trunk per tick**, measured on the wire.
3. **Seam crossing is invisible:** a scripted bot client pans across a seam at
   2 screens/s; no dropped frame, no gap in its own edit responsiveness, and
   the trunk's rendered power is continuous to within the τ the datasheet
   declares.
4. **Host-failure drill:** kill a region host. The surviving neighbour trips its
   tie within 1 s, keeps real time, and after the dead region resumes from
   checkpoint the reclose restores power with **zero lost ops** (op-log
   comparison before and after).
5. Region split at 50,000 elements completes offline and the resumed pair has
   `state_hash()` equal to the parked whole for every island that did not touch
   the new seam.

---

### 5.4 A determinism hazard that spans every stage

Tier decisions are allowed to depend on player viewports. Viewports are not
deterministic. So a naive implementation makes the simulation depend on who was
looking, and `tools/determinism.sh` plus op-log replay both die.

Two rules fix it:

1. **`k` is chosen only by a state-derived local-error controller** — never by
   wall clock, CPU load, player count or queue depth. (This is already flagged
   in `scale-parallelism.md` §5.3 as the one place a "make it adaptive to load"
   instinct destroys determinism. At MMO scale the temptation is much stronger,
   so it must be a review checklist item.)
2. **Interest-driven tier changes are logged as first-class ops.** A promotion
   is an *input* to the simulation, exactly like a switch toggle:
   `SetTier{island, tier, k, seq}` goes in the op-log. Replay then reproduces
   the tier history exactly, and hash-matching survives. Probe subscriptions
   that pin `k = 1` are logged the same way (they already are ops).

With those two rules, freezing/suspension/multirate all remain deterministic
and replayable, and the existing harness keeps its value.

---

## 6. What is not reachable, and what would have to change

Stated plainly, because the arithmetic says so.

### 6.1 Not reachable: 100,000 circuits simultaneously observed at schematic band

2,000,000 hot elements at 58.6 ns/element/substep and 50,000 substeps/s is
**5,860 core-seconds per simulated second**, i.e. ~1,300 ten-core boxes with the
measured rayon speedup `[D]`. At dt = 10 µs, 2,600 boxes.

What would have to change, and why each is refused:

| change | effect | verdict |
|---|---|---|
| dt from 20 µs to 1 ms | 50× fewer substeps → ~26 boxes | **No.** It deletes audio-band fidelity (speakers, FFT, the 12.5 kHz audio tap), the whole instrumentation pillar above ~500 Hz, and every switching circuit. It would be a different game. `scale-parallelism.md` §6 says the same thing about the GPU revisit condition. |
| beat the 14–68 ns bookkeeping floor | up to ~4× | **Maybe, but small.** The floor is stamping, state clone and `update_guesses` — an SoA rewrite could plausibly get some of it, and it collides with nothing except effort. It does not change the order of magnitude. |
| GPU | — | **No**, twice measured. `scale-parallelism.md` §2: the dispatch-latency floor alone is 1.5× the tick budget, f64 does not exist on Metal, and the addressable component is at most 75% of the cost. |
| render unobserved circuits from cached waveforms | infinite | **Forbidden.** It is the definition of faked electrical behaviour. |
| SIMD inside the factorization | ~2–3× on large dense factors only | **No** — wrong problem size (island n = 3–32) and hostile to the determinism invariant `[scale-parallelism §4.3]`. |

The honest statement to put in front of the owner: **the world can hold
100,000+ circuits; it cannot show them all at once in detail, and no amount of
solver engineering changes that.** What it can do is show each player their
neighbourhood at full fidelity, at 20–240 players per box, and this document is
the machine for making the rest of the world cost nothing.

### 6.2 Reachable, with the levers named

| target | boxes `[D]` | how |
|---|---:|---|
| 100k circuits stored, 40 concurrent builders | **1** | Stages 1–4, V=300, a=0.25, k̄=4 |
| 100k circuits stored, 2,000 concurrent builders | **~25** | + Stage 5 |
| 100k circuits stored, 2,000 builders, dt = 10 µs (plan nominal) | **~50** | same, halved capacity |
| 100k circuits, 10,000 concurrent builders | ~125 | same architecture; nothing new is required, it is linear |

The architecture is linear in concurrent observers and constant in stored
world. That is the property worth defending in every design review.

---

## 7. Open questions and what must be measured

Items this document reasons about but nobody has measured. Each should become a
bench before the stage that depends on it.

1. **Bergeron corridor coupling cost** — flagged unmeasured in
   `scale-parallelism.md` Appendix B, and the main risk to Stage 4. Measure
   both the corridor-as-device and corridor-as-scheduler-phase formulations
   (§3.3 predicts a 200× gap `[D]`).
2. **The real value of `V`** — how many elements a schematic-band viewport
   actually contains under the shipped LOD policy. §1.4 shows it is worth 4–5×
   of server capacity, so it deserves an instrumented playtest, not a guess.
3. **The real value of `a`** — the quiescent fraction of a *real* room. The
   generator's `active_percent` is a modelling knob `[M: scale-baseline is
   explicit about this]`. Instrument the demo room and any playtest.
4. **Freeze-criterion false negatives** — how often does an island that
   *should* be freezable fail the structural half of §2.4 for a silly reason
   (a 0 Hz sine source, an unused rail with `amp = 0`)? Cheap to instrument,
   and each false negative is a permanent hot island.
5. **Cold→hot materialization cost** — estimated at ~1 ms per 20-element
   island; drives the 200-promotions-per-tick admission cap.
6. **`CompiledElem` size** — computed from the struct layout (§4.5), not
   measured. One `size_of` assertion in a test pins it and catches growth.
7. **WASM preview element budget** — §2.10 derives 82 elements native and
   estimates 40–55 in WASM. Measure it; it sets the client LOD policy.
8. **Sparse LU's effect on the merge caps** (§3.7) — the caps are derived from
   the dense kernel. S3 raises them, and by how much decides whether
   district-scale shared grids can be one island.
