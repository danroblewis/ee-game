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
use sim_core::{ElementSpec, Engine, InteractOp};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::{broadcast, mpsc};
use tower_http::services::{ServeDir, ServeFile};

const DT: f64 = 10e-6;
const TICK_HZ: f64 = 30.0;
/// Wall budget per tick: cap sim work so a heavy circuit dilates sim time
/// instead of stalling the loop (Falstad's rule, plan resolution).
const MAX_STEPS_PER_TICK: u32 = 8000;

/// The show-off circuit: two loops sharing a ground rail.
/// Top: 9 V battery -> switch -> lamp. Bottom: slow 1.5 Hz sine breathing
/// through a lamp, so voltage colors and dots visibly oscillate.
fn demo_room_circuit() -> Vec<ElementSpec> {
    use sim_core::ElementKind as K;
    use sim_golden::{dc, sine, spec};
    vec![
        // top loop
        spec(1, dc(9.0), (2, 2), (2, 8)),
        spec(2, K::Wire, (2, 2), (7, 2)),
        spec(3, K::Switch { closed: false }, (7, 2), (11, 2)),
        spec(4, K::Wire, (11, 2), (16, 2)),
        spec(
            5,
            K::Lamp {
                ohms: 90.0,
                rated_watts: 1.0,
            },
            (16, 2),
            (16, 8),
        ),
        spec(6, K::Wire, (16, 8), (9, 8)),
        spec(7, K::Ground, (9, 8), (9, 8)),
        spec(8, K::Wire, (9, 8), (2, 8)),
        // bottom loop
        spec(10, sine(6.0, 1.5), (2, 12), (2, 18)),
        spec(11, K::Wire, (2, 12), (9, 12)),
        spec(
            12,
            K::Lamp {
                ohms: 40.0,
                rated_watts: 0.5,
            },
            (9, 12),
            (16, 12),
        ),
        spec(13, K::Wire, (16, 12), (16, 18)),
        spec(14, K::Wire, (16, 18), (9, 18)),
        spec(15, K::Ground, (9, 18), (9, 18)),
        spec(16, K::Wire, (9, 18), (2, 18)),
    ]
}

enum Cmd {
    Interact { id: u32, op: InteractOp },
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
            let e: Vec<[f64; 5]> = eng
                .frame()
                .iter()
                .map(|f| [f.id as f64, f.va, f.vb, f.current, f.power])
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
        _ => {}
    }
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum ClientMsg {
    Interact { id: u32, op: InteractOp },
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
