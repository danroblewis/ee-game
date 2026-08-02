//! Canonical part footprints, mirrored from `packages/app/src/catalog.ts`
//! `makePins()` with a drag of a = (0,0), b = (4,0) (facing right, y-down).
//!
//! A CI-style lockstep test against a catalog dump is milestone M-L0 work;
//! for this prototype the table is transcribed by hand and checked against
//! `ElementKind::pin_count()`.

use sim_core::{ElementKind, Point};

/// Pin offsets for the canonical pose (rot = 0, no mirror), facing right.
/// Index order is the semantic pin order of `netlist.rs` — never permuted.
pub fn footprint(kind: &ElementKind) -> Vec<Point> {
    use ElementKind::*;
    match kind {
        Ground | Rail { .. } => vec![(0, 0)],
        Timer555 => vec![(0, 0), (0, 4), (0, 1), (0, 3), (4, 3), (4, 1)],
        // catalog.ts: inputs split at A, out at B, bias below the middle.
        Ota => vec![(0, -1), (0, 1), (4, 0), (2, 2)],
        OpAmp { .. } => vec![(0, -1), (0, 1), (4, 0)],
        Npn { .. } | Pnp { .. } | Nmos { .. } | Pmos { .. } => {
            vec![(0, 0), (4, -2), (4, 2)]
        }
        Potentiometer { .. } => vec![(0, 0), (2, -2), (4, 0)],
        _ => vec![(0, 0), (4, 0)],
    }
}

/// Apply pose: mirror (y -> -y) first, then `rot` clockwise quarter turns
/// (x, y) -> (-y, x). Screen coordinates are y-down, matching the client.
pub fn xform(p: Point, rot: u8, mirror: bool) -> Point {
    let (mut x, mut y) = p;
    if mirror {
        y = -y;
    }
    for _ in 0..(rot & 3) {
        let (nx, ny) = (-y, x);
        x = nx;
        y = ny;
    }
    (x, y)
}

pub fn posed_pins(kind: &ElementKind, origin: Point, rot: u8, mirror: bool) -> Vec<Point> {
    footprint(kind)
        .into_iter()
        .map(|p| {
            let (x, y) = xform(p, rot, mirror);
            (origin.0 + x, origin.1 + y)
        })
        .collect()
}

pub fn bbox(pts: &[Point]) -> (Point, Point) {
    let mut min = (i32::MAX, i32::MAX);
    let mut max = (i32::MIN, i32::MIN);
    for &(x, y) in pts {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    }
    (min, max)
}

/// Pin semantics for layering: which way does the signal flow through a part?
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    In,
    Out,
    Pwr,
    None,
}

pub fn pin_roles(kind: &ElementKind) -> Vec<Role> {
    use ElementKind::*;
    use Role::*;
    match kind {
        OpAmp { .. } => vec![In, In, Out],
        Ota => vec![In, In, Out, In],
        Npn { .. } | Pnp { .. } | Nmos { .. } | Pmos { .. } => vec![In, Out, Out],
        Timer555 => vec![Pwr, Pwr, In, In, Out, Out],
        VoltageSource { .. } | CurrentSource { .. } | Noise { .. } => vec![Out, Out],
        Speaker { .. } | Motor { .. } | Lamp { .. } | Led { .. } => vec![In, In],
        _ => vec![None; kind.pin_count()],
    }
}

pub fn is_source(kind: &ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::VoltageSource { .. } | ElementKind::CurrentSource { .. } | ElementKind::Noise { .. }
    )
}

pub fn is_sink(kind: &ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Speaker { .. } | ElementKind::Motor { .. } | ElementKind::Lamp { .. } | ElementKind::Led { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprints_match_pin_count() {
        let kinds = vec![
            ElementKind::Wire,
            ElementKind::Ground,
            ElementKind::Resistor { ohms: 1.0 },
            ElementKind::Timer555,
            ElementKind::Ota,
            ElementKind::OpAmp {
                rail: 9.0,
                isc: 0.02,
            },
            ElementKind::Npn { beta: 100.0 },
            ElementKind::Potentiometer {
                ohms: 1.0,
                wiper: 0.5,
            },
        ];
        for k in kinds {
            assert_eq!(footprint(&k).len(), k.pin_count(), "{k:?}");
        }
    }
}
