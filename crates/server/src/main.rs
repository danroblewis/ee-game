//! Room server: one authoritative simulation, many browsers.
//!
//! M4-lite protocol (JSON over WebSocket, upgraded to the three-class
//! binary transport in M4/M5):
//!   server -> client: hello{you, elements}, frame{time, e}, op{id, op},
//!                     presence{n}, cursor{who, x, y}
//!   client -> server: interact{id, op}, cursor{x, y}

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;
use serde_json::json;
use sim_core::{DocOp, ElementSpec, Engine, InteractOp};
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
    ]
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

fn checkpoint(room: &Room) {
    let save = SaveFile {
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
async fn sim_task(room: Arc<Room>, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
    let mut eng = Engine::new(DT);
    {
        let elems = room.elements.lock().unwrap().clone();
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
            checkpoint(&room);
        }

        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Cmd::Interact { id, op } => {
                    eng.interact(id, op);
                    apply_to_specs(&room, id, op);
                    let _ = room
                        .events
                        .send(json!({"t": "op", "id": id, "op": op}).to_string());
                }
                Cmd::Edit { op } => {
                    if apply_doc_op(&room, &op) {
                        room.dirty.store(true, Ordering::Relaxed);
                        let elems = room.elements.lock().unwrap().clone();
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

        // Advance the tick — chunked when probes exist so waveforms are
        // sampled between substeps, not once per tick.
        let probes = room.probes.lock().unwrap().clone();
        let budget = steps_per_tick.min(MAX_STEPS_PER_TICK);
        if probes.is_empty() {
            eng.advance(budget);
        } else {
            let t0 = eng.time();
            let chunks = (budget / SAMPLE_EVERY).max(1);
            let mut bufs: Vec<Vec<f32>> = vec![Vec::with_capacity(chunks as usize); probes.len()];
            for _ in 0..chunks {
                eng.advance(SAMPLE_EVERY);
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
                        (ProbeKind::I, Some(fr)) => {
                            fr.iter().find(|f| f.id == p.elem).map(|f| f.i[p.pin])
                        }
                        (ProbeKind::I, None) => eng.pin_current(p.elem, p.pin),
                    };
                    buf.push(v.unwrap_or(0.0) as f32);
                }
            }
            if room.events.receiver_count() > 0 {
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
        }

        if room.events.receiver_count() > 0 {
            // Same flat layout as the WASM facade:
            // [id, npins, v0..v5, i0..i5, power].
            let e: Vec<[f64; 15]> = eng
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
                .send(json!({"t": "frame", "time": eng.time(), "e": e}).to_string());
        }
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
/// (malformed or unknown id) — the full permission/rules pipeline is M4.
fn apply_doc_op(room: &Room, op: &DocOp) -> bool {
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
    let (elements, probes, next_pid) = match saved {
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
            (s.elements, probes, s.next_pid.max(1))
        }
        None => (demo_room_circuit(), Vec::new(), 1),
    };

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
    tokio::spawn(sim_task(room.clone(), cmd_rx));

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
}
