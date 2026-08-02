//! Deterministic (seeded) generation of MNA-realistic test systems.
//!
//! Two families, per the approved design:
//! - Well-conditioned: diagonally dominant with row rotation so every
//!   elimination step must pivot (mirrors the sim-math faer oracle test —
//!   MNA voltage-source rows look like the rotated case).
//! - Ill-conditioned MNA-like: node-conductance networks with branch
//!   conductances drawn log-uniform from 1e-12..1e6 S and gmin on the
//!   diagonal — the regime where f32 is expected to fail. That is the point.

/// xorshift64* — deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.0 = s;
        s
    }
    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [-0.5, 0.5).
    pub fn sym(&mut self) -> f64 {
        self.uniform() - 0.5
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A batch of `count` dense n×n systems, row-major, matrices concatenated.
pub struct Batch {
    #[allow(dead_code)]
    pub n: usize,
    #[allow(dead_code)]
    pub count: usize,
    pub a: Vec<f64>, // count * n * n
    pub b: Vec<f64>, // count * n
}

/// Diagonally dominant + cyclic row rotation on odd matrices (forces real
/// pivoting work without conditioning trouble).
pub fn well_conditioned(n: usize, count: usize, seed: u64) -> Batch {
    let mut rng = Rng::new(seed);
    let mut a = vec![0.0; count * n * n];
    let mut b = vec![0.0; count * n];
    for m in 0..count {
        let am = &mut a[m * n * n..(m + 1) * n * n];
        let mut tmp = vec![0.0; n * n];
        for r in 0..n {
            let mut row_abs = 0.0;
            for c in 0..n {
                if r != c {
                    let v = rng.sym();
                    tmp[r * n + c] = v;
                    row_abs += v.abs();
                }
            }
            tmp[r * n + r] = row_abs + 0.5 + rng.uniform() * 0.5;
        }
        if m % 2 == 1 && n > 1 {
            let rot = 1 + m % (n - 1);
            for r in 0..n {
                let src = (r + rot) % n;
                am[r * n..r * n + n].copy_from_slice(&tmp[src * n..src * n + n]);
            }
        } else {
            am.copy_from_slice(&tmp);
        }
        for r in 0..n {
            b[m * n + r] = rng.sym() * 2.0;
        }
    }
    Batch { n, count, a, b }
}

/// MNA-like conductance network: n unknown nodes plus implicit ground.
/// Spanning chain guarantees non-singularity; extra random branches and
/// ground legs. Branch conductances drawn log-uniform from
/// 10^log_min .. 10^log_max S with gmin = 1e-12 on every diagonal, like a
/// gmin-conditioned MNA matrix. (-4..4) is "moderate" — a plausible mix of
/// 10 kΩ and 0.1 mΩ paths; (-12..6) is the design's harsh set where f32 is
/// expected to fail.
pub fn mna_network(n: usize, count: usize, seed: u64, log_min: f64, log_max: f64) -> Batch {
    let mut rng = Rng::new(seed);
    let mut a = vec![0.0; count * n * n];
    let mut b = vec![0.0; count * n];
    const GMIN: f64 = 1e-12;
    for m in 0..count {
        let am = &mut a[m * n * n..(m + 1) * n * n];
        for i in 0..n {
            am[i * n + i] = GMIN;
        }
        let log_g = |rng: &mut Rng| 10f64.powf(log_min + (log_max - log_min) * rng.uniform());
        // Spanning chain node i — node i+1, then node 0 — ground.
        for i in 0..n.saturating_sub(1) {
            let g = log_g(&mut rng);
            am[i * n + i] += g;
            am[(i + 1) * n + (i + 1)] += g;
            am[i * n + (i + 1)] -= g;
            am[(i + 1) * n + i] -= g;
        }
        am[0] += log_g(&mut rng); // node 0 to ground
        // Random internal branches.
        for _ in 0..n {
            let i = rng.below(n);
            let j = rng.below(n);
            if i == j {
                continue;
            }
            let g = log_g(&mut rng);
            am[i * n + i] += g;
            am[j * n + j] += g;
            am[i * n + j] -= g;
            am[j * n + i] -= g;
        }
        // A few extra ground legs.
        for _ in 0..(n / 4).max(1) {
            let i = rng.below(n);
            am[i * n + i] += log_g(&mut rng);
        }
        // Injected currents, mA scale.
        for i in 0..n {
            b[m * n + i] = rng.sym() * 2e-3;
        }
    }
    Batch { n, count, a, b }
}
