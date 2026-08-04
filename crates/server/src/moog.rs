//! THE LADDER — after the Moog 904A low-pass filter (1965).
//!
//! The historical instrument: Robert Moog's transistor ladder filter, US
//! patent 3,475,623, the 904A module and later the Minimoog's VCF. Four pairs
//! of transistors stacked in two legs, a capacitor bridging the legs at every
//! rung, a differential pair at the bottom taking the signal, and ONE control
//! current fed up the whole stack. Each rung's transistors present a dynamic
//! emitter resistance `re = VT/IE` set by that current, so each rung plus its
//! capacitor is one pole whose corner moves with the current: four identical
//! one-pole sections tracking one knob, 24 dB per octave.
//!
//! ## What is faithful here
//!
//! THE LADDER IS REAL. Ten `Npn` transistors — a bottom differential pair and
//! four rungs of two — four capacitors across the legs, two collector loads,
//! a resistor chain biasing every rung's bases, and a transistor current sink
//! under the whole thing. Nothing here is a filter approximation with a Moog
//! name on it: it is Ebers-Moll junctions in the topology of the patent, and
//! the 24 dB/octave slope, the resonant peak and the passband loss that comes
//! with resonance are all consequences the solver works out.
//!
//! The CONTROL LAW is faithful too, and by the same physics the real one used.
//! The current sink's base sits on the measured three-resistor divider from
//! `synth_vco.rs` — 17.9 mV per octave into a junction — so CUTOFF is a real
//! exponential converter, `Ic = Is·exp(Vbe/VT)`. Moog's own 904 did this with
//! a matched pair and a +3300 ppm tempco resistor to cancel the drift of VT;
//! there is no temperature in this simulator, so the tempco would trim
//! nothing and is the one part of that converter left out.
//!
//! ## What is a stand-in, and what is missing
//!
//!   * ONE OSCILLATOR, not three. A Minimoog has three VCOs and a noise
//!     source into the filter. A second one measured at +8 unknowns and cost
//!     goes as `unknowns^1.64`; see the scope notes. What is here is the
//!     measured `modules::vco` — an OTA integrator in an op-amp Schmitt loop
//!     on the same exponential converter, which is a 921-class saw/triangle
//!     core, not a Moog 921 down to the reset scheme.
//!   * NO RESONANCE, and this is the one that hurts. The real 904 feeds its
//!     output back into the other side of the bottom differential pair with
//!     0–4 of gain; four poles are 180° at the corner, so what is negative
//!     feedback at DC becomes positive there and the filter peaks and finally
//!     sings. Every way of closing that loop here was built and measured, and
//!     the scope notes at the bottom of this file record all four: a divider
//!     straight off the collector cannot drive the base (2 % of output), an
//!     op-amp follower latches at its rail on the startup transient, a
//!     non-inverting gain stage latches too, and the inverting regeneration
//!     amplifier that finally had the gain QUARANTINED THE ROOM at full turn.
//!     A knob a player can turn into a quarantine is not a knob, so it is not
//!     here. What ships is a four-pole ladder with no feedback path at all.
//!   * NO VCA AND NO ENVELOPE. This room is a filter bench and it drones: the
//!     oscillator runs and both knobs do something the moment you touch them.
//!     The kit for a full voice — MOSFET VCA, diode-cap AD generator, 555
//!     gate — is in `modules.rs`.
//!   * The output is taken SINGLE-ENDED off one collector rather than
//!     differentially. Half the gain and no common-mode rejection, and it
//!     saves a four-resistor difference amplifier the budget would notice.
//!
//! ## Signal flow
//!
//! ```text
//!   PITCH  ─> expo converter ─> OTA/Schmitt VCO ─ square ─ cap ─ 470k ─┐
//!                                                                       v
//!   CUTOFF ─> expo converter ─> NPN current sink ─> [ 4-RUNG LADDER ] ──┘
//!                                                          │
//!                                          collector ─ cap ─> mixer ─> 8 Ω
//! ```
//!
//! SCOPE NOTES are at the bottom of this file.

use sim_core::{ElementKind as K, ElementSpec, Point};

use crate::layout::{Sheet, DOWN, E, N, RIGHT, UP, W};
use crate::modules;
use crate::synth_vco::{R_GND, R_SCALE};

// ---------------------------------------------------------------- values

/// Supply. The ladder needs the headroom: five stacked junctions plus a
/// collector load is about 6 V before anything swings.
const SUPPLY_V: f64 = 9.0;

/// Transistor beta. 100 is an ordinary small-signal NPN, and it matters here
/// in one specific way: every rung's bases draw `Ic/beta` out of the bias
/// chain, so the chain has to run at a current far above that or the ladder
/// pulls its own bias down as CUTOFF opens.
const BETA: f64 = 100.0;

/// The rung capacitors. One pole per rung at
/// `f = 1 / (2π · C · 2·re)` with `re = VT/IE`, so 47 n against the
/// 10–160 µA the sink delivers puts the poles between about 330 Hz and
/// 5 kHz, and the four-pole cascade's corner at 0.435 of that.
const C_RUNG: f64 = 100e-9;
/// Collector loads. The ladder's single-ended voltage gain is
/// `Ic/(4·VT) · R_LOAD` — about 11 at the middle of the CUTOFF knob — and the
/// price of it is headroom: the legs drop `Ic/2 · R_LOAD` and rung 4's
/// collectors must stay above their own 4.82 V bases, which is what caps the
/// top of the CUTOFF sweep (`R_SPAN_CUTOFF`).
const R_LOAD: f64 = 22e3;

/// The base-bias chain, rail to ground, top resistor first. The taps land at
/// 4.82 / 4.02 / 3.21 / 2.40 V for the four rungs and 1.42 V for the input
/// pair — about 0.8 V a rung, which is one Vbe plus enough Vce to keep every
/// transistor out of saturation. Chain current ~89 µA against the ~1 µA of
/// base current it feeds.
const R_BIAS: [f64; 6] = [47e3, 9.1e3, 9.1e3, 9.1e3, 11e3, 16e3];

/// Base-stopper into each input-pair base, and the shunt leg of both input
/// dividers.
const R_BASE: f64 = 1e3;
/// Signal attenuator, sized against `R_BASE` to put ±10.6 mV on the base from
/// the VCO's ±5 V square — right at the edge of the differential pair's
/// linear range, so the ladder softens the top of the wave the way the real
/// one does.
const R_IN: f64 = 470e3;
/// Bias DECOUPLING, and the room does not work without it. The input pair's
/// bias tap has ~13 kΩ of chain impedance to ground, which sits in series
/// with the base stoppers and makes both input dividers fifteen times less
/// attenuating than they read. This capacitor makes the tap an AC ground, so
/// `R_IN`/`R_BASE` really is 471:1 and `R_RES`/`R_BASE` really is 2:1.
const C_DECOUPLE: f64 = 10e-6;

/// CUTOFF's exponential converter. `R_SCALE` and `R_GND` are the measured
/// `synth_vco` values and set the slope — 17.9 mV per octave at the junction.
/// `R_OFF` is this room's own, and it sets WHERE the knob starts. Solved,
/// not guessed: with `V = (Vcv/R_SC + 9/R_OFF) / (1/R_SC + 1/R_OFF + 1/R_GND)`
/// and `Ic = Is·exp(V/VT)`, asking for 100 µA at the middle of the knob gives
/// 2427 Ω. The tail then runs 15 µA at CV 0 to 471 µA at CV 5 — five octaves
/// of cutoff — where an OTA's bias pin would have wanted 55 nA.
const R_OFF_CUTOFF: f64 = 2427.0;
/// How far the CUTOFF knob reaches. `R_SPAN` is 80 k in the VCO, which puts
/// 5 V across the pot and five octaves under the knob; here the top of that
/// range runs the ladder at 471 µA and drops rung 4's collectors into their
/// own bases. 157 k stops the wiper at 3.5 V, so the tail runs 15–172 µA and
/// the corner sweeps about 100 Hz to 1.2 kHz with the stack always linear.
const R_SPAN_CUTOFF: f64 = 157e3;

/// Mixer. The filter output is around 0.1 V at the default settings and
/// several times that at resonance, and the op-amp cannot put more than
/// 0.2 V into 8 Ω (`DEFAULT_OPAMP_ISC` × 8), so the leg is deliberately quiet.
const R_MIX: f64 = 1e6;
const R_F: f64 = 470e3;
/// The bleed that gives the output coupling capacitor somewhere to discharge
/// to, and it is sized by the SETTLING TIME and not by loading. At 1 MΩ the
/// time constant is a full second: two seconds after boot the buffer was
/// still sitting 1.2 V above ground, which pushed the mixer op-amp into its
/// 25 mA current clamp and the speaker read a flat −0.19998 V — silent, and
/// for a reason that looked nothing like a DC fault. 100 k settles in a
/// tenth of a second and costs the ladder 9 % of its output swing.
const R_BLEED: f64 = 470e3;
/// The output coupling capacitor, and it is small on purpose. Against
///  the pair is a 1.5 Hz high-pass — inaudible — but a time constant
/// of only 0.1 s, so the node has drained the collector's 8.4 V of bias
/// within half a second of boot. At 1 µF the constant is 0.39 s and every
/// measurement in this file had to wait four seconds for it.
const C_OUT: f64 = 0.22e-6;

// ------------------------------------------------------------------- ids
//
// Blocks own ranges with gaps between them, and the gaps are the point: the
// first build of this room ran `modules::vco` at 20 (which claims 20..=31)
// straight into the ladder's capacitors at 30..34 and its own input resistor
// into the bias chain, and two duplicate ids are not a compile error, they
// are a filter that quietly does not work. `moog_room_is_a_legal_document`
// now asserts every id is unique.
//
//   1, 2      speaker, supply
//   10, 11    mixer
//   20..=31   VCO (`modules::vco` takes twelve)
//   100..=109 the ladder itself, input pair first
//   110..=120 the ladder's passives
//   130..=136 CUTOFF and the current sink
//   140..=150 buffer, resonance, mix leg
pub const ID_SPEAKER: u32 = 1;
pub const ID_SUPPLY: u32 = 2;
pub const ID_PITCH: u32 = 21;
pub const ID_CUTOFF: u32 = 130;

/// Default knob positions. PITCH 0.30 is about 155 Hz; CUTOFF 0.55 puts the
/// four-pole corner near 350 Hz, so the fundamental is through and the
/// square's harmonics are being cut — which is what a filter is for.
pub const PITCH_WIPER: f64 = 0.30;
pub const CUTOFF_WIPER: f64 = 0.55;

// -------------------------------------------------------------- geometry
//
// The ladder is drawn as a ladder: two legs at x = 50 and x = 62, rungs
// eight units apart climbing from the input pair at the bottom to the
// collector loads at the top, and a capacitor lying across the legs at every
// rung. The bias chain runs down the right-hand margin and feeds both sides
// of each rung; the signal enters bottom left, the output leaves top right.

const RAIL_Y: i32 = -20;
/// Left and right leg columns — where every collector and emitter sits.
const XL: i32 = 50;
const XR: i32 = 62;
/// Base columns, just outside each leg.
const XBL: i32 = 46;
const XBR: i32 = 66;
/// The bias chain.
const X_CHAIN: i32 = 74;
/// Rung base rows, bottom (input pair) first.
const Y_IN: i32 = 30;
const Y_RUNG: [i32; 4] = [22, 14, 6, -2];

fn cap(farads: f64) -> K {
    K::Capacitor { farads }
}
fn r(ohms: f64) -> K {
    K::Resistor { ohms }
}
fn npn() -> K {
    K::Npn { beta: BETA }
}

/// The ladder's output collector — rung 4's LEFT-hand leg, the side the
/// signal drives. The choice of leg is what sets the sign of the resonance
/// loop, and the loop here goes the long way round through the mixer:
///
///   in+ (right base) raises the right leg's current, so this left collector
///   RISES — non-inverting. The mixer is an inverting stage. Round the loop
///   that is negative feedback at DC, and four poles turn it 180° at the
///   corner, which is the peak.
pub fn ladder_out() -> Point {
    (XL, -4)
}
/// The AC-coupled filter output: the collector with its 8.4 V of bias taken
/// off. What the mixer sees and what a scope should listen to.
pub fn filter_out() -> Point {
    (84, -4)
}
/// The tail node — the current the whole ladder runs on, one probe.
pub fn tail_node() -> Point {
    (XL, 32)
}
/// The mixer output the speaker hangs on.
pub fn out_node() -> Point {
    (118, -4)
}

/// One rung: a matched pair facing each other across the two legs, bases at
/// `y`, emitters at `y+2` and collectors at `y-2`. Returns the base pins.
fn rung(sh: &mut Sheet, id0: u32, y: i32) -> (Point, Point) {
    // pins [base, collector, emitter]. Left transistor faces east, right
    // one is its mirror image, so the pair is drawn as a pair.
    let l = sh.part(id0, npn(), (XBL, y), E, 4, false);
    let r_ = sh.part(id0 + 1, npn(), (XBR, y), W, 4, true);
    debug_assert_eq!(l[1], (XL, y - 2));
    debug_assert_eq!(l[2], (XL, y + 2));
    debug_assert_eq!(r_[1], (XR, y - 2));
    debug_assert_eq!(r_[2], (XR, y + 2));
    (l[0], r_[0])
}

/// The whole room.
pub fn moog_room_circuit() -> Vec<ElementSpec> {
    let mut sh = Sheet::new(300);

    // ------------------------------------------------------------- supply
    sh.two(
        ID_SUPPLY,
        K::VoltageSource {
            dc: SUPPLY_V,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
            wave: sim_core::Wave::Sine,
        },
        (-16, RAIL_Y),
        (-16, RAIL_Y + 4),
    );
    sh.ground((-16, RAIL_Y + 4), DOWN);
    // One rail line along the top with a corner at every drop.
    sh.run(&[
        (-16, RAIL_Y),
        (-14, RAIL_Y),
        (-6, RAIL_Y),
        (12, RAIL_Y),
        (XL, RAIL_Y),
        (XR, RAIL_Y),
        (X_CHAIN, RAIL_Y),
    ]);

    // ---------------------------------------------------- mixer + speaker
    let (sum, out) = modules::mixer_speaker(&mut sh, 10, ID_SPEAKER, (112, -4), SUPPLY_V, R_F);
    debug_assert_eq!(out, out_node());

    // ---------------------------------------------------------------- VCO
    // `modules::vco`: OTA integrator in an op-amp Schmitt loop on the
    // measured exponential converter. Ids 20..31; the PITCH pot is 21.
    let v = modules::vco(&mut sh, 20, (-8, -2), RAIL_Y, PITCH_WIPER, 0x904A_904A);

    // ------------------------------------------------------- bias chain
    // Rail to ground down the right margin, five taps. Every rung's two
    // bases come off the same tap, which is the point: one voltage per rung,
    // matched pairs, and the ladder's symmetry is a wiring fact.
    let taps: Vec<Point> = std::iter::once((X_CHAIN, RAIL_Y))
        .chain(Y_RUNG.iter().rev().map(|y| (X_CHAIN, *y)))
        .chain(std::iter::once((X_CHAIN, Y_IN)))
        .chain(std::iter::once((X_CHAIN, 40)))
        .collect();
    for (i, w) in taps.windows(2).enumerate() {
        sh.two(110 + i as u32, r(R_BIAS[i]), w[0], w[1]);
    }
    sh.ground((X_CHAIN, 40), RIGHT);

    // ------------------------------------------------------- the ladder
    // Four rungs and the input pair, bottom to top, with a capacitor lying
    // across the legs at each rung's EMITTER row — which is also the row
    // below's collectors. Four caps, four poles, 24 dB an octave.
    let (in_l, in_r) = rung(&mut sh, 100, Y_IN);
    sh.two(116, cap(C_RUNG), (XL, Y_IN - 2), (XR, Y_IN - 2));
    for (k, &y) in Y_RUNG.iter().enumerate() {
        let (bl, br) = rung(&mut sh, 102 + 2 * k as u32, y);
        // This rung's emitters onto the row below's collectors.
        sh.wire((XL, y + 2), (XL, y + 6));
        sh.wire((XR, y + 2), (XR, y + 6));
        // The bridging capacitor, on this rung's own collector row.
        sh.two(117 + k as u32, cap(C_RUNG), (XL, y - 2), (XR, y - 2));
        // Bias: the right base straight off the chain, the left one round
        // the outside on the row between two rungs, where only the two leg
        // wires are and nothing has a pin.
        sh.run(&[(X_CHAIN, y), br]);
        sh.run(&[(X_CHAIN, y), (76, y), (76, y + 4), (38, y + 4), (38, y), bl]);
    }
    // Rung 4's collectors carry the loads. This row is the output.
    sh.two(121, r(R_LOAD), (XL, RAIL_Y + 8), (XL, -4));
    sh.two(122, r(R_LOAD), (XR, RAIL_Y + 8), (XR, -4));
    sh.wire((XL, RAIL_Y), (XL, RAIL_Y + 8));
    sh.wire((XR, RAIL_Y), (XR, RAIL_Y + 8));
    debug_assert_eq!((XL, -4), ladder_out());

    // ---------------------------------------- input pair bias and drive
    // Both bases sit on the 1.42 V tap through a stopper; the signal and the
    // feedback ride on top of it, each through its own attenuator.
    sh.two(123, r(R_BASE), (42, Y_IN), in_l);
    // The right-hand base carries no signal — it is the one the 904's
    // resonance loop would have come back into. It is biased and stopped
    // exactly like its partner so the pair stays matched.
    sh.two(124, r(R_BASE), (70, Y_IN), in_r);
    sh.run(&[(X_CHAIN, Y_IN), (70, Y_IN)]);
    // Row 36, not 26: rung 1 already owns 26 for its own left-hand base, and
    // sharing a corner point with it would put the two taps on one node —
    // which it did, and the whole ladder collapsed into saturation.
    sh.run(&[(X_CHAIN, Y_IN), (76, Y_IN), (76, 36), (38, 36), (38, Y_IN), (42, Y_IN)]);
    // ...and the tap decoupled, so it is an AC ground and both input dividers
    // divide by what they say they do.
    sh.wire((38, Y_IN), (34, Y_IN));
    sh.two(125, cap(C_DECOUPLE), (34, Y_IN), (34, 26));
    sh.ground((34, 26), UP);
    // The signal in: the VCO's square (an op-amp output, so an ideal source)
    // AC-coupled and divided by 471 into the left base.
    sh.run(&[v.square, (26, 10), (32, 10), (32, 40)]);
    sh.two(126, cap(1e-6), (32, 40), (36, 40));
    sh.two(127, r(R_IN), (36, 40), (42, 40));
    sh.run(&[(42, 40), (XBL, 40), in_l]);

    // -------------------------------------------------- CUTOFF -> the tail
    // Rail -> span -> pot -> follower -> the same three-resistor converter
    // the VCO uses, into the base of a transistor current sink. The follower
    // is what makes the law a law: driving `R_SCALE` from a bare pot wiper
    // measured 844 cents of error in `synth_vco`.
    sh.run(&[(-14, RAIL_Y), (-14, 44), (12, 44), (26, 44)]);
    sh.two(131, r(R_SPAN_CUTOFF), (12, 44), (12, 48));
    // pins [end a, wiper, end b]
    let cp = sh.part(ID_CUTOFF, K::Potentiometer { ohms: 100_000.0, wiper: CUTOFF_WIPER }, (12, 52), N, 4, true);
    debug_assert_eq!(cp[1], (14, 50));
    debug_assert_eq!(cp[2], (12, 48));
    sh.ground(cp[0], DOWN);
    // pins [in+, in-, out]
    let f = sh.part(132, K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC }, (16, 50), E, 4, false);
    sh.run(&[cp[1], (14, 49), f[0]]);
    sh.run(&[f[2], (20, 51), f[1]]);
    sh.two(133, r(R_SCALE), (22, 50), (26, 50));
    sh.wire(f[2], (22, 50));
    sh.two(134, r(R_OFF_CUTOFF), (26, 44), (26, 50));
    sh.two(135, r(R_GND), (26, 50), (26, 54));
    sh.ground((26, 54), DOWN);
    // pins [base, collector, emitter]
    let tail = sh.part(136, npn(), (30, 50), E, 4, false);
    debug_assert_eq!(tail[1], (34, 48));
    debug_assert_eq!(tail[2], (34, 52));
    sh.wire((26, 50), tail[0]);
    sh.ground(tail[2], DOWN);
    // The tail bus: the sink's collector under both input emitters.
    sh.run(&[tail[1], (34, 32), (XL, 32), (56, 32), (XR, 32)]);
    debug_assert_eq!((XL, 32), tail_node());

    // -------------------------------------------------------- the output
    // The collector is a high-impedance node sitting at about 8.4 V. It is
    // AC-coupled straight into the mixer's virtual earth and straight into
    // the resonance divider, and there is NO BUFFER between — which is not a
    // saving, it is a bug fix. An op-amp follower here latched: at t=0 the
    // coupling capacitor is empty, so its input steps to 8.4 V, the output
    // hits the rail, and it stayed there — measured in+ 0.023 V, out −9.0 V,
    // which is not a solution of the follower's own equation. The mixer is an
    // INVERTING stage and has never done it.
    sh.run(&[ladder_out(), (80, -4)]);
    sh.two(140, cap(C_OUT), (80, -4), (84, -4));
    debug_assert_eq!((84, -4), filter_out());
    sh.two(141, r(R_BLEED), (84, -4), (84, 0));
    sh.ground((84, 0), DOWN);
    // To the mixer.
    sh.two(143, r(R_MIX), (88, -4), (108, -4));
    sh.wire(filter_out(), (88, -4));
    sh.run(&[(108, -4), (108, -5), sum]);
    let mut els = sh.finish();
    name_controls(&mut els);
    els
}

/// The front-panel legend on the parts a player touches.
fn name_controls(els: &mut [ElementSpec]) {
    let named: &[(u32, &str)] = &[
        (ID_SUPPLY, "SUPPLY"),
        (ID_PITCH, "PITCH"),
        (ID_CUTOFF, "CUTOFF"),
    ];
    for e in els.iter_mut() {
        if let Some((_, n)) = named.iter().find(|(id, _)| *id == e.id) {
            e.name = (*n).to_string();
        }
    }
    debug_assert!(
        named.iter().all(|(id, _)| els.iter().any(|e| e.id == *id)),
        "a control was named that the circuit does not contain"
    );
}

/// ONE control panel spanning the instrument.
pub fn moog_panels() -> Vec<crate::synth::PanelDef> {
    vec![crate::synth::PanelDef {
        x0: -20.0,
        y0: -24.0,
        x1: 130.0,
        y1: 64.0,
        name: "THE LADDER",
    }]
}

/// Block headings, plus the honesty plaque.
pub fn moog_label_boxes() -> Vec<crate::synth::PanelDef> {
    use crate::synth::PanelDef;
    let b = |x0: f64, y0: f64, x1: f64, y1: f64, name: &'static str| PanelDef { x0, y0, x1, y1, name };
    vec![
        b(-11.0, -20.5, 29.0, 8.0, "VCO  1V/OCT  PITCH"),
        // The converter's resistor chain ends at x = 26 and the sink
        // transistor starts at x = 30, so the two headings part at 28. They
        // used to overlap between 28 and 36, and the CURRENT SINK box hung
        // out through the side of the block it was supposed to sit beside.
        b(-15.0, 42.5, 28.0, 55.0, "CUTOFF  EXPO CONVERTER"),
        b(28.0, 45.5, 40.0, 54.0, "CURRENT SINK"),
        // The bias chain is the ladder's east edge at x = 74 and the coupling
        // capacitor starts at x = 80, so these part at 78 rather than lapping
        // over each other by a unit.
        b(37.0, -14.5, 78.0, 35.0, "TRANSISTOR LADDER  4 RUNGS"),
        b(78.0, -8.5, 92.0, 2.0, "OUT  DC BLOCK"),
        b(105.0, -10.0, 126.0, 1.0, "MIXER + SPEAKER"),
        // The plaque. 28 characters a line is what a label box holds.
        b(96.0, 22.0, 128.0, 23.6, "AFTER THE MOOG 904A. THE"),
        b(96.0, 24.0, 128.0, 25.6, "LADDER IS REAL: TEN NPN"),
        b(96.0, 26.0, 128.0, 27.6, "TRANSISTORS, FOUR BRIDGING"),
        b(96.0, 28.0, 128.0, 29.6, "CAPS, ONE CURRENT SINK, AND"),
        b(96.0, 30.0, 128.0, 31.6, "AN EXPONENTIAL CONVERTER ON"),
        b(96.0, 32.0, 128.0, 33.6, "ITS BASE - 17.7 MV/OCTAVE,"),
        b(96.0, 34.0, 128.0, 35.6, "THE SAME JUNCTION MOOG USED"),
        b(96.0, 36.0, 128.0, 37.6, "(NO TEMPCO: NOTHING HERE"),
        b(96.0, 38.0, 128.0, 39.6, "DRIFTS WITH TEMPERATURE)."),
        b(96.0, 41.0, 128.0, 42.6, "MISSING, AND SAID PLAINLY:"),
        b(96.0, 43.0, 128.0, 44.6, "RESONANCE. THE LOOP NEEDS A"),
        b(96.0, 45.0, 128.0, 46.6, "BUFFER, EVERY BUFFER TRIED"),
        b(96.0, 47.0, 128.0, 48.6, "EITHER LATCHED AT ITS RAIL"),
        b(96.0, 49.0, 128.0, 50.6, "OR QUARANTINED THE ROOM AT"),
        b(96.0, 51.0, 128.0, 52.6, "FULL TURN. A KNOB THAT CAN"),
        b(96.0, 53.0, 128.0, 54.6, "BREAK THE SIM IS NOT A KNOB"),
        b(96.0, 55.0, 128.0, 56.6, "- SEE THE SCOPE NOTES."),
        b(96.0, 58.0, 128.0, 59.6, "ALSO ONE OSCILLATOR, NOT"),
        b(96.0, 60.0, 128.0, 61.6, "THREE, AND NO VCA."),
    ]
}

// ------------------------------------------------------------ SCOPE NOTES
//
// Measured on this machine (Apple M4, release, pinned 1.95.0) with several
// other agents building in parallel. Method as `synth.rs`; the room bench is
// `roombench.rs`.
//
//   ---- THE LADDER, WHICH IS THE WHOLE POINT ----
//
//   Ten Ebers-Moll transistors in the topology of the patent, and it works.
//   DC, at the middle of the CUTOFF knob (asserted by
//   `moog_ladder_is_biased_and_every_rung_is_active`):
//
//     bias chain taps     1.372  2.326  3.123  3.930  4.745 V
//     left leg collectors 1.750  2.545  3.350  4.166  8.482 V
//     right leg           1.747  2.549  3.357  4.173  8.560 V
//     current sink base   0.5954 V   (designed 0.5956)   tail 0.791 V
//
//   Every rung sits 0.8 V above the one below, every collector is clear of
//   its own base, and the two legs match to 8 mV. The tail runs 94 µA
//   against a design figure of 100.
//
//   The response, measured through the room's own square-wave oscillator
//   (`moog_response`), in dB relative to the input at the base:
//
//     CUTOFF 0.25    78 Hz +9.1   156 +1.8   311 −8.9   622 −20.4  1245 −24.5
//     CUTOFF 0.55    78 Hz +15.9  156 +10.0  311 +2.2   622 −8.3   1245 −20.7
//     CUTOFF 0.85    78 Hz +22.3  156 +16.9  311 +10.3  622 +2.5   1245 −8.6
//
//   The knob moves the same note by 31 dB across its travel, and an octave
//   past the corner costs 10–12 dB — which for a SQUARE wave, whose harmonics
//   walk into the stopband with it, is what a 24 dB/octave slope looks like.
//   A sine sweep would read steeper; this is the honest measurement of the
//   thing the room actually makes.
//
//   ---- FOUR WAYS TO FAIL TO BUILD A RESONANCE LOOP ----
//
//   The 904 closes a feedback loop from the ladder's output back into the
//   other side of the bottom pair. Every way of doing that here was built and
//   measured, and none of them shipped:
//
//   1. STRAIGHT OFF THE COLLECTOR into a divider. The collector is a 22 kΩ
//      source and the base stopper is 1 kΩ; a 10 k track at mid-travel adds
//      2.5 kΩ more. Measured: the knob moved the output by 2 %, from 0.1121
//      to 0.1101 Vpp across its whole travel. Raising the stopper to 10 k to
//      fix the impedance ratio breaks the signal divider instead.
//   2. AN OP-AMP FOLLOWER to fix the impedance. It LATCHED. At t = 0 the
//      output coupling capacitor is empty, so the follower's input steps to
//      8.4 V and its output hits the rail — and stayed there: measured
//      in+ 0.023 V, in− −9.0 V, out −9.0 V, which is not a solution of a
//      unity follower's own equation. Every measurement downstream read a
//      flat zero.
//   3. A NON-INVERTING GAIN STAGE (1 + 39k/10k). Same latch, other rail:
//      in+ 0.014 V, in− 1.837 V, out +9.0 V.
//   4. AN INVERTING REGENERATION AMPLIFIER, which is the configuration the
//      mixer uses and the only one that never latched. It had the gain — and
//      at RESONANCE full with CUTOFF at 0.75 the room QUARANTINED. A knob a
//      player can turn into a quarantine is not a knob (see the standing rule
//      about preventing invalid moves rather than tolerating them), so the
//      loop came out and the plaque on the sheet says so.
//
//   AND THE ROOM HAS THE BUDGET FOR IT. Measured at 7.62 µs/substep = 2.63x
//   real time with 40 unknowns and NR 1.22 — the cheapest of the instrument
//   rooms and less than half the shipped synth. So the missing knob is not a
//   budget cut and must not be described as one: it is a stability problem,
//   and the honest thing is to say which.
//
//   What would fix it is a buffer that cannot latch — the honest candidates
//   are an emitter follower (one more `Npn`, and a smooth nonlinearity on a
//   room that already carries eleven) or teaching the op-amp model to recover
//   from a clamped state. Both are bigger than this room.
//
//   ---- WHAT ELSE THE MEASUREMENTS CAUGHT ----
//
//   * TWO DUPLICATE IDS, neither a compile error. `modules::vco` at id 20
//     claims 20..=31; the ladder's capacitors had been given 30..34 and the
//     input resistor 45, which the bias chain also used. The filter still
//     "worked" and read 12 dB flat at every setting.
//   * A SHORTED BIAS ROUTE. Rung 1's left-hand base and the input pair's both
//     routed round the ladder through the corner (76, 26) — the same point,
//     so the same node. Measured: the chain read 0.570 / 0.570 / 1.602 /
//     2.634 / 3.666 V, the whole stack saturated, and the filter was flat.
//     Routing rows are now one per tap.
//   * A 1 SECOND TIME CONSTANT. The output bleed was 1 MΩ against 1 µF, so
//     two seconds after boot the coupled node still sat 1.2 V above ground —
//     enough to hold the mixer op-amp in its 25 mA current clamp at a dead
//     −0.19998 V. Every settling measurement in this file now runs 4 s.
//   * THE DITHER NOISE SOURCE SHUNTING THE INTEGRATOR. A `Noise` is an EMF
//     behind 1 kΩ; wired from the VCO's triangle node to ground it is a short
//     across a node being charged with tens of nanoamps. The triangle
//     collapsed to ±7 mV and the oscillator ran at 11.6 kHz at every pitch.
//     It belongs in series UNDER the timing capacitor, and now does
//     (`modules::vco`), where it does the job it is for: 79 / 159 / 366 /
//     639 Hz measured against 78 / 156 / 357 / 622 ideal.
//
#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Engine;

    const DT: f64 = 20e-6;

    fn tweak(id: u32, wiper: f64, els: &mut [ElementSpec]) {
        for e in els.iter_mut() {
            if e.id == id {
                if let K::Potentiometer { wiper: w, .. } = &mut e.kind {
                    *w = wiper;
                }
            }
        }
    }

    /// Peak-to-peak of a node over `secs`, after 0.8 s of settling — eight
    /// time constants of the output coupling network. A measurement taken
    /// before that network has drained reads the capacitor, not the filter,
    /// which is how a room that was working measured as silent.
    fn swing(els: &[ElementSpec], p: Point, secs: f64) -> (f64, bool) {
        let mut e = Engine::new(DT);
        e.set_elements(els);
        e.advance(40_000);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for _ in 0..(secs / DT) as u32 {
            e.advance(1);
            let v = e.voltage_at(p).unwrap_or(0.0);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (hi - lo, e.is_quarantined())
    }

    /// Every part is a legal shape, every wire orthogonal, every id unique.
    /// The id check is not boilerplate: this room shipped a filter that did
    /// not filter because `modules::vco` claims 20..=31 and the ladder's
    /// capacitors had been given 30..34.
    #[test]
    fn moog_room_is_a_legal_document() {
        let els = moog_room_circuit();
        for e in &els {
            assert!(
                sim_core::shape::is_rigid(&e.kind, &e.pins),
                "element {} ({}) is not in its own family: {:?}",
                e.id,
                e.kind.tag(),
                e.pins
            );
            if matches!(e.kind, K::Wire) {
                let (a, b) = (e.pins[0], e.pins[1]);
                assert!(a.0 == b.0 || a.1 == b.1, "diagonal wire {}: {a:?} {b:?}", e.id);
            }
        }
        let mut ids: Vec<u32> = els.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), els.len(), "duplicate element id");
    }

    /// The ladder is BIASED, which is the thing a stack of ten transistors
    /// can silently stop being. Every rung must sit about 0.8 V above the one
    /// below with its collector clear of its own base, or the stack saturates
    /// and the filter goes flat — which is exactly what a shorted bias route
    /// did to the first build.
    #[test]
    fn moog_ladder_is_biased_and_every_rung_is_active() {
        let els = moog_room_circuit();
        let mut e = Engine::new(DT);
        e.set_elements(&els);
        e.advance(40_000);
        assert!(!e.is_quarantined());
        let v = |p: Point| e.voltage_at(p).expect("node missing");
        // The chain: five taps, each above the last.
        let taps: Vec<f64> = std::iter::once(v((X_CHAIN, Y_IN)))
            .chain(Y_RUNG.iter().map(|y| v((X_CHAIN, *y))))
            .collect();
        for w in taps.windows(2) {
            assert!(
                w[1] - w[0] > 0.6 && w[1] - w[0] < 1.2,
                "bias steps are {taps:?} — the chain is loaded or shorted"
            );
        }
        // Every rung's collector above its own base: not saturated.
        for (k, y) in Y_RUNG.iter().enumerate() {
            let (base, coll) = (taps[k + 1], v((XL, y - 2)));
            assert!(
                coll > base - 0.2,
                "rung {k} collector {coll:.3} V is under its base {base:.3} V"
            );
        }
        // ...and the two legs match, because the pairs match.
        for y in Y_RUNG {
            let (l, r_) = (v((XL, y - 2)), v((XR, y - 2)));
            assert!((l - r_).abs() < 0.2, "legs differ by {:.3} V at y={y}", l - r_);
        }
    }

    /// CUTOFF really moves the corner, and the slope past it is a FOUR-POLE
    /// slope. Measured through the room's own square-wave oscillator, so the
    /// numbers are smaller than a sine sweep's — a square carries its
    /// harmonics into the stopband with it.
    #[test]
    fn moog_cutoff_moves_the_corner() {
        let db = |cw: f64, pw: f64| {
            let mut els = moog_room_circuit();
            tweak(ID_CUTOFF, cw, &mut els);
            tweak(ID_PITCH, pw, &mut els);
            let (s, q) = swing(&els, ladder_out(), 0.2);
            assert!(!q, "quarantined at CUTOFF {cw} PITCH {pw}");
            20.0 * (s / 0.021).log10()
        };
        // Two octaves above the corner the ladder is deeply down...
        let shut = db(0.25, 0.90);
        assert!(shut < -20.0, "high note at low cutoff only {shut:.1} dB");
        // ...and opening the knob brings it back.
        let open = db(0.85, 0.90);
        assert!(
            open - shut > 15.0,
            "CUTOFF moved the same note by only {:.1} dB",
            open - shut
        );
        // The roll-off past the corner is steep: an octave costs real dB.
        let (a, b) = (db(0.25, 0.50), db(0.25, 0.70));
        assert!(
            a - b > 8.0,
            "one octave past the corner only cost {:.1} dB",
            a - b
        );
    }

    /// Alive from boot, at every corner of both knobs, and never louder than
    /// the mixer op-amp can actually be: `DEFAULT_OPAMP_ISC` × 8 Ω = 0.2 V.
    #[test]
    fn moog_room_never_quarantines_and_plays() {
        for (cw, pw) in [(0.02, 0.02), (0.02, 0.98), (0.98, 0.02), (0.98, 0.98), (CUTOFF_WIPER, PITCH_WIPER)] {
            let mut els = moog_room_circuit();
            tweak(ID_CUTOFF, cw, &mut els);
            tweak(ID_PITCH, pw, &mut els);
            let mut eng = Engine::new(DT);
            eng.set_elements(&els);
            let mut rescues = 0;
            for _ in 0..100 {
                let rep = eng.advance(500);
                rescues += rep.rescues;
                assert!(!eng.is_quarantined(), "quarantined at CUTOFF {cw} PITCH {pw}");
            }
            assert_eq!(rescues, 0, "rescue steps at CUTOFF {cw} PITCH {pw}");
        }
        let els = moog_room_circuit();
        let (s, _) = swing(&els, out_node(), 0.3);
        assert!(s > 0.004, "speaker swing {s:.5} Vpp — the room is silent");
        assert!(s < 0.38, "speaker swing {s:.5} Vpp — into the current clamp");
    }

    /// Controls are named and inside the one panel.
    #[test]
    fn moog_controls_are_named_and_reachable() {
        let els = moog_room_circuit();
        let panels = moog_panels();
        assert_eq!(panels.len(), 1);
        let p = &panels[0];
        let mut n = 0;
        for e in &els {
            if matches!(e.kind, K::Potentiometer { .. } | K::Switch { .. }) {
                n += 1;
                assert!(!e.name.trim().is_empty(), "control {} unnamed", e.id);
                for (x, y) in &e.pins {
                    let (x, y) = (f64::from(*x), f64::from(*y));
                    assert!(
                        x >= p.x0 && x <= p.x1 && y >= p.y0 && y <= p.y1,
                        "control {} ({}) is outside the panel",
                        e.id,
                        e.name
                    );
                }
            }
        }
        assert_eq!(n, 2, "PITCH and CUTOFF, and nothing unnamed");
    }

    /// The shared VCO block on its own bench: does it oscillate, and where?
    #[test]
    #[ignore = "measurement: cargo test --release -p server moog_vco_bench -- --ignored --nocapture"]
    fn moog_vco_bench() {
        for w in [0.10, 0.30, 0.54, 0.70] {
            let mut sh = Sheet::new(300);
            sh.two(
                1,
                K::VoltageSource { dc: SUPPLY_V, amp: 0.0, hz: 0.0, phase: 0.0, wave: sim_core::Wave::Sine },
                (-16, RAIL_Y),
                (-16, RAIL_Y + 4),
            );
            sh.ground((-16, RAIL_Y + 4), DOWN);
            sh.run(&[(-16, RAIL_Y), (-6, RAIL_Y), (12, RAIL_Y)]);
            let v = modules::vco(&mut sh, 20, (-8, -2), RAIL_Y, w, 0x904A_904A);
            let els = sh.finish();
            let mut e = Engine::new(DT);
            e.set_elements(&els);
            e.advance(50_000);
            let mut xs = 0u32;
            let mut last = e.voltage_at(v.square).unwrap_or(0.0);
            for _ in 0..50_000 {
                e.advance(1);
                let s = e.voltage_at(v.square).unwrap_or(0.0);
                if (s > 0.0) != (last > 0.0) {
                    xs += 1;
                }
                last = s;
            }
            println!(
                "w={w}: cv {:.3} bias {:.4} f {:.1} Hz (ideal {:.1})",
                e.voltage_at(v.cv).unwrap_or(f64::NAN),
                e.voltage_at(v.bias).unwrap_or(f64::NAN),
                f64::from(xs) / 2.0,
                55.0 * (5.0f64 * w).exp2(),
            );
        }
    }

    /// The whole response surface, printed. Slow; run it by hand.
    #[test]
    #[ignore = "measurement: cargo test --release -p server moog_response -- --ignored --nocapture"]
    fn moog_response() {
        let els = moog_room_circuit();
        let mut e = Engine::new(DT);
        e.set_elements(&els);
        e.advance(200_000);
        let v = |p: Point| e.voltage_at(p).unwrap_or(f64::NAN);
        println!(
            "{} elements, {} devices, {} unknowns",
            els.len(),
            els.iter().filter(|x| !matches!(x.kind, K::Wire | K::Ground)).count(),
            e.unknowns()
        );
        println!(
            "bias taps in {:.3} r1 {:.3} r2 {:.3} r3 {:.3} r4 {:.3}; sink base {:.4}; tail {:.3}",
            v((X_CHAIN, Y_IN)), v((X_CHAIN, 22)), v((X_CHAIN, 14)), v((X_CHAIN, 6)), v((X_CHAIN, -2)),
            v((26, 50)), v(tail_node())
        );
        for cw in [0.15, 0.35, 0.55, 0.75, 0.95] {
            let mut line = format!("CUTOFF {cw:.2}:");
            for pw in [0.10, 0.30, 0.50, 0.70, 0.90] {
                let mut e2 = moog_room_circuit();
                tweak(ID_CUTOFF, cw, &mut e2);
                tweak(ID_PITCH, pw, &mut e2);
                let (s, q) = swing(&e2, ladder_out(), 0.2);
                line += &format!("  {:.0}Hz {:+.1}dB{}", 55.0 * (5.0f64 * pw).exp2(), 20.0 * (s / 0.021).log10(), if q { "!" } else { "" });
            }
            println!("{line}");
        }
    }
}
