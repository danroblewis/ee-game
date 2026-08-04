//! THE RHYTHM COMPOSER — after the Roland TR-808 (1980).
//!
//! The historical instrument: Roland's TR-808, whose analog voices — a
//! bridged-T resonator kick, a noise "snappy" snare over a resonant shell, a
//! twin-square-oscillator cowbell — and whose CMOS step sequencer defined
//! several decades of music. Primary sources for the topologies: Werner et
//! al.'s DAFX-14 papers on the 808 bass drum and the cowbell, and the Roland
//! service notes.
//!
//! THE ONE IDEA THIS ROOM IS BUILT ON. The 808's signature voices are
//! BRIDGED-T RESONATORS: a passive network of two capacitors and two
//! resistors wrapped round one amplifier, banged with a charge and left to
//! ring. In this engine that is the cheapest interesting thing there is — an
//! `OpAmp` is a DISCRETE nonlinearity, so it buys no Newton iterations, and
//! everything else in the loop is linear. Four of this machine's five voices
//! are the same nine-element block at four sets of values, and their ring
//! frequencies are CONTINUOUS: a capacitor-timed resonator sits on no substep
//! grid, unlike every comparator oscillator in the other rooms.
//!
//! What is faithful and what is a stand-in (the sheet says so too):
//!
//!   * FAITHFUL — the kick IS a bridged-T in an op-amp's feedback loop,
//!     pinged through a coupling capacitor, with a TONE rheostat and shunt
//!     cap after it. That is the real BD. Measured: 51.7 Hz, ~0.93 V peak,
//!     ringing down over about a fifth of a second.
//!   * FAITHFUL — the snare is the 808's whole architecture: TWO bridged-T
//!     shells about an octave apart (measured 170 Hz and 306 Hz), both pinged
//!     by the same trigger, plus white noise high-passed and gated by its own
//!     decaying envelope, with SNAPPY setting how much noise rides over the
//!     shells. STAND-IN: the snappy VCA is an `Nmos` used as a
//!     voltage-controlled resistor where Roland used a single BJT. Same job,
//!     and the MOSFET is the measured-cheap one here (`synth.rs` fact 4: an
//!     OTA that can see noise costs the whole room half a Newton iteration).
//!   * FAITHFUL — the claves are one more bridged-T, a hundredth of the
//!     kick's capacitance, ringing at 2.7 kHz for about two milliseconds. No
//!     envelope, no VCA: the Q of the network IS the decay, which is what the
//!     808's CL is.
//!   * STAND-IN — the 16-step CD4017 pattern chain is a FOUR-step binary
//!     `Counter` plus one `Mux` per voice, on its own regulated 5 V logic
//!     rail (`LOGIC_V_ABSMAX` is 7 V; logic on a 9 V rail latches up). Four
//!     and not sixteen because this engine's `Mux` addresses at most four
//!     channels, so sixteen steps would need a three-deep tree per voice —
//!     nine multiplexers and 48 more nodes, against a cost that goes as
//!     `unknowns^1.64`. The trick that makes even four steps cheap is that
//!     the Mux passes ANALOG both ways: each pattern switch simply connects
//!     the clock pulse to a channel, so the "step AND pattern bit" the 808
//!     did with gates falls out of the pass transistor for free, and there is
//!     not one steering diode in the machine.
//!   * ABSENT — the cowbell, and it is a budget cut, stated plainly. The CB
//!     is two square-wave oscillators at 540 and 800 Hz beating together;
//!     built here out of two more 555s and a band-pass it measured (and it
//!     did work: 540 and 800 Hz exactly) at 14 of the room's 62 unknowns and
//!     pushed the room to 0.83x real time — which for an instrument is not
//!     "a bit slow", it is a semitone and a half FLAT. The second snare shell
//!     was worth more than the bell.
//!   * ABSENT — the hi-hat and the cymbal, and that one is physics. The audio
//!     tap's Nyquist is 6.25 kHz and a real 808 hat lives at 8–12 kHz; band
//!     limited to fit, it measures indistinguishable from the snare (the
//!     `drums.rs` measurement). Shipping a "hi-hat" that is really a second
//!     snare would be a lie of exactly the kind this project does not tell.
//!
//! ## Signal flow
//!
//! ```text
//!   TEMPO 555 ──> CLK ──> Counter ── Q0,Q1 ─┬─> BD Mux ─> BD trigger bus
//!        │                                  ├─> SD Mux ─> SD trigger bus
//!        └── the pulse itself, through ──────┴─> CL Mux ─> CL trigger bus
//!            each row's pattern switches
//!
//!   BD bus ─ cap ─> bridged-T 52 Hz ─> TONE ─────────────────┐
//!   SD bus ─┬ cap ─> shell 1 bridged-T 170 Hz ──────────────>┤
//!           ├ cap ─> shell 2 bridged-T 306 Hz ──────────────>├─> virtual-earth
//!           └ diode ─> env ─ SNAPPY ─> gate                  │    mixer ─> 8 Ω
//!   noise ─ AA ─ HP ─> NMOS VCA ────────────────────────────>┤
//!   CL bus ─ cap ─> bridged-T 2.7 kHz ──────────────────────>┘
//! ```
//!
//! SCOPE NOTES (measured on this machine, Apple M4, release, pinned 1.95.0,
//! other agents building in parallel — method as `synth.rs`) are at the
//! bottom of this file.

use sim_core::{ElementKind as K, ElementSpec, Point};

use crate::layout::{Sheet, DOWN, E, LEFT, RIGHT, S, W};

// ---------------------------------------------------------------- values

/// The op-amps' clamp rail. There is no 9 V SOURCE anywhere in this room and
/// that is not an omission: op-amps are supply-less in this engine, the MOSFET
/// VCA is passive, and every resonator is a passive network round an op-amp.
/// The only thing here that needs a rail is the logic.
const SUPPLY_V: f64 = 9.0;
/// The logic rail. `LOGIC_V_ABSMAX` is 7 V and a chip on a 9 V rail latches
/// up, so the sequencer gets its own regulated rail — exactly as the 808's
/// logic board had its own supply separate from the voice board.
const LOGIC_V: f64 = 5.0;

// -- clock. A 555 astable with a diode across the timing rheostat so the
// CHARGE current bypasses it: HIGH = 0.693·RA·C ≈ 2.3 ms, a trigger rather
// than a gate; LOW = 0.693·(R_MIN + TEMPO)·C is the step.
const CLK_RA: f64 = 3.3e3;
const CLK_R_MIN: f64 = 22e3;
const CLK_POT: f64 = 1e6;
const CLK_C: f64 = 1e-6;
/// Default tempo: ~4 steps a second, so the four-step bar is about a second.
pub const TEMPO_WIPER: f64 = 0.33;

// -- kick. Bridged-T with C1 = C2 = C:
//     f0 = 1 / (2π·C·√(R_leg·R_bridge)),  Q = √(R_bridge/R_leg) / 2.
// 100 n, 1 k, 1 M puts f0 at 50.3 Hz with Q ≈ 15.8 — an 808 kick.
const BD_C: f64 = 100e-9;
const BD_R_LEG: f64 = 1e3;
const BD_R_BRIDGE: f64 = 1e6;
/// The mallet: the charge `C·ΔV` this cap delivers into the op-amp's virtual
/// ground is the whole excitation.
const BD_C_TRIG: f64 = 1e-9;
/// TONE — a series rheostat into a fixed shunt cap, which is what the 808's
/// BD TONE pot was: it winds the attack click off the top of the ring.
const BD_TONE_POT: f64 = 47e3;
const BD_TONE_C: f64 = 100e-9;
pub const BD_TONE_WIPER: f64 = 0.30;
const BD_R_MIX: f64 = 1.2e6;

// -- snare shells. TWO of them, as the 808 has: one drum head and the other
// about an octave up, both pinged by the same trigger. 170 Hz and 306 Hz
// here, Q ≈ 17 each.
const SD_C: f64 = 27e-9;
const SD_C2: f64 = 15e-9;
const SD_R_LEG: f64 = 1e3;
const SD_R_BRIDGE: f64 = 1.2e6;
const SD_C_TRIG: f64 = 100e-12;
const SD_C2_TRIG: f64 = 100e-12;
const SD_R_MIX: f64 = 2.2e6;
const SD_R_MIX2: f64 = 2.7e6;
/// Noise front end. The source's own 1 kΩ against 33 n is a 4.8 kHz
/// anti-alias pole — the `drums.rs` measurement: without it most of the hiss
/// folds back through the audio tap and stops sounding like a snare.
const SD_C_AA: f64 = 33e-9;
/// High-pass into the VCA, then a divider that keeps the drain swing small
/// enough for the MOSFET to behave as a resistor rather than a distorter.
const SD_C_HP: f64 = 2.2e-9;
const SD_R_SER: f64 = 220e3;
const SD_R_SHUNT: f64 = 22e3;
/// Envelope: diode into a storage cap, with SNAPPY the pot ACROSS it whose
/// wiper drives the gate — depth and decay in one part, which is what the
/// 808's SNAPPY control actually set.
const SD_C_ENV: f64 = 33e-9;
const SD_POT_SNAPPY: f64 = 2.2e6;
pub const SD_SNAPPY_WIPER: f64 = 0.80;

// -- claves. The same bridged-T, a hundredth of the capacitance: 2.7 kHz,
// Q ≈ 19, so the ring is over in about 2 ms. Comfortably under the audio
// tap's 6.25 kHz Nyquist, which the hi-hat is not.
const CL_C: f64 = 1.5e-9;
const CL_R_LEG: f64 = 1e3;
const CL_R_BRIDGE: f64 = 1.5e6;
const CL_C_TRIG: f64 = 15e-12;
const CL_R_MIX: f64 = 3.3e6;

/// Both VCAs' transconductance coefficient — the measured `synth.rs` value.
/// `Rds ≈ 1/(k·(Vgs − Vt))`, so the envelope on the gate is the gain.
const NMOS_K: f64 = 5e-5;
/// Mixer feedback. Every voice's mix resistor is trimmed against this.
///
/// THE CEILING, and it is the op-amp's and not a taste: `DEFAULT_OPAMP_ISC`
/// is 25 mA and the speaker is 8 Ω, so an op-amp output stage in this engine
/// physically cannot put more than 0.2 V across the coil — a 741 cannot drive
/// 8 Ω either. Past that the output stops being a voltage and becomes a
/// current clamp, and a 50 Hz sine comes out a square. So every voice is
/// trimmed so the WORST coincidence (kick + shell + snap on step 3) lands
/// near 0.15 V and the mixer never leaves its linear region.
const R_F: f64 = 100e3;
/// Series resistance between the snare VCA's source and the summing node. This is
/// what makes a MOSFET usable as a VCA against a 100 k transimpedance: the
/// leg's current is `V_drain / (R_ser + Rds)`, so `R_ser` sets the wide-open
/// gain while `Rds` — 5 kΩ with the envelope up, 100 MΩ with it down — still
/// does all the gating, ~46 dB of it. Without it the leg's gain would be
/// `Rf/Rds` ≈ 20 and the mixer would clip on every hit. It also keeps `Vds`
/// two hundred times smaller than the signal, which is what keeps the MOSFET
/// in its ohmic region and therefore a resistor rather than a distorter.
const SD_R_VCA: f64 = 470e3;
/// Pattern-node pulldown: a step whose switch is open must be a DEFINED node
/// at zero, not a GMIN float.
const R_STEP_PD: f64 = 100e3;

// ------------------------------------------------------------------- ids

pub const ID_SPEAKER: u32 = 1;
pub const ID_LOGIC_RAIL: u32 = 3;
pub const ID_KICK_TONE: u32 = 26;
pub const ID_SNAPPY: u32 = 48;
pub const ID_TEMPO: u32 = 83;
pub const ID_COUNTER: u32 = 90;
/// Pattern switches: `ID_ROW[v] + k` is step `k` of voice row `v`.
pub const ID_ROW: [u32; 3] = [140, 150, 160];

/// The shipped pattern, one bar of four: kick on 1 and 3, snare on 3, bell on
/// the last. Four steps is the whole bar — see the module note.
pub const PATTERN: [[bool; 4]; 3] = [
    [true, false, true, false],  // BD
    [false, false, true, false], // SD
    [false, false, false, true], // CB
];

/// Voice names, in row order, for the panel legend.
const ROW_NAMES: [&str; 3] = ["BD", "SD", "CL"];

// -------------------------------------------------------------- geometry
//
// Three voice bands, 40 units apart. Each band reads strictly left to right:
// the four pattern switches, that voice's multiplexer, the voice itself, then
// the mixer column all three converge on.

const YB: [i32; 3] = [0, 40, 80];
/// Step column x positions.
const X_STEP: [i32; 4] = [-8, 1, 10, 19];
/// The multiplexers' left edge.
const X_MUX: i32 = 28;
/// Where a voice block begins.
const X_V: i32 = 44;
/// The mixer's virtual-ground summing node and its output.
const SUM: Point = (104, 44);
const OUT: Point = (110, 45);

fn cap(farads: f64) -> K {
    K::Capacitor { farads }
}
fn r(ohms: f64) -> K {
    K::Resistor { ohms }
}
fn pot(ohms: f64, wiper: f64) -> K {
    K::Potentiometer { ohms, wiper }
}
fn opamp() -> K {
    K::OpAmp {
        rail: SUPPLY_V,
        isc: sim_core::DEFAULT_OPAMP_ISC,
    }
}
fn nmos() -> K {
    K::Nmos { vt: 1.0, k: NMOS_K }
}

/// The kick resonator's op-amp output — the boom itself, before TONE.
pub fn kick_node() -> Point {
    (54, 4)
}
/// The snare's lower shell (170 Hz).
pub fn shell_node() -> Point {
    (54, 44)
}
/// The snare's upper shell (306 Hz).
pub fn shell2_node() -> Point {
    (54, 26)
}
/// The claves resonator's op-amp output.
pub fn clave_node() -> Point {
    (54, 84)
}
/// The mixer output the speaker hangs on.
pub fn out_node() -> Point {
    OUT
}
/// Voice `v`'s trigger bus — the multiplexer output.
pub fn trigger_node(v: usize) -> Point {
    (34, YB[v] + 3)
}

/// One bridged-T resonator in an op-amp's feedback loop — the 808 voice
/// skeleton, drawn with the op-amp anchored at `t` facing east and MIRRORED
/// so the inverting input is the upper of the two.
///
/// ```text
///          R_bridge
///      ┌──────────────────┐
///   ───┤ IN−            OUT├──> ring
///      │  ┌── C ── MID ── C ┘
///      └──┘        │
///                 R_leg
///                  ▽
/// ```
/// DC-stable through the bridge, resonant at
/// `f0 = 1/(2π·C·√(R_leg·R_bridge))` with `Q = √(R_bridge/R_leg)/2`.
/// Returns `(in-, out)`; the caller pings `in-` through a coupling cap.
fn bridged_t(sh: &mut Sheet, id0: u32, t: Point, c: f64, r_leg: f64, r_bridge: f64) -> (Point, Point) {
    // pins [in+, in-, out]; mirrored, so in- is above in+.
    let op = sh.part(id0, opamp(), t, E, 4, true);
    let (plus, minus, out) = (op[0], op[1], op[2]);
    debug_assert_eq!(minus, (t.0, t.1 - 1));
    debug_assert_eq!(out, (t.0 + 4, t.1));
    sh.ground(plus, DOWN);
    // The bridge, over the top of the amplifier.
    sh.wire(minus, (t.0, t.1 - 4));
    sh.two(id0 + 1, r(r_bridge), (t.0, t.1 - 4), (t.0 + 6, t.1 - 4));
    sh.run(&[(t.0 + 6, t.1 - 4), (t.0 + 6, t.1), out]);
    // The T, below: OUT — C — MID — C — IN−, with the leg from MID to ground.
    let mid = (t.0 + 4, t.1 + 4);
    sh.two(id0 + 2, cap(c), out, mid);
    sh.two(id0 + 3, cap(c), mid, (t.0 - 2, t.1 + 4));
    sh.run(&[(t.0 - 2, t.1 + 4), (t.0 - 2, t.1 - 1), minus]);
    sh.two(id0 + 4, r(r_leg), mid, (mid.0, mid.1 + 3));
    sh.ground((mid.0, mid.1 + 3), DOWN);
    (minus, out)
}

/// The whole room.
pub fn tr808_room_circuit() -> Vec<ElementSpec> {
    let mut sh = Sheet::new(300);

    // ------------------------------------------------- mixer + speaker
    // The virtual-earth current mixer: every voice dumps current into SUM
    // through its own resistor and v(OUT) = −I·Rf. Speaker id 1 — the server
    // streams the four lowest-id speakers, so the kit can never be crowded
    // out. There is deliberately NO level pot before it (the `synth.rs`
    // damage rule: the gate winds every pot to 0.98).
    // pins [in+, in-, out]
    let mix = sh.part(10, opamp(), (104, 45), E, 6, true);
    debug_assert_eq!(mix[1], SUM);
    debug_assert_eq!(mix[2], OUT);
    sh.ground(mix[0], DOWN);
    sh.run(&[OUT, (110, 40)]);
    sh.two(11, r(R_F), (110, 40), (104, 40));
    sh.run(&[(104, 40), SUM]);
    sh.wire(OUT, (112, 45));
    sh.two(ID_SPEAKER, K::Speaker { ohms: 8.0 }, (112, 45), (116, 45));
    sh.ground((116, 45), RIGHT);
    // The mixer's collecting row: every voice ends on one of these corners.
    sh.run(&[(96, 44), (97, 44), (98, 44), (99, 44), (100, 44), SUM]);

    // -------------------------------------------------------- KICK (BD)
    // Trigger bus -> coupling cap -> bridged-T -> TONE -> mix leg.
    let trig_bd = trigger_node(0);
    sh.wire(trig_bd, (X_V, 3));
    sh.two(20, cap(BD_C_TRIG), (X_V, 3), (X_V + 4, 3));
    let (bd_in, bd_out) = bridged_t(&mut sh, 21, (50, 4), BD_C, BD_R_LEG, BD_R_BRIDGE);
    debug_assert_eq!(bd_out, kick_node());
    sh.wire((X_V + 4, 3), bd_in);
    // TONE: a rheostat (wiper strapped to the far end) into a shunt cap.
    sh.wire(bd_out, (58, 4));
    // pins [end a, wiper, end b]
    let tone = sh.part(ID_KICK_TONE, pot(BD_TONE_POT, BD_TONE_WIPER), (58, 4), E, 4, false);
    debug_assert_eq!(tone[1], (60, 2));
    debug_assert_eq!(tone[2], (62, 4));
    sh.run(&[tone[1], (62, 2), tone[2]]);
    sh.two(27, cap(BD_TONE_C), (62, 4), (62, 7));
    sh.ground((62, 7), DOWN);
    sh.two(28, r(BD_R_MIX), (66, 4), (96, 4));
    sh.wire((62, 4), (66, 4));
    sh.run(&[(96, 4), (100, 4), (100, 44)]);

    // ------------------------------------------------------- SNARE (SD)
    // The shell, pinged by the same trigger that fires the noise.
    let trig_sd = trigger_node(1);
    sh.wire(trig_sd, (X_V, 43));
    sh.two(30, cap(SD_C_TRIG), (X_V, 43), (X_V + 4, 43));
    let (sd_in, sd_out) = bridged_t(&mut sh, 31, (50, 44), SD_C, SD_R_LEG, SD_R_BRIDGE);
    debug_assert_eq!(sd_out, shell_node());
    sh.wire((X_V + 4, 43), sd_in);
    sh.two(36, r(SD_R_MIX), (58, 44), (96, 44));
    sh.wire(sd_out, (58, 44));
    // The UPPER shell, on its own row above, pinged by the same trigger — the
    // second of the 808's two drum heads. Its coupling cap is smaller because
    // the peak a ping produces goes as `q/C_shell`, and this shell's C is
    // smaller too.
    sh.run(&[(X_V, 43), (X_V, 25)]);
    sh.two(12, cap(SD_C2_TRIG), (X_V, 25), (X_V + 4, 25));
    let (sd2_in, sd2_out) = bridged_t(&mut sh, 13, (50, 26), SD_C2, SD_R_LEG, SD_R_BRIDGE);
    debug_assert_eq!(sd2_out, shell2_node());
    sh.wire((X_V + 4, 25), sd2_in);
    sh.two(18, r(SD_R_MIX2), (58, 26), (96, 26));
    sh.wire(sd2_out, (58, 26));
    sh.run(&[(96, 26), (97, 26), (97, 44)]);

    // The noise chain, its own row below the shell.
    sh.two(
        40,
        K::Noise { volts: 1.0, ohms: 1000.0, seed: 0x0808_5EED },
        (48, 60),
        (44, 60),
    );
    sh.ground((44, 60), LEFT);
    sh.two(41, cap(SD_C_AA), (48, 60), (48, 63));
    sh.ground((48, 63), DOWN);
    sh.two(42, cap(SD_C_HP), (48, 60), (52, 60));
    sh.two(43, r(SD_R_SER), (52, 60), (56, 60));
    sh.two(44, r(SD_R_SHUNT), (56, 60), (56, 63));
    sh.ground((56, 63), DOWN);
    // THE SNAPPY VCA: a MOSFET in its ohmic region between the noise and the
    // mixer's virtual ground, so the gain is Rf/Rds and the envelope on the
    // gate opens and shuts it. Source on the SUM side, where the voltage is
    // pinned, so Vgs is the envelope and nothing else.
    // pins [gate, drain, source]
    let sv = sh.part(45, nmos(), (60, 56), S, 4, true);
    let (sd_gate, sd_drain, sd_src) = (sv[0], sv[1], sv[2]);
    debug_assert_eq!(sd_drain, (58, 60));
    debug_assert_eq!(sd_src, (62, 60));
    sh.wire((56, 60), sd_drain);
    sh.two(49, r(SD_R_VCA), sd_src, (68, 60));
    sh.run(&[(68, 60), (99, 60), (99, 44)]);
    // ENVELOPE GENERATOR: diode into a storage cap, SNAPPY across it. The
    // attack is the cap charging through the trigger's own source impedance;
    // the decay is SNAPPY times the cap, and the wiper is the depth.
    sh.run(&[(X_V, 43), (42, 43), (42, 52)]);
    sh.two(46, K::Diode, (42, 52), (46, 52));
    sh.two(47, cap(SD_C_ENV), (46, 52), (46, 55));
    sh.ground((46, 55), DOWN);
    // pins [end a, wiper, end b]. Drawn facing WEST so end A — the end the
    // wiper fraction is measured from — is the GROUNDED one: turning SNAPPY
    // up then really is more snap, which is what the knob says.
    let snappy = sh.part(ID_SNAPPY, pot(SD_POT_SNAPPY, SD_SNAPPY_WIPER), (52, 52), W, 4, false);
    debug_assert_eq!(snappy[1], (50, 54));
    debug_assert_eq!(snappy[2], (48, 52));
    sh.wire((46, 52), (48, 52));
    sh.ground(snappy[0], RIGHT);
    sh.run(&[snappy[1], (60, 54), sd_gate]);

    // ------------------------------------------------------- CLAVES (CL)
    // The third resonator, and the cheapest voice in the machine: a
    // bridged-T with a hundredth of the kick's capacitance rings at 2.7 kHz
    // with a 2 ms tail — a "tok". No envelope, no VCA, no oscillator; the Q
    // of the network IS the decay, which is exactly what the 808's CL is.
    let trig_cl = trigger_node(2);
    sh.wire(trig_cl, (X_V, 83));
    sh.two(50, cap(CL_C_TRIG), (X_V, 83), (X_V + 4, 83));
    let (cl_in, cl_out) = bridged_t(&mut sh, 51, (50, 84), CL_C, CL_R_LEG, CL_R_BRIDGE);
    debug_assert_eq!(cl_out, clave_node());
    sh.wire((X_V + 4, 83), cl_in);
    sh.two(56, r(CL_R_MIX), (58, 84), (96, 84));
    sh.wire(cl_out, (58, 84));
    sh.run(&[(96, 84), (98, 84), (98, 44)]);

    // ---------------------------------------------------------- SEQUENCER
    // Its own 5 V rail, a 555 clock, a 2-bit counter, and one multiplexer
    // per voice fed by that voice's row of pattern switches.
    sh.els.push(ElementSpec {
        id: ID_LOGIC_RAIL,
        kind: K::Rail { dc: LOGIC_V, amp: 0.0, hz: 0.0, phase: 0.0, wave: sim_core::Wave::Sine },
        pins: vec![(-24, -26)],
        rot: 2,
        ..Default::default()
    });
    // The clock. pins [vcc, gnd, trg, thr, out, dis]
    let clk = sh.part(80, K::Timer555, (-16, -20), E, 4, false);
    let (cvcc, cgnd, ctrg, cthr, cout, cdis) = (clk[0], clk[1], clk[2], clk[3], clk[4], clk[5]);
    debug_assert_eq!(cout, (-12, -17));
    debug_assert_eq!(cdis, (-12, -19));
    // The 5 V bus along the top, with a corner at every drop.
    sh.run(&[(-24, -26), (-16, -26), (-12, -26), (3, -26), (4, -26)]);
    sh.wire((-16, -26), cvcc);
    sh.ground(cgnd, DOWN);
    // TRIG strapped to THR down the chip's left side; the corner at
    // (-19, -18) is the timing node everything else hangs on.
    sh.run(&[ctrg, (-17, -19), (-17, -18), (-17, -17), cthr]);
    // RA from the rail straight down into DIS.
    sh.two(81, r(CLK_RA), (-12, -26), cdis);
    // The tempo leg: DIS -> fixed R -> TEMPO rheostat -> back over the top to
    // the timing cap. The diode sits straight across the whole leg, anode on
    // the DIS side, so the CHARGE current skips it: HIGH stays a 2 ms trigger
    // however slow the tempo is, and only the LOW time is the step.
    sh.wire(cdis, (-10, -19));
    sh.two(82, r(CLK_R_MIN), (-10, -19), (-6, -19));
    // pins [end a, wiper, end b]
    let tempo = sh.part(ID_TEMPO, pot(CLK_POT, TEMPO_WIPER), (-6, -19), E, 4, false);
    debug_assert_eq!(tempo[1], (-4, -21));
    debug_assert_eq!(tempo[2], (-2, -19));
    sh.run(&[tempo[1], (-2, -21), tempo[2]]);
    sh.run(&[tempo[2], (-2, -23), (-10, -23), (-19, -23), (-19, -18), (-17, -18)]);
    sh.two(84, K::Diode, (-10, -19), (-10, -23));
    sh.two(85, cap(CLK_C), (-21, -18), (-19, -18));
    sh.ground((-21, -18), LEFT);

    // The counter. pins [VCC, GND, CLK, RST, Q0, Q1]; RST is active low and
    // tied to its own VCC, so it simply runs.
    let ctr: Vec<Point> = vec![(4, -26), (4, -20), (4, -25), (4, -24), (10, -22), (10, -23)];
    sh.els.push(ElementSpec {
        id: ID_COUNTER,
        kind: K::Counter { bits: 2, modulus: 4 },
        pins: ctr.clone(),
        ..Default::default()
    });
    sh.run(&[(3, -26), (3, -24), ctr[3]]);
    sh.ground(ctr[1], DOWN);
    // The clock's own output: down the right of the chip, along under the
    // timing network to the counter, and on down the pattern rows' riser.
    sh.run(&[cout, (-12, -15), (2, -15), (2, -25), ctr[2]]);
    sh.run(&[(-12, -15), (-12, -6), (-12, 34), (-12, 74)]);
    // Q0 and Q1 down their own columns to every multiplexer's select pins.
    sh.run(&[ctr[4], (25, -22), (25, 5), (25, 45), (25, 85)]);
    sh.run(&[ctr[5], (26, -23), (26, 6), (26, 46), (26, 86)]);
    // The 5 V bus down to the multiplexers.
    sh.run(&[(3, -26), (24, -26), (24, 0), (24, 40), (24, 80)]);

    for v in 0..3usize {
        let y = YB[v];
        // The clock pulse across this row, and a switch per step. A CLOSED
        // switch simply connects the pulse to that channel of this voice's
        // multiplexer: the 808's "step AND pattern bit" for free, because the
        // Mux is a pass gate and not a logic buffer.
        sh.run(&[
            (-12, y - 6),
            (X_STEP[0], y - 6),
            (X_STEP[1], y - 6),
            (X_STEP[2], y - 6),
            (X_STEP[3], y - 6),
        ]);
        for (k, &x) in X_STEP.iter().enumerate() {
            sh.run(&[(x, y - 6), (x, y - 5)]);
            sh.two(
                ID_ROW[v] + k as u32,
                K::Switch { closed: PATTERN[v][k] },
                (x, y - 5),
                (x, y - 2),
            );
            sh.two(ID_ROW[v] + 5 + k as u32, r(R_STEP_PD), (x, y - 2), (x + 3, y - 2));
            sh.ground((x + 3, y - 2), RIGHT);
            // Fan out to the multiplexer, each channel on its own row.
            sh.run(&[(x, y - 2), (x, y + 1 + k as i32), (X_MUX, y + 1 + k as i32)]);
        }
        // pins [VCC, GND, I0..I3, S0, S1, Y]
        let m: Vec<Point> = vec![
            (X_MUX, y),
            (X_MUX, y + 7),
            (X_MUX, y + 1),
            (X_MUX, y + 2),
            (X_MUX, y + 3),
            (X_MUX, y + 4),
            (X_MUX, y + 5),
            (X_MUX, y + 6),
            (X_MUX + 6, y + 3),
        ];
        sh.els.push(ElementSpec {
            id: 91 + v as u32,
            kind: K::Mux { sel: 2 },
            pins: m.clone(),
            ..Default::default()
        });
        sh.run(&[(24, y), m[0]]);
        sh.ground(m[1], DOWN);
        sh.run(&[(25, y + 5), m[6]]);
        sh.run(&[(26, y + 6), m[7]]);
        debug_assert_eq!(m[8], trigger_node(v));
    }

    let mut els = sh.finish();
    name_controls(&mut els);
    els
}

/// The front-panel legend on every part a player can touch.
fn name_controls(els: &mut [ElementSpec]) {
    let mut named: Vec<(u32, String)> = vec![
        (ID_LOGIC_RAIL, "LOGIC 5V".into()),
        (ID_TEMPO, "TEMPO".into()),
        (ID_KICK_TONE, "KICK TONE".into()),
        (ID_SNAPPY, "SNAPPY".into()),
    ];
    for (v, row) in ID_ROW.iter().enumerate() {
        for k in 0..4u32 {
            named.push((row + k, format!("{} {}", ROW_NAMES[v], k + 1)));
        }
    }
    for e in els.iter_mut() {
        if let Some((_, n)) = named.iter().find(|(id, _)| *id == e.id) {
            e.name = n.clone();
        }
    }
    debug_assert!(
        named.iter().all(|(id, _)| els.iter().any(|e| e.id == *id)),
        "a control was named that the circuit does not contain"
    );
}

/// ONE control panel spanning the instrument (see `synth.rs` for why one).
pub fn tr808_panels() -> Vec<crate::synth::PanelDef> {
    vec![crate::synth::PanelDef {
        x0: -24.0,
        y0: -26.0,
        x1: 120.0,
        y1: 100.0,
        name: "RHYTHM COMPOSER",
    }]
}

/// Block headings, plus the honesty plaque: what the real machine was and
/// what here is faithful versus a stand-in, on the sheet itself.
pub fn tr808_label_boxes() -> Vec<crate::synth::PanelDef> {
    use crate::synth::PanelDef;
    let b = |x0: f64, y0: f64, x1: f64, y1: f64, name: &'static str| PanelDef { x0, y0, x1, y1, name };
    vec![
        b(-23.0, -28.0, 12.0, -12.0, "CLOCK  TEMPO / COUNTER"),
        b(-10.0, -7.5, 35.0, 8.5, "BD PATTERN + MUX"),
        b(-10.0, 32.5, 35.0, 48.5, "SD PATTERN + MUX"),
        b(-10.0, 72.5, 35.0, 88.5, "CL PATTERN + MUX"),
        b(42.0, -1.5, 68.0, 8.5, "KICK  BRIDGED-T  TONE"),
        b(42.0, 20.5, 68.0, 34.0, "SNARE SHELL 2  306 HZ"),
        b(42.0, 38.5, 68.0, 48.5, "SNARE SHELL 1  170 HZ"),
        b(40.0, 49.5, 68.0, 64.5, "SNARE NOISE  SNAPPY VCA"),
        b(42.0, 78.5, 68.0, 92.0, "CLAVES  BRIDGED-T 2.7K"),
        b(101.0, 38.0, 118.0, 48.5, "MIXER + SPEAKER"),
        // The plaque. 28 characters a line is what a label box holds.
        b(72.0, -2.0, 102.0, -0.4, "AFTER THE ROLAND TR-808."),
        b(72.0, 0.0, 102.0, 1.6, "KICK, BOTH SNARE SHELLS"),
        b(72.0, 2.0, 102.0, 3.6, "AND THE CLAVES ARE REAL"),
        b(72.0, 4.0, 102.0, 5.6, "BRIDGED-T RESONATORS - THE"),
        b(72.0, 6.0, 102.0, 7.6, "808 TOPOLOGY - RINGING AT"),
        b(72.0, 8.0, 102.0, 9.6, "52 / 170 / 306 / 2700 HZ."),
        b(72.0, 10.0, 102.0, 11.6, "STAND-IN: A MOSFET FOR THE"),
        b(72.0, 12.0, 102.0, 13.6, "SNAPPY TRANSISTOR, AND 4"),
        b(72.0, 14.0, 102.0, 15.6, "STEPS NOT 16 (THE MUX IS 4"),
        b(72.0, 16.0, 102.0, 17.6, "WIDE). NO COWBELL: TWO MORE"),
        b(72.0, 18.0, 102.0, 19.6, "OSCILLATORS COST 0.83x REAL"),
        b(72.0, 20.0, 102.0, 21.6, "TIME, WHICH IS A SYNTH RUN"),
        b(72.0, 22.0, 102.0, 23.6, "FLAT. NO HI-HAT: THE AUDIO"),
        b(72.0, 24.0, 102.0, 25.6, "TAP NYQUIST IS 6.25 KHZ AND"),
        b(72.0, 26.0, 102.0, 27.6, "A REAL HAT LIVES ABOVE IT."),
    ]
}

// ------------------------------------------------------------ SCOPE NOTES
//
// Measured on this machine (Apple M4, release, pinned 1.95.0) with several
// other agents building in parallel — method as `synth.rs`: best of three
// passes of 150 000 substeps stepped the way the server steps, after two
// seconds of settling.
//
//   SHIPPED: 264 elements — 78 DEVICES plus the wires and ground symbols of
//   routing. The device count is the budget; routing is very nearly free.
//   16.47 µs/substep against the 20 µs budget = 1.21x real time offline, NR
//   1.62, 54 unknowns. Live over a websocket: see the shipping report.
//   For comparison the shipped `synth.rs` room measures 16.03 µs / 1.25x on
//   the same pass and holds rt 0.999 live, so this room is at the same bar.
//
//   ---- WHAT THE BRIDGED-T ACTUALLY DID ----
//
//   The prediction was `f0 = 1/(2π·C·√(R_leg·R_bridge))`, `Q = √(R_b/R_l)/2`.
//   Measured on the bench (`tr808_resonator_bench`), pinged and left alone:
//
//     kick    100 n, 1 k, 1 M     50.3 Hz predicted   50.0 Hz measured
//     shell 1  27 n, 1 k, 1.2 M  170.1 Hz predicted  170.0 Hz measured
//     shell 2  15 n, 1 k, 1.2 M  306.2 Hz predicted  305.0 Hz measured
//     claves  1.5 n, 1 k, 1.5 M    2.7 kHz predicted   2.7 kHz measured
//
//   To four significant figures on the two low ones. This is the payoff of
//   a LINEAR resonator: no comparator, so no `50 kHz / n` substep pitch grid
//   and no dither needed to hide it (compare `vco555.rs`, which needs 5 mV
//   of noise under its timing cap to keep a continuous average).
//
//   THE PEAK GOES AS q/C_shell, and that is the whole trimming story. The
//   first build coupled the trigger in through 22 nF as a real 808 does and
//   every resonator sat on the ±9 V rail: 110 nC into a 100 nF network is
//   not a mallet, it is a hammer. The coupling caps here are 1 nF (kick) and
//   100 pF (both shells, whose C is 4–7x smaller so the same charge makes
//   4–7x the volts) and the peaks land at 0.93 / 0.88 / 0.84 V.
//
//   ---- THE OP-AMP'S 25 mA IS THE REAL CEILING ----
//
//   `DEFAULT_OPAMP_ISC` is 25 mA and a speaker is 8 Ω, so an op-amp output
//   stage in this engine cannot put more than 0.2 V across the coil, ever.
//   The first mixer here used Rf/R_mix ratios sized by ear and clipped flat
//   at ±0.200 V — not softly, but as a current clamp, so a 50 Hz sine came
//   out a square. Every mix resistor is now trimmed so the worst coincidence
//   (kick + both shells + snap on one step) lands near 0.15 V and the mixer
//   never leaves its linear region. Measured out: −0.101 .. +0.081 V.
//   Consequence for anyone extending this room: you do not get louder by
//   turning something up, you get louder by using a bigger `Rf`.
//
//   ---- THE MOSFET VCA NEEDS A SERIES RESISTOR ----
//
//   `Rds ≈ 1/(k(Vgs−Vt))` is about 7.6 kΩ with the snare envelope up and
//   ~100 MΩ with it down. Wired straight into a 100 kΩ transimpedance that
//   is a gain of 13, which clipped the mixer on every hit. With `SD_R_VCA`
//   in series the leg's gain is `Rf/(R_ser+Rds)` — set by the resistor when
//   open, still killed by `Rds` when shut, and 470 k against 100 MΩ leaves
//   46 dB of range. It also divides the drain signal by 200 before the
//   MOSFET sees it, which is what keeps the part in its ohmic region and
//   therefore a RESISTOR rather than a distorter.
//
//   ---- THE SEQUENCER IS ALMOST FREE, THE COWBELL WAS NOT ----
//
//   Counter + three Mux + twelve switches: NR 1.62 for the whole room, and
//   the digital parts contribute none of it — a `Counter` and a `Mux` are
//   discrete nonlinearities, so the LU survives between state flips. What
//   the sequencer does cost is UNKNOWNS: 4 channel nodes and a Y per mux, 15
//   nodes of the room's 54. That is the number to watch, because cost goes
//   as `unknowns^1.64`: the cowbell version of this room measured 62
//   unknowns and 24.18 µs = 0.83x, and the fix was not optimisation, it was
//   deciding which voice to cut.
//
//   ---- WHAT A PLAYER CAN DO TO IT ----
//
//   A closed `Switch` is a 0 V source and therefore a branch unknown; an
//   open one stamps nothing. The shipped pattern closes 4 of the 12, so a
//   player who fills the whole grid adds 8 unknowns (54 -> 62) and the room
//   would land near 0.9x. That is the honest worst case of this design and
//   it is the reason the grid is four steps and not eight.

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Engine;

    const DT: f64 = 20e-6;

    /// Every part is a legal shape, every wire orthogonal, every id unique —
    /// the room is a document the editor itself would accept.
    #[test]
    fn tr808_room_is_a_legal_document() {
        let els = tr808_room_circuit();
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
                assert!(a.0 == b.0 || a.1 == b.1, "diagonal wire {}", e.id);
            }
        }
        let mut ids: Vec<u32> = els.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), els.len(), "duplicate element id");
    }

    /// Alive from boot: the clock runs, nothing quarantines, nothing needs
    /// rescuing, and the speaker is driven — but not into the op-amp's
    /// current clamp, which is what "loud" means here.
    #[test]
    fn tr808_room_never_quarantines_and_plays() {
        let els = tr808_room_circuit();
        let mut eng = Engine::new(DT);
        eng.set_elements(&els);
        let mut rescues = 0;
        for _ in 0..300 {
            let rep = eng.advance(500); // 10 ms chunks, 3 s total
            rescues += rep.rescues;
            assert!(!eng.is_quarantined(), "quarantined at t={:.3}", eng.time());
        }
        assert_eq!(rescues, 0, "rescue steps while idling");
        let (mut rms, mut peak) = (0.0f64, 0.0f64);
        let n = 150_000;
        for _ in 0..n {
            eng.advance(1);
            let v = eng.voltage_at(out_node()).unwrap_or(0.0);
            rms += v * v;
            peak = peak.max(v.abs());
        }
        let rms = (rms / f64::from(n)).sqrt();
        assert!(rms > 0.003, "speaker rms {rms:.4} V — the kit is silent");
        // 0.2 V is `DEFAULT_OPAMP_ISC` * 8 Ω: at the clamp the mixer stops
        // being an amplifier and starts being a square-wave generator.
        assert!(
            peak < 0.19,
            "peak {peak:.3} V — the mixer is in its current clamp"
        );
    }

    /// The four resonators ring where the bridged-T formula says they will.
    /// This is the room's central claim, so it is a test and not a comment.
    #[test]
    fn tr808_resonators_ring_where_the_formula_says() {
        let cases: [(&str, f64, f64, f64, f64, f64); 4] = [
            ("kick", BD_C, BD_R_LEG, BD_R_BRIDGE, BD_C_TRIG, 50.3),
            ("shell 1", SD_C, SD_R_LEG, SD_R_BRIDGE, SD_C_TRIG, 170.1),
            ("shell 2", SD_C2, SD_R_LEG, SD_R_BRIDGE, SD_C2_TRIG, 306.2),
            ("claves", CL_C, CL_R_LEG, CL_R_BRIDGE, CL_C_TRIG, 2740.0),
        ];
        for (name, c, rl, rb, ct, want) in cases {
            // f0 = 1 / (2 pi C sqrt(Rl * Rb)) — the value the doc claims.
            let f0 = 1.0 / (std::f64::consts::TAU * c * (rl * rb).sqrt());
            assert!(
                (f0 / want - 1.0).abs() < 0.01,
                "{name}: the formula gives {f0:.1} Hz, the doc says {want}"
            );
            let (hz, peak, quarantined) = ring(c, rl, rb, ct);
            assert!(!quarantined, "{name}: quarantined");
            let cents = 1200.0 * (hz / f0).log2();
            assert!(
                cents.abs() < 60.0,
                "{name}: rang at {hz:.1} Hz, {cents:+.0} cents from {f0:.1}"
            );
            assert!(
                peak > 0.3 && peak < 2.5,
                "{name}: pinged to {peak:.3} V — silent, or into the rail"
            );
        }
    }

    /// One bridged-T on a bench, pinged by a 2 Hz square: returns its ring
    /// frequency, its peak, and whether it survived.
    fn ring(c: f64, rl: f64, rb: f64, ct: f64) -> (f64, f64, bool) {
        let mut sh = Sheet::new(300);
        sh.two(
            1,
            K::VoltageSource {
                dc: 2.5,
                amp: 2.5,
                hz: 2.0,
                phase: 0.0,
                wave: sim_core::Wave::Square,
            },
            (40, 3),
            (40, 7),
        );
        sh.ground((40, 7), DOWN);
        sh.two(2, cap(ct), (40, 3), (44, 3));
        let (i, o) = bridged_t(&mut sh, 21, (50, 4), c, rl, rb);
        sh.wire((44, 3), i);
        let els = sh.finish();
        let mut e = Engine::new(DT);
        e.set_elements(&els);
        e.advance(50_000);
        // Record half a second of the ring, then read the frequency off the
        // part of it that is actually ringing: a high-Q resonator spends most
        // of the window silent between pings, and counting the noise floor's
        // crossings there would report any frequency at all.
        let mut trace = Vec::with_capacity(25_000);
        for _ in 0..25_000 {
            e.advance(1);
            trace.push(e.voltage_at(o).unwrap_or(0.0));
        }
        // A decaying sinusoid keeps crossing zero at f0 all the way down, so
        // the crossings themselves need no gating; what does need gating is
        // the DEAD time after the ring has fallen into the numerical floor,
        // where the crossings are noise. So: measure between the first and
        // the last sample that is still meaningfully ringing.
        let peak = trace.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let floor = peak * 1e-3;
        // ONE burst: from the first live sample to the last one before four
        // whole milliseconds of silence. Spanning two pings would count the
        // dead air between them as part of the ring and report a frequency
        // too low — which is exactly what it did before this line existed.
        let a = trace.iter().position(|v| v.abs() > floor).unwrap_or(0);
        let mut b = a;
        for (i, v) in trace.iter().enumerate().skip(a) {
            if v.abs() > floor {
                b = i;
            } else if i - b > 200 {
                break;
            }
        }
        let xs = trace[a..=b]
            .windows(2)
            .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
            .count();
        let secs = (b - a) as f64 * DT;
        (
            if secs > 0.0 {
                xs as f64 / 2.0 / secs
            } else {
                0.0
            },
            peak,
            e.is_quarantined(),
        )
    }

    /// The pattern really addresses the voices it says it does: over one bar
    /// each trigger bus pulses once per closed switch in its row, and never
    /// on a step that belongs to another voice.
    #[test]
    fn tr808_pattern_drives_the_right_voices() {
        let els = tr808_room_circuit();
        let mut eng = Engine::new(DT);
        eng.set_elements(&els);
        eng.advance(50_000);
        // Six seconds is a good many four-step bars at the shipped tempo.
        let n = 300_000;
        let mut hits = [0u32; 3];
        let mut was = [false; 3];
        for _ in 0..n {
            eng.advance(1);
            for v in 0..3 {
                let hi = eng.voltage_at(trigger_node(v)).unwrap_or(0.0) > 2.0;
                if hi && !was[v] {
                    hits[v] += 1;
                }
                was[v] = hi;
            }
        }
        // Bars in the window, from the clock itself.
        let steps: u32 = hits.iter().sum::<u32>();
        assert!(steps > 0, "nothing triggered at all");
        let closed: Vec<u32> = (0..3)
            .map(|v| PATTERN[v].iter().filter(|b| **b).count() as u32)
            .collect();
        // Every voice fires in proportion to its row, within one bar of slop.
        let bars = f64::from(hits[0]) / f64::from(closed[0]);
        assert!(bars > 3.0, "only {bars:.1} bars in 6 s — is the clock dead?");
        for v in 0..3 {
            let want = bars * f64::from(closed[v]);
            assert!(
                (f64::from(hits[v]) - want).abs() <= 1.5,
                "voice {v} fired {} times, expected about {want:.1}",
                hits[v]
            );
        }
    }

    /// Controls are named and inside the one panel — a panel row must read
    /// SNAPPY or BD 3, never POT #405.
    #[test]
    fn tr808_controls_are_named_and_reachable() {
        let els = tr808_room_circuit();
        let panels = tr808_panels();
        assert_eq!(panels.len(), 1);
        let p = &panels[0];
        let mut controls = 0;
        for e in &els {
            if matches!(e.kind, K::Potentiometer { .. } | K::Switch { .. }) {
                controls += 1;
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
        // Three pattern rows of four, plus TEMPO, KICK TONE and SNAPPY.
        assert_eq!(controls, 15);
    }

    #[test]
    #[ignore]
    fn tr808_resonator_bench() {
        for (c, rl, rb, ct) in [
            (BD_C, BD_R_LEG, BD_R_BRIDGE, BD_C_TRIG),
            (SD_C, SD_R_LEG, SD_R_BRIDGE, SD_C_TRIG),
        ] {
            let mut sh = Sheet::new(300);
            // A 2 Hz square as the trigger source, through the coupling cap.
            sh.two(
                1,
                K::VoltageSource {
                    dc: 2.5,
                    amp: 2.5,
                    hz: 2.0,
                    phase: 0.0,
                    wave: sim_core::Wave::Square,
                },
                (40, 3),
                (40, 7),
            );
            sh.ground((40, 7), DOWN);
            sh.two(2, cap(ct), (40, 3), (44, 3));
            let (i, o) = bridged_t(&mut sh, 21, (50, 4), c, rl, rb);
            sh.wire((44, 3), i);
            let els = sh.finish();
            let mut e = Engine::new(DT);
            e.set_elements(&els);
            e.advance(50_000);
            let (mut xs, mut peak) = (0u32, 0.0f64);
            let mut last = e.voltage_at(o).unwrap_or(0.0);
            for _ in 0..50_000 {
                e.advance(1);
                let v = e.voltage_at(o).unwrap_or(0.0);
                if (v > 0.0) != (last > 0.0) {
                    xs += 1;
                }
                peak = peak.max(v.abs());
                last = v;
            }
            println!(
                "C={c:e} Rl={rl} Rb={rb}: {:.1} Hz, peak {peak:.3} V, quarantined {}",
                f64::from(xs) / 2.0,
                e.is_quarantined()
            );
        }
    }

    #[test]
    #[ignore]
    fn tr808_debug() {
        let els = tr808_room_circuit();
        let devices = els
            .iter()
            .filter(|e| !matches!(e.kind, K::Wire | K::Ground))
            .count();
        let mut eng = Engine::new(DT);
        eng.set_elements(&els);
        eng.advance(100_000); // 2 s of settling before anything is believed
        println!(
            "{} elements, {} devices, {} unknowns, quarantined {}",
            els.len(),
            devices,
            eng.unknowns(),
            eng.is_quarantined()
        );
        // Ring frequency of each resonator, by zero crossings over 3 s.
        for (name, p) in [("kick", kick_node()), ("shell", shell_node())] {
            let mut xs = 0u32;
            let mut last = eng.voltage_at(p).unwrap_or(0.0);
            let mut peak = 0.0f64;
            for _ in 0..150_000 {
                eng.advance(1);
                let v = eng.voltage_at(p).unwrap_or(0.0);
                if (v > 0.0) != (last > 0.0) {
                    xs += 1;
                }
                peak = peak.max(v.abs());
                last = v;
            }
            println!("{name}: {:.1} Hz ring, peak {peak:.3} V", f64::from(xs) / 2.0 / 3.0);
        }
        // Envelope and VCA levels, peak over 3 s.
        let mut pk = [0.0f64; 5];
        let probes = [
            ("sd env", (46, 52)),
            ("snappy wiper", (50, 54)),
            ("sd drain", (58, 60)),
            ("shell2", (54, 26)),
            ("clave", (54, 84)),
        ];
        for _ in 0..150_000 {
            eng.advance(1);
            for (i, (_, p)) in probes.iter().enumerate() {
                pk[i] = pk[i].max(eng.voltage_at(*p).unwrap_or(0.0).abs());
            }
        }
        for (i, (n, _)) in probes.iter().enumerate() {
            println!("{n:<14} peak {:.4} V", pk[i]);
        }
        let mut edges = 0u32;
        let mut last = 0.0;
        let n = 150_000;
        let (mut kmin, mut kmax) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut smin, mut smax) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut omin, mut omax) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut trig_hits = [0u32; 3];
        let mut orms = 0.0;
        for _ in 0..n {
            eng.advance(1);
            let c = eng.voltage_at((-12, -17)).unwrap_or(0.0);
            if c > 2.5 && last <= 2.5 {
                edges += 1;
            }
            last = c;
            for v in 0..3 {
                if eng.voltage_at(trigger_node(v)).unwrap_or(0.0) > 2.0 {
                    trig_hits[v] += 1;
                }
            }
            let k = eng.voltage_at(kick_node()).unwrap_or(0.0);
            kmin = kmin.min(k);
            kmax = kmax.max(k);
            let s = eng.voltage_at(shell_node()).unwrap_or(0.0);
            smin = smin.min(s);
            smax = smax.max(s);
            let o = eng.voltage_at(out_node()).unwrap_or(0.0);
            omin = omin.min(o);
            omax = omax.max(o);
            orms += o * o;
        }
        let secs = n as f64 * DT;
        println!("clock {:.2} steps/s", f64::from(edges) / secs);
        println!(
            "trigger high fraction {:?}",
            trig_hits.map(|h| f64::from(h) / n as f64)
        );
        println!("kick  {kmin:+.3} .. {kmax:+.3} V");
        println!("shell {smin:+.3} .. {smax:+.3} V");
        println!(
            "out   {omin:+.3} .. {omax:+.3} V  rms {:.4}",
            (orms / n as f64).sqrt()
        );
        println!("quarantined {}", eng.is_quarantined());
    }
}
