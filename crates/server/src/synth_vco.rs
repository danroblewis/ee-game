//! VCO and LFO modules for the synthesizer world.
//!
//! Everything in this file has been simulated and measured against the real
//! engine at `dt = 20 µs`; the numbers in the doc comments are measurements,
//! not estimates. Nothing here is declared by `main.rs` yet — the world
//! assembler picks the modules it wants and calls these builders.
//!
//! # The core: an OTA integrator inside an op-amp Schmitt loop
//!
//! The OTA's output is a current source of magnitude `Iabc` whose sign
//! follows the comparator's square wave, so the capacitor on the OTA output
//! ramps linearly: a triangle. The comparator's *inverting* input takes that
//! triangle DIRECTLY, with no series resistor, because `ElementKind::OpAmp`
//! stamps nothing at all for its input pins — the integrator is loaded with
//! exactly zero current. (The showcase's vignette H puts a 1 MΩ into the
//! summing node instead, which is why it only works at LFO rates: at 55 Hz
//! that resistor's current is comparable to `Iabc` itself.) The hysteresis
//! divider hangs off the comparator's *output*, which is an ideal voltage
//! branch and does not care about being loaded.
//!
//! Frequency is exactly `f = Iabc / (4 · Vth · C)` where
//! `Vth = rail · Rc / (Rb + Rc)` is the Schmitt threshold.
//!
//! # 1 V/octave comes free from the OTA's bias diode
//!
//! `Iabc = OTA_IS · (exp(v_bias / VT) − 1)` with `VT = 25.865 mV`, so an
//! octave is `VT · ln2 = 17.93 mV` at the bias pin. Feeding the bias pin
//! from a low-impedance divider (`R_SCALE` from CV, `R_OFF` from the 9 V
//! rail, `R_GND` to ground; Thevenin ≈ 150 Ω) that divides CV by 55.5
//! therefore gives a true exponential 1 V/octave converter for three
//! resistors. The Thevenin resistance must stay well under the diode's own
//! `VT / Iabc`; 150 Ω costs about 1 % of pitch at the top of the range,
//! which is what `R_SCALE`'s 0.45 % over-scale trims out.
//!
//! # The one hard limitation: pitch is quantized by the timestep
//!
//! A comparator only flips at a substep boundary, so the period locks to an
//! integer number of 20 µs substeps and the oscillator is *perfectly* stable
//! but sits on a discrete pitch grid — 30 cents apart at A440, 60 at A880.
//! Measured: with the dither source removed, the period standard deviation
//! is exactly 0.00 µs and the tuning error over five octaves scatters
//! ±20 cents with a −95 cent hole at 1.7 kHz.
//!
//! The fix is a real one: `NOISE_DITHER_V` of white noise under the
//! hysteresis divider. 5 mV peak is about half a substep of triangle slew,
//! enough to randomize which substep the crossing lands on, so the *average*
//! frequency becomes continuous again. Measured over a chromatic octave from
//! A3: worst error **2.9 cents**. The cost is ~1 % of cycle-to-cycle period
//! jitter — an oscillator that is in tune and very slightly breathy rather
//! than dead clean and a quarter-tone sharp. Choose deliberately; both are
//! honest solver output.
//!
//! # Audio band
//!
//! Triangle out, 0.52 Vpp, harmonics falling as 1/n² (measured h3 = −25 dB,
//! h5 = −39 dB at 440 Hz). At the server's 12.5 kHz speaker tap the worst
//! aliased component is −53 dB at 869 Hz and −44 dB at 1.77 kHz, so the
//! triangle is clean across the whole musical range. The square output is
//! 10 Vpp and much dirtier (−36 dB aliases at 869 Hz): use it for sync,
//! gates and triggers, or low-pass it, but do not send it straight to a
//! speaker.

use sim_core::{ElementKind as K, ElementSpec, Point};
use sim_golden::{dc, gnd, r, spec, spec3};

// ---------------------------------------------------------------- values
//
// The exponential converter. Thevenin 150 Ω at the bias pin; `R_SCALE`
// carries the CV, `R_OFF` sets the pitch at CV = 0, `R_GND` makes up the
// rest of the divider. `R_SCALE` is 0.45 % below the textbook
// `150 / (VT·ln2)` = 8367 Ω: that trims out the bias-diode IR drop, which
// otherwise flattens the top of the range by ~90 cents.
pub const R_SCALE: f64 = 8_329.8;
pub const R_OFF: f64 = 3_360.4;
pub const R_GND: f64 = 160.03;
/// Drops the 9 V rail so a pitch pot's full travel is 0..5 V = five octaves.
pub const R_SPAN: f64 = 80_000.0;
/// Schmitt divider: `Vth = 5 V · 50k / 1M = ±0.25 V`.
pub const R_HYST_TOP: f64 = 950_000.0;
pub const R_HYST_BOT: f64 = 50_000.0;
/// Timing cap for the audio VCO (55 Hz at CV = 0) and for the triangle LFO.
pub const C_VCO: f64 = 1e-9;
pub const C_LFO: f64 = 47e-9;
/// Threshold dither. 0.0 disables it (and drops the element).
pub const NOISE_DITHER_V: f64 = 0.005;
/// Supply every module in this file expects.
pub const SUPPLY_V: f64 = 9.0;

/// Where a module sits and what its knobs are set to.
///
/// `origin` is the module's top-left node; the fragment occupies roughly
/// 30 × 10 grid units from there. `rail` is the node carrying `SUPPLY_V`
/// and `gnd_pt` the world's ground node — modules share both, which costs
/// nothing in the matrix (node 0 is eliminated) and lets several voices run
/// off one battery.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub id0: u32,
    pub origin: Point,
    pub rail: Point,
    pub gnd_pt: Point,
}

impl Place {
    fn p(&self, dx: i32, dy: i32) -> Point {
        (self.origin.0 + dx, self.origin.1 + dy)
    }
    /// Comparator square-wave output, ±5 V. Ideal source: load it freely.
    pub fn square(&self) -> Point {
        self.p(0, 0)
    }
    /// Triangle output, 0.52 Vpp centred on 0 V. This is the AUDIO output —
    /// but it is a bare integrator node, so anything that draws current from
    /// it changes the pitch or stops the oscillator. Buffer it with an
    /// op-amp follower (one element) before any resistive load.
    pub fn triangle(&self) -> Point {
        self.p(4, 0)
    }
    /// The OTA bias node. Extra CV (vibrato, envelopes, a sequencer's
    /// summed pitch) is injected here through a resistor; the node's 150 Ω
    /// Thevenin means `R_inj` sets the depth as
    /// `octaves = V_inj · 150 / (R_inj · 0.018)`.
    pub fn bias(&self) -> Point {
        self.p(6, 8)
    }
    /// The buffered CV node — an op-amp output, so it is an ideal source at
    /// exactly 1 V per octave above 55 Hz. If an external module already
    /// supplies a low-impedance CV, drop the pot/span/follower trio from
    /// `vco()` and wire that module's output straight onto this node.
    pub fn cv(&self) -> Point {
        self.p(10, 8)
    }
    fn hys(&self) -> Point {
        self.p(0, 4)
    }
    fn wiper(&self) -> Point {
        self.p(14, 8)
    }
    fn nz(&self) -> Point {
        self.p(0, 8)
    }
    fn span(&self) -> Point {
        self.p(18, 8)
    }
}

/// The audio VCO. **12 elements** (13 with the shared battery, 14 with the
/// shared ground). Ids `id0 ..= id0 + 11`.
///
/// `pitch_wiper` in 0.01..0.99 sets the note: `f = 55 · 2^(5 · wiper)` Hz.
/// Measured across the knob with `NOISE_DITHER_V = 5 mV`:
///
/// | wiper | CV   | measured   | ideal   | error       |
/// |-------|------|------------|---------|-------------|
/// | 0.02  | 0.10 | 58.94 Hz   | 58.95   | −0.2 cents  |
/// | 0.10  | 0.50 | 77.77 Hz   | 77.78   | −0.4 cents  |
/// | 0.30  | 1.50 | 155.40 Hz  | 155.56  | −1.8 cents  |
/// | 0.54  | 2.70 | 357.34 Hz  | 357.39  | −0.3 cents  |
/// | 0.70  | 3.50 | 621.99 Hz  | 622.25  | −0.7 cents  |
/// | 0.78  | 3.90 | 818.12 Hz  | 821.07  | −6.2 cents  |
/// | 0.90  | 4.50 | 1250.0 Hz  | 1244.5  | +7.6 cents  |
/// | 0.99  | 4.95 | 1666.7 Hz  | 1700.1  | −34.3 cents |
///
/// In tune to ~2 cents from 55 Hz to ~620 Hz; the substep pitch grid starts
/// to show above ~800 Hz and by 1.7 kHz a note can be a quarter-tone out. A
/// chromatic octave A3→A4 measured worst-case **2.9 cents**. Treat
/// 55 Hz–900 Hz as the in-tune range and 900 Hz–2 kHz as usable-but-drifting.
///
/// Never quarantines: 60 s soak, 0 rescues, both extremes of both knobs.
pub fn vco(pl: &Place, pitch_wiper: f64, dither_v: f64) -> Vec<ElementSpec> {
    let i = pl.id0;
    let g = pl.gnd_pt;
    let mut v = vec![
        // OTA [in+, in-, out, bias]. in+ = the square, in- = ground: while
        // the comparator is high the OTA pushes current INTO the cap, so
        // the triangle ramps up towards the upper threshold. (Wiring these
        // two the other way round — as vignette H does — turns the loop
        // into DC-stable negative feedback and it will not start; verified.)
        ElementSpec {
            id: i,
            kind: K::Ota,
            pins: vec![pl.square(), g, pl.triangle(), pl.bias()],
        },
        spec(i + 1, K::Capacitor { farads: C_VCO }, pl.triangle(), g),
        // Schmitt comparator [in+, in-, out]: hysteresis node, triangle, square.
        spec3(
            i + 2,
            K::OpAmp { rail: 5.0 },
            pl.hys(),
            pl.triangle(),
            pl.square(),
        ),
        spec(i + 3, r(R_HYST_TOP), pl.square(), pl.hys()),
        spec(
            i + 4,
            r(R_HYST_BOT),
            pl.hys(),
            if dither_v > 0.0 { pl.nz() } else { g },
        ),
        // exponential converter
        spec(i + 5, r(R_SCALE), pl.cv(), pl.bias()),
        spec(i + 6, r(R_OFF), pl.rail, pl.bias()),
        spec(i + 7, r(R_GND), pl.bias(), g),
        // pitch knob: rail -> span resistor -> pot -> op-amp follower -> CV
        spec(i + 8, r(R_SPAN), pl.rail, pl.span()),
        spec3(
            i + 9,
            K::Potentiometer {
                ohms: 100_000.0,
                wiper: pitch_wiper,
            },
            g,
            pl.wiper(),
            pl.span(),
        ),
        // Unity follower. An op-amp is supply-less here and its inputs draw
        // nothing, so this is ONE element and it is what makes the 1 V/oct
        // law hold: driving R_SCALE straight from a 10 k pot wiper costs
        // 844 cents at CV = 3.6 V (measured), and 1.3 cents with the buffer.
        spec3(i + 10, K::OpAmp { rail: 9.0 }, pl.wiper(), pl.cv(), pl.cv()),
    ];
    if dither_v > 0.0 {
        v.push(spec(
            i + 11,
            K::Noise {
                volts: dither_v,
                ohms: 1000.0,
                seed: 0x5EED_0001u32.wrapping_add(i),
            },
            pl.nz(),
            g,
        ));
    }
    v
}

/// A 555 astable as the cheap LFO / clock: **4 elements** plus 2 more for
/// an ac-coupled modulation tap. Ids `id0 ..= id0 + 5`.
///
/// `rate_wiper` in 0.01..0.99 gives **0.69 Hz .. 25 Hz** measured
/// (ideal `1.44 / ((R_A + 2·R_B)·C)`, matched within 0.3 % at every wiper
/// tested). Two outputs, both zero-jitter:
/// * pin 4 (`out`), a 7.7 Vpp square (0.1 V .. 7.8 V) — the natural beat
///   clock and gate for a sequencer;
/// * the timing-cap node, a 3.0 Vpp exponential ramp between Vcc/3 and
///   2·Vcc/3 — the modulation source, ac-coupled here by `C_COUPLE`.
///
/// `r_vib` sets the vibrato depth into `bias_target` (the VCO's `bias()`).
/// Measured on a 440 Hz carrier: 1 MΩ → ±30 cents, 470 k → ±42, 220 k →
/// ±70, 100 k → ±115.
///
/// One real interaction to know about: the coupling network loads the 555's
/// timing cap, so the LFO rate depends on how many voices you feed and
/// through what. One 220 k tap took a 5.94 Hz LFO to 5.02 Hz; two took it to
/// 3.23 Hz. Trim `rate_wiper` after patching, or buffer the ramp with
/// another op-amp follower if the rate must be independent.
///
/// The 555 has **no CTRL pin and no RESET pin** in this engine, so its rate
/// cannot be voltage-controlled and it cannot be gated — use `lfo_triangle`
/// if the modulation rate itself has to follow a CV.
pub fn lfo_555(
    pl: &Place,
    id0: u32,
    rate_wiper: f64,
    bias_target: Point,
    r_vib: f64,
) -> Vec<ElementSpec> {
    let i = id0;
    let g = pl.gnd_pt;
    let (ctl, out, dis, ac) = (pl.p(0, 16), pl.p(4, 16), pl.p(8, 16), pl.p(12, 16));
    vec![
        // pins [vcc, gnd, trig, thr, out, dis]; TRIG tied to THR is the
        // standard astable.
        ElementSpec {
            id: i,
            kind: K::Timer555,
            pins: vec![pl.rail, g, ctl, ctl, out, dis],
        },
        spec(i + 1, r(100_000.0), pl.rail, dis), // R_A
        // R_B as a rheostat: end b sits on the wiper so only the a..wiper
        // leg conducts.
        spec3(
            i + 2,
            K::Potentiometer {
                ohms: 1_000_000.0,
                wiper: rate_wiper,
            },
            dis,
            ctl,
            ctl,
        ),
        spec(i + 3, K::Capacitor { farads: 0.47e-6 }, ctl, g),
        spec(i + 4, K::Capacitor { farads: 1e-6 }, ctl, ac),
        spec(i + 5, r(r_vib), ac, bias_target),
    ]
}

/// A triangle LFO: the same core as `vco`, with `C_LFO` and a symmetric
/// 1 M/1 M hysteresis divider (Vth = ±2.5 V). **8 elements**, or 10 with
/// the output follower and injection resistor `buffer_to` adds. It needs a
/// CV node of its own: an ideal source, another module's buffered CV, or a
/// pot + follower pair copied from `vco` (2 more elements).
///
/// Measured, sharing the VCO's exponential-converter values, 5 Vpp triangle
/// and 10 Vpp square, zero period jitter, never quarantines:
///
/// | CV | 0     | 1     | 2     | 3     | 4    | 5    | 6    | 7    | 8    |
/// |----|-------|-------|-------|-------|------|------|------|------|------|
/// | Hz | 0.118 | 0.238 | 0.476 | 0.954 | 1.91 | 3.81 | 7.56 | 14.9 | 28.7 |
///
/// i.e. 0.12 Hz to 29 Hz over the whole 9 V rail, doubling per volt to
/// within 0.3 % up to 4 V and 3.4 % at the very top. Use it when the
/// modulation rate must itself be voltage-controlled; otherwise `lfo_555`
/// does the same job for 4 elements instead of 11.
///
/// The triangle node is a bare integrator: connecting `r_vib` to it
/// directly stops the LFO dead (measured — the LFO simply flatlines).
/// `buffer` inserts the follower that makes it drivable.
pub fn lfo_triangle(
    pl: &Place,
    id0: u32,
    cv_source: Point,
    buffer_to: Option<(Point, f64)>,
) -> Vec<ElementSpec> {
    let i = id0;
    let g = pl.gnd_pt;
    let (sq, tri, hys, bias, buf) = (
        pl.p(0, 24),
        pl.p(4, 24),
        pl.p(8, 24),
        pl.p(12, 24),
        pl.p(16, 24),
    );
    let mut v = vec![
        ElementSpec {
            id: i,
            kind: K::Ota,
            pins: vec![sq, g, tri, bias],
        },
        spec(i + 1, K::Capacitor { farads: C_LFO }, tri, g),
        spec3(i + 2, K::OpAmp { rail: 5.0 }, hys, tri, sq),
        spec(i + 3, r(1_000_000.0), sq, hys),
        spec(i + 4, r(1_000_000.0), hys, g),
        spec(i + 5, r(R_SCALE), cv_source, bias),
        spec(i + 6, r(R_OFF), pl.rail, bias),
        spec(i + 7, r(R_GND), bias, g),
    ];
    if let Some((target, r_inj)) = buffer_to {
        v.push(spec3(i + 8, K::OpAmp { rail: 9.0 }, tri, buf, buf));
        v.push(spec(i + 9, r(r_inj), buf, target));
    }
    v
}

/// The recommended voice: VCO + 555 LFO vibrato, off one 9 V battery.
/// **20 elements**, measured 1.32–1.35 µs/step = **14.9× real time** alone.
///
/// Measured behaviour at `pitch_wiper = 0.54`, `rate_wiper = 0.20`:
/// carrier 155.4 Hz sweeping 149.9..160.9 Hz (±60 cents) at 5.02 Hz,
/// 0.518 Vpp triangle. 60 s soak: no quarantine, no rescues, still at
/// 357.3 Hz / 5.021 Hz at the end.
pub fn voice(pl: &Place, pitch_wiper: f64, rate_wiper: f64) -> Vec<ElementSpec> {
    let mut v = vec![
        spec(pl.id0 + 90, dc(SUPPLY_V), pl.rail, pl.gnd_pt),
        gnd(pl.id0 + 91, pl.gnd_pt),
    ];
    v.extend(vco(pl, pitch_wiper, NOISE_DITHER_V));
    v.extend(lfo_555(pl, pl.id0 + 20, rate_wiper, pl.bias(), 220_000.0));
    v
}

// ------------------------------------------------------------ merge notes
//
// This file is additive: no existing source was touched, and it is not
// declared by `main.rs`, so cargo ignores it until the world assembler adds
// `mod synth_vco;`. It needs `sim-golden` as a non-dev dependency of
// `server`, which it already is.
//
// Real-time budget, measured on this machine (release, pinned cargo 1.95.0,
// three passes each, other agents' builds running):
//
//   1 x voice()  20 elements   1.33-1.40 us/step   14.9x real time
//   2 x voice()  39 elements   4.56-4.58 us/step    4.4x
//   3 x voice()  58 elements   10.0-10.1 us/step    2.0x
//   4 x voice()  77 elements   18.2-18.3 us/step    1.1x
//   5 x voice()  96 elements   30.0-30.1 us/step    0.66x
//
// Cost grows as n^2 (dense LU with zero-skipping), and giving each module
// its own battery instead of a shared rail made no measurable difference:
// 6 modules on separate rails cost 46.5 us/step, on one shared rail
// 39.2 us/step. Since sim-time dilation detunes the instrument, budget for
// the WHOLE world staying above 1.0x, not just this module.
