//! THE SYNTHESIZER ROOM — the second selectable sample world.
//!
//! Boot a fresh room with `EE_WORLD=synth` (see `world()` in `main.rs`) and
//! it is already playing: a four-step analog sequencer clocked by a 555
//! drives a 1 V/octave VCO through a voltage-controlled filter that a
//! bar-synced LFO sweeps open and shut; a noise source snares on whichever
//! steps the player has toggled on; and both land in one 8 Ω speaker.
//!
//! Every number in it is solved. There is no oscillator table, no sample
//! playback and no software envelope generator: the pitch is a capacitor
//! being charged by an OTA's bias current, the hiss is `ElementKind::Noise`,
//! and the beat is a 555 charging 6.8 µF through a knob.
//!
//! ## Signal flow
//!
//! ```text
//!            ┌─ 4 CV pots ─diode-OR─> CV ─> VCO ─> VCF ────────┐
//!   555 ─ramp┤                                ^                 │  virtual
//!   TEMPO    │        the same ramp ──> LFO sweep               ├─ ground
//!            │                                                  │  mixer
//!            └─ 4 BEAT toggles ──> gate ──> SNARE <── Noise ────┘     │
//!                                          (MOSFET VCA)               v
//!                                                              8 Ω speaker
//! ```
//!
//! ## Why it is shaped like this
//!
//! The binding constraint is the real-time budget, not the schematic. At
//! `dt = 20 µs` the solver has 20 µs of wall clock per substep, and when it
//! misses, sim time dilates — which for an instrument means it plays FLAT
//! and wobbles. Holding real time is a functional requirement here, so every
//! element had to earn its place; the measurements and the deliberate cuts
//! are in `SCOPE NOTES` at the bottom of this file.
//!
//! Four device-model facts shaped every value:
//!
//! 1. **An `Ota` output has exactly zero output conductance.** An unloaded
//!    OTA output floats on GMIN and runs to ~1e7 V *without* quarantining.
//!    Every OTA here terminates in a capacitor inside a closed loop, a
//!    zener, or the mixer's virtual ground.
//! 2. **The `Ota` input is linear only to about ±20 mV** (`tanh(vd/2·VT)`),
//!    so audio is attenuated ~1/500 on the way in and the gain is taken back
//!    at the mixer.
//! 3. **A comparator only flips on a substep boundary**, so any relaxation
//!    oscillator sits on a discrete pitch grid `50000/k` Hz wide — 30 cents
//!    at A440. A `Noise` dither under the hysteresis divider would randomize
//!    which substep the crossing lands on and restore a continuous average
//!    frequency; it is deliberately NOT fitted here, because the grid is only
//!    3–11 cents wide over this bass line's 220–330 Hz range, and the dither
//!    measured 2 µs per substep (14 % of the room) plus 1 % of period
//!    jitter. Below ~350 Hz the quantization is inaudible and free; a VCO
//!    meant to play up at A440 and above would want the dither back
//!    (`synth_vco.rs` has it, measured).
//! 4. **An OTA fed white noise costs the whole room a Newton iteration.**
//!    Newton's convergence test on an OTA is `1 µV + 0.1 %`; a noise source
//!    redraws every substep, so an OTA that can see it never converges on
//!    the first pass and the ENTIRE matrix is factored twice per substep.
//!    Measured: the same room with an OTA VCA on the snare runs at 26.1
//!    µs/substep, and with a MOSFET gate instead at 17.1. The snare's VCA is
//!    therefore an `Nmos` used as a voltage-controlled resistor — whose
//!    convergence test is a much looser 10 mV — which is also exactly how
//!    cheap noise gates were really built.

use sim_core::{ElementKind as K, ElementSpec, Point};
use sim_golden::{dc, gnd, r, spec, spec3};

use crate::sequencer::{self, Seq};

// ---------------------------------------------------------------- geometry
//
// The room is laid out so the part a player wants first is the part the
// camera opens on. The client's home view frames x/y in -10..60, and the
// SEQUENCER — every pitch knob, every beat toggle, the tempo knob — fills
// the bottom two thirds of exactly that box. The voice runs right-to-left
// along the top (the sequencer's CV comes out of its top right corner, so
// the signal path starts there), and the two drum voices sit under it.

/// Where the sequencer's top-left corner sits.
const SEQ_ORIGIN: Point = (-10, 12);
/// First id the sequencer owns; it uses `SEQ_ID0 + 1 ..= SEQ_ID0 + 46`.
const SEQ_ID0: u32 = 400;

/// The single 9 V rail node, on the left margin between the two halves.
const V9: Point = (-8, 16);
/// Ground symbols. All three are node 0 — several symbols is how a
/// schematic avoids one star of twenty-line-long wires, and `Ground` costs
/// no branch unknown.
const G: Point = (-8, 20); // sequencer side
const G2: Point = (20, 0); // voice side
const G3: Point = (20, 8); // drum side

// -- mixer / output --------------------------------------------------------
const SUM: Point = (24, -8); // virtual-ground current summing bus
const OUT: Point = (20, -8); // op-amp output / speaker terminal

// -- VCO -------------------------------------------------------------------
const SQ: Point = (44, -8); // comparator output: the square, ±5 V
const TRI: Point = (48, -8); // the integrator: a 0.52 Vpp triangle
const HYS: Point = (44, -4); // Schmitt divider tap
const VBIAS: Point = (52, -4); // the exponential converter's output

// -- filter ----------------------------------------------------------------
const FA: Point = (36, -8); // attenuated input, ~10 mV
const FY: Point = (32, -8); // filter output
const FB: Point = (32, -4); // cutoff bias
const FCV: Point = (36, -4); // cutoff control voltage

// -- LFO (a buffered tap off the sequencer's own sawtooth) ------------------
const LC: Point = (12, -8); // after the LFO's coupling cap

// -- snare -----------------------------------------------------------------
const NOUT: Point = (40, 4); // noise source output
const SIN: Point = (48, 4); // MOSFET gate's signal side
const SENV: Point = (48, 8); // envelope == MOSFET gate
const STRIG: Point = (52, 8); // differentiated beat bus

// ------------------------------------------------------------------ values
//
// The VCO's exponential converter, from `synth_vco.rs`, measured: the pitch
// law is `f = 55 · 2^CV` Hz and it holds to about 2 cents from 55 Hz to
// 620 Hz. `R_SCALE` is 0.45 % below the textbook value, which trims out the
// bias-diode's own IR drop.
const R_SCALE: f64 = 8_329.8;
const R_OFF: f64 = 3_360.4;
const R_GND: f64 = 160.03;
const R_HYST_TOP: f64 = 950_000.0;
const R_HYST_BOT: f64 = 50_000.0;
const C_VCO: f64 = 1e-9;
/// Supply. 9 V keeps every capacitor at a third of its 25 V rating.
const SUPPLY_V: f64 = 9.0;
/// The snare gate's transconductance coefficient. `Rds ~= 1/(k*(Vgs - Vt))`,
/// so this is the snare's level: trimmed by measurement against the bass.
const NMOS_K: f64 = 5e-5;
/// CUTOFF knob to filter bias current: `Iabc = (V_cv - 0.45 V) / R`, and
/// `fc = Iabc / (2 * 2*VT * pi * C)`. Sized so the knob's whole travel lands
/// on the bass's own harmonics rather than far above them.
const R_CUT_SCALE: f64 = 1e6;
/// LFO depth into the same bias node, in the same units: the ramp's 3 Vpp
/// divided by this is the peak-to-peak swing of the filter's bias current.
const R_LFO_DEPTH: f64 = 680e3;

/// Ids of the parts a player is meant to touch, named because the tests and
/// the labels both refer to them.
pub const ID_SPEAKER: u32 = 1;
pub const ID_CUTOFF: u32 = 40;
pub const ID_SNARE_TONE: u32 = 73;
pub const ID_NOISE: u32 = 70;

/// The sequencer's knobs and toggles, resolved against `SEQ_ID0`.
#[allow(dead_code)]
pub fn seq_ids() -> sequencer::SeqIds {
    sequencer::seq_ids(&seq_config())
}

/// How many steps the bar has. THREE, not four, and the reason is the
/// real-time budget: a step costs seven elements, and at four steps the live
/// server measured 0.86x real time — a synthesizer running 0.86x plays two
/// and a half semitones FLAT, which is not a performance problem, it is a
/// broken instrument. See `SCOPE NOTES`.
pub const SEQ_STEPS: usize = 3;

/// Pitch knob positions. `synth_room_plays_a_tune` asserts what these
/// actually sound like, in Hz, so they cannot silently drift out of tune.
pub const SEQ_WIPERS: [f64; 4] = [0.438, 0.491, 0.560, 0.493];

/// The shipped pattern. Adjacent enabled steps TIE — the beat bus only dips
/// to 4.3 V between them, so a differentiator hears one long gate instead of
/// two hits — but the bar retrace always resets it to 4 mV, so steps 3 and 1
/// across the bar line are two separate hits. Hence 1 and 3, not 1 and 2.
pub const SEQ_BEATS: [bool; 4] = [true, false, true, false];

pub fn seq_config() -> Seq {
    Seq {
        id0: SEQ_ID0,
        origin: SEQ_ORIGIN,
        rail: V9,
        gnd_pt: G,
        // ~3.9 steps/s: slow enough that every step is a distinct note,
        // fast enough to be a groove.
        tempo: 0.34,
        wipers: SEQ_WIPERS,
        beats: SEQ_BEATS,
        steps: SEQ_STEPS,
    }
}

fn cap(farads: f64) -> K {
    K::Capacitor { farads }
}
fn pot(ohms: f64, wiper: f64) -> K {
    K::Potentiometer { ohms, wiper }
}
fn ota(id: u32, inp: Point, inm: Point, out: Point, bias: Point) -> ElementSpec {
    ElementSpec {
        id,
        kind: K::Ota,
        pins: vec![inp, inm, out, bias],
        ..Default::default()
    }
}

/// The synthesizer room.
pub fn synth_room_circuit() -> Vec<ElementSpec> {
    let sq = seq_config();
    let mut e: Vec<ElementSpec> = Vec::with_capacity(100);

    // ------------------------------------------------------- supply / mixer
    e.push(spec(2, dc(SUPPLY_V), V9, G));
    e.push(gnd(3, G));
    e.push(gnd(4, G2));
    e.push(gnd(5, G3));

    // ONE op-amp is the whole output stage. Its inverting input is a
    // virtual-ground current bus: every voice dumps current into it and they
    // sum with zero crosstalk, at whatever impedance suits each one, and the
    // op-amp's branch is the only thing in this engine that can actually
    // drive an 8 Ω coil (an OTA into 8 Ω delivers gm·8 ≈ 0.01).
    //
    //   v(OUT) = −I_sum · Rf
    //
    // The speaker gets id 1: the server streams the four lowest-id Speakers,
    // so the instrument's output can never be crowded out by something a
    // player drops next to it.
    e.push(ElementSpec {
        id: 10,
        kind: K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC },
        pins: vec![G2, SUM, OUT],
        ..Default::default()
    });
    e.push(spec(11, r(6.8e6), OUT, SUM)); // Rf: transimpedance
    e.push(spec(ID_SPEAKER, K::Speaker { ohms: 8.0 }, OUT, G2));

    // --------------------------------------------------------------- VCO
    // An OTA constant-current integrator inside an op-amp Schmitt loop:
    // `f = Iabc/(4·Vth·C)`, and `Iabc = OTA_IS·(exp(v_bias/VT) − 1)` turns
    // the three-resistor divider below into a true exponential 1 V/octave
    // converter — an octave is 17.9 mV at the bias pin.
    //
    // The triangle on the cap is the clean output, but the SQUARE is what
    // feeds the filter: the triangle node is a bare integrator and any
    // resistive load on it changes the pitch (the filter's 470 kΩ input
    // would draw current comparable to Iabc itself). A filtered square is
    // also the richer sound.
    e.push(ota(20, SQ, G2, TRI, VBIAS));
    e.push(spec(21, cap(C_VCO), TRI, G2));
    e.push(spec3(22, K::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, HYS, TRI, SQ));
    e.push(spec(23, r(R_HYST_TOP), SQ, HYS));
    e.push(spec(24, r(R_HYST_BOT), HYS, G2));
    // The exponential converter's Thevenin resistance at the bias pin is
    // ~150 Ω, which is what lets the sequencer's BUFFERED CV set pitch to a
    // couple of cents: driving R_SCALE from a bare pot wiper was measured at
    // 844 cents of error.
    e.push(spec(26, r(R_SCALE), sq.cv(), VBIAS));
    e.push(spec(27, r(R_OFF), V9, VBIAS));
    e.push(spec(28, r(R_GND), VBIAS, G2));

    // -------------------------------------------------------------- VCF
    // A one-pole gm-C low-pass: the OTA drives its own inverting input
    // through the capacitor, so `f0 = gm/(2πC)` and the corner follows the
    // bias current — a real voltage-controlled filter, 6 dB/octave.
    //
    // 470 k : 1 k divides the 10 Vpp square to about ±10 mV, inside the
    // OTA's linear window. Everything inside the filter runs at millivolts
    // and the gain comes back at the mixer.
    e.push(spec(30, r(470_000.0), SQ, FA));
    e.push(spec(31, r(1_000.0), FA, G2));
    e.push(ota(32, FA, FY, FY, FB));
    e.push(spec(33, cap(22e-9), FY, G2));
    e.push(spec(37, r(R_CUT_SCALE), FCV, FB));
    // CUTOFF — the headline knob.
    e.push(spec3(ID_CUTOFF, pot(10_000.0, 0.40), G2, FCV, V9));
    // Out to the mixer, and deliberately a plain resistor rather than an OTA
    // VCA. Two reasons, both measured. One: an 8 Ω speaker passes its 0.5 W
    // rating at 2 V rms and the op-amp's rail is 9 V, so a level control that
    // could be wound up would be a way to burn the speaker by turning
    // something clockwise — a demo is not a trap, and the damage test winds
    // every pot to 0.98. With the level fixed, nothing a player can touch
    // drives the output past ~1.3 V peak. Two: an OTA tracking an audio
    // signal costs the WHOLE room a third of a Newton iteration per substep
    // (3 µs), because Newton is global. The filter's output impedance is
    // 1/gm, so 220 kΩ is a light load when the filter is open and
    // deliberately fades the voice as the cutoff knob closes it.
    e.push(spec(41, r(220_000.0), FY, SUM));

    // ---------------------------------------------------------------- LFO
    // The sequencer's 555 already makes a 3-6 V sawtooth once per bar, so
    // the filter sweep is two elements: one coupling cap and one injection
    // resistor onto the filter's bias node. It is LOCKED to the bar, which a
    // free-running LFO could never be, and it costs neither a second 555
    // (6 elements) nor a buffer op-amp (1 element and a branch unknown).
    //
    // The honest cost of dropping the buffer: `R_LFO_DEPTH` hangs on the
    // 555's own timing capacitor, so it steals about a tenth of the charging
    // current and the tempo knob's calibration shifts with it. That is
    // measured and accounted for in the shipped `tempo` value, not ignored.
    e.push(spec(51, cap(1e-6), sq.ramp(), LC));
    e.push(spec(52, r(R_LFO_DEPTH), LC, FB));

    // -------------------------------------------------------- noise + SNARE
    e.push(spec(
        ID_NOISE,
        K::Noise {
            volts: 1.0,
            ohms: 1000.0,
            seed: 0x00D1_5EA5,
        },
        NOUT,
        G3,
    ));
    // An anti-alias pole made out of the noise source's OWN 1 kΩ output
    // resistance: fc = 4.8 kHz, one element. The raw source is flat to
    // 25 kHz and the audio tap decimates to 12.5 kHz, so without this most
    // of the snare's power folds back down and it stops sounding like a
    // snare at all.
    e.push(spec(71, cap(33e-9), NOUT, G3));
    e.push(spec(74, r(470_000.0), NOUT, SIN));
    // SNARE TONE: the shunt leg of the input divider. Turning it down
    // attenuates and brightens together.
    e.push(spec3(ID_SNARE_TONE, pot(10_000.0, 0.47), SIN, G3, G3));
    // THE VCA. A MOSFET in its ohmic region is a voltage-controlled
    // resistor between the noise and the mixer's virtual ground, so the
    // gain is `Rf/Rds` and the envelope on the gate opens and closes it:
    // 100 MΩ of leak when shut, about 80 kΩ wide open, ~60 dB of range.
    // See fact 4 in the module docs for why this is not an OTA.
    e.push(ElementSpec {
        id: 76,
        kind: K::Nmos { vt: 1.0, k: NMOS_K },
        pins: vec![SENV, SIN, SUM],
        ..Default::default()
    });
    e.push(spec(77, K::Diode, STRIG, SENV));
    e.push(spec(78, cap(15e-9), SENV, G3));
    e.push(spec(79, r(3.3e6), SENV, G3)); // decay, tau = 50 ms

    // ------------------------------------------------------- trigger glue
    // The snare fires on whichever steps the player has toggled on.
    //
    // The trigger is differentiated, because a sequencer gate is high for
    // a whole step (260 ms) and an envelope cap fed from a level would just
    // sit there charged. The RC also keeps the envelope caps off an ideal
    // source: charging a cap inside one substep makes the trapezoidal
    // integrator ring (13.8 V was measured on a cap fed from a 7.8 V pulse).
    e.push(spec(85, cap(47e-9), sq.beat(), STRIG));
    e.push(spec(86, r(470_000.0), STRIG, G3));

    // ---------------------------------------------------------- SEQUENCER
    e.extend(sequencer::sequencer(&sq));

    e
}

/// A labelled region of the schematic. The client turns each one into a
/// mission-control window, and it is the only way to put words in the world:
/// `ElementSpec` has no name field.
pub struct PanelDef {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub name: &'static str,
}

/// The room's labels. Without them a player sees a hundred anonymous glyphs;
/// with them they see the VCO, the filter, the snare and a row of numbered
/// steps.
pub fn synth_panels() -> Vec<PanelDef> {
    let (ox, oy) = (SEQ_ORIGIN.0 as f64, SEQ_ORIGIN.1 as f64);
    let mut v = vec![
        PanelDef {
            x0: 41.0,
            y0: -10.0,
            x1: 55.0,
            y1: 2.0,
            name: "VCO  1V/OCT",
        },
        PanelDef {
            x0: 29.5,
            y0: -10.0,
            x1: 39.0,
            y1: -2.0,
            name: "FILTER  CUTOFF",
        },
        PanelDef {
            x0: 17.0,
            y0: -10.0,
            x1: 25.0,
            y1: -2.0,
            name: "MIXER + SPEAKER",
        },
        PanelDef {
            x0: 6.0,
            y0: -10.0,
            x1: 14.0,
            y1: -6.0,
            name: "LFO  BAR SWEEP",
        },
        PanelDef {
            x0: 38.5,
            y0: 2.0,
            x1: 54.0,
            y1: 10.0,
            name: "SNARE  (TONE)",
        },
        PanelDef {
            x0: ox + 4.0,
            y0: oy + 3.0,
            x1: ox + 17.0,
            y1: oy + 15.0,
            name: "CLOCK  TEMPO",
        },
        PanelDef {
            x0: ox + 19.0,
            y0: oy + 19.0,
            x1: ox + 36.0,
            y1: oy + 45.0,
            name: "STEP DECODER",
        },
    ];
    let pitch = [
        "STEP 1 PITCH",
        "STEP 2 PITCH",
        "STEP 3 PITCH",
        "STEP 4 PITCH",
    ];
    let beat = ["BEAT 1", "BEAT 2", "BEAT 3", "BEAT 4"];
    for n in 0..SEQ_STEPS {
        let y = oy + 10.0 * n as f64;
        v.push(PanelDef {
            x0: ox + 42.0,
            y0: y + 3.0,
            x1: ox + 47.0,
            y1: y + 9.0,
            name: pitch[n],
        });
        v.push(PanelDef {
            x0: ox + 37.0,
            y0: y + 7.0,
            x1: ox + 41.5,
            y1: y + 12.0,
            name: beat[n],
        });
    }
    v
}

// ------------------------------------------------------------ SCOPE NOTES
//
// All numbers measured on an Apple M4 (release, pinned cargo 1.95.0), with
// several other agents building in parallel — load average 7.5, which is why
// every figure below is quoted with its spread and against a control.
//
//   SHIPPED: 71 elements (75 in the live room, with the hoist fixture).
//   Solver 13.60 / 13.64 / 13.66 / 13.71 / 13.73 / 13.73 µs per substep over
//   two runs of three passes = 1.46–1.47x the 20 µs substep budget.
//   LIVE SERVER, over a websocket, reporting its own `rt`, two runs:
//   median 0.993 and 0.994, min 0.971, max 1.013.
//
//   THE CONTROL, interleaved with those runs on the same machine: the
//   SHIPPED showcase room (147 elements) reports rt median 0.985 and 0.987,
//   min 0.940, and its solver costs 10.54–10.66 µs per substep. So this room
//   reports a HIGHER realtime ratio than the room the game already ships,
//   and a steadier one. The ~1 % both give up is the server's per-tick
//   overhead (frame(), damage, the JSON broadcast, the machine
//   co-simulation — about 11 ms here, 16 ms for the showcase) on a machine
//   at load average 6–8, not the circuit.
//
// Cost in this engine is `newton_iterations × elements^1.64`, and BOTH
// factors had to be bought down. The ladder that got here, each rung
// measured on the room as it stood:
//
//   106 el  2-pole OTA filter, OTA snare VCA, gm-C kick, 4 steps  35.5 µs  0.56x  NR 2.52
//   102 el  1-pole filter,     OTA snare VCA, gm-C kick, 4 steps  26.1 µs  0.76x  NR 2.03
//    97 el  1-pole filter,     MOSFET snare,  gm-C kick, 4 steps  20.6 µs  0.97x  NR 1.76
//    84 el  no kick,                                     4 steps  19.0 µs  1.05x  NR 2.02
//    79 el  + unbuffered LFO, fewer snare caps,          4 steps  16.2 µs  1.24x  NR 2.00
//    71 el  THREE steps                                           13.7 µs  1.46x  NR 2.03
//
// The 79-element four-step version was built, tuned and playing in tune —
// and the LIVE server reported rt 0.86 for it. A synthesizer at 0.86x plays
// two and a half semitones flat, so it was cut. That is what the last row
// cost and what it bought.
//
// What was cut, and why:
//
//   * THE FOURTH STEP (7 elements). Two comparator OTAs, a zener, a CV pot,
//     a steering diode, a toggle and its diode, plus a ladder resistor. This
//     is the cut that took the room from 0.86x to 1.0x on the live server.
//     Three steps against a two-hit drum pattern is a real groove — a
//     Baby-8 run at an odd step count is a sound, not a compromise — but a
//     bar of four is what everyone expects, and this is not it.
//   * THE KICK (12 elements). `drums.rs` has a measured gm-C two-integrator
//     resonator that sweeps 117 Hz → 49 Hz over 200 ms: a real kick with a
//     continuous pitch envelope, no substep quantization, and a TUNE knob
//     that walks it from a ping to a boom. It cost 12 elements with its
//     trigger, which at the time was the difference between 0.97x and 1.23x.
//     The bass line's own low notes carry the bottom instead.
//   * THE TWO-POLE RESONANT FILTER (4 elements, and 0.5 Newton iterations).
//     `crates/sim-golden/tests/synth_filter_vca.rs` has a measured 12 dB/oct
//     state-variable OTA-C filter whose resonance knob peaks at +12 dB.
//     Three OTAs tracking an audio signal through a 1 µV convergence test
//     cost 2.5 Newton iterations per substep against the one-pole's 2.0 — a
//     25 % tax on the WHOLE room, sequencer included, because Newton is
//     global. The room ships a real voltage-controlled 6 dB/octave low-pass
//     with a live CUTOFF knob and no resonance.
//   * THE OTA VCA on the bass (2 elements) — see the note at the resistor
//     that replaced it.
//   * THE HI-HAT. The audio tap runs at 12.5 kHz, so Nyquist is 6.25 kHz and
//     a real hi-hat's 8–12 kHz cannot be transmitted at all. Band-limited to
//     fit, it measured a 3.1 kHz centroid — close enough to the snare that
//     the two stopped being distinguishable. A limit of the transport, not
//     of the budget.
//   * A SECOND PITCHED VOICE, and the modular rack the request imagined.
//   * THE DEDICATED LFO 555 (6 elements → 2) and then its buffer op-amp.
//     The sequencer's own clock already makes a sawtooth, so the filter
//     sweep is a cap and a resistor, locked to the bar.
//   * THE VCO'S OWN PITCH POT (3 elements) and its NOISE DITHER (1). Pitch
//     comes from the sequencer, which is the point of the room; the dither
//     is unnecessary below ~350 Hz (see fact 3 above).
//
// The one change that would give all of it back is per-island
// factorization: this room is one connected circuit of ~45 unknowns and the
// LU is dense. Element ordering was measured and does not help — partial
// pivoting destroys the block structure — and neither does giving each
// module its own rail. That work is in flight elsewhere and this room
// deliberately does not depend on it. With it, the four-step version with
// the kick and the resonant filter is roughly a 106-element room, which is
// exactly what was built and measured on the way here.
