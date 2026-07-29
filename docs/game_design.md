# COMMON GROUND — Unified Game Design Document (working title)

## 1. Fantasy & Pillars

You are an electrical engineer-settler on a shared frontier grid. Every light, motor, attack, rescue, message, and payment is a consequence of one authoritative circuit simulation per room.

1. **Everything is electricity.** No fake mechanics: if a number appears on screen, it came out of the MNA solver. Scoring, combat, comms, locks, and damage are emergent properties of the sim.
2. **One simulation, two worlds.** The "real world" view and the schematic are renderings of the same plane and the same netlist.
3. **The scope is the game.** Instrumentation is simultaneously debugger, contract verifier, radar, and lockpick.
4. **Degrade, never destroy.** Attacks cause brownouts, trips, jams, and repair bills — never loss of a player's work or knowledge.
5. **Trivial first circuit, bottomless ceiling.** Battery + switch + lamp in ten seconds; rolling-code encrypted breaker networks and self-tuning converters at the endgame.

## 2. The World and the Zoom Model (decided)

**The world IS the canvas.** One infinite 2D plane per room. There is no separate map: the real-world view is a semantic-zoom re-skin of the same plane, rendered by a single renderer with LOD rules. Diving from the glowing village into the transistor is one uninterrupted wheel-zoom gesture — the signature moment.

Three continuous zoom bands, crossfaded with hysteresis (never modal):

- **World band (far):** components render as physical devices at their placed positions — lamp housings glowing with brightness ∝ actual dissipated power, motors spinning at simulated back-EMF speed, speakers audible via WebAudio from node voltage, displays showing driven segments, smoke on overstressed parts. Schematic wires fade out; only heavy corridor runs remain visible as power lines glowing/pulsing ∝ real power flow. Brownouts visibly dim the district — grid health needs no HUD.
- **Block band (mid):** plots, devices, and subcircuits as blocks/ICs with live aggregate meters (port voltages, power-throughput badges) and animated power-flow arrows on inter-block lines. This is the strategic/warfare readout.
- **Schematic band (near):** full Falstad presentation — voltage-colored wires (brightness ∝ |V|), animated current dots (speed/density ∝ magnitude), junction dots, hover tooltips, in-canvas mini-waveforms beside probed wires. The sim never stops; there is no run button.

**Edit-in-context:** double-click any device or block to enter it in place — breadcrumb trail, surroundings dimmed but still live-simulating, edits propagating to all instances (Figma component semantics, KiCad hierarchy model). Subcircuits are first-class: define once, instance N times, publish as blueprints. Hierarchy is both the abstraction ladder and the progression system — the single most important UX investment.

**Geometry is electrical at world scale only.** Wire runs drawn between plots/devices across the map are stamped as gauge-dependent series resistance per meter, plus lumped series L and shunt C on long runs — long cheap wires genuinely brown out, ring, and low-pass signals; corridor trunks are lumped transmission-line sections (series R-L, shunt C), giving honest droop, reflection, and a natural per-island solver boundary. *Inside* a device or block, schematic layout is lumped and ideal — aesthetics never change electrical behavior at board scale.

**Controls:** wheel zoom-to-cursor, space/middle-drag pan, cmd+0/1/2 fit/100%/selection, grid snap (Shift disables).

**Territory & visibility.** Each player claims a **plot**; only the owner (or permitted allies) may edit inside it. Between plots run **corridors** of neutral ground carrying the shared Grid bus and Party Line trunks; anyone may run wires across neutral ground — and anyone may splice or cut wires that cross it (shorter/cheaper through, safer around: this one rule generates the tap/cut metagame). Visibility is a room-template rule: co-op rooms default to open read-only viewing of all plots; competitive rooms render rival plots in world band and block silhouettes only — you learn about enemies the way a real engineer would, by measuring what reaches the shared lines.

## 3. Shared Infrastructure

- **The Grid:** shared transmission bus (DC at launch, AC era later) with real per-segment resistance and thermal mass — segments heat with I²R, resistance rises with temperature, sustained overload opens them (repairable). Voltage anywhere is whatever the sim says: droop, back-feed, and ripple are real.
- **The Party Line:** shared low-power data trunks — plain wires with real termination impedances. There is no chat-API between machines: telemetry, trade offers, and remote control all require player-built transmitters and receivers. A UART decoder is offered, but players may invent their own encodings — which is also their security.
- **Service entrance:** each plot's sole tie to shared infrastructure contains an inherent, unbypassable, auto-resetting main fuse and an energy meter (a real integrator). Worst case, you island, lose grid income, and auto-reconnect. Local generation and internal circuits can always be made safe.
- **Sources:** each player has a finite prime mover (nonideal source with internal resistance and fuel budget) plus purchasable generation (solar with irradiance parameter, generator blocks). Scarce high-capacity **source taps** (Thevenin wells) are placed on the map to force contention or sharing.

## 4. Core Loop

**Build → Instrument → Debug → Deliver → Defend → Optimize**, on a 20–60 minute session cadence inside a days-to-weeks room arc.

Moment-to-moment: accept a contract → place parts via type-to-search radial picker → drag wires on the always-running sim → click a wire for a voltage probe, click a component for a current clamp → docked scope auto-triggers and auto-scales → see the droop when the load kicks in → knob-drag a capacitor value and watch ripple shrink live → payout ticks up → notice a sag you didn't cause → probe your Grid tie, see current flowing *out* → someone is leeching → open your breaker, or price them a deal over the Party Line. Meanwhile your homemade intrusion alarm (a comparator watching line impedance) trips: someone tapped your north run. Fire a TDR pulse, read the reflection at 210 m, ride out and cut the splice.

Every state change is visible within a frame. Brokenness is diegetic: dots stop, wires gray, breaker glyphs flash, shorts glow then smoke, with plain-language DRC hints ("this wire connects + to − with nothing in between").

## 5. Editing UX

- **Placement:** double-tap/"+" opens type-to-search radial picker with recents ring and curated categories. No menus-first placement.
- **Wiring:** drag on empty grid draws wire; drag on component moves it; auto-route with waypoints and explicit junction dots; live connectivity highlight on hover shows the whole electrical node (kills invisible-disconnection bugs alongside strong snap).
- **Values:** drag vertically on any printed value to sweep it live within a frame (EveryCircuit knob-drag); right-click for unit-aware dialog with expressions.
- **Interaction:** switches, buttons, pots, encoders, keypads clickable/draggable in every zoom band during simulation; finger-drawn arbitrary source for sketched waveforms.
- **Selection/undo:** marquee multi-select; deep undo/redo (natural atop the server op-log); copy/paste as human-readable text blobs that double as the blueprint sharing format (Wokwi pattern).
- **Subcircuits:** select → "Create Block" with auto-detected ports; enterable, instanced, publishable.

## 6. Electrical Grounding

**Palette — never gated, all available from minute one:** wire, R, C, L, pot, switch/button/relay, battery, DC/AC/arbitrary sources, diode/LED/Zener/TVS, BJT/MOSFET, op-amp, comparator, 555, transformer (AC era), fuse/breaker, logic gates (stamped-analog, Falstad-style), flip-flops, and an MCU block (event-driven layer bridged by slew-limited DACs and Schmitt ADCs).

**Device faceplates** (world-band bodies whose appearance is a pure function of simulated state): lamp, RGB LED, DC motor (back-EMF + inertia as electrical companion model; mechanical load expressed purely as its electrical equivalent), speaker, mic/line-in source, seven-seg and dot-matrix displays, servo, heater strip. Stock devices ship with working prefab internals; all internals are enterable and editable.

**Optical/RF links (post-MVP):** LED/laser/antenna emitter drives a controlled source at the receiver scaled by path attenuation and line-of-sight occlusion — a linear coupling coefficient stamped into MNA like a lossy transformer. A floodlight is both illumination and a jammable wideband transmitter.

**Damage = degrade, never destroy.** Every component has ratings (power, voltage, current). A thermal state variable integrates overstress; exceeding budget causes measurable parameter drift, then **trips** the part failed-open (or failed-short for caps/semiconductors — genuinely scarier). Failed parts smoke in world view, gray in schematic, and repair for capped currency after a cooldown. Fuses and breakers are real series elements interrupting on spent i²t budget — defense is buildable. No player can edit or delete another's circuits; worst case is "everything tripped, repair and redesign."

| Mechanic | Simulation basis |
|---|---|
| Brownout scoring | Node voltage vs. contract band, RMS over ticks |
| Line burn | I²R heat → R(T) → fuse companion model opens |
| Jamming | Low-impedance driven source on trunk node |
| Tap detection | Tap input impedance loads line → level drop; TDR reflection locates splice |
| Locks | Comparator/logic nets recognizing handshake waveforms |
| World devices | Lamp P=I²R filament, motor back-EMF, speaker node-voltage audio |
| Islanding | Undervoltage relay opens tie via stamped switch |
| Payment | ∫V·I dt at service-entrance meter |

## 7. Contracts: the Goal Engine

NPC client loads at feeder endpoints post **contracts** — executable specs verified server-side by the same measurement stack players use:

- "Deliver 48 V DC ±5%, ripple <100 mVpp, up to 2 A, for 10 minutes" (Vrms/ripple chips at the client's terminals).
- "Drive this motor into a steady speed band under a stepping load" (back-EMF measured).
- "Provide a 10 kHz carrier, THD <5%" (FFT-verified).
- "Send this byte sequence over the client's UART at 9600 baud" (decoder-verified).
- "Survive: keep the district lit through scheduled line faults" (server injects real shorts/opens).

Payouts scale with measured efficiency (client energy ÷ energy drawn, real integrals). Failed specs show the exact violating waveform. Every completion shows Zachtronics-style **histograms** — part count / energy efficiency / copper / footprint / margin percentiles against room and global populations. Retry forever. Cooperative mega-contracts exceed one player's capacity ("electrify the rail line"; "deliver regulated 48 V across two corridors with <5% droop"), payout split by metered contribution at each tap.

## 8. Multiplayer: Co-op & Conflict

All interaction is electrical; plots are edit-permission boundaries — electrons don't respect them.

**Co-op:** tie-lines between players' grids with real metering ledgers ("A exported 3.2 MJ to B"), payment negotiated over the Party Line or out loud; joint contracts; shared data buses with player-invented protocols (room intercoms, telemetry, market broadcasts); communal grid events — load spikes, line faults, storms degrading solar — demanding collective ride-through, with grid frequency/voltage as the room's shared health bar, computed from the actual bus; blueprint libraries with datasheet cards and reputation.

**Conflict (emergent circuits doing rude things — never "attack buttons"):**
- **Power warfare:** overdraw/brownout the bus; back-feed overvoltage into unprotected front-ends; burn a feeder segment thermally (slow, expensive, visible); leech quietly through the Grid; ripple/noise injection at chosen frequencies (AC era adds resonance attacks on poorly damped LC sections).
- **Signal warfare:** splice-tap a neutral-ground run (high-impedance tap nearly invisible; sloppy tap loads the line detectably) → stackable decoders recover the protocol → function generator replays or forges frames — trip their breakers with their own bus. Jam by brute force (costs real watts forever); impedance-attack edge rates with capacitive loading.
- **Cutting:** cutter device opens a neutral-ground run after a charge-up delay with a real inrush signature as audible/measurable warning; cuts are repairable at modest cost.

**Defense is real engineering:** fuses, breakers, current limiters, crowbars, TVS clamps, LC and notch filters, opto-isolators, isolation transformers, undervoltage-lockout islanding relays, checksums, rolling codes, current-loop signaling, dedicated lines through owned ground, watchdog logic of your own design.

**Detection is instrumentation:** every attack has a physics signature — current is traceable via block-band flow arrows and clamp probes; taps change line impedance (TDR locates the splice to the meter); jammers are fingerprinted by FFT for a notch-filter counter. Teaching players to read signatures is the counterplay.

**Structural anti-grief guarantees:** unbypassable service-entrance main fuse and islanding; damage capped at trip-and-repair with bounded costs; generation on owned plots protected by a lifeline auto-isolating breaker; attacker pays real, continuous energy (fuel = money) while defenses are cheap one-time series parts — asymmetry favors the defender ($1 fuse defeats a $50 surge); no offline automation (sim pauses in empty rooms — Screeps lesson); no attack removes knowledge, blueprints, or territory; room templates gate PvP entirely.

## 9. Instrumentation as Gameplay

- **One-click probing:** click wire = voltage probe, click component = current clamp; pinned color-coded flags match trace colors; hover anything for live values.
- **Never-fight-the-scope:** bottom-docked auto-stacking scopes; software auto-trigger (mid-level edge + hysteresis), autocorrelation period detection setting timebase to 2–4 cycles, envelope-tracked 1-2-5 auto-scale. Pinch/scroll timebase. Drag traces together to overlay, apart to split. Zero knob simulation.
- **Analysis:** math channels (A−B, V×I power), measurement chips (Vpp, Vrms, freq, duty, rise time, THD, phase), FFT (Hann, 4096, dB, Welch averaging), XY/I–V mode with phosphor persistence, logic view with bus-to-hex grouping, sigrok-style stackable protocol decoders (UART/SPI/I²C minimum). Decoders are always available; the gameplay gate is physical — you must build the tap.
- **Shared clock, shared probes:** probes are room-scoped entities on the authoritative tick — overlaying your inverter output against a teammate's bus or a rival's suspicious carrier is trivial and central to co-op debugging and counter-intelligence.
- **In-canvas mini-waveforms** beside any probed wire make the schematic itself the dashboard.
- **Instruments in the fiction:** advanced instruments (TDR, spectrum analyzer, injectors) are devices you build or buy — a TDR is a pulse generator plus your scope; the store version saves you the build.

## 10. Progression & Economy

**Never gate primitives.** Progression is capacity, competence, and library:

- **Currency:** joule-credits from contracts and metered energy exports. Credits buy components, copper (per meter, per gauge), higher power ratings (a 10 A diode costs more than a 1 A one — ratings are real overstress thresholds), fuel, repairs, plot expansion, extra probe channels, and instrument devices. Cost gates quantity and capacity, never access.
- **Contract tiers** (DC → AC/audio → power conversion → data → RF/optical) structure a curriculum without walls; beginners may attempt any tier.
- **Blueprints:** every block is a reusable asset; publishing with a datasheet card builds reputation; reputation unlocks bigger joint contracts. Copy-then-tinker is the sanctioned learning path.
- **Cosmetics only** in meta-progression: faceplate skins, plot decorations, trace colors — zero mechanical advantage.
- Persistent rooms (op-log + checkpoints, drop-in/drop-out, sim paused when empty) carry economy; match modes reset per session; optional seasonal leaderboards.

## 11. Modes & Rooms

Room-code matchmaking; 2–16 players; one authoritative sim per room. Templates set the social contract:
- **Sandbox** (solo/co-op, WASM-local, offline-capable): all components, no economy.
- **Commons Co-op** (2–8, PvE): shared grid vs. rising demand and fault events; shared score; PvP off; open plot visibility.
- **Free Market** (4–16, FFA/diplomacy): contracts + power trading; sabotage legal but self-costing; rival plots world-view only.
- **Blackout** (team attack/defend, post-MVP): attackers spend an energy budget to push defenders' contracted loads out of spec; defenders score uptime; sides swap.

## 12. Onboarding Non-EEs

- **First 60 seconds:** spawn with a kit; drag one wire from battery to lamp *in world view*; it lights; a contract pays. Two-terminal loop, no ground concept required (floating-tolerant solver via gmin; DRC suggests grounds only when needed).
- **Prefabs first:** early contracts completable with black-box modules (regulator brick, motor driver); opening and tuning them is where the profit margin lives — depth is a reward, not a toll. A pure "utility mogul" can prosper on prefabs and tie-lines forever.
- **Contracts are the tutorial:** light a lamp → regulate it → motor at spec → first handshake.
- **Starter blueprints:** annotated, enterable, live-simulating examples; probing them IS the tutorial.
- **Tolerant solver:** gmin, topology pre-checks, NR damping, dt-halving rescue — beginner circuits degrade gracefully; hard singularities render as a named, highlighted problem, never a crash. Adversarially pathological circuits self-throttle via per-island compute budgets that slow sim time, never the UI (Falstad's rule).

## 13. Minimal Viable Slice

**Goal: two friends brown each other out over a real DC grid in the browser — and the world↔schematic zoom feels like one object.**

One room type (Free-Market-lite with PvP toggle, doubling as co-op), 2–4 players, one handcrafted map: 2 source taps, 3 NPC client loads, plots, one neutral-ground corridor carrying one Grid bus (3 thermal segments) and one Party Line trunk, per-player service entrance with main fuse and meter.

- **Sim core (Rust → native + WASM):** MNA, trapezoidal + post-switch BE, NR with limiting, dense LU (faer), per-island partitioning at corridor boundaries, fixed timestep with real-time budget. Components: wire runs (2 gauges, real R/m), R, C, L, pot, switch, relay, fuse, battery/finite generator, DC/AC source, diode, Zener, LED, lamp, motor (+visible fan), speaker, op-amp, NPN.
- **Views:** schematic band (full Falstad feel: dots, voltage colors, knob-drag, tooltips) + world band (lamp/motor/speaker faceplates, glowing wire runs), continuous crossfade zoom; edit-in-context entry into devices; basic subcircuit boxes; blueprint copy/paste as text. Block band deferred.
- **Instrumentation:** click V/I probes (4 channels), docked WebGL scope with auto-trigger/auto-scale, math A−B, Vrms/Vpp/ripple/freq chips, one FFT view, min-max decimated streaming.
- **Multiplayer:** WebSocket three-tier sync (reliable op-log / 20 Hz delta snapshots with viewport interest / lossy probe streams), WASM local-preview prediction with snapshot resync, plot claims/permissions, metered service entrances, trip-and-repair damage.
- **Content & conflict:** 5 DC contracts (lamp brightness → dim it → motor-RPM uptime through a fuse → ripple spec → survive scheduled line fault) with efficiency histograms; component shop with joule-credits; one attack verb (splice-tap + brownout/leech), one detection verb (TDR pulse locate), cutting/repair.

**Slice validation questions:** Does world↔schematic zoom feel like one object? Does a DC contract + scope loop hold a non-EE for 30 minutes? Does one tap/TDR duel generate a story? Ship nothing else until all three are yes.

**Post-MVP order:** FFT everywhere + decoders + logic blocks → block band + full hierarchy/instancing → Commons Co-op events → MCU block + rolling-code play → optical/RF LOS links → AC grid (transformers, resonance, isolation) → Blackout mode → 16-player rooms.

**Key risks:** world-band art cost (launch with few faceplates, procedural styling); griefing tone (telemetry-tuned breaker defaults and energy costs; templates contain blast radius); solver robustness under adversarial circuits (per-island budgets, slow-sim-not-UI).