//! Synthetic worlds built by tiling the golden circuits.
//!
//! Every golden circuit is an electrically independent island: they only
//! "touch" through node 0 (ground), which is not an MNA unknown, so the
//! monolithic matrix of a tiled world is exactly block diagonal with one
//! block per island. That is also what a real game world looks like: many
//! plots, each its own board, coupled (later) only through corridors.

use sim_core::{ElementSpec, Point};

/// One island: its element list plus the size of the MNA system it would
/// need on its own.
pub struct Island {
    pub name: String,
    pub elems: Vec<ElementSpec>,
}

pub struct World {
    pub islands: Vec<Island>,
}

impl World {
    pub fn flat(&self) -> Vec<ElementSpec> {
        self.islands
            .iter()
            .flat_map(|i| i.elems.iter().cloned())
            .collect()
    }
}

fn offset(elems: &[ElementSpec], id_base: u32, d: Point) -> Vec<ElementSpec> {
    elems
        .iter()
        .map(|e| ElementSpec {
            id: e.id + id_base,
            kind: e.kind,
            pins: e.pins.iter().map(|p| (p.0 + d.0, p.1 + d.1)).collect(),
        })
        .collect()
}

/// `copies` tilings of the 15 golden circuits (~135 elements per copy,
/// i.e. one copy ~= today's demo room).
pub fn replicate(copies: usize) -> World {
    let golden = sim_golden::all_golden();
    let mut islands = Vec::new();
    let mut idx = 0u32;
    for c in 0..copies {
        for (name, elems) in golden.iter() {
            // 1000 grid units apart in x, 1000 per copy in y: no accidental
            // coincident endpoints, so no accidental merging.
            let d = ((idx as i32 % 16) * 1000, (c as i32) * 1000);
            islands.push(Island {
                name: format!("{name}#{c}"),
                elems: offset(elems, idx * 1000, d),
            });
            idx += 1;
        }
    }
    World { islands }
}

/// Same tiling, but only the linear golden circuits (no diode/BJT/MOS/
/// op-amp/OTA), so the engine's factor-reuse path stays live.
pub fn replicate_linear(copies: usize) -> World {
    let keep = ["demo_lamp", "rc_step", "rl_step", "rlc_ring", "pot_divider"];
    let golden: Vec<_> = sim_golden::all_golden()
        .into_iter()
        .filter(|(n, _)| keep.contains(n))
        .collect();
    let mut islands = Vec::new();
    let mut idx = 0u32;
    for c in 0..copies {
        for (name, elems) in golden.iter() {
            let d = ((idx as i32 % 16) * 1000, (c as i32) * 1000);
            islands.push(Island {
                name: format!("{name}#{c}"),
                elems: offset(elems, idx * 1000, d),
            });
            idx += 1;
        }
    }
    World { islands }
}

/// ONE connected circuit of arbitrary size: an RC feeder ladder (series R
/// per span, shunt C per node) fed by a DC source, optionally with a
/// rectifier head so the whole island is nonlinear. This is the opposite
/// extreme from `replicate`: a single island whose matrix cannot be split.
/// 2 elements per stage.
pub fn feeder(stages: usize, nonlinear: bool) -> Vec<ElementSpec> {
    use sim_core::ElementKind as K;
    let mut v = Vec::new();
    let gp = (0, 1000);
    v.push(ElementSpec::ground(1, gp));
    v.push(ElementSpec::two(
        2,
        K::VoltageSource {
            dc: 12.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        },
        (0, 0),
        gp,
    ));
    let mut id = 10u32;
    let mut prev = (0, 0);
    if nonlinear {
        // Series diode at the head: one nonlinear device makes the whole
        // island nonlinear, which is exactly the realistic case.
        v.push(ElementSpec::two(id, K::Diode, prev, (1, 0)));
        id += 1;
        prev = (1, 0);
    }
    for s in 0..stages {
        let next = ((s as i32) * 2 + 4, 0);
        v.push(ElementSpec::two(id, K::Resistor { ohms: 0.5 }, prev, next));
        id += 1;
        v.push(ElementSpec::two(
            id,
            K::Capacitor { farads: 1e-7 },
            next,
            gp,
        ));
        id += 1;
        prev = next;
    }
    v.push(ElementSpec::two(id, K::Resistor { ohms: 100.0 }, prev, gp));
    v
}

/// Mirror of `Engine::compile`'s wire closure + unknown layout, so the
/// bench can report the MNA size `n` of a spec list without sim-core
/// exposing it. Returns (num_nodes, num_branches).
pub fn mna_size(elems: &[ElementSpec]) -> (usize, usize) {
    use sim_core::ElementKind as K;
    let mut points: Vec<Point> = Vec::new();
    let mut ends: Vec<Vec<usize>> = Vec::new();
    for e in elems {
        let mut v = Vec::new();
        for p in e.pins.iter() {
            let i = match points.iter().position(|q| q == p) {
                Some(i) => i,
                None => {
                    points.push(*p);
                    points.len() - 1
                }
            };
            v.push(i);
        }
        ends.push(v);
    }
    let ground_root = points.len();
    let mut parent: Vec<usize> = (0..=points.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (e, je) in elems.iter().zip(ends.iter()) {
        match e.kind {
            K::Wire => {
                let (ra, rb) = (find(&mut parent, je[0]), find(&mut parent, je[1]));
                parent[ra] = rb;
            }
            K::Ground => {
                let (ra, rg) = (find(&mut parent, je[0]), find(&mut parent, ground_root));
                parent[ra] = rg;
            }
            _ => {}
        }
    }
    let groot = find(&mut parent, ground_root);
    let mut roots: Vec<usize> = vec![groot];
    let mut num_nodes = 0usize;
    for j in 0..points.len() {
        let r = find(&mut parent, j);
        if !roots.contains(&r) {
            roots.push(r);
            num_nodes += 1;
        }
    }
    let num_branches = elems.iter().filter(|e| e.kind.is_branch()).count();
    (num_nodes, num_branches)
}
