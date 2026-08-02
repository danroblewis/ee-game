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
    #[derive(serde::Serialize)]
    struct RejectOut {
        code: &'static str,
        id: Option<u32>,
        ids: Vec<u32>,
        hint: String,
    }
    let specs: Vec<ElementSpec> =
        serde_wasm_bindgen::from_value(specs).map_err(|e| JsValue::from_str(&e.to_string()))?;
    match sim_core::check_document(&specs, dt) {
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
