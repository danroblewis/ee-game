//! THE SCREAM — after the Korg MS-20's filter.
//!
//! The historical instrument: Korg's MS-20 (1978) took its low-pass from the
//! "Korg35" module — a two-pole SALLEN-KEY section whose resistors are
//! current-controlled transistor elements, wrapped in a positive-feedback
//! loop for resonance, with TWO ANTI-PARALLEL DIODES clipping that feedback
//! path. Those diodes are the whole personality. They limit the loop to
//! about 1.4 V peak-to-peak, so the filter self-oscillates into a hard limit
//! instead of running away; and because the resonance is a feedback path
//! rather than a loss inside the core, the passband does NOT thin out as you
//! turn PEAK up — which is precisely the opposite of the Moog ladder next
//! door, and why an MS-20 screams where a Minimoog hoots.
//! (Tim Stinchcombe's MS10/MS20 filter study; Will Pirkle's Korg35 notes.)
//!
//! ## What is faithful
//!
//! IT IS REALLY TWO POLES. 12 dB/octave is not a compromise here, it is what
//! an MS-20 filter IS — measured **−12.2 and −12.1 dB/octave** across
//! 500→1000→2000 Hz with the corner down at 79 Hz. The ladder room next door
//! is the 24 dB/octave one, and the difference is audible and real.
//!
//! THE DIODE LIMITER IS REAL, AND IT IS THE POINT. Two `Diode`s, anti-
//! parallel, from the resonance node to ground. They are not decoration, and
//! the proof is that they are INERT at low PEAK and BITING at high — measured
//! as how far the resonance node falls short of what the PEAK divider alone
//! would put there:
//!
//! | PEAK | node   | divider alone | clamp |
//! |------|--------|---------------|-------|
//! | 0.05 | 0.019  | 0.019         | 1.0×  |
//! | 0.30 | 0.131  | 0.178         | 1.4×  |
//! | 0.55 | 0.201  | 0.413         | 2.1×  |
//! | 0.80 | 0.264  | 0.719         | 2.7×  |
//! | 0.95 | 0.349  | 1.040         | 3.0×  |
//!
//! and in the room as it ships, with the oscillator driving it rather than a
//! bench sine, 1.3× at PEAK 0.10 rising to **6.6× at PEAK 0.95**. A ratio is
//! the honest way to state it: it needs no guess about the model's forward
//! voltage, only whether the parts are doing anything. `ms20_diodes_clip_
//! only_when_the_peak_is_up` fails at either end.
//!
//! THE RESONANCE IS A FEEDBACK LOOP AROUND THE CORE, as Korg's is, not a
//! damping control inside it. So the passband holds up: measured, the gain
//! well below the corner moves from 0.2962 V to 0.2978 V — **0.05 dB** —
//! between PEAK 0.05 and PEAK 0.95, while the corner itself gains **9.4 dB**
//! (0.3650 → 1.0781 V). A Moog ladder loses its bass to resonance by
//! construction; this does not, and that is the honest difference between
//! the two rooms.
//!
//! IT SINGS WITH NOTHING GOING IN. With DRIVE and NOISE shut right off the
//! amplifier still swings 0.0366 V at PEAK 0.10 and 0.9588 V at PEAK 0.99 —
//! a 26× rise from turning a knob on a room with no input. Stated precisely,
//! because the difference matters: what is measured is a Q high enough that
//! the microvolts still leaking past a shut-off DRIVE come back out at
//! nearly a volt, which is what "self-oscillation" means on a real filter
//! too. It is not a claim that the poles have crossed the axis with the
//! input at exactly zero. The diodes are what stop it going further.
//!
//! CUTOFF IS A REAL CURRENT CONTROL on the transconductors, and Q TRACKS IT:
//! the damping transconductor is biased from the SAME control node through a
//! fixed resistor, so `Q = gm/gm_damp` is a resistor ratio and stays put
//! while the corner sweeps. Measured −3 dB corner against the knob:
//!
//! | CUTOFF | 0.10 | 0.30 | 0.55 | 0.80 | 0.95 |
//! |--------|------|------|------|------|------|
//! | −3 dB  | 79   | 484  | 955  | 1342 | 1684 Hz |
//!
//! ## What is a stand-in
//!
//!   * THE CORE IS A STATE-VARIABLE TRANSCONDUCTOR PAIR, NOT A SALLEN-KEY
//!     WITH TRANSISTOR VCRs. Two `Ota`s round two capacitors give the same
//!     two poles and the same `f0 = gm/2πC` current law, and they converge;
//!     Korg's arrangement of the same two poles does not exist as parts here.
//!     Two poles, one control current, honest — but it is not a Korg35.
//!   * ONE OP-AMP is the resonance loop's gain stage where the MS-20 uses a
//!     transistor pair, and the same op-amp is the room's output amplifier.
//!     It costs Newton nothing, which is what pays for the two diodes.
//!   * THE SOURCE IS THIS PROJECT'S OWN VCO (a 921-class triangle/square core
//!     — see `the-ladder`), not an MS-20 VCO, plus a noise source, because a
//!     filter room with nothing going through it is a room with nothing to
//!     hear. The MS-20's own oscillators are a different instrument.
//!   * NO ENVELOPE AND NO VCA. This room is the filter: PEAK, CUTOFF, DRIVE,
//!     NOISE and PITCH, played by hand. The MS-20 is a whole synthesiser.
//!
//! ## Signal flow
//!
//! ```text
//!   VCO ─ square ─ 1M ──┐
//!                       ├─> A ─> [ OTA ─ C ─> X ─> OTA ─ C ─> Y ] ─┐
//!   NOISE ─ level ─470k ┘         ^                  ^  damping    │
//!                                 └──── in- ─────────┘   OTA       │
//!                                                                  │
//!   Y ─> op-amp ×197 ─> F ─┬─> 470k ─> MIXER ─> 8 Ω                │
//!                          └─> PEAK ─> D ─ 2 DIODES to gnd ─ 1M2 ──┘
//! ```
//!
//! SCOPE NOTES are at the bottom of this file.

use sim_core::{ElementKind as K, ElementSpec, Point};

use crate::layout::{Sheet, DOWN, E, N, RIGHT, UP};
use crate::modules;

// ---------------------------------------------------------------- values

const SUPPLY_V: f64 = 9.0;
const RAIL_Y: i32 = -30;

/// The two integrating capacitors. `f0 = gm/(2πC)`, and 22 nF puts the whole
/// audio range inside a bias current the exponential-ish converter can reach.
const C_FILT: f64 = 22e-9;

/// Input attenuator. THE OTA'S LINEAR WINDOW IS ABOUT ±20 mV (`tanh(vd/2VT)`)
/// and the VCO's square is ±5 V, so the signal has to come down by a factor
/// of a few hundred before it touches a transconductor. Everything inside
/// this filter runs at millivolts; the gain is taken back after it.
const R_DRIVE_IN: f64 = 1e6;
/// DRIVE, as a shunt: winding it DOWN shunts the input node harder and makes
/// the filter quieter, winding it up drives the transconductors into their
/// own tanh. That is a real MS-20 control and a real distortion.
const POT_DRIVE: f64 = 10e3;
pub const DRIVE_WIPER: f64 = 0.15;

/// Noise, and its own level. An MS-20 is as famous for filtered hiss as for
/// filtered oscillators.
const NOISE_V: f64 = 2.0;
const NOISE_R: f64 = 1000.0;
const NOISE_SEED: u32 = 0x5320;
const POT_NOISE: f64 = 100e3;
pub const NOISE_WIPER: f64 = 0.15;
const R_NOISE_IN: f64 = 470e3;

/// CUTOFF, straight across the rail, and the resistor from its wiper to the
/// shared bias node of both integrators.
const POT_CUTOFF: f64 = 10e3;
pub const CUTOFF_WIPER: f64 = 0.55;
const R_CUT_BIAS: f64 = 470e3;
/// The damping transconductor's bias, from THE SAME control node. Roughly
/// twice `R_CUT_BIAS` (2.13x, on preferred values) because that resistor
/// feeds two OTAs and this one feeds one, so the per-OTA currents come out
/// near enough equal and `Q = gm/gm_damp` is a pure RESISTOR RATIO. That is
/// what keeps Q put while the corner sweeps three and a half octaves — the
/// thing a cascade of independent one-poles cannot do.
const R_DAMP_BIAS: f64 = 1e6;

/// The resonance loop's gain stage, `1 + RF/RG`. It has two jobs: it lifts
/// the filter's millivolts to the volt level the DIODES need in order to
/// conduct at all, and it is the room's output amplifier.
const R_F_GAIN: f64 = 1e6;
const R_G_GAIN: f64 = 5.1e3;

/// PEAK: how much of the amplified output goes back round the loop.
const POT_PEAK: f64 = 100e3;
pub const PEAK_WIPER: f64 = 0.55;
/// The feedback resistor, from the clipped node back into the filter's second
/// integrator. Sized so that a full turn of PEAK just cancels the damping
/// transconductor's conductance — self-oscillation at the top of the knob and
/// not before.
const R_FEEDBACK: f64 = 1.2e6;

/// Output: the gain stage into the mixer's virtual earth. `v(out) =
/// −v(F)·RF_MIX/R_OUT`, trimmed against the op-amp's own 0.2 V wall.
const R_OUT: f64 = 470e3;
const RF_MIX: f64 = 15e3;

/// Ids a player touches.
pub const ID_SPEAKER: u32 = 1;
pub const ID_SUPPLY: u32 = 2;
/// The VCO block takes 20..=31; its PITCH pot is 21.
pub const ID_PITCH: u32 = 21;
pub const ID_DRIVE: u32 = 41;
pub const ID_NOISE: u32 = 44;
pub const ID_CUTOFF: u32 = 50;
pub const ID_PEAK: u32 = 71;

// ------------------------------------------------------------------ nodes

/// The filter's input node, where the oscillator and the noise sum. A few
/// millivolts, never more.
pub fn in_node() -> Point {
    (34, 0)
}
/// The first integrator's output.
pub fn x_node() -> Point {
    (46, 0)
}
/// The second integrator's output — THE FILTER OUTPUT, still at millivolts.
pub fn y_node() -> Point {
    (58, 0)
}
/// The shared bias node of the two integrating transconductors: cutoff, live.
pub fn cut_bias() -> Point {
    (45, -8)
}
/// The amplified output — volts, and what the speaker and the loop both take.
pub fn amp_out() -> Point {
    (78, 0)
}
/// The resonance node the two diodes clamp. This is the one to watch.
pub fn res_node() -> Point {
    (74, 14)
}
/// The mixer output the speaker hangs on.
pub fn out_node() -> Point {
    (94, -4)
}

fn cap(farads: f64) -> K {
    K::Capacitor { farads }
}
fn r(ohms: f64) -> K {
    K::Resistor { ohms }
}
fn pot(ohms: f64, wiper: f64) -> K {
    K::Potentiometer { ohms, wiper }
}

/// The whole room.
pub fn ms20_room_circuit() -> Vec<ElementSpec> {
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
        (-26, RAIL_Y),
        (-26, RAIL_Y + 4),
    );
    sh.ground((-26, RAIL_Y + 4), DOWN);
    // EVERY TAP MUST BE A VERTEX OF THIS RUN. A `Wire` merges its two
    // ENDPOINTS and nothing in between, so a part whose pin lands halfway
    // along a segment is not connected to it — it is drawn touching the rail
    // and floating. `modules::vco` at `a` taps the rail at `a.0 + 2` and
    // `a.0 + 20`, which is why −20 and −2 are listed here: without them the
    // oscillator has no supply, its comparator never flips, and the only
    // thing left on the triangle is the 5 mV dither. Measured exactly that.
    sh.run(&[
        (-26, RAIL_Y),
        (-22, RAIL_Y),
        (-20, RAIL_Y),
        (-2, RAIL_Y),
        (40, RAIL_Y),
        (52, RAIL_Y),
    ]);

    // ---------------------------------------------------------------- VCO
    // The project's own 921-class core: an OTA constant-current integrator
    // in an op-amp Schmitt loop, on a bias-diode exponential converter. Ids
    // 20..=31; PITCH is 21. The square is an op-amp output, so it can be
    // loaded as hard as we like.
    let vco = modules::vco(&mut sh, 20, (-22, -8), RAIL_Y, 0.30, 0x2001);
    debug_assert_eq!(vco.square, (12, -14));

    // ------------------------------------------------- input mix + DRIVE
    // The square down to millivolts, and a shunt pot to set how hard the
    // transconductors are driven.
    sh.run(&[vco.square, (12, -14), (24, -14), (24, 0)]);
    sh.two(40, r(R_DRIVE_IN), (24, 0), (30, 0));
    sh.run(&[(30, 0), in_node()]);
    // pins [end a, wiper, end b]; a rheostat to ground, wiper strapped over.
    let dp = sh.part(ID_DRIVE, pot(POT_DRIVE, DRIVE_WIPER), (30, 4), E, 4, false);
    debug_assert_eq!(dp[1], (32, 2));
    debug_assert_eq!(dp[2], (34, 4));
    sh.run(&[(30, 0), (30, 4)]);
    sh.run(&[dp[1], (34, 2), dp[2]]);
    sh.ground(dp[2], RIGHT);

    // ---- noise, and its level. Straight into the same summing node.
    sh.two(
        42,
        K::Noise {
            volts: NOISE_V,
            ohms: NOISE_R,
            seed: NOISE_SEED,
        },
        (18, 14),
        (18, 18),
    );
    sh.ground((18, 18), DOWN);
    // pins [end a, wiper, end b]
    let np = sh.part(ID_NOISE, pot(POT_NOISE, NOISE_WIPER), (18, 14), E, 4, false);
    debug_assert_eq!(np[1], (20, 12));
    debug_assert_eq!(np[2], (22, 14));
    sh.ground(np[2], RIGHT);
    sh.two(45, r(R_NOISE_IN), (20, 12), (28, 12));
    sh.run(&[(28, 12), (30, 12), (30, 6)]);
    sh.run(&[(30, 6), (30, 4)]);

    // ---------------------------------------------------- CUTOFF -> bias
    // Straight across the rail; the wiper is the control node both bias
    // resistors hang on.
    let cp = sh.part(ID_CUTOFF, pot(POT_CUTOFF, CUTOFF_WIPER), (40, RAIL_Y + 4), N, 4, true);
    debug_assert_eq!(cp[1], (42, RAIL_Y + 2));
    debug_assert_eq!(cp[2], (40, RAIL_Y));
    sh.ground(cp[0], DOWN);
    let cv = (42, RAIL_Y + 2);
    sh.two(51, r(R_CUT_BIAS), cv, (42, -8));
    sh.run(&[(42, -8), cut_bias()]);

    // ------------------------------------------------------ filter core
    // Two transconductance integrators. Integrator 1 sees (IN − Y), which is
    // the state-variable loop; integrator 2 turns X into Y. Each OTA output
    // terminates on a capacitor inside the loop — an OTA has EXACTLY zero
    // output conductance in this model, and an unterminated one floats on
    // GMIN and runs away to megavolts without ever quarantining.
    // pins [in+, in-, out, bias]
    let i1 = sh.part(52, K::Ota, (42, 0), E, 4, false);
    debug_assert_eq!(i1[0], (42, -1));
    debug_assert_eq!(i1[1], (42, 1));
    debug_assert_eq!(i1[2], x_node());
    debug_assert_eq!(i1[3], (45, -2));
    sh.run(&[in_node(), (38, 0), (38, -1), i1[0]]);
    sh.run(&[i1[3], (45, -8), cut_bias()]);
    sh.two(53, cap(C_FILT), x_node(), (46, 6));
    sh.ground((46, 6), DOWN);

    let i2 = sh.part(54, K::Ota, (54, 0), E, 4, false);
    debug_assert_eq!(i2[0], (54, -1));
    debug_assert_eq!(i2[1], (54, 1));
    debug_assert_eq!(i2[2], y_node());
    debug_assert_eq!(i2[3], (57, -2));
    sh.run(&[x_node(), (50, 0), (50, -1), i2[0]]);
    sh.ground(i2[1], DOWN);
    sh.run(&[i2[3], (57, -8), (45, -8)]);
    sh.two(55, cap(C_FILT), y_node(), (58, 6));
    sh.ground((58, 6), DOWN);
    // Y back into integrator 1's INVERTING input: the state-variable loop.
    sh.run(&[y_node(), (62, 0), (62, 10), (36, 10), (36, 1), i1[1]]);

    // ---- the damping transconductor. in+ on ground, in− on Y, output on Y:
    // a voltage-controlled conductance across the output node, and the only
    // loss in the core. Its bias comes from the SAME control node through a
    // fixed resistor, so Q is a resistor ratio and holds while cutoff moves.
    let dmp = sh.part(56, K::Ota, (54, -18), E, 4, false);
    debug_assert_eq!(dmp[0], (54, -19));
    debug_assert_eq!(dmp[1], (54, -17));
    debug_assert_eq!(dmp[2], (58, -18));
    debug_assert_eq!(dmp[3], (57, -20));
    sh.ground(dmp[0], UP);
    sh.run(&[y_node(), (62, 0), (62, -17), (54, -17)]);
    sh.run(&[dmp[2], (62, -18), (62, -17)]);
    sh.two(57, r(R_DAMP_BIAS), (52, RAIL_Y + 2), (52, -20));
    sh.run(&[cv, (42, RAIL_Y + 2), (52, RAIL_Y + 2)]);
    sh.run(&[(52, -20), dmp[3]]);

    // ------------------------------------- gain stage: millivolts -> volts
    // Non-inverting, `1 + RF/RG` ≈ 197. This is the part that makes the
    // diodes able to conduct at all, and it is also the output amplifier.
    // pins [in+, in-, out]
    let g = sh.part(70, K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC }, (72, 0), E, 6, false);
    debug_assert_eq!(g[0], (72, -1));
    debug_assert_eq!(g[1], (72, 1));
    debug_assert_eq!(g[2], amp_out());
    sh.run(&[(62, 0), (66, 0), (66, -1), g[0]]);
    // THE DIVIDER JUNCTION IS THE INVERTING INPUT, not the output. Running
    // the output straight to in− and hanging RF/RG off that node makes a
    // unity follower with two decorative resistors: measured gain 1.00
    // against the 197 on the label, and a room too quiet to hear.
    sh.run(&[amp_out(), (78, 4)]);
    sh.two(72, r(R_F_GAIN), (78, 4), (70, 4));
    sh.run(&[(70, 4), (70, 1), g[1]]);
    sh.two(73, r(R_G_GAIN), (70, 4), (70, 8));
    sh.ground((70, 8), DOWN);

    // ---------------------------------------- PEAK, the diodes, the loop
    // The MS-20's resonance is a feedback path round the core with two anti-
    // parallel diodes across it, NOT a damping control inside the core —
    // which is why its passband survives resonance where a ladder's does not.
    // pins [end a, wiper, end b]
    // THE SIGNAL END IS `end b`, NOT `end a`. A pot's wiper sits `w` of the
    // way from end a, so a divider with the signal on end a delivers
    // `v·(1 − w)` and the knob runs backwards: measured, PEAK 0.05 gave the
    // corner +7.3 dB and PEAK 0.95 gave it +1.5 dB, which is a resonance
    // control that removes resonance.
    let pk = sh.part(ID_PEAK, pot(POT_PEAK, PEAK_WIPER), (78, 10), E, 4, false);
    debug_assert_eq!(pk[1], (80, 8));
    debug_assert_eq!(pk[2], (82, 10));
    sh.ground(pk[0], DOWN);
    sh.run(&[amp_out(), (84, 0), (84, 10), (82, 10)]);
    sh.run(&[pk[1], (80, 14), res_node()]);
    // THE TWO DIODES, anti-parallel, from the resonance node to ground.
    // Anode is pin 0, so these point opposite ways and clamp both polarities.
    // How MUCH they clamp is measured as a ratio, not asserted as a forward
    // voltage — see `ms20_diodes_clip_only_when_the_peak_is_up`.
    sh.two(74, K::Diode, res_node(), (74, 20));
    sh.two(75, K::Diode, (78, 20), (78, 14));
    sh.run(&[res_node(), (78, 14)]);
    sh.run(&[(74, 20), (78, 20)]);
    sh.ground((74, 20), DOWN);
    // ...and back into the filter's output node.
    sh.two(76, r(R_FEEDBACK), (74, 14), (68, 14));
    sh.run(&[(68, 14), (62, 14), (62, 10)]);

    // ------------------------------------------------- output + speaker
    // The gain stage into a virtual-earth mixer, trimmed against the op-amp's
    // real 0.2 V ceiling into 8 Ω.
    sh.two(77, r(R_OUT), (78, -4), (86, -4));
    sh.run(&[amp_out(), (78, -4)]);
    let (sum, out) = modules::mixer_speaker(&mut sh, 80, ID_SPEAKER, (88, -4), SUPPLY_V, RF_MIX);
    debug_assert_eq!(out, out_node());
    sh.run(&[(86, -4), (86, -5), sum]);

    let mut els = sh.finish();
    name_controls(&mut els);
    els
}

/// The front-panel legend on every part a player can touch. A panel row
/// reading POT #405 is the bug this feature fixed.
fn name_controls(els: &mut [ElementSpec]) {
    let named: &[(u32, &str)] = &[
        (ID_SUPPLY, "SUPPLY"),
        (ID_PITCH, "PITCH"),
        (ID_DRIVE, "DRIVE"),
        (ID_NOISE, "NOISE"),
        (ID_CUTOFF, "CUTOFF"),
        (ID_PEAK, "PEAK"),
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
pub fn ms20_panels() -> Vec<crate::synth::PanelDef> {
    vec![crate::synth::PanelDef {
        x0: -28.0,
        y0: -32.0,
        x1: 112.0,
        y1: 24.0,
        name: "THE SCREAM",
    }]
}

/// Block headings, plus the honesty plaque. No two may overlap — see
/// `roombench::no_instrument_room_overlaps_its_own_label_boxes`.
pub fn ms20_label_boxes() -> Vec<crate::synth::PanelDef> {
    use crate::synth::PanelDef;
    let b = |x0: f64, y0: f64, x1: f64, y1: f64, name: &'static str| PanelDef { x0, y0, x1, y1, name };
    vec![
        b(-23.0, -20.5, 15.0, 5.0, "VCO  1V/OCT  PITCH"),
        b(16.0, 10.0, 29.0, 19.0, "NOISE"),
        b(23.0, -15.0, 35.0, 6.0, "INPUT MIX  DRIVE"),
        b(38.0, -28.0, 48.0, -6.0, "CUTOFF"),
        b(40.0, -5.0, 63.0, 11.5, "2-POLE OTA-C CORE"),
        b(50.0, -22.0, 63.0, -12.0, "DAMPING  Q"),
        b(64.0, -3.0, 85.0, 9.0, "GAIN  X197"),
        b(64.0, 12.0, 85.0, 22.0, "PEAK  DIODE LIMITER"),
        b(86.0, -10.0, 104.0, -0.5, "MIXER + SPEAKER"),
        // The plaque. 28 characters a line is what a label box holds.
        b(114.0, -30.0, 146.0, -28.4, "AFTER THE KORG MS-20. TWO"),
        b(114.0, -28.0, 146.0, -26.4, "POLES IS WHAT AN MS-20"),
        b(114.0, -26.0, 146.0, -24.4, "FILTER IS, NOT A COMPROMISE"),
        b(114.0, -24.0, 146.0, -22.4, "- MEASURED -12.2 DB/OCTAVE."),
        b(114.0, -22.0, 146.0, -20.4, "THE LADDER ROOM IS THE 24."),
        b(114.0, -19.0, 146.0, -17.4, "THE TWO DIODES ARE REAL AND"),
        b(114.0, -17.0, 146.0, -15.4, "THEY ARE THE POINT. INERT"),
        b(114.0, -15.0, 146.0, -13.4, "AT PEAK 0.05 (1.0X), THEY"),
        b(114.0, -13.0, 146.0, -11.4, "TAKE 6.6X OFF THE LOOP AT"),
        b(114.0, -11.0, 146.0, -9.4, "0.95. THAT IS THE SCREAM."),
        b(114.0, -8.0, 146.0, -6.4, "RESONANCE IS A LOOP ROUND"),
        b(114.0, -6.0, 146.0, -4.4, "THE CORE, AS KORG'S IS, SO"),
        b(114.0, -4.0, 146.0, -2.4, "THE PASSBAND HOLDS: 0.05 DB"),
        b(114.0, -2.0, 146.0, -0.4, "OF MOVEMENT DOWN LOW WHILE"),
        b(114.0, 0.0, 146.0, 1.6, "THE CORNER GAINS 9.4 DB. A"),
        b(114.0, 2.0, 146.0, 3.6, "LADDER LOSES ITS BASS TO"),
        b(114.0, 4.0, 146.0, 5.6, "RESONANCE; THIS DOES NOT."),
        b(114.0, 7.0, 146.0, 8.6, "STAND-IN: THE CORE IS A"),
        b(114.0, 9.0, 146.0, 10.6, "STATE-VARIABLE OTA PAIR,"),
        b(114.0, 11.0, 146.0, 12.6, "NOT A SALLEN-KEY ON"),
        b(114.0, 13.0, 146.0, 14.6, "TRANSISTOR VCRS. SAME TWO"),
        b(114.0, 15.0, 146.0, 16.6, "POLES, SAME CURRENT LAW,"),
        b(114.0, 17.0, 146.0, 18.6, "BUT IT IS NOT A KORG35."),
        b(114.0, 20.0, 146.0, 21.6, "THE VCO IS THIS PROJECT'S"),
        b(114.0, 22.0, 146.0, 23.6, "OWN, NOT AN MS-20 VCO, AND"),
        b(114.0, 24.0, 146.0, 25.6, "THERE IS NO ENVELOPE OR"),
        b(114.0, 26.0, 146.0, 27.6, "VCA. THIS ROOM IS THE"),
        b(114.0, 28.0, 146.0, 29.6, "FILTER, PLAYED BY HAND."),
    ]
}

// ------------------------------------------------------------ SCOPE NOTES
//
// Measured on this machine (Apple M4, release, pinned 1.95.0) with other
// agents building in parallel — best of three passes of 150 000 substeps,
// stepped the way the server steps, after settling.
//
//   SHIPPED: 138 elements — 37 DEVICES plus the wires and ground symbols of
//   routing. 6.23 µs/substep against the 20 µs budget = 3.21x real time
//   offline, NR 2.18, 27 unknowns. Live over a websocket: rt 0.998.
//
//   ---- WHAT THE TWO DIODES COST, AND WHY THEY WERE AFFORDABLE ----
//
//   NR 2.18 is the highest of the five instrument rooms, and it is the
//   diodes and the three OTAs: smooth-nonlinear parts refactor the whole
//   island on every Newton pass. It is still only 6.2 µs because the room is
//   SMALL — 27 unknowns — and because everything else expensive was kept
//   out. The gain stage and the mixer are op-amps, which are discrete-
//   nonlinear and cost Newton nothing; that is what paid for the diodes.
//   There is deliberately no VCA and no envelope generator here.
//
//   ---- FOUR BUGS, AND ONLY ONE OF THEM LOOKED LIKE A BUG ----
//
//   1. THE VCO HAD NO SUPPLY. `sh.run` walks a polyline of `Wire`s and a
//      `Wire` merges its two ENDPOINTS — nothing in between. `modules::vco`
//      taps the rail at `a.0+2` and `a.0+20`, and the rail run listed only
//      −22 and 40, so both taps landed mid-segment and were not connected.
//      The room did not fail: the comparator simply never flipped, the
//      triangle carried nothing but the 5 mV of dither, and the filter sat
//      there filtering silence. Every tap must be a VERTEX of the run.
//
//   2. THE GAIN STAGE WAS A FOLLOWER. The output ran straight to `in−` and
//      RF/RG hung off that node, which is a unity buffer with two decorative
//      resistors: measured gain 1.00 against the 197 on the label. The
//      divider junction has to BE the inverting input. Symptom was a room
//      too quiet to hear, not a wrong number anywhere.
//
//   3. PEAK RAN BACKWARDS. A pot's wiper sits `w` of the way from end a, so
//      a divider with the signal on end a delivers `v·(1−w)`. Measured: PEAK
//      0.05 gave the corner +7.3 dB and PEAK 0.95 gave it +1.5 dB — a
//      resonance control that removed resonance. The signal belongs on end b.
//
//   4. THE SLOPE MEASUREMENT WAS TAKEN ON THE SHOULDER. 800→3200 Hz with the
//      corner at 955 Hz reads −13.9 dB/octave, which is the resonant
//      shoulder still falling away, not the asymptote. Dropping CUTOFF to
//      0.10 (corner 79 Hz) and reading 500→2000 gives −12.2 and −12.1. A
//      two-pole claim has to be measured where two poles are all there is.
//
//   ---- THE HONESTY TEST IS THE ONE THAT MATTERS ----
//
//   `ms20_diodes_clip_only_when_the_peak_is_up` is the load-bearing test in
//   this file. The room's whole claim is that it has the MS-20's diode
//   limiter, and two diodes that never leave their linear region would make
//   that a lie one level up — the exact sin this project exists not to
//   commit. The test states it as a RATIO (how far the resonance node falls
//   short of what the PEAK divider alone would put there) so it needs no
//   guess about the model's forward voltage: 1.0x at PEAK 0.05 means inert,
//   3.0x at PEAK 0.95 on the bench and 6.6x in the room as it ships means
//   biting. If either end stops holding, the plaque comes down with it.
//
//   ---- WHAT WAS NOT BUILT ----
//
//   No Sallen-Key core. The Korg35 makes its two poles from transistor
//   voltage-controlled resistors in a Sallen-Key; those are not parts here,
//   and a discrete build of them would be smooth-nonlinear and expensive.
//   Two OTAs round two capacitors give the same two poles and the same
//   `f0 = gm/2πC` current law. The plaque says so in as many words.

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Engine;

    const DT: f64 = 20e-6;
    const TAU: f64 = std::f64::consts::TAU;

    fn tweak(id: u32, wiper: f64, els: &mut [ElementSpec]) {
        for e in els.iter_mut() {
            if e.id == id {
                if let K::Potentiometer { wiper: w, .. } = &mut e.kind {
                    *w = wiper;
                }
            }
        }
    }

    /// The room with its VCO and noise replaced by a single clean sine at the
    /// filter's own input node — the only way to read a frequency response.
    /// Everything downstream of `in_node` is the shipped circuit untouched.
    fn bench(hz: f64, amp: f64, cut: f64, peak: f64) -> Vec<ElementSpec> {
        let mut els: Vec<ElementSpec> = ms20_room_circuit()
            .into_iter()
            // Drop the VCO block (20..=31), the noise and its level pot, and
            // the two input resistors that feed them in.
            .filter(|e| !(20..=31).contains(&e.id))
            .filter(|e| ![40, 42, 45, ID_NOISE].contains(&e.id))
            .collect();
        els.push(ElementSpec {
            id: 500,
            kind: K::VoltageSource {
                dc: 0.0,
                amp,
                hz,
                phase: 0.0,
                wave: sim_core::Wave::Sine,
            },
            pins: vec![(24, 0), (24, 4)],
            ..Default::default()
        });
        els.push(ElementSpec {
            id: 501,
            kind: K::Ground,
            pins: vec![(24, 4)],
            ..Default::default()
        });
        els.push(ElementSpec {
            id: 502,
            kind: K::Resistor { ohms: R_DRIVE_IN },
            pins: vec![(24, 0), (30, 0)],
            ..Default::default()
        });
        tweak(ID_CUTOFF, cut, &mut els);
        tweak(ID_PEAK, peak, &mut els);
        els
    }

    /// Amplitude at `p` on the drive frequency, by coherent detection — it
    /// rejects the noise and the harmonics the diodes make, which a
    /// peak-to-peak reading cannot.
    fn lockin(hz: f64, amp: f64, cut: f64, peak: f64, p: Point) -> f64 {
        let els = bench(hz, amp, cut, peak);
        let mut e = Engine::new(DT);
        e.set_elements(&els);
        e.advance((0.6 / DT) as u32);
        assert!(!e.is_quarantined(), "quarantined at {hz} Hz cut {cut} peak {peak}");
        let n = ((24.0 / hz / DT) as u32).max(512);
        let (mut si, mut co) = (0.0f64, 0.0f64);
        let t0 = e.time();
        for k in 0..n {
            e.advance(1);
            let t = t0 + (f64::from(k) + 1.0) * DT;
            let v = e.voltage_at(p).unwrap_or(0.0);
            si += v * (TAU * hz * t).sin();
            co += v * (TAU * hz * t).cos();
        }
        2.0 * (si * si + co * co).sqrt() / f64::from(n)
    }

    /// Peak excursion of a node over `secs`, after settling.
    fn swing(els: &[ElementSpec], p: Point, secs: f64) -> f64 {
        let mut e = Engine::new(DT);
        e.set_elements(els);
        e.advance((0.6 / DT) as u32);
        let mut peak = 0.0f64;
        for _ in 0..(secs / DT) as u32 {
            e.advance(1);
            peak = peak.max(e.voltage_at(p).unwrap_or(0.0).abs());
        }
        assert!(!e.is_quarantined(), "quarantined while swinging");
        peak
    }

    /// Every part a legal shape, every wire orthogonal, every id unique.
    #[test]
    fn ms20_room_is_a_legal_document() {
        let els = ms20_room_circuit();
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

    /// Every control a player can turn carries a legend, and the speaker sits
    /// at a low id because the server streams the four lowest.
    #[test]
    fn ms20_controls_are_named_and_reachable() {
        let els = ms20_room_circuit();
        for id in [ID_PITCH, ID_DRIVE, ID_NOISE, ID_CUTOFF, ID_PEAK] {
            let e = els.iter().find(|e| e.id == id).expect("control missing");
            assert!(!e.name.is_empty(), "control {id} has no name");
        }
        assert!(ID_SPEAKER <= 4, "speaker must be in the streamed low ids");
    }

    /// The room plays on arrival and never quarantines.
    #[test]
    fn ms20_room_never_quarantines_and_plays() {
        let els = ms20_room_circuit();
        let peak = swing(&els, out_node(), 0.4);
        assert!(peak > 0.02, "the room is silent: {peak:.4} V peak");
        assert!(peak < 0.2, "past the op-amp's 0.2 V wall: {peak:.4} V");
    }

    /// IT IS REALLY TWO POLES. 12 dB/octave is not a compromise in an MS-20
    /// room, it is what the instrument's filter IS — and the claim on the
    /// plaque has to be the solver's, not the designer's.
    #[test]
    fn ms20_rolls_off_at_twelve_db_per_octave() {
        // CUTOFF right down puts the corner at 79 Hz, so 500 Hz is already
        // 2.6 octaves up and what is left is the asymptote and nothing else.
        // Measured across 800→3200 with the corner at 955 Hz it reads −13.9,
        // which is the resonant shoulder, not the slope.
        let (cut, peak) = (0.10, 0.05);
        let a = lockin(500.0, 1.0, cut, peak, amp_out());
        let b = lockin(1000.0, 1.0, cut, peak, amp_out());
        let c = lockin(2000.0, 1.0, cut, peak, amp_out());
        let db = |x: f64, y: f64| 20.0 * (y / x).log10();
        let (o1, o2) = (db(a, b), db(b, c));
        for (n, o) in [("first", o1), ("second", o2)] {
            assert!(
                o < -10.5 && o > -13.5,
                "{n} octave is {o:.1} dB — that is not a two-pole slope, and \
                 the plaque claims one"
            );
        }
    }

    /// Where the response has fallen 3 dB from its own low-frequency value.
    fn corner_hz(cut: f64) -> f64 {
        let base = lockin(30.0, 1.0, cut, 0.05, amp_out());
        let mut hz = 30.0;
        while hz < 8000.0 {
            if lockin(hz, 1.0, cut, 0.05, amp_out()) < base * 0.7071 {
                return hz;
            }
            hz *= 1.12;
        }
        f64::NAN
    }

    /// CUTOFF moves the CORNER, and moves it a long way. Asserting on the
    /// gain at one fixed frequency is much weaker — it reads 2.7× across the
    /// whole knob purely because 600 Hz is on the skirt at both ends — so
    /// this finds the −3 dB point at each end and compares those.
    #[test]
    fn ms20_cutoff_moves_the_corner() {
        let (lo, hi) = (corner_hz(0.10), corner_hz(0.95));
        assert!(lo.is_finite() && hi.is_finite(), "no corner found: {lo} .. {hi}");
        assert!(
            hi > lo * 8.0,
            "CUTOFF barely moves the corner: {lo:.0} Hz closed vs {hi:.0} Hz open"
        );
    }

    /// PEAK actually resonates: the corner has to gain a lot of decibels.
    #[test]
    fn ms20_peak_lifts_the_corner() {
        let corner = 600.0;
        let flat = lockin(corner, 1.0, 0.55, 0.05, amp_out());
        let hot = lockin(corner, 1.0, 0.55, 0.95, amp_out());
        let db = 20.0 * (hot / flat).log10();
        assert!(db > 8.0, "PEAK only lifts the corner {db:.1} dB");
    }

    /// THE DIODES ARE THE POINT, AND THEY MUST ACTUALLY CLIP. This is the
    /// honesty test for the whole room: a plaque that claims a diode limiter
    /// over two parts that never leave their linear region would be a lie
    /// one level up, which is the sin this project exists not to commit.
    ///
    /// At low PEAK the resonance node must stay well below a diode drop; at
    /// high PEAK it must be pinned at one.
    #[test]
    fn ms20_diodes_clip_only_when_the_peak_is_up() {
        // How far the resonance node falls short of what the PEAK divider
        // ALONE would put there. 1.0 means the diodes are not conducting;
        // anything above means they are taking the top off the loop. Stating
        // it as a ratio needs no guess about the model's forward voltage.
        let clamp = |peak: f64| {
            let els = bench(600.0, 1.0, 0.55, peak);
            let amp = swing(&els, amp_out(), 0.15);
            let node = swing(&els, res_node(), 0.15);
            amp * peak / node
        };
        let inert = clamp(0.05);
        let biting = clamp(0.95);
        assert!(
            inert < 1.15,
            "the diodes are already clipping at PEAK 0.05 ({inert:.2}x) — \
             then the filter never has a clean setting"
        );
        assert!(
            biting > 2.5,
            "the diodes barely bite at PEAK 0.95 ({biting:.2}x) — the room \
             claims a diode limiter and would be two decorative parts"
        );
    }

    /// It SINGS. With the input shut off entirely, a filter whose resonance
    /// loop has cancelled its own damping goes on ringing by itself — the
    /// MS-20's party trick, and the thing the diodes are there to stop from
    /// running away.
    #[test]
    fn ms20_sings_with_nothing_going_in() {
        let quiet = |peak: f64| {
            let mut els = ms20_room_circuit();
            tweak(ID_PEAK, peak, &mut els);
            tweak(ID_DRIVE, 0.001, &mut els);
            tweak(ID_NOISE, 0.001, &mut els);
            swing(&els, amp_out(), 0.3)
        };
        let (shut, open) = (quiet(0.10), quiet(0.99));
        assert!(
            open > shut * 5.0,
            "no self-oscillation: {shut:.4} V at PEAK 0.10 vs {open:.4} V wide open"
        );
    }

    /// THE PASSBAND SURVIVES RESONANCE, which is the honest difference
    /// between this room and the ladder. Korg's resonance is a loop around
    /// the core, so the bass does not thin out as PEAK comes up; a Moog
    /// ladder's does, by construction. If this ever stops holding, the
    /// plaque's comparison has to come down with it.
    #[test]
    fn ms20_passband_survives_resonance() {
        let low = 60.0;
        let flat = lockin(low, 1.0, 0.55, 0.05, amp_out());
        let hot = lockin(low, 1.0, 0.55, 0.95, amp_out());
        let db = 20.0 * (hot / flat).log10();
        assert!(
            db.abs() < 3.0,
            "the passband moved {db:.1} dB with resonance — that is ladder behaviour"
        );
    }

    /// Every knob, everywhere, without the solver falling over. The damage
    /// gate winds pots to their ends, so the room has to survive there.
    #[test]
    fn ms20_survives_every_knob_position() {
        for cw in [0.01, 0.5, 0.99] {
            for pw in [0.01, 0.5, 0.99] {
                for dw in [0.01, 0.99] {
                    let mut els = ms20_room_circuit();
                    tweak(ID_CUTOFF, cw, &mut els);
                    tweak(ID_PEAK, pw, &mut els);
                    tweak(ID_DRIVE, dw, &mut els);
                    tweak(ID_NOISE, dw, &mut els);
                    let mut e = Engine::new(DT);
                    e.set_elements(&els);
                    for _ in 0..100 {
                        e.advance(500);
                        assert!(
                            !e.is_quarantined(),
                            "quarantined at CUTOFF {cw} PEAK {pw} DRIVE/NOISE {dw}"
                        );
                    }
                }
            }
        }
    }

    /// The measurements that write the scope notes.
    #[test]
    #[ignore = "measurement: cargo test --release -p server ms20_response -- --ignored --nocapture"]
    fn ms20_response() {
        println!("\n== THE SCREAM: corner against CUTOFF (PEAK 0.10) ==");
        for cw in [0.10, 0.30, 0.55, 0.80, 0.95] {
            // Find where the response has fallen 3 dB from its own floor.
            let base = lockin(40.0, 1.0, cw, 0.10, amp_out());
            let mut corner = 0.0;
            let mut hz = 40.0;
            while hz < 6000.0 {
                let a = lockin(hz, 1.0, cw, 0.10, amp_out());
                if a < base * 0.7071 {
                    corner = hz;
                    break;
                }
                hz *= 1.12;
            }
            println!("  CUTOFF {cw:.2} | -3 dB at {corner:>7.1} Hz");
        }

        // WELL above the corner, and with the resonance down. Measured across
        // 800->3200 with the corner at 955 Hz it read −13.9 dB/octave, which
        // is not the slope — it is the resonant shoulder still falling away.
        // CUTOFF 0.10 puts the corner at 79 Hz, so 500 Hz is already 2.6
        // octaves up and the asymptote is the only thing left.
        println!("\n== slope, 2.6 to 4.6 octaves above the corner (CUTOFF 0.10, PEAK 0.05) ==");
        let a = lockin(500.0, 1.0, 0.10, 0.05, amp_out());
        let b = lockin(1000.0, 1.0, 0.10, 0.05, amp_out());
        let c = lockin(2000.0, 1.0, 0.10, 0.05, amp_out());
        println!(
            "  500->1000 {:.1} dB/oct | 1000->2000 {:.1} dB/oct",
            20.0 * (b / a).log10(),
            20.0 * (c / b).log10()
        );

        println!("\n== PEAK: the corner, the passband, and whether the diodes bite ==");
        println!("  (linear = what the PEAK divider alone would put on the node)");
        for pw in [0.05, 0.30, 0.55, 0.80, 0.95] {
            let corner = lockin(600.0, 1.0, 0.55, pw, amp_out());
            let pass = lockin(60.0, 1.0, 0.55, pw, amp_out());
            let amp = swing(&bench(600.0, 1.0, 0.55, pw), amp_out(), 0.15);
            let node = swing(&bench(600.0, 1.0, 0.55, pw), res_node(), 0.15);
            let linear = amp * pw;
            println!(
                "  PEAK {pw:.2} | corner {corner:>7.4} V | passband {pass:>7.4} V | res {node:>5.3} V vs linear {linear:>5.3} V = {:>4.1}x clamp",
                linear / node
            );
        }

        println!("\n== the room as it ships (VCO driving it, not a bench sine) ==");
        for pw in [0.10, 0.55, 0.95] {
            let mut els = ms20_room_circuit();
            tweak(ID_PEAK, pw, &mut els);
            let amp = swing(&els, amp_out(), 0.3);
            let node = swing(&els, res_node(), 0.3);
            let out = swing(&els, out_node(), 0.3);
            println!(
                "  PEAK {pw:.2} | amp_out {amp:>6.3} V | res {node:>5.3} V vs linear {:>5.3} V = {:>4.1}x | speaker {out:.4} V",
                amp * pw,
                amp * pw / node
            );
        }

        println!("\n== self-oscillation: input shut off, PEAK wide open ==");
        for pw in [0.10, 0.80, 0.95, 0.99] {
            let mut els = ms20_room_circuit();
            tweak(ID_PEAK, pw, &mut els);
            // DRIVE right down shunts the oscillator and the noise away.
            tweak(ID_DRIVE, 0.001, &mut els);
            tweak(ID_NOISE, 0.001, &mut els);
            let amp = swing(&els, amp_out(), 0.3);
            println!("  PEAK {pw:.2} | amp_out {amp:>7.4} V with nothing going in");
        }
    }
}
