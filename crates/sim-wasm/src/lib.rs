//! wasm-bindgen facade over sim-core for the browser client.
//!
//! Frame data crosses the boundary as a flat Float32Array (id, va, vb,
//! current, power per element) so the render loop never touches serde.

use sim_core::{ElementSpec, Engine, InteractOp};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Sim {
    engine: Engine,
    frame_buf: Vec<f32>,
}

/// Flat frame layout per element: [id, npins, v0..v5, i0..i5, power].
pub const FRAME_STRIDE: usize = 15;
/// Keep the stride honest when MAX_PINS moves (the TS mirror in
/// `circuit.ts` and the server's frame array must be bumped with it).
const _: () = assert!(FRAME_STRIDE == 3 + 2 * sim_core::MAX_PINS);

#[wasm_bindgen]
impl Sim {
    #[wasm_bindgen(constructor)]
    pub fn new(dt: f64) -> Sim {
        Sim {
            engine: Engine::new(dt),
            frame_buf: Vec::new(),
        }
    }

    /// Replace the document. `specs` is the JSON array of ElementSpec.
    #[wasm_bindgen(js_name = setElements)]
    pub fn set_elements(&mut self, specs: JsValue) -> Result<(), JsValue> {
        let specs: Vec<ElementSpec> =
            serde_wasm_bindgen::from_value(specs).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.engine.set_elements(&specs);
        Ok(())
    }

    /// Run up to `max_steps` fixed-dt substeps; returns steps taken.
    pub fn advance(&mut self, max_steps: u32) -> u32 {
        self.engine.advance(max_steps).steps
    }

    pub fn interact(&mut self, id: u32, op: JsValue) -> Result<(), JsValue> {
        let op: InteractOp =
            serde_wasm_bindgen::from_value(op).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.engine.interact(id, op);
        Ok(())
    }

    /// Flat frame: [id, npins, v0..v5, i0..i5, power] * n, in document
    /// order.
    pub fn frame(&mut self) -> Vec<f32> {
        self.frame_buf.clear();
        for f in self.engine.frame() {
            self.frame_buf.push(f.id as f32);
            self.frame_buf.push(f.npins as f32);
            for v in f.v {
                self.frame_buf.push(v as f32);
            }
            for i in f.i {
                self.frame_buf.push(i as f32);
            }
            self.frame_buf.push(f.power as f32);
        }
        self.frame_buf.clone()
    }

    pub fn time(&self) -> f64 {
        self.engine.time()
    }

    #[wasm_bindgen(js_name = isQuarantined)]
    pub fn is_quarantined(&self) -> bool {
        self.engine.is_quarantined()
    }

    #[wasm_bindgen(js_name = stateHash)]
    pub fn state_hash(&self) -> u64 {
        self.engine.state_hash()
    }
}

/// Placement-time validation — the SAME implementation the server enforces
/// (`sim_core::check_document`), so the client can refuse an op before
/// sending it (and paint the placement ghost red) without ever disagreeing
/// with the authority. `specs` is the candidate document as a JSON array of
/// ElementSpec; `dt` is the sim timestep.
///
/// Returns `null` when the document is placeable, else
/// `{code, id, ids, hint}` — `code` machine-readable ("bad_value",
/// "collapsed_pins", "shorted_source", "conflicting_sources", "source_loop",
/// "will_not_converge", "unsolvable", "unsolvable_switched"), `id` the
/// primary offending element, `ids` EVERY implicated element (so the client
/// can flash both halves of a conflict, or the whole loop), `hint` a
/// sentence for the DRC callout.
#[wasm_bindgen(js_name = checkDocument)]
pub fn check_document(specs: JsValue, dt: f64) -> Result<JsValue, JsValue> {
    let specs = specs_in(specs)?;
    verdict(sim_core::check_document(&specs, dt))
}

/// [`check_document`] plus the shape rule — the gate for a document EDIT.
///
/// `before` is the document being replaced. Alone among the gate's rules the
/// shape rule cannot be decided from the candidate: it is a rule about the
/// CHANGE, so that a part which predates it may still be dragged, rotated
/// and flipped while nothing may become newly skewed. Same return shape as
/// `checkDocument` (with one more code, "skewed_part"), and the same Rust
/// behind it as the server's edit path.
#[wasm_bindgen(js_name = checkEdit)]
pub fn check_edit(before: JsValue, after: JsValue, dt: f64) -> Result<JsValue, JsValue> {
    let before = specs_in(before)?;
    let after = specs_in(after)?;
    verdict(sim_core::check_edit(&before, &after, dt))
}

fn specs_in(v: JsValue) -> Result<Vec<ElementSpec>, JsValue> {
    serde_wasm_bindgen::from_value(v).map_err(|e| JsValue::from_str(&e.to_string()))
}

fn verdict(r: Result<(), sim_core::Reject>) -> Result<JsValue, JsValue> {
    #[derive(serde::Serialize)]
    struct RejectOut {
        code: &'static str,
        id: Option<u32>,
        ids: Vec<u32>,
        hint: String,
    }
    match r {
        Ok(()) => Ok(JsValue::NULL),
        Err(r) => serde_wasm_bindgen::to_value(&RejectOut {
            code: r.code(),
            id: r.id(),
            ids: r.ids().iter().collect(),
            hint: r.hint(),
        })
        .map_err(|e| JsValue::from_str(&e.to_string())),
    }
}

// ------------------------------------------------------- rigid part shapes
//
// The geometry of multi-pin parts is `sim_core::shape`, reached from here so
// the client has no second copy of it. A second copy would be a second set of
// rounding rules, and they diverge on the first odd number — which is exactly
// how the old TypeScript `makePins` came to place a pot's wiper on a
// different grid unit depending on which way the drag went.
//
// Pins cross as a flat Int32Array (x, y, x, y, ...) rather than as JSON: no
// serde on a path the pointer runs through 60 times a second, and the client
// already thinks in flat frame buffers.

fn pins_in(flat: &[i32]) -> Vec<sim_core::Point> {
    flat.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

fn pins_out(pins: &[sim_core::Point]) -> Vec<i32> {
    pins.iter().flat_map(|p| [p.0, p.1]).collect()
}

/// The canonical pin layout for a part of document tag `t` dragged from
/// (`ax`, `ay`) to (`bx`, `by`). Axis-snapped: a diagonal drag gives an
/// axis-aligned part, never a skewed one.
#[wasm_bindgen(js_name = partPins)]
pub fn part_pins(t: &str, ax: i32, ay: i32, bx: i32, by: i32) -> Vec<i32> {
    pins_out(&sim_core::shape::canonical_pins(
        sim_core::Shape::for_tag(t),
        (ax, ay),
        (bx, by),
    ))
}

/// Is this pin list a legal placement of `t` — the canonical layout under a
/// rotation and an optional mirror? Two-pin and one-pin parts are always
/// legal. This is the SAME predicate the placement gate enforces.
#[wasm_bindgen(js_name = partIsRigid)]
pub fn part_is_rigid(t: &str, pins: &[i32]) -> bool {
    let shape = sim_core::Shape::for_tag(t);
    !shape.is_rigid_family() || sim_core::shape::decompose(shape, &pins_in(pins)).is_some()
}

/// Snap a skewed pin list back into formation, keeping its orientation,
/// handedness and rough size. The identity on a part already in formation.
#[wasm_bindgen(js_name = partStraighten)]
pub fn part_straighten(t: &str, pins: &[i32]) -> Vec<i32> {
    pins_out(&sim_core::shape::straighten(
        sim_core::Shape::for_tag(t),
        &pins_in(pins),
    ))
}

/// Drag terminal `k` of a rigid part to (`cx`, `cy`) and give back the WHOLE
/// part: the reshape gesture, in the same code the gate judges. Empty when
/// the drag changes nothing (or the part is not a rigid family).
///
/// The client has no other way to author a multi-pin placement, which is what
/// makes "the client cannot draw a skewed part" a structural fact rather than
/// a convention.
#[wasm_bindgen(js_name = partReshape)]
pub fn part_reshape(t: &str, pins: &[i32], k: usize, cx: i32, cy: i32) -> Vec<i32> {
    let shape = sim_core::Shape::for_tag(t);
    let pins = pins_in(pins);
    match sim_core::shape::reshape_shape(shape, &pins, k, (cx, cy)) {
        Some(out) => pins_out(&out),
        None => Vec::new(),
    }
}

/// The grid point a reshape of terminal `k` turns about — the far end of
/// the part, which stays exactly where it is while the rest swings. Empty
/// for a terminal that carries the part instead of reorienting it, and for
/// parts that are not a rigid family.
///
/// The client draws it. A rigid swing moves EVERY pin (the perpendicular
/// offsets turn with the axis), so "which pins did not move" is not a
/// pivot — this is, and it is the one point the gesture promises to hold
/// still.
#[wasm_bindgen(js_name = partPivot)]
pub fn part_pivot(t: &str, pins: &[i32], k: usize) -> Vec<i32> {
    let shape = sim_core::Shape::for_tag(t);
    let pins = pins_in(pins);
    if !shape.is_rigid_family() || pins.len() != shape.pins() || k >= pins.len() {
        return Vec::new();
    }
    let Some(pl) = sim_core::shape::decompose(shape, &sim_core::shape::straighten(shape, &pins))
    else {
        return Vec::new();
    };
    match sim_core::shape::handle(shape, k) {
        sim_core::Handle::Tip => vec![pl.origin.0, pl.origin.1],
        sim_core::Handle::Anchor => vec![pl.tip().0, pl.tip().1],
        sim_core::Handle::Body => Vec::new(),
    }
}

/// The sentence the gate would print if this part were skewed — used by the
/// client when it straightens a legacy part, so the editor says the same
/// thing about rigid symbols wherever the subject comes up.
#[wasm_bindgen(js_name = partRigidHint)]
pub fn part_rigid_hint(t: &str) -> String {
    sim_core::rigid_hint(t)
}

/// What dragging terminal `k` does: 0 = nothing special (free part), 1 =
/// reorient/resize about the far end, 2 = carry the whole part. The client
/// uses it for the hover cursor and for the undo entry's name, so the
/// gesture announces itself before the pointer moves and is still called the
/// right thing afterwards.
#[wasm_bindgen(js_name = partHandle)]
pub fn part_handle(t: &str, k: usize) -> u8 {
    let shape = sim_core::Shape::for_tag(t);
    if !shape.is_rigid_family() || k >= shape.pins() {
        return 0;
    }
    match sim_core::shape::handle(shape, k) {
        sim_core::Handle::Anchor | sim_core::Handle::Tip => 1,
        sim_core::Handle::Body => 2,
    }
}

/// S1 harness: identical output format to `sim-golden`'s native `hash` bin.
#[cfg(feature = "golden")]
#[wasm_bindgen(js_name = goldenHash)]
pub fn golden_hash(name: &str, steps: u32) -> String {
    let Some((_, elems)) = sim_golden::all_golden()
        .into_iter()
        .find(|(n, _)| *n == name)
    else {
        return format!("{name} UNKNOWN");
    };
    let mut eng = Engine::new(1e-6);
    eng.set_elements(&elems);
    let report = eng.advance(steps);
    format!(
        "{name} {:016x} steps={} quarantined={}",
        eng.state_hash(),
        report.steps,
        eng.is_quarantined()
    )
}

/// The golden circuit names, for the harness driver.
#[cfg(feature = "golden")]
#[wasm_bindgen(js_name = goldenNames)]
pub fn golden_names() -> Vec<String> {
    sim_golden::all_golden()
        .iter()
        .map(|(n, _)| n.to_string())
        .collect()
}
