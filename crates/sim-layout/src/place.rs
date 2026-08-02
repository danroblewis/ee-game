//! Placement: Sugiyama-lite. BFS layering from sources over the SIGNAL
//! adjacency (ground/rail/power nets are buses, not springs), directed
//! relaxation on semantic out->in edges, barycenter ordering, canonical
//! footprint orientation search (4 rotations x mirror), then integer
//! legalization with congestion-sized channels.
//!
//! Everything is integer arithmetic; every tie is broken by element id or
//! pose index. No floats, no RNG, no map-iteration-order dependence.

use crate::extract::{Extract, Net, NetId, PinRef};
use crate::footprint::{bbox, footprint, is_sink, is_source, pin_roles, posed_pins, Role};
use sim_core::{ElementSpec, Point};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub origin: Point,
    pub rot: u8,
    pub mirror: bool,
}

pub struct Placement {
    pub geom: BTreeMap<u32, Geom>,
    pub col: BTreeMap<u32, i32>,
}

fn signal(net: &Net) -> bool {
    !net.ground && net.rails.is_empty() && !net.power
}

/// Integer square root (floor).
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub fn place(ex: &Extract, movable: &BTreeSet<u32>) -> Placement {
    let parts: Vec<&ElementSpec> = ex
        .parts
        .iter()
        .filter(|e| movable.contains(&e.id))
        .collect();
    let by_id: BTreeMap<u32, &ElementSpec> = parts.iter().map(|e| (e.id, *e)).collect();
    let ids: Vec<u32> = parts.iter().map(|e| e.id).collect();

    // adjacency over signal nets, for clustering (functional blocks are the
    // components once ground/rail/power buses are removed)
    let mut adj: BTreeMap<u32, BTreeSet<u32>> = ids.iter().map(|&i| (i, BTreeSet::new())).collect();
    for net in &ex.nets {
        if !signal(net) {
            continue;
        }
        let here: Vec<&PinRef> = net.pins.iter().filter(|q| by_id.contains_key(&q.id)).collect();
        for a in &here {
            for b in &here {
                if a.id != b.id {
                    adj.get_mut(&a.id).unwrap().insert(b.id);
                }
            }
        }
    }
    // components, ordered by smallest member id
    let mut comp_of: BTreeMap<u32, u32> = BTreeMap::new();
    let mut comps: Vec<Vec<u32>> = Vec::new();
    for &id in &ids {
        if comp_of.contains_key(&id) {
            continue;
        }
        let mut q = VecDeque::new();
        let mut members = Vec::new();
        q.push_back(id);
        comp_of.insert(id, id);
        while let Some(u) = q.pop_front() {
            members.push(u);
            for &v in &adj[&u] {
                if !comp_of.contains_key(&v) {
                    comp_of.insert(v, id);
                    q.push_back(v);
                }
            }
        }
        members.sort();
        comps.push(members);
    }

    // place each component as its own block, then tile blocks serpentine
    let mut geom: BTreeMap<u32, Geom> = BTreeMap::new();
    let mut col: BTreeMap<u32, i32> = BTreeMap::new();
    let mut blocks: Vec<(Vec<u32>, (Point, Point))> = Vec::new(); // ids + bbox
    for members in &comps {
        let block = place_block(ex, &by_id, members);
        let mut bmin = (i32::MAX, i32::MAX);
        let mut bmax = (i32::MIN, i32::MIN);
        for (&id, g) in &block.geom {
            let pins = posed_pins(&by_id[&id].kind, g.origin, g.rot, g.mirror);
            let (mn, mx) = bbox(&pins);
            bmin.0 = bmin.0.min(mn.0);
            bmin.1 = bmin.1.min(mn.1);
            bmax.0 = bmax.0.max(mx.0);
            bmax.1 = bmax.1.max(mx.1);
        }
        for (id, g) in block.geom {
            geom.insert(id, g);
        }
        for (id, c) in block.col {
            col.insert(id, c);
        }
        blocks.push((members.clone(), (bmin, bmax)));
    }
    // serpentine tiling into a near-square sheet; tiny blocks that exist
    // only to feed a power bus (a lone source) go first, next to the bus
    // lanes at the top-left
    let feeds_bus = |members: &Vec<u32>| -> bool {
        members.len() <= 2
            && members.iter().any(|id| {
                (0..by_id[id].pins.len()).any(|i| {
                    let nid = ex.pin_net[&PinRef {
                        id: *id,
                        pin: i as u8,
                    }];
                    ex.nets[nid as usize].power
                })
            })
    };
    blocks.sort_by_key(|(members, _)| (!feeds_bus(members), members[0]));
    const HGAP: i32 = 10;
    const ROWGAP: i32 = 8;
    let total_area: i64 = blocks
        .iter()
        .map(|(_, (mn, mx))| ((mx.0 - mn.0 + HGAP) as i64) * ((mx.1 - mn.1 + ROWGAP) as i64))
        .sum();
    let row_limit = (isqrt(total_area) * 3 / 2).max(48) as i32;
    let mut x = 0i32;
    let mut y = 0i32;
    let mut row_h = 0i32;
    for (members, (mn, mx)) in &blocks {
        let w = mx.0 - mn.0;
        let h = mx.1 - mn.1;
        if x > 0 && x + w > row_limit {
            y += row_h + ROWGAP;
            x = 0;
            row_h = 0;
        }
        let dx = x - mn.0;
        let dy = y - mn.1;
        for id in members {
            let g = geom.get_mut(id).unwrap();
            g.origin.0 += dx;
            g.origin.1 += dy;
        }
        x += w + HGAP;
        row_h = row_h.max(h);
    }

    Placement { geom, col }
}

struct Block {
    geom: BTreeMap<u32, Geom>,
    col: BTreeMap<u32, i32>,
}

fn place_block(ex: &Extract, all_by_id: &BTreeMap<u32, &ElementSpec>, members: &[u32]) -> Block {
    let by_id: BTreeMap<u32, &ElementSpec> = members.iter().map(|&i| (i, all_by_id[&i])).collect();
    let ids: Vec<u32> = members.to_vec();

    // ---- adjacency + directed edges over signal nets --------------------
    let mut adj: BTreeMap<u32, BTreeSet<u32>> = ids.iter().map(|&i| (i, BTreeSet::new())).collect();
    let mut dir: Vec<(u32, u32)> = Vec::new();
    for net in &ex.nets {
        if !signal(net) {
            continue;
        }
        let here: Vec<&PinRef> = net.pins.iter().filter(|q| by_id.contains_key(&q.id)).collect();
        let mut outs = Vec::new();
        let mut ins = Vec::new();
        for q in &here {
            let roles = pin_roles(&by_id[&q.id].kind);
            match roles[q.pin as usize] {
                Role::Out => outs.push(q.id),
                Role::In => ins.push(q.id),
                _ => {}
            }
        }
        for a in &here {
            for b in &here {
                if a.id != b.id {
                    adj.get_mut(&a.id).unwrap().insert(b.id);
                }
            }
        }
        for &o in &outs {
            for &i in &ins {
                if o != i {
                    dir.push((o, i));
                }
            }
        }
    }

    // ---- layering -------------------------------------------------------
    let mut layer: BTreeMap<u32, i32> = ids.iter().map(|&i| (i, 0)).collect();
    {
        let mut seen: BTreeSet<u32> = BTreeSet::new();
        let mut q: VecDeque<u32> = VecDeque::new();
        for &id in &ids {
            if is_source(&by_id[&id].kind) {
                seen.insert(id);
                q.push_back(id);
            }
        }
        if q.is_empty() {
            // no source in this block (it hangs off a power bus): seed the
            // lowest id so the chain still unrolls into columns
            if let Some(&id) = ids.first() {
                seen.insert(id);
                q.push_back(id);
            }
        }
        while let Some(u) = q.pop_front() {
            for &v in &adj[&u] {
                if seen.insert(v) {
                    layer.insert(v, layer[&u] + 1);
                    q.push_back(v);
                }
            }
        }
    }
    let cap = ids.len() as i32;
    for _ in 0..3 * ids.len().max(1) {
        let mut moved = false;
        for &(u, v) in &dir {
            let lv = layer[&v].max(layer[&u] + 1);
            if lv != layer[&v] && lv < cap {
                layer.insert(v, lv);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for &id in &ids {
        if is_sink(&by_id[&id].kind) {
            let m = adj[&id].iter().map(|q| layer[q]).max();
            if let Some(m) = m {
                layer.insert(id, m + 1);
            }
        }
    }
    // compact layer indices
    {
        let used: BTreeSet<i32> = layer.values().copied().collect();
        let remap: BTreeMap<i32, i32> = used.iter().enumerate().map(|(i, &l)| (l, i as i32)).collect();
        for &id in &ids {
            let l = remap[&layer[&id]];
            layer.insert(id, l);
        }
    }
    // cap column height: spill into inserted columns, id order
    const MAXCOL: usize = 7;
    {
        let mut by_layer: BTreeMap<i32, Vec<u32>> = BTreeMap::new();
        for &id in &ids {
            by_layer.entry(layer[&id]).or_default().push(id);
        }
        let mut shift = 0i32;
        for (l, mut lids) in by_layer {
            lids.sort();
            let cols = lids.len().div_ceil(MAXCOL) as i32;
            for (i, id) in lids.iter().enumerate() {
                layer.insert(*id, l + shift + (i / MAXCOL) as i32);
            }
            shift += cols - 1;
        }
    }
    let n_layers = layer.values().copied().max().unwrap_or(0) + 1;

    // ---- ordering: barycenter sweeps ------------------------------------
    let mut cols: Vec<Vec<u32>> = vec![Vec::new(); n_layers as usize];
    for &id in &ids {
        cols[layer[&id] as usize].push(id);
    }
    for c in &mut cols {
        c.sort();
    }
    let mut pos: BTreeMap<u32, i64> = BTreeMap::new();
    let reindex = |cols: &Vec<Vec<u32>>, pos: &mut BTreeMap<u32, i64>| {
        for c in cols {
            for (i, &id) in c.iter().enumerate() {
                pos.insert(id, i as i64);
            }
        }
    };
    reindex(&cols, &mut pos);
    for _ in 0..6 {
        for ci in 0..cols.len() {
            let mut c = cols[ci].clone();
            // barycenter as a rational (sum, count); own position if isolated
            let bary = |id: u32, pos: &BTreeMap<u32, i64>| -> (i64, i64) {
                let ns = &adj[&id];
                if ns.is_empty() {
                    (pos[&id], 1)
                } else {
                    (ns.iter().map(|q| pos[q]).sum(), ns.len() as i64)
                }
            };
            c.sort_by(|&a, &b| {
                let (sa, ca) = bary(a, &pos);
                let (sb, cb) = bary(b, &pos);
                (sa * cb).cmp(&(sb * ca)).then(a.cmp(&b))
            });
            cols[ci] = c;
            reindex(&cols, &mut pos);
        }
    }

    // ---- provisional coordinates ----------------------------------------
    const VGAP: i32 = 4;
    let mut geom: BTreeMap<u32, Geom> = BTreeMap::new();
    for (l, c) in cols.iter().enumerate() {
        let mut y = 0i32;
        for &id in c {
            let fp = footprint(&by_id[&id].kind);
            let (min, max) = bbox(&fp);
            geom.insert(
                id,
                Geom {
                    origin: ((l as i32) * 14, y - min.1 + 2),
                    rot: 0,
                    mirror: false,
                },
            );
            y += (max.1 - min.1) + VGAP + 2;
        }
    }

    // ---- orientation search ---------------------------------------------
    // Minimize sum over pins of Manhattan distance to the centroid of the
    // pin's net's OTHER pins (signal + power nets only). Ties prefer low
    // rot, then no mirror.
    let pin_world = |id: u32, g: &Geom, by_id: &BTreeMap<u32, &ElementSpec>| -> Vec<Point> {
        posed_pins(&by_id[&id].kind, g.origin, g.rot, g.mirror)
    };
    for _pass in 0..3 {
        for &id in &ids {
            let e = by_id[&id];
            if e.pins.len() < 2 {
                continue;
            }
            // targets: centroid of other pins of each pin's net
            let mut targets: Vec<Option<Point>> = Vec::new();
            for i in 0..e.pins.len() {
                let nid: NetId = ex.pin_net[&PinRef { id, pin: i as u8 }];
                let net = &ex.nets[nid as usize];
                if net.ground || !net.rails.is_empty() {
                    targets.push(None);
                    continue;
                }
                let mut sx = 0i64;
                let mut sy = 0i64;
                let mut cnt = 0i64;
                for q in &net.pins {
                    if q.id == id {
                        continue;
                    }
                    let p = match geom.get(&q.id) {
                        Some(g) => pin_world(q.id, g, &by_id)[q.pin as usize],
                        None => continue, // frozen part pins handled by caller offset; skip
                    };
                    sx += p.0 as i64;
                    sy += p.1 as i64;
                    cnt += 1;
                }
                if cnt == 0 {
                    targets.push(None);
                } else {
                    targets.push(Some((
                        (sx.div_euclid(cnt)) as i32,
                        (sy.div_euclid(cnt)) as i32,
                    )));
                }
            }
            let g0 = geom[&id];
            let two_pin = e.pins.len() == 2;
            let mut best: Option<(i64, u8, bool)> = None;
            for mirror in [false, true] {
                if two_pin && mirror {
                    continue;
                }
                for rot in 0u8..4 {
                    let g = Geom {
                        origin: g0.origin,
                        rot,
                        mirror,
                    };
                    let pins = pin_world(id, &g, &by_id);
                    let mut cost: i64 = 0;
                    for (i, t) in targets.iter().enumerate() {
                        if let Some(t) = t {
                            cost += (pins[i].0 - t.0).abs() as i64 + (pins[i].1 - t.1).abs() as i64;
                        }
                    }
                    let score = cost * 16 + rot as i64 + if mirror { 8 } else { 0 };
                    if best.map_or(true, |(bs, _, _)| score < bs) {
                        best = Some((score, rot, mirror));
                    }
                }
            }
            if let Some((_, rot, mirror)) = best {
                let g = geom.get_mut(&id).unwrap();
                g.rot = rot;
                g.mirror = mirror;
            }
        }
    }

    // ---- legalization: channel-sized columns, restacked rows ------------
    // Channel demand at boundary l -> l+1: nets whose placed pins span it.
    let mut net_cols: BTreeMap<NetId, (i32, i32)> = BTreeMap::new();
    for net in &ex.nets {
        if net.ground || !net.rails.is_empty() {
            continue;
        }
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for q in &net.pins {
            if let Some(&l) = layer.get(&q.id) {
                lo = lo.min(l);
                hi = hi.max(l);
            }
        }
        if lo <= hi {
            net_cols.insert(net.id, (lo, hi));
        }
    }
    let demand = |l: i32| -> i32 {
        net_cols
            .values()
            .filter(|&&(lo, hi)| lo <= l && hi >= l + 1)
            .count() as i32
    };
    // oriented bbox per part
    let obox = |id: u32, geom: &BTreeMap<u32, Geom>| -> (Point, Point) {
        let g = &geom[&id];
        bbox(&pin_world(id, g, &by_id))
    };
    let mut col_x = vec![0i32; n_layers as usize + 1];
    for l in 0..n_layers as usize {
        let w = cols[l]
            .iter()
            .map(|&id| {
                let (min, max) = obox(id, &geom);
                max.0 - min.0
            })
            .max()
            .unwrap_or(4);
        let gap = (5 + demand(l as i32) / 3).clamp(6, 14);
        col_x[l + 1] = col_x[l] + w + gap;
    }
    for (l, c) in cols.iter().enumerate() {
        // preserve barycenter order (current stacking order)
        let mut order = c.clone();
        order.sort_by_key(|&id| (geom[&id].origin.1, id));
        let mut y = 0i32;
        for &id in &order {
            let (min, max) = obox(id, &geom);
            let g = geom.get_mut(&id).unwrap();
            g.origin = (
                g.origin.0 + (col_x[l] - min.0),
                g.origin.1 + (y - min.1) + 2,
            );
            y += (max.1 - min.1) + VGAP + 2;
        }
    }

    Block { geom, col: layer }
}
