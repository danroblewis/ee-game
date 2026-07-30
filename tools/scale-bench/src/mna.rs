//! Circuit-shaped MNA matrix generators for the sparse-LU experiment.
//!
//! These stamp exactly like `sim-core`'s `engine.rs` does (`stamp_g` on the
//! node block, ±1 couplings for voltage-source branch rows, GMIN on every
//! node diagonal), but they live here so the experiment needs no accessor
//! into sim-core. The point of the generator is the *sparsity structure*:
//! three topologies that bracket what a real world produces.

pub const GMIN: f64 = 1e-12;

/// A matrix as a coordinate list plus n. Duplicate (r,c) entries are
/// summed, exactly like repeated `+=` stamps.
pub struct Coo {
    pub n: usize,
    pub num_nodes: usize,
    pub entries: Vec<(usize, usize, f64)>,
}

impl Coo {
    pub fn dense(&self) -> Vec<f64> {
        let mut a = vec![0.0; self.n * self.n];
        for (r, c, v) in &self.entries {
            a[r * self.n + c] += v;
        }
        a
    }
    pub fn nnz_pattern(&self) -> usize {
        let mut seen: Vec<(usize, usize)> = self.entries.iter().map(|(r, c, _)| (*r, *c)).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }
    pub fn mul(&self, x: &[f64]) -> Vec<f64> {
        let mut y = vec![0.0; self.n];
        for (r, c, v) in &self.entries {
            y[*r] += v * x[*c];
        }
        y
    }
}

struct Builder {
    num_nodes: usize,
    num_branches: usize,
    e: Vec<(usize, usize, f64)>,
}

impl Builder {
    fn new(num_nodes: usize) -> Self {
        Builder {
            num_nodes,
            num_branches: 0,
            e: Vec::new(),
        }
    }
    /// Node indices are 1-based (0 = ground, eliminated).
    fn g(&mut self, p: usize, q: usize, g: f64) {
        if p > 0 {
            self.e.push((p - 1, p - 1, g));
        }
        if q > 0 {
            self.e.push((q - 1, q - 1, g));
        }
        if p > 0 && q > 0 {
            self.e.push((p - 1, q - 1, -g));
            self.e.push((q - 1, p - 1, -g));
        }
    }
    /// Voltage-source-like branch: zero diagonal, ±1 couplings.
    fn vsrc(&mut self, p: usize, q: usize) {
        let bi = self.num_nodes + self.num_branches;
        self.num_branches += 1;
        for (pin, sgn) in [(p, 1.0), (q, -1.0)] {
            if pin > 0 {
                self.e.push((bi, pin - 1, sgn));
                self.e.push((pin - 1, bi, sgn));
            }
        }
    }
    fn finish(mut self) -> Coo {
        for k in 0..self.num_nodes {
            self.e.push((k, k, GMIN));
        }
        Coo {
            n: self.num_nodes + self.num_branches,
            num_nodes: self.num_nodes,
            entries: self.e,
        }
    }
}

/// RC ladder: series R chain with a shunt C at every node, plus a source.
/// Nearly tridiagonal — the friendliest realistic circuit for sparse LU
/// (long feeders, transmission-line corridors, filter chains).
pub fn ladder(nodes: usize, h: f64) -> Coo {
    let mut b = Builder::new(nodes);
    for k in 1..=nodes {
        if k > 1 {
            b.g(k - 1, k, 1.0 / 10.0); // series 10 ohm
        }
        b.g(k, 0, 2.0 * 1e-6 / h); // shunt cap companion (TR)
    }
    b.vsrc(1, 0);
    b.finish()
}

/// 2-D resistive mesh (a copper plane / dense power grid): the *worst*
/// realistic structure for fill-in.
pub fn mesh(w: usize, hgt: usize) -> Coo {
    let idx = |x: usize, y: usize| y * w + x + 1;
    let mut b = Builder::new(w * hgt);
    for y in 0..hgt {
        for x in 0..w {
            if x + 1 < w {
                b.g(idx(x, y), idx(x + 1, y), 1.0 / 5.0);
            }
            if y + 1 < hgt {
                b.g(idx(x, y), idx(x, y + 1), 1.0 / 5.0);
            }
        }
    }
    // Corner tied to a source, opposite corner loaded.
    b.g(idx(w - 1, hgt - 1), 0, 1.0 / 100.0);
    b.vsrc(idx(0, 0), 0);
    b.finish()
}

/// Many small independent islands in ONE matrix (block diagonal) — what a
/// tiled game world actually produces today.
pub fn islands(count: usize, nodes_each: usize) -> Coo {
    let mut b = Builder::new(count * nodes_each);
    for i in 0..count {
        let base = i * nodes_each;
        for k in 1..=nodes_each {
            if k > 1 {
                b.g(base + k - 1, base + k, 1.0 / 220.0);
            } else {
                b.g(base + k, 0, 1.0 / 1000.0);
            }
        }
        b.g(base + nodes_each, 0, 1.0 / 470.0);
        b.vsrc(base + 1, 0);
    }
    b.finish()
}

/// A right-hand side with the same flavour as a stamped one.
pub fn rhs(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 9.0
        })
        .collect()
}
