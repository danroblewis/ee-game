//! DRUM VOICES for the synth world — a kick, a snare and a hi-hat built
//! out of real components, driven by the solver and nothing else.
//!
//! Everything here was designed against the live engine and MEASURED; the
//! numbers in the doc comments are what the solver produced, not what the
//! textbook promises.
//!
//! ## Architecture
//!
//! ```text
//!   555 astable ──10k──> TRIGGER BUS ──┬─[gate]─> KICK   trig
//!   (RB bypassed by a                  ├─[gate]─> SNARE  trig
//!    diode: 8 ms pulses)               └─[gate]─> HAT    trig
//!
//!   KICK   : gm-C resonator ─────────────────> op-amp in+   (no load at all)
//!   SNARE  : noise -> BPF -> OTA VCA ──┐
//!   HAT    : noise -> HPF -> OTA VCA ──┴─────> op-amp in-   (virtual ground)
//!                                              op-amp out ─> 8 Ω speaker
//! ```
//!
//! ONE op-amp does the whole output stage. Its non-inverting input taps the
//! kick's high-Q resonator node — op-amp inputs draw exactly zero current in
//! this model, so the resonator is not loaded at all. Its inverting input is
//! a virtual-ground current bus that every OTA output dumps into; OTA output
//! current is independent of output voltage, so voices sum with zero
//! crosstalk and each OTA gets the DC path it needs (an unloaded OTA output
//! in this engine floats on GMIN and runs to 1e7 V).
//!
//!   v(OUT) = v(KA)·(1 + Rf/Rg)  −  I_sum·Rf
//!
//! ## Wiring rule
//!
//! Nodes are geometric: pins at the SAME grid point are the same node. The
//! coordinates below are the ones that were simulated. Pins may be moved
//! anywhere as long as every coincidence group listed in `NETS` is
//! preserved — nothing else about the geometry is electrical.

use sim_core::{ElementKind as K, ElementSpec, Point};

// ------------------------------------------------------------ layout

/// All coordinates are relative to this origin; the block occupies roughly
/// 36 x 46 grid units from it.
pub struct Drums {
    pub elements: Vec<ElementSpec>,
    /// Shared 9 V rail node. Sharing GROUND with other modules is free
    /// (node 0 is eliminated from the matrix); sharing this RAIL node puts
    /// them in the same LU island and the dense factorisation cost is
    /// superlinear, so give other modules their own `Rail`.
    pub v9: Point,
    /// Short positive pulse, one per beat. Everything downstream taps here.
    pub trigger_bus: Point,
    /// Per-voice trigger taps. A beat sequencer puts a latching `Switch`
    /// between `trigger_bus` and each of these.
    pub kick_trig: Point,
    pub snare_trig: Point,
    pub hat_trig: Point,
    /// Virtual-ground current summing bus: any extra OTA voice can wire its
    /// output pin straight here, any voltage-output voice through a series
    /// resistor.
    pub sum: Point,
    /// Op-amp output / speaker terminal.
    pub out: Point,
    pub ground: Point,
}

fn spec(id: u32, kind: K, a: Point, b: Point) -> ElementSpec {
    ElementSpec::two(id, kind, a, b)
}
fn r(ohms: f64) -> K {
    K::Resistor { ohms }
}
fn c(farads: f64) -> K {
    K::Capacitor { farads }
}
fn pot(id: u32, ohms: f64, wiper: f64, a: Point, w: Point, b: Point) -> ElementSpec {
    ElementSpec {
        id,
        kind: K::Potentiometer { ohms, wiper },
        pins: vec![a, w, b],
        ..Default::default()
    }
}
fn ota(id: u32, ip: Point, im: Point, out: Point, bias: Point) -> ElementSpec {
    ElementSpec {
        id,
        kind: K::Ota,
        pins: vec![ip, im, out, bias],
        ..Default::default()
    }
}

/// Build the whole kit. `id0` is the first element id used; the kit
/// consumes `id0 .. id0 + 99`. `seed` picks which hiss you get — two drum
/// kits in one world must use different seeds or they are the same noise.
///
/// 40 elements: 8 clock + 4 output stage + 2 shared noise front end
/// + 10 kick + 8 snare + 8 hat. Drop `hat` (8) or `snare` (8) to shrink.
pub fn drum_kit(o: Point, id0: u32, seed: u32) -> Drums {
    let p = |x: i32, y: i32| (o.0 + x, o.1 + y);
    let id = |n: u32| id0 + n;

    // ---- nets
    let g = p(0, 44); // ground
    let v9 = p(30, 0); // 9 V rail
    let tdis = p(34, 0); // 555 DIS / RA-RB junction
    let tcap = p(34, 4); // 555 THR+TRIG / timing cap
    let t555o = p(28, 0); // 555 OUT pin
    let trig = p(24, 0); // TRIGGER BUS
    let ktrig = p(16, 34);
    let strig = p(16, 14);
    let htrig = p(16, 18);
    let sum = p(20, 20);
    let out = p(24, 20);
    let ka = p(0, 30); // kick resonator, output node
    let kb = p(4, 30); // kick resonator, second integrator
    let kbias = p(2, 34); // both OTA bias pins
    let kenv = p(8, 34);
    let nout = p(0, 10); // noise source output
    let sa = p(2, 10);
    let sin_ = p(4, 10); // snare OTA signal input
    let sbias = p(6, 14);
    let senv = p(10, 14);
    let ha = p(2, 16);
    let hin = p(4, 16); // hat OTA signal input
    let hbias = p(6, 18);
    let henv = p(10, 18);

    let mut e = Vec::with_capacity(40);

    // ---------------------------------------------------------- clock
    // 555 astable, RA = 10 k, RB = 330 k, C = 1 µF, with a diode across RB
    // so charging bypasses it: HIGH = 0.693·RA·C, LOW = 0.693·RB·C.
    // MEASURED: 4.27 Hz, 8.14 ms pulses, 7.8 V. There is no CTRL or RESET
    // pin on this 555 model, so this is the only way to get a trigger
    // rather than a 50 %+ gate.
    e.push(ElementSpec {
        id: id(0),
        kind: K::Rail {
            dc: 9.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        },
        pins: vec![v9],
        ..Default::default()
    });
    e.push(ElementSpec {
        id: id(1),
        kind: K::Timer555,
        // [vcc, gnd, trig, thr, out, dis]
        pins: vec![v9, g, tcap, tcap, t555o, tdis],
        ..Default::default()
    });
    e.push(spec(id(2), r(10_000.0), v9, tdis)); // RA
    e.push(spec(id(3), r(330_000.0), tdis, tcap)); // RB
    e.push(spec(id(4), K::Diode, tdis, tcap)); // charge bypass
    e.push(spec(id(5), c(1e-6), tcap, g));
    // SERIES TRIGGER RESISTOR — load-bearing, not decoration. The 555 output
    // is an ideal voltage branch and the envelope diodes are ideal, so
    // without it an envelope cap is charged inside one substep and the
    // trapezoidal integrator rings: MEASURED 13.8 V on a cap fed from a
    // 7.8 V pulse (and 14.4 V on the 15 nF one). 10 k puts the charging
    // time constant well above dt/2 and the overshoot vanishes — every
    // envelope cap now peaks at 7.6 V on every hit, first or thousandth.
    e.push(spec(id(6), r(10_000.0), t555o, trig));
    e.push(ElementSpec::ground(id(7), g));

    // ---------------------------------------------------- output stage
    // Rf/Rg = 68 -> non-inverting gain 69 for the kick, transimpedance
    // 6.8 MΩ for every OTA voice on the summing bus.
    e.push(ElementSpec {
        id: id(10),
        kind: K::OpAmp { rail: 9.0, isc: sim_core::DEFAULT_OPAMP_ISC },
        pins: vec![ka, sum, out], // [in+, in-, out]
        ..Default::default()
    });
    e.push(spec(id(11), r(6.8e6), out, sum));
    e.push(spec(id(12), r(100_000.0), sum, g));
    // Give the speaker a LOW element id: the server streams the four
    // lowest-id Speakers, so this must not be crowded out.
    e.push(spec(id(13), K::Speaker { ohms: 8.0 }, out, g));

    // ------------------------------------------------------------ kick
    // gm-C two-integrator resonator. f = gm/(2πC) with gm = Iabc/(2·Vt),
    // and Iabc comes from the TUNE pot plus a decaying envelope — so one
    // trigger both rings it and sweeps it down in pitch.
    //
    // A(t)' = -(gm/C)·B - A/(Ra·C),  B' = (gm/C)·A
    // -> poles at -1/(2·Ra·C) ± j·gm/C. Amplitude decays with Ra·C
    // (independent of pitch); pitch follows the bias. Only ONE damping
    // resistor is needed: node KB is not a free integrator, because the
    // loop itself closes its DC path (verified over 30 simulated seconds).
    //
    // MEASURED at the default wiper: 117 Hz -> 49 Hz over 200 ms,
    // 0.96 V peak / 0.234 V rms at the speaker, -20 dB at 95 ms,
    // -40 dB at 215 ms, spectral centroid 91 Hz, tonality 0.86.
    e.push(ota(id(20), kb, g, ka, kbias)); // [in+, in-, out, bias]
    e.push(ota(id(21), g, ka, kb, kbias));
    e.push(spec(id(22), c(100e-9), ka, g));
    e.push(spec(id(23), c(100e-9), kb, g));
    e.push(spec(id(24), r(240_000.0), ka, g)); // sets the decay: Ra·C = 24 ms
    e.push(spec(id(25), K::Diode, ktrig, kenv));
    e.push(spec(id(26), c(33e-9), kenv, g)); // pitch-envelope cap, tau = 50 ms
    e.push(spec(id(27), r(1.5e6), kenv, kbias));
    // TUNE — a potentiometer wired as a rheostat: end b is coincident with
    // the wiper, which stamps a conductance from a node to itself, i.e.
    // exactly nothing, leaving `ohms·wiper` between end a and the wiper.
    // Sets the resting bias current and so the pitch the sweep lands on.
    // MEASURED cycle-by-cycle, dragged across its whole legal range:
    //   w=0.01 -> 2616 .. 2560 Hz  (a ping: nearly no sweep left)
    //   w=0.05 ->  588 ..  517 Hz  (a bongo; tonality 1.000, a pure sine)
    //   w=0.20 ->  199 ..  129 Hz  (a tom)
    //   w=0.52 ->  116 ..   49 Hz  (the kick; this default)
    //   w=0.80 ->   96 ..   33 Hz
    //   w=0.99 ->   88 ..   26 Hz  (a very deep boom)
    // No quarantine at any position, and the level barely moves (this is a
    // pitch knob, not a volume knob). The sweep is CONTINUOUS: a gm-C
    // resonator has no dt quantization, unlike every comparator-based
    // oscillator in this engine.
    e.push(pot(id(28), 5e6, 0.52, v9, kbias, kbias));
    // Excitation. 220 pF against the 100 nF resonator cap injects ~17 mV —
    // deliberately small: the OTA input is linear only to about ±20 mV
    // (tanh(vd/2Vt)), and a hard-driven gm-C loop compresses its own gm and
    // drops ~50 % in pitch. Keeping it small keeps the kick in tune.
    e.push(spec(id(29), c(220e-12), kenv, ka));

    // ------------------------------------------- shared noise front end
    e.push(spec(
        id(30),
        K::Noise {
            volts: 1.0,
            ohms: 1000.0,
            seed,
        },
        nout,
        g,
    ));
    // Anti-alias pole made out of the noise source's OWN 1 kΩ output
    // resistance: fc = 4.8 kHz, one element, serves every noise voice. The
    // raw source is flat to 25 kHz and the audio tap decimates to 12.5 kHz,
    // so without this the "hi-hat" is 85 % out-of-band energy that folds
    // straight back down and turns it into full-band white noise —
    // indistinguishable from the snare. MEASURED: 85 % -> 11 %.
    e.push(spec(id(31), c(33e-9), nout, g));

    // ----------------------------------------------------------- snare
    // Band-pass 160 Hz .. 1.0 kHz into an OTA VCA. The 470 k series / 4.7 k
    // shunt divider is not just a filter: it is the ÷101 attenuator that
    // keeps the OTA input inside its linear window (MEASURED 4.4 mV peak,
    // THD < 0.1 %). Skip it and the tanh clips the hiss into a buzz.
    //
    // MEASURED: 0.94 V peak / 0.116 V rms, -20 dB at 100 ms, -30 dB at
    // 125 ms, centroid 1370 Hz (1328 Hz at the 12.5 kHz tap),
    // tonality 0.169, only 1.3 % of power above Nyquist.
    e.push(spec(id(40), c(1.5e-9), nout, sa)); // HP, 220 Hz
    e.push(spec(id(41), r(470_000.0), sa, sin_));
    // TONE/LEVEL: the shunt leg as a rheostat pot. Turning it down
    // attenuates AND brightens. MEASURED across the legal range:
    //   w=0.01 -> 0.009 V rms, centroid 4386 Hz
    //   w=0.47 -> 0.116 V rms, centroid 1370 Hz  (this default)
    //   w=0.99 -> 0.161 V rms, centroid  984 Hz
    // The loud end is only 1.4x the default and puts 3.2 mW into the
    // speaker against its 0.5 W rating, so no wiper position can cook it.
    e.push(pot(id(42), 10_000.0, 0.47, sin_, g, g));
    e.push(spec(id(43), c(33e-9), sin_, g)); // LP, 1.0 kHz
    e.push(ota(id(44), sin_, g, sum, sbias));
    // Envelope: the trigger charges C through a diode (so a long gate still
    // produces a short hit), then it discharges through R into the OTA's
    // bias-pin junction. The exponential VCA curve is free — it is the bias
    // diode, exactly as an LM13700 behaves. tau = 3.3M · 15n = 50 ms.
    e.push(spec(id(45), K::Diode, strig, senv));
    e.push(spec(id(46), c(15e-9), senv, g));
    e.push(spec(id(47), r(3.3e6), senv, sbias));

    // --------------------------------------------------------- hi-hat
    // Same noise source, high-passed at 2.2 kHz, low-passed at 1.6 kHz-ish
    // by its own 10 nF plus the shared 4.8 kHz pole, and gated by a much
    // shorter envelope (tau = 22 ms).
    //
    // MEASURED: 1.04 V peak / 0.098 V rms, -20 dB at 40 ms, -30 dB at
    // 55 ms, centroid 3625 Hz (3170 Hz at the tap), tonality 0.257.
    // The tap's 6.25 kHz Nyquist is a hard ceiling: a real hi-hat lives at
    // 8-12 kHz and simply cannot be transmitted, so this is a bright noise
    // burst, not a shimmer. 11 % of its power still folds.
    e.push(spec(id(50), c(150e-12), nout, ha)); // HP, 2.2 kHz
    e.push(spec(id(51), r(470_000.0), ha, hin));
    // Same TONE knob. MEASURED: w=0.01 -> 0.008 V rms / 6194 Hz centroid,
    // w=0.47 -> 0.098 V rms / 3626 Hz, w=0.99 -> 0.134 V rms / 2978 Hz.
    e.push(pot(id(52), 10_000.0, 0.47, hin, g, g));
    e.push(spec(id(53), c(10e-9), hin, g));
    e.push(ota(id(54), hin, g, sum, hbias));
    e.push(spec(id(55), K::Diode, htrig, henv));
    e.push(spec(id(56), c(6.8e-9), henv, g));
    e.push(spec(id(57), r(3.3e6), henv, hbias));

    Drums {
        elements: e,
        v9,
        trigger_bus: trig,
        kick_trig: ktrig,
        snare_trig: strig,
        hat_trig: htrig,
        sum,
        out,
        ground: g,
    }
}

/// Hard-wire every voice to the trigger bus: all three drums fire on every
/// beat. 3 elements. Use this if the sequencer module is not present.
pub fn trigger_hardwire(d: &Drums, id0: u32) -> Vec<ElementSpec> {
    vec![
        ElementSpec::two(id0, K::Wire, d.trigger_bus, d.kick_trig),
        ElementSpec::two(id0 + 1, K::Wire, d.trigger_bus, d.snare_trig),
        ElementSpec::two(id0 + 2, K::Wire, d.trigger_bus, d.hat_trig),
    ]
}

/// Beat gates: one LATCHING `Switch` per voice (a `Button` is momentary and
/// cannot hold a pattern), plus a 1 MΩ pulldown so an open gate leaves a
/// defined node rather than a GMIN float. 2 elements per voice.
///
/// Safe against `validate::UnsolvableWhenSwitched`, which factorizes the
/// document a second time with EVERY switch forced closed: all three closed
/// is just "all three drums on this beat", MEASURED at 0.292 V rms out with
/// no quarantine. Each gate only ever joins the trigger bus to a passive
/// tap, never two driven nodes.
pub fn trigger_gates(d: &Drums, id0: u32, closed: [bool; 3]) -> Vec<ElementSpec> {
    let taps = [d.kick_trig, d.snare_trig, d.hat_trig];
    let mut v = Vec::with_capacity(6);
    for (i, (tap, cl)) in taps.iter().zip(closed.iter()).enumerate() {
        let i = i as u32 * 2;
        v.push(ElementSpec::two(
            id0 + i,
            K::Switch { closed: *cl },
            d.trigger_bus,
            *tap,
        ));
        v.push(ElementSpec::two(
            id0 + i + 1,
            K::Resistor { ohms: 1e6 },
            *tap,
            d.ground,
        ));
    }
    v
}
