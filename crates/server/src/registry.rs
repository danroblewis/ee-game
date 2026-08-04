//! The room registry: N rooms in one process, each with its own Engine, its
//! own sim task, its own file on disk.
//!
//! Lifecycle (the plan's words): create -> active -> parked -> resumed ->
//! evicted. A parked room has no sim task at all — the last player leaving
//! stops the clock and writes a checkpoint. That is what makes many rooms
//! cheap, and it is also the design rule ("the sim pauses in empty rooms").
//!
//! The command channel OUTLIVES the sim task: parking hands the RECEIVER
//! back into the handle, so every `room.cmds.send(...)` site in the session
//! loop is unchanged and a command sent to a parked room simply queues until
//! the room resumes. That is why `struct Room` needed no new fields.

use crate::templates::{self, MachineSpec, RoomSetup, View};
use crate::{ensure_fixture, Cmd, Probe, Room, SaveFile};
use damage::DamageModel;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, watch};

/// Rooms one process will hold. The real ceiling is the solver (every LIVE
/// room owns a dense-LU Engine and a 30 Hz task), so this is a guard against
/// a runaway creator, not a design limit: parked rooms cost a struct and a
/// file.
pub const MAX_ROOMS: usize = 64;

/// Ticks a room stays live with nobody in it before parking (30 Hz -> 30 s).
/// Long enough that a reconnect or a room switch does not pay a cold start,
/// short enough that an abandoned room stops burning a core.
pub const PARK_AFTER_TICKS: u32 = 900;

/// Room-code alphabet: no 0/O, 1/I/L, U — a code gets read aloud and typed
/// into a URL.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LEN: usize = 6;

pub fn valid_code(s: &str) -> bool {
    s.len() == CODE_LEN && s.bytes().all(|c| ALPHABET.contains(&c))
}

pub const MAX_ROOM_NAME: usize = 40;

/// Room names are chrome, so the only rules are "printable" and "bounded".
pub fn clean_room_name(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ROOM_NAME)
        .collect();
    s.trim().to_string()
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Identity and provenance. `template` is provenance only — a room is never
/// re-linked to the template it came from.
#[derive(Clone, Debug, Serialize)]
pub struct RoomMeta {
    pub id: String,
    pub name: String,
    pub template: String,
    pub created: u64,
    pub played: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Life {
    Live,
    Parked,
    Gone,
}

/// Everything the sim task owns as locals while it runs, handed back when it
/// parks so a resume is nothing but a re-spawn.
pub struct Parked {
    pub rx: mpsc::UnboundedReceiver<Cmd>,
    pub machine: MachineSpec,
    pub damage: DamageModel,
}

pub struct RoomHandle {
    /// The shared mirror every session talks to. Deliberately the SAME struct
    /// the single-room server had.
    pub room: Arc<Room>,
    pub meta: Mutex<RoomMeta>,
    pub view: Mutex<View>,
    /// Mirror of the machine spec (kind + footprint + last checkpointed
    /// state). The sim task is authoritative while the room is live and
    /// refreshes this before every checkpoint; the lobby reads it to save a
    /// room as a template without going through the sim.
    pub machine: Mutex<MachineSpec>,
    /// Fixed for the room's life: whether it has a machine at all. Drives the
    /// `machine` flag in `hello` and the per-tick telemetry.
    pub has_machine: bool,
    pub life: watch::Sender<Life>,
    pub parked: Mutex<Option<Parked>>,
    /// Set the moment the room is deleted: stops a racing checkpoint from
    /// resurrecting the file the delete just removed.
    pub deleted: AtomicBool,
    pub path: PathBuf,
}

impl RoomHandle {
    pub fn meta(&self) -> RoomMeta {
        self.meta.lock().unwrap().clone()
    }

    pub fn players(&self) -> u32 {
        self.room.population.load(Ordering::Relaxed)
    }

    pub fn is_live(&self) -> bool {
        *self.life.borrow() == Life::Live
    }

    pub fn subscribe_life(&self) -> watch::Receiver<Life> {
        self.life.subscribe()
    }

    /// The room as a save file: the document mirror plus identity plus
    /// whatever mechanical/thermal state the caller holds.
    pub fn snapshot(&self, machine: &MachineSpec, damage: &DamageModel) -> SaveFile {
        let meta = self.meta();
        let setup = RoomSetup {
            elements: self.room.elements.lock().unwrap().clone(),
            probes: self
                .room
                .probes
                .lock()
                .unwrap()
                .iter()
                .map(|p| p.saved())
                .collect(),
            next_pid: self.room.next_pid.load(Ordering::Relaxed),
            panels: self.room.panels.lock().unwrap().clone(),
            next_plid: self.room.next_plid.load(Ordering::Relaxed),
            scopes: Some(self.room.scopes.lock().unwrap().clone()),
            next_sid: self.room.next_sid.load(Ordering::Relaxed),
            label_boxes: self.room.label_boxes.lock().unwrap().clone(),
            next_blid: self.room.next_blid.load(Ordering::Relaxed),
            net_labels: self.room.net_labels.lock().unwrap().clone(),
            next_nlid: self.room.next_nlid.load(Ordering::Relaxed),
            layers: self.room.layers.lock().unwrap().clone(),
            next_lid: self.room.next_lid.load(Ordering::Relaxed),
            machine: *machine,
            view: self.view.lock().unwrap().clone(),
            damage: damage.clone(),
            ext: self.room.ext.load(Ordering::Relaxed),
        };
        let mut save = SaveFile::from_setup(&setup).with_identity("room", &meta.id, &meta.name);
        save.template = meta.template;
        save.created = meta.created;
        save.played = meta.played;
        save
    }

    /// Write this room's checkpoint. tmp + rename, so a torn write is never
    /// visible; a deleted room writes nothing at all.
    pub fn checkpoint(&self, machine: &MachineSpec, damage: &DamageModel) {
        if self.deleted.load(Ordering::SeqCst) {
            return;
        }
        *self.machine.lock().unwrap() = *machine;
        let save = self.snapshot(machine, damage);
        let Ok(json) = serde_json::to_string(&save) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// A template built from this room right now: the level, without the
    /// playthrough (see `RoomSetup::fresh_for_template`). `view` overrides
    /// the room's stored view — the saving client's camera and scopes are
    /// client state, so they arrive in the request body.
    pub fn as_template_setup(&self, view: Option<View>) -> RoomSetup {
        let machine = *self.machine.lock().unwrap();
        RoomSetup {
            // A TEMPLATE IS A STARTING POINT, NOT A PLAYTHROUGH. Saving a room
            // that happened to be driven from outside must not brand every
            // room later made from it.
            ext: false,
            elements: self.room.elements.lock().unwrap().clone(),
            probes: self
                .room
                .probes
                .lock()
                .unwrap()
                .iter()
                .map(|p| p.saved())
                .collect(),
            next_pid: self.room.next_pid.load(Ordering::Relaxed),
            panels: self.room.panels.lock().unwrap().clone(),
            next_plid: self.room.next_plid.load(Ordering::Relaxed),
            scopes: Some(self.room.scopes.lock().unwrap().clone()),
            next_sid: self.room.next_sid.load(Ordering::Relaxed),
            label_boxes: self.room.label_boxes.lock().unwrap().clone(),
            next_blid: self.room.next_blid.load(Ordering::Relaxed),
            net_labels: self.room.net_labels.lock().unwrap().clone(),
            next_nlid: self.room.next_nlid.load(Ordering::Relaxed),
            layers: self.room.layers.lock().unwrap().clone(),
            next_lid: self.room.next_lid.load(Ordering::Relaxed),
            machine,
            view: view
                .unwrap_or_else(|| self.view.lock().unwrap().clone())
                .sane(),
            damage: DamageModel::new(),
        }
        .fresh_for_template()
    }
}

/// What `GET /api/rooms` reports for one room.
#[derive(Serialize)]
pub struct RoomInfo {
    pub id: String,
    pub name: String,
    pub template: String,
    pub parts: usize,
    pub players: u32,
    pub live: bool,
    pub machine: bool,
    pub created: u64,
    pub played: u64,
}

pub struct Registry {
    pub dir: PathBuf,
    pub tdir: PathBuf,
    rooms: Mutex<BTreeMap<String, Arc<RoomHandle>>>,
    seq: AtomicU64,
    /// Room joined by a socket that named no room: the most recently played
    /// one (a freshly created room counts as played). So a bare `/ws` — the
    /// URL the single-room server answered — lands you back where you last
    /// were, and every other room is reached by its code.
    default_code: Mutex<Option<String>>,
}

#[derive(Debug, PartialEq)]
pub enum CreateErr {
    BadName,
    NoTemplate,
    BadTemplate(&'static str),
    TooMany,
    Io,
}

impl Registry {
    /// Open (or create) a rooms directory and load every room in it, all
    /// parked. A file that does not parse is LOGGED AND KEPT, never replaced
    /// with a fresh demo room — losing one room's world to a parse slip is
    /// bad enough without doing it silently.
    pub fn open(dir: impl Into<PathBuf>, tdir: impl Into<PathBuf>) -> Arc<Registry> {
        let dir = dir.into();
        let tdir = tdir.into();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::create_dir_all(&tdir);
        let reg = Arc::new(Registry {
            dir: dir.clone(),
            tdir,
            rooms: Mutex::new(BTreeMap::new()),
            seq: AtomicU64::new(0),
            default_code: Mutex::new(None),
        });

        let mut loaded: Vec<(String, SaveFile)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                let Some(code) = name.strip_suffix(".json") else {
                    continue;
                };
                if !valid_code(code) {
                    continue;
                }
                let path = ent.path();
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|d| serde_json::from_str::<SaveFile>(&d).ok())
                {
                    Some(s) => loaded.push((code.to_string(), s)),
                    None => tracing::error!("room file {path:?} does not parse — left alone"),
                }
            }
        }
        loaded.sort_by_key(|(_, s)| std::cmp::Reverse(s.played));
        loaded.truncate(MAX_ROOMS);
        for (code, save) in loaded {
            let meta = RoomMeta {
                id: code.clone(),
                name: if save.name.is_empty() {
                    code.clone()
                } else {
                    save.name.clone()
                },
                template: if save.template.is_empty() {
                    "demo".into()
                } else {
                    save.template.clone()
                },
                created: if save.created == 0 {
                    now_secs()
                } else {
                    save.created
                },
                played: save.played,
            };
            let Ok(setup) = save.into_setup().normalize() else {
                tracing::error!("room {code} failed validation — skipped");
                continue;
            };
            let handle = build_handle(reg.dir.join(format!("{code}.json")), meta, setup);
            reg.rooms.lock().unwrap().insert(code, handle);
        }
        reg.refresh_default();
        reg
    }

    /// Migrate a pre-rooms single-file world (`EE_SAVE`) into the rooms
    /// directory, once, in place: the owner's live world survives the
    /// upgrade with no flag and no manual step.
    pub fn import_legacy(self: &Arc<Self>, legacy: &Path) -> Option<String> {
        if !self.rooms.lock().unwrap().is_empty() || !legacy.is_file() {
            return None;
        }
        // A legacy file that will not parse is LOUD and left alone. The old
        // server swallowed that error and handed the player a fresh demo
        // room, which looks exactly like "my world is gone".
        let save: SaveFile = match std::fs::read_to_string(legacy)
            .map_err(|e| e.to_string())
            .and_then(|d| serde_json::from_str(&d).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    "{legacy:?} exists but does not parse ({e}) — NOT imported and NOT touched; \
                     fix or move it, then restart"
                );
                return None;
            }
        };
        let parts = save.elements.len();
        let setup = save.into_setup().normalize().ok()?;
        let handle = self
            .insert_new("Main Room", "demo", setup)
            .ok()
            .map(|h| h.meta().id)?;
        let _ = std::fs::rename(legacy, legacy.with_extension("json.migrated"));
        tracing::info!("imported {legacy:?} ({parts} elements) as room {handle}");
        Some(handle)
    }

    /// Boot fallback: a server with no rooms at all gets one, from the
    /// default template.
    pub fn ensure_one(self: &Arc<Self>, template: &str) {
        if !self.rooms.lock().unwrap().is_empty() {
            return;
        }
        match self.create("Main Room", template) {
            Ok(h) => tracing::info!(
                "created first room {} from template {template}",
                h.meta().id
            ),
            Err(e) => tracing::error!("could not create the first room: {e:?}"),
        }
    }

    pub fn get(&self, code: &str) -> Option<Arc<RoomHandle>> {
        self.rooms.lock().unwrap().get(code).cloned()
    }

    /// Resolve the room a socket asked for. No code (or an empty one) lands
    /// in the default room; an unknown code is None, and the session tells
    /// the client so rather than dropping the socket on the floor.
    pub fn resolve(&self, code: Option<&str>) -> Option<Arc<RoomHandle>> {
        match code.map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => self.get(&c.to_ascii_uppercase()),
            None => self.default_room(),
        }
    }

    pub fn default_room(&self) -> Option<Arc<RoomHandle>> {
        let code = self.default_code.lock().unwrap().clone();
        code.and_then(|c| self.get(&c)).or_else(|| {
            self.refresh_default();
            let code = self.default_code.lock().unwrap().clone();
            code.and_then(|c| self.get(&c))
        })
    }

    /// Most recently played room, tie-broken by creation time.
    fn refresh_default(&self) {
        let rooms = self.rooms.lock().unwrap();
        let best = rooms
            .values()
            .max_by_key(|h| {
                let m = h.meta();
                (m.played, m.created)
            })
            .map(|h| h.meta().id);
        *self.default_code.lock().unwrap() = best;
    }

    pub fn list(&self) -> Vec<RoomInfo> {
        let mut out: Vec<RoomInfo> = self
            .rooms
            .lock()
            .unwrap()
            .values()
            .map(|h| {
                let m = h.meta();
                RoomInfo {
                    id: m.id,
                    name: m.name,
                    template: m.template,
                    parts: h.room.elements.lock().unwrap().len(),
                    players: h.players(),
                    live: h.is_live(),
                    machine: h.has_machine,
                    created: m.created,
                    played: m.played,
                }
            })
            .collect();
        out.sort_by(|a, b| b.played.cmp(&a.played).then(a.id.cmp(&b.id)));
        out
    }

    pub fn create(
        self: &Arc<Self>,
        name: &str,
        template: &str,
    ) -> Result<Arc<RoomHandle>, CreateErr> {
        // Validated BEFORE the room exists, so a hand-edited template file
        // can never produce a room that arrives broken — the create call
        // fails and says which invariant it broke.
        let setup = match templates::resolve(&self.tdir, template) {
            Ok(s) => s,
            Err(templates::ResolveErr::Missing) => return Err(CreateErr::NoTemplate),
            Err(templates::ResolveErr::Bad(why)) => return Err(CreateErr::BadTemplate(why)),
        };
        let setup = setup
            .fresh_for_template()
            .normalize()
            .map_err(CreateErr::BadTemplate)?;
        self.insert_new(name, template, setup)
    }

    fn insert_new(
        self: &Arc<Self>,
        name: &str,
        template: &str,
        setup: RoomSetup,
    ) -> Result<Arc<RoomHandle>, CreateErr> {
        let name = clean_room_name(name);
        if name.is_empty() {
            return Err(CreateErr::BadName);
        }
        let mut rooms = self.rooms.lock().unwrap();
        if rooms.len() >= MAX_ROOMS {
            return Err(CreateErr::TooMany);
        }
        let code = self.mint_code(&rooms);
        let now = now_secs();
        let meta = RoomMeta {
            id: code.clone(),
            name,
            template: template.to_string(),
            created: now,
            played: now,
        };
        let handle = build_handle(self.dir.join(format!("{code}.json")), meta, setup);
        // Write the file before publishing the room: a room that exists in
        // the map but not on disk would vanish on the next boot.
        {
            let machine = *handle.machine.lock().unwrap();
            let parked = handle.parked.lock().unwrap();
            let damage = parked
                .as_ref()
                .map(|p| p.damage.clone())
                .unwrap_or_default();
            handle.checkpoint(&machine, &damage);
        }
        if !handle.path.is_file() {
            return Err(CreateErr::Io);
        }
        rooms.insert(code.clone(), handle.clone());
        drop(rooms);
        *self.default_code.lock().unwrap() = Some(code);
        Ok(handle)
    }

    pub fn rename(&self, code: &str, name: &str) -> Option<Arc<RoomHandle>> {
        let name = clean_room_name(name);
        if name.is_empty() {
            return None;
        }
        let h = self.get(code)?;
        h.meta.lock().unwrap().name = name.clone();
        // Live rooms checkpoint on the dirty flag; a parked room has no task
        // to do it, so write here.
        h.room
            .dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        {
            let parked = h.parked.lock().unwrap();
            if let Some(p) = parked.as_ref() {
                let machine = *h.machine.lock().unwrap();
                h.checkpoint(&machine, &p.damage);
            }
        }
        let _ = h
            .room
            .events
            .send(serde_json::json!({"t": "roommeta", "id": code, "name": name}).to_string());
        Some(h)
    }

    /// Evict a room: no new joins, everyone inside told why, the sim task
    /// stopped without a checkpoint, the file removed. Order matters — a
    /// checkpoint racing the unlink would resurrect the room.
    pub fn delete(&self, code: &str) -> bool {
        let Some(h) = self.rooms.lock().unwrap().remove(code) else {
            return false;
        };
        h.deleted.store(true, Ordering::SeqCst);
        h.life.send_replace(Life::Gone);
        let _ = h.room.cmds.send(Cmd::Stop { checkpoint: false });
        let _ = std::fs::remove_file(&h.path);
        let _ = std::fs::remove_file(h.path.with_extension("json.tmp"));
        self.refresh_default();
        true
    }

    /// A player is entering: count them, and resume the room if it was
    /// parked. Both halves happen under the SAME lock the parking path
    /// takes, which is what closes the "task parks while a player joins"
    /// race.
    pub fn enter(&self, h: &Arc<RoomHandle>) {
        let mut slot = h.parked.lock().unwrap();
        h.room.population.fetch_add(1, Ordering::SeqCst);
        if h.deleted.load(Ordering::SeqCst) {
            return;
        }
        if let Some(p) = slot.take() {
            h.meta.lock().unwrap().played = now_secs();
            h.life.send_replace(Life::Live);
            tokio::spawn(crate::sim_task(h.clone(), p));
        }
    }

    pub fn leave(&self, h: &Arc<RoomHandle>) {
        h.room.population.fetch_sub(1, Ordering::SeqCst);
        h.meta.lock().unwrap().played = now_secs();
    }

    /// Every live room, for graceful shutdown.
    pub fn all(&self) -> Vec<Arc<RoomHandle>> {
        self.rooms.lock().unwrap().values().cloned().collect()
    }

    /// Six characters, unique in this registry. Not cryptographic — it is a
    /// join code, and a collision is retried rather than tolerated.
    fn mint_code(&self, rooms: &BTreeMap<String, Arc<RoomHandle>>) -> String {
        use std::hash::{Hash, Hasher};
        for _ in 0..64 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
                .hash(&mut h);
            self.seq.fetch_add(1, Ordering::Relaxed).hash(&mut h);
            (self as *const Registry as usize).hash(&mut h);
            let mut n = h.finish();
            let mut code = String::with_capacity(CODE_LEN);
            for _ in 0..CODE_LEN {
                code.push(ALPHABET[(n % ALPHABET.len() as u64) as usize] as char);
                n /= ALPHABET.len() as u64;
            }
            if !rooms.contains_key(&code) && !self.dir.join(format!("{code}.json")).exists() {
                return code;
            }
        }
        // 30^6 codes and 64 rooms: unreachable in practice, but a server must
        // not loop forever if the clock is frozen.
        format!("Z{:05}", rooms.len())
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

/// Build a room (parked, sim task not running) from a setup.
pub fn build_handle(path: PathBuf, meta: RoomMeta, setup: RoomSetup) -> Arc<RoomHandle> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, _) = broadcast::channel(256);
    let RoomSetup {
        ext,
        mut elements,
        probes,
        next_pid,
        panels,
        next_plid,
        scopes,
        next_sid,
        label_boxes,
        next_blid,
        net_labels,
        next_nlid,
        layers,
        next_lid,
        machine,
        view,
        damage,
    } = setup;
    // Fixture seeding is PER ROOM and per template: only a room whose
    // template declares a machine gets ids 900-999 stood up in it. A
    // machineless room (sandbox, a synth world) keeps that id range free.
    if let MachineSpec::Hoist { rect, .. } = machine {
        ensure_fixture(&mut elements, rect);
    }
    let room = Arc::new(Room {
        cmds: cmd_tx,
        events: event_tx,
        elements: Mutex::new(elements),
        probes: Mutex::new(probes.iter().map(Probe::from_saved).collect()),
        panels: Mutex::new(panels),
        // `normalize` has already resolved seed-vs-stored, so an unresolved
        // `None` this deep is a setup that skipped it: an empty bench is the
        // safe answer (never a re-seed the players did not ask for).
        scopes: Mutex::new(scopes.unwrap_or_default()),
        label_boxes: Mutex::new(label_boxes),
        net_labels: Mutex::new(net_labels),
        layers: Mutex::new(layers),
        // Never restored from anywhere: a fresh room has nobody looking.
        claims: Mutex::new(Vec::new()),
        chat: Mutex::new(std::collections::VecDeque::new()),
        next_client: AtomicU32::new(1),
        next_pid: AtomicU32::new(next_pid.max(1)),
        next_plid: AtomicU32::new(next_plid.max(1)),
        next_sid: AtomicU32::new(next_sid.max(1)),
        next_blid: AtomicU32::new(next_blid.max(1)),
        next_nlid: AtomicU32::new(next_nlid.max(1)),
        next_lid: AtomicU32::new(next_lid.max(1)),
        population: AtomicU32::new(0),
        dirty: AtomicBool::new(false),
        ext: AtomicBool::new(ext),
    });
    let (life, _) = watch::channel(Life::Parked);
    Arc::new(RoomHandle {
        room,
        meta: Mutex::new(meta),
        view: Mutex::new(view),
        machine: Mutex::new(machine),
        has_machine: machine.is_some(),
        life,
        parked: Mutex::new(Some(Parked {
            rx: cmd_rx,
            machine,
            damage,
        })),
        deleted: AtomicBool::new(false),
        path,
    })
}
