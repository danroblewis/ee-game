//! Netlist extraction: union-find over coincident pins, mirroring
//! `sim_core::engine::compile()` semantics exactly:
//! - a net is a set of grid points made coincident transitively by Wire
//!   elements; Ground merges its point into a virtual ground root;
//! - connectivity is coincidence of pins/ENDPOINTS only.

use sim_core::{ElementKind, ElementSpec, Point};
use std::collections::{BTreeMap, BTreeSet};

pub type NetId = u32;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PinRef {
    pub id: u32,
    pub pin: u8,
}

/// Exact-bit rail identity: N rails with identical constraint parameters
/// merge into one branch unknown (validate.rs diagnose step 2), so a rail
/// net may legally be dissolved into one flag per pin IF all its rails are
/// bit-identical.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct RailKey {
    pub dc: u64,
    pub amp: u64,
    pub hz: u64,
    pub phase: u64,
}

#[derive(Clone, Debug)]
pub struct Net {
    pub id: NetId,
    /// Part pins on this net, sorted by (id, pin). Parts only — wires,
    /// grounds and rails are re-synthesized by the layout.
    pub pins: Vec<PinRef>,
    pub ground: bool,
    /// Distinct rail kinds seen on the net (must be exactly one to flag).
    pub rails: Vec<(RailKey, ElementKind)>,
    /// Source-driven with fanout: a supply bus, routed as a trunk.
    pub power: bool,
}

pub struct Extract {
    /// Non-wire/ground/rail elements, sorted by id.
    pub parts: Vec<ElementSpec>,
    pub nets: Vec<Net>,
    /// (part id, pin index) -> net id.
    pub pin_net: BTreeMap<PinRef, NetId>,
}

struct Uf {
    parent: Vec<usize>,
}

impl Uf {
    fn new(n: usize) -> Self {
        Uf {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

fn is_part(kind: &ElementKind) -> bool {
    !matches!(
        kind,
        ElementKind::Wire | ElementKind::Ground | ElementKind::Rail { .. }
    )
}

pub fn rail_key(kind: &ElementKind) -> Option<RailKey> {
    if let ElementKind::Rail { dc, amp, hz, phase, .. } = kind {
        Some(RailKey {
            dc: dc.to_bits(),
            amp: amp.to_bits(),
            hz: hz.to_bits(),
            phase: phase.to_bits(),
        })
    } else {
        None
    }
}

pub fn extract(elements: &[ElementSpec]) -> Extract {
    // Intern points in document order, like compile().
    let mut idx: BTreeMap<Point, usize> = BTreeMap::new();
    let mut n = 0usize;
    for e in elements {
        for &p in &e.pins {
            idx.entry(p).or_insert_with(|| {
                let i = n;
                n += 1;
                i
            });
        }
    }
    let ground_root = n;
    let mut uf = Uf::new(n + 1);
    for e in elements {
        match e.kind {
            ElementKind::Wire => {
                let a = idx[&e.pins[0]];
                let b = idx[&e.pins[1]];
                uf.union(a, b);
            }
            ElementKind::Ground => {
                let a = idx[&e.pins[0]];
                uf.union(a, ground_root);
            }
            _ => {}
        }
    }

    let mut parts: Vec<ElementSpec> = elements
        .iter()
        .filter(|e| is_part(&e.kind))
        .cloned()
        .collect();
    parts.sort_by_key(|e| e.id);

    // Stable net ids: ground net is always id 0; the rest in first-seen
    // order over sorted part pins.
    let groot = uf.find(ground_root);
    let mut root_net: BTreeMap<usize, NetId> = BTreeMap::new();
    let mut nets: Vec<Net> = Vec::new();
    root_net.insert(groot, 0);
    nets.push(Net {
        id: 0,
        pins: Vec::new(),
        ground: true,
        rails: Vec::new(),
        power: false,
    });
    let mut pin_net: BTreeMap<PinRef, NetId> = BTreeMap::new();
    for e in &parts {
        for (i, &p) in e.pins.iter().enumerate() {
            let r = uf.find(idx[&p]);
            let nid = *root_net.entry(r).or_insert_with(|| {
                let id = nets.len() as NetId;
                nets.push(Net {
                    id,
                    pins: Vec::new(),
                    ground: false,
                    rails: Vec::new(),
                    power: false,
                });
                id
            });
            let pr = PinRef {
                id: e.id,
                pin: i as u8,
            };
            nets[nid as usize].pins.push(pr);
            pin_net.insert(pr, nid);
        }
    }
    // Rails: attach their kind to whatever net their point belongs to.
    for e in elements {
        if let Some(k) = rail_key(&e.kind) {
            let r = uf.find(idx[&e.pins[0]]);
            if let Some(&nid) = root_net.get(&r) {
                let net = &mut nets[nid as usize];
                if !net.rails.iter().any(|(rk, _)| *rk == k) {
                    net.rails.push((k, e.kind));
                }
            }
            // A rail on a net with no part pins is dropped (it drove
            // nothing but wires); reported by the caller via element diff.
        }
    }
    // Power classification: driven by a source pin, fanout > 4.
    let by_id: BTreeMap<u32, &ElementSpec> = parts.iter().map(|e| (e.id, e)).collect();
    for net in &mut nets {
        if net.ground || !net.rails.is_empty() {
            continue;
        }
        let src = net
            .pins
            .iter()
            .filter(|q| crate::footprint::is_source(&by_id[&q.id].kind))
            .count();
        if src > 0 && net.pins.len() > 4 {
            net.power = true;
        }
    }
    Extract {
        parts,
        nets,
        pin_net,
    }
}

/// Signature of the netlist over PART pins, invariant under renumbering:
/// for every part pin, the sorted member list of its net plus the net's
/// groundedness and rail keys. Two documents with equal signatures have the
/// same electrical partition of part pins (up to node renumbering).
pub fn partition_signature(
    elements: &[ElementSpec],
) -> BTreeMap<PinRef, (Vec<PinRef>, bool, Vec<RailKey>)> {
    let ex = extract(elements);
    let mut out = BTreeMap::new();
    for net in &ex.nets {
        let mut members = net.pins.clone();
        members.sort();
        members.dedup();
        let mut rails: Vec<RailKey> = net.rails.iter().map(|(k, _)| *k).collect();
        rails.sort();
        for &q in &members {
            out.insert(q, (members.clone(), net.ground, rails.clone()));
        }
    }
    out
}

/// The set of grid-point groups per net — used by tests for a stricter
/// "same partition" statement error message.
pub fn diff_partitions(a: &[ElementSpec], b: &[ElementSpec]) -> Result<(), String> {
    let sa = partition_signature(a);
    let sb = partition_signature(b);
    if sa.len() != sb.len() {
        let ka: BTreeSet<_> = sa.keys().collect();
        let kb: BTreeSet<_> = sb.keys().collect();
        return Err(format!(
            "pin count differs: {} vs {} (only in A: {:?}, only in B: {:?})",
            sa.len(),
            sb.len(),
            ka.difference(&kb).take(8).collect::<Vec<_>>(),
            kb.difference(&ka).take(8).collect::<Vec<_>>()
        ));
    }
    for (pin, va) in &sa {
        match sb.get(pin) {
            None => return Err(format!("pin {pin:?} missing in B")),
            Some(vb) => {
                if va != vb {
                    return Err(format!(
                        "net differs at {:?}:\n  A: members {:?} ground {} rails {:?}\n  B: members {:?} ground {} rails {:?}",
                        pin, va.0, va.1, va.2, vb.0, vb.1, vb.2
                    ));
                }
            }
        }
    }
    Ok(())
}
