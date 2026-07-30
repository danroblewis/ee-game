//! Verbatim copy of `demo_room_circuit` from crates/server/src/main.rs at
//! commit 51b1f4d, so "today's demo world" is measured on the real thing
//! rather than a lookalike.

use sim_core::ElementSpec;

/// The showcase room: four vignettes on one shared simulation.
///   A: battery -> switch -> lamp (click me)
///   B: potentiometer -> NPN emitter follower dimming a lamp (drag me)
///   C: slow sine gate on an NMOS switching a lamp, cap softening the edges
///   D: op-amp comparator on a slow sine alternately blinking two LEDs
pub fn demo_room_circuit() -> Vec<ElementSpec> {
    use sim_core::ElementKind as K;
    use sim_golden::{dc, gnd, r, sine, spec, spec3};
    let lamp = |ohms: f64, watts: f64| K::Lamp {
        ohms,
        rated_watts: watts,
    };
    vec![
        // ---- A: lamp loop (top-left)
        spec(1, dc(9.0), (2, 2), (2, 8)),
        spec(2, K::Wire, (2, 2), (7, 2)),
        spec(3, K::Switch { closed: false }, (7, 2), (11, 2)),
        spec(4, K::Wire, (11, 2), (16, 2)),
        spec(5, lamp(90.0, 1.0), (16, 2), (16, 8)),
        spec(6, K::Wire, (16, 8), (9, 8)),
        gnd(7, (9, 8)),
        spec(8, K::Wire, (9, 8), (2, 8)),
        // ---- B: pot -> NPN follower lamp dimmer (top-right)
        spec(10, dc(9.0), (22, 2), (22, 8)),
        spec(11, K::Wire, (22, 2), (26, 2)),
        spec(12, K::Wire, (26, 2), (33, 2)),
        // End a at the bottom rail so dragging the wiper up raises the
        // base voltage (drag up = brighter).
        spec3(
            13,
            K::Potentiometer {
                ohms: 10_000.0,
                wiper: 0.5,
            },
            (26, 8),
            (28, 5),
            (26, 2),
        ),
        spec(14, r(1000.0), (28, 5), (31, 5)),
        // pins: [base, collector, emitter]
        spec3(15, K::Npn { beta: 100.0 }, (31, 5), (33, 2), (33, 6)),
        spec(16, lamp(60.0, 0.4), (33, 6), (33, 8)),
        spec(17, K::Wire, (33, 8), (26, 8)),
        spec(18, K::Wire, (26, 8), (24, 8)),
        gnd(19, (24, 8)),
        spec(20, K::Wire, (24, 8), (22, 8)),
        // ---- C: NMOS slow switch with capacitor fade (bottom-left)
        spec(30, dc(9.0), (2, 12), (2, 18)),
        spec(31, K::Wire, (2, 12), (6, 12)),
        spec(32, lamp(60.0, 0.6), (6, 12), (10, 12)),
        spec(33, K::Wire, (10, 12), (12, 12)),
        spec(34, K::Wire, (12, 12), (12, 13)),
        // pins: [gate, drain, source]
        spec3(
            35,
            K::Nmos { vt: 1.5, k: 0.05 },
            (10, 15),
            (12, 13),
            (12, 17),
        ),
        spec(36, K::Wire, (12, 17), (12, 18)),
        spec(37, K::Wire, (12, 18), (6, 18)),
        spec(38, K::Wire, (6, 18), (2, 18)),
        // Gate driver: 3 V ± 3 V at 0.3 Hz sweeps through the 1.5 V threshold.
        spec(
            39,
            K::VoltageSource {
                dc: 3.0,
                amp: 3.0,
                hz: 0.3,
                phase: 0.0,
            },
            (6, 15),
            (6, 18),
        ),
        spec(40, K::Wire, (6, 15), (10, 15)),
        gnd(41, (6, 18)),
        spec(42, K::Capacitor { farads: 5e-3 }, (6, 10), (10, 10)),
        spec(43, K::Wire, (6, 12), (6, 10)),
        spec(44, K::Wire, (10, 12), (10, 10)),
        // ---- D: comparator blinker (bottom-right)
        spec(50, sine(2.0, 0.4), (22, 13), (22, 18)),
        spec(51, K::Wire, (22, 13), (26, 13)),
        // pins: [in+, in-, out]
        spec3(52, K::OpAmp { rail: 5.0 }, (26, 13), (26, 15), (30, 14)),
        spec(53, K::Wire, (26, 15), (24, 15)),
        spec(54, K::Wire, (24, 15), (24, 18)),
        spec(55, r(220.0), (30, 14), (33, 14)),
        spec(56, K::Led { color: 0 }, (33, 14), (33, 18)),
        spec(57, K::Wire, (33, 14), (35, 14)),
        spec(58, K::Led { color: 1 }, (35, 18), (35, 14)),
        spec(59, K::Wire, (33, 18), (35, 18)),
        spec(60, K::Wire, (24, 18), (33, 18)),
        spec(61, K::Wire, (22, 18), (24, 18)),
        gnd(62, (24, 18)),
        // ---- E: op-amp relaxation oscillator (astable multivibrator).
        // Schmitt hysteresis from R1/R2 positive feedback (thresholds
        // ±rail/2), RC integrator on in-. f ≈ 1/(2·RC·ln3) ≈ 1 Hz; the
        // op-amp input offset self-starts it. LED blinks each + half.
        spec3(70, K::OpAmp { rail: 5.0 }, (6, 26), (6, 24), (10, 25)),
        spec(71, K::Wire, (10, 25), (12, 25)),
        spec(72, r(100_000.0), (12, 25), (12, 21)), // Rf: out -> in-
        spec(73, K::Wire, (12, 21), (4, 21)),
        spec(74, K::Wire, (4, 21), (4, 24)),
        spec(75, K::Wire, (4, 24), (6, 24)),
        spec(76, K::Capacitor { farads: 4.7e-6 }, (4, 24), (4, 28)),
        gnd(77, (4, 28)),
        spec(78, r(100_000.0), (12, 25), (12, 29)), // R1: out -> in+
        spec(79, K::Wire, (12, 29), (9, 29)),
        spec(80, K::Wire, (9, 29), (9, 26)),
        spec(81, K::Wire, (9, 26), (6, 26)),
        spec(82, r(100_000.0), (9, 29), (9, 32)), // R2: in+ -> ground
        gnd(83, (9, 32)),
        spec(84, r(470.0), (12, 25), (15, 25)),
        spec(85, K::Led { color: 3 }, (15, 25), (15, 29)),
        gnd(86, (15, 29)),
        // ---- F: half-wave rectifier with filter cap (τ=0.6 s vs 1 s
        // cycle -> visible sawtooth ripple; the lamp pulses gently).
        spec(90, sine(6.0, 1.0), (20, 22), (20, 26)),
        spec(91, K::Wire, (20, 22), (23, 22)),
        spec(92, K::Diode, (23, 22), (26, 22)),
        spec(93, K::Wire, (26, 22), (29, 22)),
        spec(94, K::Capacitor { farads: 10e-3 }, (26, 22), (26, 26)),
        spec(
            95,
            K::Lamp {
                ohms: 60.0,
                rated_watts: 0.3,
            },
            (29, 22),
            (29, 26),
        ),
        spec(96, K::Wire, (29, 26), (26, 26)),
        spec(97, K::Wire, (26, 26), (23, 26)),
        gnd(98, (23, 26)),
        spec(99, K::Wire, (23, 26), (20, 26)),
        // ---- G: zener shunt regulator feeding an LED: 9 V in, 5.6 V
        // held at the node, steady ~10 mA through the LED.
        spec(100, dc(9.0), (33, 22), (33, 26)),
        spec(101, r(220.0), (33, 22), (37, 22)),
        spec(102, K::Zener { vz: 5.6 }, (37, 26), (37, 22)), // anode down
        spec(103, r(330.0), (37, 22), (40, 22)),
        spec(104, K::Led { color: 2 }, (40, 22), (40, 26)),
        spec(105, K::Wire, (40, 26), (37, 26)),
        spec(106, K::Wire, (37, 26), (35, 26)),
        gnd(107, (35, 26)),
        spec(108, K::Wire, (35, 26), (33, 26)),
        // ---- H: OTA voltage-controlled oscillator. The OTA charges the
        // cap with ±Iabc (triangle); the op-amp Schmitt (1M/2M ->
        // thresholds ±2.5 V) flips the OTA input. Drag the pot: Iabc =
        // (Vwiper - 0.6)/100k sweeps the frequency ~0.05..8 Hz. The LED
        // blinks at the VCO rate.
        ElementSpec {
            id: 120,
            kind: K::Ota,
            pins: vec![(4, 36), (4, 38), (8, 37), (6, 40)],
        },
        spec(121, K::Wire, (4, 36), (2, 36)),
        gnd(122, (2, 36)),
        spec(123, K::Capacitor { farads: 1e-6 }, (8, 37), (8, 41)),
        gnd(124, (8, 41)),
        spec(125, r(1_000_000.0), (8, 37), (13, 37)), // triangle -> Schmitt in+
        // Schmitt trigger pins: [in+, in-, out]
        spec3(130, K::OpAmp { rail: 5.0 }, (13, 37), (13, 39), (17, 38)),
        spec(131, r(2_000_000.0), (19, 34), (19, 38)), // feedback
        spec(132, K::Wire, (17, 38), (19, 38)),
        spec(133, K::Wire, (19, 34), (13, 34)),
        spec(134, K::Wire, (13, 34), (13, 37)),
        spec(135, K::Wire, (13, 39), (11, 39)),
        gnd(136, (11, 39)),
        // Loop: square wave back to the OTA inverting input.
        spec(137, K::Wire, (17, 38), (17, 42)),
        spec(138, K::Wire, (17, 42), (2, 42)),
        spec(139, K::Wire, (2, 42), (2, 38)),
        spec(140, K::Wire, (2, 38), (4, 38)),
        // Rate indicator.
        spec(141, r(470.0), (17, 38), (21, 38)),
        spec(142, K::Led { color: 4 }, (21, 38), (21, 42)),
        gnd(143, (21, 42)),
        // Control: battery -> pot -> 100k -> bias pin.
        spec(144, dc(9.0), (25, 34), (25, 42)),
        gnd(145, (25, 42)),
        spec(146, K::Wire, (25, 34), (27, 34)),
        spec(147, K::Wire, (25, 42), (27, 42)),
        spec3(
            148,
            K::Potentiometer {
                ohms: 10_000.0,
                wiper: 0.4,
            },
            (27, 42),
            (29, 38),
            (27, 34),
        ),
        spec(149, r(100_000.0), (29, 38), (29, 44)),
        spec(150, K::Wire, (29, 44), (6, 44)),
        spec(151, K::Wire, (6, 44), (6, 40)),
    ]
}
