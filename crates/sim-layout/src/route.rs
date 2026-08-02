//! Tiered orthogonal routing on the integer grid.
//!
//! Electrical ground truth (engine.rs compile()): nets merge by COINCIDENT
//! ENDPOINTS only. So the hard legality rules are:
//! - a run endpoint (terminal, corner, tap, flag point) must never coincide
//!   with a foreign pin, a foreign run endpoint, or any point a foreign wire
//!   passes through;
//! - colinear overlap with a foreign edge is forbidden always (illegible and
//!   one split away from a short);
//! - perpendicular CROSSINGS of foreign wires are electrically safe and are
//!   allowed at a cost (staircase tier: free).
//!
//! Tiers per connection: pattern (straight/L against the growing net tree),
//! A* (bounded window, bend/crossing/apron costs), staircase (relaxed,
//! always-almost-legal L/Z; reported as degraded).

use crate::extract::NetId;
use sim_core::Point;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

pub const STEP: i64 = 1;
pub const BEND: i64 = 8;
pub const CROSS: i64 = 16;
pub const RESV: i64 = 20;

/// Pseudo net id for obstacles that belong to no real net (frozen wires on
/// part-pin-free nets): foreign to everything.
pub const NET_NONE: NetId = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Abut,
    Pattern,
    Astar,
    Staircase,
}

#[derive(Default)]
pub struct RouteStats {
    pub conns: u32,
    pub tier_pattern: u32,
    pub tier_astar: u32,
    pub tier_staircase: u32,
    pub tier_abut: u32,
    pub crossings_paid: u32,
    pub failed_nets: Vec<NetId>,
}

pub struct Router {
    pub blocked: BTreeSet<Point>,
    pub pin_owner: BTreeMap<Point, NetId>,
    pub node_use: BTreeMap<Point, NetId>,
    pub edge_use: BTreeMap<(Point, Point), NetId>,
    pub reserved: BTreeMap<Point, NetId>,
    pub runs: Vec<(NetId, Point, Point)>,
    pub stats: RouteStats,
}

fn ekey(a: Point, b: Point) -> (Point, Point) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

impl Router {
    pub fn new() -> Self {
        Router {
            blocked: BTreeSet::new(),
            pin_owner: BTreeMap::new(),
            node_use: BTreeMap::new(),
            edge_use: BTreeMap::new(),
            reserved: BTreeMap::new(),
            runs: Vec::new(),
            stats: RouteStats::default(),
        }
    }

    /// Any foreign presence AT a point: foreign pin, foreign run endpoint,
    /// or a foreign edge incident to the point (wire passing through).
    fn foreign_at(&self, p: Point, net: NetId) -> bool {
        if self.pin_owner.get(&p).is_some_and(|&n| n != net) {
            return true;
        }
        if self.node_use.get(&p).is_some_and(|&n| n != net) {
            return true;
        }
        for d in DIRS {
            let q = (p.0 + d.0, p.1 + d.1);
            if self.edge_use.get(&ekey(p, q)).is_some_and(|&n| n != net) {
                return true;
            }
        }
        false
    }

    fn endpoint_ok(&self, p: Point, net: NetId) -> bool {
        // Own pin / own node / own edge: fine (that is how taps happen).
        // A body cell is never an endpoint — except a pin, which was carved
        // out of `blocked` at build time.
        !self.blocked.contains(&p) && !self.foreign_at(p, net)
    }

    /// Check one straight segment. Returns Some(crossings) if legal.
    /// `relaxed`: staircase rules — body cells and foreign points may be
    /// crossed mid-segment (ugly but electrically safe); colinear overlap
    /// and endpoint coincidences stay forbidden.
    fn seg_ok(&self, a: Point, b: Point, net: NetId, relaxed: bool) -> Option<u32> {
        debug_assert!(a.0 == b.0 || a.1 == b.1);
        if a == b {
            return Some(0);
        }
        let d = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
        let mut crossings = 0u32;
        for s in 0..=len {
            let p = (a.0 + d.0 * s, a.1 + d.1 * s);
            let interior = s != 0 && s != len;
            // colinear overlap with a FOREIGN edge: forbidden even relaxed.
            if s < len {
                let q = (p.0 + d.0, p.1 + d.1);
                if self.edge_use.get(&ekey(p, q)).is_some_and(|&n| n != net) {
                    return None;
                }
            }
            if interior {
                if !relaxed {
                    if self.blocked.contains(&p) {
                        return None;
                    }
                    // any pin mid-segment (own included): must terminate
                    // there instead, never pass over it.
                    if self.pin_owner.contains_key(&p) {
                        return None;
                    }
                    if self.node_use.get(&p).is_some_and(|&n| n != net) {
                        return None;
                    }
                }
                // crossing a foreign wire perpendicular to us
                let (pa, pb) = if d.0 != 0 {
                    ((p.0, p.1 - 1), (p.0, p.1 + 1))
                } else {
                    ((p.0 - 1, p.1), (p.0 + 1, p.1))
                };
                let e1 = self.edge_use.get(&ekey(pa, p));
                let e2 = self.edge_use.get(&ekey(p, pb));
                if let (Some(&n1), Some(&n2)) = (e1, e2) {
                    if n1 == n2 && n1 != net {
                        crossings += 1;
                    }
                }
            }
        }
        if !self.endpoint_ok(a, net) || !self.endpoint_ok(b, net) {
            return None;
        }
        Some(crossings)
    }

    /// Commit a rectilinear path (sequence of grid points, unit steps).
    /// Decomposes into maximal straight runs of NEW edges; marks occupancy.
    pub fn commit_path(&mut self, net: NetId, path: &[Point]) {
        if path.len() < 2 {
            return;
        }
        let mut i = 0usize;
        while i + 1 < path.len() {
            // maximal straight stretch starting at i
            let d = (
                (path[i + 1].0 - path[i].0).signum(),
                (path[i + 1].1 - path[i].1).signum(),
            );
            let mut j = i + 1;
            while j + 1 < path.len() {
                let nd = (
                    (path[j + 1].0 - path[j].0).signum(),
                    (path[j + 1].1 - path[j].1).signum(),
                );
                if nd != d {
                    break;
                }
                j += 1;
            }
            // within [i..j]: emit sub-runs of edges not already owned
            let mut s = i;
            while s < j {
                // skip existing own edges
                while s < j && self.edge_use.get(&ekey(path[s], path[s + 1])) == Some(&net) {
                    s += 1;
                }
                if s >= j {
                    break;
                }
                let start = s;
                while s < j && self.edge_use.get(&ekey(path[s], path[s + 1])).is_none() {
                    self.edge_use.insert(ekey(path[s], path[s + 1]), net);
                    s += 1;
                }
                self.runs.push((net, path[start], path[s]));
                self.node_use.entry(path[start]).or_insert(net);
                self.node_use.entry(path[s]).or_insert(net);
            }
            i = j;
        }
    }

    /// Pattern tier: straight or single-L to one of the tree points.
    fn try_pattern(&self, t: Point, tree: &BTreeSet<Point>, net: NetId) -> Option<Vec<Point>> {
        let mut cands: Vec<Point> = tree.iter().copied().collect();
        cands.sort_by_key(|s| ((s.0 - t.0).abs() + (s.1 - t.1).abs(), s.0, s.1));
        cands.truncate(8);
        for &s in &cands {
            if (s.0 == t.0 || s.1 == t.1) && self.seg_ok(t, s, net, false).is_some() {
                return Some(vec![t, s]);
            }
        }
        for &s in &cands {
            if s.0 == t.0 || s.1 == t.1 {
                continue;
            }
            for c in [(t.0, s.1), (s.0, t.1)] {
                if self.seg_ok(t, c, net, false).is_some()
                    && self.seg_ok(c, s, net, false).is_some()
                {
                    return Some(vec![t, c, s]);
                }
            }
        }
        None
    }

    /// A* over (point, heading) states inside a bounded window.
    fn astar(
        &self,
        start: Point,
        tree: &BTreeSet<Point>,
        net: NetId,
        margin: i32,
    ) -> Option<(Vec<Point>, u32)> {
        let mut min = start;
        let mut max = start;
        for &p in tree.iter() {
            min.0 = min.0.min(p.0);
            min.1 = min.1.min(p.1);
            max.0 = max.0.max(p.0);
            max.1 = max.1.max(p.1);
        }
        let in_b = |p: Point| {
            p.0 >= min.0 - margin && p.0 <= max.0 + margin && p.1 >= min.1 - margin && p.1 <= max.1 + margin
        };
        // manhattan heuristic to nearest tree point is expensive for big
        // trees; plain Dijkstra (h = 0) keeps it simple and deterministic.
        #[derive(PartialEq, Eq)]
        struct Item(i64, u64, Point, u8);
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                // reversed for min-heap
                o.0.cmp(&self.0).then(o.1.cmp(&self.1))
            }
        }
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let mut open: BinaryHeap<Item> = BinaryHeap::new();
        let mut seq = 0u64;
        let mut dist: BTreeMap<(Point, u8), i64> = BTreeMap::new();
        let mut came: BTreeMap<(Point, u8), (Point, u8)> = BTreeMap::new();
        let mut cross_ct: BTreeMap<(Point, u8), u32> = BTreeMap::new();
        for d in 0..4u8 {
            dist.insert((start, d), 0);
            cross_ct.insert((start, d), 0);
            open.push(Item(0, seq, start, d));
            seq += 1;
        }
        let mut popped: BTreeSet<(Point, u8)> = BTreeSet::new();
        let mut goal: Option<(Point, u8)> = None;
        while let Some(Item(g, _, p, d)) = open.pop() {
            if !popped.insert((p, d)) {
                continue;
            }
            if g > dist[&(p, d)] {
                continue;
            }
            if tree.contains(&p) && p != start && self.endpoint_ok(p, net) {
                goal = Some((p, d));
                break;
            }
            // On a point a foreign wire passes through we may only continue
            // straight: turning would make this point a run endpoint.
            let p_foreign_edge = DIRS.iter().any(|dd| {
                let q = (p.0 + dd.0, p.1 + dd.1);
                self.edge_use.get(&ekey(p, q)).is_some_and(|&n| n != net)
            });
            for (nd, dd) in DIRS.iter().enumerate() {
                let nd = nd as u8;
                if p_foreign_edge && nd != d {
                    continue;
                }
                let np = (p.0 + dd.0, p.1 + dd.1);
                if !in_b(np) {
                    continue;
                }
                let is_target = tree.contains(&np);
                if !is_target {
                    if self.blocked.contains(&np) {
                        continue;
                    }
                    if self.pin_owner.get(&np).is_some_and(|&n| n != net) {
                        continue;
                    }
                    if self.node_use.get(&np).is_some_and(|&n| n != net) {
                        continue;
                    }
                    // never pass over ANY pin mid-route (own pins must be
                    // route endpoints, not pass-throughs)
                    if self.pin_owner.contains_key(&np) {
                        continue;
                    }
                }
                let e = ekey(p, np);
                let own_edge = self.edge_use.get(&e) == Some(&net);
                if self.edge_use.get(&e).is_some_and(|&n| n != net) {
                    continue; // colinear overlap
                }
                let mut c = if own_edge {
                    0
                } else {
                    STEP + if nd != d { BEND } else { 0 }
                };
                let mut crossed = 0u32;
                if !own_edge {
                    // crossing a foreign wire running perpendicular through np
                    let (pa, pb) = if dd.0 != 0 {
                        ((np.0, np.1 - 1), (np.0, np.1 + 1))
                    } else {
                        ((np.0 - 1, np.1), (np.0 + 1, np.1))
                    };
                    let e1 = self.edge_use.get(&ekey(pa, np));
                    let e2 = self.edge_use.get(&ekey(np, pb));
                    if let (Some(&n1), Some(&n2)) = (e1, e2) {
                        if n1 == n2 && n1 != net {
                            c += CROSS;
                            crossed = 1;
                        }
                    }
                    if self.reserved.get(&np).is_some_and(|&n| n != net) {
                        c += RESV;
                    }
                }
                let ng = g + c;
                let k = (np, nd);
                if ng < *dist.get(&k).unwrap_or(&i64::MAX) {
                    dist.insert(k, ng);
                    came.insert(k, (p, d));
                    cross_ct.insert(k, cross_ct[&(p, d)] + crossed);
                    open.push(Item(ng, seq, np, nd));
                    seq += 1;
                }
            }
        }
        let (gp, gd) = goal?;
        let crossings = cross_ct[&(gp, gd)];
        let mut path = vec![gp];
        let mut cur = (gp, gd);
        while let Some(&prev) = came.get(&cur) {
            path.push(prev.0);
            cur = prev;
        }
        path.reverse();
        // multi-seeded start: drop leading duplicate points
        path.dedup();
        Some((path, crossings))
    }

    /// Staircase tier: relaxed L/Z that is always electrically safe. May
    /// cross bodies and foreign wires; endpoints stay clean.
    fn staircase(&self, t: Point, tree: &BTreeSet<Point>, net: NetId) -> Option<Vec<Point>> {
        let mut cands: Vec<Point> = tree.iter().copied().collect();
        cands.sort_by_key(|s| ((s.0 - t.0).abs() + (s.1 - t.1).abs(), s.0, s.1));
        cands.truncate(6);
        for &s in &cands {
            if (s.0 == t.0 || s.1 == t.1) && self.seg_ok(t, s, net, true).is_some() {
                return Some(vec![t, s]);
            }
            for c in [(t.0, s.1), (s.0, t.1)] {
                if c == t || c == s {
                    continue;
                }
                if self.seg_ok(t, c, net, true).is_some() && self.seg_ok(c, s, net, true).is_some() {
                    return Some(vec![t, c, s]);
                }
            }
            // Z with a swept corner
            for k in 1..=8i32 {
                for sgn in [1, -1] {
                    let cx = if s.0 != t.0 {
                        (t.0 + s.0) / 2 + sgn * k
                    } else {
                        t.0 + sgn * k
                    };
                    let c1 = (cx, t.1);
                    let c2 = (cx, s.1);
                    if c1 == t || c2 == s || c1 == c2 {
                        continue;
                    }
                    if self.seg_ok(t, c1, net, true).is_some()
                        && self.seg_ok(c1, c2, net, true).is_some()
                        && self.seg_ok(c2, s, net, true).is_some()
                    {
                        return Some(vec![t, c1, c2, s]);
                    }
                }
            }
        }
        None
    }

    /// Public straight-segment legality check (used by the power-bus and
    /// flag planting passes in lib.rs).
    pub fn seg_ok_pub(&self, a: Point, b: Point, net: NetId, relaxed: bool) -> bool {
        (a.0 == b.0 || a.1 == b.1) && self.seg_ok(a, b, net, relaxed).is_some()
    }

    /// Connect one terminal to the growing tree through the tier ladder.
    pub fn route_one(&mut self, net: NetId, t: Point, tree: &mut BTreeSet<Point>) -> bool {
        let committed: Option<(Vec<Point>, Tier, u32)> =
            if let Some(path) = self.try_pattern(t, tree, net) {
                Some((path, Tier::Pattern, 0))
            } else if let Some((path, cr)) = self.astar(t, tree, net, 8) {
                Some((path, Tier::Astar, cr))
            } else if let Some((path, cr)) = self.astar(t, tree, net, 24) {
                Some((path, Tier::Astar, cr))
            } else if let Some(path) = self.staircase(t, tree, net) {
                Some((path, Tier::Staircase, 0))
            } else {
                None
            };
        match committed {
            Some((corners, tier, crossings)) => {
                match tier {
                    Tier::Pattern => self.stats.tier_pattern += 1,
                    Tier::Astar => self.stats.tier_astar += 1,
                    Tier::Staircase => self.stats.tier_staircase += 1,
                    Tier::Abut => {}
                }
                self.stats.crossings_paid += crossings;
                let path = expand(&corners);
                self.commit_path(net, &path);
                for p in &path {
                    tree.insert(*p);
                }
                true
            }
            None => {
                self.stats.failed_nets.push(net);
                false
            }
        }
    }

    /// Connect all `terminals` of `net` into one tree. Returns false if any
    /// terminal could not be connected even by the staircase tier.
    pub fn route_net(&mut self, net: NetId, terminals: &[Point]) -> bool {
        let mut uniq: Vec<Point> = Vec::new();
        for &p in terminals {
            if !uniq.contains(&p) {
                uniq.push(p);
            }
        }
        if uniq.len() < 2 {
            return true;
        }
        let first = uniq[0];
        let mut rest: Vec<Point> = uniq[1..].to_vec();
        rest.sort_by_key(|p| ((p.0 - first.0).abs() + (p.1 - first.1).abs(), p.0, p.1));
        let mut tree: BTreeSet<Point> = BTreeSet::new();
        tree.insert(first);
        let mut ok = true;
        for t in rest {
            if tree.contains(&t) {
                self.stats.tier_abut += 1;
                continue;
            }
            self.stats.conns += 1;
            if !self.route_one(net, t, &mut tree) {
                ok = false;
            }
        }
        ok
    }
}

/// Expand a corner list into unit-step points (A* paths are already unit
/// steps; patterns and staircases are corner lists — expand both safely).
pub fn expand(corners: &[Point]) -> Vec<Point> {
    let mut out = Vec::new();
    for w in corners.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
        for s in 0..len {
            out.push((a.0 + d.0 * s, a.1 + d.1 * s));
        }
    }
    if let Some(&last) = corners.last() {
        out.push(last);
    }
    out.dedup();
    out
}
