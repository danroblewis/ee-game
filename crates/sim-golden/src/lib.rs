//! Golden circuits: known-good netlists with closed-form expected
//! behavior. The library exposes builders so the determinism harness, the
//! benchmarks, and the tests all exercise the exact same circuits.

pub mod scale;

use sim_core::{ElementKind, ElementSpec, Point};

pub fn spec(id: u32, kind: ElementKind, a: Point, b: Point) -> ElementSpec {
    ElementSpec::two(id, kind, a, b)
}

pub fn spec3(id: u32, kind: ElementKind, a: Point, b: Point, c: Point) -> ElementSpec {
    ElementSpec::three(id, kind, a, b, c)
}

pub fn gnd(id: u32, at: Point) -> ElementSpec {
    ElementSpec::ground(id, at)
}

pub fn dc(volts: f64) -> ElementKind {
    ElementKind::VoltageSource {
        dc: volts,
        amp: 0.0,
        hz: 0.0,
        phase: 0.0,
    }
}

pub fn sine(amp: f64, hz: f64) -> ElementKind {
    ElementKind::VoltageSource {
        dc: 0.0,
        amp,
        hz,
        phase: 0.0,
    }
}

pub fn r(ohms: f64) -> ElementKind {
    ElementKind::Resistor { ohms }
}

/// 10 V source, 1 kΩ, 1 µF: v_c(t) = 10 (1 - e^(-t/τ)), τ = 1 ms.
pub fn rc_step() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(10.0), (0, 0), (0, 8)),
        spec(2, r(1000.0), (0, 0), (8, 0)),
        spec(3, ElementKind::Capacitor { farads: 1e-6 }, (8, 0), (0, 8)),
        gnd(4, (0, 8)),
    ]
}

/// 5 V source, 100 Ω, 10 mH: i(t) = 0.05 (1 - e^(-t/τ)), τ = 100 µs.
pub fn rl_step() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(5.0), (0, 0), (0, 8)),
        spec(2, r(100.0), (0, 0), (8, 0)),
        spec(3, ElementKind::Inductor { henries: 10e-3 }, (8, 0), (0, 8)),
        gnd(4, (0, 8)),
    ]
}

/// Lightly damped series RLC (1 Ω, 1 mH, 1 µF) driven by a 1 V step:
/// f0 ≈ 5.03 kHz, Q ≈ 31.6.
pub fn rlc_ring() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(1.0), (0, 0), (0, 8)),
        spec(2, r(1.0), (0, 0), (4, 0)),
        spec(3, ElementKind::Inductor { henries: 1e-3 }, (4, 0), (8, 0)),
        spec(4, ElementKind::Capacitor { farads: 1e-6 }, (8, 0), (0, 8)),
        gnd(5, (0, 8)),
    ]
}

/// Half-wave rectifier: 10 V / 60 Hz sine, diode, 1 kΩ ∥ 100 µF load.
pub fn half_wave_rectifier() -> Vec<ElementSpec> {
    vec![
        spec(1, sine(10.0, 60.0), (0, 0), (0, 8)),
        spec(2, ElementKind::Diode, (0, 0), (8, 0)),
        spec(3, r(1000.0), (8, 0), (0, 8)),
        spec(4, ElementKind::Capacitor { farads: 100e-6 }, (8, 0), (0, 8)),
        gnd(5, (0, 8)),
    ]
}

/// The M1 demo: battery -> switch -> lamp.
pub fn demo_lamp(closed: bool) -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 4)),
        spec(2, ElementKind::Wire, (0, 0), (4, 0)),
        spec(3, ElementKind::Switch { closed }, (4, 0), (8, 0)),
        spec(
            4,
            ElementKind::Lamp {
                ohms: 90.0,
                rated_watts: 1.0,
            },
            (8, 0),
            (8, 4),
        ),
        spec(5, ElementKind::Wire, (8, 4), (0, 4)),
        gnd(6, (0, 4)),
    ]
}

/// NPN saturated switch: 9 V rail, 100 Ω collector load, base driven
/// through 3.3 kΩ (β·Ib ≈ 250 mA >> 90 mA needed). Expect hard
/// saturation: V(c) well under 0.5 V.
pub fn npn_switch() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 10)),
        spec(2, r(100.0), (0, 0), (4, 2)),  // rail -> collector
        spec(3, r(3300.0), (0, 0), (4, 6)), // rail -> base
        // pins: [base, collector, emitter]
        spec3(4, ElementKind::Npn { beta: 100.0 }, (4, 6), (4, 2), (4, 10)),
        spec(5, ElementKind::Wire, (4, 10), (0, 10)),
        gnd(6, (0, 10)),
    ]
}

/// NPN emitter follower: base held at 4.5 V by a stiff divider, emitter
/// through 1 kΩ to ground. Expect V(e) ≈ 4.5 - V(be) ≈ 3.8-3.95.
pub fn emitter_follower() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 10)),
        spec(2, r(1000.0), (0, 0), (4, 4)),  // divider top
        spec(3, r(1000.0), (4, 4), (0, 10)), // divider bottom
        spec(4, ElementKind::Wire, (0, 0), (8, 0)),
        spec3(5, ElementKind::Npn { beta: 100.0 }, (4, 4), (8, 0), (8, 6)),
        spec(6, r(1000.0), (8, 6), (0, 10)), // emitter load
        gnd(7, (0, 10)),
    ]
}

/// NMOS low-side switch: gate at 5 V (vt 1.5), 90 Ω load from 9 V rail.
/// Expect the lamp on: I ≈ 90+ mA, V(ds) under 1 V.
pub fn nmos_switch() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 10)),
        spec(2, dc(5.0), (12, 6), (12, 10)),
        spec(3, r(90.0), (0, 0), (6, 2)), // rail -> drain
        // pins: [gate, drain, source]
        spec3(
            4,
            ElementKind::Nmos { vt: 1.5, k: 0.05 },
            (12, 6),
            (6, 2),
            (6, 10),
        ),
        spec(5, ElementKind::Wire, (6, 10), (0, 10)),
        spec(6, ElementKind::Wire, (12, 10), (0, 10)),
        gnd(7, (0, 10)),
    ]
}

/// Op-amp voltage follower driving a 1 kΩ load; input 2 V DC.
pub fn opamp_follower() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(2.0), (0, 0), (0, 8)),
        // pins: [in+, in-, out]; out wired back to in-.
        spec3(2, ElementKind::OpAmp { rail: 13.5, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 0), (6, 4), (8, 0)),
        spec(3, ElementKind::Wire, (8, 0), (6, 4)),
        spec(4, r(1000.0), (8, 0), (0, 8)),
        gnd(5, (0, 8)),
    ]
}

/// Op-amp comparator: +1 V vs ground; output must sit on the +5 rail.
pub fn opamp_comparator() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(1.0), (0, 0), (0, 8)),
        spec3(2, ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 0), (4, 6), (8, 0)),
        spec(3, ElementKind::Wire, (4, 6), (0, 8)), // in- to ground
        spec(4, r(1000.0), (8, 0), (0, 8)),
        gnd(5, (0, 8)),
    ]
}

/// Zener shunt regulator: 9 V through 330 Ω into a 5.6 V zener.
pub fn zener_regulator() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 8)),
        spec(2, r(330.0), (0, 0), (6, 0)),
        // Cathode to the regulated node: anode = pin 0 at ground side.
        spec(3, ElementKind::Zener { vz: 5.6 }, (0, 8), (6, 0)),
        gnd(4, (0, 8)),
    ]
}

/// Unloaded potentiometer across 9 V, wiper at 0.3 from end a:
/// V(wiper) = 9 · (1 - 0.3) = 6.3 V.
pub fn pot_divider() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 8)),
        // pins: [end a, wiper, end b]
        spec3(
            2,
            ElementKind::Potentiometer {
                ohms: 10_000.0,
                wiper: 0.3,
            },
            (0, 0),
            (4, 4),
            (0, 8),
        ),
        gnd(3, (0, 8)),
    ]
}

/// Op-amp relaxation oscillator (astable multivibrator): Schmitt
/// hysteresis from R1/R2 positive feedback, RC integration on the
/// inverting input. Period ≈ 2·RC·ln(3) ≈ 2.2 ms; self-starts via the
/// op-amp's input offset voltage.
pub fn opamp_relaxation() -> Vec<ElementSpec> {
    vec![
        // pins: [in+, in-, out]
        spec3(1, ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 6), (0, 2), (4, 4)),
        // integrator: Rf out -> in-, C in- -> ground
        spec(2, r(10_000.0), (4, 4), (8, 4)),
        spec(3, ElementKind::Wire, (8, 4), (8, 0)),
        spec(4, ElementKind::Wire, (8, 0), (0, 0)),
        spec(5, ElementKind::Wire, (0, 0), (0, 2)),
        spec(
            6,
            ElementKind::Capacitor { farads: 100e-9 },
            (0, 2),
            (-4, 2),
        ),
        gnd(7, (-4, 2)),
        // hysteresis divider: R1 out -> in+, R2 in+ -> ground
        spec(8, r(10_000.0), (4, 4), (4, 8)),
        spec(9, ElementKind::Wire, (4, 8), (0, 8)),
        spec(10, ElementKind::Wire, (0, 8), (0, 6)),
        spec(11, r(10_000.0), (0, 8), (-4, 8)),
        gnd(12, (-4, 8)),
    ]
}

/// OTA-based voltage-controlled oscillator (LM13700-datasheet style).
/// The OTA sinks/sources ±Iabc into an integrating cap (triangle wave);
/// an op-amp Schmitt trigger (thresholds ±2.5 V) flips the OTA input.
/// Iabc = (vctrl - ~0.65 V)/100k, f = Iabc / (4·C·2.5 V) — frequency is
/// proportional to the control voltage.
pub fn ota_vco(vctrl: f64) -> Vec<ElementSpec> {
    vec![
        // OTA pins: [in+, in-, out, bias]
        ElementSpec {
            id: 1,
            kind: ElementKind::Ota,
            pins: vec![(0, 0), (0, 2), (4, 1), (2, 4)],
            tier: 0,
            rot: 0,
        },
        gnd(2, (0, 0)), // in+ grounded: OTA inverts the square wave
        spec(3, ElementKind::Capacitor { farads: 10e-9 }, (4, 1), (4, 5)),
        gnd(4, (4, 5)),
        // Schmitt trigger: non-inverting comparator with hysteresis.
        spec(5, r(1_000_000.0), (4, 1), (8, 1)), // triangle -> in+ (light load!)
        spec3(6, ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (8, 1), (8, 3), (12, 2)),
        gnd(7, (8, 3)),
        spec(8, r(2_000_000.0), (12, 2), (12, -2)), // feedback: out -> in+
        spec(9, ElementKind::Wire, (12, -2), (8, -2)),
        spec(10, ElementKind::Wire, (8, -2), (8, 1)),
        // Square wave back to the OTA inverting input.
        spec(11, ElementKind::Wire, (12, 2), (12, 6)),
        spec(12, ElementKind::Wire, (12, 6), (-2, 6)),
        spec(13, ElementKind::Wire, (-2, 6), (-2, 2)),
        spec(14, ElementKind::Wire, (-2, 2), (0, 2)),
        // Control voltage -> R -> bias pin (sets Iabc).
        spec(15, dc(vctrl), (16, 4), (16, 8)),
        gnd(16, (16, 8)),
        spec(17, r(100_000.0), (16, 4), (12, 4)),
        spec(18, ElementKind::Wire, (12, 4), (2, 4)),
    ]
}

/// The textbook 555 astable multivibrator on a 9 V rail: RA = RB = 10 kΩ,
/// C = 100 nF. The cap charges through RA+RB up to 2/3 Vcc, then the DIS
/// pin saturates and it discharges through RB down to 1/3 Vcc:
/// f = 1.44/((RA + 2·RB)·C) ≈ 480 Hz, duty = (RA+RB)/(RA+2·RB) ≈ 67 %.
/// OUT drives a 1 kΩ load so the totem pole actually carries current.
pub fn timer555_astable() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 10)),
        gnd(2, (0, 10)),
        spec(3, ElementKind::Wire, (0, 0), (2, 0)),
        spec(4, ElementKind::Wire, (2, 0), (6, 0)),
        // pins: [vcc, gnd, trig, thr, out, dis]
        ElementSpec {
            id: 5,
            kind: ElementKind::Timer555,
            pins: vec![(6, 0), (6, 10), (4, 6), (4, 4), (10, 4), (2, 2)],
            tier: 0,
            rot: 0,
        },
        spec(6, r(10_000.0), (2, 0), (2, 2)), // RA: rail -> DIS
        spec(7, r(10_000.0), (2, 2), (2, 4)), // RB: DIS -> THR/TRIG
        spec(8, ElementKind::Wire, (2, 4), (4, 4)),
        spec(9, ElementKind::Wire, (4, 4), (4, 6)), // THR tied to TRIG
        spec(
            10,
            ElementKind::Capacitor { farads: 100e-9 },
            (2, 4),
            (2, 6),
        ),
        gnd(11, (2, 6)),
        spec(12, ElementKind::Wire, (0, 10), (6, 10)),
        spec(13, r(1000.0), (10, 4), (10, 6)),
        gnd(14, (10, 6)),
    ]
}

/// DC motor armature (the hoist motor: 2 Ω, 1.5 mH) fed from 12 V through a
/// 1 Ω feeder, with the rotor turning fast enough to generate 2 V of
/// back-EMF. The armature is an RL branch with a series EMF, so
/// i(t) = (12 - 2)/3 · (1 - e^(-t/τ)) with τ = L/R_loop = 0.5 ms and
/// i_ss = 3.3333 A. (Back-EMF is an input here; the mechanism that produces
/// it lives in the `machine` crate.)
pub fn motor_step() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(12.0), (0, 0), (0, 8)),
        spec(2, r(1.0), (0, 0), (4, 0)),
        spec(
            3,
            ElementKind::Motor {
                ohms: 2.0,
                henries: 1.5e-3,
                bemf: 2.0,
            },
            (4, 0),
            (0, 8),
        ),
        gnd(4, (0, 8)),
    ]
}

/// LED through 330 Ω from 9 V: forward drop ≈ 2.1 V, I ≈ 21 mA.
pub fn led_loop() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 8)),
        spec(2, r(330.0), (0, 0), (6, 0)),
        spec(3, ElementKind::Led { color: 0 }, (6, 0), (0, 8)),
        gnd(4, (0, 8)),
    ]
}

/// White-noise source (1 V peak behind 1 kΩ, seed 12345) into a 4.7 kΩ /
/// 10 nF lowpass — the front end of a snare voice, and the smallest circuit
/// that puts the PRNG on the cross-target harness.
///
/// The generator is integer-only and counter-based, so this circuit has no
/// closed-form waveform but it has exact reproducibility: native and wasm32
/// must produce the same sample sequence, hence the same state hash. The RC
/// pole (fc = 1/(2π·4.7k·10n) = 3.39 kHz) is the band-limiting every noise
/// path needs before an audio tap, so the golden exercises the real usage.
pub fn noise_rc() -> Vec<ElementSpec> {
    vec![
        ElementSpec::two(
            1,
            ElementKind::Noise {
                volts: 1.0,
                ohms: 1000.0,
                seed: 12345,
            },
            (0, 0),
            (0, 8),
        ),
        spec(2, r(4700.0), (0, 0), (6, 0)),
        spec(3, ElementKind::Capacitor { farads: 10e-9 }, (6, 0), (0, 8)),
        spec(4, r(100_000.0), (6, 0), (0, 8)),
        gnd(5, (0, 8)),
    ]
}

/// Every golden circuit, for the determinism harness.
pub fn all_golden() -> Vec<(&'static str, Vec<ElementSpec>)> {
    vec![
        ("demo_lamp", demo_lamp(true)),
        ("rc_step", rc_step()),
        ("rl_step", rl_step()),
        ("rlc_ring", rlc_ring()),
        ("half_wave_rectifier", half_wave_rectifier()),
        ("npn_switch", npn_switch()),
        ("emitter_follower", emitter_follower()),
        ("nmos_switch", nmos_switch()),
        ("opamp_follower", opamp_follower()),
        ("opamp_comparator", opamp_comparator()),
        ("zener_regulator", zener_regulator()),
        ("opamp_relaxation", opamp_relaxation()),
        ("ota_vco", ota_vco(5.0)),
        ("timer555_astable", timer555_astable()),
        ("pot_divider", pot_divider()),
        ("led_loop", led_loop()),
        ("motor_step", motor_step()),
        ("noise_rc", noise_rc()),
    ]
}
