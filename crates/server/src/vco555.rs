//! THE 555-VCO ROOM — after Thomas Henry's "555 VCO".
//!
//! The historical instrument: Thomas Henry published the 555-VCO in the
//! synth-DIY press (Midwest Analog / birthofasynth.com lineage): a matched
//! transistor pair's exponential converter sources current into a timing
//! capacitor, and a 555's threshold comparator plus its discharge transistor
//! snap the cap back — a sawtooth-core VCO with real 1 V/octave tracking,
//! built round the cheapest chip in the drawer. That is the design this room
//! carries: the 555 in this engine has exactly the THR/TRIG/DIS pins the
//! core needs, and it is a DISCRETE nonlinearity, so the comparator costs
//! Newton nothing.
//!
//! What is faithful and what is a stand-in:
//!   * FAITHFUL: the sawtooth core itself — an exponential control current
//!     charging a cap, the 555 resetting it through DIS. The saw and the
//!     pulse both come off the real circuit, and every number is solved.
//!   * STAND-IN: the exponential converter is this engine's measured
//!     OTA-bias-diode converter (`synth_vco.rs`: the OTA's bias pin is a
//!     diode junction, so an octave is 17.9 mV at the pin) rather than a
//!     matched NPN pair with a tempco resistor. Same junction physics, same
//!     1 V/oct law, two fewer smooth transistors — and this simulator's
//!     junctions do not drift with temperature, so the tempco would trim
//!     nothing.
//!   * LIMIT: a comparator only flips on a substep boundary, so pitch sits
//!     on a `50 kHz / n` grid. A 5 mV noise dither under the timing cap
//!     restores a continuous average (the `synth_vco.rs` measurement); the
//!     in-tune range here is ~55–900 Hz, drifting above that.
//!
//! ## Signal flow
//!
//! ```text
//!  PITCH pot ─ follower ─> expo converter ─> OTA current ──> C ──> SAW
//!                              ^                        555 THR/DIS ┘ (reset)
//!  VIBRATO 555 LFO ── cap ── R ┘             SAW ─ follower ─ R ──┐
//!                                            PULSE ─ sw ──── R ──┴─> mixer ─> speaker
//! ```
//!
//! SCOPE NOTES (measured on this machine, Apple M4, release, pinned 1.95.0,
//! other agents building in parallel — see `synth.rs` for the method):
//!
//!   The saw spans a measured 0.99..6.00 V (see `R_DIS` for why the bottom
//!   is not Vcc/3) and the pitch law is `51 · 2^CV` Hz: measured 71.8 /
//!   121.1 / 287.4 / 675.7 / 1111 Hz at CV 0.5 / 1.25 / 2.5 / 3.75 / 4.5.
//!   1 V/octave to a few cents through ~700 Hz; the fixed two-substep
//!   retrace compresses the top of the knob by ~60 cents. Default PITCH
//!   0.50 = 287 Hz, D4-ish.
//!
//!   Cost: see `room_realtime_budgets` in `roombench.rs` — 28 devices,
//!   measured 1.72 µs/substep = 11.6x real time offline, NR 1.00: the 555s
//!   and the op-amps are discrete nonlinearities and the OTA's pins all sit
//!   on DC, so nothing here buys Newton iterations. Live over a websocket:
//!   rt median 1.000 (see the shipping report).
//!
//!   The vibrato LFO loads its own ramp through the coupling network — the
//!   same measured interaction as `synth_vco::lfo_555` — so RATE is trimmed
//!   by ear, not by the astable formula.

use sim_core::{ElementKind as K, ElementSpec};

use crate::layout::{Sheet, DOWN, E, LEFT, N, RIGHT, UP};
use crate::synth_vco::{R_GND, R_OFF, R_SCALE, R_SPAN};

// ---------------------------------------------------------------- values

/// Supply. Same 9 V convention as every synth room.
const SUPPLY_V: f64 = 9.0;
/// Timing cap. The ramp does NOT span Vcc/3..2Vcc/3 the way a continuous
/// 555 astable's does — see `R_DIS` — it spans a MEASURED 0.99..6.00 V, so
/// `f = Iabc / (C · 5.0 V)` and the law lands at `51 · 2^CV` Hz: measured
/// 71.8 / 121.1 / 287.4 / 675.7 / 1111 Hz at CV 0.5 / 1.25 / 2.5 / 3.75 /
/// 4.5, which is 1 V/octave to a few cents up to ~700 Hz and ~60 cents
/// compressed at the very top (the fixed two-substep retrace).
const C_TIME: f64 = 220e-12;
/// Discharge resistor between DIS and the cap, and the one place this core
/// differs from the paper design. The 555's TRIG comparator only releases
/// on a substep boundary, so the discharge cannot stop AT Vcc/3: whatever
/// the cap has fallen to when the boundary arrives is the ramp's bottom.
/// A big R_DIS (say 600 k) would stop it just under 3 V but spend seven
/// substeps retracing — a fixed 140 µs that flattens the top of the range
/// badly; a tiny one would dump the cap inside a substep against the
/// trapezoidal integrator. 62 k makes the discharge time constant about
/// two thirds of a substep: the retrace is two substeps, the bottom is the
/// DETERMINISTIC trapezoidal value ~0.99 V, and the span — and with it the
/// pitch law — is stable cycle to cycle (measured 0.97..1.08 V across the
/// whole knob).
const R_DIS: f64 = 62e3;
/// Threshold dither under the timing cap: randomizes which substep the
/// comparator crossing lands on, so the average pitch is continuous
/// (`synth_vco.rs`, measured 2.9 cents worst over a chromatic octave).
const NOISE_DITHER_V: f64 = 0.005;
/// Vibrato LFO: 100 k fixed charge resistor, 1 M rate rheostat, 470 n cap.
const LFO_RA: f64 = 100e3;
const LFO_POT: f64 = 1e6;
const LFO_C: f64 = 0.47e-6;
/// Vibrato depth: the LFO ramp's 3 Vpp through this into the bias pin's
/// 150 Ω Thevenin is about ±20 cents.
const R_VIB: f64 = 680e3;
/// Mixer. Rf against 220 k (saw) and 470 k (pulse) keeps the speaker under
/// 0.1 W with both waves on — there is deliberately NO level pot before the
/// speaker (the `synth.rs` damage rule: the gate winds every pot to 0.98).
const R_F: f64 = 100e3;
const R_SAW: f64 = 220e3;
const R_PULSE: f64 = 470e3;

/// Ids a player touches, named for the panel and the tests.
pub const ID_SPEAKER: u32 = 1;
pub const ID_SUPPLY: u32 = 2;
pub const ID_PITCH: u32 = 10;
pub const ID_PULSE_SW: u32 = 34;
pub const ID_RATE: u32 = 52;

/// Default knob positions. PITCH 0.5 = CV 2.5 V ≈ 314 Hz.
pub const PITCH_WIPER: f64 = 0.50;
pub const RATE_WIPER: f64 = 0.20;

/// The SAW node (the timing cap's top plate) — what a scope listens to when
/// it wants the oscillator itself.
pub fn saw_node() -> sim_core::Point {
    (19, 2)
}

/// The mixer output the speaker hangs on.
pub fn out_node() -> sim_core::Point {
    (42, 8)
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
fn opamp(rail: f64) -> K {
    K::OpAmp { rail, isc: sim_core::DEFAULT_OPAMP_ISC }
}

/// The 555-VCO room.
pub fn vco555_room_circuit() -> Vec<ElementSpec> {
    let mut sh = Sheet::new(100);

    // ------------------------------------------------------------- supply
    // One rail line along the top with a corner at every drop, and one
    // margin column down the left for the LFO.
    sh.run(&[
        (-8, -8),
        (-6, -8),
        (2, -8),
        (12, -8),
        (20, -8),
        (23, -8),
    ]);
    sh.two(ID_SUPPLY, K::VoltageSource { dc: SUPPLY_V, amp: 0.0, hz: 0.0, phase: 0.0, wave: sim_core::Wave::Sine }, (-8, -8), (-8, -4));
    sh.ground((-8, -4), DOWN);

    // ------------------------------------------------------- PITCH -> CV
    // Rail -> span resistor -> pot -> unity follower. The follower is what
    // makes the 1 V/oct law hold: driving the scale resistor from a bare
    // wiper was measured at 844 cents of error (`synth_vco.rs`).
    sh.two(3, r(R_SPAN), (2, -8), (2, -4));
    // pins [end a = ground, wiper, end b = span]
    let p = sh.part(ID_PITCH, pot(100_000.0, PITCH_WIPER), (2, 0), N, 4, true);
    debug_assert_eq!(p[1], (4, -2));
    debug_assert_eq!(p[2], (2, -4));
    sh.ground(p[0], DOWN);
    // pins [in+, in-, out]
    let f = sh.part(11, opamp(SUPPLY_V), (6, -2), E, 4, false);
    let (fin, fneg, cv) = (f[0], f[1], f[2]);
    debug_assert_eq!(cv, (10, -2));
    sh.run(&[p[1], (4, -3), fin]);
    sh.run(&[cv, (10, -1), fneg]);

    // -------------------------------------------------- expo converter
    // Three resistors on the OTA's bias pin (measured values from
    // `synth_vco.rs`): `Iabc = 55 nA · 2^CV`, an octave is 17.9 mV at the
    // pin, and the divider's 150 Ω Thevenin is what keeps it a law.
    sh.wire(cv, (13, -2));
    sh.two(12, r(R_SCALE), (13, -2), (17, -2));
    sh.two(13, r(R_OFF), (20, -8), (20, -4));
    // pins [in+, in-, out, bias]
    let ota = sh.part(20, K::Ota, (14, 2), E, 4, false);
    let (plus, minus, saw, bias) = (ota[0], ota[1], ota[2], ota[3]);
    debug_assert_eq!(bias, (17, 0));
    debug_assert_eq!(saw, (18, 2));
    sh.run(&[(17, -2), (17, -1), bias]);
    sh.run(&[(20, -4), (20, 0), bias]);
    sh.two(14, r(R_GND), bias, (13, 0));
    sh.ground((13, 0), LEFT);
    // in+ to the rail: vd = 9 V saturates the tanh, so the OTA is a pure
    // current source of +Iabc into the cap while the 555 lets it charge.
    sh.run(&[(12, -8), (12, 1), plus]);
    sh.ground(minus, DOWN);

    // ----------------------------------------------------- the saw core
    // The timing cap, with the pitch dither under it: the noise wiggles the
    // ramp ±5 mV so the threshold crossing does not lock to the substep grid.
    sh.two(21, cap(C_TIME), (19, 2), (19, 4));
    sh.two(
        22,
        K::Noise { volts: NOISE_DITHER_V, ohms: 1000.0, seed: 0x0555_0555 },
        (19, 4),
        (19, 6),
    );
    sh.ground((19, 6), DOWN);
    // The 555: THR and TRIG both watch the saw; DIS resets it through R_DIS.
    // pins [vcc, gnd, trg, thr, out, dis]
    let dip = sh.part(23, K::Timer555, (24, 6), E, 4, true);
    let (vcc, tgnd, trg, thr, pulse, dis) = (dip[0], dip[1], dip[2], dip[3], dip[4], dip[5]);
    debug_assert_eq!(pulse, (28, 3));
    debug_assert_eq!(dis, (28, 5));
    sh.run(&[(23, -8), (23, 6), vcc]);
    sh.ground(tgnd, UP);
    // The saw bus, and the TRIG-to-THR jumper down the chip's left side.
    sh.run(&[saw, (19, 2), (20, 2), (22, 2), (22, 3), thr]);
    sh.run(&[trg, (22, 5), (22, 3)]);
    // DIS back to the saw through the discharge resistor, over the top.
    sh.run(&[dis, (28, 7)]);
    sh.two(24, r(R_DIS), (28, 7), (24, 7));
    sh.run(&[(24, 7), (22, 7), (22, 5)]);

    // ------------------------------------------------------ wave mixing
    // The saw is a bare integrator node: buffer it before it drives
    // anything resistive, or the mixer's input current would detune it.
    // pins [in+, in-, out]
    let buf = sh.part(30, opamp(SUPPLY_V), (20, 8), E, 4, false);
    let (bin, bneg, bout) = (buf[0], buf[1], buf[2]);
    sh.run(&[(20, 2), bin]);
    sh.run(&[bout, (24, 9), bneg]);
    sh.two(31, cap(1e-6), (25, 8), (28, 8));
    sh.wire(bout, (25, 8));
    sh.two(32, r(R_SAW), (28, 8), (34, 8));
    sh.run(&[(34, 8), (34, 7), (35, 7), (36, 7)]);
    // The pulse leg: the 555's own output, AC-coupled, behind a toggle.
    sh.run(&[pulse, (31, 3)]);
    sh.two(33, cap(1e-6), (31, 3), (31, 6));
    sh.two(ID_PULSE_SW, K::Switch { closed: false }, (31, 6), (33, 6));
    sh.two(35, r(R_PULSE), (33, 6), (35, 6));
    sh.run(&[(35, 6), (35, 7)]);
    // Bleed: with the toggle open the cap-switch node would float on GMIN.
    sh.two(36, r(1e6), (31, 6), (31, 9));
    sh.ground((31, 9), DOWN);

    // -------------------------------------------------- mixer + speaker
    // The virtual-ground current mixer from `synth.rs`, facing the other
    // way: both wave legs dump current into the op-amp's inverting input
    // and v(out) = -I·Rf. Speaker id 1: the server streams the four
    // lowest-id speakers, so the instrument can never be crowded out.
    // pins [in+, in-, out]
    let mix = sh.part(40, opamp(SUPPLY_V), (36, 8), E, 6, true);
    debug_assert_eq!(mix[1], (36, 7));
    debug_assert_eq!(mix[2], (42, 8));
    sh.ground(mix[0], DOWN);
    sh.run(&[(42, 8), (42, 5)]);
    sh.two(41, r(R_F), (42, 5), (38, 5));
    sh.run(&[(38, 5), (34, 5), (34, 7)]);
    sh.wire((42, 8), (44, 8));
    sh.two(ID_SPEAKER, K::Speaker { ohms: 8.0 }, (44, 8), (48, 8));
    sh.ground((48, 8), RIGHT);

    // ------------------------------------------------- vibrato 555 LFO
    // A second 555, slow: its timing-cap ramp is the vibrato, AC-coupled
    // into the expo bias node. `R_VIB` into the pin's 150 Ω Thevenin is
    // about ±20 cents of wobble.
    // pins [vcc, gnd, trg, thr, out, dis]
    let lfo = sh.part(50, K::Timer555, (-2, 18), E, 4, true);
    let (lvcc, lgnd, ltrg, lthr, _lout, ldis) = (lfo[0], lfo[1], lfo[2], lfo[3], lfo[4], lfo[5]);
    sh.run(&[(-6, -8), (-6, 18), lvcc]);
    sh.ground(lgnd, UP);
    // TRIG-to-THR jumper down the chip's left side.
    sh.run(&[ltrg, (-3, 17), (-3, 15), lthr]);
    // Charge path: rail -> RA -> DIS, RATE rheostat back to the cap.
    sh.run(&[(-6, 18), (-6, 20), (2, 20)]);
    sh.two(51, r(LFO_RA), (2, 20), (2, 17));
    debug_assert_eq!(ldis, (2, 17));
    sh.wire(ldis, (4, 17));
    // pins [end a, wiper, end b]; rheostat — wiper strapped to end b.
    let rp = sh.part(ID_RATE, pot(LFO_POT, RATE_WIPER), (4, 17), E, 4, false);
    sh.run(&[rp[1], (8, 15), (8, 17)]);
    debug_assert_eq!(rp[2], (8, 17));
    // The wiper side back to the timing node, along the row above the chip.
    sh.run(&[rp[1], (6, 13), (-3, 13), (-3, 15)]);
    // The timing cap itself, off that row's far corner.
    sh.two(53, cap(LFO_C), (-3, 13), (-5, 13));
    sh.ground((-5, 13), DOWN);
    // The vibrato tap: ramp -> cap -> depth resistor -> up to the bias pin.
    sh.two(54, cap(1e-6), (6, 13), (9, 13));
    sh.two(55, r(R_VIB), (9, 13), (9, 7));
    sh.run(&[(9, 7), (9, -5), (17, -5), (17, -2)]);

    let mut els = sh.finish();
    name_controls(&mut els);
    els
}

/// The front-panel legend on the parts a player touches.
fn name_controls(els: &mut [ElementSpec]) {
    let named: &[(u32, &str)] = &[
        (ID_SUPPLY, "SUPPLY"),
        (ID_PITCH, "PITCH"),
        (ID_PULSE_SW, "PULSE ON"),
        (ID_RATE, "VIBRATO RATE"),
    ];
    for e in els.iter_mut() {
        if let Some((_, n)) = named.iter().find(|(id, _)| *id == e.id) {
            e.name = (*n).to_string();
        }
    }
}

/// ONE control panel spanning the instrument (see `synth.rs` for why one).
pub fn vco555_panels() -> Vec<crate::synth::PanelDef> {
    vec![crate::synth::PanelDef {
        x0: -12.0,
        y0: -12.0,
        x1: 52.0,
        y1: 24.0,
        name: "555-VCO",
    }]
}

/// Block headings, plus the honesty plaque: what the real instrument was
/// and what here is faithful versus a stand-in, on the sheet itself.
pub fn vco555_label_boxes() -> Vec<crate::synth::PanelDef> {
    use crate::synth::PanelDef;
    let b = |x0: f64, y0: f64, x1: f64, y1: f64, name: &'static str| PanelDef {
        x0,
        y0,
        x1,
        y1,
        name,
    };
    vec![
        b(0.5, -6.5, 11.0, 1.5, "PITCH  1V/OCT"),
        b(11.5, -3.2, 21.5, 1.2, "EXPO CONVERTER"),
        b(12.5, 1.5, 29.5, 10.0, "SAW CORE  555 RESET"),
        b(29.8, 2.0, 36.0, 10.0, "WAVE MIX"),
        b(36.5, 4.2, 49.0, 10.5, "SPEAKER"),
        b(-7.5, 12.2, 11.5, 21.0, "VIBRATO  555 LFO"),
        // The plaque. 28 characters a line is the label-box budget.
        b(24.0, 12.5, 49.0, 14.0, "AFTER THOMAS HENRY'S"),
        b(24.0, 14.2, 49.0, 15.7, "555-VCO: EXPO CURRENT INTO"),
        b(24.0, 15.9, 49.0, 17.4, "A CAP, 555 SNAPS IT BACK."),
        b(24.0, 17.6, 49.0, 19.1, "OTA STANDS IN FOR HIS"),
        b(24.0, 19.3, 49.0, 20.8, "MATCHED PAIR. IN TUNE TO"),
        b(24.0, 21.0, 49.0, 22.5, "~900 HZ (SUBSTEP GRID)."),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Engine;

    const DT: f64 = 20e-6;

    /// Alive from boot: the saw runs, the LFO runs, nothing quarantines.
    #[test]
    fn vco555_room_never_quarantines_and_plays() {
        let els = vco555_room_circuit();
        let mut eng = Engine::new(DT);
        eng.set_elements(&els);
        let mut rescues = 0;
        for _ in 0..300 {
            let rep = eng.advance(500); // 10 ms chunks, 3 s total
            rescues += rep.rescues;
            assert!(!eng.is_quarantined(), "quarantined at t={:.3}", eng.time());
        }
        assert_eq!(rescues, 0, "rescue steps while idling");
        // The saw is really sawing: resets at substep resolution for 0.5 s.
        let mut resets = 0u32;
        let mut last = eng.voltage_at(saw_node()).unwrap_or(0.0);
        for _ in 0..25_000 {
            eng.advance(1);
            let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
            if last - v > 1.0 {
                resets += 1;
            }
            last = v;
        }
        assert!(resets > 60, "the saw only reset {resets} times in 0.5 s");
        // And the speaker is actually being driven.
        let mut rms = 0.0;
        let n = 25_000;
        for _ in 0..n {
            eng.advance(1);
            let v = eng.voltage_at(out_node()).unwrap_or(0.0);
            rms += v * v;
        }
        let rms = (rms / n as f64).sqrt();
        assert!(
            rms > 0.05 && rms < 2.0,
            "speaker rms {rms:.3} V — silent or dangerously loud"
        );
    }

    /// TEMP debug: ramp span and key node voltages.
    #[test]
    #[ignore]
    fn vco555_debug_nodes() {
        let els = vco555_room_circuit();
        let mut eng = Engine::new(DT);
        eng.set_elements(&els);
        eng.advance(50_000);
        println!(
            "cv={:?} bias={:?} span={:?} vcc={:?}",
            eng.voltage_at((10, -2)),
            eng.voltage_at((17, 0)),
            eng.voltage_at((2, -4)),
            eng.voltage_at((24, 6)),
        );
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for _ in 0..20_000 {
            eng.advance(1);
            let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        println!("saw spans {lo:.3} .. {hi:.3}");
        // Charging slope, directly.
        let mut dvs: Vec<f64> = Vec::new();
        let mut last = eng.voltage_at(saw_node()).unwrap_or(0.0);
        for _ in 0..2000 {
            eng.advance(1);
            let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
            if v > last {
                dvs.push((v - last) / DT);
            }
            last = v;
        }
        dvs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "charging slope median {:.0} V/s (I/C would be {:.0} V/s at Iabc 311 nA)",
            dvs[dvs.len() / 2],
            311e-9 / C_TIME
        );
        // Sweep the knob: span and frequency per wiper.
        for wiper in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let mut els = vco555_room_circuit();
            for e in els.iter_mut() {
                if e.id == ID_PITCH {
                    if let K::Potentiometer { wiper: w, .. } = &mut e.kind {
                        *w = wiper;
                    }
                }
            }
            let mut eng = Engine::new(DT);
            eng.set_elements(&els);
            eng.advance(50_000);
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut resets = 0u32;
            let mut last = eng.voltage_at(saw_node()).unwrap_or(0.0);
            let n = 50_000;
            for _ in 0..n {
                eng.advance(1);
                let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
                lo = lo.min(v);
                hi = hi.max(v);
                if last - v > 1.0 {
                    resets += 1;
                }
                last = v;
            }
            // Period histogram: substeps between successive big drops.
            let mut periods: Vec<u32> = Vec::new();
            let mut since = 0u32;
            let mut last = eng.voltage_at(saw_node()).unwrap_or(0.0);
            for _ in 0..20_000 {
                eng.advance(1);
                let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
                since += 1;
                if last - v > 1.0 {
                    periods.push(since);
                    since = 0;
                }
                last = v;
            }
            println!(
                "wiper {wiper}: cv {:.3} span {lo:.3}..{hi:.3} f {:.1} Hz periods {:?}",
                eng.voltage_at((10, -2)).unwrap_or(0.0),
                f64::from(resets) / (n as f64 * DT),
                &periods[..periods.len().min(8)]
            );
        }
    }

    /// The 1 V/octave law through the 555 core, at three knob positions.
    #[test]
    fn vco555_tracks_a_volt_per_octave() {
        for (wiper, want) in [(0.25, 121.0), (0.50, 287.0), (0.75, 676.0)] {
            let mut els = vco555_room_circuit();
            for e in els.iter_mut() {
                if e.id == ID_PITCH {
                    if let K::Potentiometer { wiper: w, .. } = &mut e.kind {
                        *w = wiper;
                    }
                }
            }
            let mut eng = Engine::new(DT);
            eng.set_elements(&els);
            eng.advance(50_000);
            assert!(!eng.is_quarantined());
            // Count full periods between downward resets over one second.
            // The retrace is TWO substeps (see `R_DIS`), so a refractory
            // window keeps one retrace from counting twice.
            let n = 50_000;
            let mut resets = 0u32;
            let mut since = 100u32;
            let mut last = eng.voltage_at(saw_node()).unwrap_or(0.0);
            for _ in 0..n {
                eng.advance(1);
                let v = eng.voltage_at(saw_node()).unwrap_or(0.0);
                since += 1;
                if last - v > 1.0 && since > 4 {
                    resets += 1;
                    since = 0;
                }
                last = v;
            }
            let hz = f64::from(resets) / (n as f64 * DT);
            let cents = 1200.0 * (hz / want).log2();
            assert!(
                cents.abs() < 60.0,
                "wiper {wiper}: {hz:.1} Hz, {cents:+.0} cents from {want} Hz"
            );
        }
    }

    /// Every part is a legal shape and every wire orthogonal — the room is a
    /// document the editor itself would accept.
    #[test]
    fn vco555_room_is_a_legal_document() {
        for e in vco555_room_circuit() {
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
        // No two devices share an id (routing pool starts at 100).
        let els = vco555_room_circuit();
        let mut ids: Vec<u32> = els.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), els.len(), "duplicate element id");
    }

    /// Controls are named and inside the one panel.
    #[test]
    fn vco555_controls_are_named_and_reachable() {
        let els = vco555_room_circuit();
        let panels = vco555_panels();
        assert_eq!(panels.len(), 1);
        let p = &panels[0];
        for e in &els {
            if matches!(e.kind, K::Potentiometer { .. } | K::Switch { .. }) {
                assert!(!e.name.trim().is_empty(), "control {} unnamed", e.id);
                for (x, y) in &e.pins {
                    let (x, y) = (*x as f64, *y as f64);
                    assert!(
                        x >= p.x0 && x <= p.x1 && y >= p.y0 && y <= p.y1,
                        "control {} outside the panel",
                        e.id
                    );
                }
            }
        }
    }
}
