//! Rigid part geometry: what a multi-pin part is ALLOWED to look like.
//!
//! A two-pin part *is* its two endpoints. A resistor drawn from (0,0) to
//! (7,3) is a perfectly good resistor, and dragging one endpoint is how you
//! draw it. Nothing here constrains those.
//!
//! Everything the catalogue draws as an OBJECT is different. An op-amp is a
//! triangle with `+`, `-` and an output; a 555 is a DIP with six named legs;
//! a BJT is a body with a base and two collectors' worth of geometry. Those
//! symbols mean something, and they only mean it in one shape. Letting a
//! player drag ONE terminal of an op-amp 24 units across the room to reach a
//! ground point turns the symbol into a skewed sliver that nobody can read —
//! which is exactly what happened to the synth room, where 19 of 19 multi-pin
//! parts had been pulled out of formation.
//!
//! So: **a part with more than two pins has one canonical pin layout, and a
//! legal placement is that layout under a rotation and an optional mirror,
//! always axis-aligned.** Formally, the pins of a placement of shape `S` must
//! equal
//!
//! ```text
//!     T( base(S, l) ) + t        T in D4, l >= L_MIN, t in Z^2
//! ```
//!
//! for the ordered `base` layouts written below. `D4` is the eight symmetries
//! of the square: four quarter turns and each of those mirrored. That is
//! exactly the set of transforms the editor can already apply (Q rotates, X/Y
//! mirror), so nothing a player could legitimately do is lost — and a skew is
//! not expressible at all.
//!
//! This module owns three things, and they are here TOGETHER on purpose:
//!
//! * [`canonical_pins`] — the layout generator. It is the single source of
//!   truth for placement (the client reaches it through sim-wasm rather than
//!   keeping a second copy in TypeScript, because a second copy is a second
//!   set of rounding rules and they diverge on the first odd number).
//! * [`is_rigid`] — the predicate the placement gate enforces. Being derived
//!   from the same `base` table as the generator, "the client drew it" and
//!   "the server accepts it" cannot come apart.
//! * [`reshape`] / [`straighten`] — the gesture. Dragging a terminal
//!   reorients and resizes the WHOLE part; it never moves one pin out of
//!   formation. Putting the gesture here rather than in the client means the
//!   client is *incapable* of authoring a skewed part, which is a much better
//!   guarantee than asking it not to.
//!
//! Integer arithmetic only: no floats, no transcendentals, nothing that could
//! move a state hash or differ between native and wasm32.

use crate::netlist::{ElementKind, Point};

/// Shortest a part may be along its own axis. One grid unit — the same floor
/// the drag gesture has always had; a placement gesture that ends where it
/// started still gets [`DEFAULT_LEN`].
pub const MIN_LEN: i32 = 1;

/// Farthest from the origin a pin may sit and still be considered as a rigid
/// body at all. Nothing to do with how big a world may be — it is a guard on
/// the ARITHMETIC. `apply` negates a coordinate to undo a D4 transform, and
/// `-i32::MIN` overflows; the subtractions in `decompose` overflow long before
/// that. A billion grid units is ~10^7 screens wide at full zoom, so no
/// reachable placement is anywhere near it, and a pin beyond it is not a
/// canonical part under any reading. Bounding here keeps every downstream
/// difference and negation in range without threading `Option` through the
/// whole module.
pub const MAX_ABS_COORD: i32 = 1_000_000_000;

/// Is every coordinate inside [`MAX_ABS_COORD`]?
///
/// Range-compared rather than `abs()`-compared, because `i32::MIN.abs()`
/// panics — the exact class of bug this guard exists to prevent, and it was
/// written that way first.
fn coords_sane(pins: &[Point]) -> bool {
    let ok = |v: i32| (-MAX_ABS_COORD..=MAX_ABS_COORD).contains(&v);
    pins.iter().all(|p| ok(p.0) && ok(p.1))
}

/// Axial length a degenerate (zero-length) placement drag lands on, so that
/// clicking once still puts a readable symbol down.
pub const DEFAULT_LEN: i32 = 3;

/// The pin-layout family a part belongs to.
///
/// Named for the SYMBOL, not the device: `Transistor` covers all four of
/// NPN/PNP/NMOS/PMOS because they are drawn with the same three-terminal
/// geometry, and a rule about geometry has no business knowing about carrier
/// types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// One pin (Ground, Rail). There is no geometry to constrain — the
    /// symbol's orientation lives in `ElementSpec::rot`.
    Single,
    /// Two pins, and the part IS its two endpoints. Free by design.
    Free,
    /// `[in+, in-, out]`.
    OpAmp,
    /// `[base/gate, collector/drain, emitter/source]`.
    Transistor,
    /// `[end a, wiper, end b]`.
    Pot,
    /// `[IN, OUT, CLK, GND]` — the bucket brigade. Rigid like the other
    /// chips: a player may rotate and mirror it, but not drag its pins into
    /// a shape no package has.
    Bbd,
    /// `[IN, OUT, VCO, GND, OP1-IN, OP1-OUT, OP2-IN, OP2-OUT]` — the echo
    /// chip. A wide DIP, because it has to hold a legible block diagram of
    /// its own innards.
    Pt2399,
    /// `[in+, in-, out, bias]`.
    Ota,
    /// `[vcc, gnd, trg, thr, out, dis]` — a fixed 4x4 DIP footprint with no
    /// size freedom at all. A chip is the size the chip is.
    Dip555,
}

impl Shape {
    /// The shape of a kind, by its document tag. One table, reached from
    /// both sides: [`Shape::of`] goes through [`ElementKind::tag`], and the
    /// client passes the same `t` string across the wasm boundary.
    ///
    /// An unknown tag is [`Shape::Free`] — the safe direction, because a
    /// shape rule that has never heard of a part must not refuse it.
    pub fn for_tag(tag: &str) -> Shape {
        match tag {
            "Ground" | "Rail" => Shape::Single,
            "OpAmp" => Shape::OpAmp,
            "Npn" | "Pnp" | "Nmos" | "Pmos" => Shape::Transistor,
            "Potentiometer" => Shape::Pot,
            "Ota" => Shape::Ota,
            "Timer555" => Shape::Dip555,
            "Bbd" => Shape::Bbd,
            "Pt2399" => Shape::Pt2399,
            _ => Shape::Free,
        }
    }

    pub fn of(kind: &ElementKind) -> Shape {
        Shape::for_tag(kind.tag())
    }

    /// Parts whose pins this module constrains. Everything else is free.
    pub fn is_rigid_family(self) -> bool {
        !matches!(self, Shape::Single | Shape::Free)
    }

    /// How many pins the family's layout has.
    pub fn pins(self) -> usize {
        match self {
            Shape::Single => 1,
            Shape::Free => 2,
            Shape::OpAmp | Shape::Transistor | Shape::Pot => 3,
            Shape::Ota => 4,
            Shape::Dip555 => 6,
            Shape::Bbd => 4,
            Shape::Pt2399 => 12,
        }
    }

    /// Whether the family's size is a free parameter. A DIP's is not.
    fn sized(self) -> bool {
        !matches!(self, Shape::Dip555 | Shape::Single)
    }
}

/// The canonical layout in the part's own frame: anchor at the origin,
/// axis along +x, perpendicular +y, length `l`.
///
/// The ORDER is the netlist's pin order and must never be permuted — pin 0 of
/// an op-amp is `in+` wherever the symbol ends up pointing.
fn base(shape: Shape, l: i32) -> [Point; 12] {
    // Fixed-size array (never allocates); `Shape::pins` says how much of it
    // is meaningful. Eight because the echo chip is the widest package here;
    // the 555's six and everything smaller just leave the tail unused.
    let mut p = [(0, 0); 12];
    match shape {
        Shape::Single => {}
        Shape::Free => p[1] = (l, 0),
        Shape::OpAmp => {
            // Inputs split either side of the anchor, output at the tip.
            p[0] = (0, -1);
            p[1] = (0, 1);
            p[2] = (l, 0);
        }
        Shape::Transistor => {
            // Base/gate at the anchor, the channel split across the tip.
            p[0] = (0, 0);
            p[1] = (l, -2);
            p[2] = (l, 2);
        }
        Shape::Pot => {
            // Track from anchor to tip, wiper standing off the middle of it.
            // Round the midpoint UP along the axis so the wiper sits at the
            // same place whichever way round the part is drawn — the old
            // client rounded in WORLD coordinates, which put the wiper of an
            // odd-length pot on a different unit depending on the drag
            // direction, and no single canonical layout can contain both.
            p[0] = (0, 0);
            p[1] = ((l + 1) / 2, -2);
            p[2] = (l, 0);
        }
        Shape::Ota => {
            // Like the op-amp, plus a bias lead square out of the body one
            // step back from the tip (where the transconductance balls are
            // drawn), so the lead is a straight run rather than a diagonal.
            p[0] = (0, -1);
            p[1] = (0, 1);
            p[2] = (l, 0);
            p[3] = (l - 1, -2);
        }
        Shape::Dip555 => {
            p[0] = (0, 0); // vcc
            p[1] = (0, 4); // gnd
            p[2] = (0, 1); // trg
            p[3] = (0, 3); // thr
            p[4] = (4, 3); // out
            p[5] = (4, 1); // dis
        }
        // Signal flows LEFT TO RIGHT across the package, which is the one
        // thing a reader needs from a delay: IN on the left edge, OUT on the
        // right. CLK sits under IN because it is the other thing you have to
        // wire, and GND under OUT so the return is next to the output it
        // sources from.
        Shape::Bbd => {
            p[0] = (0, 0); // in
            p[1] = (5, 0); // out
            p[2] = (0, 3); // clk
            p[3] = (5, 3); // gnd
        }
        // A DIP with signal flowing left to right, the delay pin under the
        // input, and the two op-amps on the lower rows where the block
        // diagram puts them.
        // Six rows: the delay's own pins at the top, then the four op-amp
        // stages the chip carries, each with its input on the left and its
        // output on the right.
        Shape::Pt2399 => {
            p[0] = (0, 0); // in
            p[1] = (10, 0); // out
            p[2] = (0, 2); // vco
            p[3] = (10, 2); // gnd
            p[4] = (0, 5); // op1-in
            p[5] = (10, 5); // op1-out
            p[6] = (0, 7); // op2-in
            p[7] = (10, 7); // op2-out
            p[8] = (0, 9); // lpf1-in
            p[9] = (10, 9); // lpf1-out
            p[10] = (0, 11); // lpf2-in
            p[11] = (10, 11); // lpf2-out
        }
    }
    p
}

// --------------------------------------------------------------- the group

/// The eight symmetries of the square as integer matrices `[a, b, c, d]`,
/// acting as `(x, y) -> (a*x + b*y, c*x + d*y)`.
///
/// Index 0..3 are the quarter turns clockwise on screen (+y is down), 4..7
/// are those same turns after a left-right mirror. Every one of them maps the
/// integer grid onto itself exactly, so applying them can neither round nor
/// drift — which is why rotating a part four times returns it bit-for-bit to
/// where it started.
const D4: [[i32; 4]; 8] = [
    [1, 0, 0, 1],   // 0: identity           tip -> +x
    [0, -1, 1, 0],  // 1: quarter turn cw    tip -> +y
    [-1, 0, 0, -1], // 2: half turn          tip -> -x
    [0, 1, -1, 0],  // 3: quarter turn ccw   tip -> -y
    [-1, 0, 0, 1],  // 4: mirror             tip -> -x
    [0, -1, -1, 0], // 5: mirror + cw        tip -> -y
    [1, 0, 0, -1],  // 6: mirror + half      tip -> +x
    [0, 1, 1, 0],   // 7: mirror + ccw       tip -> +y
];

/// Number of transforms in [`D4`]; the first four are the pure rotations.
const ROTATIONS: usize = 4;

fn apply(m: &[i32; 4], p: Point) -> Point {
    (m[0] * p.0 + m[1] * p.1, m[2] * p.0 + m[3] * p.1)
}

/// Inverse of a D4 matrix. Every one of them is orthogonal with determinant
/// +-1, so the inverse is the transpose — no division, no rounding.
fn apply_inv(m: &[i32; 4], p: Point) -> Point {
    (m[0] * p.0 + m[2] * p.1, m[1] * p.0 + m[3] * p.1)
}

/// The quarter turn that sends +x to `d`. `d` must be one of the four unit
/// axis directions.
fn rot_index(d: Point) -> usize {
    match d {
        (0, 1) => 1,
        (-1, 0) => 2,
        (0, -1) => 3,
        _ => 0,
    }
}

/// The transform index that points the tip along `d` with the given handedness.
///
/// A mirrored transform reverses the axis (`T(l, 0) = -l * R(1, 0)`), so the
/// mirrored family reaches direction `d` through the rotation for `-d`. Doing
/// this arithmetically rather than by table is what keeps a flipped op-amp
/// flipped when you swing it round to face the other way.
fn transform_for(d: Point, mirrored: bool) -> usize {
    if mirrored {
        ROTATIONS + rot_index((-d.0, -d.1))
    } else {
        rot_index(d)
    }
}

/// Snap a vector to the nearer axis. Ties (and zero) go horizontal, which is
/// the convention the placement drag has always used.
fn snap_axis(v: Point) -> Point {
    if v.0.abs() >= v.1.abs() {
        (if v.0 < 0 { -1 } else { 1 }, 0)
    } else {
        (0, if v.1 < 0 { -1 } else { 1 })
    }
}

/// Length of `v` along `d`, floored at [`MIN_LEN`].
fn len_along(v: Point, d: Point) -> i32 {
    let n = v.0 * d.0 + v.1 * d.1;
    n.max(MIN_LEN)
}

// ------------------------------------------------------------- the layout

/// Place a part deliberately: family `shape`, anchor end at `t`, tip pointing
/// along the unit axis `d`, axial length `l`, mirrored or not.
///
/// [`canonical_pins`] is the *gesture* — what a drag from A to B produces —
/// and it can only ever build unmirrored parts, because a drag has no way to
/// say which way round the symbol goes. This is the *other* constructor: the
/// one a room generator wants, where the author knows exactly which way the
/// op-amp's `+` should face. It reaches the same [`base`] table and the same
/// [`D4`], so anything it builds passes [`is_rigid`] by construction — which
/// is the point of it being here rather than a second layout table in the
/// server. `shape_place_is_always_rigid` holds it to that over the whole
/// product of families, directions, lengths and handednesses.
pub fn place(shape: Shape, t: Point, d: Point, l: i32, mirrored: bool) -> Vec<Point> {
    build(shape, t, d, l.max(MIN_LEN), mirrored)
}

/// Build the pins of a placement: family `shape`, base origin at `t`, tip
/// direction `d` (a unit axis), axial length `l`, mirrored or not.
fn build(shape: Shape, t: Point, d: Point, l: i32, mirrored: bool) -> Vec<Point> {
    let m = &D4[transform_for(d, mirrored)];
    let b = base(shape, l);
    (0..shape.pins())
        .map(|i| {
            let q = apply(m, b[i]);
            (t.0 + q.0, t.1 + q.1)
        })
        .collect()
}

/// The pin layout for a part dragged from grid point `a` to grid point `b`.
///
/// The direction is snapped to an axis and the length is the projection onto
/// it, so the result is ALWAYS in the canonical family — a diagonal drag
/// gives an axis-aligned part, not a skewed one. This is the whole reason the
/// generator lives beside the predicate: the old client projected nothing, so
/// ordinary diagonal placement produced parts the rule would have refused.
pub fn canonical_pins(shape: Shape, a: Point, b: Point) -> Vec<Point> {
    if shape == Shape::Single {
        return vec![a];
    }
    let v = (b.0 - a.0, b.1 - a.1);
    // A drag that ends where it started is a CLICK, and a click still has to
    // put a readable part down rather than a dot. Two-pin parts take the
    // default length lying flat, exactly as they always have.
    if v == (0, 0) {
        return if shape == Shape::Free {
            vec![a, (a.0 + DEFAULT_LEN, a.1)]
        } else {
            build(shape, a, (1, 0), DEFAULT_LEN, false)
        };
    }
    if shape == Shape::Free {
        return vec![a, b];
    }
    let d = snap_axis(v);
    build(shape, a, d, len_along(v, d), false)
}

// ---------------------------------------------------------- the predicate

/// A placement decomposed back into the family: which D4 transform, what
/// axial length, and where the base origin sits in the world.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// Index into [`D4`].
    pub transform: usize,
    pub len: i32,
    /// World position of the base origin — the part's anchor end.
    pub origin: Point,
}

impl Placement {
    /// True when the placement is a mirror image of the canonical layout
    /// rather than a plain rotation of it.
    pub fn mirrored(self) -> bool {
        self.transform >= ROTATIONS
    }
    /// Unit direction from the anchor end towards the tip.
    ///
    /// True for both handednesses without a special case: a mirrored
    /// transform is built as `R(-d) . M`, and `R(-d)(M(1,0)) = d`.
    pub fn axis(self) -> Point {
        apply(&D4[self.transform], (1, 0))
    }
    /// Unit direction of the part frame's +y — the side the `+` input, the
    /// collector or the wiper stands off on.
    pub fn perp(self) -> Point {
        apply(&D4[self.transform], (0, 1))
    }
    /// World position of the tip end.
    pub fn tip(self) -> Point {
        let d = self.axis();
        (
            self.origin.0 + d.0 * self.len,
            self.origin.1 + d.1 * self.len,
        )
    }
}

/// Recover the placement of a pin list, or `None` if these pins are not any
/// rotation/mirror of the canonical layout — i.e. the part is skewed.
///
/// Brute force over eight transforms, each a handful of integer adds. It runs
/// once per changed element per edit; there is nothing here worth being
/// clever about.
pub fn decompose(shape: Shape, pins: &[Point]) -> Option<Placement> {
    if !shape.is_rigid_family() || pins.len() != shape.pins() || !coords_sane(pins) {
        return None;
    }
    for (ti, m) in D4.iter().enumerate() {
        // Undo the transform, then read the layout's own free parameter
        // straight off the part frame.
        let q: Vec<Point> = pins.iter().map(|&p| apply_inv(m, p)).collect();
        // CHECKED, because these are player-supplied coordinates arriving from
        // the wire and `sim-core` must never panic on hostile input. An op
        // crafted with pins near the i32 extremes used to panic a debug server
        // — killing the room's tick worker, so the room silently stopped
        // applying edits with the socket still open — and in release it
        // wrapped, which is worse: the wrapped difference could pass for a
        // legal length and mint a distorted part. Overflow simply means "not
        // this transform": a part spanning two billion grid units is not a
        // canonical placement under any reading.
        let len = if shape.sized() {
            let l = match shape {
                Shape::Transistor => q[1].0.checked_sub(q[0].0),
                _ => q[2].0.checked_sub(q[0].0),
            };
            match l {
                Some(l) if l >= MIN_LEN => l,
                _ => continue,
            }
        } else {
            0
        };
        let b = base(shape, len);
        let Some(off) = q[0].0.checked_sub(b[0].0).zip(q[0].1.checked_sub(b[0].1)) else {
            continue;
        };
        if (0..shape.pins()).all(|i| {
            b[i]
                .0
                .checked_add(off.0)
                .zip(b[i].1.checked_add(off.1))
                .is_some_and(|e| q[i] == e)
        }) {
            return Some(Placement {
                transform: ti,
                len,
                origin: apply(m, off),
            });
        }
    }
    None
}

/// Does this placement respect the shape rule?
///
/// One-pin and two-pin parts are always fine — the rule is about symbols that
/// mean something, and a resistor's meaning survives any pair of endpoints.
pub fn is_rigid(kind: &ElementKind, pins: &[Point]) -> bool {
    let shape = Shape::of(kind);
    if !shape.is_rigid_family() {
        return true;
    }
    decompose(shape, pins).is_some()
}

/// Are two pin lists the same rigid body — equal up to a D4 transform and a
/// translation?
///
/// This is what grandfathers documents that predate the rule. A part that is
/// already skewed on disk may be dragged, rotated and flipped like any other
/// (all of those are rigid motions, so its SHAPE is unchanged); what it may
/// not do is become a different skewed shape. Skew can therefore only ever
/// leave a document, never enter one.
pub fn same_body(a: &[Point], b: &[Point]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return a.len() == b.len();
    }
    if !coords_sane(a) || !coords_sane(b) {
        return false;
    }
    // Checked for the same reason as `decompose`: `a` and `b` are pin lists
    // off the wire. Overflow means the two bodies are not a rigid motion apart
    // — which is the honest answer, and never a panic.
    D4.iter().any(|m| {
        let r0 = apply(m, a[0]);
        let Some(t) = b[0].0.checked_sub(r0.0).zip(b[0].1.checked_sub(r0.1)) else {
            return false;
        };
        a.iter().zip(b.iter()).all(|(&p, &q)| {
            let r = apply(m, p);
            r.0
                .checked_add(t.0)
                .zip(r.1.checked_add(t.1))
                .is_some_and(|e| e == q)
        })
    })
}

// ------------------------------------------------------------- the gesture

/// What dragging a given terminal of a rigid part does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// A terminal on the anchor end: the tip stays put and the part swings
    /// and stretches to point away from the cursor.
    Anchor,
    /// A terminal on the tip end: the anchor stays put and the part swings
    /// and stretches to point at the cursor.
    Tip,
    /// A terminal that does not define the axis (a pot's wiper, an OTA's
    /// bias lead, any leg of a DIP). There is nothing it could sensibly
    /// reshape, so it carries the whole part: grab the chip by a leg.
    Body,
}

/// Which handle terminal `k` of `shape` is.
pub fn handle(shape: Shape, k: usize) -> Handle {
    match (shape, k) {
        (Shape::OpAmp, 0 | 1) | (Shape::Transistor, 0) | (Shape::Pot, 0) | (Shape::Ota, 0 | 1) => {
            Handle::Anchor
        }
        (Shape::OpAmp, 2) | (Shape::Transistor, 1 | 2) | (Shape::Pot, 2) | (Shape::Ota, 2) => {
            Handle::Tip
        }
        _ => Handle::Body,
    }
}

/// Force a skewed pin list back into the canonical family, keeping its
/// orientation and rough size.
///
/// Used on legacy parts the moment a player takes hold of one: the anchor and
/// tip are estimated from whatever the pins currently say, and the layout is
/// rebuilt around them. Deliberately a snap, not a nudge — half-straight is
/// not a state this editor has.
pub fn straighten(shape: Shape, pins: &[Point]) -> Vec<Point> {
    if !shape.is_rigid_family() || pins.len() != shape.pins() {
        return pins.to_vec();
    }
    let mid = |a: Point, b: Point| ((a.0 + b.0) / 2, (a.1 + b.1) / 2);
    // The two ends the axis runs between, and a reference terminal whose
    // perpendicular offset says which way round the symbol is. Estimating
    // handedness rather than assuming it is what makes this IDEMPOTENT: a
    // part already in the family straightens to itself, mirror included, so
    // the gesture can run it unconditionally.
    let (a, b, r, base_perp) = match shape {
        Shape::OpAmp | Shape::Ota => (mid(pins[0], pins[1]), pins[2], 0, -1),
        Shape::Transistor => (pins[0], mid(pins[1], pins[2]), 1, -2),
        Shape::Pot => (pins[0], pins[2], 1, -2),
        // A DIP has no axial pin pair. `vcc -> out` is (4, 3) in the part
        // frame, so its dominant component is the axis; `vcc -> gnd` is pure
        // perpendicular and would tell us nothing about which is which.
        Shape::Dip555 => (pins[0], pins[4], 1, 4),
        Shape::Bbd => (pins[0], pins[1], 2, 3),
        Shape::Pt2399 => (pins[0], pins[1], 2, 11),
        Shape::Single | Shape::Free => return pins.to_vec(),
    };
    let v = (b.0 - a.0, b.1 - a.1);
    let d = if v == (0, 0) { (1, 0) } else { snap_axis(v) };
    let l = if v == (0, 0) {
        DEFAULT_LEN
    } else {
        len_along(v, d)
    };
    let perp = (-d.1, d.0);
    let off = (pins[r].0 - a.0, pins[r].1 - a.1);
    let side = off.0 * perp.0 + off.1 * perp.1;
    let mirrored = side != 0 && (side > 0) != (base_perp > 0);
    build(shape, a, d, l, mirrored)
}

/// Drag terminal `k` to `cursor`, and give back the whole part.
///
/// The three outcomes are [`Handle`]'s three cases. In every one of them the
/// result is a fresh `canonical_pins`-family layout — this function has no
/// path that moves one terminal on its own, which is the property the rule
/// depends on.
///
/// `None` means the drag would change nothing.
pub fn reshape(kind: &ElementKind, pins: &[Point], k: usize, cursor: Point) -> Option<Vec<Point>> {
    reshape_shape(Shape::of(kind), pins, k, cursor)
}

/// [`reshape`], keyed by family rather than by kind — what the client calls
/// across the wasm boundary, where all it has is the document tag.
pub fn reshape_shape(shape: Shape, pins: &[Point], k: usize, cursor: Point) -> Option<Vec<Point>> {
    if !shape.is_rigid_family() || k >= pins.len() || pins.len() != shape.pins() {
        return None;
    }
    // Always start from a known frame. `straighten` is the identity on parts
    // already in the family, so this both normalises legacy skew and leaves
    // an ordinary part (mirror and all) exactly as it was.
    let straight = straighten(shape, pins);
    let pl = decompose(shape, &straight)?;
    let out = match handle(shape, k) {
        Handle::Tip => {
            let v = (cursor.0 - pl.origin.0, cursor.1 - pl.origin.1);
            let d = snap_axis(v);
            build(shape, pl.origin, d, len_along(v, d), pl.mirrored())
        }
        Handle::Anchor => {
            let tip = pl.tip();
            let v = (tip.0 - cursor.0, tip.1 - cursor.1);
            let d = snap_axis(v);
            let l = len_along(v, d);
            let origin = (tip.0 - d.0 * l, tip.1 - d.1 * l);
            build(shape, origin, d, l, pl.mirrored())
        }
        Handle::Body => {
            let dx = cursor.0 - straight[k].0;
            let dy = cursor.1 - straight[k].1;
            straight.iter().map(|p| (p.0 + dx, p.1 + dy)).collect()
        }
    };
    if out == pins {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [Shape; 5] = [
        Shape::OpAmp,
        Shape::Transistor,
        Shape::Pot,
        Shape::Ota,
        Shape::Dip555,
    ];

    fn kind_for(shape: Shape) -> ElementKind {
        match shape {
            Shape::OpAmp => ElementKind::OpAmp {
                rail: 8.0,
                isc: 0.025,
            },
            Shape::Transistor => ElementKind::Npn { beta: 100.0 },
            Shape::Pot => ElementKind::Potentiometer {
                ohms: 10e3,
                wiper: 0.5,
            },
            Shape::Ota => ElementKind::Ota,
            Shape::Dip555 => ElementKind::Timer555,
            Shape::Bbd => ElementKind::Bbd { stages: 1024 },
            Shape::Pt2399 => ElementKind::Pt2399,
            Shape::Single => ElementKind::Ground,
            Shape::Free => ElementKind::Wire,
        }
    }

    /// [`place`] is the constructor a room generator builds a schematic with,
    /// so anything it can produce must already satisfy the gate — otherwise a
    /// shipped template would be a document the editor refuses to touch.
    #[test]
    fn shape_place_is_always_rigid() {
        for shape in SHAPES {
            let kind = kind_for(shape);
            for d in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
                for mirrored in [false, true] {
                    for l in [MIN_LEN, 1, 2, 3, 4, 5, 8, 13] {
                        for t in [(0, 0), (7, -3), (-11, 40)] {
                            let pins = place(shape, t, d, l, mirrored);
                            assert_eq!(pins.len(), kind.pin_count());
                            assert!(
                                is_rigid(&kind, &pins),
                                "{shape:?} placed at {t:?} along {d:?} len {l} \
                                 mirrored={mirrored} is not in its own family: {pins:?}"
                            );
                            let got = decompose(shape, &pins).expect("decomposes");
                            assert_eq!(got.axis(), d, "{shape:?} tip direction");
                            assert_eq!(got.mirrored(), mirrored, "{shape:?} handedness");
                        }
                    }
                }
            }
        }
    }

    /// Every kind's tag maps to a family whose pin count matches the
    /// netlist's. This is the join between the two tables and it is the only
    /// place they can disagree.
    #[test]
    fn shape_table_agrees_with_pin_count() {
        for k in all_kinds() {
            assert_eq!(
                Shape::of(&k).pins(),
                k.pin_count(),
                "{} pin count vs shape family",
                k.tag()
            );
        }
    }

    fn all_kinds() -> Vec<ElementKind> {
        use ElementKind::*;
        vec![
            Wire,
            Ground,
            Resistor { ohms: 1e3 },
            Lamp {
                ohms: 1e3,
                rated_watts: 1.0,
            },
            Speaker { ohms: 8.0 },
            Capacitor { farads: 1e-6 },
            Inductor { henries: 1e-3 },
            VoltageSource {
                wave: crate::netlist::Wave::Sine,
                dc: 9.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
            },
            CurrentSource { amps: 1e-3 },
            Rail {
                wave: crate::netlist::Wave::Sine,
                dc: 9.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
            },
            Switch { closed: false },
            Button { closed: false },
            Diode,
            Zener { vz: 5.1 },
            Led { color: 0 },
            Npn { beta: 100.0 },
            Pnp { beta: 100.0 },
            Nmos { vt: 2.0, k: 0.05 },
            Pmos { vt: 2.0, k: 0.05 },
            OpAmp {
                rail: 8.0,
                isc: 0.025,
            },
            Ota,
            Timer555,
            Potentiometer {
                ohms: 1e4,
                wiper: 0.5,
            },
            Motor {
                ohms: 1.0,
                henries: 1e-3,
                bemf: 0.0,
            },
            Noise {
                volts: 1.0,
                ohms: 1e3,
                seed: 1,
            },
        ]
    }

    /// Anything the generator can draw, the gate accepts. Runs the whole
    /// cross-product of family x drag direction x length, including the
    /// diagonal drags that used to produce skewed parts.
    #[test]
    fn every_generated_placement_is_rigid() {
        for shape in SHAPES {
            let kind = kind_for(shape);
            for dx in -9..=9 {
                for dy in -9..=9 {
                    let pins = canonical_pins(shape, (5, 7), (5 + dx, 7 + dy));
                    assert_eq!(pins.len(), shape.pins());
                    assert!(
                        is_rigid(&kind, &pins),
                        "{shape:?} dragged ({dx},{dy}) is not in its own family: {pins:?}"
                    );
                    // Axis-aligned: every pin is on the lattice the part
                    // frame defines, so no two pins are diagonal neighbours
                    // by accident of the drag.
                    let pl = decompose(shape, &pins).unwrap();
                    let ax = pl.axis();
                    assert!(ax.0.abs() + ax.1.abs() == 1, "axis not a unit direction");
                }
            }
        }
    }

    #[test]
    fn skewed_pins_are_refused() {
        let op = kind_for(Shape::OpAmp);
        // The exact shape the old client produced from a diagonal drag.
        assert!(!is_rigid(&op, &[(0, -1), (0, 1), (10, 3)]));
        // One terminal pulled out of formation.
        assert!(!is_rigid(&op, &[(0, -1), (0, 5), (4, 0)]));
        // Two of its own terminals on one point: no canonical layout has
        // that, so the rheostat/follower idioms need a wire instead.
        assert!(!is_rigid(&op, &[(0, -1), (4, 0), (4, 0)]));
        // Wrong pin count.
        assert!(!is_rigid(&op, &[(0, -1), (0, 1)]));
    }

    /// A drag that ends where it started still lands a part you can see —
    /// the behaviour the client's old `makePins` had, and the one thing in
    /// it that was not about skew.
    #[test]
    fn a_click_still_puts_a_whole_part_down() {
        assert_eq!(
            canonical_pins(Shape::Free, (5, 5), (5, 5)),
            vec![(5, 5), (5 + DEFAULT_LEN, 5)]
        );
        for shape in SHAPES {
            let pins = canonical_pins(shape, (5, 5), (5, 5));
            assert!(is_rigid(&kind_for(shape), &pins), "{shape:?}");
            assert_eq!(pins.len(), shape.pins());
        }
        assert_eq!(canonical_pins(Shape::Single, (5, 5), (5, 5)), vec![(5, 5)]);
    }

    #[test]
    fn two_pin_parts_stay_free() {
        let r = ElementKind::Resistor { ohms: 1e3 };
        assert!(is_rigid(&r, &[(0, 0), (7, 3)]));
        assert!(is_rigid(&r, &[(0, 0), (0, 0)]));
        assert!(is_rigid(&ElementKind::Ground, &[(3, 4)]));
    }

    /// Rotating four times is the identity, and every intermediate is legal.
    #[test]
    fn rotation_and_mirror_stay_in_family() {
        for shape in SHAPES {
            let kind = kind_for(shape);
            let start = canonical_pins(shape, (2, 3), (9, 3));
            let mut pins = start.clone();
            for _ in 0..4 {
                pins = pins.iter().map(|p| (-p.1, p.0)).collect();
                assert!(is_rigid(&kind, &pins), "{shape:?} rotated is not rigid");
                assert!(same_body(&start, &pins));
            }
            assert_eq!(pins, start);
            // Mirror about x = 0.
            let m: Vec<Point> = start.iter().map(|p| (-p.0, p.1)).collect();
            assert!(is_rigid(&kind, &m), "{shape:?} mirrored is not rigid");
            assert!(same_body(&start, &m));
            // ...and a mirrored part is still mirrored after a reshape.
            let pl = decompose(shape, &m).unwrap();
            let r = reshape(&kind, &m, tip_pin(shape), (pl.origin.0, pl.origin.1 + 6)).unwrap();
            assert!(is_rigid(&kind, &r));
            assert_eq!(
                decompose(shape, &r).unwrap().mirrored(),
                pl.mirrored(),
                "{shape:?} lost its handedness: {m:?} -> {r:?}"
            );
        }
    }

    fn tip_pin(shape: Shape) -> usize {
        (0..shape.pins())
            .find(|&k| handle(shape, k) == Handle::Tip)
            .unwrap_or(0)
    }

    /// The gesture cannot author a skewed part, from any starting shape, for
    /// any terminal, to any cursor position.
    #[test]
    fn reshape_never_skews() {
        for shape in SHAPES {
            let kind = kind_for(shape);
            let starts = [
                canonical_pins(shape, (0, 0), (6, 0)),
                canonical_pins(shape, (0, 0), (0, -5)),
                // A legacy skewed part: pins scattered off formation.
                {
                    let mut p = canonical_pins(shape, (0, 0), (6, 0));
                    p[shape.pins() - 1] = (13, -7);
                    p
                },
            ];
            for start in starts {
                for k in 0..shape.pins() {
                    for cx in -8..=8 {
                        for cy in -8..=8 {
                            if let Some(out) = reshape(&kind, &start, k, (cx, cy)) {
                                assert!(
                                    is_rigid(&kind, &out),
                                    "{shape:?} pin {k} -> ({cx},{cy}) produced {out:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Dragging a tip terminal points the part at the cursor and leaves the
    /// anchor exactly where it was — the property that makes the gesture read
    /// as "reorient this part" rather than "something moved".
    #[test]
    fn tip_drag_pivots_on_the_anchor() {
        let kind = kind_for(Shape::OpAmp);
        let pins = canonical_pins(Shape::OpAmp, (0, 0), (5, 0));
        let out = reshape(&kind, &pins, 2, (0, 7)).unwrap();
        let pl = decompose(Shape::OpAmp, &out).unwrap();
        assert_eq!(pl.origin, (0, 0));
        assert_eq!(pl.axis(), (0, 1));
        assert_eq!(pl.len, 7);
        assert_eq!(out[2], (0, 7));
    }

    /// Dragging an anchor terminal leaves the TIP where it was.
    #[test]
    fn anchor_drag_pivots_on_the_tip() {
        let kind = kind_for(Shape::Transistor);
        let pins = canonical_pins(Shape::Transistor, (0, 0), (5, 0));
        let tip = decompose(Shape::Transistor, &pins).unwrap().tip();
        let out = reshape(&kind, &pins, 0, (5, -6)).unwrap();
        assert_eq!(decompose(Shape::Transistor, &out).unwrap().tip(), tip);
    }

    /// A DIP has no size freedom: dragging any leg carries the chip.
    #[test]
    fn dip_legs_carry_the_chip() {
        let kind = kind_for(Shape::Dip555);
        let pins = canonical_pins(Shape::Dip555, (0, 0), (4, 0));
        let out = reshape(&kind, &pins, 4, (10, 10)).unwrap();
        assert_eq!(out[4], (10, 10));
        let d = (out[0].0 - pins[0].0, out[0].1 - pins[0].1);
        for (a, b) in pins.iter().zip(out.iter()) {
            assert_eq!((a.0 + d.0, a.1 + d.1), *b);
        }
    }

    /// Straightening a legacy part keeps its orientation and rough length.
    #[test]
    fn straighten_snaps_into_formation() {
        // The synth's CV follower: out dragged onto in-, at an angle.
        let skew = vec![(42, 34), (48, 34), (48, 34)];
        let out = straighten(Shape::OpAmp, &skew);
        assert!(is_rigid(&kind_for(Shape::OpAmp), &out));
        let pl = decompose(Shape::OpAmp, &out).unwrap();
        assert_eq!(pl.axis(), (1, 0));
        assert_eq!(pl.origin, (45, 34));
    }

    /// Straightening is the identity on anything already in the family —
    /// including mirrored placements, which an earlier version silently
    /// un-mirrored (swapping in+ and in- on every reshape of a flipped
    /// op-amp: a NETLIST change from a drawing gesture).
    #[test]
    fn straighten_is_identity_in_the_family() {
        for shape in SHAPES {
            for &(bx, by) in &[(9, 3), (-9, 3), (2, 11), (2, -11)] {
                let base = canonical_pins(shape, (2, 3), (bx, by));
                assert_eq!(straighten(shape, &base), base, "{shape:?} plain");
                for m in [
                    |p: Point| (-p.0, p.1),
                    |p: Point| (p.0, -p.1),
                    |p: Point| (-p.1, p.0),
                ] {
                    let t: Vec<Point> = base.iter().map(|&p| m(p)).collect();
                    assert!(is_rigid(&kind_for(shape), &t));
                    assert_eq!(straighten(shape, &t), t, "{shape:?} transformed");
                }
            }
        }
    }

    #[test]
    fn same_body_is_exactly_the_rigid_motions() {
        let a = vec![(0, -1), (0, 1), (7, 3)]; // skewed, deliberately
        let t: Vec<Point> = a.iter().map(|p| (p.0 + 40, p.1 - 9)).collect();
        assert!(same_body(&a, &t), "translation");
        let r: Vec<Point> = a.iter().map(|p| (-p.1, p.0)).collect();
        assert!(same_body(&a, &r), "rotation");
        let m: Vec<Point> = a.iter().map(|p| (-p.0, p.1)).collect();
        assert!(same_body(&a, &m), "mirror");
        let mut s = a.clone();
        s[2] = (8, 3);
        assert!(!same_body(&a, &s), "a different skew is a different body");
    }
}

#[cfg(test)]
mod hostile_coords {
    use super::*;

    /// `sim-core` must never panic on input off the wire. These pin lists are
    /// what a crafted op can send; before the checked arithmetic they panicked
    /// a debug build (killing the room's tick worker, so the room silently
    /// stopped applying edits with its socket still open) and wrapped in a
    /// release build, where a wrapped difference could pass for a legal length.
    #[test]
    fn extreme_coordinates_are_refused_not_panicked() {
        let hostile: &[&[Point]] = &[
            &[(0, -1), (0, 1), (i32::MAX, 0)],
            &[(i32::MIN, -1), (i32::MIN, 1), (i32::MAX, 0)],
            &[(i32::MIN, i32::MIN), (i32::MAX, i32::MAX), (0, 0)],
            &[(i32::MAX, i32::MAX), (i32::MAX, i32::MIN), (i32::MIN, 0)],
            &[(0, 0), (i32::MIN, -2), (i32::MIN, 2)],
        ];
        for pins in hostile {
            for shape in [Shape::OpAmp, Shape::Transistor, Shape::Pot] {
                // The only requirement is that it RETURNS.
                let _ = decompose(shape, pins);
            }
            assert!(
                !same_body(pins, pins) || pins.iter().all(|p| p.0.abs() <= MAX_ABS_COORD),
                "an out-of-range body must not claim to be a rigid motion"
            );
        }
    }

    /// The guard must not touch anything a player can actually draw.
    #[test]
    fn ordinary_placements_are_untouched_by_the_bound() {
        for shape in [Shape::OpAmp, Shape::Transistor, Shape::Pot, Shape::Ota] {
            let pins = canonical_pins(shape, (0, 0), (4, 0));
            assert!(decompose(shape, &pins).is_some(), "{shape:?} canonical must stay legal");
            let far: Vec<Point> = pins.iter().map(|p| (p.0 + 900_000, p.1 - 750_000)).collect();
            assert!(
                decompose(shape, &far).is_some(),
                "{shape:?} far from origin must stay legal"
            );
        }
    }
}
