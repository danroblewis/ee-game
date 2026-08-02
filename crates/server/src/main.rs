//! Room server: MANY authoritative simulations (one per room), many browsers.
//!
//! A room is a code + a name + a template it was born from + a document +
//! its instruments + optionally a machine. Rooms live one JSON file each
//! under `$EE_ROOMS`, are created from templates (`templates.rs`), are
//! created/listed/renamed/deleted over `/api` (`lobby.rs`), and are held by
//! the registry (`registry.rs`), which owns their lifecycle: a room with
//! nobody in it has no sim task at all.
//!
//! A socket joins with `/ws?room=CODE`; no code lands in the default room,
//! which is what makes a bare `http://host/` behave exactly like the
//! single-room server it grew out of.
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
//!
//! Room-aware additions, all server -> client and all ADDITIVE (a client
//! that ignores unknown `t` values keeps working):
//!   hello{..., room:{id, name, template, players}, view:{home, scopes},
//!         machine: bool}   — which room this is, where the camera lands,
//!                            and whether the room has a goal at all.
//!                            `view` and `machine` sit BESIDE `room`, not
//!                            inside it: they are the client's half of a
//!                            room, not registry metadata about it. That
//!                            exact split is pinned by
//!                            packages/app/src/wire/hello.contract.json and
//!                            asserted from both ends — see
//!                            `the_hello_a_room_sends_is_the_shape_the_
//!                            client_parses` and the client's `wirecheck`.
//!   roommeta{id, name}     — a rename, broadcast to everyone inside
//!   roomgone{id, reason}   — "deleted" | "unknown", then the socket closes
//!
//! `machine{...}` is now sent ONLY by rooms that have a machine. A room
//! whose template declares none (a sandbox, a synth world) never mentions
//! the hoist and never gets ids 900-999 injected into it.

mod lobby;
mod registry;
mod templates;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Router,
};
mod sequencer;
mod synth;
// The measured module library the synth room was assembled from. `synth.rs`
// uses `sequencer`; these two hold designs that did not fit the real-time
// budget (the gm-C kick, the VCO's own pitch knob, the 555 LFO) with the
// measurements that say so. Kept compiled so they cannot silently rot.
#[allow(dead_code)]
mod drums;
#[allow(dead_code)]
mod synth_vco;

use damage::DamageModel;
use machine::Hoist;
use registry::{Life, Parked, Registry, RoomHandle};
use serde::Deserialize;
use serde_json::json;
use sim_core::{DocOp, ElementKind, ElementSpec, Engine, InteractOp, ParamWrite};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use templates::{MachineSpec, RoomSetup, View};
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
        // 220 Ω, not 60: an EMITTER FOLLOWER burns (Vcc - Ve)·Ie in the
        // transistor, so the load current is the transistor's dissipation
        // budget. A 60 Ω lamp put 0.26 W through a TO-92 with the knob at
        // half — genuinely hot, correctly reported as smoking, and a poor
        // thing for a demo room to be doing to itself on the first frame.
        // At 220 Ω the worst case (knob centred) is 0.09 W and the lamp
        // still reaches 78 % of its nameplate wide open.
        spec(16, lamp(220.0, 0.4), (33, 6), (33, 8)),
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
        spec3(52, K::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (26, 13), (26, 15), (30, 14)),
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
        spec3(70, K::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (6, 26), (6, 24), (10, 25)),
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
            tier: 0,
            rot: 0,
        },
        spec(121, K::Wire, (4, 36), (2, 36)),
        gnd(122, (2, 36)),
        spec(123, K::Capacitor { farads: 1e-6 }, (8, 37), (8, 41)),
        gnd(124, (8, 41)),
        spec(125, r(1_000_000.0), (8, 37), (13, 37)), // triangle -> Schmitt in+
        // Schmitt trigger pins: [in+, in-, out]
        spec3(130, K::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (13, 37), (13, 39), (17, 38)),
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
            tier: 0,
            rot: 0,
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
        //
        // 2.5 V peak, not 5: the starting kit's series resistor is a ¼ W
        // film part and its speaker is a ¼ W cone, so 5 V into 16 Ω would
        // be 0.39 W mean in each of them — a demo room that cooks itself.
        // Amplitude here is loudness and nothing else; the vignette is just
        // as audible one notch down, and the parts run at 39 % of rated.
        spec(200, sine(2.5, 440.0), (2, 52), (2, 60)),
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
// across the shaft; the package's title band reads "CRATE IN BAND".
// There is no quest log and nothing to accept: the goal is measured from
// solver quantities and nothing else.
//
// WHAT THE GOAL TEACHES. Voltage sets a SPEED, not a height:
// ω_ss = (K·V/R − m·g·r)/(K²/R + b). At exactly V₀ = m·g·r·R/K = 1.88 V that
// speed is zero, and since ω = 0 is asymptotically stable (back-EMF brakes
// with slope −(K²/R + b)), a CONSTANT V₀ holds the crate still — wherever it
// already is. Height is the integral of speed, so what a constant voltage
// cannot do is CHOOSE a height: V₀ holds where you already are, and any other
// voltage drifts into a stop. Two open-loop wins therefore exist and both are
// legitimate discoveries of the torque balance rather than of feedback:
// drive up and hand-dial to V₀ (`the_balance_voltage_holds_the_band_open_loop`),
// or creep with a V in (V₀, 1.9842 V] so slowly that crossing the 40 mm band
// takes longer than the 5 s hold — 42 s at best, 234 s at 1.90 V, out of a
// 100 mV window. Feedback is what makes the crate find the height instead of
// the player finding the voltage, and what keeps it there when the load moves.
// The goal card says exactly that; it used to say a constant voltage "cannot
// hold the band", which is false.

/// The hoist's footprint SIZE in grid units. Fixed: the assembly MOVES (a
/// player drags the chip, see `move_machine`), it does not resize.
///
/// The footprint is the machine's CELL on the grid, not its package body: the
/// client insets a chip body inside it and leaves a one-unit margin outside
/// the pin columns for the legs to stand in. That is the whole reason the
/// "every pin is inside the rect" invariant (and its test) survived the move
/// to a chip presentation unchanged.
const HOIST_W: i32 = 16;
const HOIST_H: i32 = 15;

/// The footprint of a hoist whose top-left corner is at (x0, y0), in GRID
/// units — broadcast to clients as `rect`. The whole package is drawn inside
/// it and every fixture pin is derived from it, so the box and its terminals
/// can never drift apart.
const fn hoist_rect(x0: i32, y0: i32) -> [i32; 4] {
    [x0, y0, x0 + HOIST_W, y0 + HOIST_H]
}

/// SEAM, server side. Everything the room needs to stand a machine up and let
/// a player drag it around, in one value. `sane_rect_for`, `ensure_fixture_for`
/// and `move_machine_for` are written against only this — a second machine is
/// a second `MachineDef` plus its own mechanism, not a second copy of the
/// footprint/persistence/validation machinery.
struct MachineDef {
    /// Which chip presentation the client should look up (`machine.kind`).
    kind: &'static str,
    /// Footprint size in grid units.
    w: i32,
    h: i32,
    /// The terminal map: rect -> the locked child elements, pins and all.
    fixtures: fn([i32; 4]) -> Vec<ElementSpec>,
}

/// The one machine this room stands up today.
const HOIST: MachineDef = MachineDef {
    kind: "hoist",
    w: HOIST_W,
    h: HOIST_H,
    fixtures: hoist_fixture_at,
};

/// Where a fresh room stands the hoist: its own district east of the showcase
/// vignettes (which occupy x <= 40). The live footprint is room STATE from
/// here on (persisted in `SaveFile::hoist_rect`), not a constant.
const HOIST_RECT: [i32; 4] = hoist_rect(46, 2);

/// Where the SYNTH room stands it. The instrument occupies x <= 52 and the
/// client's home view frames -10..60, so this parks the machine just east of
/// the patch and still on screen. `synth_room_pins_clear_the_hoist` asserts
/// the two never overlap — an overlapping pin would silently wire the
/// sequencer into the motor terminals.
const SYNTH_HOIST_RECT: [i32; 4] = hoist_rect(56, 30);

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
fn sane_rect_for(def: &MachineDef, r: [i32; 4]) -> [i32; 4] {
    let x0 = r[0].min(r[2]).clamp(-WORLD_LIMIT, WORLD_LIMIT - def.w);
    let y0 = r[1].min(r[3]).clamp(-WORLD_LIMIT, WORLD_LIMIT - def.h);
    [x0, y0, x0 + def.w, y0 + def.h]
}

/// The room's single machine, for the call sites that predate the seam.
fn sane_rect(r: [i32; 4]) -> [i32; 4] {
    sane_rect_for(&HOIST, r)
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

/// The fixture: NINE pins on two columns, laid out like a DIP package. Rows
/// are grid units down from the footprint's top edge; the columns are one
/// unit inside its left and right edges.
///
///   row   u=1  ┌───────────────┐  u=15
///     2        │               ├──  TOP A
///     3    M+ ─┤    FREIGHT    ├──  TOP B
///     5    M− ─┤     HOIST     ├──  SNS A   (= top of travel)
///     8        │               ├──  SNS W   (= mid travel)
///    11        │               ├──  SNS B   (= the floor)
///    12        │               ├──  BOT A
///    13        └───────────────┤──  BOT B
///
///   900 Motor         [M+, M−]              left  column, rows 3 and 5
///   902 Switch        [TOP A, TOP B]        right column, rows 2 and 3
///   901 Potentiometer [SNS A, SNS W, SNS B] right column, rows 5 / 8 / 11
///   903 Switch        [BOT A, BOT B]        right column, rows 12 and 13
///
/// Left is drive, right is information — and the right column is a vertical
/// MAP OF THE SHAFT: the top stop at the top, the floor stop at the bottom,
/// the sensor spanning between them with its wiper tap at mid-travel. So
/// SNS A sits at the row the client draws the top of travel on and SNS B at
/// the row it draws the floor on, and the picture inside the package is the
/// wiring rather than a metaphor for it.
///
/// The pot's polarity follows from that and is the one thing here that is
/// electrical rather than cosmetic: `wiper` runs 0 at the head to 1 at the
/// floor (machine::Hoist::sensors), so A must be the TOP end. Excite A from
/// the supply and B to ground and SNS W rises with the crate, which is what
/// the nameplate's "12.5 mV/mm" promises.
///
/// Each device also keeps ALL of its pins in ONE column, which is what stops
/// a child from stealing clicks meant for the package: `hitTest` for a 2- or
/// 3-pin part is the distance to its pin chain, and none of those chains
/// crosses the body.
///
/// EVERY pin is derived from the rect's origin — this function is the whole
/// "terminal map" of the assembly, and the reason a move cannot separate a
/// terminal from its machine. The one-unit margin between the columns and the
/// footprint edge is where the client stands the legs up.
fn hoist_fixture_at(rect: [i32; 4]) -> Vec<ElementSpec> {
    let [x0, y0, ..] = rect;
    let (l, r) = (x0 + 1, x0 + HOIST_W - 1); // the two pin columns
    vec![
        ElementSpec::two(
            MOTOR_ID,
            ElementKind::Motor {
                ohms: machine::R_ARM,
                henries: machine::L_ARM,
                bemf: 0.0,
            },
            (l, y0 + 3),
            (l, y0 + 5),
        ),
        ElementSpec::three(
            SENSOR_ID,
            ElementKind::Potentiometer {
                ohms: SENSOR_OHMS,
                // Crate on the floor: wiper = 1 - y/H, clamped off the end.
                wiper: machine::WIPER_MAX,
            },
            (r, y0 + 5),
            (r, y0 + 8),
            (r, y0 + 11),
        ),
        ElementSpec::two(
            LIM_TOP_ID,
            ElementKind::Switch { closed: false },
            (r, y0 + 2),
            (r, y0 + 3),
        ),
        ElementSpec::two(
            LIM_BOT_ID,
            // Closed: the crate starts on the floor.
            ElementKind::Switch { closed: true },
            (r, y0 + 12),
            (r, y0 + 13),
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

/// One fixture terminal in world coordinates, on the default footprint.
/// Tests that need to wire something to a specific terminal ask for it by
/// NAME rather than spelling the coordinate out, so re-laying the package
/// out moves them with it instead of silently aiming at empty grid.
#[cfg(test)]
fn fixture_pin(id: u32, k: usize) -> (i32, i32) {
    hoist_fixture()
        .into_iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("no fixture {id}"))
        .pins[k]
}

/// The hoist motor's nameplate current (A), read from the damage table so
/// the package's plate can never disagree with the model that enforces it.
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
    // The fixture is placed at tier 0 (`hoist_fixture_at`), so the
    // nameplate the player is shown is the rating the sweep enforces.
    damage::rating(&motor, 0).map(|r| r.limit).unwrap_or(0.0)
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
fn ensure_fixture_for(
    def: &MachineDef,
    elems: &mut Vec<ElementSpec>,
    rect: [i32; 4],
) -> Vec<(u32, Vec<sim_core::Point>)> {
    let mut moved = Vec::with_capacity(4);
    for spec in (def.fixtures)(rect) {
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

/// The room's single machine, for the call sites that predate the seam.
fn ensure_fixture(
    elems: &mut Vec<ElementSpec>,
    rect: [i32; 4],
) -> Vec<(u32, Vec<sim_core::Point>)> {
    ensure_fixture_for(&HOIST, elems, rect)
}

/// Move the whole hoist assembly by an integer grid delta: the footprint AND
/// all four child fixtures, together, in one shot. This is the ONLY way any
/// of them moves — `apply_doc_op` refuses a client `DocOp::Move` on a
/// reserved id — so a player can never separate a terminal from its machine.
/// Returns the children's new pins for the broadcast, or None when the move is
/// refused (no-op, absurd step, or a destination outside the world range).
///
/// SEAM — this is the whole assembly abstraction. It reads nothing about the
/// hoist beyond its `MachineDef`, which carries per instance:
///   * a CHILD LIST (implied by `def.fixtures`, whose ids are reserved);
///   * a FOOTPRINT SIZE (`def.w` / `def.h`);
///   * a TERMINAL MAP from child pins to footprint-relative offsets
///     (`def.fixtures`, which derives every pin from the rect's origin);
///   * PER-INSTANCE WORLD STATE carried along untouched (here: the one
///     `Hoist`) — a translation is not a reset.
///
/// A second machine is a second `MachineDef`; nothing in here changes.
fn move_machine_for(
    def: &MachineDef,
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
    if !(-WORLD_LIMIT..=WORLD_LIMIT - def.w).contains(&x0)
        || !(-WORLD_LIMIT..=WORLD_LIMIT - def.h).contains(&y0)
    {
        return None;
    }
    *rect = [x0, y0, x0 + def.w, y0 + def.h];
    Some(ensure_fixture_for(def, elems, *rect))
}

/// The room's single machine, for the call sites that predate the seam.
fn move_machine(
    elems: &mut Vec<ElementSpec>,
    rect: &mut [i32; 4],
    dx: i32,
    dy: i32,
) -> Option<Vec<(u32, Vec<sim_core::Point>)>> {
    move_machine_for(&HOIST, elems, rect, dx, dy)
}

/// Ids of the document's sources, cached for the energy meter.
fn source_ids(elems: &[ElementSpec]) -> Vec<u32> {
    elems
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ElementKind::VoltageSource { .. }
                    | ElementKind::CurrentSource { .. }
                    | ElementKind::Rail { .. }
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
/// These four writes are the only mutations of the live netlist that do NOT
/// pass the placement gate, and they are ungated on purpose — the mechanism
/// is not a player action and there is no honest way to refuse one. Refusing
/// a limit-switch write would leave the crate at the top with LIM-TOP saying
/// otherwise, which is a lie about the world; and the gate costs ~1 ms while
/// this runs at 1.5 kHz.
///
/// The guarantee is therefore made at PLACEMENT time instead, which is where
/// the owner wants it: `check_document`'s layer 3 factors the document with
/// every switch closed, and layer 4 now runs its convergence trial on that
/// same clone. Anything the mechanism can do to the topology, the gate has
/// already tried. What is left ungated is bounded by construction: `bemf` is
/// RHS-only and cannot make a matrix singular, and `wiper` is clamped into
/// (0, 1) by `write_param` so the pot stays two finite resistances.
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

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, Deserialize)]
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

impl Probe {
    fn from_saved(p: &SavedProbe) -> Probe {
        Probe {
            pid: p.pid,
            elem: p.elem,
            pin: p.pin,
            kind: p.kind,
            r: p.r,
        }
    }

    fn saved(&self) -> SavedProbe {
        SavedProbe {
            pid: self.pid,
            elem: self.elem,
            pin: self.pin,
            kind: self.kind,
            r: self.r,
        }
    }
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
#[derive(Clone, Debug, serde::Serialize, Deserialize)]
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
        /// Client id of the sender, for the `reject` broadcast: an op that
        /// fails the placement gate must tell the client that (optimistically)
        /// applied it, not just silently disagree with it.
        who: u32,
        id: u32,
        op: InteractOp,
    },
    Edit {
        who: u32,
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
    /// the package in hand). Applied at a tick boundary like every other op.
    MachineMove {
        who: u32,
        dx: i32,
        dy: i32,
    },
    /// Fix a part that released its magic smoke (the repair tool).
    Repair {
        who: u32,
        id: u32,
    },
    Join,
    Leave,
    /// Stop the sim task: park (checkpoint: true, graceful shutdown) or
    /// evict (checkpoint: false, the room is being deleted and a checkpoint
    /// would resurrect its file).
    Stop {
        checkpoint: bool,
    },
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
///
/// ALSO the template format. A template is a checkpoint with the identity and
/// the playthrough stripped, which is what makes "save this running room as a
/// template" one function rather than a second serializer. `kind` says which
/// one a file is; every field added since the single-room days defaults, so
/// a `room-save.json` written before rooms existed still loads.
#[derive(serde::Serialize, Deserialize)]
struct SaveFile {
    /// Format version. Absent (0) = the flat single-room save.
    #[serde(default)]
    v: u32,
    /// "room" | "template".
    #[serde(default)]
    kind: String,
    /// Room code, or template id.
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    /// Templates only: the one-liner the create dialog shows.
    #[serde(default)]
    blurb: String,
    /// Rooms only: which template this room was made from (provenance; a room
    /// is never re-linked to it).
    #[serde(default)]
    template: String,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    played: u64,
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
    /// The machine this room arms, if any — the field that makes the hoist
    /// OPTIONAL and per room. Three states, deliberately distinguishable:
    ///   absent          legacy save: fall back to `hoist` / `hoist_rect`
    ///   {"kind":"none"} this room has no machine at all
    ///   {"kind":"hoist", "rect": [...], "state": {...}}
    #[serde(default)]
    machine: Option<MachineSpec>,
    /// Where the camera lands and which in-place scopes the room seeds.
    #[serde(default)]
    view: View,
    /// Mechanical state of the hoist. Defaulted so saves written before the
    /// hoist existed still load (crate on the floor, goal armed).
    ///
    /// LEGACY: superseded by `machine`, still written as a mirror of it so a
    /// file written by this server can still be read by the single-room one.
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

#[derive(Clone, Debug, serde::Serialize, Deserialize)]
struct SavedProbe {
    pid: u32,
    elem: u32,
    pin: usize,
    kind: ProbeKind,
    #[serde(default)]
    r: Option<(u32, usize)>,
}

impl Default for SaveFile {
    fn default() -> Self {
        SaveFile {
            v: SAVE_VERSION,
            kind: "room".into(),
            id: String::new(),
            name: String::new(),
            blurb: String::new(),
            template: String::new(),
            created: 0,
            played: 0,
            elements: Vec::new(),
            probes: Vec::new(),
            next_pid: 1,
            panels: Vec::new(),
            next_plid: 1,
            machine: None,
            view: View::default(),
            // Not [0;4]: an absent footprint means "where the hoist has
            // always stood", the same answer `default_hoist_rect` gives the
            // deserializer.
            hoist_rect: HOIST_RECT,
            hoist: Hoist::default(),
            damage: DamageModel::new(),
        }
    }
}

const SAVE_VERSION: u32 = 1;

impl SaveFile {
    /// A save/template file as a room setup. This is where a LEGACY file (no
    /// `machine` key) gets its hoist back: absent means "the single-room
    /// server wrote this, and that server always had a hoist".
    fn into_setup(self) -> RoomSetup {
        let machine = self.machine.unwrap_or(MachineSpec::Hoist {
            rect: sane_rect(self.hoist_rect),
            state: self.hoist,
        });
        RoomSetup {
            elements: self.elements,
            probes: self.probes,
            next_pid: self.next_pid.max(1),
            panels: self.panels,
            next_plid: self.next_plid.max(1),
            machine,
            view: self.view,
            damage: self.damage,
        }
    }

    fn from_setup(s: &RoomSetup) -> SaveFile {
        // The legacy hoist fields are written as a MIRROR of `machine`, never
        // as a second source of truth: a file this server writes stays
        // readable by the single-room server, and the two copies cannot
        // disagree because one is derived from the other.
        let (hoist, hoist_rect) = match s.machine {
            MachineSpec::Hoist { rect, state } => (state, rect),
            MachineSpec::None => (Hoist::default(), HOIST_RECT),
        };
        SaveFile {
            elements: s.elements.clone(),
            probes: s
                .probes
                .iter()
                .map(|p| SavedProbe {
                    pid: p.pid,
                    elem: p.elem,
                    pin: p.pin,
                    kind: p.kind,
                    r: p.r,
                })
                .collect(),
            next_pid: s.next_pid,
            panels: s.panels.clone(),
            next_plid: s.next_plid,
            machine: Some(s.machine),
            view: s.view.clone(),
            hoist,
            hoist_rect,
            damage: s.damage.clone(),
            ..SaveFile::default()
        }
    }

    fn with_identity(mut self, kind: &str, id: &str, name: &str) -> SaveFile {
        self.kind = kind.into();
        self.id = id.into();
        self.name = name.into();
        self
    }

    fn with_blurb(mut self, blurb: &str) -> SaveFile {
        self.blurb = blurb.into();
        self
    }
}

/// The pre-rooms single-file save. Read once at boot and migrated into the
/// rooms directory; never written again.
fn legacy_save_path() -> String {
    std::env::var("EE_SAVE").unwrap_or_else(|_| "room-save.json".into())
}

fn rooms_dir() -> String {
    std::env::var("EE_ROOMS").unwrap_or_else(|_| "rooms".into())
}

fn templates_dir() -> String {
    std::env::var("EE_TEMPLATES").unwrap_or_else(|_| "templates".into())
}

/// Template a fresh server (and an unqualified `POST /api/rooms`) starts
/// from. "demo" is the showcase + hoist, i.e. exactly what the single-room
/// server booted into.
fn default_template() -> String {
    std::env::var("EE_DEFAULT_TEMPLATE").unwrap_or_else(|_| "demo".into())
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

/// The sim task: sole owner of THIS ROOM's Engine. Ops apply between ticks —
/// the "tick boundary" rule from the plan, at demo scale.
///
/// One task and one Engine PER ROOM, so a room that quarantines, or one
/// carrying a circuit heavy enough to eat its whole step budget, dilates its
/// own sim clock and nothing else's. The tick budget is per task by
/// construction (`MAX_STEPS_PER_TICK`), and an empty room has no task at all
/// (see the park path at the top of the loop).
///
/// The mechanism is OPTIONAL: `machine` comes from the room's template, so a
/// machineless room (sandbox, a synth world) runs the same loop with no
/// co-simulation and no hoist telemetry on the wire.
async fn sim_task(handle: Arc<RoomHandle>, parked: Parked) {
    let Parked {
        rx: mut cmds,
        machine,
        mut damage,
    } = parked;
    let room = handle.room.clone();
    // The hoist's footprint: owned here, beside the mechanism it belongs to,
    // so a move and a machine tick can never interleave.
    let (mut hoist, mut hoist_rect, has_machine) = match machine {
        MachineSpec::Hoist { rect, state } => (state, rect, true),
        MachineSpec::None => (Hoist::default(), HOIST_RECT, false),
    };
    let spec_now = |hoist: &Hoist, rect: [i32; 4]| {
        if has_machine {
            MachineSpec::Hoist {
                rect,
                state: *hoist,
            }
        } else {
            MachineSpec::None
        }
    };
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
        // A restored document predating the placement gate (or hand-edited
        // on disk) can be unsolvable; it cannot be refused — it IS the room —
        // so say why the room is about to freeze instead of freezing mutely.
        if let Err(r) = check_room_doc(&elems) {
            tracing::warn!(
                "restored document fails placement validation ({}): \
                 the room may quarantine until the offending part is removed",
                r.code()
            );
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
    // Consecutive ticks with nobody in the room. An empty room is parked, not
    // stepped forever: N rooms x a 30 Hz solver is the difference between a
    // room selector and a box that melts.
    let mut empty_ticks: u32 = 0;

    loop {
        interval.tick().await;

        // ---- park when empty. The population check and the handover both
        // happen under `handle.parked`, the same lock `Registry::enter`
        // takes, so "the task parked while a player was joining" cannot
        // happen: whoever gets the lock first wins and the other sees the
        // decision already made.
        if room.population.load(Ordering::SeqCst) == 0 {
            empty_ticks += 1;
        } else {
            empty_ticks = 0;
        }
        if empty_ticks >= registry::PARK_AFTER_TICKS {
            let mut slot = handle.parked.lock().unwrap();
            if room.population.load(Ordering::SeqCst) == 0 {
                let machine = spec_now(&hoist, hoist_rect);
                handle.checkpoint(&machine, &damage);
                *slot = Some(Parked {
                    rx: cmds,
                    machine,
                    damage,
                });
                handle.life.send_replace(Life::Parked);
                tracing::info!("room {} parked", handle.meta().id);
                return;
            }
            empty_ticks = 0;
        }

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
            handle.checkpoint(&spec_now(&hoist, hoist_rect), &damage);
        }

        // Assembly moves arriving this tick, summed and applied once below.
        // `pending_mover` is the last client to touch the machine this tick —
        // the one a refused move is reported to.
        let mut pending_move = (0i32, 0i32);
        let mut pending_mover = 0u32;
        // Set by Cmd::Stop. Handled AFTER the drain: the receiver cannot be
        // moved out of while it is being borrowed by `try_recv`.
        let mut stop: Option<bool> = None;
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Cmd::Interact { who, id, op } => {
                    // The hoist fixture is server-owned: no knob drags, no
                    // hand-flipping its limit switches.
                    if reserved_id(id) {
                        continue;
                    }
                    // Same two-phase gate as a document edit: mirror the op
                    // into a candidate copy and validate BEFORE the engine
                    // sees it. This is what refuses a switch flip that would
                    // short a source (possible on documents that predate the
                    // placement gate) and a knob write carrying an
                    // out-of-range value — `SetValue` on a source's dc had
                    // no clamp at all.
                    let candidate = {
                        let elems = room.elements.lock().unwrap();
                        let mut next = elems.clone();
                        apply_interact_to(&mut next, id, op);
                        next
                    };
                    if let Err(r) = check_room_doc(&candidate) {
                        let _ = room.events.send(reject_msg(who, "interact", &r));
                        continue;
                    }
                    eng.interact(id, op);
                    *room.elements.lock().unwrap() = candidate;
                    let _ = room
                        .events
                        .send(json!({"t": "op", "id": id, "op": op}).to_string());
                }
                Cmd::MachineReset => {
                    // A room with no machine has nothing to reset.
                    if has_machine {
                        hoist.reset();
                        room.dirty.store(true, Ordering::Relaxed);
                    }
                }
                Cmd::MachineMove { who, dx, dy } if !has_machine => {
                    // A room with no machine has nothing to move. Dropped
                    // silently rather than refused: there is no cabinet to
                    // point a rejection at, and the client of a machineless
                    // room never draws one to drag.
                    let _ = (who, dx, dy);
                }
                Cmd::MachineMove { who, dx, dy } => {
                    // Coalesced, not applied here: a drag sends ~2 ops per
                    // tick and translation is additive, so summing them costs
                    // one netlist recompile per tick instead of one per op.
                    // Saturating, because the sum of hostile deltas must be a
                    // refused move rather than a panic.
                    pending_move.0 = pending_move.0.saturating_add(dx);
                    pending_move.1 = pending_move.1.saturating_add(dy);
                    pending_mover = who;
                }
                Cmd::Repair { who, id } => {
                    // Deliberately NOT a document op: a repair is a world
                    // event, so it is allowed on the server-owned hoist
                    // fixture (ids 900-999) and it never enters anyone's undo
                    // history. The next tick's snapshot tells every client.
                    //
                    // Gated all the same: a repair re-stamps a branch, and on
                    // a document that predates the placement gate (or was
                    // hand-edited on disk) the all-healthy topology can be
                    // singular — the gate vouches for exactly that document,
                    // so refusing the repair keeps the room alive.
                    if let Err(r) = {
                        let elems = room.elements.lock().unwrap();
                        check_room_doc(&elems)
                    } {
                        let _ = room.events.send(reject_msg(who, "repair", &r));
                        continue;
                    }
                    if apply_repair(&mut damage, &mut eng, id) {
                        room.dirty.store(true, Ordering::Relaxed);
                        tracing::info!("part #{id} repaired");
                    }
                }
                Cmd::Edit { who, op } => {
                    // Two-phase commit: build the candidate document, run the
                    // placement gate, and only then let the engine see it.
                    // An unsolvable op is refused with a machine-readable
                    // reason INSTEAD of quarantining the whole room — the
                    // netlist itself is never corrupted.
                    let candidate = {
                        let elems = room.elements.lock().unwrap();
                        let mut next = elems.clone();
                        apply_doc_op_to(&mut next, &op).then_some(next)
                    };
                    // Syntactic failures (unknown/duplicate/reserved id) stay
                    // silent drops, as before: they are client bugs or races,
                    // not placements a player can act on.
                    let Some(next) = candidate else { continue };
                    if let Err(r) = check_room_doc(&next) {
                        tracing::info!("edit refused ({}): {:?}", r.code(), op);
                        let _ = room.events.send(reject_msg(who, "edit", &r));
                        continue;
                    }
                    {
                        *room.elements.lock().unwrap() = next;
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
                Cmd::Stop { checkpoint } => {
                    // Last one wins: a delete arriving after a park request
                    // must not write the file back out.
                    stop = Some(checkpoint);
                }
            }
        }

        // ---- stop: graceful shutdown (checkpoint, then park) or eviction
        // (the room is gone; write nothing).
        if let Some(cp) = stop {
            let machine = spec_now(&hoist, hoist_rect);
            if !cp {
                return; // evicted
            }
            handle.checkpoint(&machine, &damage);
            // Park only if the room is actually empty — same lock, same
            // re-check as the timer path. A park request that raced a join
            // must not leave a player sitting in a room whose clock stopped.
            let mut slot = handle.parked.lock().unwrap();
            if room.population.load(Ordering::SeqCst) == 0 {
                *slot = Some(Parked {
                    rx: cmds,
                    machine,
                    damage,
                });
                handle.life.send_replace(Life::Parked);
                return;
            }
        }

        // The assembly move, applied once at this tick boundary: one atomic
        // translation of the footprint AND its four children. The mechanism
        // (height, velocity, hold timer, landing count) is deliberately
        // untouched — dragging the box is a move, not a reset.
        if pending_move != (0, 0) {
            // Two-phase like every other mutation: land the fixture on a
            // CANDIDATE copy and gate it. This path never goes through
            // `apply_doc_op`, and a drag can park the closed LIM-BOT switch
            // exactly on a player's source terminals — a move that would
            // freeze the whole room is refused instead (the package simply
            // does not follow the pointer, and the dragger is told why).
            let moved = {
                let elems = room.elements.lock().unwrap();
                let mut next = elems.clone();
                let mut next_rect = hoist_rect;
                move_machine(&mut next, &mut next_rect, pending_move.0, pending_move.1)
                    .map(|children| (next, next_rect, children))
            };
            if let Some((next, next_rect, children)) = moved {
                if let Err(r) = check_room_doc(&next) {
                    let _ = room.events.send(reject_msg(pending_mover, "machinemove", &r));
                } else {
                    *room.elements.lock().unwrap() = next;
                    hoist_rect = next_rect;
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
                        let op = DocOp::Move {
                            id,
                            pins,
                            rot: None,
                        };
                        let _ = room.events.send(json!({"t": "doc", "op": op}).to_string());
                    }
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
        let mut motor_i = if has_machine {
            eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0)
        } else {
            0.0
        };
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
                if has_machine && (c + 1) % per_machine == 0 && !eng.is_quarantined() {
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
            // lagged consumer skips chunks, which costs a blip of silence
            // and desyncs nothing (the client bridges small time gaps and
            // re-primes on large ones).
            //
            // A QUARANTINED solver is a different matter: `advance` is a
            // no-op with time frozen, so this tick's samples are the same
            // stale value with the same t0 as last tick's, forever. Sending
            // them made every chunk trip the client's discontinuity check
            // and read as "audio never buffers" (permanent priming). Not
            // sending is the honest signal — the stream STOPS, the client
            // fades out and reports STALLED, and rt falls to 0 to say why.
            if !taps.is_empty() && !eng.is_quarantined() {
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
            let _ = room.events.send(
                // `rt` rides every frame so the client's status strip can
                // report dilation without a speaker in the room (the
                // audio stream carries its own copy for rate matching).
                json!({"t": "frame", "time": eng.time(), "e": e,
                           "rt": (rt * 1000.0).round() / 1000.0})
                .to_string(),
            );

            // The hoist, once per tick alongside the frame — only in a room
            // that HAS one. A machineless room never mentions it.
            if has_machine {
                let _ = room.events.send(
                    machine_msg(&hoist, hoist_rect, motor_i, impact, motor_i_max()).to_string(),
                );
            }
        }
        // Keep the handle's machine mirror current (footprint AND mechanism).
        // The sim task is authoritative while the room is live; the lobby
        // reads this mirror to save a room as a template, and a cabinet
        // dragged one second ago has to be where the template says it is.
        if has_machine {
            *handle.machine.lock().unwrap() = spec_now(&hoist, hoist_rect);
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
/// assembly is draggable — so the client can draw the whole package (and
/// hit-test it) without hardcoding geometry; `impact` is non-zero
/// only on the tick a landing happened. `imax` is the motor's nameplate
/// current from the damage table — the client engraves it on the package
/// rather than hardcoding a number that could drift from the model that
/// enforces it.
///
/// `kind` names the chip presentation the client should look this machine up
/// under, and `wiper`/`limt`/`limb` are the SENSOR outputs — exactly what the
/// mechanism hands the solver. The client needs them because the stored
/// document's copy of the pot wiper and the two switch positions is written
/// once per tick but never re-broadcast, so a package that drew its sensors
/// from `ElementKind` would be showing state frozen at `hello`. `lim` is the
/// pair of trip heights (m) so the limit blocks can be drawn where they
/// actually trip instead of at the ends of travel.
fn machine_msg(
    hoist: &Hoist,
    rect: [i32; 4],
    motor_i: f64,
    impact: f64,
    i_max: f64,
) -> serde_json::Value {
    let s = hoist.sensors();
    json!({
        "imax": i_max,
        "t": "machine",
        "id": MOTOR_ID,
        "kind": HOIST.kind,
        "rect": rect,
        "wiper": s.wiper,
        "limt": s.lim_top,
        "limb": s.lim_bot,
        "lim": [machine::LIM_BOT_Y, machine::LIM_TOP_Y],
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

/// The placement gate, shared by every path that mutates the netlist (doc
/// edit, interact, repair, machine move): would sim-core accept `elems` as
/// the room's document? One implementation, in sim-core, so the client can
/// run the identical check through sim-wasm. A candidate that fails here is
/// NEVER committed — the live matrix can no longer be corrupted by a
/// placement, only refused.
fn check_room_doc(elems: &[ElementSpec]) -> Result<(), sim_core::Reject> {
    sim_core::check_document(elems, DT)
}

/// The broadcast telling client `who` why its op was refused, with a
/// machine-readable `code` (+ offending element id when there is one) and a
/// human `hint` for the DRC-style callout. Broadcast, not unicast: every
/// client already ignores unknown message types, and the sender needs it to
/// roll back its optimistic local apply.
///
/// `ids` is additive alongside the existing `id`: a conflict implicates two
/// parts and a source loop implicates the whole cycle, and the client wants
/// to flash all of them. Old clients that only read `id` keep working.
fn reject_msg(who: u32, ctx: &str, r: &sim_core::Reject) -> String {
    let ids: Vec<u32> = r.ids().iter().collect();
    json!({
        "t": "reject", "who": who, "ctx": ctx,
        "code": r.code(), "id": r.id(), "ids": ids, "hint": r.hint(),
    })
    .to_string()
}

/// Mirror an interact op into a candidate copy of the specs (same clamps as
/// `Engine::interact`), so the placement gate can judge the document the op
/// would produce BEFORE the engine sees it — and so late joiners get
/// current switch positions and values once it commits.
fn apply_interact_to(elems: &mut [ElementSpec], id: u32, op: InteractOp) {
    use sim_core::ElementKind as K;
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
        (InteractOp::SetValue { value }, K::VoltageSource { dc, .. })
        | (InteractOp::SetValue { value }, K::Rail { dc, .. }) => *dc = value,
        (InteractOp::SetValue { value }, K::CurrentSource { amps }) => *amps = value,
        (InteractOp::SetValue { value }, K::Potentiometer { wiper, .. }) => {
            *wiper = value.clamp(0.01, 0.99)
        }
        _ => {}
    }
}

/// Syntactically validate and apply a document edit. Returns false to drop
/// the op (malformed id, unknown id, or a server-owned fixture). This is
/// only the SYNTACTIC half: the electrical half is `check_room_doc`, run by
/// the edit pipeline on the candidate document this produces, before it is
/// committed.
#[cfg(test)]
fn apply_doc_op(room: &Room, op: &DocOp) -> bool {
    let mut elems = room.elements.lock().unwrap();
    apply_doc_op_to(&mut elems, op)
}

/// `apply_doc_op` against a plain vec, so the edit pipeline can build a
/// CANDIDATE document (clone, apply, validate) without touching the room.
fn apply_doc_op_to(elems: &mut Vec<ElementSpec>, op: &DocOp) -> bool {
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
        DocOp::Move { id, pins, rot } => {
            let Some(e) = elems.iter_mut().find(|e| e.id == *id) else {
                return false;
            };
            if pins.len() != e.kind.pin_count() {
                return false;
            }
            // A rotation out of 0..3 is a client bug, and a Move is the one
            // op that carries geometry for parts a player is dragging: drop
            // the whole op rather than half-applying it, exactly as a wrong
            // pin count does.
            if let Some(r) = rot {
                if *r > 3 {
                    return false;
                }
                e.rot = *r;
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

#[derive(Deserialize)]
struct WsQuery {
    /// Room code. Absent = the default room, so a bare `/ws` behaves exactly
    /// like the single-room server did.
    #[serde(default)]
    room: Option<String>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(reg): State<Arc<Registry>>,
    Query(q): Query<WsQuery>,
) -> impl IntoResponse {
    let handle = reg.resolve(q.room.as_deref());
    let asked = q.room.unwrap_or_default();
    ws.on_upgrade(move |socket| async move {
        match handle {
            Some(h) => client_session(socket, reg, h).await,
            // Accept the upgrade and say WHY: a bad or expired code has to
            // reach the client as a reason it can show, not as an opaque
            // failed handshake.
            None => no_such_room(socket, &asked).await,
        }
    })
}

async fn no_such_room(mut socket: WebSocket, asked: &str) {
    let msg = json!({"t": "roomgone", "id": asked, "reason": "unknown"}).to_string();
    let _ = socket.send(Message::Text(msg.into())).await;
    let _ = socket.send(Message::Close(None)).await;
}

/// The late-join payload: the whole room, including WHICH room it is.
fn hello_msg(handle: &RoomHandle, me: u32) -> String {
    let room = &handle.room;
    let meta = handle.meta();
    let elems = room.elements.lock().unwrap();
    let probes = room.probes.lock().unwrap();
    let panels = room.panels.lock().unwrap();
    let view = handle.view.lock().unwrap();
    json!({
        "t": "hello", "you": me,
        "room": {
            "id": meta.id, "name": meta.name, "template": meta.template,
            "players": room.population.load(Ordering::Relaxed),
        },
        "elements": *elems,
        "probes": *probes, "panels": *panels,
        "view": *view,
        // False means "this room has no goal card": the client hides the
        // hoist chrome instead of latching it forever.
        "machine": handle.has_machine,
    })
    .to_string()
}

async fn client_session(mut socket: WebSocket, reg: Arc<Registry>, handle: Arc<RoomHandle>) {
    let room = handle.room.clone();
    let me = room.next_client.fetch_add(1, Ordering::Relaxed);
    // Counts the player AND resumes the room if it was parked.
    reg.enter(&handle);
    let _ = room.cmds.send(Cmd::Join);
    let mut events = room.events.subscribe();
    let mut life = handle.subscribe_life();
    // A room deleted between resolving it and subscribing: `changed()` only
    // fires on a change AFTER the subscribe, so the already-gone case has to
    // be checked once by hand or the session would wait forever in a room
    // with no sim task.
    if matches!(*life.borrow(), Life::Gone) {
        let msg = json!({"t": "roomgone", "id": handle.meta().id, "reason": "deleted"}).to_string();
        let _ = socket.send(Message::Text(msg.into())).await;
        reg.leave(&handle);
        return;
    }

    let hello = hello_msg(&handle, me);
    if socket.send(Message::Text(hello.into())).await.is_err() {
        reg.leave(&handle);
        let _ = room.cmds.send(Cmd::Leave);
        return;
    }

    loop {
        tokio::select! {
            _ = life.changed() => {
                let gone = matches!(*life.borrow(), Life::Gone);
                if gone {
                    let msg = json!({"t": "roomgone", "id": handle.meta().id,
                                     "reason": "deleted"}).to_string();
                    let _ = socket.send(Message::Text(msg.into())).await;
                    break;
                }
            },
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
                            let _ = room.cmds.send(Cmd::Interact { who: me, id, op });
                        }
                        Ok(ClientMsg::Edit { op }) => {
                            let _ = room.cmds.send(Cmd::Edit { who: me, op });
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
                            let _ = room.cmds.send(Cmd::MachineMove { who: me, dx, dy });
                        }
                        Ok(ClientMsg::Repair { id }) => {
                            let _ = room.cmds.send(Cmd::Repair { who: me, id });
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

    reg.leave(&handle);
    let _ = room.cmds.send(Cmd::Leave);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Every room on disk, loaded PARKED: a room costs a struct and a file
    // until somebody joins it, and only then does it get a sim task.
    let reg = Registry::open(rooms_dir(), templates_dir());
    // A pre-rooms world moves into the rooms directory once, in place, with
    // no flag: the owner's `room-save.json` becomes room "Main Room".
    reg.import_legacy(std::path::Path::new(&legacy_save_path()));
    // Still nothing? A fresh checkout boots into the same showcase + hoist it
    // always did — now as a room with a name and a code.
    reg.ensure_one(&default_template());
    for r in reg.list() {
        tracing::info!(
            "room {} \"{}\" (template {}, {} parts)",
            r.id,
            r.name,
            r.template,
            r.parts
        );
    }

    let dist = std::env::var("EE_DIST").unwrap_or_else(|_| "packages/app/dist".into());
    // Two cache policies, split by path. Vite content-hashes everything under
    // /assets, so those are IMMUTABLE: a returning browser must not even
    // revalidate them (the old headerless setup left caching to browser
    // heuristics, which is exactly "sometimes instant, sometimes re-downloads
    // the world"). The 17 KB shell is the opposite: always revalidate, so a
    // rebuilt bundle never leaves a stale page pointing at dead hashes.
    // NOTE: .layer() only wraps routes registered BEFORE it — the old code
    // layered before fallback_service and the header never applied at all.
    let immutable = tower_http::set_header::SetResponseHeaderLayer::overriding(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    let revalidate = tower_http::set_header::SetResponseHeaderLayer::overriding(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    let shell =
        ServeDir::new(&dist).not_found_service(ServeFile::new(format!("{dist}/index.html")));
    let app = Router::new()
        .route("/ws", get(ws_handler))
        // The lobby: create / choose / delete rooms, and the template
        // registry. Plain HTTP, because it is what you talk to when you do
        // not have a room socket yet.
        .merge(lobby::routes())
        .nest_service(
            "/assets",
            axum::routing::get_service(ServeDir::new(format!("{dist}/assets"))).layer(immutable),
        )
        .fallback_service(axum::routing::get_service(shell).layer(revalidate))
        .with_state(reg.clone());

    let addr = std::env::var("EE_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("ee-game server on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();

    // Park every live room on the way out. The old server checkpointed only
    // every ~5 s and never on SIGINT, so a restart threw away up to five
    // seconds of every room; asking each sim task to stop makes the shutdown
    // path write the same checkpoint the tick loop would have.
    let live: Vec<_> = reg.all().into_iter().filter(|h| h.is_live()).collect();
    for h in &live {
        let _ = h.room.cmds.send(Cmd::Stop { checkpoint: true });
    }
    for _ in 0..40 {
        if live.iter().all(|h| !h.is_live()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tracing::info!("parked {} rooms", live.len());
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
            // Every hoist run is a document a player could have placed, so
            // it has to pass the same gate a placement does. This is what
            // keeps the placement gate honest about the ONE circuit the game
            // is built to teach: tighten the gate and break the intended
            // solution, and this fails before the win test even runs.
            assert_eq!(check_room_doc(&elems), Ok(()), "the run must be placeable");
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

        /// Like `step`, but samples one element's drain-source voltage on
        /// EVERY solver substep. The machine tick is 640 us and an
        /// inductive turn-off spike is tens of microseconds long, so a
        /// once-per-tick sample aliases it away completely — which is a
        /// trap for any test that tries to measure switching transients.
        fn step_watching(&mut self, id: u32, peak: &mut f64) {
            for _ in 0..MACHINE_SUBSTEPS {
                self.eng.advance(1);
                let a = self.eng.pin_voltage(id, 1).unwrap_or(0.0);
                let b = self.eng.pin_voltage(id, 2).unwrap_or(0.0);
                *peak = peak.max(a - b);
            }
            machine_step(&mut self.eng, &mut self.hoist, &self.sources);
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

        // Voltage buys SPEED: 12 V commands 0.80 m/s, so the crate crosses
        // the 40 mm band in 50 ms and parks 60 mm above it, and the hold
        // drains back to nothing. Note the claim is about THIS voltage, not
        // about constant voltages — see
        // `the_balance_voltage_holds_the_band_open_loop` for the one that
        // does hold, and the module header for why both are true.
        assert!(!run.hoist.win, "12 V open loop must never win");
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

    /// THE OTHER HALF OF THE TRUTH, and the reason the goal card no longer
    /// claims a constant voltage "cannot hold the band".
    ///
    /// ω = 0 is an *asymptotically stable* equilibrium of
    /// J·ω̇ = K(V − K·ω)/R − m·g·r − b·ω: the derivative in ω is
    /// −(K²/R + b) < 0, so the electrical brake actively pulls the speed back
    /// to whatever the applied voltage commands. At exactly
    /// V₀ = m·g·r·R/K = 1.88352 V that commanded speed is ZERO, and the crate
    /// therefore sits still — wherever it happens to be, band included.
    ///
    /// What a constant voltage cannot do is *choose* a height: `y` is a pure
    /// integrator of ω, so V₀ holds you where you already are and any other
    /// voltage drifts until it meets a stop. That distinction is the lesson,
    /// and this test is what keeps the card honest about it.
    #[test]
    fn the_balance_voltage_holds_the_band_open_loop() {
        let v0 = machine::LOAD_TORQUE * machine::R_ARM / machine::K;
        assert!((v0 - 1.88352).abs() < 1e-9, "V0 = {v0}");
        let (mp, mm) = motor_pins();
        let (sp, sm) = ((mp.0 - 9, mp.1), (mm.0 - 9, mm.1));
        let mut run = HoistRun::new(vec![
            spec(1, dc(v0), sp, sm),
            spec(2, K::Wire, sp, mp),
            spec(3, K::Wire, sm, mm),
            gnd(4, sm),
        ]);
        // Start where a player who drove up and then dialled the supply back
        // would be: mid-band, still. No feedback of any kind is wired.
        run.hoist.y = (machine::BAND_LO + machine::BAND_HI) / 2.0;

        let mut drift = 0.0f64;
        for _ in 0..(6.0 / MACHINE_H) as u32 {
            run.step();
            drift = drift.max((run.hoist.y - 0.32).abs());
            if run.hoist.win {
                break;
            }
        }
        assert!(
            run.hoist.win,
            "the balance voltage DOES hold the band: hold={:.4} s y={:.5} m",
            run.hoist.hold, run.hoist.y
        );
        assert!(
            drift < 1e-4,
            "and it holds to within a tenth of a millimetre: drift {drift:.3e} m"
        );
        assert_eq!(run.hoist.landings, 0);
        eprintln!(
            "V0 = {v0:.5} V open loop: won at {:.3} s, y = {:.5} m, drift {drift:.2e} m, {:.2} J",
            run.eng.time(),
            run.hoist.y,
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
        // Power stage, to the right of the terminal column.
        let d_low = (mm.0 + 4, mm.1); // motor low side = diode anode
        let d_high = (mp.0 + 4, mp.1); // motor high side = diode cathode
        let gate = (mm.0 + 4, mm.1 + 2);
        let drain = (mm.0 + 8, mm.1);
        let source = (mm.0 + 8, mm.1 + 4);
        let sgnd = (mm.0 + 8, mm.1 + 8);
        let (bat_p, bat_n) = ((mp.0 + 8, mp.1), (mp.0 + 8, mp.1 - 4));
        let corner = (out.0, gate.1);
        vec![
            // ---- brain: sensor excitation, 4 V across SENSE-A .. SENSE-B.
            spec(1, dc(4.0), sup_p, sup_n),
            gnd(2, sup_n),
            spec(3, K::Wire, sup_p, sa),
            spec(4, K::Wire, sup_n, sb),
            // Setpoint: 3.2 V = 4 V · (0.32 / 0.40).
            spec(5, dc(3.2), ref_p, ref_n),
            gnd(6, ref_n),
            spec(7, K::Wire, ref_p, in_p),
            // Comparator: in+ = setpoint, in- = wiper. Its output goes to a
            // GATE, not to the motor: 25 mA cannot lift a crate, and the
            // gate of a MOSFET draws exactly none.
            spec3(
                8,
                K::OpAmp {
                    rail: 5.0,
                    isc: sim_core::DEFAULT_OPAMP_ISC,
                },
                in_p,
                in_m,
                out,
            ),
            spec(9, K::Wire, in_m, sw),
            spec(10, K::Wire, out, corner),
            spec(11, K::Wire, corner, gate),
            // ---- muscle: 12 V through the motor, low-side switched by a
            // power NMOS (tier 1: TO-220 on a small heatsink), with a
            // freewheel diode across the winding.
            spec(12, dc(6.0), bat_p, bat_n),
            gnd(13, bat_n),
            spec(14, K::Wire, bat_p, mp),
            spec(15, K::Wire, mp, d_high),
            spec(16, K::Diode, d_low, d_high),
            spec(17, K::Wire, mm, d_low),
            spec(18, K::Wire, d_low, drain),
            spec3(19, K::Nmos { vt: 2.0, k: 5.0 }, gate, drain, source).at_tier(1),
            spec(20, K::Wire, source, sgnd),
            gnd(21, sgnd),
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
                pins: vec![(0, 0), (0, 4)], rot: None },
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
        // The nameplate current the package engraves comes from the damage
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
        assert_eq!(v["rect"], json!([46, 2, 62, 17]));
        assert_eq!(v["kind"], "hoist");
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
        // The sensor outputs: exactly the numbers the mechanism hands the
        // solver, so the package's picture of its own sensors can never
        // disagree with what the circuit sees.
        let s = hoist.sensors();
        assert_eq!(v["wiper"], s.wiper);
        assert_eq!(v["wiper"], (1.0 - 0.321 / machine::SHAFT_H).clamp(0.02, 0.98));
        // The limit switches are LATCHED (hysteresis), so the broadcast
        // reports what the mechanism last latched, not what `y` implies —
        // this fixture was posed at 321 mm without ticking, so it still
        // carries the floor latch it was built with. Reporting anything else
        // would be the client drawing a switch the solver does not have.
        assert_eq!(v["limt"], false);
        assert_eq!(v["limb"], true);
        assert_eq!(v["lim"], json!([machine::LIM_BOT_Y, machine::LIM_TOP_Y]));
        // rect must actually contain every fixture pin, or the client draws
        // terminals outside the box it was told about. This is the invariant
        // that makes the chip presentation cheap: the rect is the machine's
        // CELL, the package body is inset inside it, and the legs point
        // inward — so pins on legs never leave the box.
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

    /// Dragging the package is a TRANSLATION: the crate does not teleport to
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
    /// dragging the motor out of its own package.
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
                !apply_doc_op(&room, &DocOp::Move { id, pins: shifted, rot: None }),
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
            ..SaveFile::default()
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
        let [rx0, ry0, rx1, ry1] = HOIST_RECT;
        assert_eq!(sane_rect([rx1, ry1, rx0, ry0]), HOIST_RECT);
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

    /// Rotation and tier ride the ordinary op pipeline, and the pipeline is
    /// the only place they can be set — so the authority is the authority
    /// for these too, and a stale or hostile client cannot smuggle a
    /// nonsense value into a shared room.
    #[test]
    fn rotation_and_tier_flow_through_the_op_pipeline() {
        use sim_core::ElementKind as K;
        let room = test_room(vec![
            gnd(1, (0, 0)),
            spec(2, r(1000.0), (0, 0), (4, 0)),
            spec(3, dc(9.0), (4, 0), (0, 0)),
        ]);
        let rot_of = |id: u32| {
            room.elements
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .rot
        };

        // A one-pin part rotates without its pin moving: that is the whole
        // reason the field exists.
        assert_eq!(rot_of(1), 0);
        assert!(apply_doc_op(
            &room,
            &DocOp::Move {
                id: 1,
                pins: vec![(0, 0)],
                rot: Some(3),
            }
        ));
        assert_eq!(rot_of(1), 3);
        assert_eq!(
            room.elements.lock().unwrap()[0].pins,
            vec![(0, 0)],
            "a rotation must not move the connection point"
        );

        // A Move with no `rot` (every drag, and every op an older client
        // sends) leaves the symbol exactly as it was.
        assert!(apply_doc_op(
            &room,
            &DocOp::Move {
                id: 1,
                pins: vec![(2, 2)],
                rot: None,
            }
        ));
        assert_eq!(rot_of(1), 3, "a plain move must not reset the orientation");

        // Out of range: the whole op is dropped, like a wrong pin count.
        assert!(!apply_doc_op(
            &room,
            &DocOp::Move {
                id: 1,
                pins: vec![(2, 2)],
                rot: Some(9),
            }
        ));
        assert_eq!(rot_of(1), 3);

        // A tier arrives with the part, and a tier nobody has heard of is
        // refused by the shared validator rather than clamped somewhere.
        let power = |tier: u8| DocOp::Add {
            spec: ElementSpec::three(10, K::Nmos { vt: 2.0, k: 5.0 }, (8, 0), (12, 0), (12, 4))
                .at_tier(tier),
        };
        let mut next = room.elements.lock().unwrap().clone();
        assert!(apply_doc_op_to(&mut next, &power(1)));
        assert_eq!(check_room_doc(&next), Ok(()));
        assert_eq!(next.last().unwrap().tier, 1);
        assert_eq!(
            damage::rating(&K::Nmos { vt: 2.0, k: 5.0 }, 1).unwrap().limit,
            20.0,
            "and it is judged as the power part it says it is"
        );
        let mut next = room.elements.lock().unwrap().clone();
        assert!(apply_doc_op_to(&mut next, &power(sim_core::MAX_TIER + 1)));
        assert!(matches!(
            check_room_doc(&next),
            Err(sim_core::Reject::BadValue { id: 10, .. })
        ));
    }

    /// Rooms written before any of this existed have to load, and load as
    /// the parts they were: starting-kit tier, unrotated, and — for the
    /// op-amp — the 25 mA jellybean they were implicitly promising to be.
    #[test]
    fn documents_written_before_tiers_still_load() {
        let old = r#"[
            {"id":1,"kind":{"t":"Ground"},"pins":[[0,0]]},
            {"id":2,"kind":{"t":"Resistor","ohms":1000.0},"pins":[[0,0],[4,0]]},
            {"id":3,"kind":{"t":"OpAmp","rail":9.0},"pins":[[8,0],[8,2],[12,1]]}
        ]"#;
        let elems: Vec<ElementSpec> = serde_json::from_str(old).expect("old rooms must load");
        assert!(elems.iter().all(|e| e.tier == 0 && e.rot == 0));
        assert_eq!(
            elems[2].kind,
            sim_core::ElementKind::OpAmp {
                rail: 9.0,
                isc: sim_core::DEFAULT_OPAMP_ISC
            }
        );
        assert_eq!(check_room_doc(&elems), Ok(()));
        // And an old Move op — no `rot` field at all — still applies.
        let op: DocOp = serde_json::from_str(r#"{"t":"Move","id":2,"pins":[[0,0],[6,0]]}"#)
            .expect("old ops must parse");
        let mut next = elems.clone();
        assert!(apply_doc_op_to(&mut next, &op));
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
                tier: 0,
                rot: 0,
            })
            .collect();
        let room = test_room(full);
        let wire = |id: u32| DocOp::Add {
            spec: ElementSpec {
                id,
                kind: K::Wire,
                pins: vec![(2, 2), (3, 2)],
                tier: 0,
                rot: 0,
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
                    tier: 0,
                    rot: 0,
                },
            }
        ));
        assert!(!apply_doc_op(
            &room,
            &DocOp::Move {
                id: 900_001,
                pins: vec![(0, 0)], rot: None }
        ));
        assert!(apply_doc_op(
            &room,
            &DocOp::Move {
                id: 900_001,
                pins: vec![(4, 4), (5, 4)], rot: None }
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
        let limit = damage::rating(&K::Led { color: 0 }, 0).unwrap().limit;
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
        let rating = damage::rating(&K::Led { color: 0 }, 0).unwrap();
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
        let rating = damage::rating(&K::Resistor { ohms: 100.0 }, 0).unwrap();
        // 7.07 V across 100 Ω = 0.5 W into a quarter-watt part.
        let hot = vec![
            spec(1, dc(50.0f64.sqrt()), (0, 0), (0, 8)),
            spec(2, r(100.0), (0, 0), (4, 0)),
            spec(3, K::Wire, (4, 0), (0, 8)),
            gnd(4, (0, 8)),
        ];
        let mut run = DamageRun::new(&hot);
        run.tick();
        let p = run.eng.frame().iter().find(|f| f.id == 2).unwrap().power;
        assert!((p - 0.5).abs() < 1e-6, "solver says {p} W");
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

        // 10 V across 500 Ω = 0.2 W = 80 % of the same rating.
        let warm = vec![
            spec(1, dc(10.0), (0, 0), (0, 8)),
            spec(2, r(500.0), (0, 0), (4, 0)),
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
        assert!((run.current(2) - 0.02).abs() < 1e-9, "still conducting");
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
        }, 0)
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
        // The SAME circuit `comparator_feedback_holds_the_crate_in_the_band`
        // wins with — one builder, so the two halves of the claim ("it wins"
        // and "it survives") can never drift apart.
        let mut run = HoistRun::new(comparator_feedback_circuit());

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
        // Every part in the bill of materials, not just the motor — this is
        // the whole point of the recalibration pass. Ids are the ones
        // `comparator_feedback_circuit` builds.
        let bom = [
            (8, "op-amp"),
            (12, "6 V supply"),
            (14, "supply wire"),
            (16, "freewheel diode"),
            (18, "drain wire"),
            (19, "power NMOS"),
            (20, "source wire"),
        ];
        for (id, what) in bom {
            let s = run.dmg.stress(id);
            assert!(
                s < 0.75,
                "{what} (#{id}) settled at {s:.3} of failure — the intended \
                 solution has to have margin everywhere, not just at the motor"
            );
            eprintln!("  {what:<16} #{id:<3} stress {s:.3}");
        }
        eprintln!(
            "controlled drive: won at {won_at:.2} s, peak {peak_i:.2} A, \
             peak motor stress {peak_stress:.3} (rating {} A)",
            motor_i_max()
        );
    }

    /// What happens when the freewheel diode is left out — and what must
    /// NOT happen.
    ///
    /// Switch an inductive load off and its stored current has to go
    /// somewhere. With no diode the only path left is the FET itself, in
    /// avalanche. Before the MOSFET model had a breakdown clamp there was
    /// no path at all: NR diverged on the first turn-off and the whole room
    /// froze with nothing on screen to explain it — the worst possible
    /// answer, because a frozen room teaches nothing and cannot be
    /// debugged. Now the drain simply pins at the breakdown knee and the
    /// winding dumps into the part, which is what really happens.
    ///
    /// The measured verdict for THIS machine is honest and slightly
    /// anticlimactic, and the test says so rather than pretending
    /// otherwise: the hoist armature is 1.5 mH, so each turn-off is about
    /// 7 mJ — nothing to a TO-220, which is avalanche-rated for hundreds of
    /// millijoules. The diode is good practice and it keeps the switching
    /// node inside the supply rail; it is not what saves the part here. A
    /// bigger winding would be a different story, and the model would say
    /// so on its own.
    #[test]
    fn switching_a_motor_without_a_freewheel_diode_avalanches_the_fet() {
        let mut circuit = comparator_feedback_circuit();
        circuit.retain(|e| e.id != 16); // the freewheel diode
        let mut run = HoistRun::new(circuit);
        for _ in 0..(3.0 / MACHINE_H) as u32 {
            run.step(); // climb into the band, where the bang-bang lives
        }
        let mut peak_vds = 0.0f64;
        for _ in 0..(0.5 / MACHINE_H) as u32 {
            run.step_watching(19, &mut peak_vds);
        }
        assert!(
            !run.eng.is_quarantined(),
            "an unclamped turn-off must never freeze the room"
        );
        assert!(
            peak_vds > 50.0,
            "the drain must ring up to the avalanche knee: peaked at {peak_vds:.1} V \
             on a 6 V supply"
        );

        // With the diode fitted, the same drive never leaves the supply
        // rail: the winding freewheels through the diode instead of through
        // the transistor. That difference is the whole reason the part is
        // in the circuit, and it is measurable.
        let mut run = HoistRun::new(comparator_feedback_circuit());
        for _ in 0..(3.0 / MACHINE_H) as u32 {
            run.step();
        }
        let mut peak_clamped = 0.0f64;
        for _ in 0..(0.5 / MACHINE_H) as u32 {
            run.step_watching(19, &mut peak_clamped);
        }
        assert!(
            peak_clamped < 8.0,
            "with a freewheel diode the drain stays near the rail: {peak_clamped:.1} V"
        );
        eprintln!(
            "freewheel diode: drain peaks {peak_vds:.1} V without it, \
             {peak_clamped:.1} V with it"
        );
    }

    /// The other half of the honest op-amp: the OLD winning solution — an
    /// op-amp output wired straight to M+ — must now be unable to move the
    /// crate at all. Not "it breaks": it physically cannot deliver the
    /// current, which is the truthful failure and the one that teaches
    /// something. `machine::HOLD_CURRENT` is 0.94 A; a 741-class output
    /// stage folds back at 25 mA, i.e. 2.7 % of the torque gravity asks for.
    #[test]
    fn an_op_amp_cannot_drive_a_motor() {
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
            spec3(
                8,
                K::OpAmp {
                    rail: 5.0,
                    isc: sim_core::DEFAULT_OPAMP_ISC,
                },
                in_p,
                in_m,
                out,
            ),
            spec(9, K::Wire, in_m, sw),
            spec(10, K::Wire, out, mp), // straight into the motor
            spec(11, K::Wire, mm, (mm.0 - 5, mm.1)),
            gnd(12, (mm.0 - 5, mm.1)),
        ]);

        let mut peak_i = 0.0f64;
        for _ in 0..(6.0 / MACHINE_H) as u32 {
            peak_i = peak_i.max(run.step().abs());
        }
        assert!(
            peak_i <= sim_core::DEFAULT_OPAMP_ISC * 1.001,
            "an op-amp may never source more than its isc: {peak_i:.4} A"
        );
        assert_eq!(run.hoist.y, 0.0, "and the crate never leaves the floor");
        assert!(!run.hoist.win);
        // It does not explode either: a real op-amp is short-circuit proof,
        // so it sits there refusing to deliver, indefinitely.
        assert!(!run.dmg.is_broken(8), "an op-amp survives its own limit");
        // It DOES run warm: 25 mA held against a 5 V rail is 0.124 W of a
        // 0.35 W part, so the faceplate reads about a third of the way to
        // failure and stays there. Hot, honest, and immortal.
        let s = run.dmg.stress(8);
        assert!(
            (0.25..0.45).contains(&s),
            "a shorted op-amp should read plainly warm and survive: {s:.3}"
        );
        eprintln!(
            "op-amp direct drive: {peak_i:.4} A peak vs {:.4} A needed to hold",
            machine::HOLD_CURRENT
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
            ..SaveFile::default()
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
            ..SaveFile::default()
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
            gnd(11, (8, 0)),
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
        // A ground symbol never appears: it is a reference, not a part.
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
            worst.1 < 0.7,
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

    // ---------------------------------------------------------- synth room
    //
    // The second sample world. It arrived with its own `EE_WORLD` switch,
    // which the template registry supersedes — it is now the `synth`
    // built-in, reached the way every other room is reached.

    /// The default template is still the showcase plus a hoist, and the synth
    /// is a room you choose rather than a mode the binary boots into.
    #[test]
    fn the_default_template_is_still_the_showcase_and_synth_is_its_own() {
        let demo = templates::BUILTINS
            .iter()
            .find(|b| b.id == default_template())
            .expect("the default template exists");
        let setup = (demo.build)();
        let want = demo_room_circuit();
        assert!(
            setup.elements.len() > want.len(),
            "the default room is the showcase plus a hoist fixture"
        );
        for (a, b) in setup.elements.iter().zip(want.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.pins, b.pins);
        }

        let synth = templates::BUILTINS
            .iter()
            .find(|b| b.id == "synth")
            .expect("the synth ships as a template");
        let s = (synth.build)();
        assert!(!s.elements.is_empty(), "the synth room has parts");
        assert!(!s.panels.is_empty(), "and its labels");
        assert!(
            !s.machine.is_some(),
            "the synth arms no machine: its goal is that it makes a noise"
        );
    }

    /// The room is a musical instrument, and an instrument that cannot hold
    /// real time plays FLAT: sim-time dilation is a pitch error, not a frame
    /// drop. Measured on an Apple M4 (release, pinned cargo 1.95.0, machine
    /// under load from other builds, two runs of three passes): **13.60-13.73
    /// µs per substep = 1.46-1.47x real time**, and the LIVE server reports
    /// rt 0.993 against the shipped showcase room's 0.985 on the same
    /// machine. Cost in this engine goes as
    /// `newton_iterations x elements^1.64`, so the element count is the
    /// budget, and this is the guard rail on it.
    #[test]
    fn the_synth_room_fits_the_realtime_budget() {
        let n = synth::synth_room_circuit().len();
        assert!(
            n <= 72,
            "the synth room grew to {n} elements; at 1.45x real time it had \
             71, and the margin is not there to spend"
        );
    }

    /// Nothing in the instrument may sit inside the machine's footprint: an
    /// overlapping pin would wire the sequencer into the motor terminals.
    #[test]
    fn synth_room_pins_clear_the_hoist() {
        let [x0, y0, x1, y1] = SYNTH_HOIST_RECT;
        for e in synth::synth_room_circuit() {
            for (px, py) in &e.pins {
                assert!(
                    *px < x0 || *px > x1 || *py < y0 || *py > y1,
                    "element {} pin ({px},{py}) is inside {SYNTH_HOIST_RECT:?}",
                    e.id
                );
            }
        }
        // And the whole patch is inside the client's home view, so a player
        // who joins is looking at the knobs rather than at empty canvas.
        for e in synth::synth_room_circuit() {
            for (px, py) in &e.pins {
                assert!(
                    (-10..=60).contains(px) && (-10..=60).contains(py),
                    "element {} pin ({px},{py}) is outside the home view",
                    e.id
                );
            }
        }
    }

    /// The speaker must own the lowest Speaker id in the room, because the
    /// server only streams the four lowest and a player dropping speakers
    /// next to the instrument must not be able to mute it.
    #[test]
    fn the_synth_speaker_is_always_an_audio_tap() {
        let mut elems = synth::synth_room_circuit();
        assert_eq!(audio_tap_ids(&elems), vec![synth::ID_SPEAKER]);
        for id in 200..206 {
            elems.push(ElementSpec::two(
                id,
                ElementKind::Speaker { ohms: 8.0 },
                (70, 70),
                (74, 70),
            ));
        }
        assert!(audio_tap_ids(&elems).contains(&synth::ID_SPEAKER));
    }

    /// It must be ALIVE the moment the room boots and stay alive: the
    /// oscillator running, the clock stepping, nothing quarantining.
    #[test]
    fn synth_room_never_quarantines() {
        let elems = synth::synth_room_circuit();
        let sq = synth::seq_config();
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        let mut osc_flips = 0u32;
        let mut osc_high = false;
        let mut bar_flips = 0u32;
        let mut bar_high = true;
        let mut rescues = 0u32;
        // 30 simulated seconds in 10 ms chunks.
        for chunk in 0..3000 {
            let rep = eng.advance(500);
            rescues += rep.rescues;
            assert!(
                !eng.is_quarantined(),
                "quarantined at t={:.3}s (chunk {chunk})",
                eng.time()
            );
            // The VCO's comparator output, +-5 V.
            let osc = eng.voltage_at((44, -8)).unwrap_or(0.0) > 0.0;
            if osc != osc_high {
                osc_flips += 1;
                osc_high = osc;
            }
            // The 555's bar marker: high, pulsing low once per four steps.
            let bar = eng.voltage_at(sq.bar()).unwrap_or(0.0) > 4.0;
            if bar != bar_high {
                bar_flips += 1;
                bar_high = bar;
            }
        }
        // Sampled every 10 ms, so this undercounts a 250 Hz oscillator
        // enormously -- it only has to prove the thing never stopped.
        assert!(osc_flips > 500, "the VCO only flipped {osc_flips} times");
        // ~1 s per bar, and the marker is low for 5 ms so a 10 ms sampler
        // catches most but not all of them.
        assert!(
            (20..=70).contains(&bar_flips),
            "the bar marker flipped {bar_flips} times in 30 s (expect ~2 per bar)"
        );
        assert_eq!(rescues, 0, "the solver needed {rescues} rescue steps");
    }

    /// What it actually plays. The four pitch knobs ship trimmed BY
    /// MEASUREMENT -- the CV row is linear in the wiper only to about 2 %, so
    /// nominal values would be a semitone out -- and this is what stops them
    /// drifting silently out of tune when anything upstream changes.
    #[test]
    fn synth_room_plays_a_tune() {
        let elems = synth::synth_room_circuit();
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        eng.advance(150_000); // 3 s: let the bar and the LFO cap settle
        let sq = synth::seq_config();
        // Sample the oscillator and the CV bus for three bars.
        let n = 160_000usize;
        let mut osc = Vec::with_capacity(n);
        let mut cv = Vec::with_capacity(n);
        for _ in 0..n {
            eng.advance(1);
            osc.push(eng.voltage_at((44, -8)).unwrap_or(0.0));
            cv.push(eng.voltage_at(sq.cv()).unwrap_or(0.0));
        }
        // Cut the run into CV plateaus: one per step.
        let mut steps: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < n {
            let v0 = cv[i];
            let mut j = i;
            while j < n && (cv[j] - v0).abs() < 0.02 {
                j += 1;
            }
            if j - i > 4_000 {
                steps.push((i, j));
            }
            i = j.max(i + 1);
        }
        assert!(
            steps.len() >= 8,
            "only found {} steps in 3.2 s -- is the clock running?",
            steps.len()
        );
        // A minor: A3 C4 E4, repeating.
        const RIFF: [f64; 3] = [220.0, 261.626, 329.628];
        // Which step of the bar the first plateau is depends on where the
        // settling run stopped, so lock the phase onto the lowest note.
        let f0 = pitch_of(&osc[steps[0].0..steps[0].1]);
        let phase = (0..RIFF.len())
            .min_by(|a, b| {
                let d = |k: &usize| (f0 / RIFF[*k]).log2().abs();
                d(a).partial_cmp(&d(b)).unwrap()
            })
            .unwrap();
        for (k, (a, b)) in steps.iter().enumerate().take(8) {
            let want = RIFF[(phase + k) % RIFF.len()];
            let got = pitch_of(&osc[*a..*b]);
            let cents = 1200.0 * (got / want).log2();
            assert!(
                cents.abs() < 25.0,
                "step {k} played {got:.2} Hz, {cents:+.0} cents from {want:.2} Hz"
            );
            // Every step must be long enough to hear.
            let ms = (*b - *a) as f64 * DT * 1000.0;
            assert!((150.0..320.0).contains(&ms), "step {k} lasted {ms:.0} ms");
        }
    }

    /// Fundamental frequency from upward zero crossings, interpolated.
    fn pitch_of(v: &[f64]) -> f64 {
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let (mut first, mut last, mut n) = (None, 0.0, 0u32);
        for i in 1..v.len() {
            if v[i - 1] <= mean && v[i] > mean {
                let frac = (mean - v[i - 1]) / (v[i] - v[i - 1]);
                let t = (i - 1) as f64 * DT + frac * DT;
                match first {
                    None => first = Some(t),
                    Some(_) => {
                        n += 1;
                        last = t;
                    }
                }
            }
        }
        match first {
            Some(f) if n > 0 => n as f64 / (last - f),
            _ => 0.0,
        }
    }

    /// The same contract the showcase signs: a demo is not a trap. Every
    /// switch closed and every pot wound to the end that dissipates most,
    /// and nothing may settle at its failure temperature. An 8 ohm speaker
    /// passes its 0.5 W rating at 2 V rms, which is why the voice's level is
    /// set by a fixed resistor rather than by a knob.
    #[test]
    fn the_synth_room_never_cooks_itself() {
        let mut elems = synth::synth_room_circuit();
        ensure_fixture(&mut elems, SYNTH_HOIST_RECT);
        for e in elems.iter_mut() {
            if reserved_id(e.id) {
                continue;
            }
            match &mut e.kind {
                K::Switch { closed } | K::Button { closed } => *closed = true,
                K::Potentiometer { wiper: w, .. } => *w = 0.98,
                _ => {}
            }
        }
        let mut run = DamageRun::new(&elems);
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
        assert!(run.broke.is_empty(), "the synth room broke {:?}", run.broke);
        let worst = sum
            .iter()
            .map(|(id, s, n)| (*id, s / *n as f64))
            .fold((0u32, 0.0f64), |m, e| if e.1 > m.1 { e } else { m });
        assert!(
            worst.1 < 0.9,
            "part #{} settles at {:.2} of its failure temperature",
            worst.0,
            worst.1
        );
        eprintln!(
            "synth worst case: part #{} settles at {:.2} of failure",
            worst.0, worst.1
        );
    }

    /// Every knob and toggle a player can reach, driven live: the pitch pots,
    /// the beat toggles, the tempo, the cutoff and the snare tone. None of it
    /// may quarantine the room, and the pitch row must actually move.
    #[test]
    fn synth_room_knobs_are_playable() {
        let elems = synth::synth_room_circuit();
        let ids = synth::seq_ids();
        let sq = synth::seq_config();
        let mut eng = Engine::new(DT);
        eng.set_elements(&elems);
        eng.advance(60_000);
        let mut seen: Vec<f64> = Vec::new();
        for k in 0..60u32 {
            // Wind step 1's pitch across its whole legal travel.
            let w = 0.01 + 0.98 * (k as f64 / 59.0);
            eng.interact(ids.pots[0], InteractOp::SetValue { value: w });
            eng.interact(
                ids.tempo,
                InteractOp::SetValue {
                    value: 0.2 + 0.01 * k as f64,
                },
            );
            eng.interact(synth::ID_CUTOFF, InteractOp::SetValue { value: w });
            eng.interact(synth::ID_SNARE_TONE, InteractOp::SetValue { value: w });
            for (n, sw) in ids.switches.iter().take(ids.steps).enumerate() {
                eng.interact(
                    *sw,
                    InteractOp::SetSwitch {
                        closed: (k as usize + n) % 2 == 0,
                    },
                );
            }
            eng.advance(2_000);
            assert!(
                !eng.is_quarantined(),
                "quarantined after knob update {k} (wiper {w:.3})"
            );
            seen.push(eng.voltage_at(sq.cv()).unwrap_or(0.0));
        }
        let lo = seen.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = seen.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            hi - lo > 2.0,
            "the pitch knob only moved the CV bus {:.2} V",
            hi - lo
        );
    }

    /// The panels are the only words in the world -- without them a player
    /// sees eighty anonymous glyphs -- so they must be well formed and must
    /// name the parts a player is looking for.
    #[test]
    fn the_synth_room_is_labelled() {
        let panels = synth::synth_panels();
        assert!(panels.len() >= 2 * synth::SEQ_STEPS + 5 && panels.len() <= MAX_PANELS);
        for p in &panels {
            assert!(
                p.x1 - p.x0 >= MIN_PANEL_SPAN,
                "panel {} is too narrow",
                p.name
            );
            assert!(
                p.y1 - p.y0 >= MIN_PANEL_SPAN,
                "panel {} is too short",
                p.name
            );
            assert!(
                p.name.len() <= MAX_PANEL_NAME,
                "panel name {:?} is too long",
                p.name
            );
        }
        let names: Vec<&str> = panels.iter().map(|p| p.name).collect();
        for want in ["VCO  1V/OCT", "SNARE  (TONE)", "STEP 1 PITCH", "BEAT 1"] {
            assert!(names.contains(&want), "no panel named {want:?}");
        }
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

    // ------------------------------------------------- the placement gate
    //
    // Each test below is one measured breaker repro replayed through the
    // exact candidate pipeline the sim task runs: syntactic apply on a
    // clone (`apply_doc_op_to`), then the electrical gate
    // (`check_room_doc`) — refusal means the live netlist never sees it.

    /// The full room a fresh server stands up (showcase + hoist fixture).
    fn full_room() -> Vec<ElementSpec> {
        let mut elems = demo_room_circuit();
        elems.extend(hoist_fixture());
        elems
    }

    /// Apply `op` to a candidate copy of `elems` and run the gate,
    /// asserting the syntactic half accepted it (these repros are all
    /// well-formed ops — that is the point).
    fn gate(elems: &[ElementSpec], op: &DocOp) -> Result<(), sim_core::Reject> {
        let mut next = elems.to_vec();
        assert!(
            apply_doc_op_to(&mut next, op),
            "op must be syntactically fine: {op:?}"
        );
        check_room_doc(&next)
    }

    /// The room every player actually joins must pass the gate — including
    /// the worst case with every switch and button closed — or nothing
    /// could ever be placed again.
    #[test]
    fn the_shipped_room_passes_the_gate() {
        assert_eq!(check_room_doc(&full_room()), Ok(()));
    }

    #[test]
    fn gate_refuses_each_breaker_repro_class() {
        use sim_core::Reject;
        let room = full_room();
        let add = |id: u32, kind: K, a: (i32, i32), b: (i32, i32)| DocOp::Add {
            spec: spec(id, kind, a, b),
        };
        let cases: Vec<(&str, DocOp, fn(&Reject) -> bool)> = vec![
            (
                "wire shorting the vignette-A battery",
                add(5001, K::Wire, (2, 2), (2, 8)),
                |r| *r == Reject::ShortedSource { id: 1 },
            ),
            // NOTE: "second battery stacked on the first, AGREEING" is no
            // longer here. It is now ACCEPTED — see
            // `gate_accepts_a_matching_supply_in_parallel` below. All 9 V
            // supplies are assumed to come from the same supply, so two of
            // them on one node are one net, not a singular matrix.
            (
                "second battery stacked on the first (disagreeing)",
                add(5003, dc(5.0), (2, 2), (2, 8)),
                |r| matches!(r, Reject::ConflictingSources { a: 1, b: 5003, .. }),
            ),
            (
                "battery across the hoist's LIM-BOT pair (closed at rest)",
                add(
                    5004,
                    dc(9.0),
                    fixture_pin(LIM_BOT_ID, 0),
                    fixture_pin(LIM_BOT_ID, 1),
                ),
                |r| matches!(r, Reject::ConflictingSources { b: 5004, .. }),
            ),
            (
                "battery across LIM-TOP: fine until the MACHINE closes it",
                add(
                    5005,
                    dc(9.0),
                    fixture_pin(LIM_TOP_ID, 0),
                    fixture_pin(LIM_TOP_ID, 1),
                ),
                |r| *r == Reject::UnsolvableWhenSwitched,
            ),
            (
                "wire across LIM-TOP (the measured self-locking deadlock)",
                add(
                    5006,
                    K::Wire,
                    fixture_pin(LIM_TOP_ID, 0),
                    fixture_pin(LIM_TOP_ID, 1),
                ),
                |r| *r == Reject::UnsolvableWhenSwitched,
            ),
            (
                "open switch across the battery, closable by any player",
                add(5007, K::Switch { closed: false }, (2, 2), (2, 8)),
                |r| *r == Reject::UnsolvableWhenSwitched,
            ),
            (
                "ground on the same point as a fresh rail",
                DocOp::Add {
                    spec: gnd(5008, (70, 70)),
                },
                |_| unreachable!("applied after the rail below"),
            ),
            (
                "zero-ohm resistor typed into the properties panel",
                DocOp::SetKind {
                    id: 55,
                    kind: r(0.0),
                },
                |r| matches!(r, Reject::BadValue { id: 55, .. }),
            ),
            (
                "negative resistance typed into the properties panel",
                DocOp::SetKind {
                    id: 55,
                    kind: r(-100.0),
                },
                |r| matches!(r, Reject::BadValue { id: 55, .. }),
            ),
            (
                "zero-henry inductor (NaN broadcaster)",
                DocOp::SetKind {
                    id: 55,
                    kind: K::Inductor { henries: 0.0 },
                },
                |r| matches!(r, Reject::BadValue { id: 55, .. }),
            ),
            (
                "1e300 V source (energy-meter/save-file destroyer)",
                DocOp::SetKind {
                    id: 1,
                    kind: dc(1e300),
                },
                |r| matches!(r, Reject::BadValue { id: 1, .. }),
            ),
            (
                "1e150 A current source dropped in empty space",
                add(5009, K::CurrentSource { amps: 1e150 }, (90, 90), (90, 96)),
                |r| matches!(r, Reject::BadValue { id: 5009, .. }),
            ),
            (
                "pin-drag collapsing the battery onto one point",
                DocOp::Move {
                    id: 1,
                    pins: vec![(2, 2), (2, 2)], rot: None },
                |r| *r == Reject::CollapsedPins { id: 1 },
            ),
        ];
        for (why, op, want) in cases {
            if why.starts_with("ground on the same point") {
                // Two-step repro: rail first (legal), then the ground.
                let mut with_rail = room.clone();
                with_rail.push(ElementSpec {
                    id: 5100,
                    kind: K::Rail {
                        dc: 12.0,
                        amp: 0.0,
                        hz: 0.0,
                        phase: 0.0,
                    },
                    pins: vec![(70, 70)],
                    tier: 0,
                    rot: 0,
                });
                assert_eq!(check_room_doc(&with_rail), Ok(()), "lone rail is legal");
                assert_eq!(
                    gate(&with_rail, &op),
                    Err(sim_core::Reject::ShortedSource { id: 5100 }),
                    "{why}"
                );
                continue;
            }
            let got = gate(&room, &op).expect_err(why);
            assert!(want(&got), "{why}: wrong reject {got:?}");
        }
        // And the room itself is untouched by all that refusing.
        assert_eq!(check_room_doc(&room), Ok(()));
    }

    /// The other half of the source rule, on the LIVE room: an identical
    /// supply in parallel is one net, not a refusal.
    ///
    /// "It's possible to connect two 5 V sources together not because they
    /// are the same voltage but because we make the assumption that all 5 V
    /// sources are from the same source."
    #[test]
    fn gate_accepts_a_matching_supply_in_parallel() {
        let room = full_room();
        // A second 9 V battery straight across the vignette-A battery.
        let op = DocOp::Add {
            spec: spec(5002, dc(9.0), (2, 2), (2, 8)),
        };
        assert_eq!(gate(&room, &op), Ok(()), "a matching supply is one net");
        // A third one, and the same supply drawn the other way round.
        let mut two = room.clone();
        assert!(apply_doc_op_to(&mut two, &op));
        assert_eq!(
            gate(
                &two,
                &DocOp::Add {
                    spec: spec(5003, dc(-9.0), (2, 8), (2, 2)),
                }
            ),
            Ok(()),
            "the same constraint drawn backwards is still one net"
        );
        // Two closed switches in parallel across the hoist's LIM-BOT pair —
        // two-way lighting / an OR contact / a manual override. A closed
        // switch is a 0 V source, so this is the identical singularity as
        // parallel batteries and used to be refused.
        assert_eq!(
            gate(
                &room,
                &DocOp::Add {
                    spec: spec(5004, K::Switch { closed: true }, (57, 22), (61, 22)),
                }
            ),
            Ok(()),
            "a second closed switch in parallel is one net"
        );
    }

    /// The gate must NOT refuse ordinary building: every legal op class on
    /// the live room, including wiring to the hoist terminals (the intended
    /// use) and the mid-build states the breakers measured as solvable.
    #[test]
    fn gate_accepts_ordinary_building() {
        let room = full_room();
        let legal = vec![
            // A floating battery in empty space: normal mid-build.
            DocOp::Add {
                spec: spec(6001, dc(5.0), (70, 70), (70, 76)),
            },
            // A dangling current source: normal mid-build (GMIN-solvable).
            DocOp::Add {
                spec: spec(6002, K::CurrentSource { amps: 1.0 }, (72, 70), (72, 76)),
            },
            // A rail straight onto the motor's M+ terminal, ground on M-:
            // the canonical hoist drive.
            DocOp::Add {
                spec: ElementSpec {
                    id: 6003,
                    kind: K::Rail {
                        dc: 5.0,
                        amp: 0.0,
                        hz: 0.0,
                        phase: 0.0,
                    },
                    // Derived from the fixture, not hardcoded: the chip's
                    // terminals move with its package.
                    pins: vec![fixture_pin(MOTOR_ID, 0)],
                    tier: 0,
                    rot: 0,
                },
            },
            DocOp::Add {
                spec: gnd(6004, fixture_pin(MOTOR_ID, 1)),
            },
            // Ordinary properties edit and pin drag.
            DocOp::SetKind {
                id: 55,
                kind: r(470.0),
            },
            DocOp::Move {
                id: 55,
                pins: vec![(30, 14), (33, 14)], rot: None },
        ];
        let mut doc = room;
        for op in legal {
            let mut next = doc.clone();
            assert!(apply_doc_op_to(&mut next, &op), "syntactic: {op:?}");
            assert_eq!(check_room_doc(&next), Ok(()), "gate must accept {op:?}");
            doc = next; // ops build on each other, like a real session
        }
    }

    /// The one class of breakage the GAME inflicts on a document the gate
    /// blessed: `machine_step` writes LIM-TOP/LIM-BOT into the live engine
    /// every 640 us with nothing in front of it, and `write_param` never
    /// clears quarantine. A player wiring an LED in series with LIM-TOP used
    /// to be told the placement was fine, and then the crate froze the room
    /// on its way up — with no player action able to undo it.
    ///
    /// The fix is at placement time, which is where the owner wants it: the
    /// gate now runs its convergence trial on the all-switches-closed clone
    /// it was already factoring, so a circuit that only diverges once a
    /// limit switch closes is refused before it can be placed.
    #[test]
    fn gate_refuses_a_circuit_the_machine_would_break_by_closing_a_limit() {
        let room = full_room();
        // LIM-TOP's terminals, derived from the fixture rather than written
        // down: the chip re-laid the package out, and a hardcoded pair here
        // would silently test two empty grid points.
        let (top_a, top_b) = (fixture_pin(LIM_TOP_ID, 0), fixture_pin(LIM_TOP_ID, 1));
        // 9 V onto one side of LIM-TOP, an LED from the other side back to
        // ground: two dangling half-circuits while the crate is down, and an
        // ideal source straight across the LED the moment it reaches the top.
        for source in [
            dc(9.0),
            K::VoltageSource {
                dc: 0.0,
                amp: 9.0,
                hz: 50.0,
                phase: 0.0,
            },
        ] {
            let mut next = room.clone();
            next.push(spec(7101, source, top_a, (70, 46)));
            next.push(spec(7102, K::Led { color: 0 }, top_b, (70, 46)));
            next.push(gnd(7103, (70, 46)));
            // Nothing structural to say about it: it factors as placed AND
            // with every switch closed. Only the trial catches it.
            let got = check_room_doc(&next).expect_err("must be refused");
            assert!(
                matches!(got, sim_core::Reject::WillNotConverge { id: Some(7102) }),
                "{got:?}"
            );
        }
        // The same shape with the series resistor the hint asks for stays
        // placeable — this must refuse landmines, not LEDs.
        let mut next = room.clone();
        next.push(spec(7101, dc(9.0), top_a, (70, 46)));
        next.push(spec(7102, r(330.0), top_b, (70, 43)));
        next.push(spec(7104, K::Led { color: 0 }, (70, 43), (70, 46)));
        next.push(gnd(7103, (70, 46)));
        assert_eq!(check_room_doc(&next), Ok(()));
    }

    /// The machine-move path: dragging the package so its closed LIM-BOT
    /// switch lands exactly on a player's battery terminals must be refused
    /// by the same gate (this path never passes through `apply_doc_op`).
    #[test]
    fn gate_refuses_machine_move_landing_on_a_source() {
        let mut room = full_room();
        // The measured repro, aimed at where LIM-BOT WOULD land after a
        // (23, 8) drag rather than at a hardcoded coordinate — so the repro
        // follows the package instead of quietly aiming at empty grid.
        const D: (i32, i32) = (23, 8);
        let a = fixture_pin(LIM_BOT_ID, 0);
        let b = fixture_pin(LIM_BOT_ID, 1);
        let a = (a.0 + D.0, a.1 + D.1);
        let b = (b.0 + D.0, b.1 + D.1);
        room.push(spec(7001, dc(9.0), a, b));
        room.push(spec(7002, r(100.0), a, b));
        room.push(gnd(7003, b));
        assert_eq!(check_room_doc(&room), Ok(()));

        // The drag parks the closed LIM-BOT switch straight across them.
        let mut next = room.clone();
        let mut rect = HOIST_RECT;
        let moved = move_machine(&mut next, &mut rect, D.0, D.1);
        assert!(moved.is_some(), "the move itself is well-formed");
        // Refused, and NAMED: the limit switch is a 0 V constraint and the
        // battery a 9 V one, on the same node pair.
        let got = check_room_doc(&next).expect_err("landing LIM-BOT on a source must be refused");
        assert!(
            matches!(got, sim_core::Reject::ConflictingSources { .. }),
            "{got:?}"
        );
        let ids: Vec<u32> = got.ids().iter().collect();
        assert!(ids.contains(&903) && ids.contains(&7001), "{ids:?}");
        // A harmless drag of the same distance elsewhere stays legal.
        let mut next = room.clone();
        let mut rect = HOIST_RECT;
        assert!(move_machine(&mut next, &mut rect, -23, 8).is_some());
        assert_eq!(check_room_doc(&next), Ok(()));
    }

    /// The interact path: a switch flip that would short a source (possible
    /// on documents predating the gate) and an out-of-range knob write are
    /// both caught on the candidate copy.
    #[test]
    fn gate_refuses_poison_interacts() {
        // A pre-gate document: open switch straight across the battery.
        let mut room = full_room();
        room.push(spec(8001, K::Switch { closed: false }, (2, 2), (2, 8)));
        // (The gate would have refused this Add; simulate a legacy doc.)
        let mut closed = room.clone();
        apply_interact_to(&mut closed, 8001, InteractOp::SetSwitch { closed: true });
        let got = check_room_doc(&closed).expect_err("closing the shorting switch must be refused");
        assert!(
            matches!(
                got,
                sim_core::Reject::ConflictingSources { a: 1, b: 8001, .. }
            ),
            "{got:?}"
        );

        // Knob write pushing a source to an absurd value: SetValue on dc
        // had no clamp at all before the gate.
        let room = full_room();
        let mut next = room.clone();
        apply_interact_to(&mut next, 1, InteractOp::SetValue { value: 1e300 });
        assert!(
            matches!(
                check_room_doc(&next),
                Err(sim_core::Reject::BadValue { id: 1, .. })
            ),
            "1e300 V through the knob path must be refused"
        );
        // An ordinary knob write sails through.
        let mut next = room.clone();
        apply_interact_to(&mut next, 1, InteractOp::SetValue { value: 12.0 });
        assert_eq!(check_room_doc(&next), Ok(()));
    }

    /// The reject broadcast is well-formed and machine-readable.
    #[test]
    fn reject_msg_is_machine_readable() {
        let r = sim_core::Reject::UnsolvableWhenSwitched;
        let v: serde_json::Value = serde_json::from_str(&reject_msg(3, "edit", &r)).unwrap();
        assert_eq!(v["t"], "reject");
        assert_eq!(v["who"], 3);
        assert_eq!(v["ctx"], "edit");
        assert_eq!(v["code"], "unsolvable_switched");
        assert!(v["id"].is_null());
        assert_eq!(v["ids"].as_array().unwrap().len(), 0);
        assert!(v["hint"].as_str().is_some_and(|h| !h.is_empty()));

        // A named refusal carries EVERY implicated part, so the client can
        // flash both halves of a conflict rather than guessing.
        let r = sim_core::Reject::ConflictingSources {
            a: 1,
            b: 5003,
            va: 9.0,
            vb: 5.0,
        };
        let v: serde_json::Value = serde_json::from_str(&reject_msg(3, "edit", &r)).unwrap();
        assert_eq!(v["code"], "conflicting_sources");
        assert_eq!(v["id"], 1);
        assert_eq!(v["ids"], serde_json::json!([1, 5003]));
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("9 V") && hint.contains("5 V"), "{hint}");
    }
}
