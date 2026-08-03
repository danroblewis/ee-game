//! CPU baselines (the authoritative sim-math f64 LU) and f64 accuracy
//! metrics. All truth/residual arithmetic here is f64 on the CPU — the
//! "refinement gate" of the design.

use rayon::prelude::*;
use sim_math::DenseLu;

/// Solve every system in the batch single-threaded, reusing one DenseLu
/// (matches how the engine's NR loop reuses its allocation).
pub fn solve_batch_single(n: usize, a: &[f64], b: &[f64], x: &mut [f64]) {
    let count = b.len() / n;
    x.copy_from_slice(b);
    let mut lu = DenseLu::new(n);
    for m in 0..count {
        let am = &a[m * n * n..(m + 1) * n * n];
        assert!(lu.factor(am), "singular test matrix");
        lu.solve(&mut x[m * n..(m + 1) * n]);
    }
}

/// Same, fanned across cores with rayon (the design's Phase-0 CPU story).
pub fn solve_batch_rayon(n: usize, a: &[f64], b: &[f64], x: &mut [f64]) {
    x.copy_from_slice(b);
    a.par_chunks(n * n)
        .zip(x.par_chunks_mut(n))
        .for_each_init(
            || DenseLu::new(n),
            |lu, (am, xm)| {
                assert!(lu.factor(am), "singular test matrix");
                lu.solve(xm);
            },
        );
}

pub struct Accuracy {
    /// max over batch of normwise ‖x − x_truth‖∞ / ‖x_truth‖∞
    pub max_rel_err: f64,
    /// max over batch of the normwise backward error
    /// ‖A·x − b‖∞ / (‖A‖∞·‖x‖∞ + ‖b‖∞)   (f64, CPU)
    pub max_residual: f64,
    /// matrices whose backward error exceeds the 1e-6 gate
    pub over_gate: usize,
}

pub const RESIDUAL_GATE: f64 = 1e-6;

pub fn residuals(n: usize, a: &[f64], b: &[f64], x: &[f64], truth: Option<&[f64]>) -> Accuracy {
    let count = b.len() / n;
    let mut max_rel_err = 0.0f64;
    let mut max_residual = 0.0f64;
    let mut over_gate = 0;
    for m in 0..count {
        let am = &a[m * n * n..(m + 1) * n * n];
        let bm = &b[m * n..(m + 1) * n];
        let xm = &x[m * n..(m + 1) * n];
        let mut rinf = 0.0f64;
        let mut binf = 0.0f64;
        let mut ainf = 0.0f64;
        let mut xinf = 0.0f64;
        for r in 0..n {
            let mut s = 0.0f64;
            let mut row_abs = 0.0f64;
            for c in 0..n {
                s += am[r * n + c] * xm[c];
                row_abs += am[r * n + c].abs();
            }
            rinf = rinf.max((s - bm[r]).abs());
            binf = binf.max(bm[r].abs());
            ainf = ainf.max(row_abs);
            xinf = xinf.max(xm[r].abs());
        }
        let res = rinf / (ainf * xinf + binf).max(f64::MIN_POSITIVE);
        max_residual = max_residual.max(res);
        if res > RESIDUAL_GATE {
            over_gate += 1;
        }
        if let Some(t) = truth {
            let tm = &t[m * n..(m + 1) * n];
            let mut dinf = 0.0f64;
            let mut tinf = 0.0f64;
            for r in 0..n {
                dinf = dinf.max((xm[r] - tm[r]).abs());
                tinf = tinf.max(tm[r].abs());
            }
            max_rel_err = max_rel_err.max(dinf / tinf.max(f64::MIN_POSITIVE));
        }
    }
    Accuracy {
        max_rel_err,
        max_residual,
        over_gate,
    }
}

/// r = b − A·x in f64. Returns the max normwise backward error alongside.
pub fn residual_vec(n: usize, a: &[f64], b: &[f64], x: &[f64], r: &mut [f64]) -> f64 {
    let count = b.len() / n;
    let mut worst = 0.0f64;
    for m in 0..count {
        let am = &a[m * n * n..(m + 1) * n * n];
        let bm = &b[m * n..(m + 1) * n];
        let xm = &x[m * n..(m + 1) * n];
        let rm = &mut r[m * n..(m + 1) * n];
        let mut rinf = 0.0f64;
        let mut binf = 0.0f64;
        let mut ainf = 0.0f64;
        let mut xinf = 0.0f64;
        for row in 0..n {
            let mut s = 0.0f64;
            let mut row_abs = 0.0f64;
            for c in 0..n {
                s += am[row * n + c] * xm[c];
                row_abs += am[row * n + c].abs();
            }
            rm[row] = bm[row] - s;
            rinf = rinf.max(rm[row].abs());
            binf = binf.max(bm[row].abs());
            ainf = ainf.max(row_abs);
            xinf = xinf.max(xm[row].abs());
        }
        worst = worst.max(rinf / (ainf * xinf + binf).max(f64::MIN_POSITIVE));
    }
    worst
}

/// CPU reference implementation of the WGSL df64 (double-single) LU, using
/// strict-IEEE Rust f32. Exists to separate "the df64 algorithm is wrong"
/// from "the Metal compiler's default fast-math destroyed the error-free
/// transformations" — Rust f32 arithmetic is never contracted or
/// reassociated, so this is what the GPU df64 kernel *should* produce.
/// (two_prod's exact error term is derived via f64 widening, which is
/// exact for f32 products; the GPU kernel uses fma() for the same thing.)
pub mod df64_ref {
    #[derive(Clone, Copy)]
    pub struct Df(pub f32, pub f32); // (hi, lo)

    pub fn from_f64(x: f64) -> Df {
        let hi = x as f32;
        Df(hi, (x - hi as f64) as f32)
    }
    pub fn to_f64(a: Df) -> f64 {
        a.0 as f64 + a.1 as f64
    }
    fn qts(a: f32, b: f32) -> Df {
        let s = a + b;
        Df(s, b - (s - a))
    }
    fn two_sum(a: f32, b: f32) -> Df {
        let s = a + b;
        let bb = s - a;
        Df(s, (a - (s - bb)) + (b - bb))
    }
    fn two_prod(a: f32, b: f32) -> Df {
        let p = a * b;
        // Exact product error via f64 widening (== what a fused fma yields).
        let e = (a as f64 * b as f64 - p as f64) as f32;
        Df(p, e)
    }
    pub fn add(a: Df, b: Df) -> Df {
        let s = two_sum(a.0, b.0);
        qts(s.0, s.1 + (a.1 + b.1))
    }
    pub fn sub(a: Df, b: Df) -> Df {
        add(a, Df(-b.0, -b.1))
    }
    pub fn mul(a: Df, b: Df) -> Df {
        let p = two_prod(a.0, b.0);
        qts(p.0, p.1 + (a.0 * b.1 + a.1 * b.0))
    }
    pub fn div(a: Df, b: Df) -> Df {
        let q1 = a.0 / b.0;
        let r1 = sub(a, mul(Df(q1, 0.0), b));
        let q2 = r1.0 / b.0;
        let r2 = sub(r1, mul(Df(q2, 0.0), b));
        let q3 = r2.0 / b.0;
        add(qts(q1, q2), Df(q3, 0.0))
    }

    /// Mirror of the WGSL factor_solve kernel, one matrix.
    pub fn lu_solve(n: usize, a64: &[f64], b64: &[f64]) -> Vec<f64> {
        let mut a: Vec<Df> = a64.iter().map(|&v| from_f64(v)).collect();
        let mut x: Vec<Df> = b64.iter().map(|&v| from_f64(v)).collect();
        for k in 0..n {
            let mut p = k;
            let mut pmax = a[k * n + k].0.abs();
            for r in (k + 1)..n {
                let v = a[r * n + k].0.abs();
                if v > pmax {
                    pmax = v;
                    p = r;
                }
            }
            if p != k {
                for c in 0..n {
                    a.swap(k * n + c, p * n + c);
                }
                x.swap(k, p);
            }
            let pivot = a[k * n + k];
            for r in (k + 1)..n {
                let mf = div(a[r * n + k], pivot);
                a[r * n + k] = mf;
                for c in (k + 1)..n {
                    a[r * n + c] = sub(a[r * n + c], mul(mf, a[k * n + c]));
                }
            }
        }
        for k in 0..n {
            let xk = x[k];
            for r in (k + 1)..n {
                x[r] = sub(x[r], mul(a[r * n + k], xk));
            }
        }
        for k in (0..n).rev() {
            let mut s = x[k];
            for c in (k + 1)..n {
                s = sub(s, mul(a[k * n + c], x[c]));
            }
            x[k] = div(s, a[k * n + k]);
        }
        x.into_iter().map(to_f64).collect()
    }
}
