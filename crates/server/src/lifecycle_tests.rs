//! Room lifecycle: create from every template, join, park/resume, rename,
//! delete, save-as-template, reload from disk.
//!
//! Every test gets its OWN rooms + templates directory pair (the registry
//! takes both as arguments), so nothing here races on an env var or on the
//! filesystem.

use super::*;
use crate::templates::{self, MachineSpec};
use crate::{DocOp, MOTOR_ID, SENSOR_ID};
use sim_core::ElementKind as K;
use sim_golden::{dc, gnd, r, spec};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn dirs(tag: &str) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("ee-rooms-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (rooms, tpls) = (base.join("rooms"), base.join("templates"));
    std::fs::create_dir_all(&rooms).unwrap();
    std::fs::create_dir_all(&tpls).unwrap();
    (rooms, tpls)
}

fn small_circuit() -> Vec<sim_core::ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 8)),
        spec(2, r(1000.0), (0, 0), (4, 0)),
        spec(3, K::Wire, (4, 0), (0, 8)),
        gnd(4, (0, 8)),
    ]
}

/// NO FALSE REJECTIONS. Every shipped template must still pass the gate it
/// passed before the shape rule existed, and must still accept an ordinary
/// edit afterwards.
///
/// The second half is the one that matters: `check_shapes` is a rule about a
/// CHANGE, so a template full of parts that predate it has to stay editable.
/// If this ever fails, a room made from that template cannot be worked on —
/// which is the exact failure mode a whole-document rule would have caused
/// in every one of these five.
#[test]
fn every_builtin_template_still_loads_and_still_accepts_edits() {
    for b in templates::BUILTINS {
        let doc = (b.build)().normalize().unwrap().elements;
        assert!(
            crate::check_room_doc(&doc).is_ok(),
            "template {} no longer validates",
            b.id
        );
        // An unrelated part dropped into the room: the ordinary edit.
        let mut next = doc.clone();
        next.push(spec(880, r(1000.0), (-400, -400), (-396, -400)));
        assert!(
            crate::check_room_edit(&doc, &next).is_ok(),
            "template {} refuses a plain edit",
            b.id
        );
        // ...and every multi-pin part in it can still be picked up and moved.
        for e in doc.iter().filter(|e| e.pins.len() > 2) {
            let mut moved = doc.clone();
            let t = moved.iter_mut().find(|x| x.id == e.id).unwrap();
            t.pins = t.pins.iter().map(|p| (p.0 + 100, p.1 + 100)).collect();
            assert!(
                sim_core::check_shapes(&doc, &moved).is_ok(),
                "template {} cannot move part #{}",
                b.id,
                e.id
            );
        }
    }
}

/// How much of each shipped template is already in formation. Not a
/// requirement — the grandfather clause means a legacy part costs nothing —
/// but a number worth keeping honest, because every part outside its family
/// is a part that will visibly snap the first time someone drags it.
#[test]
fn the_shipped_templates_shape_census() {
    let mut lines = Vec::new();
    for b in templates::BUILTINS {
        let doc = (b.build)().normalize().unwrap().elements;
        let multi: Vec<_> = doc
            .iter()
            .filter(|e| e.pins.len() > 2 && !crate::reserved_id(e.id))
            .collect();
        let bad = multi
            .iter()
            .filter(|e| !sim_core::is_rigid(&e.kind, &e.pins))
            .count();
        lines.push(format!(
            "{}: {}/{} in formation",
            b.id,
            multi.len() - bad,
            multi.len()
        ));
    }
    let census = lines.join("  |  ");
    // Recorded, not asserted part by part: the synth is rebuilt in formation
    // by the layout pass that follows this one, and the two showcase rooms
    // are hand-placed vignettes nobody has revisited since.
    assert!(census.contains("sandbox: 0/0"), "{census}");
    println!("shape census — {census}");
}

#[test]
fn every_builtin_template_creates_a_room_that_matches_its_advertisement() {
    let (rd, td) = dirs("builtins");
    let reg = Registry::open(&rd, &td);
    for b in templates::BUILTINS {
        let h = reg
            .create(b.name, b.id)
            .unwrap_or_else(|e| panic!("template {} failed: {e:?}", b.id));
        let m = h.meta();
        assert_eq!(m.template, b.id);
        assert_eq!(m.name, b.name);
        assert!(valid_code(&m.id), "{} minted a bad code {}", b.id, m.id);
        assert!(h.path.is_file(), "{} wrote no room file", b.id);
        // What the listing promised is what the room got.
        let info = templates::list(&td)
            .into_iter()
            .find(|t| t.id == b.id)
            .unwrap();
        let has_fixture = h
            .room
            .elements
            .lock()
            .unwrap()
            .iter()
            .any(|e| crate::reserved_id(e.id));
        assert_eq!(has_fixture, h.has_machine, "{}", b.id);
        assert_eq!(h.has_machine, info.machine == "hoist", "{}", b.id);
        assert_eq!(h.room.panels.lock().unwrap().len(), info.panels);
        assert_eq!(h.room.probes.lock().unwrap().len(), info.probes);
    }
    assert_eq!(reg.list().len(), templates::BUILTINS.len());
}

#[test]
fn the_hoist_template_ships_its_ui_not_just_its_parts() {
    let (rd, td) = dirs("hoistui");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Hoist practice", "hoist").unwrap();

    // Parts: the four locked fixtures, and nothing else — this template is
    // the crate-and-motor game, not the showcase.
    let elems = h.room.elements.lock().unwrap().clone();
    assert_eq!(elems.len(), 4);
    for id in [900, 901, 902, 903] {
        assert!(elems.iter().any(|e| e.id == id), "missing fixture {id}");
    }
    // UI, part 1: a control panel over the empty bench west of the cabinet.
    let panels = h.room.panels.lock().unwrap().clone();
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0].name, "DRIVE");
    assert!(
        panels[0].x1 < crate::HOIST_RECT[0] as f64,
        "the DRIVE panel belongs on the bench, not on the cabinet"
    );
    // UI, part 2: two scope channels, armed on the two numbers the goal is
    // actually about.
    let probes = h.room.probes.lock().unwrap().clone();
    assert_eq!(probes.len(), 2);
    assert!(probes
        .iter()
        .any(|p| p.elem == MOTOR_ID && p.kind == crate::ProbeKind::I));
    assert!(probes
        .iter()
        .any(|p| p.elem == SENSOR_ID && p.kind == crate::ProbeKind::V));
    assert_eq!(h.room.next_pid.load(Ordering::Relaxed), 3);
    // UI, part 3: a camera framing the hoist district, and an in-place scope
    // already showing both channels.
    let v = h.view.lock().unwrap().clone();
    let home = v.home.expect("the hoist template frames its own district");
    assert!(
        home[0] > 0.0 && home[2] > crate::HOIST_RECT[2] as f64,
        "{home:?}"
    );
    assert_eq!(v.scopes.len(), 1);
    // And the machine itself.
    assert!(h.has_machine);
    assert!(matches!(
        *h.machine.lock().unwrap(),
        MachineSpec::Hoist { .. }
    ));
}

/// The client contract for "which room am I in": every joiner is told, in
/// the frame it already parses.
#[test]
fn the_hello_frame_names_the_room_and_carries_its_whole_setup() {
    let (rd, td) = dirs("hello");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Hoist practice", "hoist").unwrap();
    let msg: serde_json::Value = serde_json::from_str(&crate::hello_msg(&h, 7)).unwrap();
    assert_eq!(msg["t"], "hello");
    assert_eq!(msg["you"], 7);
    assert_eq!(msg["room"]["id"], h.meta().id);
    assert_eq!(msg["room"]["name"], "Hoist practice");
    assert_eq!(msg["room"]["template"], "hoist");
    assert_eq!(msg["machine"], true);
    assert_eq!(msg["elements"].as_array().unwrap().len(), 4);
    assert_eq!(msg["probes"].as_array().unwrap().len(), 2);
    assert_eq!(msg["panels"].as_array().unwrap().len(), 1);
    assert!(msg["view"]["home"].is_array());
    assert_eq!(msg["view"]["scopes"].as_array().unwrap().len(), 1);
    // A machineless room says so, so the client can hide the goal card
    // instead of latching it forever.
    let s = reg.create("Empty", "sandbox").unwrap();
    let m2: serde_json::Value = serde_json::from_str(&crate::hello_msg(&s, 1)).unwrap();
    assert_eq!(m2["machine"], false);
    assert!(m2["elements"].as_array().unwrap().is_empty());
    assert_eq!(m2["room"]["template"], "sandbox");
}

/// The JSON type of a dotted path, in the names the client's `typeof` uses.
fn json_type_at(root: &serde_json::Value, path: &str) -> &'static str {
    let mut v = root;
    for key in path.split('.') {
        match v.get(key) {
            Some(next) => v = next,
            None => return "missing",
        }
    }
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// THE OTHER HALF of `hello`: not "does the server send it" — the test above
/// already asked that, passed, and the feature was still broken — but "is it
/// the shape the client PARSES".
///
/// `view` and `machine` reached the socket in perfect health and stopped at
/// the client boundary, which forwarded `hello.room` alone as its RoomHello.
/// The TypeScript interface declared all six fields; `JSON.parse` returns
/// `any`; nothing on either side could notice. A template's camera, its
/// seeded scopes and its goal card simply never happened.
///
/// So the shape now lives in one file that neither half owns —
/// `packages/app/src/wire/hello.contract.json` — and both halves assert
/// against it: this test against a real `hello_msg`, and
/// `pnpm --filter @ee/app wirecheck` against `parseHello`. Move a field on
/// either side and one of the two fails, naming the path.
#[test]
fn the_hello_a_room_sends_is_the_shape_the_client_parses() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../packages/app/src/wire/hello.contract.json"
    ))
    .expect("the hello contract is not valid JSON");
    let types = contract["types"]
        .as_object()
        .expect("the contract lists the paths a hello must carry");
    let sample = &contract["sample"];

    let (rd, td) = dirs("hellocontract");
    let reg = Registry::open(&rd, &td);
    // The hoist template is the one that exercises every path by PRESENCE:
    // parts, panels, probes, a framed camera, a seeded scope and a goal.
    let h = reg.create("Hoist practice", "hoist").unwrap();
    let hello: serde_json::Value = serde_json::from_str(&crate::hello_msg(&h, 1)).unwrap();

    for (path, want) in types {
        let want = want.as_str().expect("contract types are type names");
        assert_eq!(
            json_type_at(&hello, path),
            want,
            "hello.{path}: the client parses this as {want}. If the server \
             moved or renamed it, move it in hello.contract.json too — and \
             fix packages/app/src/net.ts, which is where it lands."
        );
        // The sample in the file is what the client's own check parses, so it
        // must not rot away from the contract it illustrates.
        assert_eq!(
            json_type_at(sample, path),
            want,
            "hello.contract.json: `sample` disagrees with `types` at {path}"
        );
    }

    // Where the two client-side fields live is the whole point: BESIDE
    // `room`, not inside it. They are this client's half of a room — a camera
    // to fly, instruments to materialize, a goal card to show — not registry
    // metadata about it, and `room` is the object the lobby also serves.
    assert!(
        hello["room"].get("view").is_none() && hello["room"].get("machine").is_none(),
        "view/machine are top-level in `hello`; a second copy inside `room` \
         is a fork waiting to disagree with itself"
    );

    // And the values, not just the shapes: a template that frames a district
    // and ships an instrument has to say so in the payload the client reads.
    assert_eq!(hello["machine"], true);
    let home = hello["view"]["home"].as_array().unwrap();
    assert_eq!(home.len(), 4, "a camera rect is [x0, y0, x1, y1]");
    assert!(home[2].as_f64().unwrap() > home[0].as_f64().unwrap());
    assert_eq!(hello["view"]["scopes"].as_array().unwrap().len(), 1);

    // A hand-written template's home reaches the same path — this is the
    // field that decides whether a player lands on their own level or 500
    // units away from it.
    std::fs::write(
        td.join("district.json"),
        serde_json::json!({
            "name": "District", "blurb": "far from the origin",
            "elements": [], "probes": [], "panels": [],
            "machine": {"kind": "none"},
            "view": {"home": [500.0, 500.0, 540.0, 530.0], "scopes": []}
        })
        .to_string(),
    )
    .unwrap();
    let d = reg.create("Far away", "district").unwrap();
    let dh: serde_json::Value = serde_json::from_str(&crate::hello_msg(&d, 2)).unwrap();
    assert_eq!(
        dh["view"]["home"],
        serde_json::json!([500.0, 500.0, 540.0, 530.0])
    );
    assert_eq!(dh["machine"], false);
}

#[test]
fn a_machineless_template_keeps_the_reserved_ids_free() {
    let (rd, td) = dirs("sandbox");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Empty", "sandbox").unwrap();
    assert!(h.room.elements.lock().unwrap().is_empty());
    assert!(!h.has_machine);
    assert_eq!(*h.machine.lock().unwrap(), MachineSpec::None);
    // And it must not grow one across a restart either.
    drop(reg);
    let reg2 = Registry::open(&rd, &td);
    let h2 = reg2.get(&h.meta().id).unwrap();
    assert!(h2.room.elements.lock().unwrap().is_empty());
    assert!(!h2.has_machine);
}

#[test]
fn rooms_survive_a_restart_with_their_names_templates_and_documents() {
    let (rd, td) = dirs("reload");
    let (a, b) = {
        let reg = Registry::open(&rd, &td);
        let a = reg.create("Alpha", "hoist").unwrap();
        let b = reg.create("Beta", "sandbox").unwrap();
        // A player builds something in Beta and the room checkpoints.
        b.room.elements.lock().unwrap().extend(small_circuit());
        let machine = *b.machine.lock().unwrap();
        b.checkpoint(&machine, &DamageModel::new());
        (a.meta().id, b.meta().id)
    };
    let reg = Registry::open(&rd, &td);
    assert_eq!(reg.list().len(), 2);
    let ra = reg.get(&a).unwrap();
    let rb = reg.get(&b).unwrap();
    assert_eq!(ra.meta().name, "Alpha");
    assert_eq!(ra.meta().template, "hoist");
    assert!(ra.has_machine);
    assert_eq!(rb.meta().name, "Beta");
    assert!(
        !rb.has_machine,
        "a machineless room must not gain a hoist on reload"
    );
    assert_eq!(rb.room.elements.lock().unwrap().len(), 4);
    // Both come back PARKED: no player, no sim task, no core burned.
    assert!(!ra.is_live() && !rb.is_live());
    assert!(ra.parked.lock().unwrap().is_some());
}

#[test]
fn a_legacy_single_file_save_becomes_a_room_with_its_hoist() {
    let (rd, td) = dirs("legacy");
    let base = rd.parent().unwrap().to_path_buf();
    let legacy = base.join("room-save.json");
    // The pre-rooms format: no v, no kind, no machine key at all.
    let json = serde_json::json!({
        "elements": small_circuit(),
        "probes": [], "next_pid": 1, "panels": [], "next_plid": 1,
        "hoist_rect": [46, 2, 64, 24],
    });
    std::fs::write(&legacy, serde_json::to_string(&json).unwrap()).unwrap();

    let reg = Registry::open(&rd, &td);
    let code = reg.import_legacy(&legacy).expect("legacy import");
    let h = reg.get(&code).unwrap();
    assert_eq!(h.meta().name, "Main Room");
    assert_eq!(h.meta().template, "demo");
    // An absent `machine` means the single-room server wrote this, and that
    // server always had a hoist: it comes back.
    assert!(h.has_machine);
    let elems = h.room.elements.lock().unwrap().clone();
    assert_eq!(elems.len(), 8, "4 player parts + the 4 fixtures");
    assert!(elems.iter().any(|e| e.id == MOTOR_ID));
    // Moved aside, never deleted, and never imported twice.
    assert!(!legacy.is_file());
    assert!(base.join("room-save.json.migrated").is_file());
    assert!(reg.import_legacy(&legacy).is_none());
}

#[test]
fn saving_a_running_room_as_a_template_keeps_the_level_and_drops_the_playthrough() {
    let (rd, td) = dirs("astemplate");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Hoist practice", "hoist").unwrap();
    // Somebody plays it: parts added, the crate lifted, the goal won, the
    // cabinet dragged somewhere else.
    h.room.elements.lock().unwrap().extend(small_circuit());
    {
        let mut m = h.machine.lock().unwrap();
        if let MachineSpec::Hoist { rect, state } = &mut *m {
            state.y = 1.2;
            state.win = true;
            state.joules = 42.0;
            *rect = crate::sane_rect([rect[0] + 10, rect[1] + 3, 0, 0]);
        }
    }
    let setup = h.as_template_setup(None).normalize().unwrap();
    // The level survives...
    assert_eq!(setup.elements.len(), 8);
    assert_eq!(setup.panels.len(), 1);
    assert_eq!(setup.probes.len(), 2);
    // ...including where the player left the cabinet...
    let rect = match setup.machine {
        MachineSpec::Hoist { rect, state } => {
            // ...but the playthrough does not.
            assert_eq!(
                state,
                machine::Hoist::default(),
                "the goal must be re-armed"
            );
            rect
        }
        MachineSpec::None => panic!("the hoist template has a machine"),
    };
    assert_ne!(rect, crate::HOIST_RECT);
    assert!(setup.damage.broken_ids().is_empty());

    templates::write(&td, "my-lab", "My Lab", "a bench", &setup).unwrap();
    // It shows up in the listing beside the built-ins...
    let list = templates::list(&td);
    let info = list.iter().find(|t| t.id == "my-lab").unwrap();
    assert_eq!(info.source, "file");
    assert_eq!((info.parts, info.panels, info.probes), (8, 1, 2));
    assert_eq!(info.machine, "hoist");
    // ...and a room made from it is that level, re-armed, on that footprint.
    let h2 = reg.create("Second bench", "my-lab").unwrap();
    assert_eq!(h2.room.elements.lock().unwrap().len(), 8);
    assert_eq!(h2.room.panels.lock().unwrap()[0].name, "DRIVE");
    assert_eq!(h2.room.probes.lock().unwrap().len(), 2);
    assert_eq!(
        *h2.machine.lock().unwrap(),
        MachineSpec::Hoist {
            rect,
            state: machine::Hoist::default()
        }
    );
    // A saved template survives a restart of the server, like any file.
    let reg2 = Registry::open(&rd, &td);
    assert!(reg2.create("third", "my-lab").is_ok());
}

#[test]
fn a_file_template_shadows_the_builtin_with_the_same_id() {
    let (rd, td) = dirs("shadow");
    let reg = Registry::open(&rd, &td);
    let setup = templates::RoomSetup {
        elements: small_circuit(),
        ..Default::default()
    };
    templates::write(&td, "sandbox", "Sandbox (house rules)", "", &setup).unwrap();
    let list = templates::list(&td);
    assert_eq!(list.iter().filter(|t| t.id == "sandbox").count(), 1);
    let s = list.iter().find(|t| t.id == "sandbox").unwrap();
    assert_eq!(s.source, "file");
    assert_eq!(s.parts, 4);
    // Built-ins that are NOT shadowed are still there.
    assert!(list
        .iter()
        .any(|t| t.id == "hoist" && t.source == "builtin"));
    let h = reg.create("House", "sandbox").unwrap();
    assert_eq!(h.room.elements.lock().unwrap().len(), 4);
}

#[test]
fn a_corrupt_template_file_is_ignored_rather_than_served() {
    let (rd, td) = dirs("corrupt");
    let reg = Registry::open(&rd, &td);
    std::fs::write(td.join("junk.json"), "{ this is not json").unwrap();
    assert!(!templates::list(&td).iter().any(|t| t.id == "junk"));
    assert_eq!(reg.create("x", "junk").err(), Some(CreateErr::NoTemplate));
    // A template whose document is malformed (a two-pin resistor with one
    // pin) is refused at CREATE time, so a hand-edited file can never
    // produce a room that arrives broken.
    let bad = serde_json::json!({
        "v": 1, "kind": "template", "id": "bad", "name": "Bad",
        "elements": [{"id": 1, "kind": {"t": "Resistor", "ohms": 100.0}, "pins": [[0, 0]]}],
        "machine": {"kind": "none"},
    });
    std::fs::write(td.join("bad.json"), serde_json::to_string(&bad).unwrap()).unwrap();
    assert_eq!(
        reg.create("x", "bad").err(),
        Some(CreateErr::BadTemplate("badpins"))
    );
    assert!(reg.list().is_empty());
}

#[test]
fn codes_are_unique_clean_and_never_reach_the_filesystem_unchecked() {
    let (rd, td) = dirs("codes");
    let reg = Registry::open(&rd, &td);
    let mut seen = std::collections::HashSet::new();
    for i in 0..12 {
        let h = reg.create(&format!("room {i}"), "sandbox").unwrap();
        let id = h.meta().id;
        assert!(valid_code(&id), "{id}");
        assert!(!id.contains(['0', 'O', '1', 'I', 'L', 'U']), "{id}");
        assert!(seen.insert(id));
    }
    // Path traversal is refused by the SHAPE of a code, before anything
    // builds a path out of it.
    assert!(!valid_code("../../etc/passwd"));
    assert!(!valid_code("ABC"));
    assert!(reg.resolve(Some("../../etc/passwd")).is_none());
    assert!(reg.get("../../etc/passwd").is_none());
    assert!(!templates::valid_id("../evil"));
    assert!(templates::write(&td, "../evil", "x", "", &Default::default()).is_err());
    assert!(!rd.parent().unwrap().join("evil.json").exists());
    // A socket that names no room still lands somewhere; a wrong code does
    // not silently land in someone else's room.
    assert!(reg.resolve(None).is_some());
    assert!(reg.resolve(Some("ZZZZZZ")).is_none());
    // Codes are case-insensitive on the way in (they get typed by hand).
    let any = reg.list()[0].id.clone();
    assert!(reg.resolve(Some(&any.to_lowercase())).is_some());
}

#[test]
fn the_room_budget_is_enforced() {
    let (rd, td) = dirs("cap");
    let reg = Registry::open(&rd, &td);
    for i in 0..MAX_ROOMS {
        reg.create(&format!("r{i}"), "sandbox").unwrap();
    }
    assert_eq!(
        reg.create("one too many", "sandbox").err(),
        Some(CreateErr::TooMany)
    );
    assert_eq!(reg.list().len(), MAX_ROOMS);
    // Deleting one makes room again.
    let victim = reg.list()[0].id.clone();
    assert!(reg.delete(&victim));
    assert!(reg.create("fits now", "sandbox").is_ok());
    // A directory holding more than the budget loads the most recently
    // played MAX_ROOMS and leaves the rest on disk.
    let reloaded = Registry::open(&rd, &td);
    assert_eq!(reloaded.list().len(), MAX_ROOMS);
}

#[test]
fn creating_from_an_unknown_template_is_refused_without_touching_disk() {
    let (rd, td) = dirs("notemplate");
    let reg = Registry::open(&rd, &td);
    assert_eq!(reg.create("x", "nope").err(), Some(CreateErr::NoTemplate));
    assert_eq!(reg.create("   ", "sandbox").err(), Some(CreateErr::BadName));
    assert!(reg.list().is_empty());
    assert_eq!(std::fs::read_dir(&rd).unwrap().count(), 0);
}

#[tokio::test]
async fn joining_resumes_a_parked_room_and_parking_preserves_everything() {
    let (rd, td) = dirs("park");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Bench", "hoist").unwrap();
    assert!(!h.is_live(), "a room with nobody in it has no sim task");

    // A player joins: the room resumes.
    let presence = reg.enter(&h, 1);
    assert!(h.is_live());
    assert!(h.parked.lock().unwrap().is_none());

    // They build something; the sim task applies it at a tick boundary.
    for e in small_circuit() {
        let _ = h.room.cmds.send(crate::Cmd::Edit {
            // `who` is the client a refusal is reported back to; these test
            // edits are all valid, so any id will do.
            who: 1,
            op: DocOp::Add { spec: e },
        });
    }
    // ...and they write on the sheet. ANNOTATION IS ROOM STATE: a box with a
    // title and a name pinned to a grid point both go through the tick, both
    // get checkpointed, and both have to come back off disk saying exactly
    // what they said.
    let _ = h.room.cmds.send(crate::Cmd::LabelBox {
        op: crate::LabelBoxOp::Add {
            x0: 2.0,
            y0: 2.0,
            x1: 14.0,
            y1: 9.0,
            name: Some("POWER STAGE".into()),
        },
    });
    let _ = h.room.cmds.send(crate::Cmd::NetLabel {
        op: crate::NetLabelOp::Add {
            x: 3,
            y: 4,
            name: Some("5V RAIL".into()),
        },
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(h.room.elements.lock().unwrap().len(), 8);
    assert_eq!(h.room.label_boxes.lock().unwrap().len(), 1);
    assert_eq!(h.room.net_labels.lock().unwrap().len(), 1);

    // They leave and the room parks. `Stop` is the same handover the 30 s
    // empty-room timer takes, without making the test wait 30 s for it.
    drop(presence);
    let _ = h.room.cmds.send(crate::Cmd::Stop { checkpoint: true });
    for _ in 0..80 {
        if !h.is_live() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!h.is_live(), "the room should have parked");
    assert!(h.parked.lock().unwrap().is_some(), "state handed back");

    // Everything survived, in memory and on disk.
    assert_eq!(h.room.elements.lock().unwrap().len(), 8);
    assert_eq!(h.room.probes.lock().unwrap().len(), 2);
    assert_eq!(h.room.panels.lock().unwrap().len(), 1);
    let reloaded = Registry::open(&rd, &td);
    let back = reloaded.get(&h.meta().id).unwrap();
    assert_eq!(back.room.elements.lock().unwrap().len(), 8);
    assert_eq!(back.room.probes.lock().unwrap().len(), 2);
    assert_eq!(back.room.panels.lock().unwrap()[0].name, "DRIVE");
    // The words survive the round trip through the file, in both primitives.
    let boxes = back.room.label_boxes.lock().unwrap().clone();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].name, "POWER STAGE");
    assert_eq!((boxes[0].x0, boxes[0].y1), (2.0, 9.0));
    let nets = back.room.net_labels.lock().unwrap().clone();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].name, "5V RAIL");
    assert_eq!((nets[0].x, nets[0].y), (3, 4), "the anchor is a grid point");
    // A restored room must not hand the next label an id somebody already has.
    assert!(back.room.next_blid.load(Ordering::Relaxed) > boxes[0].blid);
    assert!(back.room.next_nlid.load(Ordering::Relaxed) > nets[0].nlid);
    assert!(back.has_machine);

    // And it resumes again.
    let presence = reg.enter(&h, 2);
    assert!(h.is_live());
    drop(presence);
    let _ = h.room.cmds.send(crate::Cmd::Stop { checkpoint: true });
}

#[tokio::test]
async fn two_rooms_step_independently() {
    let (rd, td) = dirs("two");
    let reg = Registry::open(&rd, &td);
    let a = reg.create("A", "sandbox").unwrap();
    let b = reg.create("B", "sandbox").unwrap();
    let pa = reg.enter(&a, 1);
    let pb = reg.enter(&b, 1);
    // An edit in A is an edit in A. Nothing crosses.
    for e in small_circuit() {
        let _ = a.room.cmds.send(crate::Cmd::Edit {
            // `who` is the client a refusal is reported back to; these test
            // edits are all valid, so any id will do.
            who: 1,
            op: DocOp::Add { spec: e },
        });
    }
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(a.room.elements.lock().unwrap().len(), 4);
    assert!(b.room.elements.lock().unwrap().is_empty());
    assert!(a.is_live() && b.is_live());
    drop((pa, pb));
    for h in [&a, &b] {
        let _ = h.room.cmds.send(crate::Cmd::Stop { checkpoint: true });
    }
}

#[tokio::test]
async fn deleting_a_room_evicts_its_players_and_removes_its_file() {
    let (rd, td) = dirs("delete");
    let reg = Registry::open(&rd, &td);
    let keep = reg.create("Keep", "sandbox").unwrap();
    let doomed = reg.create("Doomed", "sandbox").unwrap();
    let code = doomed.meta().id;
    let _presence = reg.enter(&doomed, 1);
    let mut life = doomed.subscribe_life();
    assert!(doomed.path.is_file());

    assert!(reg.delete(&code));
    // The session loop's third select! arm: everyone inside is told why.
    life.changed().await.unwrap();
    assert_eq!(*life.borrow(), Life::Gone);
    assert!(!doomed.path.is_file(), "the file goes with the room");
    assert!(reg.get(&code).is_none(), "no new joins");
    assert!(reg.resolve(Some(&code)).is_none());
    assert_eq!(reg.list().len(), 1);
    assert_eq!(reg.default_room().unwrap().meta().id, keep.meta().id);
    assert!(!reg.delete(&code), "deleting twice is false, not a panic");

    // A checkpoint racing the delete must not resurrect the file.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    doomed.checkpoint(&MachineSpec::None, &DamageModel::new());
    assert!(!doomed.path.is_file());
    // The survivor is untouched and still reloads.
    let reloaded = Registry::open(&rd, &td);
    assert_eq!(reloaded.list().len(), 1);
    assert_eq!(reloaded.list()[0].name, "Keep");
}

#[tokio::test]
async fn renaming_a_room_broadcasts_and_persists() {
    let (rd, td) = dirs("rename");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Before", "sandbox").unwrap();
    let mut ev = h.room.events.subscribe();
    assert!(reg.rename(&h.meta().id, "  After  ").is_some());
    assert_eq!(h.meta().name, "After");
    let msg = ev.try_recv().unwrap();
    assert!(msg.contains("roommeta") && msg.contains("After"), "{msg}");
    assert!(reg.rename(&h.meta().id, "   ").is_none());
    assert!(reg.rename("ZZZZZZ", "nope").is_none());
    // A parked room has no sim task to checkpoint for it, so the rename is
    // written on the spot.
    let reloaded = Registry::open(&rd, &td);
    assert_eq!(reloaded.get(&h.meta().id).unwrap().meta().name, "After");
}

/// A CHAT MESSAGE IS NOT ROOM STATE. The scrollback lives in memory beside
/// `claims`, and the save file must never learn what anybody said: a
/// checkpoint is the circuit, the panels and the instruments — a conversation
/// is not part of the document, and a player who joins later has not missed
/// a wire. Serialize a real snapshot of a room WITH chat in it and prove the
/// words are not in the bytes.
#[test]
fn a_conversation_never_reaches_the_save_file() {
    let (rd, td) = dirs("chatnodisk");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Chatty", "sandbox").unwrap();
    crate::push_chat(
        &mut h.room.chat.lock().unwrap(),
        crate::ChatLine {
            who: 3,
            text: "meet at the north rail".into(),
        },
    );
    let save = h.snapshot(&MachineSpec::None, &DamageModel::new());
    let json = serde_json::to_string(&save).unwrap();
    assert!(
        !json.contains("north rail") && !json.contains("\"chat\""),
        "a chat line leaked into the checkpoint: {json}"
    );
    // And the round trip agrees: a reloaded room starts with an empty tail.
    h.checkpoint(&MachineSpec::None, &DamageModel::new());
    let reloaded = Registry::open(&rd, &td);
    let h2 = reloaded.get(&h.meta().id).unwrap();
    assert!(h2.room.chat.lock().unwrap().is_empty());
}

/// The regression that motivated `Presence`: population must return to zero
/// no matter HOW a session ends — clean drop, panic, or the whole future
/// dropped mid-await. The old three hand-placed `leave()` calls covered only
/// the returns; a session that ended any other way kept its +1 forever,
/// which is exactly "the player count climbs by one per refresh and never
/// comes down" (and, since a room parks only at population 0, also "the sim
/// task of an abandoned room runs forever").
#[tokio::test]
async fn population_returns_to_zero_by_every_exit_path() {
    let (rd, td) = dirs("presence");
    let reg = Arc::new(Registry::open(&rd, &td));
    let h = reg.create("Bench", "sandbox").unwrap();
    let pop = || h.room.population.load(Ordering::SeqCst);

    // Exit path 1: the ordinary drop at the end of a session.
    let p1 = reg.enter(&h, 1);
    let p2 = reg.enter(&h, 2);
    assert_eq!(pop(), 2, "two enters count two players");
    drop(p1);
    assert_eq!(pop(), 1);

    // Exit path 2: a panic while present. Unwinding drops the guard.
    let reg2 = reg.clone();
    let h2 = h.clone();
    let died = std::thread::spawn(move || {
        let _p = reg2.enter(&h2, 3);
        panic!("session died mid-flight");
    })
    .join();
    assert!(died.is_err(), "the thread must actually have panicked");
    assert_eq!(pop(), 1, "a panicked session still uncounts itself");

    // Exit path 3: the future is DROPPED, not returned from — no code after
    // the await point ever runs. This is what happens to any task the
    // runtime aborts, and it is the path no hand-placed call can cover.
    let p4 = reg.enter(&h, 4);
    assert_eq!(pop(), 2);
    let task = tokio::spawn(async move {
        let _held = p4;
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;
    assert_eq!(pop(), 1, "an aborted session still uncounts itself");

    drop(p2);
    assert_eq!(pop(), 0, "everyone gone reads zero — the room can park");
    let _ = h.room.cmds.send(crate::Cmd::Stop { checkpoint: false });
}

/// The socket resolved its handle, then the room was deleted before the
/// session entered. The count must still balance — an unbalanced enter here
/// would pin a ghost in a room that no longer exists.
#[tokio::test]
async fn entering_a_room_being_deleted_still_balances_the_count() {
    let (rd, td) = dirs("presence-del");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Doomed", "sandbox").unwrap();
    assert!(reg.delete(&h.meta().id));
    let p = reg.enter(&h, 1);
    assert!(!h.is_live(), "a deleted room must not be resumed");
    assert_eq!(h.room.population.load(Ordering::SeqCst), 1);
    drop(p);
    assert_eq!(h.room.population.load(Ordering::SeqCst), 0);
}

/// The room-side half of leaving rides the same guard: dropping a Presence
/// sends `Cmd::Leave { who }`, so the sim task drops the player's layer
/// claims and fails their driven parts dark no matter how the session ended.
#[tokio::test]
async fn dropping_a_presence_tells_the_room_who_left() {
    let (rd, td) = dirs("presence-cmd");
    let reg = Registry::open(&rd, &td);
    let h = reg.create("Bench", "sandbox").unwrap();
    // Steal the parked receiver first, so entering cannot resume a sim task
    // that would race this test for the command stream.
    let mut rx = h.parked.lock().unwrap().take().unwrap().rx;
    let p = reg.enter(&h, 7);
    drop(p);
    let mut saw = false;
    while let Ok(cmd) = rx.try_recv() {
        if matches!(cmd, crate::Cmd::Leave { who: 7 }) {
            saw = true;
        }
    }
    assert!(saw, "the room must hear who left");
}
