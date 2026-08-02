//! Room templates: what a new room starts as.
//!
//! A template is a WHOLE ROOM SETUP, not a netlist — parts, scope channels
//! (probes), control panels, the machine/goal a room arms, the camera the
//! player lands on, and the instruments the room ships with. That is the
//! whole point: THE HOIST is not "four fixture parts", it is four fixture
//! parts PLUS a DRIVE panel over the bench PLUS an armature-current channel
//! PLUS a height channel PLUS a scope already showing both.
//!
//! Two sources, one list:
//!   * BUILT-INS — compiled in, so a bare `git clone` boots with content and
//!     no data directory, and so the hoist template can reference the fixture
//!     code (`hoist_fixture_at`, `HOIST_RECT`, `MOTOR_ID`) directly and never
//!     drift from it.
//!   * FILES — `$EE_TEMPLATES/<id>.json`, re-scanned on every list/resolve.
//!     Same format as a room checkpoint (`SaveFile`), which is what makes
//!     "save this running room as a template" one function instead of a
//!     second serialization format. A file id SHADOWS a built-in of the same
//!     id, so the owner can override a shipped template without a rebuild.
//!
//! Adding a template therefore never requires a code change (drop a JSON in
//! the directory, or press "save this room as a template"), and when a
//! template *should* ship with the binary it is one entry in `BUILTINS`.

use crate::{
    demo_room_circuit, hoist_fixture_at, sane_rect, Panel, ProbeKind, SaveFile, SavedProbe,
    HOIST_RECT, MAX_ELEMENTS, MAX_PANELS, MAX_PROBES,
};
use damage::DamageModel;
use machine::Hoist;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sim_core::ElementSpec;
use std::path::{Path, PathBuf};

/// Largest template/room file we will read off disk. A template is a
/// document, not a stream; 8 MB is ~50k elements of JSON with room to spare.
pub const MAX_TEMPLATE_BYTES: u64 = 8 * 1024 * 1024;

/// Seed scopes a template may carry (see `View`).
pub const MAX_SEED_SCOPES: usize = 8;

/// File templates one directory will serve. `GET /api/templates` parses
/// every one of them on every call, so this bounds that work; built-ins are
/// on top of it and always listed.
pub const MAX_FILE_TEMPLATES: usize = 128;

/// The machine a room arms. `None` is a first-class answer: a synth world or
/// a sandbox has no hoist, gets no fixture injected into ids 900-999, and
/// broadcasts no `machine` telemetry.
///
/// Internally tagged so a template author can hand-write
/// `"machine": {"kind":"none"}` — and so ABSENT (legacy saves) stays
/// distinguishable from an explicit "no machine".
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MachineSpec {
    None,
    Hoist {
        rect: [i32; 4],
        #[serde(default)]
        state: Hoist,
    },
}

impl MachineSpec {
    pub fn is_some(&self) -> bool {
        !matches!(self, MachineSpec::None)
    }

    /// The name the wire uses ("none" / "hoist").
    pub fn kind_name(&self) -> &'static str {
        match self {
            MachineSpec::None => "none",
            MachineSpec::Hoist { .. } => "hoist",
        }
    }

    /// Same machine, mechanism re-armed: crate on the floor, goal not won,
    /// joules zero. What a template must ship — otherwise the template
    /// carries somebody's finished game.
    pub fn fresh(&self) -> MachineSpec {
        match *self {
            MachineSpec::None => MachineSpec::None,
            MachineSpec::Hoist { rect, .. } => MachineSpec::Hoist {
                rect: sane_rect(rect),
                state: Hoist::default(),
            },
        }
    }
}

/// The client-side half of a room setup: where the camera lands, and which
/// in-place scopes the room ships with.
///
/// `scopes` is deliberately OPAQUE to the server (a list of JSON objects the
/// client understands). In-place scopes are client-local state today — they
/// are a SEED, materialized once per room per browser — so giving the server
/// a schema for them would be inventing replication this change does not do.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct View {
    /// The rect the camera frames on first join, in grid units
    /// `[x0, y0, x1, y1]`. None = the client's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<[f64; 4]>,
    #[serde(default)]
    pub scopes: Vec<serde_json::Value>,
}

impl View {
    /// Force a hand-edited view back onto its invariants.
    pub fn sane(mut self) -> View {
        if let Some(h) = self.home {
            if !h.iter().all(|v| v.is_finite()) || h[2] - h[0] <= 0.0 || h[3] - h[1] <= 0.0 {
                self.home = None;
            }
        }
        self.scopes.retain(|s| s.is_object());
        self.scopes.truncate(MAX_SEED_SCOPES);
        self
    }
}

/// Everything a room starts as. This is exactly what `SaveFile` holds minus
/// per-run identity — which is why a checkpoint IS a template.
#[derive(Clone, Debug)]
pub struct RoomSetup {
    pub elements: Vec<ElementSpec>,
    pub probes: Vec<SavedProbe>,
    pub next_pid: u32,
    pub panels: Vec<Panel>,
    pub next_plid: u32,
    pub machine: MachineSpec,
    pub view: View,
    pub damage: DamageModel,
}

impl Default for RoomSetup {
    fn default() -> Self {
        RoomSetup {
            elements: Vec::new(),
            probes: Vec::new(),
            next_pid: 1,
            panels: Vec::new(),
            next_plid: 1,
            machine: MachineSpec::None,
            view: View::default(),
            damage: DamageModel::new(),
        }
    }
}

impl RoomSetup {
    /// Force a setup (built-in, hand-edited file, or restored save) onto the
    /// room invariants. Returns Err only for things a room could not survive;
    /// everything else is repaired quietly, because a template must never be
    /// able to create a room that arrives broken.
    pub fn normalize(mut self) -> Result<RoomSetup, &'static str> {
        if self.elements.len() > MAX_ELEMENTS {
            return Err("toobig");
        }
        // Structural gate: pin counts must match the kind, ids must be
        // unique. A malformed element would be refused by `apply_doc_op` if a
        // player sent it; a template does not get a lower bar.
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for e in &self.elements {
            if e.pins.len() != e.kind.pin_count() {
                return Err("badpins");
            }
            if !seen.insert(e.id) {
                return Err("dupid");
            }
        }
        // Probes on parts that are not here are dropped, not honored: a
        // dangling channel would draw a flat line forever.
        self.probes.retain(|p| {
            self.elements
                .iter()
                .any(|e| e.id == p.elem && p.pin < e.pins.len())
        });
        self.probes.truncate(MAX_PROBES);
        self.panels.truncate(MAX_PANELS);
        self.next_pid = self
            .next_pid
            .max(self.probes.iter().map(|p| p.pid + 1).max().unwrap_or(1))
            .max(1);
        self.next_plid = self
            .next_plid
            .max(self.panels.iter().map(|p| p.plid + 1).max().unwrap_or(1))
            .max(1);
        if let MachineSpec::Hoist { rect, state } = self.machine {
            self.machine = MachineSpec::Hoist {
                rect: sane_rect(rect),
                state,
            };
        }
        self.view = self.view.sane();
        Ok(self)
    }
}

/// A template that ships with the binary.
pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    pub build: fn() -> RoomSetup,
}

/// THE registry. Adding a shipped template is ONE ENTRY here; adding a
/// template at runtime is a file in `$EE_TEMPLATES` and touches no code at
/// all.
///
/// Registering the synth world (or any other world) later:
///
/// ```ignore
/// Builtin {
///     id: "synth", name: "Synth World",
///     blurb: "A patchbay, three oscillators and a speaker.",
///     build: || RoomSetup {
///         elements: synth_world_circuit(),   // yours, anywhere in the crate
///         panels:  synth_panels(),  next_plid: 4,
///         probes:  synth_probes(),  next_pid: 3,
///         machine: MachineSpec::None,        // <- keeps the hoist out
///         view: View { home: Some([-4.0, -4.0, 60.0, 40.0]), scopes: vec![] },
///         ..RoomSetup::default()
///     },
/// },
/// ```
///
/// The only rule: do not use element ids 900-999 (`reserved_id`) unless the
/// template declares the machine that owns them. A new `ElementKind` lands
/// in sim-core and this registry never learns about it — `ElementSpec` is
/// the entire vocabulary here.
pub static BUILTINS: &[Builtin] = &[
    Builtin {
        id: "demo",
        name: "Showcase + Hoist",
        blurb: "The four showcase vignettes, plus THE HOIST standing east of them.",
        build: demo_setup,
    },
    Builtin {
        id: "hoist",
        name: "THE HOIST",
        blurb: "Crate and motor. Lift it into the green band and hold it for five seconds.",
        build: hoist_setup,
    },
    Builtin {
        id: "showcase",
        name: "Showcase",
        blurb: "Four wired vignettes to poke at. No machine, no goal.",
        build: showcase_setup,
    },
    Builtin {
        id: "synth",
        name: "Analog Synthesizer",
        blurb: "A patchable synth: 555 clocks, OTA filters, a pot-and-button sequencer and noise drums. It is already playing.",
        build: synth_setup,
    },
    Builtin {
        id: "sandbox",
        name: "Sandbox",
        blurb: "An empty plane. Build anything.",
        build: sandbox_setup,
    },
];

/// Today's `None` branch, bit for bit: the showcase plus a hoist. Still the
/// default, so a fresh checkout boots into exactly what it booted into
/// before rooms existed.
fn demo_setup() -> RoomSetup {
    let mut elements = demo_room_circuit();
    elements.extend(hoist_fixture_at(HOIST_RECT));
    RoomSetup {
        elements,
        machine: MachineSpec::Hoist {
            rect: HOIST_RECT,
            state: Hoist::default(),
        },
        view: View {
            // The client's old hardcoded HOME_RECT, now room data.
            home: Some([-10.0, -10.0, 60.0, 60.0]),
            scopes: Vec::new(),
        },
        ..RoomSetup::default()
    }
}

fn showcase_setup() -> RoomSetup {
    RoomSetup {
        elements: demo_room_circuit(),
        machine: MachineSpec::None,
        view: View {
            home: Some([-10.0, -10.0, 60.0, 60.0]),
            scopes: Vec::new(),
        },
        ..RoomSetup::default()
    }
}

/// The analog synthesizer. It arrived on its own branch with an `EE_WORLD`
/// switch, which templates supersede — this is the same room, reached the way
/// every other room is reached.
fn synth_setup() -> RoomSetup {
    let panels: Vec<Panel> = crate::synth::synth_panels()
        .into_iter()
        .enumerate()
        .map(|(i, p)| Panel {
            plid: i as u32 + 1,
            x0: p.x0,
            y0: p.y0,
            x1: p.x1,
            y1: p.y1,
            name: p.name.to_string(),
        })
        .collect();
    let next_plid = panels.len() as u32 + 1;
    RoomSetup {
        elements: crate::synth::synth_room_circuit(),
        panels,
        next_plid,
        // No machine: the synth's goal is that it makes a noise.
        machine: MachineSpec::None,
        view: View {
            home: Some([-4.0, -4.0, 76.0, 44.0]),
            scopes: Vec::new(),
        },
        ..RoomSetup::default()
    }
}

fn sandbox_setup() -> RoomSetup {
    RoomSetup {
        view: View {
            home: Some([-10.0, -10.0, 10.0, 10.0]),
            scopes: Vec::new(),
        },
        ..RoomSetup::default()
    }
}

/// THE HOIST as a template — and the proof that a template pre-populates UI,
/// not just parts. A player who creates this room lands looking at the
/// cabinet with:
///   * the four fixture parts (ids 900-903) standing on the default footprint;
///   * a DRIVE control panel over the empty bench west of the cabinet, so the
///     first knob they place gets a mission-control window for free (panel
///     membership is geometric, so the window fills in as they build);
///   * two scope channels already armed — the motor's armature current and
///     the height sensor's wiper, the two numbers the goal is actually about;
///   * an in-place scope under the bench showing both.
fn hoist_setup() -> RoomSetup {
    let [x0, y0, x1, y1] = HOIST_RECT.map(|v| v as f64);
    RoomSetup {
        elements: hoist_fixture_at(HOIST_RECT),
        probes: vec![
            // Armature current: the number the motor's nameplate is about.
            SavedProbe {
                pid: 1,
                elem: crate::MOTOR_ID,
                pin: 0,
                kind: ProbeKind::I,
                r: None,
            },
            // Height: the sensor wiper, i.e. the feedback the goal needs.
            SavedProbe {
                pid: 2,
                elem: crate::SENSOR_ID,
                pin: 1,
                kind: ProbeKind::V,
                r: None,
            },
        ],
        next_pid: 3,
        panels: vec![Panel {
            plid: 1,
            x0: x0 - 20.0,
            y0: y0 + 2.0,
            x1: x0 - 2.0,
            y1: y0 + 18.0,
            name: "DRIVE".into(),
        }],
        next_plid: 2,
        machine: MachineSpec::Hoist {
            rect: HOIST_RECT,
            state: Hoist::default(),
        },
        view: View {
            home: Some([x0 - 24.0, y0 - 4.0, x1 + 4.0, y1 + 4.0]),
            scopes: vec![json!({
                "x": x0 - 20.0, "y": y1 - 2.0, "w": 18.0, "h": 9.0,
                "pids": [1, 2], "timebase": 0.5
            })],
        },
        ..RoomSetup::default()
    }
}

/// Directory templates live in. Env-free: `main` passes it down so tests can
/// use their own directory without racing on a process-global.
pub fn template_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// `^[a-z0-9][a-z0-9-]{0,31}$`. Also the only thing standing between a
/// template id and the filesystem, so it must reject `..`, `/` and friends
/// by construction rather than by inspection.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    if b.is_empty() || b.len() > 32 {
        return false;
    }
    if !(b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return false;
    }
    b.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn read_file(dir: &Path, id: &str) -> Option<SaveFile> {
    if !valid_id(id) {
        return None;
    }
    let path = template_path(dir, id);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_TEMPLATE_BYTES {
        return None;
    }
    let data = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<SaveFile>(&data) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!("template {id} does not parse, ignoring: {e}");
            None
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ResolveErr {
    /// No such template — and a file that does not parse shadows nothing.
    Missing,
    /// The template exists but does not describe a valid room. Carries the
    /// reason so the create call can say WHY instead of "no".
    Bad(&'static str),
}

/// Resolve a template id to a room setup. Files shadow built-ins.
pub fn resolve(dir: &Path, id: &str) -> Result<RoomSetup, ResolveErr> {
    if let Some(save) = read_file(dir, id) {
        return save
            .into_setup()
            .fresh_for_template()
            .normalize()
            .map_err(ResolveErr::Bad);
    }
    let b = BUILTINS
        .iter()
        .find(|b| b.id == id)
        .ok_or(ResolveErr::Missing)?;
    (b.build)().normalize().map_err(ResolveErr::Bad)
}

/// What `GET /api/templates` reports for one template.
#[derive(Serialize)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub blurb: String,
    /// "builtin" | "file". A file id shadowing a built-in reports "file".
    pub source: &'static str,
    pub parts: usize,
    pub panels: usize,
    pub probes: usize,
    pub scopes: usize,
    pub machine: &'static str,
}

fn info(id: &str, name: &str, blurb: &str, source: &'static str, s: &RoomSetup) -> TemplateInfo {
    TemplateInfo {
        id: id.into(),
        name: name.into(),
        blurb: blurb.into(),
        source,
        parts: s.elements.len(),
        panels: s.panels.len(),
        probes: s.probes.len(),
        scopes: s.view.scopes.len(),
        machine: s.machine.kind_name(),
    }
}

/// Built-ins and files as ONE list, files shadowing built-ins. Rescans the
/// directory on every call, so a dropped-in template shows up without a
/// restart.
pub fn list(dir: &Path) -> Vec<TemplateInfo> {
    let mut out: Vec<TemplateInfo> = Vec::new();
    let mut from_file: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            if out.len() >= MAX_FILE_TEMPLATES {
                break;
            }
            let name = ent.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if !valid_id(id) {
                continue;
            }
            let Some(save) = read_file(dir, id) else {
                continue;
            };
            let label = if save.name.is_empty() {
                id.to_string()
            } else {
                save.name.clone()
            };
            let blurb = save.blurb.clone();
            let Ok(setup) = save.into_setup().normalize() else {
                continue;
            };
            out.push(info(id, &label, &blurb, "file", &setup));
            from_file.push(id.to_string());
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let mut builtins: Vec<TemplateInfo> = BUILTINS
        .iter()
        .filter(|b| !from_file.iter().any(|f| f == b.id))
        .filter_map(|b| {
            (b.build)()
                .normalize()
                .ok()
                .map(|s| info(b.id, b.name, b.blurb, "builtin", &s))
        })
        .collect();
    builtins.append(&mut out);
    builtins
}

pub fn is_builtin(id: &str) -> bool {
    BUILTINS.iter().any(|b| b.id == id)
}

pub fn file_exists(dir: &Path, id: &str) -> bool {
    valid_id(id) && template_path(dir, id).is_file()
}

/// How many template files the directory holds — the budget `MAX_FILE_TEMPLATES`
/// is checked against before writing a new one.
pub fn file_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_suffix(".json")
                        .map(valid_id)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

/// Write a template file. Same tmp+rename as a room checkpoint, so a
/// half-written template is never visible in the listing.
pub fn write(
    dir: &Path,
    id: &str,
    name: &str,
    blurb: &str,
    setup: &RoomSetup,
) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bad template id",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let save = SaveFile::from_setup(setup)
        .with_identity("template", id, name)
        .with_blurb(blurb);
    let json = serde_json::to_string(&save)?;
    let path = template_path(dir, id);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

pub fn delete(dir: &Path, id: &str) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bad template id",
        ));
    }
    std::fs::remove_file(template_path(dir, id))
}

impl RoomSetup {
    /// Strip a live room's run state so it can be shipped as a template:
    /// no accumulated thermal stress or broken parts, and the mechanism
    /// re-armed. Everything a level IS (parts, panels, channels, where the
    /// cabinet stands) survives.
    pub fn fresh_for_template(mut self) -> RoomSetup {
        self.damage = DamageModel::new();
        self.machine = self.machine.fresh();
        self
    }
}
