//! The lobby: plain HTTP under `/api` for the room lifecycle.
//!
//! Deliberately NOT websocket ops. The lobby exists BEFORE you have a room
//! socket ("which rooms are there?" is the question you ask when you have no
//! room), and keeping it off the wire protocol leaves `ClientMsg`/`Cmd`
//! untouched.
//!
//!   GET    /api/rooms                 list rooms + the default one
//!   POST   /api/rooms                 {name, template} -> create
//!   PATCH  /api/rooms/{code}          {name} -> rename (broadcasts roommeta)
//!   DELETE /api/rooms/{code}          evict: players told, file removed
//!   GET    /api/templates             built-ins + $EE_TEMPLATES/*.json
//!   POST   /api/templates             {from, id, name, blurb, view} -> save
//!                                     a running room AS a template
//!   DELETE /api/templates/{id}        remove a file template (never a builtin)
//!
//! If `EE_ADMIN_TOKEN` is set, every mutating call must carry it in
//! `X-EE-Token`. Unset (the default) leaves the lobby open, which is what a
//! LAN game wants; real auth is M4's session work.

use crate::registry::{CreateErr, Registry};
use crate::templates::{self, View};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Reply = (StatusCode, Json<Value>);

fn err(code: StatusCode, e: &str, hint: &str) -> Reply {
    (code, Json(json!({"error": e, "hint": hint})))
}

fn authed(headers: &HeaderMap) -> bool {
    match std::env::var("EE_ADMIN_TOKEN") {
        Err(_) => true,
        Ok(want) if want.is_empty() => true,
        Ok(want) => headers
            .get("x-ee-token")
            .and_then(|v| v.to_str().ok())
            .map(|got| got == want)
            .unwrap_or(false),
    }
}

pub fn routes() -> Router<Arc<Registry>> {
    Router::new()
        .route("/api/rooms", get(list_rooms).post(create_room))
        .route(
            "/api/rooms/{code}",
            axum::routing::patch(rename_room).delete(delete_room),
        )
        .route("/api/templates", get(list_templates).post(save_template))
        .route("/api/templates/{id}", delete(delete_template))
}

async fn list_rooms(State(reg): State<Arc<Registry>>) -> impl IntoResponse {
    let def = reg.default_room().map(|h| h.meta().id);
    (
        StatusCode::OK,
        Json(json!({"rooms": reg.list(), "default": def})),
    )
}

#[derive(Deserialize)]
struct CreateBody {
    #[serde(default)]
    name: String,
    #[serde(default)]
    template: String,
}

async fn create_room(
    State(reg): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Reply {
    if !authed(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden", "bad admin token");
    }
    let template = if body.template.trim().is_empty() {
        crate::default_template()
    } else {
        body.template.trim().to_string()
    };
    let name = if body.name.trim().is_empty() {
        // A room the player did not name still needs a name they can read.
        templates::list(&reg.tdir)
            .into_iter()
            .find(|t| t.id == template)
            .map(|t| t.name)
            .unwrap_or_else(|| "New Room".into())
    } else {
        body.name.clone()
    };
    match reg.create(&name, &template) {
        Ok(h) => (StatusCode::CREATED, Json(json!({"room": room_json(&h)}))),
        Err(CreateErr::BadName) => err(StatusCode::BAD_REQUEST, "badname", "1-40 printable chars"),
        Err(CreateErr::NoTemplate) => err(
            StatusCode::NOT_FOUND,
            "notemplate",
            "no such template — GET /api/templates",
        ),
        Err(CreateErr::BadTemplate(why)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "badtemplate", "code": why,
                        "hint": "the template does not describe a valid room"})),
        ),
        Err(CreateErr::TooMany) => err(
            StatusCode::CONFLICT,
            "toomany",
            "this server is at its room limit — delete one first",
        ),
        Err(CreateErr::Io) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io",
            "could not write the room file",
        ),
    }
}

fn room_json(h: &Arc<crate::registry::RoomHandle>) -> Value {
    let m = h.meta();
    json!({
        "id": m.id, "name": m.name, "template": m.template,
        "parts": h.room.elements.lock().unwrap().len(),
        "players": h.players(), "live": h.is_live(), "machine": h.has_machine,
        "created": m.created, "played": m.played,
    })
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_room(
    State(reg): State<Arc<Registry>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Reply {
    if !authed(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden", "bad admin token");
    }
    let code = code.to_ascii_uppercase();
    if reg.get(&code).is_none() {
        return err(StatusCode::NOT_FOUND, "noroom", "no such room");
    }
    match reg.rename(&code, &body.name) {
        Some(h) => (StatusCode::OK, Json(json!({"room": room_json(&h)}))),
        None => err(StatusCode::BAD_REQUEST, "badname", "1-40 printable chars"),
    }
}

async fn delete_room(
    State(reg): State<Arc<Registry>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Reply {
    if !authed(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden", "bad admin token");
    }
    let code = code.to_ascii_uppercase();
    if reg.delete(&code) {
        (StatusCode::OK, Json(json!({"deleted": code})))
    } else {
        err(StatusCode::NOT_FOUND, "noroom", "no such room")
    }
}

async fn list_templates(State(reg): State<Arc<Registry>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"templates": templates::list(&reg.tdir)})),
    )
}

#[derive(Deserialize)]
struct SaveTemplateBody {
    /// Room code to snapshot.
    from: String,
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    blurb: String,
    /// The SAVING CLIENT's camera and in-place scopes — client state, so it
    /// travels in the request rather than being read off the room.
    #[serde(default)]
    view: Option<View>,
    #[serde(default)]
    overwrite: bool,
}

async fn save_template(
    State(reg): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(body): Json<SaveTemplateBody>,
) -> Reply {
    if !authed(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden", "bad admin token");
    }
    let id = body.id.trim().to_ascii_lowercase();
    if !templates::valid_id(&id) {
        return err(
            StatusCode::BAD_REQUEST,
            "badid",
            "a-z, 0-9 and dashes, 1-32 chars",
        );
    }
    let already = templates::file_exists(&reg.tdir, &id);
    if !body.overwrite && already {
        return err(
            StatusCode::CONFLICT,
            "exists",
            "a template with that id already exists",
        );
    }
    if !already && templates::file_count(&reg.tdir) >= templates::MAX_FILE_TEMPLATES {
        return err(
            StatusCode::CONFLICT,
            "toomany",
            "this server is at its saved-template limit — delete one first",
        );
    }
    let Some(h) = reg.get(&body.from.to_ascii_uppercase()) else {
        return err(StatusCode::NOT_FOUND, "noroom", "no such room");
    };
    let setup = match h.as_template_setup(body.view.clone()).normalize() {
        Ok(s) => s,
        Err(why) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "badroom", "code": why})),
            )
        }
    };
    let name = if body.name.trim().is_empty() {
        h.meta().name
    } else {
        body.name.trim().to_string()
    };
    if let Err(e) = templates::write(&reg.tdir, &id, &name, body.blurb.trim(), &setup) {
        tracing::error!("template write failed: {e}");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io",
            "could not write the template",
        );
    }
    let info = templates::list(&reg.tdir)
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    (StatusCode::CREATED, Json(json!({"template": info})))
}

async fn delete_template(
    State(reg): State<Arc<Registry>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Reply {
    if !authed(&headers) {
        return err(StatusCode::FORBIDDEN, "forbidden", "bad admin token");
    }
    let id = id.to_ascii_lowercase();
    if !templates::valid_id(&id) {
        return err(StatusCode::BAD_REQUEST, "badid", "bad template id");
    }
    if !templates::file_exists(&reg.tdir, &id) {
        return if templates::is_builtin(&id) {
            err(
                StatusCode::FORBIDDEN,
                "builtin",
                "built-in templates ship with the server",
            )
        } else {
            err(StatusCode::NOT_FOUND, "notemplate", "no such template")
        };
    }
    match templates::delete(&reg.tdir, &id) {
        Ok(()) => (StatusCode::OK, Json(json!({"deleted": id}))),
        Err(_) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io",
            "could not remove the template",
        ),
    }
}
