//! SCHEMATIC LAYOUT — the small vocabulary the generated rooms draw with.
//!
//! Before rigid parts, a generated room placed a device by naming the node
//! points its pins had to reach: an op-amp whose `+` needed the ground node
//! simply *had* its `+` at the ground node, twenty units away, and the symbol
//! stretched to suit. That is how the synth ended up as nineteen skewed
//! slivers over a star of diagonals.
//!
//! A part is now a rigid body, so the two jobs separate and both of them live
//! here:
//!
//! * **Place** the body — [`Sheet::part`] puts a symbol at a spot, facing a
//!   direction, at a length, through `sim_core::shape::place`. The same table
//!   the placement gate checks against, so a generated room is a room a
//!   player could have drawn, and every template is editable.
//! * **Route** to it — [`Sheet::run`] walks an orthogonal polyline of
//!   `Wire`s from pin to pin, and [`Sheet::ground`] drops a local ground
//!   symbol straight onto a pin.
//!
//! Both are nearly free, which is what makes this affordable: a `Wire` merges
//! its ends in `compile`'s union-find and stamps nothing, so it adds no node
//! and no branch unknown; a `Ground` adds neither either. Measured on this
//! room, sixty routing wires cost 0.7 µs per substep against a 20 µs budget
//! (`SCOPE NOTES` in `synth.rs`). Wires are cheap; *devices* are not.
//!
//! Ids: routing is drawn from a pool the devices do not use, so every device
//! in a room keeps the id it has always had. That is what lets the netlist be
//! diffed part-by-part across a re-layout.

use sim_core::shape::{self, Shape};
use sim_core::{ElementKind as K, ElementSpec, Point};

/// Quarter turns for a one-pin symbol's stem, matching the client's
/// `oneAxis`: 0 = down, 1 = left, 2 = up, 3 = right. A `Ground` stem hangs
/// along this direction, a `Rail`'s points against it.
pub const DOWN: u8 = 0;
pub const LEFT: u8 = 1;
pub const UP: u8 = 2;
pub const RIGHT: u8 = 3;

/// Unit axis directions a part's tip can face.
pub const E: Point = (1, 0);
pub const S: Point = (0, 1);
pub const W: Point = (-1, 0);
pub const N: Point = (0, -1);

/// A sheet of schematic under construction.
///
/// Devices and routing are collected SEPARATELY and concatenated
/// devices-first by [`Sheet::finish`], and that is not cosmetic. `compile`
/// numbers electrical nodes by first-seen junction, scanning elements in
/// order, and the node order is the pivot order of a dense LU — so shuffling
/// the element list perturbs the arithmetic in the last bits. Keeping the
/// devices in the order the room has always had, with every new wire and
/// ground symbol appended behind them, is what makes a re-layout provably
/// **bit-identical** rather than merely close: the routing introduces its
/// corner points after every device pin has already been numbered, and a
/// device pin moved onto a fresh point takes the junction index its old
/// point had.
pub struct Sheet {
    /// Devices, in emission order. This order is part of the netlist.
    pub els: Vec<ElementSpec>,
    /// Wires and ground symbols, which carry no current unknown and are
    /// appended after every device.
    pub route: Vec<ElementSpec>,
    next: u32,
}

impl Sheet {
    /// `route_id0` is the first id routing (wires and ground symbols) may
    /// use. It must not collide with the device ids the caller emits — the
    /// devices keep their historical ids so the netlist stays diffable.
    pub fn new(route_id0: u32) -> Sheet {
        Sheet {
            els: Vec::with_capacity(64),
            route: Vec::with_capacity(128),
            next: route_id0,
        }
    }

    /// Devices first, then routing. See the note on [`Sheet`].
    pub fn finish(mut self) -> Vec<ElementSpec> {
        self.els.append(&mut self.route);
        self.els
    }

    /// Take the next routing id. For the rare case where a caller has to
    /// interleave a historical id with fresh ones.
    pub fn next_route_id(&mut self) -> u32 {
        self.id()
    }

    fn id(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// A two-pin device. Free geometry by design: a resistor *is* its two
    /// endpoints, and drawing one as a long straight run from a rail down to
    /// a node is what a schematic does instead of a wire plus a stub.
    pub fn two(&mut self, id: u32, kind: K, a: Point, b: Point) -> &mut Self {
        debug_assert!(
            a.0 == b.0 || a.1 == b.1,
            "element {id} is drawn diagonally from {a:?} to {b:?}"
        );
        self.els.push(ElementSpec {
            id,
            kind,
            pins: vec![a, b],
            ..Default::default()
        });
        self
    }

    /// A rigid multi-pin part: anchor end at `t`, tip along `d`, axial length
    /// `l`, optionally mirrored. Returns its pins so the caller can route to
    /// them by name rather than by recomputing the offsets.
    pub fn part(
        &mut self,
        id: u32,
        kind: K,
        t: Point,
        d: Point,
        l: i32,
        mirrored: bool,
    ) -> Vec<Point> {
        let pins = shape::place(Shape::of(&kind), t, d, l, mirrored);
        debug_assert!(
            shape::is_rigid(&kind, &pins),
            "element {id} was placed outside its own family"
        );
        self.els.push(ElementSpec {
            id,
            kind,
            pins: pins.clone(),
            ..Default::default()
        });
        pins
    }

    /// A local ground symbol hanging off `at`, its stem pointing `rot`.
    ///
    /// Several ground symbols is how a schematic avoids one star of
    /// twenty-unit spokes, and it is the cheapest connection in the engine:
    /// `Ground` pins its point to node 0 in the union-find and stamps
    /// nothing at all.
    pub fn ground(&mut self, at: Point, rot: u8) -> &mut Self {
        let id = self.id();
        self.ground_as(id, at, rot)
    }

    /// A ground symbol that keeps a specific id. Used where a generated room
    /// had a ground symbol before it was re-laid-out: the symbol moves, but
    /// carrying its id means every element in the room can be diffed against
    /// the old netlist by id alone.
    pub fn ground_as(&mut self, id: u32, at: Point, rot: u8) -> &mut Self {
        self.route.push(ElementSpec {
            id,
            kind: K::Ground,
            pins: vec![at],
            rot,
            ..Default::default()
        });
        self
    }

    /// One orthogonal wire. Panics in debug if it is not axis-aligned: a
    /// diagonal in a routed schematic is always a mistake, and this is the
    /// only place one could enter.
    pub fn wire(&mut self, a: Point, b: Point) -> &mut Self {
        let id = self.id();
        self.wire_as(id, a, b)
    }

    /// A wire that keeps a specific id — same reason as [`Sheet::ground_as`].
    pub fn wire_as(&mut self, id: u32, a: Point, b: Point) -> &mut Self {
        debug_assert!(
            a.0 == b.0 || a.1 == b.1,
            "diagonal wire from {a:?} to {b:?}"
        );
        if a == b {
            return self;
        }
        self.route.push(ElementSpec {
            id,
            kind: K::Wire,
            pins: vec![a, b],
            ..Default::default()
        });
        self
    }

    /// An orthogonal polyline of wires through the given corners. Each
    /// consecutive pair must share an axis; zero-length hops are dropped, so
    /// a route may repeat a point without cost.
    pub fn run(&mut self, pts: &[Point]) -> &mut Self {
        for w in pts.windows(2) {
            self.wire(w[0], w[1]);
        }
        self
    }

}
