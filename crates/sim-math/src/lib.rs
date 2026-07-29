//! Deterministic numeric kernels for the circuit simulator.
//!
//! Every operation here must produce bit-identical results on x86-64,
//! aarch64, and wasm32. That rules out: FMA (`mul_add`), SIMD, fast-math,
//! platform `libm` transcendentals, and any reduction whose order depends
//! on the target. Plain scalar f64 `+ - * /` are exact IEEE-754 and safe.

mod dense;

pub use dense::DenseLu;

/// Canonicalize a possibly-NaN value so state hashes are stable across
/// targets (NaN payloads are the one place IEEE-754 results may differ).
#[inline]
pub fn canon(x: f64) -> f64 {
    if x.is_nan() {
        f64::NAN // the compiler emits the canonical quiet NaN constant
    } else {
        x
    }
}
