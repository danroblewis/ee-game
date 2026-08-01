//! Room server: one authoritative simulation, many browsers.
//!
//! M4-lite protocol (JSON over WebSocket, upgraded to the three-class
//! binary transport in M4/M5):
//!   server -> client: hello{you, elements}, frame{time, e}, op{id, op},
//!                     presence{n}, cursor{who, x, y},
//!                     samples{t0, dts, s} (probe scopes, 3.125 kHz),
//!                     audio{t0, dts, rt, s} (speaker taps, 12.5 kHz; `rt` is
//!                       sim seconds produced per wall second)
//!   client -> server: interact{id, op}, cursor{x, y}
//!
//! `samples` and `audio` are both best-effort: they ride the same broadcast
//! channel as `frame`, so a lagged consumer skips chunks. A dropped chunk
//! costs a few milliseconds of a trace or of silence and desyncs nothing —
//! the next chunk carries its own absolute `t0`.
//!                     presence{n}, cursor{who, x, y}, machine{...},
//!                     damage{parts:[[id, stress, broken], ...]}
//!   client -> server: interact{id, op}, cursor{x, y}, machinereset{},
//!                     machinemove{dx, dy}, repair{id}
//!
//! `damage` is a lossy SNAPSHOT, not a delta: each message lists everything
//! worth drawing (dead parts first), so a client replaces its whole damage
//! map from it and a dropped message costs one frame of staleness. A room
//! with nothing stressed sends none at all.

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use damage::DamageModel;
use machine::Hoist;
use serde::Deserialize;
use serde_json::json;
use sim_core::{DocOp, ElementKind, ElementSpec, Engine, InteractOp, ParamWrite};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::{broadcast, mpsc};
use tower_http::services::{ServeDir, ServeFile};

// The showcase room has slow dynamics (sub-Hz sines, 0.3 s RC fades), so
// the plan's 5-20 µs configurable band lets us sit at the cheap end while
// the mixed nonlinear circuit refactors every NR iteration.
const DT: f64 = 20e-6;
const TICK_HZ: f64 = 30.0;
/// Wall budget per tick: cap sim work so a heavy circuit dilates sim time
/// instead of stalling the loop (Falstad's rule, plan resolution).
const MAX_STEPS_PER_TICK: u32 = 8000;

/// Machine co-simulation cadence: the mechanism integrates once per 32
/// substeps (640 µs at DT = 20 µs). Between chunks the server reads the
/// motor's branch current out of the solver and writes back-EMF, sensor
/// wiper and limit-switch positions back in. h_m/τ_mech = 0.026, so the
/// mechanism's explicit Euler is nowhere near its stability limit.
const MACHINE_SUBSTEPS: u32 = 32;
const MACHINE_H: f64 = MACHINE_SUBSTEPS as f64 * DT;

/// Most parts listed in one `damage` snapshot. The message is lossy by
/// design (see the header): dead parts take the slots first, because those
/// are the ones a player has to go and find.
///
/// Cost: a row is ~22 bytes of JSON, so a fully wrecked room caps out around
/// 11 kB/tick (340 kB/s) — the same order as one speaker's audio tap, and
/// only while hundreds of parts are actually broken. A healthy room sends
/// nothing at all.
const MAX_DAMAGE_REPORT: usize = 512;

/// The showcase room: four vignettes on one shared simulation.
///   A: battery -> switch -> lamp (click me)
///   B: potentiometer -> NPN emitter follower dimming a lamp (drag me)
///   C: slow sine gate on an NMOS switching a lamp, cap softening the edges
///   D: op-amp comparator on a slow sine alternately blinking two LEDs
///
/// Lamp nameplates here are sized for what their own vignette actually
/// drives them with, which matters now that ratings have teeth: vignette B at
/// full brightness and vignette C fully switched on both put ~1.15 W into
/// their bulb, so a 0.4 W nameplate would have meant the showcase burned
/// itself out the first time someone dragged the dimmer to the top.
fn demo_room_circuit() -> Vec<ElementSpec> {
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
        spec(16, lamp(60.0, 1.2), (33, 6), (33, 8)),
        spec(17, K::Wire, (33, 8), (26, 8)),
        spec(18, K::Wire, (26, 8), (24, 8)),
        gnd(19, (24, 8)),
        spec(20, K::Wire, (24, 8), (22, 8)),
        // ---- C: NMOS slow switch with capacitor fade (bottom-left)
        spec(30, dc(9.0), (2, 12), (2, 18)),
        spec(31, K::Wire, (2, 12), (6, 12)),
        spec(32, lamp(60.0, 1.2), (6, 12), (10, 12)),
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
                rated_watts: 0.6,
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
        // ---- I: 555 astable blinking an LED at ~1 Hz. RA = RB = 100k,
        // C = 4.7 µF -> f = 1.44/((RA + 2·RB)·C) ≈ 1.0 Hz, duty ≈ 67 %.
        // The cap charges through RA+RB to 2/3 Vcc, then DIS saturates and
        // it drains through RB to 1/3 Vcc. Hold the pushbutton to ground
        // TRIG: the trigger comparator wins, so the output pins high and
        // the LED stays lit until you let go.
        spec(160, dc(9.0), (34, 36), (34, 48)),
        gnd(161, (34, 48)),
        spec(162, K::Wire, (34, 36), (34, 34)),
        spec(163, K::Wire, (34, 34), (52, 34)), // rail, routed over the chip
        spec(164, K::Wire, (34, 36), (40, 36)), // rail -> VCC pin
        // pins: [vcc, gnd, trig, thr, out, dis]
        ElementSpec {
            id: 165,
            kind: K::Timer555,
            pins: vec![(40, 36), (40, 44), (40, 38), (40, 42), (46, 42), (46, 38)],
        },
        spec(166, r(100_000.0), (52, 34), (52, 38)), // RA: rail -> DIS
        spec(167, K::Wire, (52, 38), (46, 38)),
        spec(168, r(100_000.0), (52, 38), (52, 42)), // RB: DIS -> THR/TRIG
        spec(169, K::Wire, (52, 42), (52, 46)),
        spec(170, K::Wire, (52, 46), (38, 46)), // routed under the chip
        spec(171, K::Wire, (38, 46), (38, 42)),
        spec(172, K::Wire, (38, 42), (40, 42)), // -> THR pin
        spec(173, K::Wire, (38, 42), (38, 38)),
        spec(174, K::Wire, (38, 38), (40, 38)), // -> TRIG pin (tied to THR)
        spec(175, K::Capacitor { farads: 4.7e-6 }, (38, 46), (38, 48)),
        gnd(176, (38, 48)),
        spec(177, K::Wire, (40, 44), (36, 44)), // GND pin
        gnd(178, (36, 44)),
        // Manual retrigger: hold to short TRIG to ground.
        spec(179, K::Wire, (38, 38), (36, 38)),
        spec(180, K::Button { closed: false }, (36, 38), (36, 40)),
        gnd(181, (36, 40)),
        // Blinker on OUT.
        spec(182, r(470.0), (46, 42), (49, 42)),
        spec(183, K::Led { color: 3 }, (49, 42), (49, 44)),
        gnd(184, (49, 44)),
        // ---- J: concert A. A 440 Hz source through a series 8 Ω into an
        // 8 Ω speaker: close the switch and you HEAR it, because the server
        // streams the coil's own terminal voltage at 12.5 kHz. The switch
        // starts open so joining a room is quiet.
        spec(200, sine(5.0, 440.0), (2, 52), (2, 60)),
        spec(201, K::Wire, (2, 52), (5, 52)),
        spec(202, K::Switch { closed: false }, (5, 52), (8, 52)),
        spec(203, r(8.0), (8, 52), (11, 52)),
        spec(204, K::Speaker { ohms: 8.0 }, (11, 52), (11, 60)),
        spec(205, K::Wire, (11, 60), (2, 60)),
        gnd(206, (2, 60)),
    ]
}

// ------------------------------------------------------------- the hoist
//
// THE HOIST — the room's first goal, in its own district east of the
// showcase vignettes (which occupy x ≤ 40).
//
// A crate hangs on a platform in a vertical shaft, driven by a DC motor
// whose two leads are real wire-able terminals. A green band is painted
// across the shaft; the faceplate reads "CRATE IN BAND — HOLD 5.0 s".
// Wiring a constant voltage lifts the crate but cannot hold it — voltage
// buys speed, not position — so holding it needs feedback from the
// position sensor. There is no quest log and nothing to accept: the goal
// is measured from solver quantities and nothing else.

/// The hoist's footprint SIZE in grid units. Fixed: the assembly MOVES (a
/// player drags the cabinet, see `move_machine`), it does not resize.
const HOIST_W: i32 = 18;
const HOIST_H: i32 = 22;

/// The footprint of a hoist whose top-left corner is at (x0, y0), in GRID
/// units — broadcast to clients as `rect`. All hoist chrome is drawn inside
/// it and every fixture pin is derived from it, so the box and its terminals
/// can never drift apart.
const fn hoist_rect(x0: i32, y0: i32) -> [i32; 4] {
    [x0, y0, x0 + HOIST_W, y0 + HOIST_H]
}

/// Where a fresh room stands the hoist: its own district east of the showcase
/// vignettes (which occupy x <= 40). The live footprint is room STATE from
/// here on (persisted in `SaveFile::hoist_rect`), not a constant.
const HOIST_RECT: [i32; 4] = hoist_rect(46, 2);

/// Grid coordinates the machine may occupy. The world is meant to be huge, so
/// this is a guard against a runaway client (or a corrupt save) parking the
/// machine at 1e9 where no player could ever find it again — not a design
/// limit on where a machine may live.
const WORLD_LIMIT: i32 = 1_000_000;

/// Largest single assembly move accepted, in grid units. A drag sends small
/// increments; one undo sends the whole gesture back. Anything past this is a
/// malformed client, not a player.
const MAX_MACHINE_STEP: i32 = 100_000;

/// Force a footprint back onto its invariants: normalized corners, the fixed
/// size, origin inside the world range. Used on load (a save may predate the
/// field, or be hand-edited) and on every move.
fn sane_rect(r: [i32; 4]) -> [i32; 4] {
    let x0 = r[0].min(r[2]).clamp(-WORLD_LIMIT, WORLD_LIMIT - HOIST_W);
    let y0 = r[1].min(r[3]).clamp(-WORLD_LIMIT, WORLD_LIMIT - HOIST_H);
    hoist_rect(x0, y0)
}

/// The four locked fixture elements. Players wire to their terminals but
/// cannot move, edit or delete them: ids 900-999 are server-owned and every
/// doc op touching them is refused.
const MOTOR_ID: u32 = 900;
const SENSOR_ID: u32 = 901;
const LIM_TOP_ID: u32 = 902;
const LIM_BOT_ID: u32 = 903;

/// Position-sensor pot resistance; light enough to drive a comparator
/// input, heavy enough not to load a player's supply.
const SENSOR_OHMS: f64 = 10_000.0;

/// Ids in this range belong to machine fixtures.
fn reserved_id(id: u32) -> bool {
    (900..=999).contains(&id)
}

/// The fixture, laid out on the faceplate inside the footprint:
///   900 Motor         [M+, M-]
///   901 Potentiometer [SENSE-A, SENSE-W, SENSE-B]  (the position sensor)
///   902 Switch        [LIM-TOP-a, LIM-TOP-b]
///   903 Switch        [LIM-BOT-a, LIM-BOT-b]
/// The shaft itself takes the left of the rect; the terminal column sits on
/// the right so wires can reach it without crossing the crate.
///
/// EVERY pin is derived from the rect's origin — this function is the whole
/// "terminal map" of the assembly, and the reason a move cannot separate a
/// terminal from its machine.
fn hoist_fixture_at(rect: [i32; 4]) -> Vec<ElementSpec> {
    let [x0, y0, ..] = rect;
    let (a, b) = (x0 + 11, x0 + 15); // terminal column
    vec![
        ElementSpec::two(
            MOTOR_ID,
            ElementKind::Motor {
                ohms: machine::R_ARM,
                henries: machine::L_ARM,
                bemf: 0.0,
            },
            (a, y0 + 3),
            (a, y0 + 7),
        ),
        ElementSpec::three(
            SENSOR_ID,
            ElementKind::Potentiometer {
                ohms: SENSOR_OHMS,
                // Crate on the floor: wiper = 1 - y/H, clamped off the end.
                wiper: machine::WIPER_MAX,
            },
            (a, y0 + 10),
            (b, y0 + 12),
            (a, y0 + 14),
        ),
        ElementSpec::two(
            LIM_TOP_ID,
            ElementKind::Switch { closed: false },
            (a, y0 + 17),
            (b, y0 + 17),
        ),
        ElementSpec::two(
            LIM_BOT_ID,
            // Closed: the crate starts on the floor.
            ElementKind::Switch { closed: true },
            (a, y0 + 20),
            (b, y0 + 20),
        ),
    ]
}

/// The fixture on the default footprint. Test-only: the live room always has a
/// rect to derive from, and reaching for the default in the running server is
/// exactly the drift this refactor removed.
#[cfg(test)]
fn hoist_fixture() -> Vec<ElementSpec> {
    hoist_fixture_at(HOIST_RECT)
}

/// The hoist motor's nameplate current (A), read from the damage table so
/// the faceplate can never disagree with the model that enforces it.
///
/// Design note, and the reason the goal card no longer says "wire 12 V to
/// M+/M−": at 12 V the armature draws V/R = 6 A whenever the rotor is not
/// turning — at start, and forever once the crate parks against the head
/// stop. That is twice this rating, so an uncontrolled 12 V lead cooks the
/// motor in a couple of seconds. Running current is ~0.94 A and a controlled
/// drive's switching transients average well under the rating, so the
/// intended solution survives indefinitely.
fn motor_i_max() -> f64 {
    let motor = ElementKind::Motor {
        ohms: machine::R_ARM,
        henries: machine::L_ARM,
        bemf: 0.0,
    };
    damage::rating(&motor).map(|r| r.limit).unwrap_or(0.0)
}

/// Stand the fixture up inside `rect`: inject anything missing — a checkpoint
/// written before the hoist existed has none of it, and it can never be
/// removed after — and put every surviving child's pins where the rect says
/// they go. Persisted VALUES survive (a restored wiper or limit-switch
/// position is real state); pin GEOMETRY is always re-derived, which is what
/// keeps the footprint and its terminals in lockstep across a move and across
/// a reload.
///
/// Returns (id, pins) for each child, ready to broadcast as `DocOp::Move`.
fn ensure_fixture(
    elems: &mut Vec<ElementSpec>,
    rect: [i32; 4],
) -> Vec<(u32, Vec<sim_core::Point>)> {
    let mut moved = Vec::with_capacity(4);
    for spec in hoist_fixture_at(rect) {
        let (id, pins) = (spec.id, spec.pins.clone());
        match elems.iter_mut().find(|e| e.id == id) {
            // A save written before ids 900-999 were reserved could hold a
            // player's part on a fixture id. The fixture wins: the machine
            // would otherwise be writing back-EMF into someone's resistor.
            Some(e) if std::mem::discriminant(&e.kind) != std::mem::discriminant(&spec.kind) => {
                *e = spec;
            }
            Some(e) => e.pins = pins.clone(),
            None => elems.push(spec),
        }
        moved.push((id, pins));
    }
    moved
}

/// Move the whole hoist assembly by an integer grid delta: the footprint AND
/// all four child fixtures, together, in one shot. This is the ONLY way any
/// of them moves — `apply_doc_op` refuses a client `DocOp::Move` on a
/// reserved id — so a player can never separate a terminal from its machine.
/// Returns the children's new pins for the broadcast, or None when the move is
/// refused (no-op, absurd step, or a destination outside the world range).
///
/// SEAM — this is the whole assembly abstraction, deliberately hard-wired to
/// THIS machine. A future generic `Container` part would need, per instance:
///   * a CHILD LIST (here: the four reserved ids, implied by `hoist_fixture_at`);
///   * a FOOTPRINT (here: the room's single `hoist_rect`);
///   * a TERMINAL MAP from child pins to footprint-relative offsets (here:
///     `hoist_fixture_at`, which derives every pin from the rect's origin);
///   * PER-INSTANCE WORLD STATE carried along untouched (here: the one
///     `Hoist`) — a translation is not a reset.
///
/// Everything else in this function is generic already.
fn move_machine(
    elems: &mut Vec<ElementSpec>,
    rect: &mut [i32; 4],
    dx: i32,
    dy: i32,
) -> Option<Vec<(u32, Vec<sim_core::Point>)>> {
    if dx == 0 && dy == 0 {
        return None; // nothing to broadcast, nothing to checkpoint
    }
    // `unsigned_abs`, not `abs`: a client is free to send i32::MIN, and
    // negating that panics in debug and wraps in release.
    let step = MAX_MACHINE_STEP.unsigned_abs();
    if dx.unsigned_abs() > step || dy.unsigned_abs() > step {
        return None;
    }
    // Checked arithmetic for the same reason — a hostile delta must be a
    // dropped message, never a panicking sim task.
    let (x0, y0) = (rect[0].checked_add(dx)?, rect[1].checked_add(dy)?);
    // Every corner of the box (and therefore every derived pin) has to land
    // somewhere a player could plausibly follow it to.
    if !(-WORLD_LIMIT..=WORLD_LIMIT - HOIST_W).contains(&x0)
        || !(-WORLD_LIMIT..=WORLD_LIMIT - HOIST_H).contains(&y0)
    {
        return None;
    }
    *rect = hoist_rect(x0, y0);
    Some(ensure_fixture(elems, *rect))
}

/// Ids of the document's sources, cached for the energy meter.
fn source_ids(elems: &[ElementSpec]) -> Vec<u32> {
    elems
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ElementKind::VoltageSource { .. } | ElementKind::CurrentSource { .. }
            )
        })
        .map(|e| e.id)
        .collect()
}

/// Power the sources are delivering right now (W), from solver quantities
/// only: p = (v0 - v1)·i is negative when a source pushes current out of its
/// + terminal, and a sinking source earns no refund.
fn source_watts(eng: &Engine, ids: &[u32]) -> f64 {
    let mut watts = 0.0;
    for id in ids {
        let (Some(v0), Some(v1), Some(i)) = (
            eng.pin_voltage(*id, 0),
            eng.pin_voltage(*id, 1),
            eng.pin_current(*id, 0),
        ) else {
            continue;
        };
        let p = (v0 - v1) * i;
        if p < 0.0 {
            watts -= p;
        }
    }
    watts
}

/// Put a broken part back into service: clear the bookkeeping, then tell the
/// solver to stamp it again. Returns false when the id was not broken, so a
/// stale click from a client costs nothing.
///
/// Both halves matter and they must stay together: clearing only the model
/// would leave a part that reads healthy and conducts nothing, and clearing
/// only the engine would leave one that conducts and reads dead.
fn apply_repair(damage: &mut DamageModel, eng: &mut Engine, id: u32) -> bool {
    if !damage.repair(id) {
        return false;
    }
    eng.set_broken(id, false);
    true
}

/// One machine tick: read the motor's armature current out of the solver,
/// integrate the mechanism, write its state back into the live circuit.
///
/// `pin_current` (not `frame()`) on purpose: the branch current is an MNA
/// unknown already sitting in the solved vector, while `frame()` is
/// O(elements) and runs KCL propagation for wire currents — 1.5 kHz of that
/// would be the most expensive thing in the room.
fn machine_step(eng: &mut Engine, hoist: &mut Hoist, sources: &[u32]) -> machine::Writes {
    let i = eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0);
    let w = hoist.tick(i, MACHINE_H);
    // Back-EMF is RHS-only; the wiper invalidates the factorization; a limit
    // switch is a topology change and only recompiles when it really moves.
    eng.write_param(MOTOR_ID, ParamWrite::Bemf { volts: w.bemf });
    eng.write_param(SENSOR_ID, ParamWrite::Wiper { frac: w.wiper });
    eng.write_param(LIM_TOP_ID, ParamWrite::Switch { closed: w.lim_top });
    eng.write_param(LIM_BOT_ID, ParamWrite::Switch { closed: w.lim_bot });
    hoist.accumulate_joules(source_watts(eng, sources), MACHINE_H);
    w
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProbeKind {
    V,
    I,
}

#[derive(Clone, Copy, serde::Serialize)]
struct Probe {
    pid: u32,
    elem: u32,
    pin: usize,
    kind: ProbeKind,
    /// Optional reference point for differential voltage measurement:
    /// the trace shows v(pin) - v(ref). None = referenced to ground.
    r: Option<(u32, usize)>,
}

/// Sim substeps between waveform samples: dt=20 µs × 16 → 3.125 kHz
/// effective sample rate per probe.
const SAMPLE_EVERY: u32 = 16;
const MAX_PROBES: usize = 8;

/// Sim substeps between SPEAKER samples: dt=20 µs × 4 → 12.5 kHz, i.e. a
/// 6.25 kHz Nyquist limit. Speakers get their own, four-times-faster
/// cadence than probes because the probe rate is a scope rate, not an audio
/// rate: 3.125 kHz gives a 440 Hz tone only ~7 samples per cycle, which the
/// worklet's linear interpolation turns into a buzzsaw. At 12.5 kHz the same
/// tone gets 28.4 samples per cycle and everything a small speaker can
/// actually reproduce below 6 kHz survives.
///
/// Must divide SAMPLE_EVERY: the tick advances in AUDIO_EVERY-sized chunks
/// and takes a probe sample every `SAMPLE_EVERY / AUDIO_EVERY` of them, so
/// both streams stay exactly aligned to sim time (no label drift).
///
/// Cost, measured on the showcase room (136 elements, 1667 steps/tick,
/// release build): the solver alone is 12.66 ms/tick; adding 4 speaker taps
/// — 416 chunks x 4 taps x 2 pin reads = 3328 O(1) reads, plus the finer
/// `advance` chunking — takes it to 12.73 ms. That is 0.6 % of the tick and
/// ~2 % of the 33 ms wall budget. The same reads through `pin_voltage`'s id
/// scan cost 1.9 us here but grow with the document: ~0.7 ms/tick at 50k
/// elements, which is why `Engine::tap` exists.
const AUDIO_EVERY: u32 = 4;
/// Simultaneously streamed speakers. There is no server-side notion of
/// "nearest to a listener" (the camera is a client concept), so this is
/// simply the first N Speaker elements by element id — deterministic, stable
/// across ticks, and the same set for every client in the room.
const MAX_AUDIO_TAPS: usize = 4;

/// Element ids of the speakers this tick will stream: the lowest
/// MAX_AUDIO_TAPS Speaker ids in the document. O(elements) with a tiny
/// constant, and only called when somebody is listening.
fn audio_tap_ids(elements: &[ElementSpec]) -> Vec<u32> {
    let mut ids: Vec<u32> = Vec::with_capacity(MAX_AUDIO_TAPS + 1);
    for e in elements {
        if !matches!(e.kind, sim_core::ElementKind::Speaker { .. }) {
            continue;
        }
        let at = ids.partition_point(|&x| x < e.id);
        if at >= MAX_AUDIO_TAPS {
            continue;
        }
        ids.insert(at, e.id);
        ids.truncate(MAX_AUDIO_TAPS);
    }
    ids
}

/// Wire-safe sample: quantized to 0.1 mV (a ~94 dB noise floor under a 5 V
/// peak, well below anything a player can hear) and never NaN/±inf — a
/// quarantined solver must produce silence, not a `null` the client has to
/// defend against.
///
/// f64, deliberately: serde_json widens f32 to f64 before printing, so an
/// f32 sample serializes as its exact binary value ("1.2345999479293823",
/// 18 characters) and triples the stream. Quantizing in f64 keeps the short
/// decimal the quantization was for — measured 4x smaller on the wire.
fn wire_sample(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    (v * 10_000.0).round() / 10_000.0
}

/// A control panel: a dotted region of the schematic that gets a
/// mission-control window on every client. Room-scoped like probes, so a
/// panel one player draws is a shared instrument. Only the rectangle is
/// stored — membership is re-derived from element geometry by the client,
/// never persisted (moving a part in or out re-wires the panel live).
#[derive(Clone, serde::Serialize, Deserialize)]
struct Panel {
    plid: u32,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    name: String,
}

/// Document budget. The world is meant to hold enormous circuits (whole
/// districts), so this is a guard against a runaway client, not a design
/// limit. NOTE: the sim is the real ceiling long before this — sim-core
/// still factors a DENSE matrix (O(n³)); the fixed-pattern sparse LU is
/// spike S3 and is not built yet.
const MAX_ELEMENTS: usize = 50_000;

const MAX_PANELS: usize = 256;
const MAX_PANEL_NAME: usize = 28;
/// Smallest accepted region in grid units: a stray click must not make one.
const MIN_PANEL_SPAN: f64 = 1.0;

#[derive(Clone, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum PanelOp {
    Add {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        #[serde(default)]
        name: Option<String>,
    },
    Remove {
        plid: u32,
    },
    /// Move/resize the region (the client drags the name tab).
    Rect {
        plid: u32,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
    },
    Rename {
        plid: u32,
        name: String,
    },
}

enum Cmd {
    Interact {
        id: u32,
        op: InteractOp,
    },
    Edit {
        op: DocOp,
    },
    Probe {
        elem: u32,
        pin: usize,
        kind: ProbeKind,
    },
    ProbeRef {
        pid: u32,
        elem: u32,
        pin: usize,
    },
    Panel {
        op: PanelOp,
    },
    /// Lower the crate to the floor and re-arm the hoist's goal.
    MachineReset,
    /// Drag the whole hoist assembly by an integer grid delta (the player has
    /// the cabinet in hand). Applied at a tick boundary like every other op.
    MachineMove {
        dx: i32,
        dy: i32,
    },
    /// Fix a part that released its magic smoke (the repair tool).
    Repair {
        id: u32,
    },
    Join,
    Leave,
}

struct Room {
    cmds: mpsc::UnboundedSender<Cmd>,
    events: broadcast::Sender<String>,
    /// Element specs kept in sync with applied ops, for `hello` on join.
    elements: std::sync::Mutex<Vec<ElementSpec>>,
    /// Room-scoped probes (shared instrumentation — plan pillar: probes
    /// live on the authoritative tick so cross-player overlay is trivial).
    probes: std::sync::Mutex<Vec<Probe>>,
    /// Room-scoped control-panel regions (shared, same rationale as probes).
    panels: std::sync::Mutex<Vec<Panel>>,
    next_client: AtomicU32,
    next_pid: AtomicU32,
    next_plid: AtomicU32,
    population: AtomicU32,
    /// Set when the document changes; the sim task checkpoints to disk.
    dirty: std::sync::atomic::AtomicBool,
}

/// Room checkpoint: the document, probes and panels survive server restarts
/// (the continuous electrical state re-settles within milliseconds).
#[derive(serde::Serialize, Deserialize)]
struct SaveFile {
    elements: Vec<ElementSpec>,
    #[serde(default)]
    probes: Vec<SavedProbe>,
    #[serde(default)]
    next_pid: u32,
    /// serde defaults: saves written before panels existed still load.
    #[serde(default)]
    panels: Vec<Panel>,
    #[serde(default)]
    next_plid: u32,
    /// Mechanical state of the hoist. Defaulted so saves written before the
    /// hoist existed still load (crate on the floor, goal armed).
    #[serde(default)]
    hoist: Hoist,
    /// Where the hoist stands, in GRID units. Defaulted so saves written
    /// before the machine could be dragged still load, landing it on the
    /// original constant — exactly where those saves' fixture pins already
    /// are. Sanitized through `sane_rect` on load.
    #[serde(default = "default_hoist_rect")]
    hoist_rect: [i32; 4],
    /// Accumulated thermal stress and the broken set. Defaulted, so a save
    /// written before parts could break still loads (everything healthy).
    #[serde(default)]
    damage: DamageModel,
}

fn default_hoist_rect() -> [i32; 4] {
    HOIST_RECT
}

#[derive(serde::Serialize, Deserialize)]
struct SavedProbe {
    pid: u32,
    elem: u32,
    pin: usize,
    kind: ProbeKind,
    #[serde(default)]
    r: Option<(u32, usize)>,
}

fn save_path() -> String {
    std::env::var("EE_SAVE").unwrap_or_else(|_| "room-save.json".into())
}

fn load_room() -> Option<SaveFile> {
    let data = std::fs::read_to_string(save_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn checkpoint(room: &Room, hoist: &Hoist, hoist_rect: [i32; 4], damage: &DamageModel) {
    let save = SaveFile {
        hoist: *hoist,
        hoist_rect,
        damage: damage.clone(),
        elements: room.elements.lock().unwrap().clone(),
        probes: room
            .probes
            .lock()
            .unwrap()
            .iter()
            .map(|p| SavedProbe {
                pid: p.pid,
                elem: p.elem,
                pin: p.pin,
                kind: p.kind,
                r: p.r,
            })
            .collect(),
        next_pid: room.next_pid.load(Ordering::Relaxed),
        panels: room.panels.lock().unwrap().clone(),
        next_plid: room.next_plid.load(Ordering::Relaxed),
    };
    if let Ok(json) = serde_json::to_string(&save) {
        let path = save_path();
        let tmp = format!("{path}.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Smoothing for the realtime ratio: one EMA pole at 0.2 is ~5 ticks (170 ms),
/// slow enough that one late tick does not flash a warning at the player, fast
/// enough that flipping a heavy circuit on shows up immediately.
const RT_ALPHA: f64 = 0.2;

/// Sim seconds produced per wall second, smoothed — the server's own honesty
/// about how dilated it is.
///
/// The tick advances at most `MAX_STEPS_PER_TICK` substeps and the loop never
/// waits for more (Falstad's rule: a heavy circuit slows SIM time, never the
/// loop). Speaker audio is generated on that sim clock and consumed by the
/// client's sound card on the WALL clock, so this ratio IS the rate at which
/// every listener's ring buffer drains: at 0.6x, six seconds of audio arrive
/// for every ten seconds played, and no client-side buffering or resampling
/// can fix that. Reported so the client can say "the circuit is too heavy"
/// instead of "the sound is broken".
///
/// A quarantined solver advances no sim time at all, so this correctly falls
/// to 0: there is no audio being produced.
fn blend_realtime(prev: f64, advanced: f64, wall: f64) -> f64 {
    // A wall gap of zero (or a clock that went backwards, or a sim time that
    // did — a room reload) carries no information: keep the last estimate.
    // NaN fails `is_finite`, so it takes this exit too rather than poisoning
    // the EMA forever.
    if !wall.is_finite() || wall <= 1e-6 || !advanced.is_finite() || advanced < 0.0 {
        return prev;
    }
    let inst = (advanced / wall).clamp(0.0, 4.0);
    let next = prev + (inst - prev) * RT_ALPHA;
    if next.is_finite() {
        next
    } else {
        1.0
    }
}

/// `{"t":"audio", t0, dts, rt, s:{elemId: [...]}}` — one speaker chunk per tap.
/// Keyed by ELEMENT id (not pid): speaker audio exists whether or not anyone
/// probed the part, which is the whole point of this stream.
///
/// `rt` is `blend_realtime`'s ratio. It rides the AUDIO message rather than
/// `frame` for three reasons: it is a property of this stream's production
/// rate, so a client never has to correlate two messages to explain a chunk;
/// it costs nothing in a room with no speakers, where nobody would read it;
/// and `frame` is the per-element payload that becomes the binary transport
/// in M4/M5, whose layout should not grow a scalar that belongs to audio.
fn audio_message(
    t0: f64,
    dts: f64,
    rt: f64,
    taps: &[(u32, sim_core::ElemTap)],
    bufs: Vec<Vec<f64>>,
) -> String {
    let s: serde_json::Map<String, serde_json::Value> = taps
        .iter()
        .zip(bufs)
        .map(|((id, _), b)| (id.to_string(), serde_json::json!(b)))
        .collect();
    // Three decimals: 0.001x of dilation is 1 ms of audio per second, far
    // below anything the readout or the ear can act on.
    let rt = (rt * 1000.0).round() / 1000.0;
    json!({"t": "audio", "t0": t0, "dts": dts, "rt": rt, "s": s}).to_string()
}

/// The sim task: sole owner of the Engine. Ops apply between ticks —
/// the "tick boundary" rule from the plan, at demo scale.
async fn sim_task(
    room: Arc<Room>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    mut hoist: Hoist,
    // The hoist's footprint: owned here, beside the mechanism it belongs to,
    // so a move and a machine tick can never interleave.
    mut hoist_rect: [i32; 4],
    mut damage: DamageModel,
) {
    let mut eng = Engine::new(DT);
    let mut sources;
    {
        let elems = room.elements.lock().unwrap().clone();
        sources = source_ids(&elems);
        eng.set_elements(&elems);
        // Restored damage: the ratings are re-derived from the document (they
        // are never persisted) and every part that was dead when the server
        // stopped is dead again before the first step runs.
        damage.set_document(&elems);
        for id in damage.broken_ids() {
            eng.set_broken(id, true);
        }
    }
    // Was the last damage snapshot non-empty? One empty snapshot is sent
    // after the room goes quiet, so clients clear their overlays; after that,
    // silence costs nothing.
    let mut damage_shown = false;

    let tick = std::time::Duration::from_secs_f64(1.0 / TICK_HZ);
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let steps_per_tick = ((1.0 / TICK_HZ) / DT).round() as u32;
    let mut ticks_since_save: u32 = 0;
    // Dilation tracking: sim time produced vs wall time spent. Measured at the
    // TOP of the loop, so it includes everything a tick costs (solve, command
    // drain, serialization, scheduler slip) — which is what a client's sound
    // card experiences. `rt_ready` skips the first pass, where no sim time has
    // been advanced yet and the ratio would read 0.
    let mut rt = 1.0f64;
    let mut rt_ready = false;
    let mut last_wall = std::time::Instant::now();
    let mut last_sim = eng.time();

    loop {
        interval.tick().await;

        {
            let now = std::time::Instant::now();
            let wall = now.duration_since(last_wall).as_secs_f64();
            last_wall = now;
            let sim = eng.time();
            let advanced = sim - last_sim;
            last_sim = sim;
            if rt_ready {
                rt = blend_realtime(rt, advanced, wall);
            }
            rt_ready = true;
        }

        // Checkpoint the document every ~5 s when it has changed.
        ticks_since_save += 1;
        if ticks_since_save >= 150 && room.dirty.swap(false, Ordering::Relaxed) {
            ticks_since_save = 0;
            checkpoint(&room, &hoist, hoist_rect, &damage);
        }

        // Assembly moves arriving this tick, summed and applied once below.
        let mut pending_move = (0i32, 0i32);
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Cmd::Interact { id, op } => {
                    // The hoist fixture is server-owned: no knob drags, no
                    // hand-flipping its limit switches.
                    if reserved_id(id) {
                        continue;
                    }
                    eng.interact(id, op);
                    apply_to_specs(&room, id, op);
                    let _ = room
                        .events
                        .send(json!({"t": "op", "id": id, "op": op}).to_string());
                }
                Cmd::MachineReset => {
                    hoist.reset();
                    room.dirty.store(true, Ordering::Relaxed);
                }
                Cmd::MachineMove { dx, dy } => {
                    // Coalesced, not applied here: a drag sends ~2 ops per
                    // tick and translation is additive, so summing them costs
                    // one netlist recompile per tick instead of one per op.
                    // Saturating, because the sum of hostile deltas must be a
                    // refused move rather than a panic.
                    pending_move.0 = pending_move.0.saturating_add(dx);
                    pending_move.1 = pending_move.1.saturating_add(dy);
                }
                Cmd::Repair { id } => {
                    // Deliberately NOT a document op: a repair is a world
                    // event, so it is allowed on the server-owned hoist
                    // fixture (ids 900-999) and it never enters anyone's undo
                    // history. The next tick's snapshot tells every client.
                    if apply_repair(&mut damage, &mut eng, id) {
                        room.dirty.store(true, Ordering::Relaxed);
                        tracing::info!("part #{id} repaired");
                    }
                }
                Cmd::Edit { op } => {
                    if apply_doc_op(&room, &op) {
                        room.dirty.store(true, Ordering::Relaxed);
                        let elems = room.elements.lock().unwrap().clone();
                        sources = source_ids(&elems);
                        eng.set_elements(&elems); // continuous state survives by id
                                                  // Ratings follow the document (a SetKind can change
                                                  // them); stress and the broken set follow the id, so
                                                  // moving a dead part does not repair it. An
                                                  // InteractOp cannot change a rating, so knob drags
                                                  // deliberately skip this.
                        damage.set_document(&elems);
                        // Probes on a removed element die with it.
                        if let DocOp::Remove { id } = &op {
                            let mut probes = room.probes.lock().unwrap();
                            let before = probes.len();
                            probes.retain(|p| p.elem != *id);
                            let mut ref_cleared = false;
                            for p in probes.iter_mut() {
                                if matches!(p.r, Some((e, _)) if e == *id) {
                                    p.r = None;
                                    ref_cleared = true;
                                }
                            }
                            if probes.len() != before || ref_cleared {
                                let _ = room
                                    .events
                                    .send(json!({"t": "probes", "list": *probes}).to_string());
                            }
                        }
                        let _ = room.events.send(json!({"t": "doc", "op": op}).to_string());
                    }
                }
                Cmd::Probe { elem, pin, kind } => {
                    let mut probes = room.probes.lock().unwrap();
                    room.dirty.store(true, Ordering::Relaxed);
                    if let Some(k) = probes
                        .iter()
                        .position(|p| p.elem == elem && p.pin == pin && p.kind == kind)
                    {
                        probes.remove(k); // toggle off
                    } else if probes.len() < MAX_PROBES
                        && room
                            .elements
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|e| e.id == elem && pin < e.pins.len())
                    {
                        let pid = room.next_pid.fetch_add(1, Ordering::Relaxed);
                        probes.push(Probe {
                            pid,
                            elem,
                            pin,
                            kind,
                            r: None,
                        });
                    }
                    let _ = room
                        .events
                        .send(json!({"t": "probes", "list": *probes}).to_string());
                }
                Cmd::ProbeRef { pid, elem, pin } => {
                    let mut probes = room.probes.lock().unwrap();
                    room.dirty.store(true, Ordering::Relaxed);
                    if let Some(p) = probes.iter_mut().find(|p| p.pid == pid) {
                        // Same point again clears the reference (ground).
                        p.r = match p.r {
                            Some((e, n)) if e == elem && n == pin => None,
                            _ => Some((elem, pin)),
                        };
                        let _ = room
                            .events
                            .send(json!({"t": "probes", "list": *probes}).to_string());
                    }
                }
                Cmd::Panel { op } => {
                    let mut panels = room.panels.lock().unwrap();
                    if apply_panel_op(&mut panels, &room.next_plid, &op) {
                        room.dirty.store(true, Ordering::Relaxed);
                        let _ = room
                            .events
                            .send(json!({"t": "panels", "list": *panels}).to_string());
                    }
                }
                Cmd::Join | Cmd::Leave => {
                    let n = room.population.load(Ordering::Relaxed);
                    let _ = room
                        .events
                        .send(json!({"t": "presence", "n": n}).to_string());
                }
            }
        }

        // The assembly move, applied once at this tick boundary: one atomic
        // translation of the footprint AND its four children. The mechanism
        // (height, velocity, hold timer, landing count) is deliberately
        // untouched — dragging the box is a move, not a reset.
        if pending_move != (0, 0) {
            let moved = {
                let mut elems = room.elements.lock().unwrap();
                move_machine(&mut elems, &mut hoist_rect, pending_move.0, pending_move.1)
            };
            if let Some(children) = moved {
                room.dirty.store(true, Ordering::Relaxed);
                // The children's pins moved, so the netlist's geometry did:
                // recompile (continuous state survives by id). `sources`
                // cannot change — a move touches no element's kind.
                let elems = room.elements.lock().unwrap().clone();
                eng.set_elements(&elems);
                // The children reach every client through the ordinary doc
                // path, and the new footprint rides this tick's `machine`
                // message — so a client that never sent the op is consistent
                // within one tick.
                for (id, pins) in children {
                    let op = DocOp::Move { id, pins };
                    let _ = room.events.send(json!({"t": "doc", "op": op}).to_string());
                }
            }
        }

        // Advance the tick in nested cadences. The mechanism co-simulates
        // every MACHINE_SUBSTEPS, probes sample every SAMPLE_EVERY and speaker
        // taps every AUDIO_EVERY; 32 = 2 x 16 = 8 x 4, so one inner step
        // serves all three and every sample lands on an exact sim time.
        let probes = room.probes.lock().unwrap().clone();
        // Nobody subscribed = no audio work at all: a room full of speakers
        // with an empty gallery must cost exactly what a silent one costs.
        let listeners = room.events.receiver_count() > 0;
        let taps: Vec<(u32, sim_core::ElemTap)> = if listeners {
            // Resolved once per tick (after the edit drain, so the handles
            // match the netlist this tick will actually step).
            audio_tap_ids(&room.elements.lock().unwrap())
                .into_iter()
                .filter_map(|id| eng.tap(id).map(|t| (id, t)))
                .collect()
        } else {
            Vec::new()
        };
        let budget = steps_per_tick.min(MAX_STEPS_PER_TICK);
        // Machine + goal state the tick reports afterwards. `motor_i` is
        // seeded from the current netlist so a tick that quarantines still
        // reports the last honest reading rather than zero.
        let mut motor_i = eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0);
        let mut impact = 0.0f64;
        let won_before = hoist.win;
        // The machine's last write-back this tick, mirrored into the stored
        // document after the loop (the consumer is outside this scope).
        let mut writes: Option<machine::Writes> = None;
        // Sim time at the top of the tick: the damage model integrates over
        // the time the solver ACTUALLY advanced, so a budget-limited or
        // quarantined tick cooks less (or nothing) rather than pretending a
        // full 33 ms passed.
        let tick_t0 = eng.time();
        {
            // Finest cadence anyone needs this tick (the machine always runs).
            let step = if !taps.is_empty() {
                AUDIO_EVERY
            } else if !probes.is_empty() {
                SAMPLE_EVERY
            } else {
                MACHINE_SUBSTEPS
            };
            let per_probe = (SAMPLE_EVERY / step).max(1);
            let per_machine = (MACHINE_SUBSTEPS / step).max(1);
            // Whole machine periods, which are also whole probe periods: a
            // ragged tail would leak substeps of drift into `t0 + k * dts`.
            let chunks = (budget / MACHINE_SUBSTEPS).max(1) * per_machine;
            let t0 = eng.time();
            let mut bufs: Vec<Vec<f32>> =
                vec![Vec::with_capacity((chunks / per_probe) as usize); probes.len()];
            let mut abufs: Vec<Vec<f64>> = vec![Vec::with_capacity(chunks as usize); taps.len()];
            for c in 0..chunks {
                eng.advance(step);
                // Speakers: the drive ACROSS the coil, v(pin0) - v(pin1),
                // which is what a voltage-driven cone follows. Read through
                // the O(1) tap — frame() is O(elements) and runs KCL.
                for (buf, (_, tap)) in abufs.iter_mut().zip(taps.iter()) {
                    buf.push(wire_sample(eng.tap_delta(*tap, 0, 1)));
                }
                if !probes.is_empty() && (c + 1) % per_probe == 0 {
                    sample_probes(&eng, &probes, &mut bufs);
                }
                // A quarantined solver has no current to report, so the
                // machine freezes with it rather than coasting on stale
                // numbers.
                if (c + 1) % per_machine == 0 && !eng.is_quarantined() {
                    motor_i = eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0);
                    writes = Some(machine_step(&mut eng, &mut hoist, &sources));
                    impact = impact.max(hoist.impact);
                }
            }
            if listeners && !probes.is_empty() {
                let s: serde_json::Map<String, serde_json::Value> = probes
                    .iter()
                    .zip(bufs)
                    .map(|(p, b)| (p.pid.to_string(), serde_json::json!(b)))
                    .collect();
                let _ = room.events.send(
                    json!({
                        "t": "samples",
                        "t0": t0,
                        "dts": DT * SAMPLE_EVERY as f64,
                        "s": s,
                    })
                    .to_string(),
                );
            }
            // A separate message type so scope decimation and speaker audio
            // never have to agree on a cadence. Best-effort like `frame`: a
            // lagged consumer skips chunks, which costs a few ms of silence
            // and desyncs nothing (the client re-primes on the time gap).
            if !taps.is_empty() {
                let _ =
                    room.events
                        .send(audio_message(t0, DT * AUDIO_EVERY as f64, rt, &taps, abufs));
            }
        }

        // Keep the stored document tracking what the machine drives, so
        // `hello` and checkpoints carry the fixture's real state. Clients
        // render the hoist from the machine message, not from these.
        if let Some(w) = writes {
            let mut elems = room.elements.lock().unwrap();
            for e in elems.iter_mut() {
                match (e.id, &mut e.kind) {
                    (SENSOR_ID, ElementKind::Potentiometer { wiper, .. }) => *wiper = w.wiper,
                    (LIM_TOP_ID, ElementKind::Switch { closed }) => *closed = w.lim_top,
                    (LIM_BOT_ID, ElementKind::Switch { closed }) => *closed = w.lim_bot,
                    _ => {}
                }
            }
        }

        // ---- damage: one frame per tick, shared with the broadcast below.
        //
        // The document is swept ONCE here. A quarantined solver publishes no
        // new numbers, so nothing accumulates stress from stale ones — a
        // frozen circuit cannot cook a part.
        let fr = eng.frame();
        if !eng.is_quarantined() {
            for b in damage.tick(&fr, eng.time() - tick_t0) {
                // The mechanism: sim-core now stamps this part as an open
                // circuit. Everything else about the failure lives outside it.
                eng.set_broken(b.id, true);
                room.dirty.store(true, Ordering::Relaxed);
                tracing::info!(
                    "{} #{} released its magic smoke ({:.1}x its limit)",
                    b.kind,
                    b.id,
                    b.load
                );
            }
        }

        if room.events.receiver_count() > 0 {
            if let Some(msg) = damage_msg(&damage, &mut damage_shown) {
                let _ = room.events.send(msg);
            }
            // Same flat layout as the WASM facade:
            // [id, npins, v0..v5, i0..i5, power].
            let e: Vec<[f64; 15]> = fr
                .iter()
                .map(|f| {
                    [
                        f.id as f64,
                        f.npins as f64,
                        f.v[0],
                        f.v[1],
                        f.v[2],
                        f.v[3],
                        f.v[4],
                        f.v[5],
                        f.i[0],
                        f.i[1],
                        f.i[2],
                        f.i[3],
                        f.i[4],
                        f.i[5],
                        f.power,
                    ]
                })
                .collect();
            let _ = room
                .events
                .send(
                    // `rt` rides every frame so the client's status strip can
                    // report dilation without a speaker in the room (the
                    // audio stream carries its own copy for rate matching).
                    json!({"t": "frame", "time": eng.time(), "e": e,
                           "rt": (rt * 1000.0).round() / 1000.0})
                    .to_string(),
                );

            // The hoist, once per tick alongside the frame.
            let _ = room
                .events
                .send(machine_msg(&hoist, hoist_rect, motor_i, impact, motor_i_max()).to_string());
        }
        // A win is worth a checkpoint even if nobody edited anything.
        if hoist.win && !won_before {
            room.dirty.store(true, Ordering::Relaxed);
        }
    }
}

/// The hoist's per-tick broadcast — the fixed client contract.
///
/// Every number in it is a solver quantity or an integral of one: `i` is the
/// motor's branch unknown, `y`/`vel` are integrals of `i`, `hold` is measured
/// from `y`, and `joules` integrates source power. Nothing here is asserted.
/// `rect` is the LIVE footprint in GRID units — room state now, since the
/// assembly is draggable — so the client can draw all hoist chrome (and
/// hit-test the cabinet) without hardcoding geometry; `impact` is non-zero
/// only on the tick a landing happened. `imax` is the motor's nameplate
/// current from the damage table — the client engraves it on the faceplate
/// rather than hardcoding a number that could drift from the model that
/// enforces it.
fn machine_msg(
    hoist: &Hoist,
    rect: [i32; 4],
    motor_i: f64,
    impact: f64,
    i_max: f64,
) -> serde_json::Value {
    json!({
        "imax": i_max,
        "t": "machine",
        "id": MOTOR_ID,
        "rect": rect,
        "h": machine::SHAFT_H,
        "band": [machine::BAND_LO, machine::BAND_HI],
        "y": hoist.y,
        "vel": hoist.velocity(),
        "i": motor_i,
        "hold": hoist.hold,
        "need": machine::HOLD_NEED,
        "impact": impact,
        "landings": hoist.landings,
        "win": hoist.win,
        "joules": hoist.joules,
    })
}

/// This tick's `damage` broadcast, or `None` when there is nothing to say.
///
/// The message is a full SNAPSHOT, so a client can rebuild its whole damage
/// overlay from any one of them — which is also why exactly one EMPTY
/// snapshot follows the last repair: without it, a client that had drawn a
/// broken part would keep drawing it forever. `shown` carries that one bit of
/// state across ticks.
fn damage_msg(damage: &DamageModel, shown: &mut bool) -> Option<String> {
    let report = damage.report(MAX_DAMAGE_REPORT);
    if report.is_empty() && !*shown {
        return None; // a quiet room costs nothing at all
    }
    *shown = !report.is_empty();
    Some(json!({"t": "damage", "parts": report}).to_string())
}

/// Push one sample per probe into its buffer, from the last solved step.
fn sample_probes(eng: &Engine, probes: &[Probe], bufs: &mut [Vec<f32>]) {
    // Wire currents come from KCL propagation (frame-only).
    let need_frame = probes
        .iter()
        .any(|p| p.kind == ProbeKind::I && eng.is_wire(p.elem));
    let fr = need_frame.then(|| eng.frame());
    for (buf, p) in bufs.iter_mut().zip(probes.iter()) {
        let v = match (p.kind, &fr) {
            (ProbeKind::V, _) => {
                let v = eng.pin_voltage(p.elem, p.pin);
                // Differential: subtract the reference point.
                let vref =
                    p.r.and_then(|(re, rp)| eng.pin_voltage(re, rp))
                        .unwrap_or(0.0);
                v.map(|v| v - vref)
            }
            (ProbeKind::I, Some(fr)) => fr.iter().find(|f| f.id == p.elem).map(|f| f.i[p.pin]),
            (ProbeKind::I, None) => eng.pin_current(p.elem, p.pin),
        };
        buf.push(v.unwrap_or(0.0) as f32);
    }
}

/// Mirror an applied op into the stored specs so late joiners get current
/// switch positions and values.
fn apply_to_specs(room: &Room, id: u32, op: InteractOp) {
    use sim_core::ElementKind as K;
    let mut elems = room.elements.lock().unwrap();
    let Some(e) = elems.iter_mut().find(|e| e.id == id) else {
        return;
    };
    match (op, &mut e.kind) {
        (InteractOp::SetSwitch { closed }, K::Switch { closed: c })
        | (InteractOp::SetSwitch { closed }, K::Button { closed: c }) => *c = closed,
        (InteractOp::SetValue { value }, K::Resistor { ohms })
        | (InteractOp::SetValue { value }, K::Lamp { ohms, .. })
        | (InteractOp::SetValue { value }, K::Speaker { ohms }) => *ohms = value.max(1e-6),
        (InteractOp::SetValue { value }, K::Capacitor { farads }) => *farads = value.max(1e-15),
        (InteractOp::SetValue { value }, K::Inductor { henries }) => *henries = value.max(1e-12),
        (InteractOp::SetValue { value }, K::VoltageSource { dc, .. }) => *dc = value,
        (InteractOp::SetValue { value }, K::CurrentSource { amps }) => *amps = value,
        (InteractOp::SetValue { value }, K::Potentiometer { wiper, .. }) => {
            *wiper = value.clamp(0.01, 0.99)
        }
        _ => {}
    }
}

/// Validate and apply a document edit. Returns false to drop the op
/// (malformed id, unknown id, or a server-owned fixture) — the full
/// permission/rules pipeline is M4.
fn apply_doc_op(room: &Room, op: &DocOp) -> bool {
    // Machine fixtures (ids 900-999) cannot be added, moved, reconfigured or
    // deleted by players. Wiring TO their terminals is untouched: that is an
    // op on the player's own wire.
    let target = match op {
        DocOp::Add { spec } => spec.id,
        DocOp::Remove { id } | DocOp::Move { id, .. } | DocOp::SetKind { id, .. } => *id,
    };
    if reserved_id(target) {
        return false;
    }
    let mut elems = room.elements.lock().unwrap();
    match op {
        DocOp::Add { spec } => {
            if spec.pins.len() != spec.kind.pin_count()
                || elems.iter().any(|e| e.id == spec.id)
                || elems.len() >= MAX_ELEMENTS
            {
                return false;
            }
            elems.push(spec.clone());
            true
        }
        DocOp::Remove { id } => {
            let before = elems.len();
            elems.retain(|e| e.id != *id);
            elems.len() != before
        }
        DocOp::Move { id, pins } => {
            let Some(e) = elems.iter_mut().find(|e| e.id == *id) else {
                return false;
            };
            if pins.len() != e.kind.pin_count() {
                return false;
            }
            e.pins = pins.clone();
            true
        }
        DocOp::SetKind { id, kind } => {
            let Some(e) = elems.iter_mut().find(|e| e.id == *id) else {
                return false;
            };
            if kind.pin_count() != e.pins.len() {
                return false;
            }
            e.kind = *kind;
            true
        }
    }
}

fn clean_panel_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(MAX_PANEL_NAME)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Normalize a drag rectangle. None = degenerate or non-finite input, which
/// the caller drops (same "validate then apply" rule as document ops).
fn norm_panel_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Option<(f64, f64, f64, f64)> {
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return None;
    }
    let (ax, bx) = (x0.min(x1), x0.max(x1));
    let (ay, by) = (y0.min(y1), y0.max(y1));
    if bx - ax < MIN_PANEL_SPAN || by - ay < MIN_PANEL_SPAN {
        return None;
    }
    Some((ax, ay, bx, by))
}

/// Apply a panel op to the room's panel list. Returns false to drop the op
/// (malformed rect, unknown plid, panel budget reached).
fn apply_panel_op(panels: &mut Vec<Panel>, next_plid: &AtomicU32, op: &PanelOp) -> bool {
    match op {
        PanelOp::Add {
            x0,
            y0,
            x1,
            y1,
            name,
        } => {
            let Some((x0, y0, x1, y1)) = norm_panel_rect(*x0, *y0, *x1, *y1) else {
                return false;
            };
            if panels.len() >= MAX_PANELS {
                return false;
            }
            let plid = next_plid.fetch_add(1, Ordering::Relaxed);
            let name = name
                .as_deref()
                .map(clean_panel_name)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("PANEL {plid}"));
            panels.push(Panel {
                plid,
                x0,
                y0,
                x1,
                y1,
                name,
            });
            true
        }
        PanelOp::Remove { plid } => {
            let before = panels.len();
            panels.retain(|p| p.plid != *plid);
            panels.len() != before
        }
        PanelOp::Rect {
            plid,
            x0,
            y0,
            x1,
            y1,
        } => {
            let Some((x0, y0, x1, y1)) = norm_panel_rect(*x0, *y0, *x1, *y1) else {
                return false;
            };
            let Some(p) = panels.iter_mut().find(|p| p.plid == *plid) else {
                return false;
            };
            (p.x0, p.y0, p.x1, p.y1) = (x0, y0, x1, y1);
            true
        }
        PanelOp::Rename { plid, name } => {
            let name = clean_panel_name(name);
            if name.is_empty() {
                return false;
            }
            let Some(p) = panels.iter_mut().find(|p| p.plid == *plid) else {
                return false;
            };
            p.name = name;
            true
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum ClientMsg {
    Interact {
        id: u32,
        op: InteractOp,
    },
    Edit {
        op: DocOp,
    },
    Probe {
        elem: u32,
        pin: usize,
        kind: ProbeKind,
    },
    ProbeRef {
        pid: u32,
        elem: u32,
        pin: usize,
    },
    Panel {
        op: PanelOp,
    },
    Cursor {
        x: f64,
        y: f64,
    },
    /// Lower the crate and re-arm the hoist's goal.
    MachineReset,
    /// Move the whole hoist assembly by an integer grid delta. Deltas, not an
    /// absolute rect: a drag is a stream of small increments and one undo is
    /// the negated total, so nothing has to agree on a coordinate system.
    MachineMove {
        dx: i32,
        dy: i32,
    },
    /// The repair tool: put a broken part back into service.
    Repair {
        id: u32,
    },
}

async fn ws_handler(ws: WebSocketUpgrade, State(room): State<Arc<Room>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| client_session(socket, room))
}

async fn client_session(mut socket: WebSocket, room: Arc<Room>) {
    let me = room.next_client.fetch_add(1, Ordering::Relaxed);
    room.population.fetch_add(1, Ordering::Relaxed);
    let _ = room.cmds.send(Cmd::Join);
    let mut events = room.events.subscribe();

    let hello = {
        let elems = room.elements.lock().unwrap();
        let probes = room.probes.lock().unwrap();
        let panels = room.panels.lock().unwrap();
        json!({
            "t": "hello", "you": me, "elements": *elems,
            "probes": *probes, "panels": *panels,
        })
        .to_string()
    };
    if socket.send(Message::Text(hello.into())).await.is_err() {
        room.population.fetch_sub(1, Ordering::Relaxed);
        let _ = room.cmds.send(Cmd::Leave);
        return;
    }

    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(msg) => {
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                // Slow consumer skipped some frames; that is fine — the
                // next frame is a full snapshot.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                if let Message::Text(text) = msg {
                    match serde_json::from_str::<ClientMsg>(&text) {
                        Ok(ClientMsg::Interact { id, op }) => {
                            let _ = room.cmds.send(Cmd::Interact { id, op });
                        }
                        Ok(ClientMsg::Edit { op }) => {
                            let _ = room.cmds.send(Cmd::Edit { op });
                        }
                        Ok(ClientMsg::Probe { elem, pin, kind }) => {
                            let _ = room.cmds.send(Cmd::Probe { elem, pin, kind });
                        }
                        Ok(ClientMsg::ProbeRef { pid, elem, pin }) => {
                            let _ = room.cmds.send(Cmd::ProbeRef { pid, elem, pin });
                        }
                        Ok(ClientMsg::Panel { op }) => {
                            let _ = room.cmds.send(Cmd::Panel { op });
                        }
                        Ok(ClientMsg::MachineReset) => {
                            let _ = room.cmds.send(Cmd::MachineReset);
                        }
                        Ok(ClientMsg::MachineMove { dx, dy }) => {
                            let _ = room.cmds.send(Cmd::MachineMove { dx, dy });
                        }
                        Ok(ClientMsg::Repair { id }) => {
                            let _ = room.cmds.send(Cmd::Repair { id });
                        }
                        Ok(ClientMsg::Cursor { x, y }) => {
                            let _ = room.events.send(
                                json!({"t": "cursor", "who": me, "x": x, "y": y}).to_string(),
                            );
                        }
                        Err(_) => {} // ignore malformed input
                    }
                }
            }
        }
    }

    room.population.fetch_sub(1, Ordering::Relaxed);
    let _ = room.cmds.send(Cmd::Leave);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, _) = broadcast::channel(256);
    // Restore the room from the last checkpoint; fresh rooms start with
    // the showcase circuit.
    let saved = load_room();
    let (mut elements, probes, next_pid, panels, next_plid, hoist, hoist_rect, damage) = match saved
    {
        Some(s) => {
            tracing::info!(
                "restored room from {} ({} elements)",
                save_path(),
                s.elements.len()
            );
            let probes = s
                .probes
                .iter()
                .map(|p| Probe {
                    pid: p.pid,
                    elem: p.elem,
                    pin: p.pin,
                    kind: p.kind,
                    r: p.r,
                })
                .collect();
            // Never hand out a plid a restored panel already owns.
            let next_plid = s
                .next_plid
                .max(s.panels.iter().map(|p| p.plid + 1).max().unwrap_or(1))
                .max(1);
            (
                s.elements,
                probes,
                s.next_pid.max(1),
                s.panels,
                next_plid,
                s.hoist,
                sane_rect(s.hoist_rect),
                s.damage,
            )
        }
        None => (
            demo_room_circuit(),
            Vec::new(),
            1,
            Vec::new(),
            1,
            Hoist::default(),
            HOIST_RECT,
            DamageModel::new(),
        ),
    };
    // The hoist fixture is not optional: a room without it has no goal. This
    // also re-derives the children's pins from the restored footprint, so a
    // save whose fixture pins were edited by hand cannot leave a terminal
    // stranded outside the box.
    ensure_fixture(&mut elements, hoist_rect);

    let room = Arc::new(Room {
        cmds: cmd_tx,
        events: event_tx,
        elements: std::sync::Mutex::new(elements),
        probes: std::sync::Mutex::new(probes),
        panels: std::sync::Mutex::new(panels),
        next_client: AtomicU32::new(1),
        next_pid: AtomicU32::new(next_pid),
        next_plid: AtomicU32::new(next_plid),
        population: AtomicU32::new(0),
        dirty: std::sync::atomic::AtomicBool::new(false),
    });
    tokio::spawn(sim_task(room.clone(), cmd_rx, hoist, hoist_rect, damage));

    let dist = std::env::var("EE_DIST").unwrap_or_else(|_| "packages/app/dist".into());
    let static_files =
        ServeDir::new(&dist).not_found_service(ServeFile::new(format!("{dist}/index.html")));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        // Dev-friendly caching: the app shell must always revalidate so a
        // rebuilt bundle never leaves a stale page pointing at dead hashes.
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache"),
            ),
        )
        .fallback_service(static_files)
        .with_state(room);

    let addr = std::env::var("EE_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("ee-game server on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::ElementKind as K;
    use sim_golden::{dc, gnd, r, spec, spec3};

    /// Substeps in one room tick, exactly as `sim_task` budgets them.
    fn steps_per_tick() -> u32 {
        (((1.0 / TICK_HZ) / DT).round() as u32).min(MAX_STEPS_PER_TICK)
    }

    /// The damage half of the room loop: sweep the frame ONCE, integrate
    /// stress over the sim time that actually elapsed, and stamp whatever
    /// broke as open — the same order, and the same single sweep, as the
    /// tick in `sim_task`.
    fn damage_tick(eng: &mut Engine, dmg: &mut DamageModel, t0: f64, broke: &mut Vec<(f64, u32)>) {
        if eng.is_quarantined() {
            return; // no new numbers: a frozen circuit cooks nothing
        }
        let fr = eng.frame();
        for b in dmg.tick(&fr, eng.time() - t0) {
            eng.set_broken(b.id, true);
            broke.push((eng.time(), b.id));
        }
    }

    /// A plain circuit driven through the room loop's damage cadence.
    struct DamageRun {
        eng: Engine,
        dmg: DamageModel,
        /// (sim time, id) for every part that broke, in order.
        broke: Vec<(f64, u32)>,
    }

    impl DamageRun {
        fn new(elems: &[ElementSpec]) -> Self {
            let mut eng = Engine::new(DT);
            eng.set_elements(elems);
            let mut dmg = DamageModel::new();
            dmg.set_document(elems);
            DamageRun {
                eng,
                dmg,
                broke: Vec::new(),
            }
        }

        /// One room tick.
        fn tick(&mut self) {
            let t0 = self.eng.time();
            self.eng.advance(steps_per_tick());
            damage_tick(&mut self.eng, &mut self.dmg, t0, &mut self.broke);
        }

        /// Run for `secs` of sim time, stopping early once `id` breaks.
        /// Returns the sim time it broke at.
        fn run_until_break(&mut self, id: u32, secs: f64) -> Option<f64> {
            let ticks = (secs * TICK_HZ).round() as u32;
            for _ in 0..ticks {
                self.tick();
                if let Some((t, _)) = self.broke.iter().find(|(_, b)| *b == id) {
                    return Some(*t);
                }
            }
            None
        }

        fn run(&mut self, secs: f64) {
            let ticks = (secs * TICK_HZ).round() as u32;
            for _ in 0..ticks {
                self.tick();
            }
        }

        fn current(&self, id: u32) -> f64 {
            self.eng.pin_current(id, 0).unwrap_or(f64::NAN)
        }
    }

    /// The hoist fixture plus a player circuit, driven exactly the way
    /// `sim_task` drives it: MACHINE_SUBSTEPS of solver, then one machine
    /// tick that reads the motor's branch current and writes back — with the
    /// damage sweep landing on the room's 30 Hz tick, not the machine's.
    struct HoistRun {
        eng: Engine,
        hoist: Hoist,
        sources: Vec<u32>,
        /// The whole document, so a test can drag the assembly the way
        /// `sim_task` does.
        elems: Vec<ElementSpec>,
        rect: [i32; 4],
        dmg: DamageModel,
        /// Sim time owed to the next damage sweep.
        owed: f64,
        last_sweep: f64,
        /// (sim time, id) for every part that broke, in order.
        broke: Vec<(f64, u32)>,
    }

    impl HoistRun {
        fn new(player_circuit: Vec<ElementSpec>) -> Self {
            let mut elems = hoist_fixture();
            elems.extend(player_circuit);
            let sources = source_ids(&elems);
            let mut eng = Engine::new(DT);
            eng.set_elements(&elems);
            let mut dmg = DamageModel::new();
            dmg.set_document(&elems);
            HoistRun {
                eng,
                hoist: Hoist::default(),
                sources,
                elems,
                rect: HOIST_RECT,
                dmg,
                owed: 0.0,
                last_sweep: 0.0,
                broke: Vec::new(),
            }
        }

        /// Drag the assembly, exactly the way the sim task does it: one
        /// `move_machine`, then recompile the netlist. The player's own parts
        /// are NOT part of the assembly, so the test slides them along itself
        /// — otherwise the machine would simply walk out from under its wires.
        fn drag_machine(&mut self, dx: i32, dy: i32) -> Vec<(u32, Vec<sim_core::Point>)> {
            for e in self.elems.iter_mut() {
                if reserved_id(e.id) {
                    continue;
                }
                for p in e.pins.iter_mut() {
                    *p = (p.0 + dx, p.1 + dy);
                }
            }
            let moved = move_machine(&mut self.elems, &mut self.rect, dx, dy)
                .expect("the move must be accepted");
            self.eng.set_elements(&self.elems);
            moved
        }

        /// Returns the armature current the machine just used (A).
        fn step(&mut self) -> f64 {
            self.eng.advance(MACHINE_SUBSTEPS);
            assert!(
                !self.eng.is_quarantined(),
                "solver quarantined at t={:.4} s (y={:.4} m)",
                self.eng.time(),
                self.hoist.y
            );
            let i = self.eng.pin_current(MOTOR_ID, 0).unwrap();
            machine_step(&mut self.eng, &mut self.hoist, &self.sources);
            self.owed += MACHINE_H;
            if self.owed >= 1.0 / TICK_HZ {
                self.owed = 0.0;
                let t0 = self.last_sweep;
                self.last_sweep = self.eng.time();
                damage_tick(&mut self.eng, &mut self.dmg, t0, &mut self.broke);
            }
            i
        }

        fn motor_broken(&self) -> bool {
            self.dmg.is_broken(MOTOR_ID)
        }

        fn motor_stress(&self) -> f64 {
            self.dmg.stress(MOTOR_ID)
        }
    }

    /// The motor's two terminals, from the fixture geometry.
    fn motor_pins() -> (sim_core::Point, sim_core::Point) {
        let m = hoist_fixture()
            .into_iter()
            .find(|e| e.id == MOTOR_ID)
            .unwrap();
        (m.pins[0], m.pins[1])
    }

    #[test]
    fn constant_voltage_lifts_the_crate_but_cannot_hold_it() {
        // A 12 V battery straight across the motor leads — the naive wiring.
        let (mp, mm) = motor_pins();
        let (sp, sm) = ((mp.0 - 9, mp.1), (mm.0 - 9, mm.1));
        let mut run = HoistRun::new(vec![
            spec(1, dc(12.0), sp, sm),
            spec(2, K::Wire, sp, mp),
            spec(3, K::Wire, sm, mm),
            gnd(4, sm),
        ]);

        let mut i_free = 0.0; // armature current while still travelling
        let mut t_top = f64::NAN;
        let mut top_switch = false;
        // 1.0 s of machine time.
        for _ in 0..(1.0 / MACHINE_H) as u32 {
            let i = run.step();
            if run.hoist.y < machine::SHAFT_H {
                i_free = i;
            } else if t_top.is_nan() {
                t_top = run.eng.time();
            }
            top_switch |= run.hoist.y >= machine::LIM_TOP_Y;
        }

        // Solver current against the torque balance: i = (V - K·ω)/R with
        // ω = 40.2086 rad/s -> 0.9740 A.
        let omega_ss = (machine::K * 12.0 / machine::R_ARM - machine::LOAD_TORQUE)
            / (machine::K * machine::K / machine::R_ARM + machine::VISCOUS_B);
        let i_expect = (12.0 - machine::K * omega_ss) / machine::R_ARM;
        assert!(
            (i_free - i_expect).abs() < 0.005,
            "armature current {i_free} A, closed form {i_expect} A"
        );
        assert!(
            (run.hoist.velocity() - 0.0).abs() < 1e-9 || run.hoist.y == machine::SHAFT_H,
            "must be parked at the head stop"
        );
        // Travel time: 0.40/0.8042 + τ_mech = 0.522 s (plus up to one machine
        // tick of back-EMF lag through the solver).
        let expect_top = machine::SHAFT_H / (machine::DRUM_R * omega_ss) + machine::TAU_MECH;
        assert!(
            (t_top - expect_top).abs() < 0.01,
            "head stop at {t_top} s, closed form {expect_top} s"
        );
        assert!(top_switch, "LIM-TOP must have been reached");

        // The design pillar, measured: voltage buys speed, not position. The
        // crate crosses the 40 mm band at 0.80 m/s (50 ms of credit) and then
        // parks 60 mm above it, so the hold drains back to nothing.
        assert!(!run.hoist.win, "constant voltage must never win");
        assert_eq!(run.hoist.hold, 0.0, "hold must drain at the head stop");
        assert_eq!(run.hoist.landings, 0, "it never came down");
        assert!(
            run.hoist.joules > 0.0,
            "the battery delivered energy: {} J",
            run.hoist.joules
        );
        eprintln!(
            "12 V open loop: i={i_free:.4} A, top at {t_top:.4} s, {:.2} J",
            run.hoist.joules
        );
    }

    /// The winning wiring, as a player would draw it — shared by the goal test
    /// and the assembly-move test (which needs a live control loop to prove a
    /// move does not disturb one).
    ///
    ///   sensor pot (SENSE-A to +4 V, SENSE-B to ground) -> wiper voltage
    ///   4·y/H, compared against a 3.2 V reference (= band centre 0.32 m).
    ///   The op-amp comparator drives the motor: +5 V lifts, -5 V lowers.
    /// Bang-bang, and the 3× hold drain is sized to let it win.
    fn comparator_feedback_circuit() -> Vec<ElementSpec> {
        let (mp, mm) = motor_pins();
        let sensor = hoist_fixture()
            .into_iter()
            .find(|e| e.id == SENSOR_ID)
            .unwrap();
        let (sa, sw, sb) = (sensor.pins[0], sensor.pins[1], sensor.pins[2]);
        let (sup_p, sup_n) = ((sa.0 - 17, sa.1), (sb.0 - 17, sb.1));
        let (ref_p, ref_n) = ((sa.0 - 17, sa.1 + 8), (sa.0 - 17, sa.1 + 12));
        let (in_p, in_m, out) = (
            (sa.0 - 9, sa.1 + 8),
            (sa.0 - 9, sa.1 + 2),
            (sa.0 - 5, sa.1 + 5),
        );
        vec![
            // Sensor excitation: 4 V across SENSE-A .. SENSE-B.
            spec(1, dc(4.0), sup_p, sup_n),
            gnd(2, sup_n),
            spec(3, K::Wire, sup_p, sa),
            spec(4, K::Wire, sup_n, sb),
            // Setpoint: 3.2 V = 4 V · (0.32 / 0.40).
            spec(5, dc(3.2), ref_p, ref_n),
            gnd(6, ref_n),
            spec(7, K::Wire, ref_p, in_p),
            // Comparator: in+ = setpoint, in- = wiper, out -> M+.
            spec3(8, K::OpAmp { rail: 5.0 }, in_p, in_m, out),
            spec(9, K::Wire, in_m, sw),
            spec(10, K::Wire, out, mp),
            spec(11, K::Wire, mm, (mm.0 - 5, mm.1)),
            gnd(12, (mm.0 - 5, mm.1)),
        ]
    }

    #[test]
    fn comparator_feedback_holds_the_crate_in_the_band() {
        // The discovery this goal exists to force: close the loop.
        let mut run = HoistRun::new(comparator_feedback_circuit());

        let mut peak = 0.0f64;
        let mut entered = f64::NAN;
        // 9 s of machine time: ~1.3 s to climb into the band, 5 s to hold,
        // and slack for the bang-bang chatter.
        for _ in 0..(9.0 / MACHINE_H) as u32 {
            run.step();
            peak = peak.max(run.hoist.y);
            if entered.is_nan() && run.hoist.in_band() {
                entered = run.eng.time();
            }
            if run.hoist.win {
                break;
            }
        }

        assert!(
            run.hoist.win,
            "comparator feedback must win: hold={:.4} s y={:.4} m",
            run.hoist.hold, run.hoist.y
        );
        assert!(
            run.hoist.in_band(),
            "and still be in the band: y={}",
            run.hoist.y
        );
        assert!(
            peak < machine::LIM_TOP_Y,
            "feedback must not slam the head stop: peak {peak} m"
        );
        assert_eq!(run.hoist.landings, 0, "and must not drop the crate");
        // The hold ran continuously from the moment it entered the band: the
        // win lands HOLD_NEED after entry, to within the one machine tick
        // that `entered` is sampled late (the timestamp is read after the
        // tick that crossed 0.300 m, which is also the tick that first
        // credited hold).
        let elapsed = run.eng.time() - entered;
        assert!(
            (machine::HOLD_NEED - MACHINE_H..machine::HOLD_NEED + 1.0).contains(&elapsed),
            "won after {elapsed:.4} s in the band (need {})",
            machine::HOLD_NEED
        );
        eprintln!(
            "comparator: entered band at {entered:.3} s, won at {:.3} s (band time {elapsed:.3} s), \
             y={:.4} peak={peak:.4}, {:.1} J",
            run.eng.time(),
            run.hoist.y,
            run.hoist.joules
        );
    }

    #[test]
    fn shorted_leads_lower_the_crate_under_regenerative_braking() {
        // No source at all: the motor is its own brake through a 2 Ω ballast,
        // and the crate descends at the closed-form 0.297 m/s.
        let (mp, mm) = motor_pins();
        let mut run = HoistRun::new(vec![
            spec(1, r(2.0), (mp.0 - 6, mp.1), (mp.0 - 6, mm.1)),
            spec(2, K::Wire, (mp.0 - 6, mp.1), mp),
            spec(3, K::Wire, (mp.0 - 6, mm.1), mm),
            gnd(4, (mp.0 - 6, mm.1)),
        ]);
        run.hoist.y = machine::SHAFT_H;
        for _ in 0..(0.30 / MACHINE_H) as u32 {
            run.step();
        }
        // 4 Ω total loop: ω(K²/R + b) = -m·g·r -> -0.2975 m/s.
        let expect = machine::DRUM_R * -machine::LOAD_TORQUE
            / (machine::K * machine::K / 4.0 + machine::VISCOUS_B);
        assert!(
            (run.hoist.velocity() - expect).abs() < 0.005,
            "descent {} m/s, closed form {expect} m/s",
            run.hoist.velocity()
        );
        assert_eq!(run.hoist.joules, 0.0, "no source, no energy spent");
        eprintln!("2 Ω ballast: {:.4} m/s", run.hoist.velocity());
    }

    #[test]
    fn fixture_edits_are_refused_but_wiring_to_it_is_not() {
        let (cmd_tx, _rx) = mpsc::unbounded_channel();
        let (event_tx, _ev) = broadcast::channel(4);
        let mut elements = demo_room_circuit();
        ensure_fixture(&mut elements, HOIST_RECT);
        let room = Room {
            cmds: cmd_tx,
            events: event_tx,
            elements: std::sync::Mutex::new(elements),
            probes: std::sync::Mutex::new(Vec::new()),
            panels: std::sync::Mutex::new(Vec::new()),
            next_client: AtomicU32::new(1),
            next_pid: AtomicU32::new(1),
            next_plid: AtomicU32::new(1),
            population: AtomicU32::new(0),
            dirty: std::sync::atomic::AtomicBool::new(false),
        };

        let (mp, _) = motor_pins();
        for op in [
            DocOp::Remove { id: MOTOR_ID },
            DocOp::Move {
                id: MOTOR_ID,
                pins: vec![(0, 0), (0, 4)],
            },
            DocOp::SetKind {
                id: SENSOR_ID,
                kind: K::Potentiometer {
                    ohms: 1.0,
                    wiper: 0.5,
                },
            },
            DocOp::SetKind {
                id: LIM_TOP_ID,
                kind: K::Switch { closed: true },
            },
            DocOp::Add {
                spec: spec(950, K::Wire, (0, 0), (0, 4)),
            },
        ] {
            assert!(!apply_doc_op(&room, &op), "must refuse {op:?}");
        }
        // The fixture is intact.
        {
            let elems = room.elements.lock().unwrap();
            for id in [MOTOR_ID, SENSOR_ID, LIM_TOP_ID, LIM_BOT_ID] {
                assert!(elems.iter().any(|e| e.id == id), "{id} vanished");
            }
        }
        // Wiring TO a fixture terminal is a normal player op.
        assert!(apply_doc_op(
            &room,
            &DocOp::Add {
                spec: spec(500, K::Wire, mp, (mp.0 - 4, mp.1)),
            }
        ));
    }

    /// The wire format is a fixed contract with the client: both halves were
    /// written against it independently, so it gets a test of its own.
    #[test]
    fn machine_protocol_matches_the_contract() {
        let mut hoist = Hoist::default();
        hoist.y = 0.321;
        hoist.omega = 1.5;
        hoist.hold = 2.5;
        hoist.landings = 3;
        hoist.joules = 12.5;
        let v = machine_msg(&hoist, HOIST_RECT, 0.94, 1.75, motor_i_max());
        assert_eq!(v["t"], "machine");
        // The nameplate current the faceplate engraves comes from the damage
        // table, and it has to bracket the motor's two operating points: the
        // ~0.94 A it runs at must be safe, the 6 A it stalls at on a bare
        // 12 V lead must not be.
        assert_eq!(v["imax"], 3.0);
        let i_stall = 12.0 / machine::R_ARM;
        assert!(
            machine::HOLD_CURRENT < 3.0 && 3.0 < i_stall,
            "rating {} must sit between hold {} A and stall {i_stall} A",
            3.0,
            machine::HOLD_CURRENT
        );
        assert_eq!(v["id"], MOTOR_ID);
        assert_eq!(v["rect"], json!([46, 2, 64, 24]));
        assert_eq!(v["h"], 0.40);
        assert_eq!(v["band"], json!([0.30, 0.34]));
        assert_eq!(v["y"], 0.321);
        assert_eq!(v["vel"], 1.5 * machine::DRUM_R);
        assert_eq!(v["i"], 0.94);
        assert_eq!(v["hold"], 2.5);
        assert_eq!(v["need"], 5.0);
        assert_eq!(v["impact"], 1.75);
        assert_eq!(v["landings"], 3);
        assert_eq!(v["win"], false);
        assert_eq!(v["joules"], 12.5);
        // rect must actually contain every fixture pin, or the client draws
        // terminals outside the box it was told about.
        let [x0, y0, x1, y1] = HOIST_RECT;
        for e in hoist_fixture() {
            for (px, py) in e.pins {
                assert!(
                    (x0..=x1).contains(&px) && (y0..=y1).contains(&py),
                    "element {} pin ({px},{py}) is outside rect {HOIST_RECT:?}",
                    e.id
                );
            }
        }
        // And the fixture must not collide with the showcase district.
        for e in demo_room_circuit() {
            for (px, py) in e.pins {
                assert!(
                    !((x0..=x1).contains(&px) && (y0..=y1).contains(&py)),
                    "showcase element {} sits inside the hoist rect",
                    e.id
                );
            }
        }
    }

    #[test]
    fn machine_reset_is_accepted_on_the_wire() {
        let msg: ClientMsg = serde_json::from_str(r#"{"t":"machinereset"}"#).unwrap();
        assert!(matches!(msg, ClientMsg::MachineReset));
    }

    #[test]
    fn machine_move_is_accepted_on_the_wire() {
        let msg: ClientMsg = serde_json::from_str(r#"{"t":"machinemove","dx":3,"dy":-2}"#).unwrap();
        let ClientMsg::MachineMove { dx, dy } = msg else {
            panic!("expected a machinemove message")
        };
        assert_eq!((dx, dy), (3, -2));
        // Grid units are integers: a fractional drag is not a move at all.
        assert!(
            serde_json::from_str::<ClientMsg>(r#"{"t":"machinemove","dx":0.5,"dy":0}"#).is_err()
        );
        assert!(
            serde_json::from_str::<ClientMsg>(r#"{"t":"machinemove","dx":"3","dy":0}"#).is_err()
        );
    }

    /// The invariant the whole feature rests on: the footprint the client draws
    /// from and the terminals the player wires to move as ONE thing.
    #[test]
    fn machine_move_keeps_the_rect_and_the_children_in_lockstep() {
        let mut elems = demo_room_circuit();
        let mut rect = HOIST_RECT;
        ensure_fixture(&mut elems, rect);

        for (dx, dy) in [(7, 5), (-3, 0), (0, -11), (120, 340)] {
            let before = rect;
            let moved = move_machine(&mut elems, &mut rect, dx, dy).expect("accepted");
            // The rect translated by exactly the delta, and kept its size.
            assert_eq!(
                rect,
                [
                    before[0] + dx,
                    before[1] + dy,
                    before[2] + dx,
                    before[3] + dy
                ]
            );
            assert_eq!((rect[2] - rect[0], rect[3] - rect[1]), (HOIST_W, HOIST_H));
            // Every child sits exactly where a fixture derived from the new
            // rect would sit — that is the lockstep, not an approximation of it.
            let want = hoist_fixture_at(rect);
            assert_eq!(moved.len(), want.len());
            for w in &want {
                let live = elems.iter().find(|e| e.id == w.id).expect("child present");
                assert_eq!(live.pins, w.pins, "child {} drifted from the rect", w.id);
                assert!(moved
                    .iter()
                    .any(|(id, pins)| *id == w.id && *pins == w.pins));
                // And inside the box, so the client can draw its screw pads.
                for (px, py) in &live.pins {
                    assert!(
                        (rect[0]..=rect[2]).contains(px) && (rect[1]..=rect[3]).contains(py),
                        "child {} pin ({px},{py}) outside rect {rect:?}",
                        w.id
                    );
                }
            }
        }
        // The player's own parts are never touched by an assembly move.
        let showcase = demo_room_circuit();
        for e in &showcase {
            let live = elems.iter().find(|x| x.id == e.id).unwrap();
            assert_eq!(live.pins, e.pins, "element {} was dragged along", e.id);
        }
    }

    #[test]
    fn machine_move_refuses_nonsense() {
        let mut elems = demo_room_circuit();
        let mut rect = HOIST_RECT;
        ensure_fixture(&mut elems, rect);
        let pins_before: Vec<Vec<sim_core::Point>> = hoist_fixture_at(rect)
            .iter()
            .map(|e| e.pins.clone())
            .collect();

        for (dx, dy) in [
            (0, 0),                     // not a move at all
            (MAX_MACHINE_STEP + 1, 0),  // past the per-op step cap
            (0, -MAX_MACHINE_STEP - 1), // same, negative
            (WORLD_LIMIT, 0),           // a jump the size of the world
            (0, WORLD_LIMIT),           //
            (-WORLD_LIMIT * 2, 0),      // twice the world, westward
            (i32::MIN, i32::MAX),       // overflow bait: `abs` would panic here
        ] {
            assert!(
                move_machine(&mut elems, &mut rect, dx, dy).is_none(),
                "must refuse ({dx},{dy})"
            );
            // A refused move changes nothing at all.
            assert_eq!(rect, HOIST_RECT);
            for (e, want) in hoist_fixture_at(rect).iter().zip(&pins_before) {
                let live = elems.iter().find(|x| x.id == e.id).unwrap();
                assert_eq!(&live.pins, want);
            }
        }
        // Walking east forever hits a wall rather than falling off the end of
        // the world: the last accepted move leaves the whole box in range, and
        // the next one is simply refused.
        let mut steps = 0;
        while move_machine(&mut elems, &mut rect, MAX_MACHINE_STEP, 0).is_some() {
            steps += 1;
            assert!(steps < 1000, "the range must be a wall, not a treadmill");
        }
        assert!(rect[0] > HOIST_RECT[0], "it did travel east");
        assert!(
            rect[2] <= WORLD_LIMIT && rect[0] >= -WORLD_LIMIT,
            "the box stayed inside the world: {rect:?}"
        );
        // Still in lockstep out there.
        for w in hoist_fixture_at(rect) {
            let live = elems.iter().find(|e| e.id == w.id).unwrap();
            assert_eq!(live.pins, w.pins);
        }
    }

    /// Dragging the cabinet is a TRANSLATION: the crate does not teleport to
    /// the floor, the hold timer does not restart, the landing count does not
    /// clear. The whole point of putting the footprint in state rather than
    /// rebuilding the machine.
    #[test]
    fn machine_move_does_not_disturb_the_mechanism() {
        let mut run = HoistRun::new(comparator_feedback_circuit());
        // Long enough to be genuinely airborne, in the band, banking hold.
        for _ in 0..(2.5 / MACHINE_H) as u32 {
            run.step();
        }
        assert!(
            run.hoist.in_band() && run.hoist.hold > 0.5,
            "setup: expected a live hold, got y={} hold={}",
            run.hoist.y,
            run.hoist.hold
        );
        let before = run.hoist; // Hoist is Copy: a full snapshot
        let t_before = run.eng.time();
        let hold_before = run.hoist.hold;

        let moved = run.drag_machine(5, 3);
        assert_eq!(moved.len(), 4, "all four children moved");
        assert_eq!(run.rect, hoist_rect(51, 5));

        // Not one mechanical or goal quantity changed: height, velocity, the
        // hold timer, the landing count, the energy meter.
        assert_eq!(run.hoist, before, "a move must not disturb the mechanism");
        assert_eq!(run.hoist.y, before.y);
        assert_eq!(run.hoist.velocity(), before.velocity());
        assert_eq!(run.hoist.hold, hold_before);
        assert_eq!(run.hoist.landings, before.landings);
        assert_eq!(run.eng.time(), t_before, "sim time does not rewind");
        assert!(
            !run.eng.is_quarantined(),
            "the move must not break the solve"
        );

        // And the loop the player built still works at the new address: the
        // hold carries on from where it was and reaches the win.
        for _ in 0..(4.0 / MACHINE_H) as u32 {
            run.step();
            if run.hoist.win {
                break;
            }
        }
        assert!(
            run.hoist.win,
            "the control loop must survive the move: hold={:.3} y={:.4}",
            run.hoist.hold, run.hoist.y
        );
        assert_eq!(run.hoist.landings, 0, "and must not have dropped the crate");
        eprintln!(
            "moved mid-hold at {t_before:.3} s (hold {hold_before:.3} s), won at {:.3} s",
            run.eng.time()
        );
    }

    /// A client can only move the machine through the assembly op. Direct
    /// document ops on a fixture stay refused, which is what stops a player
    /// dragging the motor out of its own cabinet.
    #[test]
    fn a_direct_move_of_a_fixture_is_still_refused() {
        let mut elements = demo_room_circuit();
        ensure_fixture(&mut elements, HOIST_RECT);
        let room = test_room(elements);
        let before: Vec<Vec<sim_core::Point>> =
            hoist_fixture().iter().map(|e| e.pins.clone()).collect();

        for id in [MOTOR_ID, SENSOR_ID, LIM_TOP_ID, LIM_BOT_ID] {
            let pins = room
                .elements
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .pins
                .clone();
            let shifted: Vec<sim_core::Point> = pins.iter().map(|(x, y)| (x + 1, *y)).collect();
            assert!(
                !apply_doc_op(&room, &DocOp::Move { id, pins: shifted }),
                "a direct Move on {id} must be refused"
            );
        }
        let elems = room.elements.lock().unwrap();
        for (e, want) in hoist_fixture().iter().zip(&before) {
            assert_eq!(&elems.iter().find(|x| x.id == e.id).unwrap().pins, want);
        }
    }

    #[test]
    fn old_saves_load_without_hoist_state() {
        let save: SaveFile = serde_json::from_str(r#"{"elements":[]}"#).unwrap();
        assert_eq!(save.hoist, Hoist::default());
        assert_eq!(save.hoist.y, 0.0);
        assert!(!save.hoist.win);
        // A save written before the machine could be dragged lands on the
        // original footprint — which is where its fixture pins already are.
        assert_eq!(save.hoist_rect, HOIST_RECT);
        assert_eq!(sane_rect(save.hoist_rect), HOIST_RECT);
    }

    #[test]
    fn a_saved_footprint_survives_a_restart_and_is_sanitized() {
        // Round trip: the rect a drag left behind comes back byte-identical.
        let mut elements = vec![spec(1, K::Wire, (0, 0), (0, 4))];
        let mut rect = HOIST_RECT;
        ensure_fixture(&mut elements, rect);
        move_machine(&mut elements, &mut rect, -20, 40).unwrap();
        let json = serde_json::to_string(&SaveFile {
            elements: elements.clone(),
            probes: Vec::new(),
            next_pid: 1,
            panels: Vec::new(),
            next_plid: 1,
            hoist: Hoist::default(),
            hoist_rect: rect,
            damage: DamageModel::new(),
        })
        .unwrap();
        let back: SaveFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hoist_rect, rect);
        // Reloading re-derives the children from the restored rect, so the
        // save and the fixture cannot come back disagreeing.
        let mut restored = back.elements;
        ensure_fixture(&mut restored, sane_rect(back.hoist_rect));
        for w in hoist_fixture_at(rect) {
            assert_eq!(restored.iter().find(|e| e.id == w.id).unwrap().pins, w.pins);
        }
        // A hand-edited or corrupt rect is forced back onto the invariants:
        // normalized corners, fixed size, inside the world.
        assert_eq!(sane_rect([64, 24, 46, 2]), HOIST_RECT);
        let far = sane_rect([i32::MAX, i32::MIN, 0, 0]);
        assert_eq!((far[2] - far[0], far[3] - far[1]), (HOIST_W, HOIST_H));
        assert!(far[2] <= WORLD_LIMIT && far[1] >= -WORLD_LIMIT, "{far:?}");
    }

    #[test]
    fn ensure_fixture_repairs_old_rooms() {
        // A pre-hoist save: no fixture at all.
        let mut elems = vec![spec(1, K::Wire, (0, 0), (0, 4))];
        ensure_fixture(&mut elems, HOIST_RECT);
        assert_eq!(elems.len(), 5);
        // Idempotent, and persisted state survives a reload.
        if let Some(e) = elems.iter_mut().find(|e| e.id == SENSOR_ID) {
            e.kind = K::Potentiometer {
                ohms: SENSOR_OHMS,
                wiper: 0.25,
            };
        }
        ensure_fixture(&mut elems, HOIST_RECT);
        assert_eq!(elems.len(), 5);
        let sensor = elems.iter().find(|e| e.id == SENSOR_ID).unwrap();
        assert!(matches!(
            sensor.kind,
            K::Potentiometer { wiper, .. } if wiper == 0.25
        ));
        // A save from before the ids were reserved, with a player's part
        // squatting on a fixture id: the fixture takes it back.
        let mut squatted = vec![spec(MOTOR_ID, K::Resistor { ohms: 100.0 }, (0, 0), (0, 4))];
        ensure_fixture(&mut squatted, HOIST_RECT);
        let motor = squatted.iter().find(|e| e.id == MOTOR_ID).unwrap();
        assert!(matches!(motor.kind, K::Motor { .. }), "fixture must win");
        assert_eq!(motor.pins, motor_pins_vec());
    }

    fn motor_pins_vec() -> Vec<sim_core::Point> {
        let (a, b) = motor_pins();
        vec![a, b]
    }

    /// A room with no sim task behind it, for exercising op validation.
    fn test_room(elements: Vec<ElementSpec>) -> Room {
        let (cmds, _rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(8);
        Room {
            cmds,
            events,
            elements: std::sync::Mutex::new(elements),
            probes: std::sync::Mutex::new(Vec::new()),
            panels: std::sync::Mutex::new(Vec::new()),
            next_client: AtomicU32::new(1),
            next_pid: AtomicU32::new(1),
            next_plid: AtomicU32::new(1),
            population: AtomicU32::new(0),
            dirty: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The world is big now (50k parts), but the budget is still a hard wall
    /// and malformed specs are still refused.
    #[test]
    fn document_cap_is_big_and_still_validates() {
        use sim_core::ElementKind as K;
        let full: Vec<ElementSpec> = (0..MAX_ELEMENTS as u32)
            .map(|k| ElementSpec {
                id: k + 1,
                kind: K::Wire,
                pins: vec![(0, 0), (1, 0)],
            })
            .collect();
        let room = test_room(full);
        let wire = |id: u32| DocOp::Add {
            spec: ElementSpec {
                id,
                kind: K::Wire,
                pins: vec![(2, 2), (3, 2)],
            },
        };
        // At the cap the op is dropped, not applied.
        assert!(!apply_doc_op(&room, &wire(900_001)));
        assert_eq!(room.elements.lock().unwrap().len(), MAX_ELEMENTS);
        // One slot free: the same op lands, and only once.
        room.elements.lock().unwrap().pop();
        assert!(apply_doc_op(&room, &wire(900_001)));
        assert!(!apply_doc_op(&room, &wire(900_001)), "duplicate id");
        // Pin-count validation survives the raised cap (checked before it).
        assert!(!apply_doc_op(
            &room,
            &DocOp::Add {
                spec: ElementSpec {
                    id: 900_002,
                    kind: K::Wire,
                    pins: vec![(0, 0)],
                },
            }
        ));
        assert!(!apply_doc_op(
            &room,
            &DocOp::Move {
                id: 900_001,
                pins: vec![(0, 0)],
            }
        ));
        assert!(apply_doc_op(
            &room,
            &DocOp::Move {
                id: 900_001,
                pins: vec![(4, 4), (5, 4)],
            }
        ));
        assert!(!apply_doc_op(&room, &DocOp::Remove { id: 12_345_678 }));
    }

    #[test]
    fn showcase_room_never_quarantines() {
        let mut eng = Engine::new(DT);
        eng.set_elements(&demo_room_circuit());
        // 30 simulated seconds in 10 ms chunks; the relaxation oscillator
        // must flip repeatedly and nothing may quarantine.
        let mut flips = 0;
        let mut last_sign = 0i32;
        // Vignette I's 555 astable must blink at ~1 Hz the whole time.
        let mut t555_flips = 0;
        let mut t555_high = false;
        for chunk in 0..3000 {
            eng.advance(500);
            if eng.is_quarantined() {
                let out = eng.voltage_at((12, 25)).unwrap_or(f64::NAN);
                let vm = eng.voltage_at((4, 24)).unwrap_or(f64::NAN);
                panic!(
                    "quarantined at t={:.4}s (chunk {chunk}): osc out={out:.4} vm={vm:.4}",
                    eng.time()
                );
            }
            let out = eng.voltage_at((12, 25)).unwrap_or(0.0);
            let s = if out > 1.0 {
                1
            } else if out < -1.0 {
                -1
            } else {
                0
            };
            if s != 0 && last_sign != 0 && s != last_sign {
                flips += 1;
            }
            if s != 0 {
                last_sign = s;
            }
            let t555 = eng.voltage_at((46, 42)).unwrap_or(0.0) > 4.5;
            if t555 != t555_high {
                t555_flips += 1;
                t555_high = t555;
            }
        }
        assert!(flips >= 10, "oscillator only flipped {flips} times in 30 s");
        assert!(
            t555_flips >= 20,
            "555 astable only flipped {t555_flips} times in 30 s (expect ~60)"
        );
    }

    /// A 440 Hz AC source driving a speaker: the tap stream must carry the
    /// coil's real waveform at the audio cadence, and the two cadences must
    /// stay locked to sim time (no drift in the `t0 + k*dts` labels).
    #[test]
    fn speaker_tap_streams_the_coil_waveform() {
        use sim_core::ElementKind as K;
        use sim_golden::{gnd, r, spec};
        let elems = vec![
            spec(
                1,
                K::VoltageSource {
                    dc: 0.0,
                    amp: 5.0,
                    hz: 440.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 8),
            ),
            spec(2, r(8.0), (0, 0), (4, 0)),
            spec(3, K::Speaker { ohms: 8.0 }, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        let taps: Vec<(u32, sim_core::ElemTap)> = audio_tap_ids(&elems)
            .into_iter()
            .filter_map(|id| eng.tap(id).map(|t| (id, t)))
            .collect();
        assert_eq!(taps.len(), 1, "one speaker, one tap");
        assert_eq!(taps[0].0, 3);

        // One tick's worth of audio chunks, exactly as sim_task does it.
        let steps_per_tick = ((1.0 / TICK_HZ) / DT).round() as u32;
        let budget = steps_per_tick.min(MAX_STEPS_PER_TICK);
        let chunks = (budget / AUDIO_EVERY).max(1);
        let t0 = eng.time();
        let mut buf: Vec<f64> = Vec::with_capacity(chunks as usize);
        for _ in 0..chunks {
            eng.advance(AUDIO_EVERY);
            buf.push(wire_sample(eng.tap_delta(taps[0].1, 0, 1)));
        }
        let dts = DT * AUDIO_EVERY as f64;

        // The labels must be the truth: the last sample's stated time is the
        // engine's own time, to within one sample.
        let stated_end = t0 + buf.len() as f64 * dts;
        assert!(
            (stated_end - eng.time()).abs() < dts * 0.5,
            "sample clock drifted: stated {stated_end}, engine {}",
            eng.time()
        );

        // 12.5 kHz on a 440 Hz tone: ~28 samples per cycle, and the coil sees
        // half the 5 V source across the 8+8 Ω divider.
        assert!((dts * 440.0).recip() > 20.0, "too few samples per cycle");
        let peak = buf.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(
            (peak - 2.5).abs() < 0.05,
            "coil peak should be ~2.5 V, got {peak}"
        );
        // Zero crossings confirm the frequency survived the decimation:
        // 13.3 ms of 440 Hz is ~11.7 cycles, so ~23 crossings.
        let crossings = buf
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let cycles = buf.len() as f64 * dts * 440.0;
        assert!(
            (crossings as f64 - 2.0 * cycles).abs() < 3.0,
            "expected ~{:.0} zero crossings for {cycles:.1} cycles, got {crossings}",
            2.0 * cycles
        );
        assert!(buf.iter().all(|v| v.is_finite()), "no NaN on the wire");

        // Honest bandwidth number, asserted so it cannot silently balloon.
        // Measured: ~94 kB/s per speaker (0.75 Mbit/s), 4 taps => ~375 kB/s.
        // That is the price of the M4-lite JSON transport; the binary
        // transport (M4/M5) carries the same 12.5 kHz f32 stream in 50 kB/s.
        let msg = audio_message(t0, dts, 0.6234, &taps, vec![buf.clone()]);
        let per_sec = msg.len() as f64 * TICK_HZ;
        assert!(
            per_sec < 110_000.0,
            "one speaker costs {:.0} kB/s of JSON",
            per_sec / 1000.0
        );
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["t"], "audio");
        assert_eq!(v["dts"].as_f64().unwrap(), dts);
        assert_eq!(v["s"]["3"].as_array().unwrap().len(), buf.len());
        // The client needs the dilation figure on this message to tell "the
        // sim is too slow to make audio" from "my socket hiccuped".
        assert_eq!(v["rt"].as_f64().unwrap(), 0.623);
    }

    /// The realtime ratio must report what the wall clock actually saw: a tick
    /// that advanced less sim time than wall time IS the audio producer
    /// falling behind the sound card, and the client shows it as "sim 0.6x".
    #[test]
    fn realtime_ratio_tracks_sim_dilation() {
        // Perfect realtime stays at 1.
        let mut rt = 1.0;
        for _ in 0..50 {
            rt = blend_realtime(rt, 1.0 / TICK_HZ, 1.0 / TICK_HZ);
        }
        assert!((rt - 1.0).abs() < 1e-9, "realtime drifted to {rt}");

        // A tick that takes 55 ms of wall to advance 33.3 ms of sim is 0.6x.
        // The EMA has to converge there, not overshoot or stick.
        for _ in 0..80 {
            rt = blend_realtime(rt, 1.0 / TICK_HZ, 0.0555_556);
        }
        assert!(
            (rt - 0.6).abs() < 0.01,
            "expected ~0.6x under 1.67x load, got {rt}"
        );
        // ...and recovers when the load goes away.
        for _ in 0..80 {
            rt = blend_realtime(rt, 1.0 / TICK_HZ, 1.0 / TICK_HZ);
        }
        assert!((rt - 1.0).abs() < 0.01, "did not recover: {rt}");

        // A quarantined solver advances no sim time: honest 0, not a stale 1.
        for _ in 0..120 {
            rt = blend_realtime(rt, 0.0, 1.0 / TICK_HZ);
        }
        assert!(rt < 0.01, "a stalled sim should read ~0, got {rt}");

        // Garbage in (zero wall gap, negative sim step, NaN) never poisons it.
        assert_eq!(blend_realtime(0.5, 0.033, 0.0), 0.5);
        assert_eq!(blend_realtime(0.5, -0.1, 0.033), 0.5);
        assert_eq!(blend_realtime(0.5, f64::NAN, 0.033), 0.5);
        assert!(blend_realtime(1.0, 10.0, 1e-9).is_finite());
    }

    /// The tap set is the first N speakers by element id — deterministic, and
    /// capped whatever order the document happens to be in.
    #[test]
    fn audio_taps_are_the_first_n_speakers_by_id() {
        use sim_core::ElementKind as K;
        use sim_golden::{r, spec};
        let mut elems = vec![spec(500, r(100.0), (0, 0), (2, 0))];
        // Deliberately out of order, and more speakers than the cap.
        for id in [90, 12, 77, 3, 41, 55] {
            elems.push(spec(id, K::Speaker { ohms: 8.0 }, (0, 0), (2, 0)));
        }
        assert_eq!(audio_tap_ids(&elems), vec![3, 12, 41, 55]);
        assert_eq!(audio_tap_ids(&elems).len(), MAX_AUDIO_TAPS);
        // No speakers, no stream.
        assert!(audio_tap_ids(&[spec(1, r(1.0), (0, 0), (1, 0))]).is_empty());
        // The cadences must stay commensurate or the sample labels drift.
        assert_eq!(SAMPLE_EVERY % AUDIO_EVERY, 0);
    }

    /// The wire never carries a non-finite sample, whatever the solver did.
    #[test]
    fn wire_samples_are_finite_and_quantized() {
        assert_eq!(wire_sample(f64::NAN), 0.0);
        assert_eq!(wire_sample(f64::INFINITY), 0.0);
        assert_eq!(wire_sample(f64::NEG_INFINITY), 0.0);
        assert_eq!(wire_sample(0.0), 0.0);
        // 0.1 mV quantization: below a 5 V peak that is a ~94 dB noise floor.
        assert_eq!(wire_sample(1.234_561_7), 1.2346);
        assert_eq!(wire_sample(-1.234_561_7), -1.2346);
        assert!((wire_sample(2.5) - 2.5).abs() < 1e-6);
    }

    #[test]
    fn panel_ops_add_move_rename_remove() {
        let mut panels: Vec<Panel> = Vec::new();
        let next = AtomicU32::new(1);

        // A backwards drag is normalized; the plid comes from the room.
        let add = PanelOp::Add {
            x0: 10.0,
            y0: 9.0,
            x1: 2.0,
            y1: 1.0,
            name: None,
        };
        assert!(apply_panel_op(&mut panels, &next, &add));
        assert_eq!(panels.len(), 1);
        let plid = panels[0].plid;
        assert_eq!((panels[0].x0, panels[0].y0), (2.0, 1.0));
        assert_eq!((panels[0].x1, panels[0].y1), (10.0, 9.0));
        assert_eq!(panels[0].name, format!("PANEL {plid}"));

        // Degenerate and non-finite rectangles are dropped.
        for bad in [
            PanelOp::Add {
                x0: 0.0,
                y0: 0.0,
                x1: 0.5,
                y1: 8.0,
                name: None,
            },
            PanelOp::Add {
                x0: f64::NAN,
                y0: 0.0,
                x1: 8.0,
                y1: 8.0,
                name: None,
            },
        ] {
            assert!(!apply_panel_op(&mut panels, &next, &bad));
        }
        assert_eq!(panels.len(), 1);

        // Move/resize, then rename (control chars stripped, length capped).
        assert!(apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Rect {
                plid,
                x0: 4.0,
                y0: 4.0,
                x1: 12.0,
                y1: 10.0,
            }
        ));
        assert_eq!((panels[0].x0, panels[0].y1), (4.0, 10.0));
        assert!(apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Rename {
                plid,
                name: "  dimmer\nbench  ".into(),
            }
        ));
        assert_eq!(panels[0].name, "dimmerbench");
        assert!(!apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Rename {
                plid,
                name: "   ".into(),
            }
        ));
        assert!(!apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Rect {
                plid: plid + 999,
                x0: 0.0,
                y0: 0.0,
                x1: 5.0,
                y1: 5.0,
            }
        ));

        // Budget, then removal.
        while panels.len() < MAX_PANELS {
            assert!(apply_panel_op(
                &mut panels,
                &next,
                &PanelOp::Add {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 6.0,
                    y1: 6.0,
                    name: Some("bench".into()),
                }
            ));
        }
        assert!(!apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Add {
                x0: 0.0,
                y0: 0.0,
                x1: 6.0,
                y1: 6.0,
                name: None,
            }
        ));
        assert!(apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Remove { plid }
        ));
        assert!(!apply_panel_op(
            &mut panels,
            &next,
            &PanelOp::Remove { plid }
        ));
        assert_eq!(panels.len(), MAX_PANELS - 1);
    }

    #[test]
    fn old_saves_without_panels_load() {
        let s: SaveFile =
            serde_json::from_str(r#"{"elements":[],"probes":[],"next_pid":3}"#).unwrap();
        assert!(s.panels.is_empty());
        assert_eq!(s.next_plid, 0);
    }

    // ---------------------------------------------------------------- damage
    //
    // The teaching contract, measured end to end: solver output -> thermal
    // stress -> failure -> an open circuit -> repair. Every number these
    // tests assert on comes out of the engine.

    /// THE first lesson in electronics, and it has to bite immediately: an
    /// LED with nothing limiting its current dies, the same LED behind a
    /// proper resistor lives, and the dead one stops conducting.
    ///
    /// The "no series resistor" circuit carries 1 Ω of lead resistance, which
    /// is both realistic and necessary: an IDEAL voltage source straight
    /// across sim-core's ideal exponential junction is a singular DC problem
    /// (the node voltage is pinned, so the diode current is whatever
    /// `Is·exp(9/0.05)` says — 1e60 A), and NR cannot converge on it however
    /// far the rescue ladder halves the step. That case quarantines the
    /// solver instead of breaking the LED — see the assertion at the end of
    /// this test. Fixing it needs source/gmin stepping inside sim-core, which
    /// is a numerics change and therefore its own pass.
    #[test]
    fn an_led_without_a_series_resistor_releases_its_magic_smoke() {
        let bare = vec![
            spec(1, dc(9.0), (0, 0), (0, 8)),
            spec(5, r(1.0), (0, 0), (2, 0)), // lead/contact resistance
            spec(2, K::Led { color: 0 }, (2, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&bare);
        // The solver's own verdict on a diode with nothing to limit it, read
        // before the damage sweep gets a chance to kill it.
        run.eng.advance(200);
        assert!(!run.eng.is_quarantined(), "the solver must survive it");
        let i = run.current(2);
        let limit = damage::rating(&K::Led { color: 0 }).unwrap().limit;
        assert!(
            i.abs() > 10.0 * limit,
            "a bare LED must be wildly over its {limit} A rating, got {i} A"
        );
        // One tick, i.e. under 40 ms of simulated time. There is no ramp to
        // watch here and there should not be one.
        let t = run
            .run_until_break(2, 1.0)
            .expect("a bare LED must not survive");
        assert!(t <= 0.1, "it should die at once, not at t={t}");
        assert_eq!(run.broke[0].1, 2, "the LED dies first, and alone");
        // The 1 Ω lead was dissipating 43 W into a half-watt rating and is
        // left badly scorched — it only survives because the LED opened the
        // circuit first. That is exactly what happens on a bench, and it is
        // worth asserting so the model is not quietly forgiving.
        assert!(
            run.dmg.stress(5) > 0.3,
            "the lead resistance should be scorched: {}",
            run.dmg.stress(5)
        );
        // ...and it is now an open circuit, not a dim LED.
        run.run(0.5);
        assert_eq!(run.current(2), 0.0);
        assert!(run.eng.is_broken(2));
        assert!(!run.eng.is_quarantined());

        // The same LED, driven the way the textbook says: 9 V - 2.1 V over
        // 330 Ω is ~21 mA against a 40 mA rating. It never breaks, and the
        // closed form says it never will however long the room runs.
        let proper = vec![
            spec(1, dc(9.0), (0, 0), (0, 8)),
            spec(2, r(330.0), (0, 0), (4, 0)),
            spec(3, K::Led { color: 0 }, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&proper);
        run.run(5.0);
        let i = run.current(3);
        assert!(
            (0.015..0.025).contains(&i.abs()),
            "expected ~21 mA through the LED, got {i} A"
        );
        assert!(run.broke.is_empty(), "a properly driven LED must survive");
        assert!(!run.dmg.is_broken(3));
        let rating = damage::rating(&K::Led { color: 0 }).unwrap();
        let load = i.abs() / rating.limit;
        assert_eq!(
            damage::time_to_break(damage::heat(rating.metric, load), rating.tau),
            None,
            "at {load:.2}x its rating it must never break, not merely survive 5 s"
        );
        assert!(
            run.dmg.stress(3) < 0.35,
            "and it must not even read as stressed: {}",
            run.dmg.stress(3)
        );

        // The fully ideal version of the same mistake, documented rather than
        // papered over: with literally nothing in series, the solver gives up
        // (and the damage model correctly refuses to cook a part from numbers
        // that were never accepted). The room freezes until somebody edits it.
        let ideal = vec![
            spec(1, dc(9.0), (0, 0), (0, 8)),
            spec(2, K::Led { color: 0 }, (0, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&ideal);
        run.run(0.5);
        assert!(
            run.eng.is_quarantined(),
            "an ideal source across an ideal junction is singular; if this \
             ever converges, delete this branch and assert the LED breaks"
        );
        assert!(
            run.broke.is_empty() && run.dmg.stress(2) == 0.0,
            "a quarantined solver must not cook anything: stale numbers are \
             not evidence"
        );
    }

    /// The overload budget is thermal, not a trip: 2× rated cooks a resistor
    /// in seconds, 80 % of rated runs it hot forever.
    #[test]
    fn a_resistor_cooks_at_twice_its_rating_and_lives_at_eighty_percent() {
        let rating = damage::rating(&K::Resistor { ohms: 100.0 }).unwrap();
        // 10 V across 100 Ω = 1.0 W into a half-watt part.
        let hot = vec![
            spec(1, dc(10.0), (0, 0), (0, 8)),
            spec(2, r(100.0), (0, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&hot);
        run.tick();
        let p = run.eng.frame().iter().find(|f| f.id == 2).unwrap().power;
        assert!((p - 1.0).abs() < 1e-6, "solver says {p} W");
        assert!((p / rating.limit - 2.0).abs() < 1e-6, "that is 2x rated");
        let t = run
            .run_until_break(2, 10.0)
            .expect("2x rated power must cook it");
        // Closed form: tau·ln(2) = 4.16 s. Seconds — long enough to watch it
        // discolour and smoke, short enough to be a lesson.
        let want = damage::time_to_break(2.0, rating.tau).unwrap();
        assert!(
            (t - want).abs() < 0.1,
            "broke at {t} s, closed form {want} s"
        );
        assert!((1.0..8.0).contains(&t), "'in seconds' means {t}");
        assert_eq!(run.current(2), 0.0, "and it is open afterwards");

        // 10 V across 250 Ω = 0.4 W = 80 % of the same rating.
        let warm = vec![
            spec(1, dc(10.0), (0, 0), (0, 8)),
            spec(2, r(250.0), (0, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&warm);
        run.run(20.0);
        let p = run.eng.frame().iter().find(|f| f.id == 2).unwrap().power;
        assert!(
            (p / rating.limit - 0.8).abs() < 1e-6,
            "{p} W is 80 % of rated"
        );
        assert!(run.broke.is_empty(), "80 % of rated must never break");
        // Never, not just "not in 20 s": a resistor at 80 % of rated
        // dissipation settles at 0.8 of its failure temperature.
        assert_eq!(
            damage::time_to_break(damage::heat(rating.metric, 0.8), rating.tau),
            None
        );
        let s = run.dmg.stress(2);
        assert!(
            (0.75..0.81).contains(&s),
            "it should read plainly hot and stay there: {s}"
        );
        assert!((run.current(2) - 0.04).abs() < 1e-9, "still conducting");
    }

    /// A broken part is a gap in the circuit, not a hole in the netlist: its
    /// neighbours keep working and a repair puts it back.
    #[test]
    fn a_broken_part_is_open_and_a_repair_restores_it() {
        // Two lamps in parallel on one 9 V supply.
        let elems = vec![
            spec(1, dc(9.0), (0, 0), (0, 8)),
            spec(
                2,
                K::Lamp {
                    ohms: 90.0,
                    rated_watts: 1.0,
                },
                (0, 0),
                (4, 0),
            ),
            spec(3, K::Wire, (4, 0), (0, 8)),
            spec(
                4,
                K::Lamp {
                    ohms: 45.0,
                    rated_watts: 5.0,
                },
                (0, 0),
                (4, 4),
            ),
            spec(5, K::Wire, (4, 4), (0, 8)),
            gnd(6, (0, 8)),
        ];
        let mut run = DamageRun::new(&elems);
        run.run(0.2);
        assert!((run.current(2) - 0.1).abs() < 1e-6);
        assert!((run.current(4) - 0.2).abs() < 1e-6);

        // Break the 90 Ω lamp the way the room loop does.
        run.dmg.force_break(2);
        run.eng.set_broken(2, true);
        run.run(0.2);
        assert_eq!(run.current(2), 0.0, "dead lamp draws nothing");
        let f = run.eng.frame();
        let dead = f.iter().find(|e| e.id == 2).unwrap();
        assert_eq!(dead.power, 0.0, "and dissipates nothing");
        assert!(
            (run.current(4) - 0.2).abs() < 1e-6,
            "the healthy lamp beside it is untouched"
        );
        assert_eq!(run.dmg.report(64), vec![[2.0, 1.0, 1.0]]);

        // Repair through the server's own path.
        assert!(apply_repair(&mut run.dmg, &mut run.eng, 2));
        assert!(!apply_repair(&mut run.dmg, &mut run.eng, 2), "idempotent");
        assert!(!apply_repair(&mut run.dmg, &mut run.eng, 999), "unknown id");
        assert_eq!(run.dmg.stress(2), 0.0, "a repaired part starts cold");
        assert!(run.dmg.report(64).is_empty(), "nothing left to draw");
        run.run(0.2);
        assert!((run.current(2) - 0.1).abs() < 1e-6, "conducting again");
        assert!(
            run.dmg.stress(2) < 0.05,
            "and it is barely warm at 0.9 W of its 2 W limit: {}",
            run.dmg.stress(2)
        );
        assert!(!run.eng.is_quarantined());
    }

    /// THE HOIST, half one: the reason this feature exists. A 12 V supply
    /// straight across the motor leads parks the crate on the head stop,
    /// where the rotor stops turning, the back-EMF vanishes and the armature
    /// sits at V/R = 6 A — twice its rating. It cooks, and the crate drops.
    #[test]
    fn bare_twelve_volts_across_the_motor_cooks_it() {
        let (mp, mm) = motor_pins();
        let (sp, sm) = ((mp.0 - 9, mp.1), (mm.0 - 9, mm.1));
        let mut run = HoistRun::new(vec![
            spec(1, dc(12.0), sp, sm),
            spec(2, K::Wire, sp, mp),
            spec(3, K::Wire, sm, mm),
            gnd(4, sm),
        ]);

        let mut i_running = 0.0;
        let mut t_break = f64::NAN;
        let mut reached_top = false;
        let mut stress_at_top = 0.0;
        for _ in 0..(8.0 / MACHINE_H) as u32 {
            let i = run.step();
            if !reached_top {
                if run.hoist.y >= machine::SHAFT_H {
                    reached_top = true;
                    stress_at_top = run.motor_stress();
                } else if run.hoist.y > 0.05 {
                    i_running = i;
                }
            }
            if run.motor_broken() && t_break.is_nan() {
                t_break = run.eng.time();
                break;
            }
        }

        assert!(reached_top, "it should lift the crate first");
        assert!(
            stress_at_top < 1.0 && i_running.abs() < motor_i_max(),
            "the LIFT is not what kills it: {i_running:.3} A while travelling, \
             stress {stress_at_top:.3} at the head stop"
        );
        assert!(
            !t_break.is_nan(),
            "a bare 12 V lead must cook the motor (stress {:.3})",
            run.motor_stress()
        );
        // The stall current is the killer, and it takes a couple of seconds:
        // long enough to see the motor heat up and smoke, short enough that
        // the lesson lands.
        let i_stall = 12.0 / machine::R_ARM;
        let rating = damage::rating(&K::Motor {
            ohms: machine::R_ARM,
            henries: machine::L_ARM,
            bemf: 0.0,
        })
        .unwrap();
        let want = damage::time_to_break(
            damage::heat(rating.metric, i_stall / rating.limit),
            rating.tau,
        )
        .unwrap();
        assert!(
            (0.5..5.0).contains(&t_break),
            "broke at {t_break} s (stall {i_stall} A alone would take {want} s)"
        );
        // Consequences: no torque, so the freight comes back down.
        assert_eq!(run.eng.pin_current(MOTOR_ID, 0).unwrap(), 0.0);
        let y_at_break = run.hoist.y;
        for _ in 0..(1.0 / MACHINE_H) as u32 {
            run.step();
        }
        assert!(
            run.hoist.y < y_at_break,
            "a dead motor cannot hold the crate: {} -> {}",
            y_at_break,
            run.hoist.y
        );
        assert!(!run.hoist.win);
        eprintln!(
            "bare 12 V: {i_running:.3} A travelling, broke at {t_break:.3} s, \
             crate fell {:.3} -> {:.3} m",
            y_at_break, run.hoist.y
        );
    }

    /// THE HOIST, half two: the intended solution must be immortal. The same
    /// comparator loop that wins the goal never overheats the motor — its
    /// bang-bang chatter averages far below the nameplate current, which is
    /// exactly the lesson (control the current, do not just apply volts).
    #[test]
    fn a_controlled_drive_holds_the_band_without_cooking_the_motor() {
        let (mp, mm) = motor_pins();
        let sensor = hoist_fixture()
            .into_iter()
            .find(|e| e.id == SENSOR_ID)
            .unwrap();
        let (sa, sw, sb) = (sensor.pins[0], sensor.pins[1], sensor.pins[2]);
        let (sup_p, sup_n) = ((sa.0 - 17, sa.1), (sb.0 - 17, sb.1));
        let (ref_p, ref_n) = ((sa.0 - 17, sa.1 + 8), (sa.0 - 17, sa.1 + 12));
        let (in_p, in_m, out) = (
            (sa.0 - 9, sa.1 + 8),
            (sa.0 - 9, sa.1 + 2),
            (sa.0 - 5, sa.1 + 5),
        );
        let mut run = HoistRun::new(vec![
            spec(1, dc(4.0), sup_p, sup_n),
            gnd(2, sup_n),
            spec(3, K::Wire, sup_p, sa),
            spec(4, K::Wire, sup_n, sb),
            spec(5, dc(3.2), ref_p, ref_n),
            gnd(6, ref_n),
            spec(7, K::Wire, ref_p, in_p),
            spec3(8, K::OpAmp { rail: 5.0 }, in_p, in_m, out),
            spec(9, K::Wire, in_m, sw),
            spec(10, K::Wire, out, mp),
            spec(11, K::Wire, mm, (mm.0 - 5, mm.1)),
            gnd(12, (mm.0 - 5, mm.1)),
        ]);

        // 20 s: ~1.3 s to climb in, 5 s to win, and then 14 s more of holding
        // — over two motor thermal time constants (tau = 6 s), so the stress
        // has converged and "indefinitely" is measured, not hoped for.
        let mut won_at = f64::NAN;
        let mut peak_stress = 0.0f64;
        let mut peak_i = 0.0f64;
        for _ in 0..(20.0 / MACHINE_H) as u32 {
            let i = run.step();
            peak_i = peak_i.max(i.abs());
            peak_stress = peak_stress.max(run.motor_stress());
            if run.hoist.win && won_at.is_nan() {
                won_at = run.eng.time();
            }
            assert!(
                !run.motor_broken(),
                "the intended solution must never break the motor \
                 (t={:.2} s, stress {:.3}, peak {peak_i:.2} A)",
                run.eng.time(),
                run.motor_stress()
            );
        }

        assert!(!won_at.is_nan(), "and it still has to win the goal");
        assert!(run.hoist.in_band(), "still in the band: y={}", run.hoist.y);
        assert!(
            run.broke.is_empty(),
            "nothing at all may break: {:?}",
            run.broke
        );
        // Converged well clear of failure: the switching transients peak
        // above the rating but their heat integral does not come close.
        assert!(
            peak_stress < 0.75,
            "motor stress peaked at {peak_stress:.3} (peak current {peak_i:.2} A)"
        );
        eprintln!(
            "controlled drive: won at {won_at:.2} s, peak {peak_i:.2} A, \
             peak motor stress {peak_stress:.3} (rating {} A)",
            motor_i_max()
        );
    }

    /// Break and repair are the room's state, so they have to survive a
    /// restart — and a save written before parts could break must still load.
    #[test]
    fn stress_and_broken_survive_a_save_round_trip() {
        let elems = vec![
            spec(1, r(100.0), (0, 0), (4, 0)),
            spec(2, K::Led { color: 0 }, (4, 0), (8, 0)),
        ];
        let mut dmg = DamageModel::new();
        dmg.set_document(&elems);
        dmg.force_break(2);
        // A warm-but-alive part, from the model's own integrator.
        let mut warm = DamageRun::new(&[
            spec(1, dc(7.0), (0, 0), (0, 8)),
            spec(2, r(100.0), (0, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ]);
        warm.run(3.0);
        let hot = warm.dmg.stress(2);
        assert!(hot > REPORT_STRESS_FLOOR, "need a warm part: {hot}");

        let save = SaveFile {
            elements: elems.clone(),
            probes: Vec::new(),
            next_pid: 1,
            panels: Vec::new(),
            next_plid: 1,
            hoist: Hoist::default(),
            hoist_rect: HOIST_RECT,
            damage: warm.dmg.clone(),
        };
        let json = serde_json::to_string(&save).unwrap();
        let back: SaveFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.damage.stress(2), hot, "stress must survive verbatim");

        let mut dead = DamageModel::new();
        dead.set_document(&elems);
        dead.force_break(2);
        let json = serde_json::to_string(&SaveFile {
            elements: elems.clone(),
            probes: Vec::new(),
            next_pid: 1,
            panels: Vec::new(),
            next_plid: 1,
            hoist: Hoist::default(),
            hoist_rect: HOIST_RECT,
            damage: dead,
        })
        .unwrap();
        let mut back: SaveFile = serde_json::from_str(&json).unwrap();
        assert!(back.damage.is_broken(2), "the broken set must survive");
        // Ratings are derived, never persisted: they come back from the
        // document, and the restored broken set still applies.
        back.damage.set_document(&back.elements);
        assert!(back.damage.rating_of(2).is_some());
        assert_eq!(back.damage.broken_ids(), vec![2]);

        // Pre-damage saves load as a healthy room.
        let old: SaveFile = serde_json::from_str(r#"{"elements":[]}"#).unwrap();
        assert!(old.damage.parts().is_empty());
        assert!(old.damage.broken_ids().is_empty());
    }

    /// Mirror of `damage::REPORT_AT`, so the test above states its own floor.
    const REPORT_STRESS_FLOOR: f64 = damage::REPORT_AT;

    /// The wire contract for damage, and the client's half of the loop. Both
    /// halves of the protocol are written independently, so both get pinned.
    #[test]
    fn damage_snapshot_and_repair_match_the_contract() {
        let elems = vec![
            spec(7, r(100.0), (0, 0), (4, 0)),
            spec(9, K::Led { color: 0 }, (4, 0), (8, 0)),
            spec(11, K::Wire, (8, 0), (12, 0)),
        ];
        let mut dmg = DamageModel::new();
        dmg.set_document(&elems);
        // Nothing stressed: nothing on the wire at all.
        assert!(dmg.report(MAX_DAMAGE_REPORT).is_empty());

        dmg.force_break(9);
        let msg = json!({"t": "damage", "parts": dmg.report(MAX_DAMAGE_REPORT)}).to_string();
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["t"], "damage");
        let parts = v["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        let row = parts[0].as_array().unwrap();
        assert_eq!(row.len(), 3, "[id, stress, broken]");
        assert_eq!(row[0], 9.0);
        assert_eq!(row[1], 1.0);
        assert_eq!(row[2], 1.0);
        // Wires and grounds never appear: they do not break in this pass.
        assert!(dmg.rating_of(11).is_none());

        // What every client in the room actually receives, tick by tick: a
        // break reaches them on the next tick, a repair on the tick after it,
        // and a quiet room stops talking entirely.
        let mut dmg = DamageModel::new();
        dmg.set_document(&elems);
        let mut shown = false;
        assert_eq!(damage_msg(&dmg, &mut shown), None, "quiet room, no traffic");
        dmg.force_break(9);
        let msg = damage_msg(&dmg, &mut shown).expect("a break must be broadcast");
        assert!(msg.contains(r#""t":"damage""#) && msg.contains("[9.0,1.0,1.0]"));
        assert!(shown);
        // Repeated every tick while it is dead: a late joiner sees it too.
        assert!(damage_msg(&dmg, &mut shown).is_some());
        assert!(dmg.repair(9));
        let cleared = damage_msg(&dmg, &mut shown).expect("the repair must clear it");
        assert!(cleared.contains(r#""parts":[]"#), "{cleared}");
        assert!(!shown);
        assert_eq!(damage_msg(&dmg, &mut shown), None, "and then silence again");

        // Client -> server: the repair verb, including on a locked fixture id
        // (a repair is not a document op, so the 900-999 lock does not apply).
        let msg: ClientMsg = serde_json::from_str(r#"{"t":"repair","id":900}"#).unwrap();
        let ClientMsg::Repair { id } = msg else {
            panic!("expected a repair message")
        };
        assert_eq!(id, MOTOR_ID);
        assert!(reserved_id(id), "and it is a server-owned fixture");
        assert!(!apply_doc_op(
            &test_room(hoist_fixture()),
            &DocOp::Remove { id: MOTOR_ID }
        ));

        // The whole fixture is repairable through the normal path.
        let fixture = hoist_fixture();
        let mut dmg = DamageModel::new();
        dmg.set_document(&fixture);
        let mut eng = Engine::new(DT);
        eng.set_elements(&fixture);
        for id in [MOTOR_ID, SENSOR_ID, LIM_TOP_ID, LIM_BOT_ID] {
            dmg.force_break(id);
            eng.set_broken(id, true);
            assert!(eng.is_broken(id));
            assert!(apply_repair(&mut dmg, &mut eng, id), "#{id} must repair");
            assert!(!eng.is_broken(id));
        }
        assert!(dmg.report(MAX_DAMAGE_REPORT).is_empty());
    }

    /// The showcase room is a demo, not a trap: nothing in it may sit at or
    /// over its rating anywhere in its own operating range, or the gallery
    /// would burn itself down unattended.
    ///
    /// Asserted on LOAD, not on accumulated stress: load < 1 is exactly the
    /// closed-form condition for "never breaks, however long it runs", so a
    /// few seconds of simulation proves an unbounded claim (and the suite does
    /// not have to sit through several thermal time constants of a
    /// 136-element room).
    #[test]
    fn the_showcase_room_never_cooks_itself() {
        let mut elems = demo_room_circuit();
        ensure_fixture(&mut elems, HOIST_RECT);
        // Worst case a visitor can reach without wiring anything new: every
        // switch closed and both pots wound to the end that dissipates most.
        for e in elems.iter_mut() {
            if reserved_id(e.id) {
                continue; // the machine owns its own fixture positions
            }
            match &mut e.kind {
                K::Switch { closed } | K::Button { closed } => *closed = true,
                K::Potentiometer { wiper: w, .. } => *w = 0.98,
                _ => {}
            }
        }
        let mut run = DamageRun::new(&elems);
        // Mean heat input per part over the run. For anything periodic (the
        // 440 Hz speaker vignette, the 0.3 Hz gate sweep) that mean IS the
        // temperature the accumulator converges to, so it is the honest
        // quantity to compare against 1.
        let mut sum: Vec<(u32, f64, u32)> = Vec::new();
        let ticks = (4.0 * TICK_HZ) as u32;
        for _ in 0..ticks {
            run.tick();
            for f in run.eng.frame() {
                let Some(rt) = run.dmg.rating_of(f.id) else {
                    continue;
                };
                let hot = damage::heat(rt.metric, damage::load(&rt, &f));
                match sum.iter_mut().find(|(id, _, _)| *id == f.id) {
                    Some(e) => {
                        e.1 += hot;
                        e.2 += 1;
                    }
                    None => sum.push((f.id, hot, 1)),
                }
            }
        }
        assert!(run.broke.is_empty(), "the showcase broke {:?}", run.broke);
        let worst = sum
            .iter()
            .map(|(id, s, n)| (*id, s / *n as f64))
            .fold((0u32, 0.0f64), |m, e| if e.1 > m.1 { e } else { m });
        assert!(
            worst.1 < 0.9,
            "part #{} settles at {:.2} of its failure temperature — too close \
             to the edge for a demo room",
            worst.0,
            worst.1
        );
        eprintln!(
            "showcase worst case: part #{} settles at {:.2} of failure",
            worst.0, worst.1
        );
    }

    #[test]
    fn panel_client_msg_parses() {
        let msg: ClientMsg =
            serde_json::from_str(r#"{"t":"panel","op":{"t":"add","x0":1,"y0":2,"x1":9,"y1":8}}"#)
                .unwrap();
        let ClientMsg::Panel { op } = msg else {
            panic!("expected a panel message")
        };
        let mut panels = Vec::new();
        assert!(apply_panel_op(&mut panels, &AtomicU32::new(7), &op));
        assert_eq!(panels[0].plid, 7);
    }
}
