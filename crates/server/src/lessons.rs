//! The intro series: ten lesson rooms that teach the electrical ideas the
//! games are built on, in order, by doing.
//!
//! A LESSON IS A TEMPLATE — a whole room setup (parts, probes, panels, seed
//! scopes, camera, machine) exactly like every other template, authored as
//! the same `SaveFile` JSON a room checkpoint uses and embedded into the
//! binary here so a bare `git clone` ships the course. The files live in
//! `crates/server/templates/`; drop an edited copy into `$EE_TEMPLATES` and
//! it shadows the shipped one without a rebuild (see `templates.rs`), which
//! is also the authoring loop: play a lesson room, fix it in place, "save as
//! template", copy the file back here.
//!
//! The curriculum (fixed — the games are built against it):
//!    1 loop        a circuit is a loop; current is a loop, not a fuel
//!    2 divider     Ohm's law; the pot/divider as the sensor you keep
//!    3 kirchhoff   in = out at a junction; drops share the push
//!    4 smoke       watts, ratings, the damage model (burn an LED, repair it)
//!    5 time        RC: charge on a curve, hold, dump
//!    6 diode       one-way current, fixed forward toll
//!    7 opamp       comparator first; then the honest 25 mA
//!    8 mosfet      free gate, real amps — switch the LOW side
//!    9 muscle      package watts, tiers: TO-92 vs TO-220
//!   10 close-loop  sense/compare/drive on the real hoist, one wire missing
//!
//! The client half lives in `packages/app/src/lesson.ts`: a lesson card
//! (goal-card language) keyed by the room's TEMPLATE id, whose step checks
//! read live solver output. Element ids in these files are therefore a
//! CONTRACT with lesson.ts — renumber here, renumber there.
//!
//! Element ids 900-999 appear only in lesson 10, which declares the hoist
//! that owns them (the fixture is listed so the armature/wiper probes
//! survive `normalize`; `ensure_fixture` re-derives the pins on load).
//!
//! The lessons lead the template list, so "new room" opens on lesson 1. To
//! make a server LAND new players in the course, set
//! `EE_DEFAULT_TEMPLATE=intro-01-loop` — the default room then IS lesson 1.

use crate::templates::RoomSetup;
use crate::SaveFile;

/// One embedded lesson file -> a room setup. Panics only on a corrupt
/// embedded asset, which `lessons_all_resolve` catches in CI before any
/// server ships it.
fn parse(id: &str, json: &'static str) -> RoomSetup {
    let save: SaveFile =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("embedded lesson {id}: {e}"));
    save.into_setup()
}

macro_rules! lesson {
    ($fn_name:ident, $id:literal) => {
        pub fn $fn_name() -> RoomSetup {
            parse($id, include_str!(concat!("../templates/", $id, ".json")))
        }
    };
}

lesson!(lesson_01, "intro-01-loop");
lesson!(lesson_02, "intro-02-divider");
lesson!(lesson_03, "intro-03-kirchhoff");
lesson!(lesson_04, "intro-04-smoke");
lesson!(lesson_05, "intro-05-time");
lesson!(lesson_06, "intro-06-diode");
lesson!(lesson_07, "intro-07-opamp");
lesson!(lesson_08, "intro-08-mosfet");
lesson!(lesson_09, "intro-09-muscle");
lesson!(lesson_10, "intro-10-close-the-loop");

#[cfg(test)]
mod tests {
    use crate::templates::{self, MachineSpec};

    /// Every lesson id, in course order.
    const LESSON_IDS: [&str; 10] = [
        "intro-01-loop",
        "intro-02-divider",
        "intro-03-kirchhoff",
        "intro-04-smoke",
        "intro-05-time",
        "intro-06-diode",
        "intro-07-opamp",
        "intro-08-mosfet",
        "intro-09-muscle",
        "intro-10-close-the-loop",
    ];

    /// The embedded JSON parses, normalizes, and keeps its instruments: a
    /// lesson that arrives with its probes dropped (dangling `elem`) or its
    /// scopes gone teaches nothing.
    #[test]
    fn lessons_all_resolve() {
        let empty = std::path::Path::new("/nonexistent-ee-templates");
        for id in LESSON_IDS {
            let setup = templates::resolve(empty, id)
                .unwrap_or_else(|e| panic!("lesson {id} does not resolve: {e:?}"));
            assert!(!setup.elements.is_empty(), "{id}: no parts");
            assert!(!setup.probes.is_empty(), "{id}: probes were dropped");
            assert!(!setup.view.scopes.is_empty(), "{id}: no seed scope");
            assert!(
                setup.view.home.is_some(),
                "{id}: no authored camera — the player would land nowhere"
            );
        }
    }

    /// Every lesson circuit is solvable EXACTLY AS SHIPPED: the placement
    /// gate must never refuse a room the course itself stood up.
    #[test]
    fn lessons_are_solvable_as_shipped() {
        let empty = std::path::Path::new("/nonexistent-ee-templates");
        for id in LESSON_IDS {
            let mut setup = templates::resolve(empty, id).unwrap();
            if let MachineSpec::Hoist { rect, .. } = setup.machine {
                crate::ensure_fixture(&mut setup.elements, rect);
            }
            if let Err(e) = sim_core::check_document(&setup.elements, crate::DT) {
                panic!("{id}: shipped circuit refused by the placement gate: {e:?}");
            }
        }
    }

    /// Only lesson 10 owns a machine, and it is the hoist with a fresh
    /// (un-won) state.
    #[test]
    fn only_the_finale_has_a_machine() {
        let empty = std::path::Path::new("/nonexistent-ee-templates");
        for id in LESSON_IDS {
            let setup = templates::resolve(empty, id).unwrap();
            match (id, setup.machine) {
                ("intro-10-close-the-loop", MachineSpec::Hoist { state, .. }) => {
                    assert!(!state.win, "{id}: template ships a won game");
                    assert_eq!(state.y, 0.0, "{id}: crate must start on the floor");
                }
                ("intro-10-close-the-loop", m) => panic!("{id}: expected a hoist, got {m:?}"),
                (_, MachineSpec::None) => {}
                (_, m) => panic!("{id}: unexpected machine {m:?}"),
            }
        }
    }

    /// The course is listed in order (ids sort lexically) and before the
    /// sandbox worlds, so "new room" opens on lesson 1.
    #[test]
    fn lessons_lead_the_template_list() {
        let empty = std::path::Path::new("/nonexistent-ee-templates");
        let list = templates::list(empty);
        let ids: Vec<&str> = list.iter().map(|t| t.id.as_str()).collect();
        for (k, id) in LESSON_IDS.iter().enumerate() {
            assert_eq!(ids[k], *id, "lesson {k} out of order in the listing");
        }
    }
}
