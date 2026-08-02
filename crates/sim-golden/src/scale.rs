//! Synthetic large-world generator for the scale baseline.
//!
//! The point of this module is to produce *game-shaped* worlds, not academic
//! matrices: a world is a set of player builds ("districts"), each a small
//! schematic hung off a supply rail and a ground return, drawn with the
//! wire-heavy sloppiness of a real player. Two knobs matter for the solver:
//!
//! - **island structure** — `Structure::Districts { size }` makes N
//!   electrically disconnected builds (what a real room looks like);
//!   `Structure::One` makes a single connected mega-circuit (worst case).
//! - **nonlinear population** — the engine's `linear` flag is global, so a
//!   single diode anywhere forces the WHOLE world matrix to refactor on
//!   every Newton-Raphson iteration. `nonlinear_percent` controls what
//!   fraction of blocks come from the nonlinear pool.
//!
//! Construction is deterministic: block choice and component values come
//! from a fixed LCG seeded per district, so the same parameters always
//! produce byte-identical netlists. No RNG ever enters `sim-core`.
//!
//! Device inventory note: the engine has no 555 and no DC-motor device
//! (`docs/plan.md` schedules the motor for M6). The astable/relaxation
//! oscillator is therefore built the way the golden circuits build it — an
//! op-amp Schmitt trigger plus an RC integrator, which is what a 555 astable
//! is electrically — and the motor stand-in is an R+L series load. Those are
//! the two substitutions; everything else is a real engine device.

use sim_core::{ElementKind, ElementSpec, Point};
use std::collections::HashMap;

/// Island structure of a generated world.
#[derive(Clone, Copy, Debug)]
pub enum Structure {
    /// One connected circuit containing every element.
    One,
    /// Disconnected districts of roughly `size` elements each.
    Districts { size: usize },
}

impl Structure {
    pub fn label(&self) -> String {
        match self {
            Structure::One => "one-circuit".to_string(),
            Structure::Districts { size } => format!("districts~{size}"),
        }
    }
}

/// Generator parameters. `total_elements` is a target, not a guarantee:
/// blocks are atomic, so the achieved count overshoots slightly and is
/// always reported rather than assumed.
#[derive(Clone, Copy, Debug)]
pub struct GenParams {
    pub total_elements: usize,
    pub structure: Structure,
    /// Percent of blocks drawn from the nonlinear pool (diode/LED/BJT/
    /// MOSFET/op-amp).
    pub nonlinear_percent: u32,
    /// Percent of districts that contain a switching block (relaxation
    /// oscillator or AC source) and are therefore never electrically
    /// static. This is a *modeling assumption about worlds*, not a
    /// measurement of one.
    pub active_percent: u32,
}

impl GenParams {
    pub fn new(total_elements: usize, structure: Structure) -> Self {
        GenParams {
            total_elements,
            structure,
            nonlinear_percent: 30,
            active_percent: 20,
        }
    }

    pub fn nonlinear(mut self, percent: u32) -> Self {
        self.nonlinear_percent = percent;
        self
    }

    pub fn active(mut self, percent: u32) -> Self {
        self.active_percent = percent;
        self
    }
}

/// A generated world, kept split by district so the islands experiment can
/// simulate each district in its own `Engine` without regenerating.
pub struct World {
    pub params: GenParams,
    pub districts: Vec<Vec<ElementSpec>>,
    /// District indices that contain a switching (never-static) block.
    pub active: Vec<usize>,
}

impl World {
    pub fn flat(&self) -> Vec<ElementSpec> {
        self.districts.iter().flatten().cloned().collect()
    }

    pub fn element_count(&self) -> usize {
        self.districts.iter().map(|d| d.len()).sum()
    }

    /// District index an element id belongs to (ids are allocated
    /// `district * ID_STRIDE + local`).
    pub fn district_of(id: u32) -> usize {
        (id / ID_STRIDE) as usize
    }

    pub fn label(&self) -> String {
        format!(
            "{} elems, {}, {}% nonlinear blocks, {}% active districts",
            self.element_count(),
            self.params.structure.label(),
            self.params.nonlinear_percent,
            self.params.active_percent
        )
    }
}

const ID_STRIDE: u32 = 100_000;
/// Districts are placed on disjoint coordinate strips so no two districts
/// can ever share a junction point.
const DISTRICT_X_STRIDE: i32 = 1_000_000;
/// Supply rail / ground return rail y coordinates.
const RAIL_Y: i32 = 0;
const GND_Y: i32 = 240;
/// Horizontal pitch of one block column.
const COL: i32 = 16;

/// Deterministic 64-bit LCG (Knuth's MMIX constants). Generator-only: no
/// randomness ever reaches `sim-core`.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed ^ 0x2545_f491_4f6c_dd1d)
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Scale a nominal value by an E-series-ish multiplier. Real builds use
    /// preferred values spread over decades, which is also what gives the
    /// matrix a realistic conductance spread (and therefore realistic
    /// conditioning) instead of a suspiciously uniform one.
    fn e_series(&mut self, v: f64) -> f64 {
        const MUL: [f64; 7] = [0.22, 0.47, 1.0, 1.0, 2.2, 4.7, 10.0];
        v * MUL[self.pick(MUL.len())]
    }
}

struct Builder {
    ox: i32,
    next_id: u32,
    out: Vec<ElementSpec>,
    rng: Lcg,
}

impl Builder {
    fn new(district: usize) -> Self {
        Builder {
            ox: district as i32 * DISTRICT_X_STRIDE,
            next_id: district as u32 * ID_STRIDE + 1,
            out: Vec::new(),
            rng: Lcg::new(district as u64 * 0x9e37_79b9 + 7),
        }
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn two(&mut self, kind: ElementKind, a: Point, b: Point) {
        let id = self.id();
        self.out.push(ElementSpec::two(id, kind, a, b));
    }

    fn three(&mut self, kind: ElementKind, a: Point, b: Point, c: Point) {
        let id = self.id();
        self.out.push(ElementSpec::three(id, kind, a, b, c));
    }

    fn wire(&mut self, a: Point, b: Point) {
        self.two(ElementKind::Wire, a, b);
    }
    fn r(&mut self, a: Point, b: Point, ohms: f64) {
        let ohms = self.rng.e_series(ohms);
        self.two(ElementKind::Resistor { ohms }, a, b);
    }
    fn c(&mut self, a: Point, b: Point, farads: f64) {
        let farads = self.rng.e_series(farads);
        self.two(ElementKind::Capacitor { farads }, a, b);
    }
    fn l(&mut self, a: Point, b: Point, henries: f64) {
        let henries = self.rng.e_series(henries);
        self.two(ElementKind::Inductor { henries }, a, b);
    }
    fn ground(&mut self, at: Point) {
        let id = self.id();
        self.out.push(ElementSpec::ground(id, at));
    }

    // ---------------------------------------------------------- blocks
    // Every block hangs between the supply rail point (cx, RAIL_Y) and the
    // ground return point (cx, GND_Y), both of which are rail junctions.

    /// Divider + decoupling cap + a tapped load. The bread and butter.
    fn rc_divider(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        let mid = (cx, 80);
        self.r(sup, mid, 4_700.0);
        self.r(mid, gnd, 4_700.0);
        self.c(mid, gnd, 100e-9);
        self.wire(mid, (cx + 8, 80));
        self.r((cx + 8, 80), gnd, 22_000.0);
    }

    /// Series R+L load with a snubber cap: the DC-motor stand-in until the
    /// M6 motor device exists.
    fn rl_load(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.wire(sup, (cx + 4, 40));
        self.l((cx + 4, 40), (cx + 4, 120), 10e-3);
        self.r((cx + 4, 120), gnd, 100.0);
        self.c((cx + 4, 120), gnd, 1e-6);
    }

    /// Closed switch feeding a load: adds a branch-current unknown, the
    /// thing that makes MNA matrices indefinite and forces pivoting.
    fn switch_load(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.two(ElementKind::Switch { closed: true }, sup, (cx + 2, 40));
        self.r((cx + 2, 40), (cx + 2, 140), 220.0);
        self.wire((cx + 2, 140), gnd);
    }

    /// LED + series resistor: the indicator every build has.
    fn led_indicator(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.r(sup, (cx + 2, 60), 470.0);
        let color = self.rng.pick(5) as u8;
        self.two(ElementKind::Led { color }, (cx + 2, 60), (cx + 2, 160));
        self.wire((cx + 2, 160), gnd);
    }

    /// Diode + reservoir cap + load: a rectifier / reverse-polarity guard.
    fn rectifier(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.two(ElementKind::Diode, sup, (cx + 2, 50));
        self.c((cx + 2, 50), gnd, 10e-6);
        self.r((cx + 2, 50), gnd, 1_000.0);
        self.wire((cx + 2, 50), (cx + 6, 50));
    }

    /// NPN low-side switch driven hard into saturation.
    fn bjt_switch(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.r(sup, (cx + 2, 60), 100.0);
        self.r(sup, (cx + 6, 100), 10_000.0);
        self.three(
            ElementKind::Npn { beta: 100.0 },
            (cx + 6, 100),
            (cx + 2, 60),
            (cx + 2, 180),
        );
        self.wire((cx + 2, 180), gnd);
    }

    /// NMOS low-side switch with a gate divider.
    fn nmos_switch(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.r(sup, (cx + 8, 60), 10_000.0);
        self.r((cx + 8, 60), gnd, 10_000.0);
        self.r(sup, (cx + 2, 60), 100.0);
        self.three(
            ElementKind::Nmos { vt: 1.5, k: 0.05 },
            (cx + 8, 60),
            (cx + 2, 60),
            (cx + 2, 180),
        );
        self.wire((cx + 2, 180), gnd);
    }

    /// Op-amp voltage follower buffering a divider into a load.
    fn opamp_buffer(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        self.r(sup, (cx + 8, 60), 10_000.0);
        self.r((cx + 8, 60), gnd, 10_000.0);
        self.three(
            ElementKind::OpAmp { rail: 12.0, isc: sim_core::DEFAULT_OPAMP_ISC },
            (cx + 8, 60),
            (cx + 4, 120),
            (cx + 2, 60),
        );
        self.wire((cx + 2, 60), (cx + 4, 120));
        self.r((cx + 2, 60), gnd, 1_000.0);
    }

    /// Op-amp relaxation oscillator: the 555-astable equivalent (Schmitt
    /// hysteresis + RC integrator). Period ~2.2 ms, so it switches every
    /// ~55 substeps at dt = 20 us — this block is what makes a district
    /// permanently non-quiescent.
    fn relax_osc(&mut self, cx: i32) {
        let (sup, gnd) = ((cx, RAIL_Y), (cx, GND_Y));
        let (hp, hn, o) = ((cx + 10, 100), (cx + 4, 100), (cx + 7, 60));
        self.three(ElementKind::OpAmp { rail: 5.0, isc: sim_core::DEFAULT_OPAMP_ISC }, hp, hn, o);
        self.r(o, hn, 10_000.0);
        self.c(hn, gnd, 100e-9);
        self.r(o, hp, 10_000.0);
        self.r(hp, gnd, 10_000.0);
        // The op-amp takes its rail as a parameter, not as a pin, so a bare
        // Schmitt oscillator would touch nothing but ground and be its own
        // island. A weak offset trim from the supply plus a decoupling RC is
        // both realistic and what puts the block in its district's island.
        self.r(sup, hp, 470_000.0);
        self.r(sup, (cx + 2, 40), 10.0);
        self.c((cx + 2, 40), gnd, 10e-6);
    }

    /// A 1 kHz AC source driving an RC: active, but linear.
    fn ac_load(&mut self, cx: i32) {
        let gnd = (cx, GND_Y);
        let hot = (cx + 12, 40);
        let id = self.id();
        self.out.push(ElementSpec::two(
            id,
            ElementKind::VoltageSource {
                dc: 0.0,
                amp: 1.0,
                hz: 1_000.0,
                phase: 0.0,
            },
            hot,
            gnd,
        ));
        self.r(hot, (cx + 12, 120), 1_000.0);
        self.c((cx + 12, 120), gnd, 100e-9);
        // AC-coupled into a rail-biased node: the link that puts this block
        // in its district's island rather than in one of its own.
        self.c((cx + 12, 120), (cx + 8, 120), 1e-6);
        self.r((cx + 8, 120), (cx, RAIL_Y), 100_000.0);
    }
}

const LINEAR_POOL: usize = 3;
const NONLINEAR_POOL: usize = 5;

/// Build one district of at least `target` elements. Returns the district
/// and whether it contains a switching block.
fn district(index: usize, target: usize, params: GenParams) -> (Vec<ElementSpec>, bool) {
    let mut b = Builder::new(index);
    let ox = b.ox;
    // Supply and ground return for this district.
    b.ground((ox, GND_Y));
    let id = b.id();
    b.out.push(ElementSpec::two(
        id,
        ElementKind::VoltageSource {
            dc: 12.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        },
        (ox, RAIL_Y),
        (ox, GND_Y),
    ));

    // Deterministic scatter of the active districts across the world.
    let active = (index as u32).wrapping_mul(37) % 100 < params.active_percent;
    let mut col = 0usize;
    while b.out.len() < target {
        let cx = ox + (col as i32 + 1) * COL;
        // Supply rail and ground return, one wire run per column each: wire
        // elements are the bulk of any real schematic and they add no
        // unknowns (closure merges them into one node per rail).
        let px = cx - COL;
        b.wire((px, RAIL_Y), (cx, RAIL_Y));
        b.wire((px, GND_Y), (cx, GND_Y));

        // The first column of an active district carries the switching
        // block; everything else comes from the linear/nonlinear pools.
        if active && col == 0 {
            // The relaxation oscillator is an op-amp circuit, so a world
            // asked for zero nonlinear content gets the linear AC-source
            // block instead: `nonlinear_percent == 0` means a genuinely
            // linear world.
            if index.is_multiple_of(2) && params.nonlinear_percent > 0 {
                b.relax_osc(cx);
            } else {
                b.ac_load(cx);
            }
        } else if (b.rng.pick(100) as u32) < params.nonlinear_percent {
            match b.rng.pick(NONLINEAR_POOL) {
                0 => b.led_indicator(cx),
                1 => b.rectifier(cx),
                2 => b.bjt_switch(cx),
                3 => b.nmos_switch(cx),
                _ => b.opamp_buffer(cx),
            }
        } else {
            match b.rng.pick(LINEAR_POOL) {
                0 => b.rc_divider(cx),
                1 => b.rl_load(cx),
                _ => b.switch_load(cx),
            }
        }
        col += 1;
    }
    (b.out, active)
}

/// Generate a world. Deterministic in `params` alone.
pub fn generate(params: GenParams) -> World {
    let per_district = match params.structure {
        Structure::One => params.total_elements,
        Structure::Districts { size } => size,
    };
    let ndistricts = params.total_elements.div_ceil(per_district.max(1)).max(1);
    let mut districts = Vec::with_capacity(ndistricts);
    let mut active = Vec::new();
    for i in 0..ndistricts {
        let (d, is_active) = district(i, per_district, params);
        if is_active {
            active.push(i);
        }
        districts.push(d);
    }
    World {
        params,
        districts,
        active,
    }
}

// --------------------------------------------------------------- topology

/// What `Engine::compile` will produce, computed independently so the bench
/// can size a run before allocating an n x n dense matrix — and so the
/// engine's own numbers have something to be checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topology {
    pub junctions: usize,
    pub nodes: usize,
    pub branches: usize,
    pub unknowns: usize,
    /// Connected components of the node graph, counting ground as a shared
    /// *reference* rather than a coupling path (which is how a per-island
    /// solver would partition the world).
    pub islands: usize,
}

struct Uf(Vec<usize>);

impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        self.0[ra] = rb;
    }
}

pub fn topology(specs: &[ElementSpec]) -> Topology {
    // Junction indices (hashed, unlike the engine's linear scan).
    let mut jix: HashMap<Point, usize> = HashMap::new();
    let mut ends: Vec<Vec<usize>> = Vec::with_capacity(specs.len());
    for s in specs {
        let mut e = Vec::with_capacity(s.pins.len());
        for p in &s.pins {
            let next = jix.len();
            e.push(*jix.entry(*p).or_insert(next));
        }
        ends.push(e);
    }
    let np = jix.len();
    // Wire closure + ground merge, exactly as compile() does it.
    let mut uf = Uf::new(np + 1);
    let groot = np;
    for (s, e) in specs.iter().zip(ends.iter()) {
        match s.kind {
            ElementKind::Wire => uf.union(e[0], e[1]),
            ElementKind::Ground => uf.union(e[0], groot),
            _ => {}
        }
    }
    let gr = uf.find(groot);
    let mut node_of_root: HashMap<usize, usize> = HashMap::from([(gr, 0)]);
    let mut nodes = 0usize;
    let node_of_junction: Vec<usize> = (0..np)
        .map(|j| {
            let r = uf.find(j);
            match node_of_root.get(&r) {
                Some(n) => *n,
                None => {
                    nodes += 1;
                    node_of_root.insert(r, nodes);
                    nodes
                }
            }
        })
        .collect();
    let branches = specs.iter().filter(|s| s.kind.is_branch()).count();

    // Islands: components of nodes 1..=nodes linked by non-ground elements.
    let mut isl = Uf::new(nodes + 1);
    for (s, e) in specs.iter().zip(ends.iter()) {
        if matches!(s.kind, ElementKind::Ground) {
            continue;
        }
        let ns: Vec<usize> = e
            .iter()
            .map(|j| node_of_junction[*j])
            .filter(|n| *n != 0)
            .collect();
        for w in ns.windows(2) {
            isl.union(w[0], w[1]);
        }
    }
    let mut roots: Vec<usize> = (1..=nodes).map(|n| isl.find(n)).collect();
    roots.sort_unstable();
    roots.dedup();
    Topology {
        junctions: np,
        nodes,
        branches,
        unknowns: nodes + branches,
        islands: roots.len(),
    }
}

// ------------------------------------------------------------- LU op counts

/// What one `DenseLu::factor` call actually executes.
///
/// Wall-clock factor time is *data dependent* in the current kernel: the
/// `if m != 0.0` guard in `dense.rs` skips the whole inner row update when
/// the multiplier is exactly zero, so a matrix with structure (islands, or a
/// star of blocks hung off two shared rails) costs far less than a full
/// `n^3/3`. Which entries are zero depends on the pivots, which depend on
/// the values — so the same circuit costs different amounts at different
/// operating points. Counting the ops separates "the matrix got bigger" from
/// "the elimination got denser", which a stopwatch alone cannot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LuOps {
    pub n: usize,
    /// Inner-loop `lu[r][c] -= m * lu[k][c]` executions: the dominant term,
    /// one multiply and one subtract each.
    pub updates: u64,
    /// Multiplier divisions `lu[r][k] / pivot` (one per subdiagonal entry,
    /// zero or not).
    pub divisions: u64,
    /// Row updates skipped because the multiplier was exactly zero. Each
    /// skip saves `n - k - 1` updates; this is the structure the current
    /// dense kernel already exploits by accident.
    pub skipped_rows: u64,
    /// Pivot-search comparisons: `n(n-1)/2`, data independent.
    pub pivot_cmps: u64,
    /// Element moves spent on pivot row swaps (`n` per swap).
    pub swap_moves: u64,
    /// Nonzeros in the resulting factor.
    pub factor_nnz: usize,
    pub singular: bool,
}

impl LuOps {
    /// `n^3/3` — what a structure-blind dense factor would execute.
    pub fn dense_updates(&self) -> u64 {
        let n = self.n as u64;
        n * (n - 1) * (2 * n - 1) / 6
    }

    /// How much of the theoretical dense work the zero-skip avoids.
    pub fn structure_saving(&self) -> f64 {
        let d = self.dense_updates();
        if d == 0 {
            return 0.0;
        }
        1.0 - self.updates as f64 / d as f64
    }
}

/// Bit-exact mirror of `sim_math::DenseLu::factor` that counts its work.
///
/// Deliberately a copy of the kernel rather than a wrapper: `sim-math` must
/// not grow counters in its hot loop (they would cost the shipping solver
/// real time). `lu_ops_matches_dense_lu` in `tests/scale.rs` asserts the
/// mirror stays bit-identical to the real kernel, so a drift in `dense.rs`
/// fails a test instead of silently invalidating the numbers here.
pub fn lu_ops(a: &[f64], n: usize) -> (LuOps, Vec<f64>) {
    const PIVOT_TOL: f64 = 1e-30;
    let abs = |x: f64| f64::from_bits(x.to_bits() & 0x7fff_ffff_ffff_ffff);
    let mut ops = LuOps {
        n,
        ..LuOps::default()
    };
    let mut lu = a.to_vec();
    for k in 0..n {
        let mut p = k;
        let mut pmax = abs(lu[k * n + k]);
        for r in (k + 1)..n {
            ops.pivot_cmps += 1;
            let v = abs(lu[r * n + k]);
            if v > pmax {
                pmax = v;
                p = r;
            }
        }
        if pmax < PIVOT_TOL {
            ops.singular = true;
            return (ops, lu);
        }
        if p != k {
            ops.swap_moves += n as u64;
            for c in 0..n {
                lu.swap(k * n + c, p * n + c);
            }
        }
        let pivot = lu[k * n + k];
        for r in (k + 1)..n {
            ops.divisions += 1;
            let m = lu[r * n + k] / pivot;
            lu[r * n + k] = m;
            if m != 0.0 {
                ops.updates += (n - k - 1) as u64;
                for c in (k + 1)..n {
                    lu[r * n + c] -= m * lu[k * n + c];
                }
            } else {
                ops.skipped_rows += 1;
            }
        }
    }
    ops.factor_nnz = lu.iter().filter(|v| **v != 0.0).count();
    (ops, lu)
}

/// Element mix by kind, most common first, plus the nonlinear total.
pub fn mix(specs: &[ElementSpec]) -> (Vec<(&'static str, usize)>, usize) {
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for s in specs {
        *counts.entry(kind_name(&s.kind)).or_insert(0) += 1;
    }
    let mut v: Vec<(&'static str, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let nl = specs.iter().filter(|s| s.kind.is_nonlinear()).count();
    (v, nl)
}

pub fn kind_name(k: &ElementKind) -> &'static str {
    use ElementKind::*;
    match k {
        Wire => "wire",
        Ground => "ground",
        Resistor { .. } => "resistor",
        Lamp { .. } => "lamp",
        Capacitor { .. } => "cap",
        Inductor { .. } => "inductor",
        VoltageSource { .. } => "vsource",
        CurrentSource { .. } => "isource",
        Switch { .. } => "switch",
        Diode => "diode",
        Zener { .. } => "zener",
        Led { .. } => "led",
        Npn { .. } => "npn",
        Pnp { .. } => "pnp",
        Nmos { .. } => "nmos",
        Pmos { .. } => "pmos",
        OpAmp { .. } => "opamp",
        Ota => "ota",
        Potentiometer { .. } => "pot",
        Speaker { .. } => "speaker",
        Rail { .. } => "rail",
        Button { .. } => "button",
        Timer555 => "555",
        Motor { .. } => "motor",
        Noise { .. } => "noise",
        Photocell { .. } => "photocell",
    }
}
