//! The MNA engine: compile a netlist into a solvable system, advance it in
//! fixed timesteps, keep every displayed number honest.
//!
//! Unknown vector layout: `[v_node1 .. v_nodeN, i_branch1 .. i_branchM]`
//! where branches are voltage-source-like elements (sources, closed
//! switches, op-amp outputs). Node 0 is ground and is not an unknown.
//! Every node gets a `GMIN` leak to ground so floating circuits stay
//! solvable (beginner-tolerant solver).
//!
//! Sign conventions used throughout:
//! - `pin_i[p]` is the current flowing INTO the element at pin `p`.
//! - A constant current I into pin p stamps `b[p] -= I`.
//! - A dependence dI_p/dV_n stamps `a[p][n] += g`.

use crate::netlist::{ElementKind, ElementSpec, InteractOp, ParamWrite, Point, MAX_PINS};
use sim_math::DenseLu;

pub const GMIN: f64 = 1e-12;

/// Thermal voltage at room temperature.
const VT: f64 = 0.025865;
/// Default diode: n = 2 emission (Falstad-family, NR-friendly).
const DIODE_IS: f64 = 1.7143528192808883e-7;
const DIODE_NVT: f64 = 2.0 * VT;
/// LED: tuned for a ~2.1 V forward drop at 20 mA.
const LED_IS: f64 = 1e-20;
const LED_NVT: f64 = 0.05;
/// Zener: n = 1 junction both directions; knee offset places 5 mA at -vz.
const ZENER_IS: f64 = 1e-14;
const ZENER_NVT: f64 = VT;
/// BJT Ebers-Moll.
const BJT_IS: f64 = 1e-14;
const BJT_BETA_R: f64 = 1.0;
/// MOSFET off-state drain-source leak, and per-iteration voltage damping.
const MOS_LEAK: f64 = 1e-8;
const MOS_DAMP: f64 = 0.5;
/// Op-amp open-loop gain and input offset voltage. The offset is a real
/// device property, and it matters here: an ideal offset-free op-amp in a
/// positive-feedback loop has an exact metastable solution that a
/// noiseless deterministic solver would sit on forever — the offset is
/// what lets relaxation oscillators and flip-flops self-start.
const OPAMP_GAIN: f64 = 1e5;
const OPAMP_VOFF: f64 = 1e-4;
/// OTA bias-pin diode (LM13700-style: Iabc injected into a junction).
const OTA_IS: f64 = 1e-14;
/// Bipolar 555: totem-pole output drops (sourcing from VCC / sinking to
/// GND), the saturated discharge transistor's conductance (10 Ω), and the
/// quiescent supply conductance (~3 mA across a 9 V rail).
const T555_VDROP_HIGH: f64 = 1.2;
const T555_VSAT_LOW: f64 = 0.1;
const T555_G_DIS: f64 = 0.1;
const T555_G_QUIESCENT: f64 = 3.3e-4;
/// Comparator thresholds as fractions of the live supply, from the
/// internal 3-resistor divider.
const T555_THR_FRAC: f64 = 2.0 / 3.0;
const T555_TRIG_FRAC: f64 = 1.0 / 3.0;

const NR_MAX_ITERS: usize = 100;
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
    /// Junction-voltage NR guesses: diode vd / BJT (vbe, vbc) — stored
    /// polarity-normalized so PNP shares the NPN code path.
    vg1: f64,
    vg2: f64,
    /// Op-amp rail region: -1, 0 (linear), +1. Doubles as the 555's RS
    /// latch: 0 = output low, 1 = output high.
    region: i8,
    /// Damped per-pin voltages for MOSFET NR stabilization.
    lastv: [f64; MAX_PINS],
    /// Currents INTO the element per pin, from the last accepted step.
    pin_i: [f64; MAX_PINS],
}

struct CompiledElem {
    spec: ElementSpec,
    /// Electrical node index per pin (0 = ground; unused pins 0).
    node: [usize; MAX_PINS],
    /// Index into the branch-current unknowns for branch devices.
    branch: Option<usize>,
    state: ElemState,
    /// The part has failed OPEN: it stamps nothing, owns no branch unknown
    /// and carries no current. Its pins remain junction points, so anything
    /// else wired to them keeps working — a dead part is a gap, not a hole
    /// in the netlist.
    ///
    /// This is the ONLY damage mechanism inside sim-core. Ratings, thermal
    /// accumulators and the decision to break live outside the solve path
    /// (see `crates/damage`), because none of that is numerics and none of
    /// it may perturb the golden state hashes.
    broken: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AdvanceReport {
    pub steps: u32,
    pub nr_iters: u32,
    pub rescues: u32,
    pub quarantined: bool,
}

/// An O(1) handle to one compiled element, for callers that sample the same
/// element far more often than once per tick (audio taps). Obtained from
/// [`Engine::tap`]; invalidated by [`Engine::set_elements`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElemTap {
    slot: usize,
}

/// Per-element view of the live simulation for rendering: everything the
/// client paints comes from here and nowhere else.
#[derive(Clone, Copy, Debug, Default)]
pub struct ElemFrame {
    pub id: u32,
    pub npins: usize,
    /// Voltage at each pin.
    pub v: [f64; MAX_PINS],
    /// Current INTO the element at each pin (wires included, via KCL
    /// propagation).
    pub i: [f64; MAX_PINS],
    /// Dissipated power (negative = delivering power).
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
    /// Count of numeric factorizations since construction (instrumentation
    /// only; never read by the solver, never hashed).
    factorizations: u64,
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
            factorizations: 0,
        }
    }

    // ------------------------------------------------------ instrumentation
    // Read-only views for the scale benchmark (`sim-golden`, bin `scale`).
    // None of these participate in the solve or in `state_hash`.

    /// MNA unknowns: `num_nodes + num_branches`. There is exactly ONE
    /// matrix for the whole world — disconnected islands share it.
    pub fn unknowns(&self) -> usize {
        self.n
    }

    pub fn node_count(&self) -> usize {
        self.num_nodes
    }

    pub fn branch_count(&self) -> usize {
        self.num_branches
    }

    pub fn element_count(&self) -> usize {
        self.elems.len()
    }

    /// False if ANY element is nonlinear: the flag is global, so one diode
    /// makes the entire world refactor on every NR iteration.
    pub fn is_linear(&self) -> bool {
        self.linear
    }

    /// The last stamped MNA matrix, row-major `n x n`.
    pub fn matrix(&self) -> &[f64] {
        &self.a
    }

    /// Structural nonzeros in the last stamped matrix.
    pub fn matrix_nnz(&self) -> usize {
        self.a.iter().filter(|v| **v != 0.0).count()
    }

    /// The last solved unknown vector.
    pub fn solution(&self) -> &[f64] {
        &self.x
    }

    /// Numeric factorizations performed since construction.
    pub fn factorizations(&self) -> u64 {
        self.factorizations
    }

    /// `(element id, node index per pin)` in element order — lets a caller
    /// map unknowns back to the part of the world that owns them.
    pub fn element_nodes(&self) -> Vec<(u32, [usize; MAX_PINS])> {
        self.elems.iter().map(|e| (e.spec.id, e.node)).collect()
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
    /// inductor current) and the broken flag survive for elements whose id
    /// persists: moving a dead resistor does not repair it.
    pub fn set_elements(&mut self, specs: &[ElementSpec]) {
        let old_state: Vec<(u32, ElemState, bool)> = self
            .elems
            .iter()
            .map(|e| (e.spec.id, e.state, e.broken))
            .collect();
        self.elems.clear();
        for s in specs {
            if s.pins.len() != s.kind.pin_count() {
                continue; // malformed element: drop rather than panic
            }
            let (state, broken) = old_state
                .iter()
                .find(|(id, _, _)| *id == s.id)
                .map(|(_, st, b)| (*st, *b))
                .unwrap_or_default();
            self.elems.push(CompiledElem {
                spec: s.clone(),
                node: [0; MAX_PINS],
                branch: None,
                state,
                broken,
            });
        }
        self.compile();
    }

    /// Break a part open, or repair it. Returns false when the id is unknown.
    ///
    /// A break/repair is a world EVENT, not a numeric write: it changes the
    /// unknown count (a broken source or switch loses its branch) so it goes
    /// through the full compile path exactly like a switch flip, which also
    /// re-arms the post-event backward-Euler steps and clears `quarantined`.
    /// That is correct for both directions: the circuit really did change, and
    /// a solver that diverged on the old topology deserves a fresh start on
    /// the new one. (Contrast `write_param`, which fires at kHz rates and must
    /// therefore carry those flags across untouched.)
    ///
    /// The part's continuous state is reset both ways: a part that has just
    /// released its magic smoke has no charge or flux left, and a repaired one
    /// is a new part out of the drawer.
    pub fn set_broken(&mut self, id: u32, broken: bool) -> bool {
        let Some(e) = self.elems.iter_mut().find(|e| e.spec.id == id) else {
            return false;
        };
        if e.broken == broken {
            return true; // idempotent, and free: no recompile
        }
        e.broken = broken;
        e.state = ElemState::default();
        self.compile();
        true
    }

    /// Has this part failed open? Unknown ids read false.
    pub fn is_broken(&self, id: u32) -> bool {
        self.elems.iter().any(|e| e.spec.id == id && e.broken)
    }

    pub fn interact(&mut self, id: u32, op: InteractOp) {
        let Some(e) = self.elems.iter_mut().find(|e| e.spec.id == id) else {
            return;
        };
        match (op, &mut e.spec.kind) {
            (InteractOp::SetSwitch { closed }, ElementKind::Switch { closed: c })
            | (InteractOp::SetSwitch { closed }, ElementKind::Button { closed: c }) => *c = closed,
            (InteractOp::SetValue { value }, k) => match k {
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => *ohms = value.max(1e-6),
                ElementKind::Capacitor { farads } => *farads = value.max(1e-15),
                ElementKind::Inductor { henries } => *henries = value.max(1e-12),
                ElementKind::VoltageSource { dc, .. } | ElementKind::Rail { dc, .. } => *dc = value,
                ElementKind::CurrentSource { amps } => *amps = value,
                ElementKind::Potentiometer { wiper, .. } => *wiper = value.clamp(0.01, 0.99),
                _ => return,
            },
            _ => return,
        }
        // Switch flips change topology (branch count); value changes only
        // invalidate the factorization. Recompiling handles both and is
        // cheap at current scale.
        self.compile();
    }

    /// Write a live element's parameter from a co-simulated machine, at the
    /// cheapest correct cost (see `ParamWrite`). Returns false when the id
    /// or the parameter/device pairing does not exist.
    ///
    /// This is deliberately NOT `interact()`: machine writes land at kHz
    /// rates, and `interact()`/`compile()` both clear `quarantined` and
    /// re-arm `be_steps`. Clearing quarantine that often would resurrect a
    /// diverged circuit every 640 µs and hide the failure forever; re-arming
    /// BE would silently keep the integrator in first order.
    pub fn write_param(&mut self, id: u32, write: ParamWrite) -> bool {
        let Some(e) = self.elems.iter_mut().find(|e| e.spec.id == id) else {
            return false;
        };
        let mut invalidate = false;
        let mut topology = false;
        match (write, &mut e.spec.kind) {
            (ParamWrite::Bemf { volts }, ElementKind::Motor { bemf, .. }) => {
                // RHS only: `build()` rewrites b[branch] every step.
                *bemf = volts;
            }
            (ParamWrite::Wiper { frac }, ElementKind::Potentiometer { wiper, .. }) => {
                let new = frac.clamp(0.01, 0.99);
                if *wiper != new {
                    *wiper = new;
                    invalidate = true;
                }
            }
            (ParamWrite::Switch { closed }, ElementKind::Switch { closed: c }) => {
                if *c != closed {
                    *c = closed;
                    topology = true;
                }
            }
            _ => return false,
        }
        if invalidate {
            self.factor_valid = false;
        }
        if topology {
            // A branch appears/disappears: only the compile path can
            // renumber the unknowns. Carry the solver's health flags across
            // it untouched.
            let (be_steps, quarantined) = (self.be_steps, self.quarantined);
            self.compile();
            self.be_steps = be_steps;
            self.quarantined = quarantined;
        }
        true
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
        let mut ends: Vec<Vec<usize>> = Vec::with_capacity(self.elems.len());
        for e in &self.elems {
            ends.push(e.spec.pins.iter().map(|p| jix(&mut points, *p)).collect());
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
            e.node = [0; MAX_PINS];
            for (i, j) in je.iter().enumerate() {
                e.node[i] = node_of_junction[*j];
            }
            // A broken part owns no unknown: it is an open circuit that
            // happens to still be drawn on the schematic.
            e.branch = (!e.broken && e.spec.kind.is_branch()).then(|| {
                num_branches += 1;
                num_branches - 1
            });
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
        // A broken nonlinear device stamps nothing, so it cannot make the
        // system nonlinear: the last dead LED in a room hands the solver its
        // single-pass linear path back.
        self.linear = !self
            .elems
            .iter()
            .any(|e| !e.broken && e.spec.kind.is_nonlinear());
        self.factor_valid = false;
        self.quarantined = false;
        self.be_steps = BE_STEPS_AFTER_EVENT;
    }

    /// Stamp and LU-factor the system for the current document WITHOUT
    /// advancing any state: true when the MNA matrix is nonsingular, i.e.
    /// the next step could at least factor. This is the structural half of
    /// placement validation (`crate::validate`) — a `false` here is exactly
    /// the condition that would quarantine the engine one step later.
    ///
    /// Backward Euler on purpose: it is what the post-edit event steps run,
    /// and structural singularity (dependent source rows, zero rows) is
    /// independent of `h` and the integration mode anyway. Device state and
    /// the solution vector are untouched; the trial factorization is
    /// discarded so a subsequent step re-stamps from scratch.
    pub fn probe_solvable(&mut self) -> bool {
        if self.n == 0 {
            return true; // an empty world is trivially solvable
        }
        // Clear BEFORE as well as after. `build()` skips stamping AND
        // factoring when a retained factorization is still valid, so probing
        // with one armed would answer `true` without testing anything — the
        // probe must always do the work it claims to cost.
        self.factor_valid = false;
        let ok = self.build(self.time + self.dt, self.dt, true).is_ok();
        self.factor_valid = false;
        ok
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
        let iters = if self.linear { 1 } else { NR_MAX_ITERS };
        let mut converged = self.linear;
        // Reset per-pass discrete-state-change budgets for the op-amp rail
        // region and the 555 latch (lastv[0] doubles as the counter for
        // both; neither has MOS damping state).
        for e in self.elems.iter_mut() {
            if matches!(
                e.spec.kind,
                ElementKind::OpAmp { .. } | ElementKind::Timer555
            ) {
                e.state.lastv[0] = 0.0;
            }
        }
        for _ in 0..iters {
            report.nr_iters += 1;
            self.build(self.time + h, h, be)?;
            self.x.copy_from_slice(&self.b);
            self.lu.solve(&mut self.x);
            if self.x.iter().any(|v| !v.is_finite()) {
                return Err(());
            }
            if self.linear {
                break;
            }
            converged = self.update_guesses();
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
    fn xv(&self, node: usize) -> f64 {
        if node == 0 {
            0.0
        } else {
            self.x[node - 1]
        }
    }

    // ------------------------------------------------------------ stamping

    #[inline]
    fn stamp_g(&mut self, p: usize, q: usize, g: f64) {
        let n = self.n;
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

    /// dI(into element at pin-node p)/dV(node q).
    #[inline]
    fn stamp_partial(&mut self, p: usize, q: usize, g: f64) {
        if p > 0 && q > 0 {
            self.a[(p - 1) * self.n + (q - 1)] += g;
        }
    }

    /// Constant current I INTO the element at pin-node p.
    #[inline]
    fn stamp_i_into(&mut self, p: usize, i: f64) {
        if p > 0 {
            self.b[p - 1] -= i;
        }
    }

    /// Stamp the full system for time `t_new` and step `h`. Factors the
    /// matrix unless a valid linear factorization is being reused.
    fn build(&mut self, t_new: f64, h: f64, be: bool) -> Result<(), ()> {
        let need_factor =
            !(self.linear && self.factor_valid && self.factored_h == h && self.factored_be == be);
        if need_factor {
            self.a.iter_mut().for_each(|v| *v = 0.0);
        }
        self.b.iter_mut().for_each(|v| *v = 0.0);

        if need_factor {
            let n = self.n;
            for k in 0..self.num_nodes {
                self.a[k * n + k] += GMIN;
            }
        }

        for ei in 0..self.elems.len() {
            let (kind, node, branch, state, broken) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch, e.state, e.broken)
            };
            if broken {
                continue; // failed open: stamps nothing at all
            }
            let n = self.n;
            match kind {
                ElementKind::Wire | ElementKind::Ground => {}
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => {
                    if need_factor {
                        self.stamp_g(node[0], node[1], 1.0 / ohms);
                    }
                }
                ElementKind::Potentiometer { ohms, wiper } => {
                    if need_factor {
                        let r1 = (ohms * wiper).max(1e-3);
                        let r2 = (ohms * (1.0 - wiper)).max(1e-3);
                        self.stamp_g(node[0], node[1], 1.0 / r1);
                        self.stamp_g(node[1], node[2], 1.0 / r2);
                    }
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let ieq = if be {
                        -geq * state.v_prev
                    } else {
                        -(geq * state.v_prev + state.i_prev)
                    };
                    if need_factor {
                        self.stamp_g(node[0], node[1], geq);
                    }
                    self.stamp_i_into(node[0], ieq);
                    self.stamp_i_into(node[1], -ieq);
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let ieq = if be {
                        state.i_prev
                    } else {
                        state.i_prev + geq * state.v_prev
                    };
                    if need_factor {
                        self.stamp_g(node[0], node[1], geq);
                    }
                    self.stamp_i_into(node[0], ieq);
                    self.stamp_i_into(node[1], -ieq);
                }
                ElementKind::CurrentSource { amps } => {
                    self.stamp_i_into(node[0], amps);
                    self.stamp_i_into(node[1], -amps);
                }
                ElementKind::VoltageSource { dc, amp, hz, phase } => {
                    let v = if amp == 0.0 {
                        dc
                    } else {
                        dc + amp * libm::sin(TWO_PI * hz * t_new + phase)
                    };
                    let bi = self.num_nodes + branch.ok_or(())?;
                    if need_factor {
                        for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                            if pin > 0 {
                                self.a[bi * n + (pin - 1)] += sgn;
                                self.a[(pin - 1) * n + bi] += sgn;
                            }
                        }
                    }
                    self.b[bi] = v;
                }
                ElementKind::Rail { dc, amp, hz, phase } => {
                    // A voltage source whose far terminal IS ground: only the
                    // one pin stamps, and node 0 has no row to receive the
                    // return current (exactly like a grounded two-pin source).
                    let v = if amp == 0.0 {
                        dc
                    } else {
                        dc + amp * libm::sin(TWO_PI * hz * t_new + phase)
                    };
                    let bi = self.num_nodes + branch.ok_or(())?;
                    if need_factor && node[0] > 0 {
                        self.a[bi * n + (node[0] - 1)] += 1.0;
                        self.a[(node[0] - 1) * n + bi] += 1.0;
                    }
                    self.b[bi] = v;
                }
                ElementKind::Motor {
                    ohms,
                    henries,
                    bemf,
                } => {
                    // v0 - v1 = R·i + L·di/dt + bemf with i the branch
                    // unknown (current INTO pin 0). Backward Euler on the
                    // inductive term — di/dt ≈ (i - i_prev)/h — gives the
                    // row  v0 - v1 - (R + L/h)·i = bemf - (L/h)·i_prev.
                    // BE unconditionally: the armature pole (L/R = 0.75 ms
                    // for the shipped hoist motor) is stiff next to the
                    // machine tick, and BE cannot ring against it.
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let gl = henries / h;
                    if need_factor {
                        for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                            if pin > 0 {
                                self.a[bi * n + (pin - 1)] += sgn;
                                self.a[(pin - 1) * n + bi] += sgn;
                            }
                        }
                        self.a[bi * n + bi] -= ohms + gl;
                    }
                    self.b[bi] = bemf - gl * state.i_prev;
                }
                ElementKind::Switch { closed } | ElementKind::Button { closed } => {
                    if closed {
                        let bi = self.num_nodes + branch.ok_or(())?;
                        if need_factor {
                            for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                                if pin > 0 {
                                    self.a[bi * n + (pin - 1)] += sgn;
                                    self.a[(pin - 1) * n + bi] += sgn;
                                }
                            }
                        }
                        self.b[bi] = 0.0;
                    }
                }
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let vg = state.vg1;
                    let ef = libm::exp(vg / nvt);
                    let mut g = is / nvt * ef;
                    let mut i0 = is * (ef - 1.0);
                    if let Some(voff) = voff {
                        // Reverse breakdown branch (Zener).
                        let er = libm::exp(-(vg + voff) / nvt);
                        g += is / nvt * er;
                        i0 -= is * er;
                    }
                    let i_lin = i0 - g * vg;
                    self.stamp_g(node[0], node[1], g);
                    self.stamp_i_into(node[0], i_lin);
                    self.stamp_i_into(node[1], -i_lin);
                }
                ElementKind::Npn { beta } | ElementKind::Pnp { beta } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let (b_, c, e) = (node[0], node[1], node[2]);
                    let (vbe, vbc) = (state.vg1, state.vg2);
                    let ebe = libm::exp(vbe / VT);
                    let ebc = libm::exp(vbc / VT);
                    let gbe = BJT_IS / VT * ebe;
                    let gbc = BJT_IS / VT * ebc;
                    let i_f = BJT_IS * (ebe - 1.0);
                    let i_r = BJT_IS * (ebc - 1.0);
                    // Currents into collector/base (polarity-normalized).
                    let ic = i_f - i_r * (1.0 + 1.0 / BJT_BETA_R);
                    let ib = i_f / beta + i_r / BJT_BETA_R;
                    let d_ic = (gbe, -gbc * (1.0 + 1.0 / BJT_BETA_R)); // (d/dvbe, d/dvbc)
                    let d_ib = (gbe / beta, gbc / BJT_BETA_R);
                    // Conductance stamps are polarity-independent (pol^2).
                    for (pin, cur, d) in [(c, ic, d_ic), (b_, ib, d_ib)] {
                        self.stamp_partial(pin, b_, d.0 + d.1);
                        self.stamp_partial(pin, e, -d.0);
                        self.stamp_partial(pin, c, -d.1);
                        self.stamp_i_into(pin, pol * (cur - d.0 * vbe - d.1 * vbc));
                    }
                    // Emitter = -(collector + base).
                    let d_ie = (-(d_ic.0 + d_ib.0), -(d_ic.1 + d_ib.1));
                    self.stamp_partial(e, b_, d_ie.0 + d_ie.1);
                    self.stamp_partial(e, e, -d_ie.0);
                    self.stamp_partial(e, c, -d_ie.1);
                    let ie = -(ic + ib);
                    self.stamp_i_into(e, pol * (ie - d_ie.0 * vbe - d_ie.1 * vbc));
                }
                ElementKind::Nmos { vt, k } | ElementKind::Pmos { vt, k } => {
                    let pol = if matches!(kind, ElementKind::Nmos { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let m = mos_eval(pol, vt, k, &state.lastv);
                    // Currents into effective drain d / source s.
                    let (g_, d, s) = (node[0], m.d_pin(node), m.s_pin(node));
                    let i0 = pol * (m.id - m.gm * m.vgs - m.gds * m.vds);
                    self.stamp_partial(d, g_, m.gm);
                    self.stamp_partial(d, d, m.gds);
                    self.stamp_partial(d, s, -(m.gm + m.gds));
                    self.stamp_i_into(d, i0);
                    self.stamp_partial(s, g_, -m.gm);
                    self.stamp_partial(s, d, -m.gds);
                    self.stamp_partial(s, s, m.gm + m.gds);
                    self.stamp_i_into(s, -i0);
                }
                ElementKind::Ota => {
                    let (p, m, out, bias) = (node[0], node[1], node[2], node[3]);
                    // Bias pin: diode junction to ground; injected current
                    // is Iabc.
                    let vb = state.vg2;
                    let eb = libm::exp(vb / VT);
                    let g_b = OTA_IS / VT * eb;
                    let i_b = OTA_IS * (eb - 1.0);
                    self.stamp_partial(bias, bias, g_b);
                    self.stamp_i_into(bias, i_b - g_b * vb);
                    // Output: Iout = Iabc * tanh(vd / 2Vt) flowing OUT of
                    // the out pin. Linearize in vd AND vbias.
                    let iabc = i_b.max(0.0);
                    let vd = state.vg1;
                    let th = libm::tanh(vd / (2.0 * VT));
                    let gm_eff = iabc / (2.0 * VT) * (1.0 - th * th);
                    let d_ib = if i_b > 0.0 { g_b } else { 0.0 };
                    let iout = iabc * th;
                    // I_into(out) = -Iout; partials are negated.
                    self.stamp_partial(out, p, -gm_eff);
                    self.stamp_partial(out, m, gm_eff);
                    self.stamp_partial(out, bias, -d_ib * th);
                    self.stamp_i_into(out, -iout + gm_eff * vd + d_ib * th * vb);
                }
                ElementKind::Timer555 => {
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let (vcc, gp, out, dis) = (node[0], node[1], node[4], node[5]);
                    // Quiescent supply current: the chip's own bias
                    // network, so the rails carry current even with the
                    // output unloaded and KCL stays sane.
                    self.stamp_g(vcc, gp, T555_G_QUIESCENT);
                    // Discharge pin: saturated transistor to GND while the
                    // latch is low, open circuit while it is high.
                    if state.region == 0 {
                        self.stamp_g(dis, gp, T555_G_DIS);
                    }
                    // Totem-pole output as a branch voltage source, referred
                    // to the rail it is working against: high sources from
                    // the VCC pin at vcc - 1.2 V, low sinks into the GND pin
                    // at 0.1 V. Tying the return to a supply pin is what
                    // makes the output current actually come out of the
                    // battery instead of appearing from nowhere.
                    let (ret, drop) = if state.region != 0 {
                        (vcc, -T555_VDROP_HIGH)
                    } else {
                        (gp, T555_VSAT_LOW)
                    };
                    if out > 0 {
                        self.a[(out - 1) * n + bi] += 1.0;
                        self.a[bi * n + (out - 1)] += 1.0;
                    }
                    if ret > 0 {
                        self.a[(ret - 1) * n + bi] -= 1.0;
                        self.a[bi * n + (ret - 1)] -= 1.0;
                    }
                    self.b[bi] = drop;
                }
                ElementKind::OpAmp { rail } => {
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let (p, m, out) = (node[0], node[1], node[2]);
                    // Output branch current column.
                    if out > 0 {
                        self.a[(out - 1) * n + bi] += 1.0;
                    }
                    // Constraint row depends on rail region.
                    match state.region {
                        0 => {
                            if p > 0 {
                                self.a[bi * n + (p - 1)] += OPAMP_GAIN;
                            }
                            if m > 0 {
                                self.a[bi * n + (m - 1)] -= OPAMP_GAIN;
                            }
                            if out > 0 {
                                self.a[bi * n + (out - 1)] -= 1.0;
                            }
                            // vout = A(vp - vm + Voff)
                            self.b[bi] = -OPAMP_GAIN * OPAMP_VOFF;
                        }
                        r => {
                            if out > 0 {
                                self.a[bi * n + (out - 1)] += 1.0;
                            }
                            self.b[bi] = r as f64 * rail;
                        }
                    }
                }
            }
        }

        if need_factor {
            self.factorizations += 1;
            let ok = {
                let a = core::mem::take(&mut self.a);
                let ok = self.lu.factor(&a);
                self.a = a;
                ok
            };
            #[cfg(feature = "dump-matrix")]
            {
                std::eprintln!("t={t_new} n={}", self.n);
                for r in 0..self.n {
                    let row: Vec<f64> = (0..self.n).map(|c| self.a[r * self.n + c]).collect();
                    std::eprintln!("  {row:?} | {}", self.b[r]);
                }
            }
            if !ok {
                return Err(());
            }
            if self.linear {
                self.factor_valid = true;
                self.factored_h = h;
                self.factored_be = be;
            }
        }
        let _ = t_new;
        Ok(())
    }

    /// Post-solve NR bookkeeping: limit and store each nonlinear device's
    /// new operating-point guess. Returns true when every device agrees
    /// with its previous guess (converged).
    fn update_guesses(&mut self) -> bool {
        let mut converged = true;
        let close = |a: f64, b: f64| (a - b).abs() < NR_ABSTOL + NR_RELTOL * a.abs().max(b.abs());
        for ei in 0..self.elems.len() {
            let (kind, node, broken) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.broken)
            };
            if broken {
                continue; // no operating point to iterate: it stamps nothing
            }
            match kind {
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let vd = self.xv(node[0]) - self.xv(node[1]);
                    let old = self.elems[ei].state.vg1;
                    let vcrit = nvt * libm::log(nvt / (core::f64::consts::SQRT_2 * is));
                    let new = if let (Some(voff), true) = (voff, vd < 0.0) {
                        // Limit the reverse junction like a forward one.
                        -(pnjlim(-(vd + voff), -(old + voff), nvt, vcrit)) - voff
                    } else {
                        pnjlim(vd, old, nvt, vcrit)
                    };
                    if !close(new, old) {
                        converged = false;
                    }
                    self.elems[ei].state.vg1 = new;
                }
                ElementKind::Npn { .. } | ElementKind::Pnp { .. } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let vcrit = VT * libm::log(VT / (core::f64::consts::SQRT_2 * BJT_IS));
                    let vbe = pol * (self.xv(node[0]) - self.xv(node[2]));
                    let vbc = pol * (self.xv(node[0]) - self.xv(node[1]));
                    let st = &mut self.elems[ei].state;
                    let nbe = pnjlim(vbe, st.vg1, VT, vcrit);
                    let nbc = pnjlim(vbc, st.vg2, VT, vcrit);
                    if !close(nbe, st.vg1) || !close(nbc, st.vg2) {
                        converged = false;
                    }
                    st.vg1 = nbe;
                    st.vg2 = nbc;
                }
                ElementKind::Nmos { .. } | ElementKind::Pmos { .. } => {
                    let vs: Vec<f64> = (0..3).map(|p| self.xv(node[p])).collect();
                    let st = &mut self.elems[ei].state;
                    for (last, v) in st.lastv.iter_mut().zip(vs.iter()) {
                        let delta = v - *last;
                        if delta.abs() > 0.01 {
                            converged = false;
                        }
                        *last += delta.clamp(-MOS_DAMP, MOS_DAMP);
                    }
                }
                ElementKind::Ota => {
                    let vd = self.xv(node[0]) - self.xv(node[1]);
                    let vb = self.xv(node[3]);
                    let vcrit = VT * libm::log(VT / (core::f64::consts::SQRT_2 * OTA_IS));
                    let st = &mut self.elems[ei].state;
                    let nb = pnjlim(vb, st.vg2, VT, vcrit);
                    if !close(vd, st.vg1) || !close(nb, st.vg2) {
                        converged = false;
                    }
                    st.vg1 = vd; // tanh is safe at any argument; no limiting
                    st.vg2 = nb;
                }
                ElementKind::Timer555 => {
                    // Thresholds track the LIVE supply through the internal
                    // divider: a sagging rail moves both comparators.
                    let vg = self.xv(node[1]);
                    let vcc = self.xv(node[0]) - vg;
                    let vtrig = self.xv(node[2]) - vg;
                    let vthr = self.xv(node[3]) - vg;
                    let st = &mut self.elems[ei].state;
                    // RS latch. Trigger below vcc/3 sets (output high) and
                    // dominates — holding TRIG low pins the output high on
                    // a real 555 too; threshold above 2·vcc/3 resets.
                    let latch = if vtrig < vcc * T555_TRIG_FRAC {
                        1
                    } else if vthr > vcc * T555_THR_FRAC {
                        0
                    } else {
                        st.region
                    };
                    // At most 2 latch changes per NR pass, exactly like the
                    // op-amp rail regions: right at a comparator crossing
                    // the two states can point at each other forever, and
                    // holding the current one yields a consistent solve that
                    // the next substep's capacitor motion resolves.
                    if latch != st.region && st.lastv[0] < 2.0 {
                        st.lastv[0] += 1.0;
                        converged = false;
                        st.region = latch;
                    }
                }
                ElementKind::OpAmp { rail } => {
                    let target = OPAMP_GAIN * (self.xv(node[0]) - self.xv(node[1]) + OPAMP_VOFF);
                    let st = &mut self.elems[ei].state;
                    let new_region = match st.region {
                        0 => {
                            let vout = self.x[if node[2] > 0 { node[2] - 1 } else { 0 }];
                            if node[2] > 0 && vout.abs() > rail * 1.000001 {
                                if vout > 0.0 {
                                    1
                                } else {
                                    -1
                                }
                            } else {
                                0
                            }
                        }
                        r => {
                            // Any opposing drive flips DIRECTLY to the
                            // other rail: positive-feedback circuits leave
                            // a rail hard, and routing through the linear
                            // region would chatter (its solution sits back
                            // on the old rail) until NR gives up —
                            // Schmitt triggers and relaxation oscillators
                            // with slow RC vs dt hit this exactly at the
                            // threshold crossing. Only a weakening
                            // same-sign drive relaxes to linear (negative
                            // feedback coming out of saturation).
                            let drive = (r as f64) * target;
                            if drive >= rail {
                                r
                            } else if drive < 0.0 {
                                -r
                            } else {
                                0
                            }
                        }
                    };
                    if new_region != st.region {
                        // At most 2 region changes per NR pass. At the
                        // exact threshold crossing (within the offset
                        // window, microvolts wide) the railed and linear
                        // regions can point at each other indefinitely;
                        // holding the current region yields a consistent
                        // solve, and the next substep's capacitor motion
                        // resolves the ambiguity cleanly.
                        if st.lastv[0] < 2.0 {
                            st.lastv[0] += 1.0;
                            converged = false;
                            st.region = new_region;
                        }
                    }
                }
                _ => {}
            }
        }
        converged
    }

    /// Commit device history and pin currents from the solved unknowns.
    fn accept(&mut self, h: f64, be: bool) {
        for ei in 0..self.elems.len() {
            let (kind, node, branch, broken) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch, e.broken)
            };
            if broken {
                // Carries nothing, stores nothing. Reported as exactly zero so
                // no display can show a stale current for a dead part.
                let st = &mut self.elems[ei].state;
                st.pin_i = [0.0; MAX_PINS];
                st.v_prev = 0.0;
                st.i_prev = 0.0;
                continue;
            }
            let v01 = self.xv(node[0]) - self.xv(node[1]);
            let bi_val = branch.map(|b| self.x[self.num_nodes + b]);
            let mut vs = [0.0; MAX_PINS];
            for (k, v) in vs.iter_mut().enumerate() {
                *v = self.xv(node[k]);
            }
            let st = &mut self.elems[ei].state;
            let mut two = |i: f64| {
                st.pin_i = [0.0; MAX_PINS];
                st.pin_i[0] = i;
                st.pin_i[1] = -i;
            };
            match kind {
                ElementKind::Wire | ElementKind::Ground => {}
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => two(v01 / ohms),
                ElementKind::Potentiometer { ohms, wiper } => {
                    let r1 = (ohms * wiper).max(1e-3);
                    let r2 = (ohms * (1.0 - wiper)).max(1e-3);
                    let ia = (vs[0] - vs[1]) / r1;
                    let ib = (vs[2] - vs[1]) / r2;
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = ia;
                    st.pin_i[1] = -(ia + ib);
                    st.pin_i[2] = ib;
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let i = if be {
                        geq * (v01 - st.v_prev)
                    } else {
                        geq * (v01 - st.v_prev) - st.i_prev
                    };
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let i = if be {
                        st.i_prev + geq * v01
                    } else {
                        st.i_prev + geq * (v01 + st.v_prev)
                    };
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::CurrentSource { amps } => two(amps),
                ElementKind::VoltageSource { .. } => two(bi_val.unwrap_or(0.0)),
                ElementKind::Rail { .. } => {
                    // One real pin: its current is the branch unknown; the
                    // return leg lives in ground and has no pin to report.
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = bi_val.unwrap_or(0.0);
                }
                ElementKind::Motor { .. } => {
                    // The armature current is the branch unknown; it is also
                    // the inductive history for the next step (same slot the
                    // plain inductor uses).
                    let i = bi_val.unwrap_or(0.0);
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::Switch { closed } | ElementKind::Button { closed } => {
                    two(if closed { bi_val.unwrap_or(0.0) } else { 0.0 })
                }
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let mut i = is * (libm::exp(v01 / nvt) - 1.0);
                    if let Some(voff) = voff {
                        i -= is * libm::exp(-(v01 + voff) / nvt);
                    }
                    st.vg1 = v01;
                    two(i);
                }
                ElementKind::Npn { beta } | ElementKind::Pnp { beta } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let vbe = pol * (vs[0] - vs[2]);
                    let vbc = pol * (vs[0] - vs[1]);
                    let i_f = BJT_IS * (libm::exp(vbe / VT) - 1.0);
                    let i_r = BJT_IS * (libm::exp(vbc / VT) - 1.0);
                    let ic = i_f - i_r * (1.0 + 1.0 / BJT_BETA_R);
                    let ib = i_f / beta + i_r / BJT_BETA_R;
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = pol * ib;
                    st.pin_i[1] = pol * ic;
                    st.pin_i[2] = -pol * (ib + ic);
                    st.vg1 = vbe;
                    st.vg2 = vbc;
                }
                ElementKind::Nmos { vt, k } | ElementKind::Pmos { vt, k } => {
                    let pol = if matches!(kind, ElementKind::Nmos { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let m = mos_eval(pol, vt, k, &vs);
                    let id = pol * m.id;
                    st.pin_i = [0.0; MAX_PINS];
                    // Current enters the effective drain, leaves the source.
                    st.pin_i[m.d_index] = id;
                    st.pin_i[m.s_index] = -id;
                }
                ElementKind::OpAmp { .. } => {
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[2] = bi_val.unwrap_or(0.0);
                }
                ElementKind::Timer555 => {
                    st.pin_i = [0.0; MAX_PINS];
                    // Quiescent rail current.
                    let iq = (vs[0] - vs[1]) * T555_G_QUIESCENT;
                    st.pin_i[0] = iq;
                    st.pin_i[1] = -iq;
                    // Discharge transistor (only conducting when low).
                    if st.region == 0 {
                        let idis = (vs[5] - vs[1]) * T555_G_DIS;
                        st.pin_i[5] = idis;
                        st.pin_i[1] -= idis;
                    }
                    // Output branch: sourced from VCC when high, sunk into
                    // GND when low.
                    let io = bi_val.unwrap_or(0.0);
                    st.pin_i[4] = io;
                    st.pin_i[if st.region != 0 { 0 } else { 1 }] -= io;
                }
                ElementKind::Ota => {
                    let eb = libm::exp(vs[3] / VT);
                    let iabc = (OTA_IS * (eb - 1.0)).max(0.0);
                    let iout = iabc * libm::tanh((vs[0] - vs[1]) / (2.0 * VT));
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[2] = -iout;
                    st.pin_i[3] = OTA_IS * (eb - 1.0);
                    st.vg1 = vs[0] - vs[1];
                    st.vg2 = vs[3];
                }
            }
        }
    }

    /// Voltage at an electrical node from the last solve.
    pub fn node_voltage(&self, node: usize) -> f64 {
        self.xv(node)
    }

    /// Voltage at a geometric point, if it is a junction.
    pub fn voltage_at(&self, p: Point) -> Option<f64> {
        self.junctions
            .iter()
            .find(|(q, _)| *q == p)
            .map(|(_, node)| self.xv(*node))
    }

    /// Voltage at one pin of an element, from the last solve.
    pub fn pin_voltage(&self, id: u32, pin: usize) -> Option<f64> {
        let e = self.elems.iter().find(|e| e.spec.id == id)?;
        if pin >= e.spec.pins.len() {
            return None;
        }
        Some(self.xv(e.node[pin]))
    }

    /// Resolve an element id to a handle for repeated sampling. `pin_voltage`
    /// scans the document per call, which is fine once a tick and ruinous at
    /// audio rates (hundreds of samples per tick per tap), so a high-rate
    /// sampler resolves once and then reads through the handle.
    ///
    /// The handle is INVALIDATED by `set_elements` — resolve it again after
    /// any document edit. A stale handle reads 0, it never panics.
    pub fn tap(&self, id: u32) -> Option<ElemTap> {
        let slot = self.elems.iter().position(|e| e.spec.id == id)?;
        Some(ElemTap { slot })
    }

    /// `v(pin a) - v(pin b)` at a tap, from the last accepted step, in O(1).
    /// This is the quantity a voltage-driven device follows: the drive across
    /// a loudspeaker's voice coil is exactly its terminal difference.
    /// Out-of-range slots/pins read 0 so a tap on a deleted element goes
    /// silent instead of panicking.
    pub fn tap_delta(&self, t: ElemTap, a: usize, b: usize) -> f64 {
        let Some(e) = self.elems.get(t.slot) else {
            return 0.0;
        };
        let n = e.spec.pins.len();
        let va = if a < n { self.xv(e.node[a]) } else { 0.0 };
        let vb = if b < n { self.xv(e.node[b]) } else { 0.0 };
        va - vb
    }

    /// The element id a tap currently points at, for callers that want to
    /// confirm a handle still means what they resolved it from.
    pub fn tap_id(&self, t: ElemTap) -> Option<u32> {
        self.elems.get(t.slot).map(|e| e.spec.id)
    }

    /// Current into one pin of an element, from the last accepted step.
    /// NOTE: wires get their current from KCL propagation, which only runs
    /// in `frame()` — for a wire probe, sample via `frame()` instead.
    pub fn pin_current(&self, id: u32, pin: usize) -> Option<f64> {
        let e = self.elems.iter().find(|e| e.spec.id == id)?;
        if pin >= e.spec.pins.len() {
            return None;
        }
        Some(e.state.pin_i[pin])
    }

    pub fn is_wire(&self, id: u32) -> bool {
        self.elems
            .iter()
            .any(|e| e.spec.id == id && matches!(e.spec.kind, ElementKind::Wire))
    }

    /// Per-element render frame. Wire currents are recovered by KCL
    /// propagation over junctions (wires are node-merged so they have no
    /// unknown of their own).
    pub fn frame(&self) -> Vec<ElemFrame> {
        let mut out: Vec<ElemFrame> = self
            .elems
            .iter()
            .map(|e| {
                let npins = e.spec.pins.len();
                let mut v = [0.0; MAX_PINS];
                for (i, val) in v.iter_mut().enumerate().take(npins) {
                    *val = self.xv(e.node[i]);
                }
                let i = e.state.pin_i;
                let power = (0..npins).map(|p| v[p] * i[p]).sum();
                ElemFrame {
                    id: e.spec.id,
                    npins,
                    v,
                    i,
                    power,
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
        let mut incident: Vec<Vec<(usize, usize)>> = vec![Vec::new(); self.junctions.len()];
        let jix = |p: Point| self.junctions.iter().position(|(q, _)| *q == p);
        for (i, e) in self.elems.iter().enumerate() {
            for (t, p) in e.spec.pins.iter().enumerate() {
                if let Some(j) = jix(*p) {
                    incident[j].push((i, t));
                }
            }
        }
        let is_wire = |i: usize| matches!(self.elems[i].spec.kind, ElementKind::Wire);
        let mut known: Vec<bool> = (0..self.elems.len()).map(|i| !is_wire(i)).collect();
        loop {
            let mut progressed = false;
            for inc in incident.iter() {
                // Grounds can sink arbitrary current; their junctions are
                // not solvable by balance.
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
                // Total current flowing from this junction into all known
                // elements; the unknown wire pin must supply it.
                let mut into_known = 0.0;
                for &(i, t) in inc {
                    if i != wi && known[i] {
                        into_known += frames[i].i[t];
                    }
                }
                let pin_current = -into_known;
                frames[wi].i[wt] = pin_current;
                frames[wi].i[1 - wt] = -pin_current;
                known[wi] = true;
                progressed = true;
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
            put(e.state.vg1);
            put(e.state.vg2);
            put(e.state.region as f64);
            for p in 0..MAX_PINS {
                put(e.state.lastv[p]);
                put(e.state.pin_i[p]);
            }
            // Broken parts are discrete state and must reach the digest — but
            // only when one EXISTS. An unconditional field would feed one more
            // f64 per element into the hash and change every golden hash in
            // the S1 cross-target harness, for a feature no golden circuit
            // uses. Nothing broken => not one byte differs.
            if e.broken {
                put(e.spec.id as f64);
            }
        }
        h.digest()
    }
}

/// (saturation current, n·Vt, reverse-breakdown offset).
fn diode_params(kind: &ElementKind) -> (f64, f64, Option<f64>) {
    match kind {
        ElementKind::Led { .. } => (LED_IS, LED_NVT, None),
        ElementKind::Zener { vz } => {
            // Offset the reverse exponent so |i| = 5 mA exactly at -vz.
            let knee = ZENER_NVT * libm::log(0.005 / ZENER_IS);
            (ZENER_IS, ZENER_NVT, Some(vz - knee))
        }
        _ => (DIODE_IS, DIODE_NVT, None),
    }
}

/// Level-1 MOSFET evaluation on polarity-normalized, drain/source-swapped
/// voltages. `id` flows into the effective drain.
struct MosOp {
    id: f64,
    gm: f64,
    gds: f64,
    vgs: f64,
    vds: f64,
    d_index: usize,
    s_index: usize,
}

impl MosOp {
    fn d_pin(&self, node: [usize; MAX_PINS]) -> usize {
        node[self.d_index]
    }
    fn s_pin(&self, node: [usize; MAX_PINS]) -> usize {
        node[self.s_index]
    }
}

fn mos_eval(pol: f64, vt: f64, k: f64, v: &[f64]) -> MosOp {
    let vg = pol * v[0];
    let (vd, vs) = (pol * v[1], pol * v[2]);
    // The terminal at lower (normalized) potential acts as the source.
    let (d_index, s_index, vdn, vsn) = if vd >= vs {
        (1, 2, vd, vs)
    } else {
        (2, 1, vs, vd)
    };
    let vgs = vg - vsn;
    let vds = vdn - vsn;
    let vgst = vgs - vt;
    let (id, gm, gds) = if vgst <= 0.0 {
        (0.0, 0.0, MOS_LEAK)
    } else if vds < vgst {
        (
            k * (vgst * vds - vds * vds * 0.5),
            k * vds,
            k * (vgst - vds) + MOS_LEAK,
        )
    } else {
        (0.5 * k * vgst * vgst, k * vgst, MOS_LEAK)
    };
    MosOp {
        id: id + MOS_LEAK * vds,
        gm,
        gds,
        vgs,
        vds,
        d_index,
        s_index,
    }
}

/// SPICE-style junction voltage limiting: keeps NR from exponent overflow
/// by pulling large forward-bias steps back onto the exponential.
fn pnjlim(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > vt + vt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                vold + vt * libm::log(arg)
            } else {
                vcrit
            }
        } else {
            vt * libm::log(vnew.max(vt) / vt)
        }
    } else {
        vnew
    }
}
