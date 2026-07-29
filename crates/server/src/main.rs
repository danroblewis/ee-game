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
    ]
}

enum Cmd {
    Interact { id: u32, op: InteractOp },
    Edit { op: DocOp },
    Join,
    Leave,
}

struct Room {
    cmds: mpsc::UnboundedSender<Cmd>,
    events: broadcast::Sender<String>,
    /// Element specs kept in sync with applied ops, for `hello` on join.
    elements: std::sync::Mutex<Vec<ElementSpec>>,
    next_client: AtomicU32,
    population: AtomicU32,
}

/// The sim task: sole owner of the Engine. Ops apply between ticks —
/// the "tick boundary" rule from the plan, at demo scale.
async fn sim_task(room: Arc<Room>, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
    let mut eng = Engine::new(DT);
    eng.set_elements(&demo_room_circuit());

    let tick = std::time::Duration::from_secs_f64(1.0 / TICK_HZ);
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let steps_per_tick = ((1.0 / TICK_HZ) / DT).round() as u32;

    loop {
        interval.tick().await;

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
                        let elems = room.elements.lock().unwrap().clone();
                        eng.set_elements(&elems); // continuous state survives by id
                        let _ = room.events.send(json!({"t": "doc", "op": op}).to_string());
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

        eng.advance(steps_per_tick.min(MAX_STEPS_PER_TICK));

        if room.events.receiver_count() > 0 {
            // Same flat layout as the WASM facade:
            // [id, npins, v0, v1, v2, i0, i1, i2, power].
            let e: Vec<[f64; 9]> = eng
                .frame()
                .iter()
                .map(|f| {
                    [
                        f.id as f64,
                        f.npins as f64,
                        f.v[0],
                        f.v[1],
                        f.v[2],
                        f.i[0],
                        f.i[1],
                        f.i[2],
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
    }
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum ClientMsg {
    Interact { id: u32, op: InteractOp },
    Edit { op: DocOp },
    Cursor { x: f64, y: f64 },
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
        json!({"t": "hello", "you": me, "elements": *elems}).to_string()
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
    let room = Arc::new(Room {
        cmds: cmd_tx,
        events: event_tx,
        elements: std::sync::Mutex::new(demo_room_circuit()),
        next_client: AtomicU32::new(1),
        population: AtomicU32::new(0),
    });
    tokio::spawn(sim_task(room.clone(), cmd_rx));

    let dist = std::env::var("EE_DIST").unwrap_or_else(|_| "packages/app/dist".into());
    let static_files =
        ServeDir::new(&dist).not_found_service(ServeFile::new(format!("{dist}/index.html")));

    let app = Router::new()
        .route("/ws", get(ws_handler))
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
