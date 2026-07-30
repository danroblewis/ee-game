//! Room server: one authoritative simulation, many browsers.
//!
//! M4-lite protocol (JSON over WebSocket, upgraded to the three-class
//! binary transport in M4/M5):
//!   server -> client: hello{you, elements}, frame{time, e}, op{id, op},
//!                     presence{n}, cursor{who, x, y}, machine{...}
//!   client -> server: interact{id, op}, cursor{x, y}, machinereset{}

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
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

/// The showcase room: four vignettes on one shared simulation.
///   A: battery -> switch -> lamp (click me)
///   B: potentiometer -> NPN emitter follower dimming a lamp (drag me)
///   C: slow sine gate on an NMOS switching a lamp, cap softening the edges
///   D: op-amp comparator on a slow sine alternately blinking two LEDs
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

/// The hoist's footprint in GRID units, broadcast to clients as `rect`.
/// All hoist chrome is drawn inside it; the geometry lives here only.
const HOIST_RECT: [i32; 4] = [46, 2, 64, 24];

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

/// The fixture, laid out on the faceplate inside `HOIST_RECT`:
///   900 Motor         [M+, M-]
///   901 Potentiometer [SENSE-A, SENSE-W, SENSE-B]  (the position sensor)
///   902 Switch        [LIM-TOP-a, LIM-TOP-b]
///   903 Switch        [LIM-BOT-a, LIM-BOT-b]
/// The shaft itself takes the left of the rect; the terminal column sits on
/// the right so wires can reach it without crossing the crate.
fn hoist_fixture() -> Vec<ElementSpec> {
    let [x0, y0, ..] = HOIST_RECT;
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

/// Inject any missing fixture element — a checkpoint written before the
/// hoist existed has none of them, and they can never be removed after.
/// Persisted pins and values survive when the part is already the right kind
/// (a restored wiper or limit-switch position is real state).
fn ensure_fixture(elems: &mut Vec<ElementSpec>) {
    for spec in hoist_fixture() {
        match elems.iter_mut().find(|e| e.id == spec.id) {
            // A save written before ids 900-999 were reserved could hold a
            // player's part on a fixture id. The fixture wins: the machine
            // would otherwise be writing back-EMF into someone's resistor.
            Some(e) if std::mem::discriminant(&e.kind) != std::mem::discriminant(&spec.kind) => {
                *e = spec;
            }
            Some(_) => {}
            None => elems.push(spec),
        }
    }
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
    /// Lower the crate to the floor and re-arm the hoist's goal.
    MachineReset,
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
    next_client: AtomicU32,
    next_pid: AtomicU32,
    population: AtomicU32,
    /// Set when the document changes; the sim task checkpoints to disk.
    dirty: std::sync::atomic::AtomicBool,
}

/// Room checkpoint: the document and probes survive server restarts (the
/// continuous electrical state re-settles within milliseconds).
#[derive(serde::Serialize, Deserialize)]
struct SaveFile {
    elements: Vec<ElementSpec>,
    #[serde(default)]
    probes: Vec<SavedProbe>,
    #[serde(default)]
    next_pid: u32,
    /// Mechanical state of the hoist. Defaulted so saves written before the
    /// hoist existed still load (crate on the floor, goal armed).
    #[serde(default)]
    hoist: Hoist,
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

fn checkpoint(room: &Room, hoist: &Hoist) {
    let save = SaveFile {
        hoist: *hoist,
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
    };
    if let Ok(json) = serde_json::to_string(&save) {
        let path = save_path();
        let tmp = format!("{path}.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// The sim task: sole owner of the Engine. Ops apply between ticks —
/// the "tick boundary" rule from the plan, at demo scale.
async fn sim_task(room: Arc<Room>, mut cmds: mpsc::UnboundedReceiver<Cmd>, mut hoist: Hoist) {
    let mut eng = Engine::new(DT);
    let mut sources;
    {
        let elems = room.elements.lock().unwrap().clone();
        sources = source_ids(&elems);
        eng.set_elements(&elems);
    }

    let tick = std::time::Duration::from_secs_f64(1.0 / TICK_HZ);
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let steps_per_tick = ((1.0 / TICK_HZ) / DT).round() as u32;
    let mut ticks_since_save: u32 = 0;

    loop {
        interval.tick().await;

        // Checkpoint the document every ~5 s when it has changed.
        ticks_since_save += 1;
        if ticks_since_save >= 150 && room.dirty.swap(false, Ordering::Relaxed) {
            ticks_since_save = 0;
            checkpoint(&room, &hoist);
        }

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
                Cmd::Edit { op } => {
                    if apply_doc_op(&room, &op) {
                        room.dirty.store(true, Ordering::Relaxed);
                        let elems = room.elements.lock().unwrap().clone();
                        sources = source_ids(&elems);
                        eng.set_elements(&elems); // continuous state survives by id
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
                Cmd::Join | Cmd::Leave => {
                    let n = room.population.load(Ordering::Relaxed);
                    let _ = room
                        .events
                        .send(json!({"t": "presence", "n": n}).to_string());
                }
            }
        }

        // Advance the tick in machine-sized chunks: the mechanism co-simulates
        // every MACHINE_SUBSTEPS, and probes sample every SAMPLE_EVERY inside
        // each chunk (32 = 2 × 16, so waveforms keep their 3.125 kHz rate).
        let probes = room.probes.lock().unwrap().clone();
        let budget = steps_per_tick.min(MAX_STEPS_PER_TICK);
        let chunks = (budget / MACHINE_SUBSTEPS).max(1);
        let samples_per_chunk = (MACHINE_SUBSTEPS / SAMPLE_EVERY).max(1);
        let t0 = eng.time();
        let mut bufs: Vec<Vec<f32>> =
            vec![Vec::with_capacity((chunks * samples_per_chunk) as usize); probes.len()];
        let mut motor_i = eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0);
        let mut impact = 0.0f64;
        let mut writes: Option<machine::Writes> = None;
        let won_before = hoist.win;
        for _ in 0..chunks {
            if probes.is_empty() {
                eng.advance(MACHINE_SUBSTEPS);
            } else {
                for _ in 0..samples_per_chunk {
                    eng.advance(MACHINE_SUBSTEPS / samples_per_chunk);
                    sample_probes(&eng, &probes, &mut bufs);
                }
            }
            // A quarantined solver has no current to report, so the machine
            // freezes with it rather than coasting on stale numbers.
            if !eng.is_quarantined() {
                motor_i = eng.pin_current(MOTOR_ID, 0).unwrap_or(0.0);
                writes = Some(machine_step(&mut eng, &mut hoist, &sources));
                impact = impact.max(hoist.impact);
            }
        }
        if !probes.is_empty() && room.events.receiver_count() > 0 {
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

        if room.events.receiver_count() > 0 {
            // Same flat layout as the WASM facade:
            // [id, npins, v0..v3, i0..i3, power].
            let e: Vec<[f64; 11]> = eng
                .frame()
                .iter()
                .map(|f| {
                    [
                        f.id as f64,
                        f.npins as f64,
                        f.v[0],
                        f.v[1],
                        f.v[2],
                        f.v[3],
                        f.i[0],
                        f.i[1],
                        f.i[2],
                        f.i[3],
                        f.power,
                    ]
                })
                .collect();
            let _ = room
                .events
                .send(json!({"t": "frame", "time": eng.time(), "e": e}).to_string());

            // The hoist, once per tick alongside the frame.
            let _ = room
                .events
                .send(machine_msg(&hoist, motor_i, impact).to_string());
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
/// `rect` is the footprint in GRID units so the client can draw all hoist
/// chrome without hardcoding geometry; `impact` is non-zero only on the tick
/// a landing happened.
fn machine_msg(hoist: &Hoist, motor_i: f64, impact: f64) -> serde_json::Value {
    json!({
        "t": "machine",
        "id": MOTOR_ID,
        "rect": HOIST_RECT,
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
        (InteractOp::SetSwitch { closed }, K::Switch { closed: c }) => *c = closed,
        (InteractOp::SetValue { value }, K::Resistor { ohms })
        | (InteractOp::SetValue { value }, K::Lamp { ohms, .. }) => *ohms = value.max(1e-6),
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
                || elems.len() >= 2000
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
    Cursor {
        x: f64,
        y: f64,
    },
    /// Lower the crate and re-arm the hoist's goal.
    MachineReset,
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
        json!({"t": "hello", "you": me, "elements": *elems, "probes": *probes}).to_string()
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
                        Ok(ClientMsg::MachineReset) => {
                            let _ = room.cmds.send(Cmd::MachineReset);
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
    let (mut elements, probes, next_pid, hoist) = match saved {
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
            (s.elements, probes, s.next_pid.max(1), s.hoist)
        }
        None => (demo_room_circuit(), Vec::new(), 1, Hoist::default()),
    };
    // The hoist fixture is not optional: a room without it has no goal.
    ensure_fixture(&mut elements);

    let room = Arc::new(Room {
        cmds: cmd_tx,
        events: event_tx,
        elements: std::sync::Mutex::new(elements),
        probes: std::sync::Mutex::new(probes),
        next_client: AtomicU32::new(1),
        next_pid: AtomicU32::new(next_pid),
        population: AtomicU32::new(0),
        dirty: std::sync::atomic::AtomicBool::new(false),
    });
    tokio::spawn(sim_task(room.clone(), cmd_rx, hoist));

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

    /// The hoist fixture plus a player circuit, driven exactly the way
    /// `sim_task` drives it: MACHINE_SUBSTEPS of solver, then one machine
    /// tick that reads the motor's branch current and writes back.
    struct HoistRun {
        eng: Engine,
        hoist: Hoist,
        sources: Vec<u32>,
    }

    impl HoistRun {
        fn new(player_circuit: Vec<ElementSpec>) -> Self {
            let mut elems = hoist_fixture();
            elems.extend(player_circuit);
            let sources = source_ids(&elems);
            let mut eng = Engine::new(DT);
            eng.set_elements(&elems);
            HoistRun {
                eng,
                hoist: Hoist::default(),
                sources,
            }
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
            i
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

    #[test]
    fn comparator_feedback_holds_the_crate_in_the_band() {
        // The discovery this goal exists to force: close the loop.
        //   sensor pot (SENSE-A to +4 V, SENSE-B to ground) -> wiper voltage
        //   4·y/H, compared against a 3.2 V reference (= band centre 0.32 m).
        //   The op-amp comparator drives the motor: +5 V lifts, -5 V lowers.
        // Bang-bang, and the 3× hold drain is sized to let it win.
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
        ]);

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
        ensure_fixture(&mut elements);
        let room = Room {
            cmds: cmd_tx,
            events: event_tx,
            elements: std::sync::Mutex::new(elements),
            probes: std::sync::Mutex::new(Vec::new()),
            next_client: AtomicU32::new(1),
            next_pid: AtomicU32::new(1),
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
        let v = machine_msg(&hoist, 0.94, 1.75);
        assert_eq!(v["t"], "machine");
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
    fn old_saves_load_without_hoist_state() {
        let save: SaveFile = serde_json::from_str(r#"{"elements":[]}"#).unwrap();
        assert_eq!(save.hoist, Hoist::default());
        assert_eq!(save.hoist.y, 0.0);
        assert!(!save.hoist.win);
    }

    #[test]
    fn ensure_fixture_repairs_old_rooms() {
        // A pre-hoist save: no fixture at all.
        let mut elems = vec![spec(1, K::Wire, (0, 0), (0, 4))];
        ensure_fixture(&mut elems);
        assert_eq!(elems.len(), 5);
        // Idempotent, and persisted state survives a reload.
        if let Some(e) = elems.iter_mut().find(|e| e.id == SENSOR_ID) {
            e.kind = K::Potentiometer {
                ohms: SENSOR_OHMS,
                wiper: 0.25,
            };
        }
        ensure_fixture(&mut elems);
        assert_eq!(elems.len(), 5);
        let sensor = elems.iter().find(|e| e.id == SENSOR_ID).unwrap();
        assert!(matches!(
            sensor.kind,
            K::Potentiometer { wiper, .. } if wiper == 0.25
        ));
        // A save from before the ids were reserved, with a player's part
        // squatting on a fixture id: the fixture takes it back.
        let mut squatted = vec![spec(MOTOR_ID, K::Resistor { ohms: 100.0 }, (0, 0), (0, 4))];
        ensure_fixture(&mut squatted);
        let motor = squatted.iter().find(|e| e.id == MOTOR_ID).unwrap();
        assert!(matches!(motor.kind, K::Motor { .. }), "fixture must win");
        assert_eq!(motor.pins, motor_pins_vec());
    }

    fn motor_pins_vec() -> Vec<sim_core::Point> {
        let (a, b) = motor_pins();
        vec![a, b]
    }

    #[test]
    fn showcase_room_never_quarantines() {
        let mut eng = Engine::new(DT);
        eng.set_elements(&demo_room_circuit());
        // 30 simulated seconds in 10 ms chunks; the relaxation oscillator
        // must flip repeatedly and nothing may quarantine.
        let mut flips = 0;
        let mut last_sign = 0i32;
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
        }
        assert!(flips >= 10, "oscillator only flipped {flips} times in 30 s");
    }
}
