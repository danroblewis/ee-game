//! sim-layout: deterministic schematic re-layout from netlist + part
//! geometry (docs/plan: the "Tidy" / generator-final-pass crate).
//!
//! Contract (all verified before anything is returned):
//! - the netlist partition of part pins is EXACTLY the input's (up to node
//!   renumbering) — self-checked by re-extraction, because check_document
//!   validates solvability, not intent;
//! - the output passes `sim_core::check_document`;
//! - the computation is a pure integer function of the input document: no
//!   floats, no RNG, no clocks, no hash-iteration order. Same input, same
//!   bytes, on native and wasm alike.
//!
//! What it may change: pins of non-frozen elements (canonical footprint x
//! rotation x mirror only), and every Wire/Ground/Rail element wholesale
//! (deleted and re-synthesized: grounds and rails become per-pin flags —
//! electrically free, validate.rs merges identical rail constraints).
//! What it never changes: kinds, tiers, params, ids of parts, pin order,
//! frozen elements (reserved ids 900-999).

pub mod extract;
pub mod footprint;
pub mod place;
pub mod route;

use extract::{extract, NetId, PinRef};
use footprint::{bbox, posed_pins};
use route::{Router, NET_NONE};
use sim_core::{ElementKind, ElementSpec, Point};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quality {
    Clean,
    Flagged,
    Failed,
}

#[derive(Debug)]
pub struct LayoutReport {
    pub quality: Quality,
    pub elements_before: usize,
    pub elements_after: usize,
    pub wires_before: usize,
    pub wires_after: usize,
    pub flags_after: usize,
    pub conns: u32,
    pub tier_abut: u32,
    pub tier_pattern: u32,
    pub tier_astar: u32,
    pub tier_staircase: u32,
    pub crossings_paid: u32,
    pub failed_nets: Vec<NetId>,
    pub notes: Vec<String>,
}

pub struct LayoutResult {
    pub elements: Vec<ElementSpec>,
    pub report: LayoutReport,
}

pub fn reserved_id(id: u32) -> bool {
    (900..1000).contains(&id)
}

fn escape_dir(pin: Point, min: Point, max: Point) -> (i32, i32) {
    if pin.0 == min.0 && min.0 != max.0 {
        (-1, 0)
    } else if pin.0 == max.0 && min.0 != max.0 {
        (1, 0)
    } else if pin.1 == min.1 {
        (0, -1)
    } else {
        (0, 1)
    }
}

pub fn relayout(input: &[ElementSpec]) -> Result<LayoutResult, String> {
    let ex = extract(input);
    let mut notes: Vec<String> = Vec::new();

    // ---- sanity of flag dissolution ------------------------------------
    for net in &ex.nets {
        if net.rails.len() > 1 {
            return Err(format!(
                "net {} carries {} distinct rail constraints — input is degenerate, refusing",
                net.id,
                net.rails.len()
            ));
        }
        if net.ground && !net.rails.is_empty() {
            return Err(format!("net {} is both grounded and railed — refusing", net.id));
        }
    }

    // ---- who moves ------------------------------------------------------
    let movable: BTreeSet<u32> = ex
        .parts
        .iter()
        .filter(|e| !reserved_id(e.id))
        .map(|e| e.id)
        .collect();
    let frozen: Vec<&ElementSpec> = input.iter().filter(|e| reserved_id(e.id)).collect();
    let dropped_wires = input
        .iter()
        .filter(|e| !reserved_id(e.id) && matches!(e.kind, ElementKind::Wire))
        .count();
    let dropped_gr = input
        .iter()
        .filter(|e| {
            !reserved_id(e.id)
                && matches!(e.kind, ElementKind::Ground | ElementKind::Rail { .. })
        })
        .count();
    let _ = (dropped_wires, dropped_gr);

    // ---- placement -------------------------------------------------------
    let placement = place::place(&ex, &movable);
    let mut geom = placement.geom;

    // Shift the placed sheet clear of frozen fixtures.
    if !frozen.is_empty() && !geom.is_empty() {
        let mut fmin = (i32::MAX, i32::MAX);
        let mut fmax = (i32::MIN, i32::MIN);
        for e in &frozen {
            for &p in &e.pins {
                fmin.0 = fmin.0.min(p.0);
                fmin.1 = fmin.1.min(p.1);
                fmax.0 = fmax.0.max(p.0);
                fmax.1 = fmax.1.max(p.1);
            }
        }
        let by_id: BTreeMap<u32, &ElementSpec> = ex.parts.iter().map(|e| (e.id, e)).collect();
        let mut smin = (i32::MAX, i32::MAX);
        for (&id, g) in &geom {
            let pins = posed_pins(&by_id[&id].kind, g.origin, g.rot, g.mirror);
            let (mn, _) = bbox(&pins);
            smin.0 = smin.0.min(mn.0);
            smin.1 = smin.1.min(mn.1);
        }
        let dx = (fmax.0 + 8) - smin.0;
        let dy = fmin.1 - smin.1;
        for g in geom.values_mut() {
            g.origin.0 += dx;
            g.origin.1 += dy;
        }
    }

    // ---- world pin positions --------------------------------------------
    let by_id: BTreeMap<u32, &ElementSpec> = ex.parts.iter().map(|e| (e.id, e)).collect();
    let world_pins = |id: u32| -> Vec<Point> {
        match geom.get(&id) {
            Some(g) => posed_pins(&by_id[&id].kind, g.origin, g.rot, g.mirror),
            None => by_id[&id].pins.clone(), // frozen
        }
    };
    let mut pin_pos: BTreeMap<PinRef, Point> = BTreeMap::new();
    for e in &ex.parts {
        for (i, _) in e.pins.iter().enumerate() {
            let w = world_pins(e.id);
            pin_pos.insert(
                PinRef {
                    id: e.id,
                    pin: i as u8,
                },
                w[i],
            );
        }
    }

    // ---- occupancy -------------------------------------------------------
    let mut r = Router::new();
    // bodies first, then carve pins (bodies of neighbours must not swallow
    // another part's pin)
    for e in &ex.parts {
        let pins = world_pins(e.id);
        let (mn, mx) = bbox(&pins);
        for x in mn.0..=mx.0 {
            for y in mn.1..=mx.1 {
                r.blocked.insert((x, y));
            }
        }
    }
    for e in &ex.parts {
        let pins = world_pins(e.id);
        for (i, &p) in pins.iter().enumerate() {
            r.blocked.remove(&p);
            let net = ex.pin_net[&PinRef {
                id: e.id,
                pin: i as u8,
            }];
            r.pin_owner.insert(p, net);
        }
    }
    // Frozen non-part elements are immovable obstacles with their real nets.
    // (point_net: net of any input point that hosts a part pin; others are
    // NET_NONE — foreign to every route.)
    let mut point_net: BTreeMap<Point, NetId> = BTreeMap::new();
    for e in &ex.parts {
        for (i, &p) in e.pins.iter().enumerate() {
            point_net.insert(
                p,
                ex.pin_net[&PinRef {
                    id: e.id,
                    pin: i as u8,
                }],
            );
        }
    }
    for e in &frozen {
        match e.kind {
            ElementKind::Wire => {
                let (a, b) = (e.pins[0], e.pins[1]);
                let net = *point_net.get(&a).or(point_net.get(&b)).unwrap_or(&NET_NONE);
                if a.0 == b.0 || a.1 == b.1 {
                    let d = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
                    let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
                    for s in 0..len {
                        let p = (a.0 + d.0 * s, a.1 + d.1 * s);
                        let q = (a.0 + d.0 * (s + 1), a.1 + d.1 * (s + 1));
                        let k = if p <= q { (p, q) } else { (q, p) };
                        r.edge_use.insert(k, net);
                    }
                } else {
                    notes.push(format!("frozen wire {} is diagonal; kept as endpoint obstacles only", e.id));
                }
                r.node_use.insert(a, net);
                r.node_use.insert(b, net);
            }
            ElementKind::Ground => {
                r.pin_owner.insert(e.pins[0], 0);
            }
            ElementKind::Rail { .. } => {
                let net = *point_net.get(&e.pins[0]).unwrap_or(&NET_NONE);
                r.pin_owner.insert(e.pins[0], net);
            }
            _ => {} // frozen parts already handled via ex.parts
        }
    }

    // pin escape aprons: soft-reserve the 2 cells in front of every pin
    let mut escape: BTreeMap<Point, (i32, i32)> = BTreeMap::new();
    for e in &ex.parts {
        let pins = world_pins(e.id);
        let (mn, mx) = bbox(&pins);
        for (i, &p) in pins.iter().enumerate() {
            let net = ex.pin_net[&PinRef {
                id: e.id,
                pin: i as u8,
            }];
            let d = escape_dir(p, mn, mx);
            escape.entry(p).or_insert(d);
            for s in 1..=2 {
                let c = (p.0 + d.0 * s, p.1 + d.1 * s);
                if r.blocked.contains(&c) || r.pin_owner.contains_key(&c) {
                    break;
                }
                r.reserved.entry(c).or_insert(net);
            }
        }
    }

    // ---- routing ---------------------------------------------------------
    // sheet top (for power bus lanes): above every body and pin
    let mut top = i32::MAX;
    let mut all_ok = true;
    for p in r.blocked.iter().chain(r.pin_owner.keys()) {
        top = top.min(p.1);
    }
    if top == i32::MAX {
        top = 0;
    }

    // power nets first: they get horizontal bus lanes above the sheet
    let mut power: Vec<&extract::Net> = ex.nets.iter().filter(|n| n.power).collect();
    power.sort_by_key(|n| (usize::MAX - n.pins.len(), n.id));
    let mut lane = 0i32;
    for net in &power {
        let bus_y = top - 3 - 2 * lane;
        lane += 1;
        let terms: Vec<Point> = net.pins.iter().map(|q| pin_pos[q]).collect();
        if !route_power(&mut r, net.id, &terms, &escape, bus_y) {
            all_ok = false;
        }
    }

    // signal nets, ascending span
    let mut signal: Vec<&extract::Net> = ex
        .nets
        .iter()
        .filter(|n| !n.ground && n.rails.is_empty() && !n.power && n.pins.len() > 1)
        .collect();
    let span = |n: &extract::Net| -> i32 {
        let pts: Vec<Point> = n.pins.iter().map(|q| pin_pos[q]).collect();
        let (mn, mx) = bbox(&pts);
        (mx.0 - mn.0) + (mx.1 - mn.1)
    };
    signal.sort_by_key(|n| (span(n), n.id));
    for net in &signal {
        let terms: Vec<Point> = net.pins.iter().map(|q| pin_pos[q]).collect();
        if !r.route_net(net.id, &terms) {
            all_ok = false;
        }
    }

    // ---- flags (after signal routing so they cannot seal pin escapes) ---
    // grounds: one flag per unique ground-net pin point; rails likewise.
    let mut flags: Vec<(NetId, ElementKind, Point, u8)> = Vec::new();
    for net in &ex.nets {
        let is_gnd = net.ground;
        let rail_kind = net.rails.first().map(|(_, k)| *k);
        if !is_gnd && rail_kind.is_none() {
            continue;
        }
        let mut seen: BTreeSet<Point> = BTreeSet::new();
        for q in &net.pins {
            let p = pin_pos[q];
            if !seen.insert(p) {
                continue;
            }
            let (at, dir) = plant_flag(&mut r, net.id, p, is_gnd, &escape);
            let rot = match dir {
                (0, 1) => 0u8,
                (-1, 0) => 1,
                (0, -1) => 2,
                _ => 3,
            };
            let kind = if is_gnd {
                ElementKind::Ground
            } else {
                rail_kind.unwrap()
            };
            flags.push((net.id, kind, at, rot));
        }
        // A frozen Ground/Rail already supplies the constraint for its net;
        // we still flag every pin — redundant grounds are free.
    }

    // ---- emit ------------------------------------------------------------
    let mut out: Vec<ElementSpec> = Vec::new();
    for e in &ex.parts {
        let mut spec = (*by_id[&e.id]).clone();
        if movable.contains(&e.id) {
            spec.pins = world_pins(e.id);
            spec.rot = 0;
        }
        out.push(spec);
    }
    // frozen non-part elements, byte-identical
    for e in &frozen {
        if !matches!(
            e.kind,
            ElementKind::Wire | ElementKind::Ground | ElementKind::Rail { .. }
        ) {
            continue; // already emitted from ex.parts
        }
        out.push((*e).clone());
    }
    out.sort_by_key(|e| e.id);

    // split runs at every same-net endpoint (pins, corners, taps, flags):
    // every electrical meeting point must be a wire ENDPOINT.
    let mut net_endpoints: BTreeMap<NetId, BTreeSet<Point>> = BTreeMap::new();
    for &(net, a, b) in &r.runs {
        net_endpoints.entry(net).or_default().insert(a);
        net_endpoints.entry(net).or_default().insert(b);
    }
    for (pr, &net) in &ex.pin_net {
        net_endpoints.entry(net).or_default().insert(pin_pos[pr]);
    }
    for (net, _, at, _) in &flags {
        net_endpoints.entry(*net).or_default().insert(*at);
    }
    let mut wires: Vec<(Point, Point)> = Vec::new();
    for &(net, a, b) in &r.runs {
        let eps = &net_endpoints[&net];
        let d = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
        let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
        let mut start = a;
        for s in 1..=len {
            let p = (a.0 + d.0 * s, a.1 + d.1 * s);
            if s == len || eps.contains(&p) {
                if start != p {
                    wires.push((start, p));
                }
                start = p;
            }
        }
    }
    wires.sort();
    wires.dedup();

    let mut next_id = input.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let mut alloc = || {
        if reserved_id(next_id) {
            next_id = 1000;
        }
        let id = next_id;
        next_id += 1;
        id
    };
    let wires_after = wires.len();
    for (a, b) in wires {
        out.push(ElementSpec {
            id: alloc(),
            kind: ElementKind::Wire,
            pins: vec![a, b],
            tier: 0,
            rot: 0,
        });
    }
    let flags_after = flags.len();
    for (_, kind, at, rot) in flags {
        out.push(ElementSpec {
            id: alloc(),
            kind,
            pins: vec![at],
            tier: 0,
            rot,
        });
    }

    // ---- verify ----------------------------------------------------------
    extract::diff_partitions(input, &out)
        .map_err(|e| format!("NETLIST CHANGED (layout bug): {e}"))?;
    sim_core::check_document(&out, 20e-6)
        .map_err(|e| format!("check_document refused the layout: {e:?}"))?;

    let quality = if !all_ok {
        Quality::Failed
    } else if r.stats.tier_staircase > 0 {
        Quality::Flagged
    } else {
        Quality::Clean
    };
    if !all_ok {
        return Err(format!(
            "unroutable nets: {:?} — keeping the original document",
            r.stats.failed_nets
        ));
    }

    let report = LayoutReport {
        quality,
        elements_before: input.len(),
        elements_after: out.len(),
        wires_before: input
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Wire))
            .count(),
        wires_after,
        flags_after,
        conns: r.stats.conns,
        tier_abut: r.stats.tier_abut,
        tier_pattern: r.stats.tier_pattern,
        tier_astar: r.stats.tier_astar,
        tier_staircase: r.stats.tier_staircase,
        crossings_paid: r.stats.crossings_paid,
        failed_nets: r.stats.failed_nets.clone(),
        notes,
    };
    Ok(LayoutResult {
        elements: out,
        report,
    })
}

/// Power-net routing: a horizontal bus lane above the sheet, per-terminal
/// escape (stub into the channel, riser to the lane), taps joined along the
/// lane. Falls back to the generic tiers per terminal.
fn route_power(
    r: &mut Router,
    net: NetId,
    terminals: &[Point],
    escape: &BTreeMap<Point, (i32, i32)>,
    bus_y: i32,
) -> bool {
    let mut uniq: Vec<Point> = Vec::new();
    for &p in terminals {
        if !uniq.contains(&p) {
            uniq.push(p);
        }
    }
    if uniq.len() < 2 {
        return true;
    }
    uniq.sort();
    let mut tree: BTreeSet<Point> = BTreeSet::new();
    let mut taps: Vec<Point> = Vec::new();
    let mut ok = true;
    for &t in &uniq {
        if tree.contains(&t) {
            r.stats.tier_abut += 1;
            continue;
        }
        r.stats.conns += 1;
        // escape to the bus lane
        let esc = escape.get(&t).copied().unwrap_or((1, 0));
        let mut corners: Option<Vec<Point>> = None;
        let dirs: Vec<(i32, i32)> = if esc.0 != 0 {
            vec![esc, (-esc.0, 0)]
        } else if esc == (0, -1) {
            vec![(0, -1), (1, 0), (-1, 0)]
        } else {
            vec![(1, 0), (-1, 0)]
        };
        'outer: for d in dirs {
            if d == (0, -1) {
                // direct riser
                let end = (t.0, bus_y);
                if r.seg_ok_pub(t, end, net, false) {
                    corners = Some(vec![t, end]);
                    break 'outer;
                }
                continue;
            }
            for k in 1..=14i32 {
                let c = (t.0 + d.0 * k, t.1);
                let e = (c.0, bus_y);
                if r.seg_ok_pub(t, c, net, false) && r.seg_ok_pub(c, e, net, false) {
                    corners = Some(vec![t, c, e]);
                    break 'outer;
                }
            }
        }
        match corners {
            Some(cs) => {
                let tap = *cs.last().unwrap();
                let path = route::expand(&cs);
                r.commit_path(net, &path);
                for p in &path {
                    tree.insert(*p);
                }
                // join along the lane to the nearest existing tap
                if let Some(&near) = taps
                    .iter()
                    .min_by_key(|q| ((q.0 - tap.0).abs(), q.0, q.1))
                {
                    if near != tap {
                        let path = route::expand(&[near, tap]);
                        r.commit_path(net, &path);
                        for p in &path {
                            tree.insert(*p);
                        }
                    }
                }
                taps.push(tap);
                r.stats.tier_pattern += 1;
            }
            None => {
                // generic fallback to whatever tree exists
                if tree.is_empty() {
                    tree.insert(t);
                    continue;
                }
                if !r.route_one(net, t, &mut tree) {
                    ok = false;
                }
            }
        }
    }
    ok
}

/// Plant a Ground/Rail flag for one pin: short stub away from the body
/// (down for ground, up for rail), else directly on the pin.
fn plant_flag(
    r: &mut Router,
    net: NetId,
    pin: Point,
    ground: bool,
    escape: &BTreeMap<Point, (i32, i32)>,
) -> (Point, (i32, i32)) {
    let esc = escape.get(&pin).copied().unwrap_or((0, 1));
    let mut prefer: Vec<(i32, i32)> = Vec::new();
    if esc.0 != 0 {
        prefer.push(esc);
    }
    if ground {
        prefer.extend([(0, 1), (0, -1), (1, 0), (-1, 0)]);
    } else {
        prefer.extend([(0, -1), (0, 1), (1, 0), (-1, 0)]);
    }
    for d in prefer {
        for len in 2..=4i32 {
            let end = (pin.0 + d.0 * len, pin.1 + d.1 * len);
            if !r.seg_ok_pub(pin, end, net, false) {
                continue;
            }
            // avoid parking a flag inside a foreign pin apron
            if r.reserved.get(&end).is_some_and(|&n| n != net) {
                continue;
            }
            let path = route::expand(&[pin, end]);
            r.commit_path(net, &path);
            return (end, d);
        }
    }
    (pin, (0, 1))
}
