//! THE STEP SEQUENCER — four CV knobs, four drum toggles, one tempo knob.
//!
//! Every number in this file is a measurement taken against the real engine
//! at `dt = 20 µs`, not an estimate. Nothing here is declared by `main.rs`
//! yet: the world assembler picks the modules it wants and calls
//! [`sequencer`].
//!
//! # How it works
//!
//! There is no digital logic in the engine — no gate, no flip-flop, no
//! counter — so the "which step are we on" decision is made the way analog
//! hardware made it before CMOS: with a ramp and a bank of comparators.
//!
//! 1. **Clock.** A 555 in *sawtooth* astable (charge through `RA`, discharge
//!    straight into DIS through a small `RB`) puts a 3.000 → 6.000 V ramp on
//!    its timing cap, retracing in 4.8 ms. The ramp IS the sequencer's
//!    position; the 555's OUT pin is a free bar marker.
//!
//! 2. **Window decoder.** Step *n* is on while `th[n-1] < ramp < th[n]`. Each
//!    step gets a *pair* of OTAs: "a" with `in+ = ramp, in− = th[n-1]`, "b"
//!    with the inputs swapped against `th[n]`. `tanh(vd / 2VT)` saturates
//!    within ±100 mV, so an OTA *is* a comparator, and the pair's output
//!    currents land on one node: `Iabc·[tanh(a) − tanh(b)]` is `2·Iabc`
//!    inside the window and **exactly zero** outside, because both OTAs hang
//!    off one shared bias node and therefore carry identical `Iabc`. That
//!    exact cancellation is the whole trick — it is what makes the gates
//!    one-hot instead of a thermometer code. Measured off-level: 2.2–8.0 mV
//!    against a 4.995 V on-level.
//!
//!    Only three ladder taps are needed for four windows: the outer window
//!    edges are the ground and rail nodes, which already exist.
//!
//! 3. **Gate clamp.** An OTA output is a current source with *exactly zero*
//!    output conductance, so a zener across each gate turns it into a
//!    regulated 4.995 V logic level that no downstream load can move. This
//!    is what decouples the drum row from the pitch row: with all four
//!    toggles switched from open to closed the CV moves by 0.03 %.
//!
//! 4. **CV row.** A pot sits across each gate (ground → wiper → gate), so its
//!    wiper is `wiper × 5 V` while its step is on and 0 V otherwise. The four
//!    wipers are diode-OR'd onto one bus: the inactive ones sit near 0 V and
//!    their diodes are reverse-biased, so they contribute nothing at all —
//!    **measured knob-to-knob crosstalk is zero to four decimal places.** An
//!    op-amp follower buffers the bus, because a 1 V/oct converter drives its
//!    scale resistor straight off this node and must not see a diode's
//!    dynamic resistance.
//!
//! 5. **Beat row.** A toggle switch and a steering diode take each gate to a
//!    shared drum-gate bus.
//!
//! # THE DRAWING
//!
//! The module is laid out as a schematic, not as a net list with coordinates
//! bolted on. It reads left to right, in five columns, and every one of them
//! is a labelled panel in the finished room:
//!
//! ```text
//!   CLOCK          decode           gates        PITCH          buses
//!   +9V rail | ladder | ramp |  OTA pairs  |  zener/pot/toggle | CV, BEAT
//! ```
//!
//! and top to bottom in three identical STEP LANES, one per step of the bar.
//! The threshold ladder is a single vertical chain whose junctions land at
//! the lane boundaries, because step *n*'s upper window edge *is* step
//! *n+1*'s lower one — so the divider that decodes the bar is drawn as the
//! staircase it actually is, and its taps reach their comparators in two
//! grid units.
//!
//! Everything multi-pin is placed through `sim_core::shape::place`, so every
//! part in the room is a rotation/mirror of its canonical symbol and a
//! player can pick any of them up without the placement gate refusing the
//! move. Everything else is an orthogonal `Wire` run or a local `Ground`
//! symbol, both of which cost the solver nothing: a wire merges its ends in
//! `compile`'s union-find and stamps nothing at all.
//!
//! # Measured behaviour (default config: 46 devices from this module,
//! 48 with the world's rail and ground symbol; 24 nodes + 5 branches = 29
//! unknowns, 31 with all four toggles closed)
//!
//! | quantity | measured |
//! |---|---|
//! | bar period | 1.0557 s = 3.79 steps/s, **0.000 ms jitter** |
//! | step durations | 0.2649 / 0.2642 / 0.2628 / 0.2616 s (spread 1.25 %) |
//! | tempo range (`tempo` 0.01→0.99) | 11.76 → 2.95 steps/s |
//! | ramp | 3.000 → 6.000 V, retrace 4.78 ms (0.45 % of the bar) |
//! | gate | off 2.2–8.0 mV, on 4.9941–4.9972 V (0.06 % spread) |
//! | CV, wiper 0.01 → 0.99 | 0.0195 → 4.7252 V, monotonic, ±2 % of a straight line |
//! | CV plateau ripple | 0.0003 V (0.015 %) |
//! | CV output impedance | 0 Ω (buffered; unchanged under a 22 kΩ load) |
//! | CV glide, step to step | 5.5 ms 10–90 % |
//! | BEAT | on 4.810–4.820 V, off 3.5–8.9 mV |
//! | worst rating load (all toggles closed, all pots 0.98) | 0.240 (the timing cap) |
//! | cost | 5.51–5.75 µs/step over 12 passes, **3.48–3.63× real time** |
//! | 60 s endurance, 3 runs | no quarantine, **zero rescues**, 5.54–5.64 µs/step |
//!
//! With a 6-element OTA triangle VCO on the CV bus the four steps play
//! **126.26 / 370.39 / 625.00 / 892.86 Hz** at wipers 0.25/0.50/0.75/0.99,
//! with **0.00 Hz cycle-to-cycle spread** inside each step. (A second
//! harness reads step 2 as 371.76 Hz: that 0.4 % is the probe VCO's own
//! substep-grid quantization, not sequencer instability — the sequencer's
//! CV plateau is flat to 0.3 mV.)
//!
//! # What a player can do, live
//!
//! | action | client op | musical result |
//! |---|---|---|
//! | drag/edit a CV pot | `InteractOp::SetValue` → `wiper` | that step's pitch, 0.02–4.73 V of CV; other steps do not move |
//! | click a beat toggle | `InteractOp::SetSwitch` | that step's drum on/off |
//! | drag/edit the tempo pot | `InteractOp::SetValue` → `wiper` | 2.95–11.76 steps/s |
//!
//! `Engine::interact` runs `compile()` for all three, which costs nothing
//! here: the circuit is nonlinear, so it refactors on every NR iteration
//! anyway (measured 18 759 factorizations across a 40-update knob drag —
//! exactly the number the same 12 000 substeps do when nobody touches
//! anything). Note that `interact` also clears `quarantined` and re-arms two
//! backward-Euler steps; at human knob rates that is 0.24 % of substeps in
//! first order. The properties-editor path (`DocOp::SetKind` →
//! `set_elements`) also works live — `set_elements` carries `ElemState`
//! across by id, so the ramp, the latch and the CV cap keep their values and
//! the sequence does not restart (verified: ramp voltage identical either
//! side of the edit).
//!
//! # Limitations found in the device models
//!
//! * **No CTRL or RESET pin on `Timer555`**, so the clock cannot be pitch-
//!   modulated or reset externally. Tempo is set by the charge resistor.
//! * **`Ota` has exactly zero output conductance.** An unloaded gate node
//!   would run away to ~1e7 V without quarantining. Every OTA output here
//!   has a zener and a pot on it; do not remove them.
//! * **`Switch` is the latching part; `Button` is momentary** (the client
//!   holds it closed only while the pointer is down). A drum pattern must be
//!   made of `Switch`.
//! * **Adjacent enabled steps tie.** BEAT only dips to 4.276 V between two
//!   consecutive on-steps, so a drum voice hears one long gate rather than
//!   two hits. Non-adjacent steps fall to 0.745 V, and the bar line always
//!   resets to 0.004 V because the ramp retrace sweeps every window off.
//!   This is exactly how a Baby-8 behaves. Giving the windows dead time
//!   would fix it but costs three more ladder resistors and punches a 55 ms
//!   hole in the CV, which is worse.
//! * Windows crossfade over about 17 ms (the OTA's ±100 mV transition width
//!   at the ramp's 2.2–2.9 V/s slew), which is why the CV glides rather than
//!   steps. It is a feature, and it is now the ONLY source of glide: the
//!   100 nF slew cap that used to sit on the CV bus was removed when this
//!   module was assembled into `synth.rs`.
//! * **Wires and ground symbols are not devices.** `Wire` merges its ends in
//!   `compile`'s union-find and stamps nothing; `Ground` pins its point to
//!   node 0 and stamps nothing. This module's routing is therefore free in
//!   the matrix — an earlier comment here claimed two cosmetic corners on the
//!   555's jumper were "not worth their price", which was wrong: measured,
//!   the whole room's routing costs 1.4 µs per substep out of 20.
//! * **A pot's wiper may not sit on one of its own ends.** The tempo knob is
//!   a two-terminal rheostat, which used to be drawn by putting the wiper
//!   pin *on* end b — a shape no canonical potentiometer can have. It is now
//!   drawn the way a rheostat is actually drawn: the wiper strapped back to
//!   the end with a wire. Same node, same netlist, one more (free) wire.

use sim_core::{ElementKind as K, ElementSpec, Point};

use crate::layout::{Sheet, DOWN, E, LEFT, RIGHT, UP, W};

// ------------------------------------------------------------------ values

/// Fixed part of the 555 charge resistor, in series with the tempo pot.
pub const RA_FIXED: f64 = 68e3;
/// Tempo pot, wired as a two-terminal rheostat.
pub const RA_POT: f64 = 220e3;
/// Discharge resistor. Small, so the retrace is 0.45 % of the bar.
pub const RB: f64 = 1e3;
/// Timing cap. `bar ≈ 0.693 · (RA_FIXED + RA_POT·tempo + RB) · CT`.
pub const CT: f64 = 6.8e-6;
/// Window edges as fractions of the rail, per step count. Spaced
/// *geometrically*, not evenly, because the 555's ramp is an exponential:
/// `tap[i] = 1 - (2/3)·2^(-(i+1)/N)` puts the crossings at equal *times*,
/// measured to 1.25 % for N = 4. Written out rather than computed so the
/// netlist carries no float math of its own.
pub const TAPS4: [f64; 3] = [0.4394, 0.5286, 0.6036];
pub const TAPS3: [f64; 2] = [0.4786, 0.5872];
/// Total resistance of the threshold ladder.
pub const R_LADDER: f64 = 100e3;
/// One bias resistor for all eight comparator OTAs: 8 diodes in parallel on
/// one node, ~100 µA each, so each pair delivers ~200 µA into its gate.
pub const R_CBIAS: f64 = 10.5e3;
/// Gate clamp. Sets the gate logic level and therefore the full-scale CV.
pub const VZ_GATE: f64 = 5.1;
/// CV pots. Large enough that the gate's zener keeps plenty of headroom.
pub const POT_CV: f64 = 47e3;
/// CV bus pull-down.
pub const R_CV: f64 = 470e3;
/// Drum-gate bus pull-down.
pub const R_BEAT: f64 = 1e6;
/// Supply this module expects.
pub const SUPPLY_V: f64 = 9.0;

// ----------------------------------------------------------------- the grid
//
// Local coordinates, `origin` at local (0, 0). Column x's first, then the
// step lanes. Every one of these is referenced by name below, so the drawing
// can be re-proportioned without hunting for magic numbers.

/// The +9 V rail bus: down the left margin, then along the bottom.
const X_RAIL: i32 = 0;
/// The 555's left-hand pin column (VCC/TRIG/THR/GND).
const X_CHIP: i32 = 8;
/// The trig-to-thr jumper, and the ramp's riser, down the chip's left side.
const X_JUMP: i32 = 6;
/// The threshold ladder: one vertical chain, taps at the lane boundaries.
const X_LADDER: i32 = 22;
/// The ramp bus, daisy-chained down the comparators' input face.
const X_RAMP: i32 = 23;
/// Comparator OTA anchors — their input face.
const X_OTA: i32 = 24;
/// Comparator length, so out lands on `X_OTA + L_OTA` and the shared bias
/// pin on `X_OTA + L_OTA - 1`.
const L_OTA: i32 = 4;
/// The shared comparator-bias bus.
const X_CBIAS: i32 = 29;
/// The gate rail of a lane: zener down to ground, then out to the knobs.
const X_GATE: i32 = 32;
/// CV pot anchor (its grounded end); the track runs back left to the gate.
const X_POT: i32 = 44;
const L_POT: i32 = 6;
/// The tempo rheostat's track length.
const L_TEMPO: i32 = 6;
/// The beat toggle's own row, under the gate rail.
const X_SW: i32 = 38;
/// The drum-gate bus, and the diode-OR'd pitch-CV bus outside it.
const X_BEATBUS: i32 = 46;
const X_CVBUS: i32 = 48;
/// The CV follower and the two output risers.
const X_BUF: i32 = 52;
const X_CVOUT: i32 = 56;

/// Centre line of the first step lane, and the pitch between lanes.
const Y_LANE0: i32 = 8;
const LANE_PITCH: i32 = 12;
/// The top edge of the block: where the rail arrives and the outputs leave.
const Y_TOP: i32 = -4;
/// The bottom rail run, under the lanes.
const Y_RAILBOT: i32 = 38;
/// The CV buffer's row, at the foot of the CV bus.
const Y_BUF: i32 = 36;

/// Where the sequencer sits, and what its knobs and toggles are set to.
///
/// `origin` is the module's top-left node; the drawing occupies about
/// 60 × 48 grid units from there, from `Y_TOP` above it to the pull-down
/// grounds below. `rail` is where the world's +9 V arrives — it must be
/// `origin + (X_RAIL, Y_TOP)`, and [`Seq::rail_in`] says so.
#[derive(Clone, Copy, Debug)]
pub struct Seq {
    pub id0: u32,
    pub origin: Point,
    /// First id the routing (wires and ground symbols) may use. The devices
    /// keep the ids they have always had, so a re-layout can be diffed
    /// against the netlist part by part.
    pub route_id0: u32,
    /// Tempo knob, 0.01..0.99. 0.01 = 11.76 steps/s, 0.99 = 2.95 steps/s.
    pub tempo: f64,
    /// Pitch knob per step, 0.01..0.99 → 0.02..4.73 V of CV.
    pub wipers: [f64; 4],
    /// Drum pattern: one latching toggle per step.
    pub beats: [bool; 4],
    /// How many steps the bar has. Three or four; only the first `steps`
    /// entries of `wipers` and `beats` are used.
    ///
    /// Each step costs SEVEN devices — two comparator OTAs, a zener, a CV
    /// pot, a steering diode, a toggle and its steering diode — plus a
    /// ladder resistor. In a room that has to hold real time to stay in
    /// tune, that is not a free parameter: see `SCOPE NOTES` in `synth.rs`.
    pub steps: usize,
}

impl Default for Seq {
    fn default() -> Self {
        Seq {
            id0: 400,
            origin: (0, 0),
            route_id0: 500,
            tempo: 0.70,
            wipers: [0.25, 0.5, 0.75, 0.99],
            beats: [true, false, true, false],
            steps: 4,
        }
    }
}

impl Seq {
    fn p(&self, dx: i32, dy: i32) -> Point {
        (self.origin.0 + dx, self.origin.1 + dy)
    }
    /// Centre line of step lane `n`.
    fn lane(&self, n: usize) -> i32 {
        Y_LANE0 + LANE_PITCH * n as i32
    }
    /// Where the world's +9 V bus must arrive.
    pub fn rail_in(&self) -> Point {
        self.p(X_RAIL, Y_TOP)
    }
    /// **Pitch CV out**: 0.02..4.73 V, buffered to a zero-impedance source,
    /// one flat plateau per step. Brought up to the top edge of the block on
    /// its own riser; feed a 1 V/oct converter's scale resistor straight off
    /// this node.
    pub fn cv(&self) -> Point {
        self.p(X_CVOUT, Y_TOP)
    }
    /// **Drum gate out**: 0 V / 4.81 V, high for the whole of an enabled
    /// step, on the riser beside the CV. AC-couple it if the voice wants a
    /// trigger rather than a gate.
    pub fn beat(&self) -> Point {
        self.p(X_BEATBUS, Y_TOP)
    }
    /// The step gates themselves, 0 V / 4.995 V one-hot: the gate rail of
    /// lane `n`, where the zener clamps it. Free to tap for envelopes,
    /// accents or a second drum bus — but only through a diode or a resistor
    /// above ~470 kΩ, or the zener's regulation is all that keeps the CV row
    /// honest.
    pub fn gate(&self, n: usize) -> Point {
        self.p(X_GATE, self.lane(n) + 2)
    }
    /// The 555's OUT pin: a bar marker, high 7.8 V and pulsing low to 0.1 V
    /// for 4.8 ms once per bar. Free — nothing else uses it.
    pub fn bar(&self) -> Point {
        self.p(X_CHIP + 4, 3)
    }
    /// The sawtooth itself, 3.000..6.000 V, on the ramp's run along the top
    /// of the block. A ready-made ramp LFO.
    pub fn ramp(&self) -> Point {
        self.p(X_JUMP, 0)
    }
}

/// The `steps - 1` window edges, as fractions of the rail.
fn taps(steps: usize) -> &'static [f64] {
    match steps {
        3 => &TAPS3,
        _ => &TAPS4,
    }
}

/// Ladder resistor values, bottom-up: the gaps between successive taps
/// (and the two outer gaps to ground and to the rail) across `R_LADDER`.
fn ladder(steps: usize) -> Vec<f64> {
    let t = taps(steps);
    let mut v = Vec::with_capacity(steps);
    let mut last = 0.0;
    for x in t {
        v.push((x - last) * R_LADDER);
        last = *x;
    }
    v.push((1.0 - last) * R_LADDER);
    v
}


fn r(ohms: f64) -> K {
    K::Resistor { ohms }
}

/// The sequencer fragment: 46 devices, keeping the ids they have always had,
/// plus its routing (wires and ground symbols) from `route_id0`. The world's
/// +9 V must arrive at [`Seq::rail_in`]; every ground in the block is a local
/// symbol drawn here.
pub fn sequencer(sq: &Seq) -> Vec<ElementSpec> {
    let mut sh = Sheet::new(sq.route_id0);
    let mut ids = sq.id0;
    // Devices are emitted in the historical order, so their ids — and with
    // them the whole netlist diff — are unchanged by the re-layout.
    macro_rules! id {
        () => {{
            ids += 1;
            ids
        }};
    }
    let n = sq.steps.clamp(3, 4);
    let p = |dx, dy| sq.p(dx, dy);
    let lane = |k: usize| Y_LANE0 + LANE_PITCH * k as i32;
    // A lane's four input rows: the ladder taps sit outside, the ramp inside.
    let y_lo = |k: usize| lane(k) - 3;
    let y_hi = |k: usize| lane(k) + 1;

    // ---- THE +9 V RAIL: down the left margin, then along the bottom under
    // the lanes, where the ladder's top and the comparator bias resistor both
    // reach it in a straight line.
    sh.run(&[
        p(X_RAIL, Y_TOP),
        p(X_RAIL, 6),
        p(X_RAIL, 10),
        p(X_RAIL, Y_RAILBOT),
        p(X_LADDER, Y_RAILBOT),
        p(X_CBIAS, Y_RAILBOT),
    ]);

    // ---- CLOCK: a 555 sawtooth astable on the client's 4x4 DIP footprint.
    //
    // Mirrored, so GND is the top leg and VCC the bottom one: that puts the
    // TRIG/THR pair in the middle of the left face with clear air above them
    // for the ramp to leave by, and the supply leg pointing at the rail.
    // pins [vcc, gnd, trg, thr, out, dis]
    let dip = sh.part(id!(), K::Timer555, p(X_CHIP, 6), E, 4, true);
    let (vcc_pin, gnd_pin, trg, thr, dis) = (dip[0], dip[1], dip[2], dip[3], dip[5]);
    // The documented taps have to BE where the drawing puts them.
    debug_assert_eq!(dip[4], sq.bar(), "the bar marker is the 555's OUT pin");
    // The chip's own ground symbol and the TRIG-to-THR jumper keep the ids
    // they were emitted with before the re-layout, so every element in the
    // room can still be diffed against the old netlist by id.
    sh.ground_as(id!(), gnd_pin, UP);
    let jumper = id!();
    // Charge path: rail -> fixed R -> TEMPO rheostat -> DIS. Emitted in the
    // order the room has always emitted it, because `compile` numbers nodes
    // first-seen and that order is part of the netlist's identity.
    sh.two(id!(), r(RA_FIXED), p(X_RAIL, 10), p(X_JUMP, 10));
    // The tempo pot is a RHEOSTAT: its wiper is strapped to end b. That used
    // to be drawn by stacking the two pins on one point, which is not a shape
    // a potentiometer has; the strap is a wire now, which is how a rheostat is
    // drawn on paper anyway. Same node, same netlist, one free wire.
    let tp = sh.part(
        id!(),
        K::Potentiometer { ohms: RA_POT, wiper: sq.tempo },
        p(X_JUMP, 10),
        E,
        L_TEMPO,
        true,
    );
    sh.two(id!(), r(RB), p(X_CHIP + 6, 5), p(X_CHIP + 6, 0));
    sh.two(id!(), K::Capacitor { farads: CT }, p(2, 0), p(2, 4));

    // ...and now the wiring of all that.
    sh.wire(vcc_pin, p(X_RAIL, 6));
    // TRIG tied to THR: a jumper down the chip's left side. The ramp leaves
    // by the same corner, up and over the top of the block, so that it comes
    // down on the comparators from ABOVE and never has to cross the ladder.
    sh.wire_as(jumper, trg, p(X_JUMP, 5));
    sh.run(&[p(X_JUMP, 5), p(X_JUMP, 3), thr]);
    sh.run(&[p(X_JUMP, 3), p(X_JUMP, 0), p(X_CHIP + 6, 0), p(X_RAMP, 0)]);
    sh.wire(p(X_JUMP, 0), p(2, 0));
    sh.ground(p(2, 4), DOWN);
    sh.run(&[tp[1], p(X_CHIP + 4, 12), tp[2]]);
    sh.run(&[dis, p(X_CHIP + 4, 10)]);
    sh.wire(dis, p(X_CHIP + 6, 5));

    // ---- THRESHOLD LADDER. One vertical chain, with its junctions at the
    // lane boundaries: the upper edge of step k's window IS the lower edge of
    // step k+1's, so each tap reaches both comparators that want it in two
    // grid units. Only `n - 1` taps exist — the outer edges are ground and
    // the rail, which the room already has.
    let lad = ladder(n);
    sh.ground(p(X_LADDER, y_lo(0)), LEFT);
    let mut lo_y = y_lo(0);
    for (i, ohms) in lad.iter().enumerate() {
        let last = i + 1 == lad.len();
        let hi_y = if last { y_hi(n - 1) } else { y_hi(i) };
        sh.two(id!(), r(*ohms), p(X_LADDER, lo_y), p(X_LADDER, hi_y));
        if last {
            // The top of the ladder IS the rail: carry on down to the bottom
            // rail run rather than reaching across the block for it.
            sh.run(&[p(X_LADDER, hi_y), p(X_LADDER, Y_RAILBOT)]);
            sh.wire(p(X_LADDER, hi_y), p(X_OTA, hi_y));
        } else {
            sh.run(&[p(X_LADDER, hi_y), p(X_LADDER, y_lo(i + 1))]);
            sh.wire(p(X_LADDER, hi_y), p(X_OTA, hi_y));
            sh.wire(p(X_LADDER, y_lo(i + 1)), p(X_OTA, y_lo(i + 1)));
            lo_y = y_lo(i + 1);
        }
    }
    // Step 0's lower window edge is ground: a local symbol on the pin.
    sh.ground(p(X_OTA, y_lo(0)), LEFT);

    // ---- one bias resistor for every comparator, straight up off the bottom
    // rail run into the bias bus.
    sh.two(
        id!(),
        r(R_CBIAS),
        p(X_CBIAS, Y_RAILBOT),
        p(X_CBIAS, lane(n - 1)),
    );

    // ---- WINDOW DECODER, one lane per step. The two comparators of a lane
    // are mirror images of each other stacked about the lane's centre line.
    // That is what makes the lane readable: their shared bias pins land on
    // the SAME point (one stub to the bias bus per lane, not two), the ramp
    // lands on the two INNER input pins and the ladder taps on the two OUTER
    // ones, so the ramp bus and the ladder never reach past one another.
    // pins [in+, in-, out, bias]
    let mut ramp_pins: Vec<Point> = Vec::new();
    let mut gate: Vec<Point> = Vec::new();
    for k in 0..n {
        let y = lane(k);
        let a = sh.part(id!(), K::Ota, p(X_OTA, y - 2), E, L_OTA, true);
        let b = sh.part(id!(), K::Ota, p(X_OTA, y + 2), E, L_OTA, false);
        debug_assert_eq!(a[3], b[3], "the comparator pair shares one bias pin");
        ramp_pins.push(a[0]);
        ramp_pins.push(b[1]);
        // Both outputs onto the lane's gate rail.
        sh.run(&[a[2], b[2]]);
        sh.wire(b[2], p(X_GATE, y + 2));
        // One bias stub per lane, and the bus that chains the lanes together.
        sh.wire(a[3], p(X_CBIAS, y));
        if k > 0 {
            sh.wire(p(X_CBIAS, lane(k - 1)), p(X_CBIAS, y));
        }
        debug_assert_eq!(p(X_GATE, y + 2), sq.gate(k), "the gate tap is the gate rail");
        gate.push(sq.gate(k));
    }
    // The ramp bus itself: it comes down the input face from ABOVE (which is
    // why the clock sends it over the top of the block) and is daisy-chained
    // from one comparator input to the next, so the only thing it ever has to
    // cross is a ladder tap on its way past a lane boundary.
    let mut at = p(X_RAMP, 0);
    for q in &ramp_pins {
        sh.run(&[at, (at.0, q.1), *q]);
        at = (at.0, q.1);
    }

    // ---- GATE CLAMP. Mandatory: an OTA output has zero output conductance,
    // and this is also what stops the drum row detuning the pitch row. The
    // zener hangs straight down off the gate rail onto its own ground symbol.
    // Zener anode = pin 0, so ground goes first.
    for k in 0..n {
        let y = lane(k) + 2;
        sh.two(id!(), K::Zener { vz: VZ_GATE }, p(X_GATE, y + 4), p(X_GATE, y));
    }
    // The zeners' shared ground used to be one symbol at one point; it is a
    // symbol per lane now, and the first of them keeps the old id.
    let mut zgnd = id!();
    for k in 0..n {
        sh.ground_as(zgnd, p(X_GATE, lane(k) + 6), DOWN);
        zgnd = sh.next_route_id();
    }

    // ---- CV ROW: a pot across each gate, wipers diode-OR'd onto one bus,
    // then buffered. Pot pins [end a, wiper, end b]; end a on ground makes
    // the wiper voltage rise with the knob, so the pot is drawn facing back
    // along the gate rail with its grounded end outboard and its wiper
    // standing up into the diode's row.
    for k in 0..n {
        let y = lane(k) + 2;
        sh.wire(gate[k], p(X_POT - L_POT, y));
        let pot = sh.part(
            id!(),
            K::Potentiometer { ohms: POT_CV, wiper: sq.wipers[k] },
            p(X_POT, y),
            W,
            L_POT,
            true,
        );
        sh.ground(pot[0], RIGHT);
        sh.two(id!(), K::Diode, pot[1], p(X_CVBUS, lane(k)));
        if k > 0 {
            sh.wire(p(X_CVBUS, lane(k - 1)), p(X_CVBUS, lane(k)));
        }
    }
    // Bus pull-down and the unity-gain buffer. The follower's inverting input
    // and its output are one node; that is a strap, and a strap is a wire —
    // an op-amp whose in- pin sat ON its out pin was not an op-amp shape.
    sh.run(&[
        p(X_CVBUS, lane(n - 1)),
        p(X_CVBUS, Y_BUF),
        p(X_CVBUS, Y_RAILBOT + 2),
    ]);
    sh.two(
        id!(),
        r(R_CV),
        p(X_CVBUS, Y_RAILBOT + 2),
        p(X_CVBUS, Y_RAILBOT + 6),
    );
    sh.ground_as(id!(), p(X_CVBUS, Y_RAILBOT + 6), DOWN);
    sh.wire(p(X_CVBUS, Y_BUF), p(X_BUF, Y_BUF));
    // pins [in+, in-, out]
    let buf = sh.part(
        id!(),
        K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC },
        p(X_BUF, Y_BUF + 1),
        E,
        4,
        false,
    );
    sh.run(&[
        buf[2],
        p(X_CVOUT, Y_BUF + 4),
        p(X_BUF, Y_BUF + 4),
        buf[1],
    ]);
    // CV out, up its own riser on the outside of everything.
    sh.run(&[buf[2], p(X_CVOUT, Y_TOP)]);

    // ---- BEAT ROW: a latching toggle and a steering diode per step.
    // Diodes, not resistors: they keep the bus amplitude independent of how
    // many steps are switched on, and they keep the gate nodes isolated from
    // each other when several toggles are closed at once.
    for k in 0..n {
        let y = lane(k) + 2;
        sh.wire(p(X_POT - L_POT, y), p(X_POT - L_POT, y + 4));
        sh.two(
            id!(),
            K::Switch { closed: sq.beats[k] },
            p(X_POT - L_POT, y + 4),
            p(X_SW + 4, y + 4),
        );
        sh.two(id!(), K::Diode, p(X_SW + 4, y + 4), p(X_BEATBUS, y + 4));
        if k > 0 {
            sh.wire(p(X_BEATBUS, lane(k - 1) + 6), p(X_BEATBUS, lane(k) + 6));
        }
    }
    sh.two(
        id!(),
        r(R_BEAT),
        p(X_BEATBUS, lane(n - 1) + 6),
        p(X_BEATBUS, lane(n - 1) + 10),
    );
    sh.ground(p(X_BEATBUS, lane(n - 1) + 10), DOWN);
    // BEAT out, up the riser beside the CV's.
    sh.run(&[p(X_BEATBUS, lane(0) + 6), p(X_BEATBUS, Y_TOP)]);

    sh.finish()
}

/// Element ids a player interacts with, given the same `Seq` that built the
/// fragment. Ids are assigned in emission order from `id0`, so these are
/// stable as long as `sequencer` is not reordered.
#[allow(dead_code)]
pub struct SeqIds {
    pub tempo: u32,
    pub pots: [u32; 4],
    pub switches: [u32; 4],
    /// How many entries of `pots` / `switches` are real.
    pub steps: usize,
}

#[allow(dead_code)]
pub fn seq_ids(sq: &Seq) -> SeqIds {
    let els = sequencer(sq);
    let mut out = SeqIds {
        tempo: 0,
        pots: [0; 4],
        switches: [0; 4],
        steps: sq.steps.clamp(3, 4),
    };
    let (mut pi, mut si) = (0, 0);
    for e in &els {
        match e.kind {
            K::Potentiometer { .. } => {
                if out.tempo == 0 {
                    out.tempo = e.id;
                } else {
                    out.pots[pi] = e.id;
                    pi += 1;
                }
            }
            K::Switch { .. } => {
                out.switches[si] = e.id;
                si += 1;
            }
            _ => {}
        }
    }
    out
}
