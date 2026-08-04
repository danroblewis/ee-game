//! SHARED SYNTH BLOCKS — the parts every instrument room turned out to need.
//!
//! Four rooms wanted the same four things: an exponential 1 V/octave VCO, an
//! attack-decay envelope generator, a MOSFET VCA, and a virtual-earth mixer
//! into a speaker. This is those four, laid out with `layout::Sheet` so they
//! come out as real schematic blocks, and parameterised only where a room
//! actually differs.
//!
//! ## Why the shipped `synth.rs` does not use this
//!
//! It could — the circuits are the same circuits — and it deliberately does
//! not. `compile` numbers electrical nodes by first-seen junction, scanning
//! the element list in order, and that node order IS the pivot order of the
//! dense LU. Re-emitting the shipped room's elements from a shared helper
//! would reorder them and perturb its arithmetic in the last bits, for no
//! gain. The VALUES are what carries across, and they are imported from
//! `synth_vco` here rather than retyped.
//!
//! ## Cost, which is the only thing that decides a synth's architecture here
//!
//! Measured (`roombench.rs`), cost ≈ `0.0147 · NR · unknowns^1.64` µs per
//! substep against a 20 µs budget. Two consequences shape every block below:
//!
//!   * A **discrete** nonlinearity — `OpAmp`, `Timer555`, `Gate`, `Counter`,
//!     `Mux` — costs Newton NOTHING; the factorization survives between its
//!     state flips. A **smooth** one — `Ota`, `Diode`, `Npn`, `Nmos` — is
//!     re-linearised on every Newton pass, over the whole island.
//!   * UNKNOWNS, not devices, are the budget. A node is a node whether a
//!     resistor or a transistor made it, and 1.64 is a steep exponent.
//!
//! So: op-amps freely, one OTA where an exponential current is wanted,
//! MOSFETs rather than OTAs for VCAs (measured 17.1 µs against 26.1 for the
//! same room), and no node that does not earn its place.

use sim_core::{ElementKind as K, Point};

use crate::layout::{Sheet, DOWN, E, LEFT, N, RIGHT, S, UP, W};
use crate::synth_vco::{
    C_VCO, NOISE_DITHER_V, R_GND, R_HYST_BOT, R_HYST_TOP, R_OFF, R_SCALE, R_SPAN,
};

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
    K::OpAmp {
        rail,
        isc: sim_core::DEFAULT_OPAMP_ISC,
    }
}

// ------------------------------------------------------------------- VCO

/// What a [`vco`] hands back.
pub struct Vco {
    /// The comparator's square, ±5 V. An op-amp output, so an ideal source:
    /// load it as hard as you like.
    pub square: Point,
    /// The triangle, ~0.5 Vpp centred on zero. This is the CLEAN output, but
    /// it is a bare integrator node — anything resistive hung on it changes
    /// the pitch. Buffer it, or take the square.
    pub triangle: Point,
    /// The OTA's bias node, the exponential converter's own summing point.
    /// Extra control voltage goes in here through a resistor; the node's
    /// ~150 Ω Thevenin means `octaves = V · 150 / (R_inj · 0.0179)`.
    pub bias: Point,
    /// The buffered pitch CV — an op-amp output at 1 V per octave.
    pub cv: Point,
}

/// The audio VCO: an OTA constant-current integrator inside an op-amp
/// Schmitt loop, on the measured three-resistor exponential converter.
///
/// `f = Iabc / (4·Vth·C)` and `Iabc = Is·(exp(v_bias/VT) − 1)`, so an octave
/// is 17.9 mV at the bias pin and the law is a real one — the junction's, not
/// a lookup table's. With the pitch pot at `w`, `f ≈ 55 · 2^(5w)` Hz.
///
/// The `Noise` under the timing cap is not decoration. A comparator can only
/// flip on a substep boundary, so without it the pitch sits on a `50 kHz / n`
/// grid — 30 cents at A440. Five millivolts of dither randomises which
/// substep the crossing lands on and restores a continuous average: measured
/// worst case 2.9 cents over a chromatic octave (`synth_vco.rs`).
///
/// 14 devices. Occupies about 34 × 16 grid units right and down from `a`,
/// and reaches up to `rail_y` for its two supply taps.
pub fn vco(sh: &mut Sheet, id0: u32, a: Point, rail_y: i32, pitch_wiper: f64, seed: u32) -> Vco {
    let p = |dx: i32, dy: i32| (a.0 + dx, a.1 + dy);
    // ---- PITCH -> CV. The follower is what makes the law hold: driving
    // R_SCALE from a bare wiper measured 844 cents of error (`synth_vco`).
    sh.two(id0, r(R_SPAN), p(2, rail_y - a.1), p(2, -4));
    // pins [end a, wiper, end b]
    let pp = sh.part(id0 + 1, pot(100_000.0, pitch_wiper), p(2, 0), N, 4, true);
    debug_assert_eq!(pp[1], p(4, -2));
    debug_assert_eq!(pp[2], p(2, -4));
    sh.ground(pp[0], DOWN);
    // pins [in+, in-, out]
    let f = sh.part(id0 + 2, opamp(9.0), p(6, -2), E, 4, false);
    let cv = f[2];
    debug_assert_eq!(cv, p(10, -2));
    sh.run(&[pp[1], p(4, -3), f[0]]);
    sh.run(&[cv, p(10, -1), f[1]]);

    // ---- the exponential converter: three resistors on the bias pin.
    sh.two(id0 + 3, r(R_SCALE), p(13, -2), p(17, -2));
    sh.two(id0 + 4, r(R_OFF), p(20, rail_y - a.1), p(20, -4));
    sh.wire(cv, p(13, -2));
    // pins [in+, in-, out, bias]; mirrored, so in+ is the LOWER input and
    // the square can come back along the bottom of the block.
    let ota = sh.part(id0 + 5, K::Ota, p(14, 2), E, 4, true);
    let (sq_in, ota_gnd, tri, vbias) = (ota[0], ota[1], ota[2], ota[3]);
    debug_assert_eq!(sq_in, p(14, 3));
    debug_assert_eq!(tri, p(18, 2));
    debug_assert_eq!(vbias, p(17, 4));
    sh.ground(ota_gnd, UP);
    sh.run(&[p(17, -2), p(17, 0), p(15, 0), p(15, 4), vbias]);
    sh.run(&[p(20, -4), p(20, 0), p(19, 0), p(19, 4), vbias]);
    sh.two(id0 + 6, r(R_GND), vbias, p(17, 7));
    sh.ground(p(17, 7), DOWN);

    // ---- the integrator, its dither, and the comparator round the top.
    // The dither goes UNDER the cap, in series with its bottom plate — never
    // from the integrator node to ground. A `Noise` is an EMF behind 1 kΩ of
    // output resistance, and 1 kΩ hung on a node an OTA is charging with tens
    // of nanoamps is a dead short: measured, the triangle collapsed from
    // ±250 mV to ±7 mV and the loop ran at the substep limit, 11.6 kHz at
    // every pitch. In series it wiggles the whole ramp by 5 mV and shunts
    // nothing.
    sh.two(id0 + 7, cap(C_VCO), p(22, 2), p(22, 5));
    sh.two(
        id0 + 8,
        K::Noise {
            volts: NOISE_DITHER_V,
            ohms: 1000.0,
            seed,
        },
        p(22, 5),
        p(22, 8),
    );
    sh.ground(p(22, 8), DOWN);
    sh.run(&[tri, p(22, 2), p(24, 2)]);
    // pins [in+, in-, out]
    let cmp = sh.part(id0 + 9, opamp(5.0), p(28, -6), E, 6, false);
    let (hys, tri_in, sq) = (cmp[0], cmp[1], cmp[2]);
    debug_assert_eq!(sq, p(34, -6));
    sh.run(&[p(24, 2), p(26, 2), p(26, -5), tri_in]);
    // The Schmitt divider: ±5 V through 950 k / 50 k is a ±0.25 V window,
    // which is the triangle's whole swing.
    sh.run(&[sq, p(34, -9)]);
    sh.two(id0 + 10, r(R_HYST_TOP), p(34, -9), p(28, -9));
    sh.run(&[p(28, -9), hys]);
    sh.two(id0 + 11, r(R_HYST_BOT), p(26, -7), hys);
    sh.ground(p(26, -7), LEFT);
    // The square back round the bottom into the integrator's own input.
    sh.run(&[sq, p(36, -6), p(36, 12), p(12, 12), p(12, 3), sq_in]);

    Vco {
        square: sq,
        triangle: p(24, 2),
        bias: vbias,
        cv,
    }
}

// -------------------------------------------------------------- ENVELOPE

/// An attack-decay envelope generator: a diode into a storage capacitor with
/// a rheostat across it. **Not a device model and not a curve** — the attack
/// is the cap charging through the trigger's own source impedance and the
/// decay is the pot times the cap, which is all a real AD generator is.
///
/// `trig` must be a PULSE, not a level: a cap fed from a gate that stays high
/// for a whole step just sits there charged. Returns the storage node, which
/// is what a VCA gate wants.
///
/// 4 devices (the strap is a wire). Occupies about 12 × 6 units from `a`.
pub fn ad_envelope(
    sh: &mut Sheet,
    id0: u32,
    a: Point,
    trig: Point,
    farads: f64,
    pot_ohms: f64,
    wiper: f64,
) -> Point {
    let p = |dx: i32, dy: i32| (a.0 + dx, a.1 + dy);
    let env = p(4, 0);
    sh.two(id0, K::Diode, trig, p(0, 0));
    sh.wire(p(0, 0), env);
    sh.two(id0 + 1, cap(farads), env, p(4, 3));
    sh.ground(p(4, 3), DOWN);
    // The DECAY rheostat: wiper strapped back to the far end, so end-to-end
    // is `ohms · wiper`. The strap is a WIRE, because a potentiometer whose
    // wiper pin sits on one of its own end pins is not a shape a
    // potentiometer has and the placement gate would refuse it.
    // pins [end a, wiper, end b]
    let d = sh.part(id0 + 2, pot(pot_ohms, wiper), env, E, 4, false);
    debug_assert_eq!(d[1], p(6, -2));
    debug_assert_eq!(d[2], p(8, 0));
    sh.run(&[d[1], p(8, -2), d[2]]);
    sh.ground(d[2], RIGHT);
    env
}

// ------------------------------------------------------------------- VCA

/// A voltage-controlled amplifier that is one transistor.
///
/// An `Nmos` in its ohmic region is a voltage-controlled resistor,
/// `Rds ≈ 1/(k·(Vgs − Vt))` — about 8 kΩ with a 3 V envelope on the gate and
/// ~100 MΩ with it down. Put `r_ser` between its source and the mixer's
/// virtual ground and the leg's gain is `Rf/(r_ser + Rds)`: the resistor sets
/// how loud wide-open is, `Rds` still does all the gating, and the drain
/// signal is divided down before the MOSFET sees it, which is what keeps the
/// part a resistor rather than a distorter.
///
/// This is deliberately NOT a new `ElementKind`. A VCA multiplies two
/// signals, and a MOSFET's channel resistance already does — as every
/// Minimoog-era VCA and every cheap noise gate really did. It is also the
/// measured-cheap choice: the same room with an OTA VCA where the noise can
/// reach it runs at 26.1 µs/substep, and with a MOSFET at 17.1.
///
/// `a` anchors the gate; drain is two left and four below, source two right
/// and four below. Returns the far end of the series resistor.
pub fn nmos_vca(sh: &mut Sheet, id0: u32, a: Point, sig: Point, k: f64, r_ser: f64) -> (Point, Point) {
    // pins [gate, drain, source]
    let m = sh.part(id0, K::Nmos { vt: 1.0, k }, a, S, 4, true);
    let (gate, drain, src) = (m[0], m[1], m[2]);
    debug_assert_eq!(drain, (a.0 - 2, a.1 + 4));
    debug_assert_eq!(src, (a.0 + 2, a.1 + 4));
    sh.wire(sig, drain);
    sh.two(id0 + 1, r(r_ser), src, (a.0 + 8, a.1 + 4));
    (gate, (a.0 + 8, a.1 + 4))
}

// -------------------------------------------------------- MIXER + SPEAKER

/// The output stage every room shares: one op-amp as a virtual-earth current
/// mixer, and a speaker on its output.
///
/// `v(out) = −I_sum · rf`, so voices sum with no crosstalk at whatever
/// impedance each one likes, and the op-amp's branch is the only thing in
/// this engine that can actually drive an 8 Ω coil (an OTA into 8 Ω delivers
/// `gm · 8 ≈ 0.01`).
///
/// THE CEILING IS THE OP-AMP'S, and every room has to be trimmed against it:
/// `DEFAULT_OPAMP_ISC` is 25 mA and the coil is 8 Ω, so the output physically
/// cannot exceed 0.2 V. Past that it stops being a voltage source and becomes
/// a current clamp — a sine comes out a square. Size `rf` so the worst
/// coincidence of voices lands near 0.15 V.
///
/// The speaker takes `speaker_id`, which should be low: the server streams
/// the four lowest-id speakers, so an instrument on id 1 can never be crowded
/// out by something a player drops in later. There is deliberately no level
/// pot before it — the damage gate winds every pot to 0.98.
///
/// Returns `(sum, out)`. 4 devices; occupies about 14 × 6 units from `a`.
pub fn mixer_speaker(sh: &mut Sheet, id0: u32, speaker_id: u32, a: Point, rail: f64, rf: f64) -> (Point, Point) {
    // pins [in+, in-, out]; mirrored so the summing node is the upper input
    // and the feedback resistor can go over the top.
    let m = sh.part(id0, opamp(rail), a, E, 6, true);
    let (plus, sum, out) = (m[0], m[1], m[2]);
    debug_assert_eq!(sum, (a.0, a.1 - 1));
    debug_assert_eq!(out, (a.0 + 6, a.1));
    sh.ground(plus, DOWN);
    sh.run(&[out, (a.0 + 6, a.1 - 5)]);
    sh.two(id0 + 1, r(rf), (a.0 + 6, a.1 - 5), (a.0, a.1 - 5));
    sh.run(&[(a.0, a.1 - 5), sum]);
    sh.wire(out, (a.0 + 8, a.1));
    sh.two(speaker_id, K::Speaker { ohms: 8.0 }, (a.0 + 8, a.1), (a.0 + 12, a.1));
    sh.ground((a.0 + 12, a.1), RIGHT);
    (sum, out)
}

// ------------------------------------------------------------- 555 CLOCK

/// A 555 astable that emits a short TRIGGER, not a 50 % gate: the diode
/// across the timing rheostat is bypassed by the charge current, so
/// `HIGH = 0.693·ra·c` and `LOW = 0.693·(r_min + pot·wiper)·c`.
///
/// This is the only way to get a pulse out of this 555 model, which has no
/// CTRL and no RESET pin. Returns `(out, pot_id)`; the pot takes `id0 + 3`.
///
/// 5 devices. Occupies about 16 × 12 units right and down from `a`, and
/// reaches up to the rail row.
#[allow(clippy::too_many_arguments)]
pub fn clock_555(
    sh: &mut Sheet,
    id0: u32,
    a: Point,
    rail_pt: Point,
    ra: f64,
    r_min: f64,
    pot_ohms: f64,
    wiper: f64,
    farads: f64,
) -> Point {
    // pins [vcc, gnd, trg, thr, out, dis]
    let d = sh.part(id0, K::Timer555, a, E, 4, false);
    let (vcc, gnd, trg, thr, out, dis) = (d[0], d[1], d[2], d[3], d[4], d[5]);
    sh.run(&[rail_pt, (a.0, rail_pt.1), vcc]);
    sh.ground(gnd, DOWN);
    // TRIG strapped to THR down the chip's left side; the middle corner is
    // the timing node everything else hangs on.
    sh.run(&[trg, (a.0 - 1, a.1 + 1), (a.0 - 1, a.1 + 2), (a.0 - 1, a.1 + 3), thr]);
    // RA from the rail straight down into DIS.
    sh.run(&[(a.0, rail_pt.1), (a.0 + 4, rail_pt.1)]);
    sh.two(id0 + 1, r(ra), (a.0 + 4, rail_pt.1), dis);
    // The timing leg out to the right, round the bottom and back.
    sh.wire(dis, (a.0 + 6, dis.1));
    sh.two(id0 + 2, r(r_min), (a.0 + 6, dis.1), (a.0 + 10, dis.1));
    // pins [end a, wiper, end b]; rheostat, wiper strapped to end b.
    let pp = sh.part(id0 + 3, pot(pot_ohms, wiper), (a.0 + 10, dis.1), E, 4, false);
    sh.run(&[pp[1], (a.0 + 14, dis.1 - 2), pp[2]]);
    sh.run(&[pp[2], (a.0 + 14, a.1 + 8), (a.0 - 1, a.1 + 8), (a.0 - 1, a.1 + 3)]);
    // The diode straight across the whole leg, ANODE ON THE DIS SIDE — and
    // that direction is the entire point of the part, not a detail. The
    // charge current runs rail → ra → DIS node → cap, so the diode has to
    // conduct THAT way to shunt the rheostat and make the HIGH time short;
    // pointed the other way it shunts the discharge instead and the 555
    // comes out high about 95 % of the time. A room hung off that gets a
    // trigger that never falls, an envelope capacitor that is never allowed
    // to decay, and a drum that sits silently at DC — which is exactly how
    // this shipped the first time. `Diode`'s anode is pin 0.
    sh.two(id0 + 4, K::Diode, (a.0 + 6, dis.1), (a.0 + 6, a.1 + 8));
    sh.run(&[(a.0 + 6, a.1 + 8), (a.0 - 1, a.1 + 8)]);
    sh.two(id0 + 5, cap(farads), (a.0 - 3, a.1 + 8), (a.0 - 1, a.1 + 8));
    sh.ground((a.0 - 3, a.1 + 8), LEFT);
    out
}

/// Keeps `W` in use for rooms that route westward; the layout module exports
/// all four and clippy would otherwise call the import dead.
#[allow(dead_code)]
pub const WEST: Point = W;
