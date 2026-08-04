//! BASS++ — after Thomas Henry's drum voice of that name.
//!
//! The historical instrument: Thomas Henry published Bass++ as a two-chip
//! analogue bass drum — an LM13700 "shell VCO" whose triangle sweeps from
//! about 10 Hz to 5 kHz, a quad op-amp round it, one envelope generator
//! driving both the pitch sweep and the level, a passive "impact" click
//! network with its own tone control, and a mixer to set how much shell and
//! how much impact you hear. Panel: PITCH, SWEEP, DECAY, IMPACT.
//! (birthofasynth.com's project page; Eddy Bergman's build #59.)
//!
//! ## What is faithful
//!
//! THE SHELL IS AN LM13700-CLASS TRANSCONDUCTANCE OSCILLATOR, which is what
//! Bass++ is built on. Two `Ota`s in a two-integrator loop:
//!
//! ```text
//!   A' = −(gm/C)·B − A/(R_decay·C)      B' = (gm/C)·A
//!   → poles at −1/(2·R_decay·C) ± j·gm/C
//! ```
//!
//! so the PITCH is `gm/2πC` and follows the bias current, while the DECAY is
//! `R_decay·C` and does not — which is exactly the separation Henry's panel
//! gives you. One trigger both RINGS it and SWEEPS it, because the same
//! envelope capacitor that pings the loop also decays into the bias node.
//! And because the loop is capacitor-timed rather than comparator-timed, the
//! sweep is CONTINUOUS: no `50 kHz / n` substep pitch grid anywhere in it.
//!
//! Measured in THIS room by `bass_pitch_table`, attack (0–40 ms after the
//! trigger) against tail (120–300 ms), synchronised to a hit:
//!
//! | PITCH | attack → tail | what it is |
//! |-------|---------------|------------|
//! | 0.01  | 2600 → 2561   | a ping     |
//! | 0.05  |  575 →  522   | a bongo    |
//! | 0.20  |  175 →  128   | a tom      |
//! | 0.52  |  100 →   50   | the kick   |
//! | 0.99  |   75 →   28   | a boom     |
//!
//! One knob, five instruments, and Henry's "10 Hz to 5.2 kHz" claim for the
//! shell VCO is very nearly the range that falls out. The figures land within
//! a few per cent of `drums.rs`'s independently measured table for the same
//! resonator (2616→2560, 588→517, 199→129, 116→49, 88→26), which is the
//! cross-check that the shell really was lifted intact.
//!
//! SYNCHRONISING THAT MEASUREMENT ON THE HIT is not a nicety. Counting
//! crossings in a fixed window from an arbitrary start mixes fresh attack
//! with old tail in every window and averages the sweep away to nothing: the
//! first version of this table read 2640 → 2630 at PITCH 0.01 and 130 → 122
//! at 0.52, and would have let a room with no pitch envelope at all ship
//! looking correct.
//!
//! THE IMPACT IS PASSIVE, as his is: the trigger through a series resistor
//! into a tone rheostat to ground, AC-coupled into the mixer. A click, made
//! of the trigger itself and nothing else.
//!
//! ## What is a stand-in
//!
//!   * ONE OP-AMP does the whole output stage where Bass++ has a quad. Its
//!     non-inverting input taps the shell's resonator node directly — op-amp
//!     inputs draw exactly zero current in this model, so the resonator is
//!     not loaded at all — and its inverting input is a virtual-earth bus the
//!     impact dumps into. `v(out) = v(shell)·(1 + Rf/Rg) − I_impact·Rf`, so
//!     one part is both the shell amplifier and the mixer.
//!   * NO SEPARATE LEVEL VCA. Henry's envelope drives a VCA as well as the
//!     sweep; here the shell's own damping IS the amplitude envelope, which
//!     is what `R_decay·C` in the pole expression above means. It costs a
//!     transistor and a control, and it is the reason DECAY does one job here
//!     where Bass++ splits it into two.
//!   * The trigger comes from a 555 astable with a TEMPO knob, plus a HIT
//!     button in parallel for playing it by hand. Bass++ takes a gate from
//!     whatever you plug into it.
//!
//! ## Signal flow
//!
//! ```text
//!   TEMPO 555 ─┬─ 10k ─> TRIGGER ─ diode ─> env cap ─ SWEEP ─> OTA bias
//!   HIT button ┘                       │                          ^
//!                                      │                     PITCH ┘
//!                                      ├─ 220 pF ──> shell node A
//!                                      └─ 1M ─ IMPACT ─ cap ─ 1M ─┐
//!   shell: OTA ─> A ─> OTA ─> B ─> back      DECAY across A       │
//!                     A ──> op-amp in+ ─────────────────> out ────┴─> 8 Ω
//! ```
//!
//! SCOPE NOTES are at the bottom of this file.

use sim_core::{ElementKind as K, ElementSpec, Point};

use crate::layout::{Sheet, DOWN, E, RIGHT, UP};
use crate::modules;

// ---------------------------------------------------------------- values

const SUPPLY_V: f64 = 9.0;
const RAIL_Y: i32 = -24;

// -- clock. The diode across the timing rheostat is bypassed by the charge
// current, so HIGH is a 7 ms trigger and only the LOW time is the beat.
const CLK_RA: f64 = 10e3;
const CLK_R_MIN: f64 = 47e3;
const CLK_POT: f64 = 1e6;
const CLK_C: f64 = 1e-6;
pub const TEMPO_WIPER: f64 = 0.40;

/// SERIES TRIGGER RESISTOR — load-bearing, not decoration. The 555's output
/// is an ideal voltage branch and a diode is ideal, so without it the
/// envelope capacitor is charged inside one substep and the trapezoidal
/// integrator rings: `drums.rs` measured 13.8 V on a cap fed from a 7.8 V
/// pulse. 10 k puts the charging time constant well above dt/2 and the
/// overshoot vanishes.
const R_TRIG: f64 = 10e3;
/// The HIT button's own series resistor, same job.
const R_HIT: f64 = 10e3;

// -- envelope. Diode into a storage cap; the cap both pings the shell and
// decays into its bias, which is the one-envelope-two-jobs Bass++ trick.
const C_ENV: f64 = 33e-9;
/// SWEEP: how much of the envelope reaches the bias node, and therefore how
/// far the pitch falls. A rheostat, so 0 is no sweep at all.
const POT_SWEEP: f64 = 5e6;
pub const SWEEP_WIPER: f64 = 0.30;
/// PITCH: the resting bias current, and so where the sweep LANDS.
const POT_PITCH: f64 = 5e6;
pub const PITCH_WIPER: f64 = 0.52;

// -- the shell. `f = gm/(2πC)`, `gm = Iabc/(2·VT)`.
const C_SHELL: f64 = 100e-9;
/// DECAY: the damping resistor across the resonator node. The amplitude
/// falls with `R·C` and the PITCH does not, which is why these are two knobs
/// and not one. 0.24 of a 1 MΩ track is the 240 k `drums.rs` measured at
/// −20 dB in 95 ms.
const POT_DECAY: f64 = 1e6;
pub const DECAY_WIPER: f64 = 0.24;
/// Excitation. 220 pF against the shell's 100 nF injects about 17 mV, and
/// that is deliberately small: an OTA input is linear only to about ±20 mV
/// (`tanh(vd/2VT)`), and a hard-driven gm-C loop compresses its own
/// transconductance and drops up to half its pitch. Small keeps it in tune.
const C_EXCITE: f64 = 220e-12;

// -- impact. Series resistor into a tone rheostat to ground: it attenuates
// and brightens together, which is what Henry's IMPACT TONE does.
const R_IMPACT_IN: f64 = 1e6;
const POT_IMPACT: f64 = 10e3;
pub const IMPACT_WIPER: f64 = 0.50;
const C_IMPACT: f64 = 10e-9;
const R_IMPACT_OUT: f64 = 1e6;

// -- output stage. Non-inverting gain `1 + RF/RG` for the shell, and `RF` is
// the transimpedance for the impact bus. Sized against the op-amp's real
// ceiling: `DEFAULT_OPAMP_ISC` is 25 mA into 8 Ω, so nothing can exceed
// 0.2 V and a 17 mV shell wants a gain near eight, not near seventy.
const R_F: f64 = 680e3;
const R_G: f64 = 100e3;

/// Ids a player touches.
pub const ID_SPEAKER: u32 = 1;
pub const ID_SUPPLY: u32 = 2;
pub const ID_TEMPO: u32 = 23;
pub const ID_HIT: u32 = 32;
pub const ID_SWEEP: u32 = 35;
pub const ID_PITCH: u32 = 36;
pub const ID_DECAY: u32 = 44;
pub const ID_IMPACT: u32 = 51;

/// The shell's resonator node — the drum itself, and what the op-amp taps.
pub fn shell_node() -> Point {
    (26, -8)
}
/// The envelope's storage node.
pub fn env_node() -> Point {
    (0, -8)
}
/// The OTAs' shared bias node: pitch, live.
pub fn bias_node() -> Point {
    (18, -12)
}
/// The mixer output the speaker hangs on.
pub fn out_node() -> Point {
    (54, -8)
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
pub fn bass_room_circuit() -> Vec<ElementSpec> {
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
        (-24, RAIL_Y),
        (-24, RAIL_Y + 4),
    );
    sh.ground((-24, RAIL_Y + 4), DOWN);
    sh.run(&[(-24, RAIL_Y), (-20, RAIL_Y), (0, RAIL_Y), (10, RAIL_Y)]);

    // ---------------------------------------------------- clock + trigger
    let clk = modules::clock_555(
        &mut sh,
        20,
        (-20, -16),
        (-20, RAIL_Y),
        CLK_RA,
        CLK_R_MIN,
        CLK_POT,
        TEMPO_WIPER,
        CLK_C,
    );
    debug_assert_eq!(clk, (-16, -13));
    sh.run(&[clk, (-12, -13)]);
    sh.two(30, r(R_TRIG), (-12, -13), (-4, -13));
    // The HIT button, straight off the rail through its own stopper: play it
    // by hand as well as by clock.
    sh.two(31, r(R_HIT), (0, RAIL_Y), (0, -18));
    sh.two(ID_HIT, K::Button { closed: false }, (0, -18), (0, -13));
    sh.wire((0, -13), (-4, -13));

    // ---------------------------------------------------------- envelope
    // One capacitor, three jobs: it pings the shell, it sweeps the pitch, and
    // through the impact network it is the click.
    sh.run(&[(-4, -13), (-4, -8)]);
    sh.two(33, K::Diode, (-4, -8), env_node());
    sh.two(34, cap(C_ENV), env_node(), (0, -4));
    sh.ground((0, -4), DOWN);
    // SWEEP: a rheostat between the envelope and the bias node.
    // pins [end a, wiper, end b]
    let sw = sh.part(ID_SWEEP, pot(POT_SWEEP, SWEEP_WIPER), (2, -8), E, 4, false);
    debug_assert_eq!(sw[1], (4, -10));
    debug_assert_eq!(sw[2], (6, -8));
    sh.wire(env_node(), (2, -8));
    sh.run(&[sw[1], (6, -10), sw[2]]);
    sh.run(&[sw[2], (8, -8), (8, -12), bias_node()]);

    // PITCH: a rheostat from the rail into the same bias node, so it sets the
    // resting current the sweep falls towards.
    let pp = sh.part(ID_PITCH, pot(POT_PITCH, PITCH_WIPER), (10, -16), E, 4, false);
    debug_assert_eq!(pp[1], (12, -18));
    debug_assert_eq!(pp[2], (14, -16));
    sh.run(&[(10, RAIL_Y), (10, -16)]);
    sh.run(&[pp[1], (14, -18), pp[2]]);
    sh.run(&[pp[2], (18, -16), bias_node()]);

    // ------------------------------------------------------------- shell
    // The two-integrator loop. A is the resonator node the output stage taps;
    // B is the second integrator. Only ONE damping resistor is needed: B is
    // not a free integrator, because the loop itself closes its DC path.
    // pins [in+, in-, out, bias]; A is mirrored so both bias pins face inward.
    let oa = sh.part(40, K::Ota, (22, -8), E, 4, true);
    let ob = sh.part(41, K::Ota, (22, 4), E, 4, false);
    let (a_plus, a_minus, ka, a_bias) = (oa[0], oa[1], oa[2], oa[3]);
    let (b_plus, b_minus, kb, b_bias) = (ob[0], ob[1], ob[2], ob[3]);
    debug_assert_eq!(ka, shell_node());
    debug_assert_eq!(a_bias, (25, -6));
    debug_assert_eq!(b_bias, (25, 2));
    sh.ground(a_minus, UP);
    sh.ground(b_plus, UP);
    sh.run(&[a_bias, b_bias]);
    sh.run(&[bias_node(), (18, -6), a_bias]);
    // The two integrating capacitors.
    sh.two(42, cap(C_SHELL), ka, (26, -12));
    sh.ground((26, -12), UP);
    sh.two(43, cap(C_SHELL), kb, (26, 8));
    sh.ground((26, 8), DOWN);
    // ...and the cross-coupling that makes it a loop: B back into A's
    // non-inverting input, A into B's inverting one.
    sh.run(&[kb, (30, 4), (30, -18), (20, -18), (20, -7), a_plus]);
    sh.run(&[ka, (34, -8), (34, -4), (34, 12), (18, 12), (18, 5), b_minus]);
    // DECAY across the resonator node.
    let dp = sh.part(ID_DECAY, pot(POT_DECAY, DECAY_WIPER), (34, -4), E, 4, false);
    debug_assert_eq!(dp[1], (36, -6));
    debug_assert_eq!(dp[2], (38, -4));
    sh.run(&[dp[1], (38, -6), dp[2]]);
    sh.ground(dp[2], RIGHT);
    // The mallet: the envelope capacitor, over the top, into the resonator.
    sh.run(&[env_node(), (0, -20), (36, -20), (36, -12)]);
    sh.two(45, cap(C_EXCITE), (36, -12), (36, -8));
    sh.run(&[(34, -8), (36, -8)]);

    // ------------------------------------------------------------ impact
    // Passive, as Henry's is: the trigger through a series resistor into a
    // rheostat to ground, then AC-coupled into the summing bus. The knob
    // attenuates and brightens together.
    sh.run(&[(-4, -8), (-4, 16)]);
    sh.two(50, r(R_IMPACT_IN), (-4, 16), (2, 16));
    let ip = sh.part(ID_IMPACT, pot(POT_IMPACT, IMPACT_WIPER), (2, 16), E, 4, false);
    debug_assert_eq!(ip[1], (4, 14));
    debug_assert_eq!(ip[2], (6, 16));
    sh.run(&[ip[1], (6, 14), ip[2]]);
    sh.ground(ip[2], RIGHT);
    sh.run(&[(2, 16), (2, 20)]);
    sh.two(52, cap(C_IMPACT), (2, 20), (6, 20));
    sh.two(53, r(R_IMPACT_OUT), (6, 20), (40, 20));

    // ------------------------------------------------- output + speaker
    // ONE op-amp is the whole output stage: in+ taps the resonator (op-amp
    // inputs draw exactly zero current in this model, so it is not loaded at
    // all) and in− is a virtual-earth bus the impact dumps into.
    //   v(out) = v(shell)·(1 + RF/RG) − I_impact·RF
    // pins [in+, in-, out]
    let op = sh.part(60, K::OpAmp { rail: SUPPLY_V, isc: sim_core::DEFAULT_OPAMP_ISC }, (48, -8), E, 6, false);
    let (plus, sum, out) = (op[0], op[1], op[2]);
    debug_assert_eq!(plus, (48, -9));
    debug_assert_eq!(sum, (48, -7));
    debug_assert_eq!(out, out_node());
    sh.run(&[(36, -8), (44, -8), (44, -9), plus]);
    sh.run(&[out, (54, -2)]);
    sh.two(61, r(R_F), (54, -2), (46, -2));
    sh.run(&[(46, -2), (46, -7), sum]);
    sh.run(&[sum, (48, -5)]);
    sh.two(62, r(R_G), (48, -5), (48, -1));
    sh.ground((48, -1), DOWN);
    sh.run(&[(40, 20), (44, 20), (44, -5), (48, -5)]);
    // Speaker id 1: the server streams the four lowest-id speakers.
    sh.wire(out, (56, -8));
    sh.two(ID_SPEAKER, K::Speaker { ohms: 8.0 }, (56, -8), (60, -8));
    sh.ground((60, -8), RIGHT);

    let mut els = sh.finish();
    name_controls(&mut els);
    els
}

/// The front-panel legend on every part a player can touch.
fn name_controls(els: &mut [ElementSpec]) {
    let named: &[(u32, &str)] = &[
        (ID_SUPPLY, "SUPPLY"),
        (ID_TEMPO, "TEMPO"),
        (ID_HIT, "HIT"),
        (ID_SWEEP, "SWEEP"),
        (ID_PITCH, "PITCH"),
        (ID_DECAY, "DECAY"),
        (ID_IMPACT, "IMPACT"),
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
pub fn bass_panels() -> Vec<crate::synth::PanelDef> {
    vec![crate::synth::PanelDef {
        x0: -28.0,
        y0: -28.0,
        x1: 64.0,
        y1: 26.0,
        name: "BASS++",
    }]
}

/// Block headings, plus the honesty plaque.
pub fn bass_label_boxes() -> Vec<crate::synth::PanelDef> {
    use crate::synth::PanelDef;
    let b = |x0: f64, y0: f64, x1: f64, y1: f64, name: &'static str| PanelDef { x0, y0, x1, y1, name };
    vec![
        // Signal left to right: clock, envelope, pitch, shell, out. The
        // boundaries are chosen so no two touch — see the overlap test.
        b(-25.0, -26.0, -6.0, -7.0, "CLOCK  TEMPO"),
        b(-1.0, -20.0, 9.0, -12.0, "HIT"),
        b(-5.0, -11.0, 8.0, -2.0, "ENVELOPE  SWEEP"),
        b(9.0, -20.0, 20.0, -14.0, "PITCH"),
        b(19.0, -14.0, 33.0, 10.0, "SHELL  GM-C RESONATOR"),
        b(33.0, -7.0, 43.0, -2.5, "DECAY"),
        b(-5.0, 12.0, 42.0, 22.5, "IMPACT  PASSIVE CLICK"),
        b(44.0, -12.0, 62.0, -2.0, "MIXER + SPEAKER"),
        // The plaque, EAST OF THE MIXER and clear of every block box. It sat
        // at x 20..52 y 14..37 first, straight through the IMPACT block and
        // the DECAY heading, and the room read as a pile of overlapping
        // rectangles. 28 characters a line is what a label box holds.
        b(66.0, -14.0, 98.0, -12.4, "AFTER THOMAS HENRY'S BASS++"),
        b(66.0, -12.0, 98.0, -10.4, "THE SHELL IS A REAL LM13700"),
        b(66.0, -10.0, 98.0, -8.4, "CLASS GM-C RESONATOR: TWO"),
        b(66.0, -8.0, 98.0, -6.4, "OTAS, TWO CAPS, PITCH FROM"),
        b(66.0, -6.0, 98.0, -4.4, "THE BIAS CURRENT AND DECAY"),
        b(66.0, -4.0, 98.0, -2.4, "FROM RC - TWO KNOBS, TWO"),
        b(66.0, -2.0, 98.0, -0.4, "POLES, NO SUBSTEP GRID. ONE"),
        b(66.0, 0.0, 98.0, 1.6, "ENVELOPE BOTH PINGS IT AND"),
        b(66.0, 2.0, 98.0, 3.6, "SWEEPS IT, AS HIS DOES."),
        b(66.0, 5.0, 98.0, 6.6, "STAND-IN: ONE OP-AMP DOES"),
        b(66.0, 7.0, 98.0, 8.6, "THE WHOLE OUTPUT STAGE WHERE"),
        b(66.0, 9.0, 98.0, 10.6, "BASS++ HAS A QUAD, AND THE"),
        b(66.0, 11.0, 98.0, 12.6, "SHELL'S OWN DAMPING IS THE"),
        b(66.0, 13.0, 98.0, 14.6, "LEVEL ENVELOPE, SO THERE IS"),
        b(66.0, 15.0, 98.0, 16.6, "NO SEPARATE VCA. THE TRIGGER"),
        b(66.0, 17.0, 98.0, 18.6, "IS A 555 PLUS A HIT BUTTON;"),
        b(66.0, 19.0, 98.0, 20.6, "HIS TAKES AN EXTERNAL GATE."),
    ]
}

/// No label box may overlap another — the room is read as a schematic, and
/// two headings on the same square are a smear, not a diagram.
#[cfg(test)]
fn boxes_overlap(a: &crate::synth::PanelDef, b: &crate::synth::PanelDef) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

// ------------------------------------------------------------ SCOPE NOTES
//
// Filled in by the measurement tests below and `roombench.rs`.

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

    /// Every part a legal shape, every wire orthogonal, every id unique. The
    /// id check is not boilerplate: `modules::clock_555` claims a block of
    /// ids from its `id0`, and a room that reuses one silently loses a part.
    #[test]
    fn bass_room_is_a_legal_document() {
        let els = bass_room_circuit();
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

    /// A panel row reading POT #405 is the bug the naming feature fixed.
    /// Every part a player can turn carries a legend, and the speaker is at
    /// a low id because the server streams the four lowest.
    #[test]
    fn bass_controls_are_named_and_reachable() {
        let els = bass_room_circuit();
        for id in [ID_TEMPO, ID_HIT, ID_SWEEP, ID_PITCH, ID_DECAY, ID_IMPACT] {
            let e = els.iter().find(|e| e.id == id).expect("control missing");
            assert!(!e.name.is_empty(), "control {id} has no name");
        }
        assert!(ID_SPEAKER <= 4, "speaker must be in the streamed low ids");
    }

    /// The sheet reads as a SCHEMATIC, which means no two label boxes may
    /// sit on top of each other. Not a style rule: this room first shipped
    /// with its honesty plaque laid straight across the IMPACT block and the
    /// DECAY heading, and the whole east half read as a pile of rectangles.
    #[test]
    fn bass_label_boxes_do_not_overlap() {
        let boxes = bass_label_boxes();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(
                    !boxes_overlap(a, b),
                    "label boxes overlap: {:?} ({}, {}, {}, {}) vs {:?} ({}, {}, {}, {})",
                    a.name, a.x0, a.y0, a.x1, a.y1, b.name, b.x0, b.y0, b.x1, b.y1
                );
            }
        }
    }

    /// The room plays. It must never quarantine, and the speaker must
    /// actually move — a drum that is silent between hits is correct, so
    /// this measures across a whole clock period.
    #[test]
    fn bass_room_never_quarantines_and_plays() {
        let els = bass_room_circuit();
        let mut e = Engine::new(DT);
        e.set_elements(&els);
        e.advance(20_000);
        assert!(!e.is_quarantined(), "quarantined during warmup");
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for _ in 0..100_000 {
            e.advance(1);
            let v = e.voltage_at(out_node()).unwrap_or(0.0);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(!e.is_quarantined(), "quarantined while playing");
        assert!(hi - lo > 0.02, "the room is silent: {:.4} Vpp", hi - lo);
    }

    /// The design claim, tested: ONE envelope does TWO jobs. The storage
    /// capacitor must both ping the shell and pull the bias node, so a hit
    /// has to show up on the bias node as a real excursion. If SWEEP does
    /// nothing the room is a fixed-pitch drum with a decorative knob.
    #[test]
    fn bass_sweep_moves_the_bias_node() {
        let span = |w: f64| {
            let mut els = bass_room_circuit();
            tweak(ID_SWEEP, w, &mut els);
            let mut e = Engine::new(DT);
            e.set_elements(&els);
            e.advance(20_000);
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for _ in 0..60_000 {
                e.advance(1);
                let v = e.voltage_at(bias_node()).unwrap_or(0.0);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert!(!e.is_quarantined(), "quarantined at SWEEP {w}");
            hi - lo
        };
        let (off, on) = (span(0.999), span(0.02));
        // The pot is a rheostat from the envelope INTO the bias node: at the
        // far end of the track it is 5 MΩ of isolation and almost nothing
        // gets through; near zero it is a short and the whole envelope lands.
        assert!(
            on > off * 3.0,
            "SWEEP does not sweep: {on:.4} V at full vs {off:.4} V at none"
        );
    }

    /// DECAY damps and PITCH tunes, and they are SEPARATE — that separation
    /// is the whole reason Henry's panel has two knobs. Turning DECAY must
    /// change how long the shell rings without being the pitch control.
    #[test]
    fn bass_decay_changes_the_ring_length() {
        let tail = |w: f64| {
            let mut els = bass_room_circuit();
            tweak(ID_DECAY, w, &mut els);
            let mut e = Engine::new(DT);
            e.set_elements(&els);
            // Settle, then find a hit and measure how long the shell stays
            // above a tenth of its own peak.
            e.advance(20_000);
            let mut peak = 0.0f64;
            let mut samples = Vec::with_capacity(30_000);
            for _ in 0..30_000 {
                e.advance(1);
                let v = e.voltage_at(shell_node()).unwrap_or(0.0).abs();
                peak = peak.max(v);
                samples.push(v);
            }
            assert!(!e.is_quarantined(), "quarantined at DECAY {w}");
            samples.iter().filter(|v| **v > peak * 0.1).count() as f64 * DT
        };
        let (short, long) = (tail(0.02), tail(0.9));
        assert!(
            long > short * 1.5,
            "DECAY does not change the ring: {short:.3} s vs {long:.3} s"
        );
    }

    /// Every knob, everywhere, without the solver falling over. The damage
    /// gate winds pots to their ends, so the room has to survive there too.
    #[test]
    fn bass_survives_every_knob_position() {
        for pw in [0.01, 0.5, 0.99] {
            for dw in [0.01, 0.5, 0.99] {
                for sw in [0.01, 0.99] {
                    let mut els = bass_room_circuit();
                    tweak(ID_PITCH, pw, &mut els);
                    tweak(ID_DECAY, dw, &mut els);
                    tweak(ID_SWEEP, sw, &mut els);
                    let mut e = Engine::new(DT);
                    e.set_elements(&els);
                    for _ in 0..100 {
                        e.advance(500);
                        assert!(
                            !e.is_quarantined(),
                            "quarantined at PITCH {pw} DECAY {dw} SWEEP {sw}"
                        );
                    }
                }
            }
        }
    }

    /// Ring frequency of the shell in a window `[from, to)` substeps after
    /// the NEXT hit, counting zero crossings of the capacitor's own voltage.
    ///
    /// SYNCHRONISING ON THE HIT is the whole method, and the first version of
    /// this measurement did not: it counted crossings in a fixed window from
    /// an arbitrary start, so every window held a mix of fresh attack and old
    /// tail and the sweep averaged itself away to nothing (2640 → 2630 Hz for
    /// a shell that really moves several hundred). Find the envelope's rising
    /// edge first, then measure from there.
    fn ring_after_hit(els: &[ElementSpec], from: u32, to: u32) -> f64 {
        let mut e = Engine::new(DT);
        e.set_elements(els);
        e.advance(20_000);
        // Wait for the envelope to fall back down, then catch the next edge.
        let mut armed = false;
        let mut found = false;
        for _ in 0..200_000u32 {
            e.advance(1);
            let env = e.voltage_at(env_node()).unwrap_or(0.0);
            if env < 0.5 {
                armed = true;
            } else if armed && env > 2.0 {
                found = true;
                break;
            }
        }
        assert!(found, "no hit inside 4 s — is the clock running?");
        let node = |e: &Engine| {
            e.voltage_at(shell_node()).unwrap_or(0.0) - e.voltage_at((26, -12)).unwrap_or(0.0)
        };
        for _ in 0..from {
            e.advance(1);
        }
        let mut prev = node(&e);
        let mut crossings = 0u32;
        for _ in from..to {
            e.advance(1);
            let v = node(&e);
            if prev <= 0.0 && v > 0.0 {
                crossings += 1;
            }
            prev = v;
        }
        f64::from(crossings) / (f64::from(to - from) * DT)
    }

    /// Peak level at the speaker across the knobs, against the op-amp's own
    /// hard ceiling. `DEFAULT_OPAMP_ISC` is 25 mA into 8 Ω, so 0.2 V is not a
    /// design target but a physical wall: past it the branch stops being a
    /// voltage source and a drum comes out a square. Aim the loudest
    /// coincidence near 0.15 V.
    #[test]
    #[ignore = "measurement: cargo test --release -p server bass_output_level -- --ignored --nocapture"]
    fn bass_output_level() {
        println!("\n== BASS++ peak at the speaker (op-amp wall is 0.200 V) ==");
        for (pw, dw, iw) in [
            (0.52, 0.24, 0.50),
            (0.52, 0.99, 0.50),
            (0.01, 0.24, 0.50),
            (0.99, 0.99, 0.99),
            (0.20, 0.99, 0.01),
        ] {
            let mut els = bass_room_circuit();
            tweak(ID_PITCH, pw, &mut els);
            tweak(ID_DECAY, dw, &mut els);
            tweak(ID_IMPACT, iw, &mut els);
            let mut e = Engine::new(DT);
            e.set_elements(&els);
            e.advance(20_000);
            let mut peak = 0.0f64;
            for _ in 0..100_000 {
                e.advance(1);
                peak = peak.max(e.voltage_at(out_node()).unwrap_or(0.0).abs());
            }
            println!("  PITCH {pw:.2} DECAY {dw:.2} IMPACT {iw:.2} | peak {peak:.4} V");
        }
    }

    /// The measurement that writes the scope notes: what PITCH actually
    /// does, in hertz, from the attack into the tail.
    #[test]
    #[ignore = "measurement: cargo test --release -p server bass_pitch_table -- --ignored --nocapture"]
    fn bass_pitch_table() {
        println!("\n== BASS++ shell: PITCH against the ring, synced to the hit ==");
        println!("  (attack = 0-40 ms after the trigger, tail = 120-300 ms)");
        for pw in [0.01, 0.05, 0.20, 0.52, 0.99] {
            let mut els = bass_room_circuit();
            tweak(ID_PITCH, pw, &mut els);
            let attack = ring_after_hit(&els, 0, 2_000);
            let tail = ring_after_hit(&els, 6_000, 15_000);
            println!("  PITCH {pw:.2} | attack {attack:>7.1} Hz -> tail {tail:>7.1} Hz");
        }
    }

    /// The sweep is REAL and it is DOWNWARD — the pitch envelope is what
    /// makes this a drum rather than a tuned ping, and `bass_sweep_moves_the
    /// _bias_node` only proves the bias node moves, not that the shell
    /// follows it. Measured on the shell itself, synced to a hit.
    #[test]
    fn bass_pitch_sweeps_downward_after_a_hit() {
        let els = bass_room_circuit();
        let attack = ring_after_hit(&els, 0, 2_000);
        let tail = ring_after_hit(&els, 6_000, 15_000);
        assert!(
            attack > tail * 1.15,
            "the shell does not sweep: {attack:.1} Hz attack vs {tail:.1} Hz tail"
        );
    }
}
