//! The MNA engine: compile a netlist into a solvable system, advance it in
//! fixed timesteps, keep every displayed number honest.
//!
//! Unknown vector layout: `[v_node1 .. v_nodeN, i_branch1 .. i_branchM]`
//! where branches are voltage-source-like elements (sources, closed
//! switches). Node 0 is ground and is not an unknown. Every node gets a
//! `GMIN` leak to ground so floating circuits stay solvable
//! (beginner-tolerant solver).

use crate::netlist::{ElementKind, ElementSpec, InteractOp, Point};
use sim_math::DenseLu;

pub const GMIN: f64 = 1e-12;
/// Diode saturation current and thermal voltage (n = 2 emission holds NR
/// friendlier for game circuits than n = 1; same family as Falstad's
/// default diode).
const DIODE_IS: f64 = 1.7143528192808883e-7;
const DIODE_VT: f64 = 2.0 * 0.025865;
const NR_MAX_ITERS: usize = 60;
const NR_ABSTOL: f64 = 1e-6;
const NR_RELTOL: f64 = 1e-3;
/// dt-halving rescue depth: 2^4 = 16x finer before quarantine.
const RESCUE_DEPTH: u32 = 4;
/// Steps integrated with backward Euler after a discontinuity (edit,
/// switch flip) to kill trapezoidal ringing.
const BE_STEPS_AFTER_EVENT: u32 = 2;

const TWO_PI: f64 = core::f64::consts::TAU;

#[derive(Clone, Copy, Default)]
struct ElemState {
    /// Companion history: capacitor voltage / inductor current at the
    /// previous accepted step.
    v_prev: f64,
    i_prev: f64,
    /// Diode NR voltage guess.
    v_guess: f64,
    /// Last computed branch current (a -> b), for rendering and probes.
    current: f64,
}

struct CompiledElem {
    spec: ElementSpec,
    /// Electrical node index per terminal (0 = ground).
    node: [usize; 2],
    /// Index into the branch-current unknowns for voltage-source-likes.
    branch: Option<usize>,
    state: ElemState,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AdvanceReport {
    pub steps: u32,
    pub nr_iters: u32,
    pub rescues: u32,
    pub quarantined: bool,
}

/// Per-element view of the live simulation for rendering: everything the
/// client paints comes from here and nowhere else.
#[derive(Clone, Copy, Debug, Default)]
pub struct ElemFrame {
    pub id: u32,
    pub va: f64,
    pub vb: f64,
    /// Current a -> b through the element (wires included, via KCL
    /// propagation).
    pub current: f64,
    pub power: f64,
}

pub struct Engine {
    elems: Vec<CompiledElem>,
    /// Junction (geometric point) -> electrical node.
    junctions: Vec<(Point, usize)>,
    num_nodes: usize,
    num_branches: usize,
    n: usize,
    a: Vec<f64>,
    b: Vec<f64>,
    x: Vec<f64>,
    lu: DenseLu,
    /// Linear circuits factor once per edit and reuse. The factorization
    /// is only valid for the (step size, integration mode) it was stamped
    /// with — companion conductances depend on both.
    linear: bool,
    factor_valid: bool,
    factored_h: f64,
    factored_be: bool,
    dt: f64,
    time: f64,
    be_steps: u32,
    quarantined: bool,
}

impl Engine {
    pub fn new(dt: f64) -> Self {
        Engine {
            elems: Vec::new(),
            junctions: Vec::new(),
            num_nodes: 0,
            num_branches: 0,
            n: 0,
            a: Vec::new(),
            b: Vec::new(),
            x: Vec::new(),
            lu: DenseLu::new(0),
            linear: true,
            factor_valid: false,
            factored_h: 0.0,
            factored_be: false,
            dt,
            time: 0.0,
            be_steps: 0,
            quarantined: false,
        }
    }

    pub fn dt(&self) -> f64 {
        self.dt
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn is_quarantined(&self) -> bool {
        self.quarantined
    }

    /// Replace the document and recompile. Continuous state (cap voltage,
    /// inductor current) survives for elements whose id persists.
    pub fn set_elements(&mut self, specs: &[ElementSpec]) {
        let mut old_state: Vec<(u32, ElemState)> = Vec::with_capacity(self.elems.len());
        for e in &self.elems {
            old_state.push((e.spec.id, e.state));
        }
        self.elems.clear();
        for s in specs {
            let mut state = ElemState::default();
            if let Some((_, st)) = old_state.iter().find(|(id, _)| *id == s.id) {
                state = *st;
            }
            self.elems.push(CompiledElem {
                spec: *s,
                node: [0, 0],
                branch: None,
                state,
            });
        }
        self.compile();
    }

    pub fn interact(&mut self, id: u32, op: InteractOp) {
        let Some(e) = self.elems.iter_mut().find(|e| e.spec.id == id) else {
            return;
        };
        match (op, &mut e.spec.kind) {
            (InteractOp::SetSwitch { closed }, ElementKind::Switch { closed: c }) => *c = closed,
            (InteractOp::SetValue { value }, k) => match k {
                ElementKind::Resistor { ohms } | ElementKind::Lamp { ohms, .. } => {
                    *ohms = value.max(1e-6)
                }
                ElementKind::Capacitor { farads } => *farads = value.max(1e-15),
                ElementKind::Inductor { henries } => *henries = value.max(1e-12),
                ElementKind::VoltageSource { dc, .. } => *dc = value,
                ElementKind::CurrentSource { amps } => *amps = value,
                _ => return,
            },
            _ => return,
        }
        // Switch flips change topology (branch count); value changes only
        // invalidate the factorization. Recompiling handles both and is
        // cheap at M1 scale.
        self.compile();
    }

    /// Wire closure + node numbering + unknown layout.
    fn compile(&mut self) {
        // 1. Junctions: unique endpoints.
        let mut points: Vec<Point> = Vec::new();
        let jix = |points: &mut Vec<Point>, p: Point| -> usize {
            match points.iter().position(|q| *q == p) {
                Some(i) => i,
                None => {
                    points.push(p);
                    points.len() - 1
                }
            }
        };
        let mut ends: Vec<[usize; 2]> = Vec::with_capacity(self.elems.len());
        for e in &self.elems {
            let ja = jix(&mut points, e.spec.a);
            let jb = jix(&mut points, e.spec.b);
            ends.push([ja, jb]);
        }

        // 2. Union-find: wires merge their endpoints; grounds pin to a
        //    virtual ground root.
        let ground_root = points.len();
        let mut parent: Vec<usize> = (0..=points.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for (e, je) in self.elems.iter().zip(ends.iter()) {
            match e.spec.kind {
                ElementKind::Wire => {
                    let (ra, rb) = (find(&mut parent, je[0]), find(&mut parent, je[1]));
                    parent[ra] = rb;
                }
                ElementKind::Ground => {
                    let (ra, rg) = (find(&mut parent, je[0]), find(&mut parent, ground_root));
                    parent[ra] = rg;
                }
                _ => {}
            }
        }

        // 3. Number electrical nodes: ground set -> 0, others 1..=N.
        let groot = find(&mut parent, ground_root);
        let mut node_of_root: Vec<(usize, usize)> = vec![(groot, 0)];
        let mut num_nodes = 0usize;
        let node_of_junction: Vec<usize> = (0..points.len())
            .map(|j| {
                let r = find(&mut parent, j);
                match node_of_root.iter().find(|(root, _)| *root == r) {
                    Some((_, n)) => *n,
                    None => {
                        num_nodes += 1;
                        node_of_root.push((r, num_nodes));
                        num_nodes
                    }
                }
            })
            .collect();

        // 4. Branch unknowns for voltage-source-likes.
        let mut num_branches = 0usize;
        for (e, je) in self.elems.iter_mut().zip(ends.iter()) {
            e.node = [node_of_junction[je[0]], node_of_junction[je[1]]];
            e.branch = match e.spec.kind {
                ElementKind::VoltageSource { .. } => Some(num_branches),
                ElementKind::Switch { closed: true } => Some(num_branches),
                _ => None,
            };
            if e.branch.is_some() {
                num_branches += 1;
            }
        }

        self.junctions = points
            .iter()
            .zip(node_of_junction.iter())
            .map(|(p, n)| (*p, *n))
            .collect();
        self.num_nodes = num_nodes;
        self.num_branches = num_branches;
        self.n = num_nodes + num_branches;
        self.a.resize(self.n * self.n, 0.0);
        self.b.resize(self.n, 0.0);
        self.x.resize(self.n, 0.0);
        self.lu.resize(self.n);
        self.linear = !self
            .elems
            .iter()
            .any(|e| matches!(e.spec.kind, ElementKind::Diode));
        self.factor_valid = false;
        self.quarantined = false;
        self.be_steps = BE_STEPS_AFTER_EVENT;
    }

    /// Advance up to `max_steps` fixed-dt substeps. The caller owns the
    /// wall-clock budget (Falstad's rule: heavy circuits slow sim time,
    /// never the UI).
    pub fn advance(&mut self, max_steps: u32) -> AdvanceReport {
        let mut report = AdvanceReport::default();
        if self.n == 0 || self.quarantined {
            report.quarantined = self.quarantined;
            return report;
        }
        for _ in 0..max_steps {
            let be = self.be_steps > 0;
            match self.step(self.dt, be, 0, &mut report) {
                Ok(()) => {
                    self.time += self.dt;
                    self.be_steps = self.be_steps.saturating_sub(1);
                    report.steps += 1;
                }
                Err(()) => {
                    self.quarantined = true;
                    break;
                }
            }
        }
        report.quarantined = self.quarantined;
        report
    }

    /// One accepted step of size `h`, recursing into halved BE steps if NR
    /// fails. On success, device history state has been advanced.
    fn step(&mut self, h: f64, be: bool, depth: u32, report: &mut AdvanceReport) -> Result<(), ()> {
        let saved: Vec<ElemState> = self.elems.iter().map(|e| e.state).collect();
        match self.solve_step(h, be, report) {
            Ok(()) => Ok(()),
            Err(()) => {
                for (e, s) in self.elems.iter_mut().zip(saved.iter()) {
                    e.state = *s;
                }
                if depth >= RESCUE_DEPTH {
                    return Err(());
                }
                report.rescues += 1;
                // Backward Euler at half the step, twice: robust against
                // both nonconvergence and trapezoidal ringing.
                self.factor_valid = false;
                self.step(h * 0.5, true, depth + 1, report)?;
                self.step(h * 0.5, true, depth + 1, report)?;
                self.factor_valid = false;
                Ok(())
            }
        }
    }

    /// Newton-Raphson (single pass for linear circuits) at t + h.
    fn solve_step(&mut self, h: f64, be: bool, report: &mut AdvanceReport) -> Result<(), ()> {
        let t_new = self.time + h;
        let iters = if self.linear { 1 } else { NR_MAX_ITERS };
        let mut converged = self.linear;
        for _ in 0..iters {
            report.nr_iters += 1;
            self.build(t_new, h, be)?;
            self.x.copy_from_slice(&self.b);
            self.lu.solve(&mut self.x);
            if self.x.iter().any(|v| !v.is_finite()) {
                return Err(());
            }
            if self.linear {
                break;
            }
            // Convergence = every diode's limited new guess agrees with
            // its previous guess.
            converged = true;
            for ei in 0..self.elems.len() {
                if let ElementKind::Diode = self.elems[ei].spec.kind {
                    let node = self.elems[ei].node;
                    let v = self.x_voltage(node[0]) - self.x_voltage(node[1]);
                    let e = &mut self.elems[ei];
                    let limited = pnjlim(v, e.state.v_guess);
                    if (limited - e.state.v_guess).abs()
                        > NR_ABSTOL + NR_RELTOL * limited.abs().max(e.state.v_guess.abs())
                    {
                        converged = false;
                    }
                    e.state.v_guess = limited;
                }
            }
            if converged {
                break;
            }
        }
        if !converged {
            return Err(());
        }
        self.accept(h, be);
        Ok(())
    }

    #[inline]
    fn x_voltage(&self, node: usize) -> f64 {
        if node == 0 {
            0.0
        } else {
            self.x[node - 1]
        }
    }

    /// Stamp the full system for time `t_new` and step `h`. Factors the
    /// matrix unless a valid linear factorization is being reused.
    fn build(&mut self, t_new: f64, h: f64, be: bool) -> Result<(), ()> {
        let n = self.n;
        let need_factor =
            !(self.linear && self.factor_valid && self.factored_h == h && self.factored_be == be);
        if need_factor {
            self.a.iter_mut().for_each(|v| *v = 0.0);
        }
        self.b.iter_mut().for_each(|v| *v = 0.0);

        if need_factor {
            for k in 0..self.num_nodes {
                self.a[k * n + k] += GMIN;
            }
        }

        // Local stamp helpers writing straight into self.a / self.b.
        macro_rules! stamp_g {
            ($p:expr, $q:expr, $g:expr) => {{
                let (p, q, g) = ($p, $q, $g);
                if need_factor {
                    if p > 0 {
                        self.a[(p - 1) * n + (p - 1)] += g;
                    }
                    if q > 0 {
                        self.a[(q - 1) * n + (q - 1)] += g;
                    }
                    if p > 0 && q > 0 {
                        self.a[(p - 1) * n + (q - 1)] -= g;
                        self.a[(q - 1) * n + (p - 1)] -= g;
                    }
                }
            }};
        }
        // Current `i` flowing a -> b *through* the element: leaves node a.
        macro_rules! stamp_i {
            ($p:expr, $q:expr, $i:expr) => {{
                let (p, q, i) = ($p, $q, $i);
                if p > 0 {
                    self.b[p - 1] -= i;
                }
                if q > 0 {
                    self.b[q - 1] += i;
                }
            }};
        }

        for ei in 0..self.elems.len() {
            let (kind, node, branch, state) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch, e.state)
            };
            let (p, q) = (node[0], node[1]);
            match kind {
                ElementKind::Wire | ElementKind::Ground => {}
                ElementKind::Resistor { ohms } | ElementKind::Lamp { ohms, .. } => {
                    stamp_g!(p, q, 1.0 / ohms);
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let ieq = if be {
                        -geq * state.v_prev
                    } else {
                        -(geq * state.v_prev + state.i_prev)
                    };
                    stamp_g!(p, q, geq);
                    stamp_i!(p, q, ieq);
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let ieq = if be {
                        state.i_prev
                    } else {
                        state.i_prev + geq * state.v_prev
                    };
                    stamp_g!(p, q, geq);
                    stamp_i!(p, q, ieq);
                }
                ElementKind::CurrentSource { amps } => {
                    stamp_i!(p, q, amps);
                }
                ElementKind::VoltageSource { dc, amp, hz, phase } => {
                    let v = if amp == 0.0 {
                        dc
                    } else {
                        dc + amp * libm::sin(TWO_PI * hz * t_new + phase)
                    };
                    let bi = self.num_nodes + branch.ok_or(())?;
                    if need_factor {
                        if p > 0 {
                            self.a[bi * n + (p - 1)] += 1.0;
                            self.a[(p - 1) * n + bi] += 1.0;
                        }
                        if q > 0 {
                            self.a[bi * n + (q - 1)] -= 1.0;
                            self.a[(q - 1) * n + bi] -= 1.0;
                        }
                    }
                    self.b[bi] = v;
                }
                ElementKind::Switch { closed } => {
                    if closed {
                        let bi = self.num_nodes + branch.ok_or(())?;
                        if need_factor {
                            if p > 0 {
                                self.a[bi * n + (p - 1)] += 1.0;
                                self.a[(p - 1) * n + bi] += 1.0;
                            }
                            if q > 0 {
                                self.a[bi * n + (q - 1)] -= 1.0;
                                self.a[(q - 1) * n + bi] -= 1.0;
                            }
                        }
                        self.b[bi] = 0.0;
                    }
                }
                ElementKind::Diode => {
                    let vg = state.v_guess;
                    let ex = libm::exp(vg / DIODE_VT);
                    let geq = DIODE_IS / DIODE_VT * ex;
                    let i0 = DIODE_IS * (ex - 1.0) - geq * vg;
                    stamp_g!(p, q, geq);
                    stamp_i!(p, q, i0);
                }
            }
        }

        #[cfg(feature = "dump-matrix")]
        {
            std::eprintln!("t={t_new} need_factor={need_factor} n={n}");
            for r in 0..n {
                let row: Vec<f64> = (0..n).map(|c| self.a[r * n + c]).collect();
                std::eprintln!("  {row:?} | {}", self.b[r]);
            }
        }
        if need_factor {
            let ok = {
                // Split borrows: factor reads a copy internally.
                let a = core::mem::take(&mut self.a);
                let ok = self.lu.factor(&a);
                self.a = a;
                ok
            };
            if !ok {
                return Err(());
            }
            if self.linear {
                self.factor_valid = true;
                self.factored_h = h;
                self.factored_be = be;
            }
        }
        Ok(())
    }

    /// Commit device history from the solved unknowns.
    fn accept(&mut self, h: f64, be: bool) {
        for ei in 0..self.elems.len() {
            let (kind, node, branch) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch)
            };
            let v = self.x_voltage(node[0]) - self.x_voltage(node[1]);
            let st = &mut self.elems[ei].state;
            match kind {
                ElementKind::Resistor { ohms } | ElementKind::Lamp { ohms, .. } => {
                    st.current = v / ohms;
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let i = if be {
                        geq * (v - st.v_prev)
                    } else {
                        geq * (v - st.v_prev) - st.i_prev
                    };
                    st.v_prev = v;
                    st.i_prev = i;
                    st.current = i;
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let i = if be {
                        st.i_prev + geq * v
                    } else {
                        st.i_prev + geq * (v + st.v_prev)
                    };
                    st.v_prev = v;
                    st.i_prev = i;
                    st.current = i;
                }
                ElementKind::CurrentSource { amps } => st.current = amps,
                ElementKind::VoltageSource { .. } | ElementKind::Switch { closed: true } => {
                    if let Some(bi) = branch {
                        st.current = self.x[self.num_nodes + bi];
                    }
                }
                ElementKind::Diode => {
                    st.current = DIODE_IS * (libm::exp(v / DIODE_VT) - 1.0);
                    st.v_guess = v;
                }
                _ => st.current = 0.0,
            }
        }
    }

    /// Voltage at an electrical node from the last solve.
    pub fn node_voltage(&self, node: usize) -> f64 {
        self.x_voltage(node)
    }

    /// Voltage at a geometric point, if it is a junction.
    pub fn voltage_at(&self, p: Point) -> Option<f64> {
        self.junctions
            .iter()
            .find(|(q, _)| *q == p)
            .map(|(_, node)| self.x_voltage(*node))
    }

    /// Per-element render frame. Wire currents are recovered by KCL
    /// propagation over junctions (wires are node-merged so they have no
    /// unknown of their own).
    pub fn frame(&self) -> Vec<ElemFrame> {
        let mut out: Vec<ElemFrame> = self
            .elems
            .iter()
            .map(|e| {
                let va = self.x_voltage(e.node[0]);
                let vb = self.x_voltage(e.node[1]);
                ElemFrame {
                    id: e.spec.id,
                    va,
                    vb,
                    current: e.state.current,
                    power: (va - vb) * e.state.current,
                }
            })
            .collect();
        self.solve_wire_currents(&mut out);
        out
    }

    /// KCL relaxation: a wire whose endpoint junction has exactly one
    /// unknown incident current gets solved by that junction's balance.
    /// Pure-wire loops are ambiguous and settle at 0 (harmless: dots just
    /// don't move there).
    fn solve_wire_currents(&self, frames: &mut [ElemFrame]) {
        // Incident terminals per junction: (elem index, terminal 0|1).
        let mut incident: Vec<Vec<(usize, usize)>> = vec![Vec::new(); self.junctions.len()];
        let jix = |p: Point| self.junctions.iter().position(|(q, _)| *q == p);
        for (i, e) in self.elems.iter().enumerate() {
            if matches!(e.spec.kind, ElementKind::Ground) {
                if let Some(j) = jix(e.spec.a) {
                    incident[j].push((i, 0));
                }
                continue;
            }
            for (t, p) in [(0usize, e.spec.a), (1usize, e.spec.b)] {
                if let Some(j) = jix(p) {
                    incident[j].push((i, t));
                }
            }
        }
        let is_wire = |i: usize| matches!(self.elems[i].spec.kind, ElementKind::Wire);
        let mut known: Vec<bool> = (0..self.elems.len()).map(|i| !is_wire(i)).collect();
        // Ground elements can sink current (via gmin it is ~0, but a
        // grounded junction's balance may be off by the ground current);
        // treat junctions containing a Ground as never-solvable instead of
        // wrong. Same for solitary endpoints: nothing to solve.
        loop {
            let mut progressed = false;
            for (j, inc) in incident.iter().enumerate() {
                if inc
                    .iter()
                    .any(|(i, _)| matches!(self.elems[*i].spec.kind, ElementKind::Ground))
                {
                    continue;
                }
                let unknowns: Vec<&(usize, usize)> =
                    inc.iter().filter(|(i, _)| !known[*i]).collect();
                if unknowns.len() != 1 {
                    continue;
                }
                let &&(wi, wt) = unknowns.first().unwrap();
                // Sum of currents INTO the junction from known branches.
                let mut sum = 0.0;
                for &(i, t) in inc {
                    if i == wi || !known[i] {
                        continue;
                    }
                    // current field is a -> b through the element, so it
                    // arrives at the junction via terminal b (t == 1).
                    let c = frames[i].current;
                    sum += if t == 1 { c } else { -c };
                }
                // The unknown wire must carry that sum away. If the wire
                // leaves via terminal a (wt == 0), current a -> b = sum.
                frames[wi].current = if wt == 0 { sum } else { -sum };
                known[wi] = true;
                progressed = true;
                let _ = j;
            }
            if !progressed {
                break;
            }
        }
    }

    /// Deterministic digest of all continuous + discrete state; the S1
    /// cross-target harness asserts these match bit-for-bit.
    pub fn state_hash(&self) -> u64 {
        use xxhash_rust::xxh3::Xxh3;
        let mut h = Xxh3::new();
        let mut put = |x: f64| h.update(&sim_math::canon(x).to_bits().to_le_bytes());
        put(self.time);
        for v in &self.x {
            put(*v);
        }
        for e in &self.elems {
            put(e.state.v_prev);
            put(e.state.i_prev);
            put(e.state.current);
        }
        h.digest()
    }
}

/// SPICE-style junction voltage limiting: keeps NR from exponent overflow
/// by pulling large forward-bias steps back onto the exponential.
fn pnjlim(vnew: f64, vold: f64) -> f64 {
    let vt = DIODE_VT;
    let vcrit = vt * libm::log(vt / (core::f64::consts::SQRT_2 * DIODE_IS));
    if vnew > vcrit && (vnew - vold).abs() > vt + vt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                vold + vt * libm::log(arg)
            } else {
                vcrit
            }
        } else {
            vt * libm::log(vnew / vt)
        }
    } else {
        vnew
    }
}
