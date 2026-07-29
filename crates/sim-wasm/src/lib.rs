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

pub const FRAME_STRIDE: usize = 5;

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

    /// Flat frame: [id, va, vb, current, power] * n, in document order.
    pub fn frame(&mut self) -> Vec<f32> {
        self.frame_buf.clear();
        for f in self.engine.frame() {
            self.frame_buf.push(f.id as f32);
            self.frame_buf.push(f.va as f32);
            self.frame_buf.push(f.vb as f32);
            self.frame_buf.push(f.current as f32);
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

/// S1 harness: identical output format to `sim-golden`'s native `hash` bin.
#[cfg(feature = "golden")]
#[wasm_bindgen(js_name = goldenHash)]
pub fn golden_hash(name: &str, steps: u32) -> String {
    let elems = match name {
        "demo_lamp" => sim_golden::demo_lamp(true),
        "rc_step" => sim_golden::rc_step(),
        "rl_step" => sim_golden::rl_step(),
        "rlc_ring" => sim_golden::rlc_ring(),
        "half_wave_rectifier" => sim_golden::half_wave_rectifier(),
        _ => return format!("{name} UNKNOWN"),
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
