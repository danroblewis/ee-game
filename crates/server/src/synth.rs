//! THE SYNTHESIZER ROOM — the second selectable sample world.
//!
//! Boot a fresh room with `EE_WORLD=synth` (see `world()` in `main.rs`) and
//! it is already playing: a FOUR-step analog sequencer clocked by a 555
//! drives a 1 V/octave VCO through a voltage-controlled filter that a
//! bar-synced LFO sweeps open and shut, then through a VCA that an
//! attack-decay envelope generator accents on the beat; a noise source snares
//! through a second VCA and a second envelope on whichever steps the player
//! has toggled on; and both land in one 8 Ω speaker.
//!
//! Every number in it is solved. There is no oscillator table, no sample
//! playback and no software envelope generator: the pitch is a capacitor
//! being charged by an OTA's bias current, the hiss is `ElementKind::Noise`,
//! the beat is a 555 charging 6.8 µF through a knob, and the two envelopes
//! are two capacitors charging through a diode and discharging through a
//! resistor. **Neither the VCA nor the envelope generator is a device model.**
//! There is no `ElementKind::Vca` and no `ElementKind::Envelope`, and adding
//! one would be a mistake — see `THE VCAs` below.
//!
//! ## Signal flow
//!
//! ```text
//!            ┌─ 4 CV pots ─diode-OR─> CV ─> VCO ─> VCF ─> VCA ──┐
//!   555 ─ramp┤                                ^          ^       │  virtual
//!   TEMPO    │        the same ramp ──> LFO sweep     AD EG 2    ├─ ground
//!            │                                           ^       │  mixer
//!            └─ 4 BEAT toggles ──> BEAT ──d/dt──> trigger┤       │     │
//!                                                        v       │     │
//!                             Noise ──> SNARE VCA <── AD EG 1 ───┘     v
//!                                                                8 Ω speaker
//! ```
//!
//! ## THE VCAs, and why neither is a new `ElementKind`
//!
//! Both VCAs are one `Nmos` each, run in the ohmic region as a
//! voltage-controlled resistor: `Rds ≈ 1/(k(Vgs − Vt))`, so the envelope on
//! the gate is the gain. Both envelope generators are a `Diode` into a
//! `Capacitor` with a DECAY knob across it (a `Potentiometer` strapped as a
//! rheostat) — attack is the cap charging through the trigger's source
//! impedance, decay is `R·C` and a player can turn it, and there is no
//! sustain stage because an AD generator does not have one.
//!
//! That is a deliberate decision against adding device models, and the reason
//! is arithmetic, not taste. A real VCA's output is *gain × signal* — a
//! product of two unknowns, which is a SMOOTH nonlinearity. A faithful `Vca`
//! kind would land in `needs_newton` alongside `Ota`, and fact 4 below has
//! already measured what that costs this room: 26.1 µs/substep against 17.1.
//! It would be slower than the transistor it replaced. The only cheaper model
//! samples its control voltage outside the solve and holds it constant
//! through Newton — and a gain that does not come out of the solver is the
//! one thing this game does not ship.
//!
//! The rest of the bill for a new kind is real too: a stamp, an arm in every
//! exhaustive `match` in `netlist.rs`/`engine.rs`, a range check in
//! `validate.rs` (nothing reaches the sim without one), a rigid `Shape` and
//! its eleven mentions in `shape.rs`, a damage rating, a footprint, a golden
//! circuit with a closed form, a client symbol and a catalogue entry. Against
//! that: one transistor, which is also exactly what an MS-20-era VCA and
//! every cheap noise gate really were, and which a player can take apart.
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
use sim_golden::{dc, r};

use crate::layout::{Sheet, DOWN, E, LEFT, N, RIGHT, S, UP, W};
use crate::sequencer::{self, Seq};

// ---------------------------------------------------------------- geometry
//
// THE DRAWING. The room is one continuous signal path drawn as a
// counter-clockwise loop, so a player can follow it with a finger:
//
//   the SEQUENCER fills the bottom two thirds, reading left to right —
//   clock, threshold ladder, one lane per step, pitch knobs, beat toggles —
//   and puts its CV and BEAT buses out on risers up its right-hand edge.
//   The VOICE then reads RIGHT TO LEFT along the top: VCO, filter, mixer,
//   speaker. The SNARE runs in its own band underneath the voice, fed from
//   the BEAT riser on the right and dumping its current into the mixer's
//   summing node on the left. The +9 V rail is a single line down the left
//   margin and along the top; every ground is a local symbol on the pin
//   that needs it.
//
// Every multi-pin part is placed through `sim_core::shape::place`, so it is
// a rotation or mirror of its canonical symbol and nothing in the room is
// skewed. Everything else is an orthogonal `Wire` run: a wire merges its
// ends in `compile`'s union-find and stamps nothing, so routing is free in
// the matrix and costs only an element visit.

/// Where the sequencer's rail-in corner sits.
const SEQ_ORIGIN: Point = (-10, 14);
/// First id the sequencer owns for its devices; routing comes from
/// [`SEQ_ROUTE_ID0`].
const SEQ_ID0: u32 = 400;
/// Routing ids. The voice draws from [`VOICE_ROUTE_ID0`] and the sequencer
/// from [`SEQ_ROUTE_ID0`]; devices keep the ids they have always had, which
/// is what lets the netlist be diffed part by part across a re-layout.
const VOICE_ROUTE_ID0: u32 = 100;
const SEQ_ROUTE_ID0: u32 = 500;

// -- the supply ------------------------------------------------------------
/// The rail line: down the left margin from the sequencer, then along the
/// top of the voice.
const RAIL_Y: i32 = -10;
const RAIL_X: i32 = -10;

// -- mixer / output --------------------------------------------------------
const SUM: Point = (20, -5); // virtual-ground current summing bus
const OUT: Point = (14, -4); // op-amp output / speaker terminal

// -- VCO -------------------------------------------------------------------
const SQ: Point = (44, -8); // comparator output: the square, +-5 V

// -- envelopes -------------------------------------------------------------
/// The bass envelope's storage node — the top of its bus, and the gate rail
/// that runs from there down onto the bass VCA.
const BASS_ENV: Point = (30, 2);
/// The snare envelope's storage node, which is the snare VCA's own gate pin.
const SNARE_ENV: Point = (30, 7);

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
/// Both VCAs' transconductance coefficient. `Rds ~= 1/(k*(Vgs - Vt))`, so on
/// the snare this is its level (trimmed by measurement against the bass) and
/// on the bass it is how little the VCA costs when it is open: at the
/// measured 3.0 V envelope peak `Rds` is 8 kOhm against the 220 kOhm that
/// feeds it, which is 0.3 dB.
const NMOS_K: f64 = 5e-5;
/// The trigger differentiator's cap. Sized for the CHARGE two envelope
/// storage caps need, not for a time constant: `C_TRIG * dV` is what the
/// pulse can deliver, and 47 nF (enough when only the snare hung on it) left
/// both envelopes at 1.4 V, barely over the MOSFETs' 1.0 V threshold.
const C_TRIG: f64 = 100e-9;
/// DECAY, one knob per envelope generator, wired as a rheostat across the
/// storage cap: `tau = ohms*wiper*C` into 15 nF. The shipped positions are a
/// 50 ms snare (a hit) and a 150 ms bass (a plucked note), and the whole
/// travel is about 1 ms to 100 / 300 ms. This is what makes each of them an
/// envelope GENERATOR rather than a fixed RC. Attack stays fixed: these two
/// cost nothing in DEVICES (each replaced a resistor) but +0.40 µs for their
/// two wiper nodes, and an attack knob would be two more devices on top. The
/// margin is 1.11x, not 1.5x.
const POT_SNARE_DECAY: f64 = 6.8e6;
const W_SNARE_DECAY: f64 = 0.485; // 3.3 MOhm, tau = 50 ms
const POT_BASS_DECAY: f64 = 20e6;
const W_BASS_DECAY: f64 = 0.50; // 10 MOhm, tau = 150 ms
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
/// The two envelope generators' decay knobs — 79 keeps the id the snare's
/// fixed decay resistor had, because it is the same part in the same place
/// doing the same job with a shaft on it.
pub const ID_SNARE_DECAY: u32 = 79;
pub const ID_BASS_DECAY: u32 = 89;

/// The sequencer's knobs and toggles, resolved against `SEQ_ID0`.
#[allow(dead_code)]
pub fn seq_ids() -> sequencer::SeqIds {
    sequencer::seq_ids(&seq_config())
}

/// The VCO's square-wave node: what a test listens to when it wants the
/// oscillator itself rather than the speaker.
#[allow(dead_code)]
pub fn vco_square() -> Point {
    SQ
}

/// The mixer's output — the node the speaker hangs on.
#[allow(dead_code)]
pub fn out_node() -> Point {
    OUT
}

/// The bass AD envelope's storage node, which is also the bass VCA's gate.
#[allow(dead_code)]
pub fn bass_env() -> Point {
    BASS_ENV
}

/// The snare AD envelope's storage node, which is also the snare VCA's gate.
#[allow(dead_code)]
pub fn snare_env() -> Point {
    SNARE_ENV
}

/// How many steps the bar has. FOUR — a bar of four is what everyone expects
/// and this room did not have it, because a step costs eight devices and a
/// four-step room once measured 0.86x on the live server, which is a
/// synthesizer two and a half semitones FLAT.
///
/// It measures 0.999 live now, and NOT because the solver got faster. The
/// 0.86 was measured on a room that still carried the gm-C kick and the
/// two-pole OTA filter (see the ladder in `SCOPE NOTES`), and it was never
/// re-taken against the room as it actually ships. Re-measured, on this
/// machine, the fourth step costs **+3.20 µs/substep — 1.385x → 1.134x**,
/// and the live server does not move off rt 0.999.
///
/// It also DID NOT WORK. `sequencer.rs` accepted `steps: 4` and nothing ever
/// called it: its bottom rail row was a constant that assumed three lanes, so
/// a fourth lane put the CV bus pull-down's ground symbol straight onto the
/// CV bus and every step played 0 V. That is fixed (`Seq::railbot`), and the
/// four-step path is now the one the room ships, so it is tested.
///
/// The step count is still not a free parameter: 1.134x is the margin, and
/// `the_synth_room_fits_the_realtime_budget` is where the next person meets
/// it.
pub const SEQ_STEPS: usize = 4;

/// Pitch knob positions. `synth_room_plays_a_tune` asserts what these
/// actually sound like, in Hz, so they cannot silently drift out of tune.
///
/// One note per step: A3 220, C4 261.6, E4 329.6, D4 293.7 — A minor up and
/// back down onto the fourth. Trimmed BY MEASUREMENT, because the CV row is
/// linear in the wiper only to about 2 %; these land at 220.2 / 263.1 /
/// 329.0 / 294.1, all inside 10 cents.
///
/// The fourth knob was 0.493 when the fourth step did not exist, a hair off
/// the second's 0.491: the entry was in the array but had never been given a
/// pitch of its own.
///
/// It is a D and not the G an A-minor-seventh would want, and that is fact 3
/// in the module docs biting. The VCO's comparator can only flip on a substep
/// boundary, so a period is an EVEN number of substeps and the pitch grid is
/// `50000/2m` Hz — 27 cents wide at 392 Hz. Its nearest line there is 6 cents
/// under G4 with the next 21 cents over, and the CV's own bar-to-bar wander
/// is enough to pick either: swept, that knob measured 384.6 / 390.6 / 396.8
/// from one bar to the next. At 294 Hz the grid line is 2.7 cents off D4 and
/// its neighbours are 18 and 23 cents away, so it lands on the same note
/// every bar — measured 294.09–294.12 over ten bars.
pub const SEQ_WIPERS: [f64; 4] = [0.4418, 0.4943, 0.5632, 0.5283];

/// The shipped pattern. Adjacent enabled steps TIE — the beat bus only dips
/// to 4.3 V between them, so a differentiator hears one long gate instead of
/// two hits — but the bar retrace always resets it to 4 mV, so steps 3 and 1
/// across the bar line are two separate hits. Hence 1 and 3, not 1 and 2.
///
/// With two AD envelopes hanging off that bus this row is now the NOTE-ON
/// row as well as the drum row: it accents the bass and fires the snare. The
/// bass still sounds on every step (see the bypass resistor at the VCA), so
/// all four pitch knobs are audible whatever the pattern is.
pub const SEQ_BEATS: [bool; 4] = [true, false, true, false];

pub fn seq_config() -> Seq {
    Seq {
        id0: SEQ_ID0,
        origin: SEQ_ORIGIN,
        route_id0: SEQ_ROUTE_ID0,
        // ~3.9 steps/s: slow enough that every step is a distinct note,
        // fast enough to be a groove.
        tempo: 0.50,
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

/// The synthesizer room.
pub fn synth_room_circuit() -> Vec<ElementSpec> {
    let sq = seq_config();
    let mut sh = Sheet::new(VOICE_ROUTE_ID0);

    // ------------------------------------------------------- supply
    // One rail line: up the left margin out of the sequencer, then straight
    // across the top of the voice. Every module drops a resistor onto it
    // rather than reaching for a star point.
    let rail_in = sq.rail_in();
    sh.run(&[
        rail_in,
        (RAIL_X, RAIL_Y),
        (-8, RAIL_Y),
        (32, RAIL_Y),
        (58, RAIL_Y),
    ]);
    sh.two(2, dc(SUPPLY_V), (-8, -8), (-8, -4));
    sh.wire((-8, RAIL_Y), (-8, -8));
    sh.ground((-8, -4), DOWN);

    // ------------------------------------------------------- mixer + speaker
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
    // pins [in+, in-, out]
    let mix = sh.part(
        10,
        K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC },
        (20, -4),
        W,
        6,
        false,
    );
    debug_assert_eq!(mix[1], SUM);
    debug_assert_eq!(mix[2], OUT);
    sh.ground(mix[0], DOWN);
    // Rf, over the top of the amplifier: the transimpedance that turns the
    // summing current into volts.
    sh.run(&[OUT, (14, -1)]);
    sh.two(11, r(6.8e6), (14, -1), (21, -1));
    sh.run(&[(21, -1), (21, -5), SUM]);
    sh.run(&[OUT, (12, -4)]);
    sh.two(ID_SPEAKER, K::Speaker { ohms: 8.0 }, (12, -4), (8, -4));
    sh.ground((8, -4), LEFT);

    // --------------------------------------------------------------- VCO
    // An OTA constant-current integrator inside an op-amp Schmitt loop:
    // `f = Iabc/(4·Vth·C)`, and `Iabc = OTA_IS·(exp(v_bias/VT) − 1)` turns
    // the three-resistor divider below into a true exponential 1 V/octave
    // converter — an octave is 17.9 mV at the bias pin.
    //
    // Drawn as the loop it is: the comparator across the top, the integrator
    // under it, the square running back down the left-hand edge into the
    // integrator's own input and on out to the filter. The triangle is the
    // clean output but the SQUARE is what feeds the filter: the triangle
    // node is a bare integrator and any resistive load on it changes the
    // pitch (the filter's 470 kΩ input would draw current comparable to
    // Iabc itself). A filtered square is also the richer sound.
    // pins [in+, in-, out, bias]
    let ota = sh.part(20, K::Ota, (48, -4), E, 4, true);
    let (sq_in, ota_gnd, tri, vbias) = (ota[0], ota[1], ota[2], ota[3]);
    sh.two(21, cap(C_VCO), (54, -4), (54, -2));
    // pins [in+, in-, out]
    let cmp = sh.part(
        22,
        K::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC },
        (50, -8),
        W,
        6,
        false,
    );
    let (hys, tri_in, sq_out) = (cmp[0], cmp[1], cmp[2]);
    debug_assert_eq!(sq_out, SQ);
    // Schmitt divider.
    sh.two(23, r(R_HYST_TOP), (44, -6), (50, -6));
    sh.two(24, r(R_HYST_BOT), (52, -7), (54, -7));
    // The exponential converter: three resistors on the bias pin. Its
    // Thevenin resistance there is ~150 Ohm, which is what lets the
    // sequencer's BUFFERED CV set pitch to a couple of cents — driving
    // R_SCALE from a bare pot wiper was measured at 844 cents of error.
    sh.two(26, r(R_SCALE), (46, -1), (50, -1));
    sh.two(27, r(R_OFF), (58, RAIL_Y), (58, -1));
    sh.two(28, r(R_GND), vbias, (47, -2));

    // ...and the wiring.
    sh.ground(ota_gnd, UP);
    // The square: down the VCO's left edge, into the integrator and on out
    // to the filter.
    sh.run(&[sq_out, (44, -6), (44, -5), (44, -3), sq_in]);
    // The triangle: round the outside of the comparator and back into it.
    sh.run(&[tri, (54, -4), (56, -4), (56, -9), tri_in]);
    sh.ground((54, -2), LEFT);
    sh.run(&[(50, -6), hys, (52, -7)]);
    sh.ground((54, -7), RIGHT);
    sh.ground((47, -2), LEFT);
    sh.run(&[sq.cv(), (46, -1)]);
    sh.run(&[(58, -1), (51, -1), (50, -1)]);
    sh.run(&[(51, -1), vbias]);

    // -------------------------------------------------------------- VCF
    // A one-pole gm-C low-pass: the OTA drives its own inverting input
    // through the capacitor, so `f0 = gm/(2πC)` and the corner follows the
    // bias current — a real voltage-controlled filter, 6 dB/octave.
    //
    // 470 k : 1 k divides the 10 Vpp square to about ±10 mV, inside the
    // OTA's linear window. Everything inside the filter runs at millivolts
    // and the gain comes back at the mixer.
    //
    // The gm-C strap — output back to the inverting input — used to be drawn
    // by stacking those two pins on one point, which is not a shape an OTA
    // has. It is a wire now, which is how a gm-C integrator is drawn anyway.
    sh.two(30, r(470_000.0), (42, -5), (36, -5));
    sh.two(31, r(1_000.0), (36, -5), (36, -2));
    // pins [in+, in-, out, bias]
    let f = sh.part(32, K::Ota, (34, -4), W, 4, true);
    let (fa, fy_in, fy, fb) = (f[0], f[1], f[2], f[3]);
    // The cutoff resistor and the LFO's injection land straight on the bias
    // pin: no stub, because the symbol's own lead already points at them.
    debug_assert_eq!(fb, (31, -6));
    sh.two(33, cap(22e-9), (32, -2), (32, 0));
    sh.two(37, r(R_CUT_SCALE), (29, -6), (31, -6));
    // CUTOFF — the headline knob. pins [end a, wiper, end b]
    let cut = sh.part(ID_CUTOFF, pot(10_000.0, 0.40), (26, -8), E, 6, true);
    debug_assert_eq!(cut[1], (29, -6));
    // Out to the mixer through a fixed resistor and THEN the bass VCA. The
    // resistor is not a level control and must not become one: an 8 Ohm
    // speaker passes its 0.5 W rating at 2 V rms against a 9 V rail, so a
    // level a player could wind up is a way to burn the speaker by turning
    // something clockwise, and the damage test winds every pot to 0.98. It is
    // also the filter's DC RETURN — an `Ota` output has exactly zero output
    // conductance, so with the VCA shut and nothing else on the node the
    // filter output would float on GMIN. The resistor stays; the VCA goes
    // after it, between it and the virtual ground, where its source really
    // does sit at 0 V and the gate voltage alone sets the gain.
    sh.two(41, r(220_000.0), (30, -5), (26, -5));

    // ------------------------------------------------------- BASS VCA
    // THE SECOND VCA, and the same part as the first: an `Nmos` in its ohmic
    // region is a voltage-controlled resistor, so `gain = Rf/(220k + Rds)`
    // and the envelope on the gate opens and shuts it. About 8 kOhm wide open
    // (3 % off the level the room had when this path was a bare wire) and
    // over 100 MOhm shut.
    //
    // This is deliberately NOT a new `ElementKind`. A real VCA multiplies two
    // unknowns, which is a smooth nonlinearity — a `Vca` device would land in
    // `needs_newton` and cost the room MORE than the transistor it replaced,
    // which is exactly the trade fact 4 in the module docs already measured
    // and rejected. The only cheaper model would sample its control voltage
    // outside the solve, and a gain that does not come out of the solver is
    // the one thing this game does not ship. So: a transistor, the way a
    // Minimoog-era VCA and every cheap noise gate really were.
    // pins [gate, drain, source]
    let bass = sh.part(42, K::Nmos { vt: 1.0, k: NMOS_K }, (24, -1), N, 4, true);
    let (benv_g, bin_d, bsum) = (bass[0], bass[1], bass[2]);
    debug_assert_eq!(bin_d, (26, -5));
    debug_assert_eq!(bsum, (22, -5));
    // INITIAL GAIN — one resistor bypassing the channel, and it is what makes
    // the pitch row audible. A hard-gated bass only sounds on the steps whose
    // BEAT toggle is down, so two of the four pitch knobs would be knobs for
    // a note nobody hears; and the sequencer's windows are contiguous, so
    // there is no per-step trigger to gate them with instead (the gates OR
    // into a constant, and the 0.24 V steps on the CV bus cannot push a
    // diode). With the bypass the bass sings all four notes and the envelope
    // ACCENTS the beats: measured 11.9 dB between shut and wide open, which
    // is a VCA doing what a VCA's initial-gain trim does on real hardware.
    sh.two(43, r(680_000.0), (26, -6), (22, -6));

    // ...and the wiring.
    sh.run(&[fy, (30, -2), (32, -2), (34, -2), fy_in]);
    sh.ground((32, 0), DOWN);
    sh.run(&[(44, -5), (42, -5)]);
    sh.run(&[(36, -5), fa]);
    sh.ground((36, -2), DOWN);
    sh.run(&[fy, (30, -5)]);
    sh.run(&[bin_d, (26, -6)]);
    sh.run(&[(22, -6), bsum, (21, -5)]);
    sh.ground(cut[0], LEFT);
    sh.run(&[cut[2], (32, RAIL_Y)]);


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
    //
    // It gets a lane of its own along the top: up the sequencer's left-hand
    // margin and straight across under the rail.
    sh.run(&[sq.ramp(), (-4, -9)]);
    sh.two(51, cap(1e-6), (-4, -9), (0, -9));
    sh.two(52, r(R_LFO_DEPTH), (0, -9), (6, -9));
    sh.run(&[(6, -9), (34, -9), (34, -6), (31, -6)]);

    // -------------------------------------------------------- noise + SNARE
    // Its own band under the voice, reading right to left like the voice
    // above it: hiss on the right, the VCA in the middle, and the mixer's
    // summing node on the left. The trigger comes up out of the sequencer's
    // BEAT riser at the right-hand end.
    sh.two(
        ID_NOISE,
        K::Noise { volts: 1.0, ohms: 1000.0, seed: 0x00D1_5EA5 },
        (40, 3),
        (44, 3),
    );
    // An anti-alias pole made out of the noise source's OWN 1 kOhm output
    // resistance: fc = 4.8 kHz, one element. The raw source is flat to
    // 25 kHz and the audio tap decimates to 12.5 kHz, so without this most
    // of the snare's power folds back down and it stops sounding like a
    // snare at all.
    sh.two(71, cap(33e-9), (40, 3), (40, 5));
    sh.two(74, r(470_000.0), (40, 3), (36, 3));
    // SNARE TONE: the shunt leg of the input divider, a grounded rheostat.
    // Turning it down attenuates and brightens together. Its wiper and its
    // lower end are both on ground, which used to be drawn by stacking the
    // two pins on the ground point; two ground symbols say the same thing
    // and are a shape a potentiometer actually has.
    // pins [end a, wiper, end b]
    let tone = sh.part(ID_SNARE_TONE, pot(10_000.0, 0.47), (36, 3), S, 2, false);
    // THE SNARE VCA. A MOSFET in its ohmic region is a voltage-controlled
    // resistor between the noise and the mixer's virtual ground, so the
    // gain is `Rf/Rds` and the envelope on the gate opens and closes it:
    // 100 MOhm of leak when shut, about 80 kOhm wide open, ~60 dB of range.
    // See fact 4 in the module docs for why this is not an OTA.
    // pins [gate, drain, source]
    let vca = sh.part(76, K::Nmos { vt: 1.0, k: NMOS_K }, SNARE_ENV, N, 4, true);
    let (senv, sin_d, ssum) = (vca[0], vca[1], vca[2]);
    debug_assert_eq!(senv, SNARE_ENV);
    // ENVELOPE GENERATOR 1 — snare. Diode, storage cap, decay knob: an
    // attack-decay contour with no sustain stage, which is what it is made
    // of and not a curve read out of a table. ATTACK is the storage cap
    // charging through the trigger's own source impedance (about 3 ms here);
    // DECAY is the pot times the cap.
    //
    // The pot is a RHEOSTAT — wiper strapped back to the far end, so a to b
    // is `ohms * wiper` — and the strap is a WIRE, because a potentiometer
    // whose wiper pin sits on one of its own end pins is not a shape a
    // potentiometer has and the placement gate would refuse it.
    // pins [end a, wiper, end b]
    sh.two(77, K::Diode, (34, 7), senv);
    sh.two(78, cap(15e-9), senv, (30, 9));
    let sdec = sh.part(
        79,
        pot(POT_SNARE_DECAY, W_SNARE_DECAY),
        senv,
        W,
        4,
        true,
    );
    debug_assert_eq!(sdec[2], (26, 7));

    // ------------------------------------------------------- trigger glue
    // ONE trigger bus, two envelope generators. Both voices fire on whichever
    // steps the player has toggled on, so the BEAT row is the note-on row.
    //
    // The trigger is DIFFERENTIATED, because a sequencer gate is high for a
    // whole step (170 ms) and an envelope cap fed from a level would just sit
    // there charged. The RC also keeps the envelope caps off an ideal source:
    // charging a cap inside one substep makes the trapezoidal integrator ring
    // (13.8 V was measured on a cap fed from a 7.8 V pulse).
    //
    // `C_TRIG` is 100 nF rather than the 47 nF it was when one envelope hung
    // on it: the pulse has to fill two storage caps now, and the charge it
    // can deliver is `C_TRIG * dV`. Measured, both envelopes peak at 3.0 V
    // against the single envelope's old 3.6 V, which is well clear of the
    // MOSFETs' 1.0 V threshold.
    sh.two(85, cap(C_TRIG), sq.beat(), (36, 8));
    sh.two(86, r(470_000.0), (34, 8), (34, 10));

    // ------------------------------------------------- BASS AD ENVELOPE
    // ENVELOPE GENERATOR 2 — bass, in its own bay under the mixer where the
    // VCA it drives already is. Same three parts as the snare's, same shape
    // on the sheet, a longer decay: this one has to sound like a plucked note
    // rather than a hit, so 15 nF into 10 MOhm is 150 ms against the snare's
    // 50. The envelope BUS is a run along one row with the storage cap and
    // the decay resistor standing up off it, because a node with three things
    // on it is a bus, and the gate rail then drops straight onto the VCA.
    sh.two(87, K::Diode, (34, 2), BASS_ENV);
    sh.two(88, cap(15e-9), (26, 2), (26, 0));
    // pins [end a, wiper, end b]
    let bdec = sh.part(89, pot(POT_BASS_DECAY, W_BASS_DECAY), (28, 2), N, 4, true);
    debug_assert_eq!(bdec[1], (30, 0));
    debug_assert_eq!(bdec[2], (28, -2));

    // ...and the snare's wiring.
    sh.ground((44, 3), RIGHT);
    sh.ground((40, 5), DOWN);
    sh.ground(tone[1], RIGHT);
    sh.ground(tone[2], DOWN);
    sh.run(&[tone[0], sin_d]);
    sh.run(&[ssum, (22, 3), (22, -5)]);
    sh.ground((30, 9), DOWN);
    // SNARE DECAY's rheostat strap, and the ground its far end sits on.
    sh.run(&[sdec[1], (26, 5), sdec[2]]);
    sh.ground(sdec[2], LEFT);
    sh.run(&[(36, 8), (34, 8), (34, 7)]);
    sh.ground((34, 10), DOWN);

    // ...and the bass envelope's. The trigger comes up the column the snare's
    // own diode already stands on, so the two generators visibly share one
    // bus rather than each tapping the beat riser for itself.
    sh.run(&[(34, 7), (34, 2)]);
    sh.run(&[BASS_ENV, (28, 2), (26, 2), (24, 2)]);
    sh.run(&[(24, 2), benv_g]);
    sh.ground((26, 0), UP);
    // BASS DECAY's strap and ground.
    sh.run(&[bdec[1], (30, -1), (28, -1), bdec[2]]);
    sh.ground(bdec[2], LEFT);

    // ---------------------------------------------------------- SEQUENCER
    let mut els = sh.finish();
    els.extend(sequencer::sequencer(&sq));
    name_controls(&mut els, &sq);
    els
}

/// Put the front-panel legend on the parts a player actually touches.
///
/// This is why the room no longer needs a panel region per switch. A panel
/// used to be the only way to put words in the world, so a knob got a name by
/// being wrapped in a box that had one — which is how a five-knob instrument
/// ended up with thirteen windows. A part carries its own name now, so one
/// region can hold the whole control surface and every row still reads.
///
/// Names are the ONLY thing set here: no pin moves, no value changes, and
/// nothing reaches the solver. The netlist is exactly what it was.
fn name_controls(els: &mut [ElementSpec], sq: &Seq) {
    let ids = sequencer::seq_ids(sq);
    let mut named: Vec<(u32, String)> = vec![
        // The supply is a widget too — it has a voltage box — so it needs a
        // legend for the same reason the knobs do.
        (2, "SUPPLY".into()),
        (ID_CUTOFF, "CUTOFF".into()),
        (ID_SNARE_TONE, "SNARE TONE".into()),
        (ID_SNARE_DECAY, "SNARE DECAY".into()),
        (ID_BASS_DECAY, "BASS DECAY".into()),
        (ids.tempo, "TEMPO".into()),
    ];
    for k in 0..ids.steps {
        named.push((ids.pots[k], format!("STEP {} PITCH", k + 1)));
        named.push((ids.switches[k], format!("BEAT {}", k + 1)));
    }
    for e in els.iter_mut() {
        if let Some((_, n)) = named.iter().find(|(id, _)| *id == e.id) {
            e.name = n.clone();
        }
    }
    debug_assert!(
        named
            .iter()
            .all(|(id, _)| els.iter().any(|e| e.id == *id)),
        "a control was named that the circuit does not contain"
    );
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

/// ONE region, holding the whole control surface.
///
/// It used to be thirteen. A panel was the only way to put words in the
/// world, so every knob and switch that needed a legend got wrapped in its
/// own box purely to borrow that box's name — which is how a five-knob
/// instrument grew a thirteen-window sidebar, most of them holding exactly
/// one control.
///
/// Parts carry their own names now (`name_controls`), so one region does the
/// job: a panel lists every part whose pins all sit inside it, and this one
/// spans the instrument, so it lists every control there is — each row
/// reading TEMPO or BEAT 2 rather than POT #405 or SW #433.
///
/// The rect is the sheet plus a margin. It has to CONTAIN each control
/// wholly, pins included, or that control silently drops off the panel.
pub fn synth_panels() -> Vec<PanelDef> {
    vec![PanelDef {
        x0: -13.0,
        y0: -13.0,
        x1: 62.0,
        y1: 61.0,
        name: "SYNTHESIZER",
    }]
}

/// THE BLOCK HEADINGS — the thirteen words this sheet lost, back on it.
///
/// The history in three steps. Every one of these was a `PanelDef` once,
/// because a panel was the only way to put words in the world. Then parts got
/// names, the thirteen windows collapsed into one `SYNTHESIZER` region — and
/// the schematic lost its headings, because the headings WERE the panels. A
/// player looking at the sheet could read every knob and could no longer see
/// where the VCO ended and the filter began.
///
/// So they come back as LABEL BOXES: the same rectangles, the same words, and
/// none of the consequences. Thirteen label boxes open no windows, list no
/// parts, capture no scopes and add nothing to the sidebar. The one control
/// panel is still the one control panel.
///
/// The rects are the originals with one difference, and it is the point of
/// the whole feature. The old FILTER rect was stretched to `x1 = 42.2` purely
/// so a divider's input pin at `(42, -5)` fell inside it — a panel lists a
/// part only when EVERY pin is in the rect, so a heading had to swallow whole
/// devices to keep them on its window. A label box has no membership at all,
/// so this one is sized for READING: it stops at 39, clear of the VCO, and
/// nothing anywhere notices.
pub fn synth_label_boxes() -> Vec<PanelDef> {
    let (ox, oy) = (SEQ_ORIGIN.0 as f64, SEQ_ORIGIN.1 as f64);
    let mut v = vec![
        PanelDef { x0: 42.5, y0: -10.6, x1: 59.0, y1: 0.0, name: "VCO  1V/OCT" },
        // 39.0, not the old 42.2: see above. The heading is drawn where it
        // reads, not where a membership rule needed it to end.
        PanelDef { x0: 22.0, y0: -8.4, x1: 39.0, y1: 1.0, name: "FILTER  CUTOFF" },
        PanelDef { x0: 7.0, y0: -6.0, x1: 22.0, y1: 0.0, name: "MIXER + SPEAKER" },
        PanelDef { x0: -6.0, y0: -10.4, x1: 6.6, y1: -8.4, name: "LFO  BAR SWEEP" },
        PanelDef { x0: 24.0, y0: 1.6, x1: 45.0, y1: 10.6, name: "SNARE  (TONE)" },
        PanelDef {
            x0: ox - 0.6,
            y0: oy - 1.0,
            x1: ox + 16.0,
            y1: oy + 13.5,
            name: "CLOCK  TEMPO",
        },
        PanelDef {
            x0: ox + 20.5,
            y0: oy + 2.0,
            x1: ox + 33.0,
            y1: oy + 40.0,
            name: "STEP DECODER",
        },
    ];
    // One box per step, round the knob and its steering diode, and one round
    // the toggle and its own. Two per step, `SEQ_STEPS` steps.
    let pitch = ["STEP 1 PITCH", "STEP 2 PITCH", "STEP 3 PITCH", "STEP 4 PITCH"];
    let beat = ["BEAT 1", "BEAT 2", "BEAT 3", "BEAT 4"];
    for n in 0..SEQ_STEPS {
        let y = oy + 8.0 + 12.0 * n as f64;
        v.push(PanelDef {
            x0: ox + 37.0,
            y0: y - 1.0,
            x1: ox + 48.6,
            y1: y + 3.0,
            name: pitch[n],
        });
        v.push(PanelDef {
            x0: ox + 37.0,
            y0: y + 4.6,
            x1: ox + 46.6,
            y1: y + 7.4,
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
//   SHIPPED: 250 elements — 77 DEVICES plus the wires and ground symbols of
//   routing. The device count is the budget; the routing is very nearly free,
//   and the two numbers must not be confused.
//
//   ---- THE FOURTH STEP, THE VCAs AND THE ENVELOPES (this change) ----
//
//   Re-measured from scratch, same machine, same session, load average 5–7,
//   best of three passes of 160 000 substeps stepped the way the server steps
//   (chunks of AUDIO_EVERY with a tap on each):
//
//     64 dev / 206 el   BEFORE: three steps, no VCA on the bass  12.88 µs  1.55x  NR 1.99
//     69 dev / 219 el   + bass VCA + its AD envelope             14.44 µs  1.39x  NR 1.98
//     77 dev / 245 el   + THE FOURTH STEP                        17.64 µs  1.13x  NR 2.00
//     77 dev / 250 el   + two DECAY knobs (pots for resistors)   18.04 µs  1.11x  NR 2.00
//
//   So: the VCA and its envelope generator cost +1.56 µs for 5 devices, the
//   fourth step +3.20 µs for 8, and putting a shaft on the two decay
//   resistors +0.40 µs for no devices at all — a pot is a resistor plus a
//   WIPER NODE, and unknowns are what the `^1.64` counts. Newton's iteration
//   count never moved, which is the point: a MOSFET VCA is bought entirely
//   out of the `devices^1.64` term and an OTA VCA would not be (fact 4).
//
//   LIVE SERVER, real binary over a websocket, reading its own `rt` off every
//   audio frame, on the same loaded machine, interleaved:
//
//     64 dev, 60 s   median 0.999  min 0.990  mean 0.9995  0/1800 below 0.97
//     77 dev, 60 s   median 0.999  min 0.990  mean 0.9992  0/1799 below 0.97
//     77 dev, 3 x 90 s  median 0.999 / 0.999 / 0.999  mean 0.9988 / 0.9989 / 0.9984
//     64 dev, 2 x 90 s  median 0.999 / 0.999          mean 0.9977 / 0.9993
//
//   The 90 s runs each catch a handful of samples under 0.97 — and so does
//   the BEFORE room, worse (41/2694 against 25/2696, min 0.734 against
//   0.851). Those are the OS descheduling the sim task on a machine with
//   several agents compiling on it, not the circuit: the room's own cost is
//   flat, and the median does not move. The instrument holds real time and
//   plays in tune.
//
//   What was spent is the MARGIN: 1.55x to 1.11x offline. That is the number
//   the next feature has to come out of, and it is why the four-VCA version
//   of this was not built (a survey measured four voices plus four steps at
//   0.86–0.94x live, which is one to two and a half semitones flat and
//   sustained, not a blip).
//
//   ---- the earlier work, kept because it is why the room is shaped so ----
//
//   Solver, best of three passes of 30 000 substeps, interleaved against the
//   same room drawn the OLD way (star of diagonals, 71 elements, the same 64
//   devices) on the same machine:
//
//     71 el / 64 dev   10.79 / 11.42 / 11.95 µs per substep   1.67–1.85x
//    201 el / 64 dev   12.21 / 13.44 / 13.36 µs per substep   1.49–1.64x
//
//   So drawing the room as a schematic costs about 1.4 µs per substep — 7 %
//   of the 20 µs budget — for 137 more elements. `frame()`, where
//   `solve_wire_currents` is quadratic in the wire count, goes 3.2 → 23.2 µs
//   per tick; at the server's 30 Hz that is 0.1 → 0.7 ms per wall-clock
//   second, which is noise.
//
//   LIVE SERVER, over a websocket, reporting its own `rt`, 900 samples over
//   30 s, three runs: median 0.999 / 0.999 / 1.000, min 0.989, max 1.014,
//   and ZERO samples below 0.97. The instrument holds real time and plays in
//   tune.
//
//   WHY IT IS ALMOST FREE. A `Wire` merges its two ends in `compile`'s
//   union-find and stamps NOTHING (`engine.rs`), so it adds no node, no
//   branch unknown and no Newton work; a `Ground` pins its point to node 0
//   and likewise stamps nothing. What is left is one element visit per
//   substep each. The cost model below is written in elements because, when
//   it was measured, every element was a device.
//
// Cost in this engine is `newton_iterations × devices^1.64`, and BOTH
// factors had to be bought down. The ladder that got here, each rung
// measured on the room as it stood (element counts here are device counts —
// the room carried almost no routing at the time):
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
// AND THAT WAS THE TRAP IN IT. Every rung of that ladder is a DIFFERENT room:
// the 79-element four-step version still carried things that were cut in the
// rungs below it, so "the fourth step took the room from 0.86x to 1.0x" was
// never a controlled comparison — it was the difference between two rooms
// that differed in more than the step count. Measured against the room as it
// ships, with only the step count changed, the fourth step is +3.20 µs and
// the live rt does not move. The cut was right when it was made and wrong to
// carry forward without re-measuring. When a scope note quotes a cost,
// re-take the measurement on the room in front of you before believing it.
//
// What was cut, and why:
//
//   * THE FOURTH STEP — PUT BACK. Eight devices: two comparator OTAs, a
//     zener, a CV pot, a steering diode, a toggle and its diode, plus a
//     ladder resistor. Re-measured against the room as it ships it is
//     +3.20 µs/substep and rt 0.999 live, so it is in, with a note of its own
//     at `SEQ_STEPS`.
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
//   * THE OTA VCA on the bass — still cut, and the bass has a VCA anyway. It
//     is an `Nmos`, one device, for the reason in fact 4: an OTA tracking an
//     audio signal costs the WHOLE room about a third of a Newton iteration
//     (3 µs), because Newton is global, and a MOSFET run as a
//     voltage-controlled resistor costs the `devices^1.64` term alone. A
//     `Vca` DEVICE KIND was considered for this change and rejected on the
//     same arithmetic — see `THE VCAs` in the module docs.
//   * THE ATTACK KNOBS. Both AD generators got a DECAY knob — the pot
//     replaces the decay resistor, so it is the same device count and only
//     the wiper's extra node, measured at +0.40 µs for the pair (17.64 →
//     18.04, 1.13x → 1.11x). An ATTACK knob is a pot in SERIES with each
//     diode, which is two more devices and their nodes on top of that. It was
//     not built: attack here is 3 ms and the interesting control is the
//     decay. If it is ever wanted, measure first — this is the room's whole
//     remaining margin, not a rounding error.
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
