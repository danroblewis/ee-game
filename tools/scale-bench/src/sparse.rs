//! Prototype fixed-pattern sparse LU (the shape of the plan's S3 spike),
//! written to answer one question: how fast is refactor+solve on real
//! circuit sparsity versus the dense LU we ship today?
//!
//! Deliberately KLU-shaped:
//!   * symbolic analysis ONCE per topology edit (ordering + fill pattern),
//!   * numeric REFACTOR per Newton-Raphson iteration over that frozen
//!     pattern, with no pivot search and no reordering,
//!   * triangular solve per iteration.
//!
//! Determinism-relevant properties, by construction:
//!   * the pattern fixes the exact sequence of multiply/subtract/divide
//!     operations; no data-dependent reordering, no reductions with
//!     hardware-dependent order, no FMA, no transcendentals,
//!   * so a given (pattern, values) input produces one bit pattern on every
//!     target, the same argument that makes the dense path deterministic.
//!
//! Simplifications versus a shippable version, stated honestly:
//!   * ordering is exact greedy minimum degree on the elimination graph of
//!     A+Aᵀ, not AMD (approximate minimum degree). AMD is faster to compute
//!     and produces slightly worse orderings; the fill numbers here are
//!     therefore a mild *best case* for ordering quality and a *worst case*
//!     for analyze time.
//!   * no threshold partial pivoting. Instead, MNA structure is exploited:
//!     node rows are eliminated before voltage-source branch rows, which
//!     guarantees the branch diagonal is filled before it is used as a
//!     pivot. `refactor` reports failure if any pivot is tiny, which is the
//!     signal a real implementation would use to fall back to dense.

/// Pivots below this are treated as failure (same tolerance as the dense
/// path).
const PIVOT_TOL: f64 = 1e-30;

pub struct SparseLu {
    n: usize,
    /// perm[new] = old index.
    perm: Vec<usize>,
    /// L: strict lower triangle by row, unit diagonal, sorted col indices.
    lp: Vec<usize>,
    li: Vec<usize>,
    lx: Vec<f64>,
    /// U: diagonal + strict upper by row, sorted col indices.
    up: Vec<usize>,
    ui: Vec<usize>,
    ux: Vec<f64>,
    /// For each input COO entry: where its value accumulates.
    slot: Vec<(bool, usize)>,
    work: Vec<f64>,
    /// Scratch for the permuted RHS.
    pb: Vec<f64>,
}

pub struct Analysis {
    pub lu: SparseLu,
    pub nnz_a: usize,
    pub nnz_lu: usize,
    /// Multiply-subtract pairs one refactor performs.
    pub flops: usize,
}

impl SparseLu {
    /// Symbolic analysis: ordering + fill pattern + scatter map.
    pub fn analyze(coo: &crate::mna::Coo) -> Analysis {
        let n = coo.n;
        let num_nodes = coo.num_nodes;

        // 1. Symmetrised adjacency (no diagonal).
        let mut adj: Vec<std::collections::BTreeSet<usize>> = vec![Default::default(); n];
        for (r, c, _) in &coo.entries {
            if r != c {
                adj[*r].insert(*c);
                adj[*c].insert(*r);
            }
        }
        let nnz_a = coo.nnz_pattern();

        // 2. Greedy minimum degree, node rows before branch rows.
        let mut eliminated = vec![false; n];
        let mut perm = Vec::with_capacity(n);
        // Fill pattern: for eliminated v, the set of not-yet-eliminated
        // neighbours (all of which come later in the order).
        let mut reach: Vec<Vec<usize>> = vec![Vec::new(); n];
        for phase in 0..2 {
            loop {
                let mut best = usize::MAX;
                let mut best_deg = usize::MAX;
                for v in 0..n {
                    if eliminated[v] {
                        continue;
                    }
                    let is_node = v < num_nodes;
                    if (phase == 0) != is_node {
                        continue;
                    }
                    let d = adj[v].len();
                    if d < best_deg {
                        best_deg = d;
                        best = v;
                    }
                }
                if best == usize::MAX {
                    break;
                }
                let v = best;
                eliminated[v] = true;
                perm.push(v);
                let nb: Vec<usize> = adj[v].iter().copied().filter(|u| !eliminated[*u]).collect();
                reach[v] = nb.clone();
                // Clique the remaining neighbours (this is the fill).
                for i in 0..nb.len() {
                    for j in (i + 1)..nb.len() {
                        adj[nb[i]].insert(nb[j]);
                        adj[nb[j]].insert(nb[i]);
                    }
                }
                for u in &nb {
                    adj[*u].remove(&v);
                }
            }
            let _ = phase;
        }
        let mut iperm = vec![0usize; n];
        for (new, old) in perm.iter().enumerate() {
            iperm[*old] = new;
        }

        // 3. Build L/U row patterns in new indices.
        //    (u, v) with order(u) > order(v): L row u gets col order(v),
        //    U row v gets col order(u).
        let mut lrow: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut urow: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, row) in urow.iter_mut().enumerate() {
            row.push(i); // diagonal always present
        }
        for old_v in 0..n {
            let v = iperm[old_v];
            for old_u in &reach[old_v] {
                let u = iperm[*old_u];
                debug_assert!(u > v);
                lrow[u].push(v);
                urow[v].push(u);
            }
        }
        for i in 0..n {
            lrow[i].sort_unstable();
            lrow[i].dedup();
            urow[i].sort_unstable();
            urow[i].dedup();
        }

        let (lp, li) = flatten(&lrow);
        let (up, ui) = flatten(&urow);
        let lx = vec![0.0; li.len()];
        let ux = vec![0.0; ui.len()];

        // 4. Scatter map for the COO entries.
        let mut slot = Vec::with_capacity(coo.entries.len());
        for (r, c, _) in &coo.entries {
            let (nr, nc) = (iperm[*r], iperm[*c]);
            let (upper, base, idx) = if nc < nr {
                (false, lp[nr], find(&li[lp[nr]..lp[nr + 1]], nc))
            } else {
                (true, up[nr], find(&ui[up[nr]..up[nr + 1]], nc))
            };
            slot.push((upper, base + idx.expect("entry outside symbolic pattern")));
        }

        // 5. Count the multiply-subtract work of one refactor.
        let mut flops = 0usize;
        for i in 0..n {
            for k in li[lp[i]..lp[i + 1]].iter().copied() {
                flops += up[k + 1] - up[k] - 1;
            }
        }

        let nnz_lu = li.len() + ui.len();
        Analysis {
            lu: SparseLu {
                n,
                perm,
                lp,
                li,
                lx,
                up,
                ui,
                ux,
                slot,
                work: vec![0.0; n],
                pb: vec![0.0; n],
            },
            nnz_a,
            nnz_lu,
            flops,
        }
    }

    /// Numeric refactorization over the frozen pattern. `vals` must be the
    /// value column of the same COO entry list `analyze` saw.
    pub fn refactor(&mut self, vals: &[f64]) -> bool {
        debug_assert_eq!(vals.len(), self.slot.len());
        self.lx.iter_mut().for_each(|v| *v = 0.0);
        self.ux.iter_mut().for_each(|v| *v = 0.0);
        for (v, (upper, s)) in vals.iter().zip(self.slot.iter()) {
            if *upper {
                self.ux[*s] += v;
            } else {
                self.lx[*s] += v;
            }
        }

        for i in 0..self.n {
            // Scatter row i of A into the dense workspace.
            for p in self.lp[i]..self.lp[i + 1] {
                self.work[self.li[p]] = self.lx[p];
            }
            for p in self.up[i]..self.up[i + 1] {
                self.work[self.ui[p]] = self.ux[p];
            }
            // Eliminate, columns in increasing order.
            for p in self.lp[i]..self.lp[i + 1] {
                let k = self.li[p];
                let pivot = self.ux[self.up[k]]; // U(k,k) is first in the row
                let m = self.work[k] / pivot;
                self.work[k] = m;
                if m != 0.0 {
                    for q in (self.up[k] + 1)..self.up[k + 1] {
                        self.work[self.ui[q]] -= m * self.ux[q];
                    }
                }
            }
            // Gather back.
            for p in self.lp[i]..self.lp[i + 1] {
                self.lx[p] = self.work[self.li[p]];
                self.work[self.li[p]] = 0.0;
            }
            for p in self.up[i]..self.up[i + 1] {
                self.ux[p] = self.work[self.ui[p]];
                self.work[self.ui[p]] = 0.0;
            }
            let d = self.ux[self.up[i]];
            if abs(d) < PIVOT_TOL {
                return false;
            }
        }
        true
    }

    /// Solve in place: `x` is b on entry, the solution on exit.
    pub fn solve(&mut self, x: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            self.pb[i] = x[self.perm[i]];
        }
        for i in 0..n {
            let mut s = self.pb[i];
            for p in self.lp[i]..self.lp[i + 1] {
                s -= self.lx[p] * self.pb[self.li[p]];
            }
            self.pb[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = self.pb[i];
            for p in (self.up[i] + 1)..self.up[i + 1] {
                s -= self.ux[p] * self.pb[self.ui[p]];
            }
            self.pb[i] = s / self.ux[self.up[i]];
        }
        for i in 0..n {
            x[self.perm[i]] = self.pb[i];
        }
    }
}

fn flatten(rows: &[Vec<usize>]) -> (Vec<usize>, Vec<usize>) {
    let mut ptr = Vec::with_capacity(rows.len() + 1);
    let mut idx = Vec::new();
    ptr.push(0);
    for r in rows {
        idx.extend_from_slice(r);
        ptr.push(idx.len());
    }
    (ptr, idx)
}

fn find(sorted: &[usize], v: usize) -> Option<usize> {
    sorted.binary_search(&v).ok()
}

#[inline]
fn abs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & 0x7fff_ffff_ffff_ffff)
}
