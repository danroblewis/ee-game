//! Dense LU factorization with partial pivoting, written as plain scalar
//! loops so codegen is identical on every target (no FMA, no SIMD).
//!
//! Circuit MNA matrices are small (the dense path serves n < ~150; larger
//! islands move to the fixed-pattern sparse solver later). The API is
//! shaped for the Newton-Raphson loop: reuse the allocation, refactor in
//! place, solve against a single RHS.

/// In-place dense LU with partial pivoting over a row-major matrix.
pub struct DenseLu {
    n: usize,
    /// Row-major storage of the factored matrix (L below diagonal with
    /// implicit unit diagonal, U on and above).
    lu: Vec<f64>,
    /// piv[k] = row swapped into position k at elimination step k.
    piv: Vec<usize>,
    singular: bool,
}

/// Pivots with |pivot| below this are treated as singular. MNA matrices are
/// pre-conditioned by gmin so a healthy circuit never gets near this.
const PIVOT_TOL: f64 = 1e-30;

impl DenseLu {
    pub fn new(n: usize) -> Self {
        DenseLu {
            n,
            lu: vec![0.0; n * n],
            piv: vec![0; n],
            singular: false,
        }
    }

    pub fn n(&self) -> usize {
        self.n
    }

    /// Resize the workspace (topology changed). Contents become garbage
    /// until the next `factor`.
    pub fn resize(&mut self, n: usize) {
        self.n = n;
        self.lu.resize(n * n, 0.0);
        self.piv.resize(n, 0);
        self.singular = false;
    }

    /// Factor a row-major `n x n` matrix. Returns false if singular.
    pub fn factor(&mut self, a: &[f64]) -> bool {
        let n = self.n;
        debug_assert_eq!(a.len(), n * n);
        self.lu.copy_from_slice(a);
        self.singular = false;
        let lu = &mut self.lu;

        for k in 0..n {
            // Partial pivot: largest |value| in column k, rows k..n.
            let mut p = k;
            let mut pmax = abs(lu[k * n + k]);
            for r in (k + 1)..n {
                let v = abs(lu[r * n + k]);
                if v > pmax {
                    pmax = v;
                    p = r;
                }
            }
            self.piv[k] = p;
            if pmax < PIVOT_TOL {
                self.singular = true;
                return false;
            }
            if p != k {
                for c in 0..n {
                    lu.swap(k * n + c, p * n + c);
                }
            }
            let pivot = lu[k * n + k];
            for r in (k + 1)..n {
                let m = lu[r * n + k] / pivot;
                lu[r * n + k] = m;
                if m != 0.0 {
                    for c in (k + 1)..n {
                        lu[r * n + c] -= m * lu[k * n + c];
                    }
                }
            }
        }
        true
    }

    pub fn is_singular(&self) -> bool {
        self.singular
    }

    /// The stored factor, row-major (L below the diagonal with implicit unit
    /// diagonal, U on and above). Read-only instrumentation: the scale bench
    /// checks its op-counting mirror of `factor` against this bit-for-bit.
    pub fn factor_slice(&self) -> &[f64] {
        &self.lu
    }

    /// Row permutation as recorded by `factor`: `piv[k]` is the row swapped
    /// into position `k` at elimination step `k`. Read-only instrumentation.
    pub fn pivots(&self) -> &[usize] {
        &self.piv
    }

    /// Structural nonzeros in the stored factor (L below the diagonal, U on
    /// and above). Instrumentation only: fill-in against the original nnz is
    /// what decides whether a sparse solver pays off, and it is also why the
    /// dense factor of a block-diagonal (many-island) matrix costs a
    /// fraction of a connected one of the same size — the `m != 0.0` skip in
    /// `factor` walks straight past the empty blocks.
    pub fn factor_nnz(&self) -> usize {
        self.lu.iter().filter(|v| **v != 0.0).count()
    }

    /// Solve `A x = b` using the current factorization. `x` is `b` on entry
    /// and the solution on exit.
    pub fn solve(&self, x: &mut [f64]) {
        let n = self.n;
        debug_assert_eq!(x.len(), n);
        debug_assert!(!self.singular);
        let lu = &self.lu;

        // The factorization swapped whole rows (multipliers included), so
        // the stored L/U satisfy P·A = L·U: apply ALL of P to the RHS
        // first, then substitute cleanly.
        for k in 0..n {
            let p = self.piv[k];
            if p != k {
                x.swap(k, p);
            }
        }
        // Forward-substitute L (unit diagonal).
        for k in 0..n {
            let xk = x[k];
            if xk != 0.0 {
                for r in (k + 1)..n {
                    x[r] -= lu[r * n + k] * xk;
                }
            }
        }
        // Back-substitute U.
        for k in (0..n).rev() {
            let mut s = x[k];
            for c in (k + 1)..n {
                s -= lu[k * n + c] * x[c];
            }
            x[k] = s / lu[k * n + k];
        }
    }
}

/// `f64::abs` lowers to a bit-op everywhere, but keep it explicit and
/// branch-free so there is exactly one implementation to audit.
#[inline]
fn abs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & 0x7fff_ffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solve_dense(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
        let mut lu = DenseLu::new(n);
        assert!(lu.factor(a), "unexpected singular matrix");
        let mut x = b.to_vec();
        lu.solve(&mut x);
        x
    }

    #[test]
    fn solves_identity() {
        let a = [1.0, 0.0, 0.0, 1.0];
        let x = solve_dense(&a, &[3.0, -4.0], 2);
        assert_eq!(x, vec![3.0, -4.0]);
    }

    #[test]
    fn solves_3x3_exactly_enough() {
        // A * [1, -2, 3] with A chosen to need pivoting (zero on diagonal).
        let a = [0.0, 2.0, 1.0, 1.0, 0.0, 4.0, 5.0, 1.0, 0.0];
        let x_true = [1.0, -2.0, 3.0];
        let b: Vec<f64> = (0..3)
            .map(|r| (0..3).map(|c| a[r * 3 + c] * x_true[c]).sum())
            .collect();
        let x = solve_dense(&a, &b, 3);
        for (xi, ti) in x.iter().zip(x_true.iter()) {
            assert!((xi - ti).abs() < 1e-12, "{xi} vs {ti}");
        }
    }

    #[test]
    fn detects_singular() {
        let a = [1.0, 2.0, 2.0, 4.0];
        let mut lu = DenseLu::new(2);
        assert!(!lu.factor(&a));
        assert!(lu.is_singular());
    }

    /// Differential test against faer on random circuit-shaped systems.
    #[test]
    fn matches_faer_oracle() {
        use faer::prelude::*;
        // Deterministic xorshift so the test needs no RNG dependency.
        let mut s: u64 = 0x9e3779b97f4a7c15;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for trial in 0..400 {
            let n = 2 + (trial % 40);
            // Diagonally dominant like a gmin-conditioned MNA matrix...
            let mut a = vec![0.0f64; n * n];
            for r in 0..n {
                let mut row_abs = 0.0;
                for c in 0..n {
                    if r != c {
                        let v = rnd();
                        a[r * n + c] = v;
                        row_abs += v.abs();
                    }
                }
                a[r * n + r] = row_abs + 0.5 + rnd().abs();
            }
            // ...then, on odd trials, cyclically rotate the rows so the
            // dominant entries leave the diagonal and every elimination
            // step must pivot (MNA voltage-source rows look like this).
            if trial % 2 == 1 {
                let rot = 1 + (trial / 2) % (n - 1);
                let mut rotated = vec![0.0f64; n * n];
                for r in 0..n {
                    let src = (r + rot) % n;
                    rotated[r * n..r * n + n].copy_from_slice(&a[src * n..src * n + n]);
                }
                a = rotated;
            }
            let b: Vec<f64> = (0..n).map(|_| rnd()).collect();

            let x = solve_dense(&a, &b, n);

            let af = Mat::from_fn(n, n, |r, c| a[r * n + c]);
            let bf = Mat::from_fn(n, 1, |r, _| b[r]);
            let xf = af.partial_piv_lu().solve(&bf);
            for r in 0..n {
                let d = (x[r] - xf[(r, 0)]).abs();
                let scale = xf[(r, 0)].abs().max(1.0);
                assert!(d / scale < 1e-10, "trial {trial} row {r}: {d}");
            }
        }
    }
}
