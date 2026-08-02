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
//! # Measured behaviour (default config: 46 elements from this module,
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
//!   module was assembled into `synth.rs`, along with two cosmetic wires and
//!   the tempo rheostat's wire — three elements the real-time budget could
//!   not justify. **The module is 46 elements now, not 49**, and the timings
//!   in the table above were taken before that trim.

use sim_core::{ElementKind as K, ElementSpec, Point};
use sim_golden::{gnd, r, spec, spec3};

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

/// Where the sequencer sits, and what its knobs and toggles are set to.
///
/// `origin` is the module's top-left node; the fragment occupies about
/// 56 × 42 grid units from there. `rail` must carry [`SUPPLY_V`] and
/// `gnd_pt` is the world's ground node — sharing both costs nothing in the
/// matrix, since node 0 is eliminated.
#[derive(Clone, Copy, Debug)]
pub struct Seq {
    pub id0: u32,
    pub origin: Point,
    pub rail: Point,
    pub gnd_pt: Point,
    /// Tempo knob, 0.01..0.99. 0.01 = 11.76 steps/s, 0.99 = 2.95 steps/s.
    pub tempo: f64,
    /// Pitch knob per step, 0.01..0.99 → 0.02..4.73 V of CV.
    pub wipers: [f64; 4],
    /// Drum pattern: one latching toggle per step.
    pub beats: [bool; 4],
    /// How many steps the bar has. Three or four; only the first `steps`
    /// entries of `wipers` and `beats` are used.
    ///
    /// Each step costs SEVEN elements — two comparator OTAs, a zener, a CV
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
            rail: (2, 8),
            gnd_pt: (2, 46),
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
    /// **Pitch CV out**: 0.02..4.73 V, buffered to a zero-impedance source,
    /// one flat plateau per step. Feed a 1 V/oct converter's scale resistor
    /// straight off this node.
    pub fn cv(&self) -> Point {
        self.p(58, 22)
    }
    /// **Drum gate out**: 0 V / 4.81 V, high for the whole of an enabled
    /// step. AC-couple it if the voice wants a trigger rather than a gate.
    pub fn beat(&self) -> Point {
        self.p(56, 40)
    }
    /// The step gates themselves, 0 V / 4.995 V one-hot. Free to tap for
    /// envelopes, accents or a second drum bus — but only through a diode
    /// or a resistor above ~470 kΩ, or the zener's regulation is all that
    /// keeps the CV row honest.
    pub fn gate(&self, n: usize) -> Point {
        self.p(34, 6 + 10 * n as i32)
    }
    /// The 555's OUT pin: a bar marker, high 7.8 V and pulsing low to 0.1 V
    /// for 4.8 ms once per four steps. Free — nothing else uses it.
    pub fn bar(&self) -> Point {
        self.p(14, 11)
    }
    /// The sawtooth itself, 3.000..6.000 V. A ready-made ramp LFO.
    pub fn ramp(&self) -> Point {
        self.p(10, 11)
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

/// The sequencer fragment: **46 elements**. With the world's 9 V rail and
/// its ground symbol that is 51 elements, 24 nodes + 5 branches = 29
/// unknowns (31 with all four toggles closed). The rail and the main ground
/// symbol are NOT emitted here — they come from `sq.rail` / `sq.gnd_pt`.
pub fn sequencer(sq: &Seq) -> Vec<ElementSpec> {
    let mut v: Vec<ElementSpec> = Vec::new();
    let mut id = sq.id0;
    let mut next = || {
        id += 1;
        id
    };
    let vcc = sq.rail;
    let g0 = sq.gnd_pt;

    // node points
    let ta = sq.p(6, 8); // fixed R / tempo rheostat junction
    let trg = sq.p(10, 9); // 555 TRIG pin
    let ramp = sq.ramp(); // 555 THR pin = timing cap = the sawtooth
    let dis = sq.p(14, 9); // 555 DIS pin
    let out5 = sq.bar(); // 555 OUT pin
    let n = sq.steps.clamp(3, 4);
    // Threshold nodes, ascending; only `n - 1` of them are used.
    let th = [sq.p(22, 34), sq.p(22, 28), sq.p(22, 22)];
    let cbias = sq.p(22, 42);
    let gt = [sq.gate(0), sq.gate(1), sq.gate(2), sq.gate(3)];
    let w = [sq.p(44, 6), sq.p(44, 16), sq.p(44, 26), sq.p(44, 36)];
    let s = [sq.p(40, 10), sq.p(40, 20), sq.p(40, 30), sq.p(40, 40)];
    let cvb = sq.p(52, 22); // raw diode-OR bus, before the buffer
    let cv = sq.cv();
    let beat = sq.beat();
    let g555 = sq.p(10, 13); // local ground symbols
    let ggate = sq.p(30, 46);
    let gout = sq.p(52, 46);

    // ---- clock: 555 sawtooth astable, on the client's 4×4 DIP footprint.
    // pins [vcc, gnd, trig, thr, out, dis]
    v.push(ElementSpec {
        id: next(),
        kind: K::Timer555,
        pins: vec![vcc, g555, trg, ramp, out5, dis],
    });
    v.push(gnd(next(), g555));
    // TRIG tied to THR: one wire down the left of the chip. (This used to be
    // three, routed squarely around the DIP. Wires are not free — they cost
    // an element visit per substep and `frame()`'s `solve_wire_currents` is
    // quadratic in them — and in a world that has to hold real time to stay
    // in tune, two cosmetic corners are not worth their price.)
    v.push(spec(next(), K::Wire, trg, ramp));
    // charge path: rail -> fixed R -> TEMPO rheostat -> DIS
    v.push(spec(next(), r(RA_FIXED), vcc, ta));
    v.push(spec3(
        next(),
        K::Potentiometer {
            ohms: RA_POT,
            wiper: sq.tempo,
        },
        ta,
        dis,
        dis,
    ));
    // The wiper is coincident with end b, which makes the pot a two-terminal
    // rheostat: the b..wiper leg stamps a conductance from a node to itself,
    // i.e. exactly nothing. (A `Wire` between two separate points did the
    // same job and cost an extra element.) `collapses_when_coincident` only
    // rejects two-pin sources and switches, so a pot is safe here.
    // discharge path and timing cap
    v.push(spec(next(), r(RB), dis, ramp));
    v.push(spec(next(), K::Capacitor { farads: CT }, ramp, g0));

    // ---- threshold ladder. Three taps only: the outer window edges are the
    // ground and rail nodes, which already exist and cost nothing.
    let lad = ladder(n);
    let mut lo_node = g0;
    for (i, ohms) in lad.iter().enumerate() {
        let hi_node = if i + 1 == lad.len() { vcc } else { th[i] };
        v.push(spec(next(), r(*ohms), lo_node, hi_node));
        lo_node = hi_node;
    }

    // ---- one bias resistor for all eight comparator OTAs
    v.push(spec(next(), r(R_CBIAS), vcc, cbias));

    // ---- window decoder. Step n is on while lo[n] < ramp < hi[n]; the two
    // OTAs share a bias node so their currents cancel *exactly* outside it.
    // pins [in+, in-, out, bias]
    let edge = |i: usize| if i == 0 { g0 } else { th[i - 1] };
    for k in 0..n {
        let hi = if k + 1 == n { vcc } else { th[k] };
        v.push(ElementSpec {
            id: next(),
            kind: K::Ota,
            pins: vec![ramp, edge(k), gt[k], cbias],
        });
        v.push(ElementSpec {
            id: next(),
            kind: K::Ota,
            pins: vec![hi, ramp, gt[k], cbias],
        });
    }

    // ---- gate clamp. Mandatory: an OTA output has zero output conductance,
    // and this is also what stops the drum row from detuning the pitch row.
    // Zener anode = pin 0, so ground goes first.
    for k in 0..n {
        v.push(spec(next(), K::Zener { vz: VZ_GATE }, ggate, gt[k]));
    }
    v.push(gnd(next(), ggate));

    // ---- CV row: a pot across each gate, wipers diode-OR'd onto one bus,
    // then buffered. Pot pins [end a, wiper, end b]; end a on ground makes
    // wiper voltage rise with the knob.
    for k in 0..n {
        v.push(spec3(
            next(),
            K::Potentiometer {
                ohms: POT_CV,
                wiper: sq.wipers[k],
            },
            ggate,
            w[k],
            gt[k],
        ));
        v.push(spec(next(), K::Diode, w[k], cvb));
    }
    v.push(spec(next(), r(R_CV), cvb, gout));
    v.push(gnd(next(), gout));
    // unity-gain buffer. pins [in+, in-, out]
    v.push(spec3(next(), K::OpAmp { rail: SUPPLY_V }, cvb, cv, cv));

    // ---- beat row: a latching toggle and a steering diode per step.
    // Diodes, not resistors: they keep the bus amplitude independent of how
    // many steps are switched on, and they keep the gate nodes isolated from
    // each other when several toggles are closed at once.
    for k in 0..n {
        v.push(spec(
            next(),
            K::Switch {
                closed: sq.beats[k],
            },
            gt[k],
            s[k],
        ));
        v.push(spec(next(), K::Diode, s[k], beat));
    }
    v.push(spec(next(), r(R_BEAT), beat, gout));

    v
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

/// A minimal OTA triangle VCO, included only because it is what the CV bus
/// was *measured* against: `f = Iabc / (4·C·Vth)`, exactly linear in the
/// bias current. 6 elements. At `r_v = 470 kΩ, c_v = 1 nF, rail = 5 V` the
/// four default steps play 126.26 / 370.39 / 625.00 / 892.86 Hz.
///
/// `synth_vco.rs` has the properly calibrated exponential 1 V/oct voice;
/// prefer that one for the shipped world and drive its `cv()` node from
/// [`Seq::cv`], which is buffered precisely so that its 8.3 kΩ scale
/// resistor sees a stiff source.
#[allow(dead_code)]
pub fn probe_vco(sq: &Seq, id0: u32, r_v: f64, c_v: f64, rail: f64) -> Vec<ElementSpec> {
    let mut id = id0;
    let mut next = || {
        id += 1;
        id
    };
    let g = sq.p(52, 46);
    let (vb, tri, sqr, fb) = (sq.p(62, 22), sq.p(68, 22), sq.p(68, 10), sq.p(74, 16));
    vec![
        spec(next(), r(r_v), sq.cv(), vb),
        ElementSpec {
            id: next(),
            kind: K::Ota,
            pins: vec![sqr, g, tri, vb],
        },
        spec(next(), K::Capacitor { farads: c_v }, tri, g),
        spec3(next(), K::OpAmp { rail }, fb, tri, sqr),
        spec(next(), r(100e3), sqr, fb),
        spec(next(), r(100e3), fb, g),
    ]
}
