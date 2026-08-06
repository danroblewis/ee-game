//! THE LOGIC BENCH — one of every digital part, wired and running.
//!
//! This room exists because of a specific complaint: "I can't get the shift
//! register to work, I don't know where the input pin is." Both halves of
//! that are fair, and neither is a bug in the device — the shift register
//! shifts correctly and has done since it landed. What it lacks is a place
//! where you can SEE one working and copy the wiring, which is what every
//! other chip in this file also lacks.
//!
//! THE TWO THINGS THAT MAKE A LOGIC CHIP LOOK BROKEN, both on display here:
//!
//!   1. `/RST` IS ACTIVE LOW. A reset pin left unwired sits at whatever the
//!      node happens to be — near zero — and a shift register held in reset
//!      is a shift register whose outputs are all zero forever, no matter
//!      how you clock it. It looks exactly like a dead part. Every chip in
//!      this room ties `/RST` to the 5 V rail, and the label box says so.
//!   2. THE SERIAL INPUT IS CALLED `SER`. It is the second input pin down
//!      the left edge, under `CLK`. Here it is on a switch you can flip
//!      while the clock runs, so you can watch the one you fed in walk
//!      across Q0 -> Q1 -> Q2 -> Q3 and fall off the end.
//!
//! Every input that a player is not driving gets a PULL-DOWN. A logic input
//! draws a nanoamp (`LOGIC_G_IN`), so an unwired one is not "low", it is
//! undefined and will sit whereever the last connected thing left it. A
//! 100 kΩ resistor to ground costs 50 µA at 5 V and removes the entire
//! class of "it works until I touch something else" mysteries.
//!
//! The clock is a 2 Hz square wave rather than anything faster, because the
//! point of the room is to WATCH the state move. Every output is on a lamp:
//! at 470 Ω against the 50 Ω a logic output drives through, a lit one is
//! carrying about 43 mW against a 100 mW nameplate, which is bright without
//! being anywhere near its rating.

use crate::templates::{RoomSetup, View};
use sim_core::{ElementKind as K, ElementSpec, Wave};
use sim_golden::{gnd, r, spec};

use crate::LabelBox;

/// Supply, in volts. Well under `LOGIC_V_ABSMAX` (7 V): logic on a 9 V rail
/// latches up, which is its own lesson in another room.
const VCC: f64 = 5.0;
/// How fast the shared clock runs. Slow enough to follow by eye.
const CLK_HZ: f64 = 2.0;
/// Pull-down on every player-driven input.
const PULL: f64 = 100_000.0;
/// Indicator lamp: resistance and nameplate.
const LAMP_R: f64 = 470.0;
const LAMP_W: f64 = 0.1;

fn lamp() -> K {
    K::Lamp {
        ohms: LAMP_R,
        rated_watts: LAMP_W,
    }
}

/// The 5 V bus, the clock bus, and the ground bus: three horizontal lines the
/// whole room hangs off, so no block needs its own supply and the sheet reads
/// left to right as five independent experiments.
const BUS_V: i32 = 0;
const BUS_C: i32 = 2;
const BUS_G: i32 = 24;
/// Left edge of the first block, and the spacing between blocks.
const X0: i32 = 6;
/// A station reaches from `x - 15` (the mux's sixth switch leg) to `x + 14`
/// (a four-output chip's last lamp), so anything under ~30 overlaps its
/// neighbour and the label boxes cross each other.
const DX: i32 = 34;
/// A chip's own box: `y` of its VCC pin, its height, and its width.
const CHIP_Y: i32 = 8;
const CHIP_H: i32 = 10;
const CHIP_W: i32 = 6;

/// A logic chip's pin list, in the order every logic part uses:
/// `[VCC, GND, inputs.., outputs..]`. Inputs run down the left edge under
/// VCC, outputs down the right edge, which is what the renderer draws a DIP
/// around and what a reader expects to find.
fn chip_pins(x: i32, y: i32, n_in: usize, n_out: usize) -> Vec<(i32, i32)> {
    let mut p = vec![(x, y), (x, y + CHIP_H)];
    for k in 0..n_in {
        p.push((x, y + 1 + k as i32));
    }
    for j in 0..n_out {
        p.push((x + CHIP_W, y + 1 + j as i32));
    }
    p
}

fn chip(id: u32, kind: K, pins: Vec<(i32, i32)>) -> ElementSpec {
    ElementSpec {
        id,
        kind,
        pins,
        ..Default::default()
    }
}

/// One station: the chip, its supply stubs, an input leg per input pin and a
/// lamp per output. Every station is wired identically so that comparing two
/// of them compares the CHIPS, not two different routing habits.
///
/// `clk` says input 0 is a clock and comes off the shared bus; `rst_at` says
/// which input is the active-low reset and must be tied to the supply.
/// Everything else gets a switch to 5 V over a pull-down.
/// Where a bus is tapped. A CONNECTION IS A SHARED ENDPOINT, not a crossing:
/// a stub that stops on the middle of one long bus wire touches nothing, and
/// the whole room reads 0 V with no error anywhere — which is exactly what
/// the first draft of this file did. So every station records the x it taps
/// on each bus, and the buses are emitted afterwards as one segment per gap.
#[derive(Default)]
struct Taps {
    v: Vec<i32>,
    c: Vec<i32>,
    g: Vec<i32>,
}

#[allow(clippy::too_many_arguments)]
fn station(
    e: &mut Vec<ElementSpec>,
    id: &mut u32,
    taps: &mut Taps,
    slot: i32,
    kind: K,
    n_in: usize,
    n_out: usize,
    clk: bool,
    rst_at: Option<usize>,
) {
    let mut n = || {
        *id += 1;
        *id
    };
    let x = X0 + DX * slot;
    let pins = chip_pins(x, CHIP_Y, n_in, n_out);
    e.push(chip(n(), kind, pins.clone()));
    // VCC up to the 5 V bus, GND down to the ground bus.
    e.push(spec(n(), K::Wire, pins[0], (x, BUS_V)));
    taps.v.push(x);
    e.push(spec(n(), K::Wire, pins[1], (x, BUS_G)));
    taps.g.push(x);
    // Inputs. Pin index 2 is the first input.
    for k in 0..n_in {
        let p = pins[2 + k];
        if clk && k == 0 {
            // The clock rides in on the shared bus.
            e.push(spec(n(), K::Wire, p, (x - 2, p.1)));
            e.push(spec(n(), K::Wire, (x - 2, p.1), (x - 2, BUS_C)));
            taps.c.push(x - 2);
        } else if rst_at == Some(k) {
            // /RST TIED HIGH. This is the wire whose absence makes a shift
            // register look dead, so it is drawn as its own short run to the
            // supply rather than buried in a bus.
            e.push(spec(n(), K::Wire, p, (x - 3, p.1)));
            e.push(spec(n(), K::Wire, (x - 3, p.1), (x - 3, BUS_V)));
            taps.v.push(x - 3);
        } else {
            // A switch to 5 V with a pull-down under it: closed = high, open
            // = a DEFINED low rather than a floating guess.
            let sx = x - 5 - 2 * k as i32;
            e.push(spec(n(), K::Wire, p, (sx, p.1)));
            e.push(spec(n(), K::Switch { closed: false }, (sx, p.1), (sx, p.1 - 3)));
            e.push(spec(n(), K::Wire, (sx, p.1 - 3), (sx, BUS_V)));
            taps.v.push(sx);
            e.push(spec(n(), r(PULL), (sx, p.1), (sx, BUS_G)));
            taps.g.push(sx);
        }
    }
    // Outputs, each on its own lamp down to the ground bus.
    for j in 0..n_out {
        let p = pins[2 + n_in + j];
        let lx = x + CHIP_W + 2 + j as i32 * 2;
        e.push(spec(n(), K::Wire, p, (lx, p.1)));
        e.push(spec(n(), lamp(), (lx, p.1), (lx, BUS_G)));
        taps.g.push(lx);
    }
}

/// Lay a bus down as one wire per gap between the points that tap it, so
/// every tap lands on a segment ENDPOINT and therefore actually connects.
fn bus(e: &mut Vec<ElementSpec>, id: &mut u32, y: i32, taps: &[i32]) {
    let mut xs: Vec<i32> = taps.to_vec();
    xs.sort_unstable();
    xs.dedup();
    for w in xs.windows(2) {
        *id += 1;
        e.push(spec(*id, K::Wire, (w[0], y), (w[1], y)));
    }
}

/// The room. One station per chip, left to right.
pub fn logic_room_circuit() -> Vec<ElementSpec> {
    let mut e: Vec<ElementSpec> = Vec::new();
    let mut id = 100u32;
    let mut taps = Taps::default();

    // The supply and the clock both live at the far left, and both are a tap
    // like any other.
    let src = X0 - 14;
    e.push(ElementSpec {
        id: 1,
        kind: K::Rail {
            dc: VCC,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
            wave: Wave::Sine,
        },
        pins: vec![(src, BUS_V)],
        ..Default::default()
    });
    taps.v.push(src);
    // The clock: a 0..5 V square, so it crosses both logic thresholds.
    e.push(spec(
        3,
        K::VoltageSource {
            dc: VCC / 2.0,
            amp: VCC / 2.0,
            hz: CLK_HZ,
            phase: 0.0,
            wave: Wave::Square,
        },
        (src, BUS_C),
        (src, BUS_G),
    ));
    taps.c.push(src);
    taps.g.push(src);
    e.push(gnd(6, (src, BUS_G)));

    // 0 — AND gate: no clock, two switches, one lamp. The reference station.
    station(&mut e, &mut id, &mut taps, 0, K::Gate { op: sim_core::GateOp::And, ins: 2 }, 2, 1, false, None);
    // 1 — D flip-flop: CLK, D on a switch, /RST tied high. Q and /Q.
    station(&mut e, &mut id, &mut taps, 1, K::FlipFlop { edge: true }, 3, 2, true, Some(2));
    // 2 — THE SHIFT REGISTER. CLK, SER on a switch, /RST tied high, Q0..Q3.
    station(&mut e, &mut id, &mut taps, 2, K::ShiftReg { bits: 4 }, 3, 4, true, Some(2));
    // 3 — Counter: CLK, /RST tied high, Q0..Q3 counting in binary.
    station(&mut e, &mut id, &mut taps, 3, K::Counter { bits: 4, modulus: 16 }, 2, 4, true, Some(1));
    // 4 — 4-way mux: four channels and two select lines, all on switches.
    station(&mut e, &mut id, &mut taps, 4, K::Mux { sel: 2 }, 6, 1, false, None);

    bus(&mut e, &mut id, BUS_V, &taps.v);
    bus(&mut e, &mut id, BUS_C, &taps.c);
    bus(&mut e, &mut id, BUS_G, &taps.g);
    e
}

/// The block headings, which are where the two gotchas are written down.
pub fn logic_label_boxes() -> Vec<LabelBox> {
    let b = |i: u32, slot: i32, name: &str| LabelBox {
        blid: i,
        x0: (X0 + DX * slot - 17) as f64,
        y0: (BUS_V - 3) as f64,
        x1: (X0 + DX * slot + CHIP_W + 10) as f64,
        y1: (BUS_G + 3) as f64,
        name: name.to_string(),
    };
    vec![
        b(1, 0, "AND - both switches up"),
        b(2, 1, "D FLIP-FLOP - CLK samples D"),
        b(3, 2, "SHIFT REG - input is SER"),
        b(4, 3, "COUNTER - /RST tied high"),
        b(5, 4, "MUX4 - S0/S1 select I0..3"),
    ]
}

pub fn logic_setup() -> RoomSetup {
    let label_boxes = logic_label_boxes();
    let next_blid = label_boxes.len() as u32 + 1;
    RoomSetup {
        elements: logic_room_circuit(),
        label_boxes,
        next_blid,
        machine: crate::templates::MachineSpec::None,
        view: View {
            // The whole bench: the supply rail sits at `X0 - 14` and the
            // first label box starts at `X0 - 17`, so a home rect measured
            // from the first CHIP cuts the left-hand station in half.
            home: Some([
                (X0 - 22) as f64,
                (BUS_V - 7) as f64,
                (X0 + DX * 4 + CHIP_W + 18) as f64,
                (BUS_G + 8) as f64,
            ]),
            scopes: Vec::new(),
        },
        ..RoomSetup::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2e::Room;

    /// Where each station's parts actually landed, so the tests below can name
    /// pins by meaning instead of by index into a list nobody can read.
    fn shiftreg_pins() -> Vec<(i32, i32)> {
        chip_pins(X0 + DX * 2, CHIP_Y, 3, 4)
    }

    #[test]
    fn the_bench_is_a_legal_room_that_solves() {
        Room::template("logic").gate_ok();
    }

    /// THE HEADLINE, and the reason this room exists. Feed a 1 into SER, let
    /// the clock run, and it must appear at Q0 and then walk to Q1, Q2, Q3.
    ///
    /// This is asserted on the SOLVER's node voltages, not on the device's
    /// internal state: what the player sees is lamps, and a lamp is lit by a
    /// node voltage. A shift register whose bits move but whose outputs never
    /// reach the lamps would pass an internal check and fail the room.
    #[test]
    fn a_one_walks_across_the_outputs() {
        let mut room = Room::template("logic");
        room.gate_ok();
        let p = shiftreg_pins();
        // [VCC, GND, CLK, SER, /RST, Q0, Q1, Q2, Q3]
        let (q0, q1, q2, q3) = (p[5], p[6], p[7], p[8]);
        let hi = |v: f64| v > 2.5;

        // Everything starts empty: /RST is tied high so the register is NOT
        // held in reset, and with SER open (pulled down) nothing is shifted
        // in. Two clock periods is four edges, plenty to settle.
        room.run(1.0);
        assert!(
            !hi(room.volts(q0)) && !hi(room.volts(q3)),
            "an idle register with SER low must read zero, got Q0={} Q3={}",
            room.volts(q0),
            room.volts(q3)
        );

        // Close the SER switch. Which switch that is: the shift register's
        // station is slot 2, and SER is input 1, so its switch is the one at
        // that leg's x. Find it by position rather than by id arithmetic.
        let ser_leg = p[3];
        let sx = X0 + DX * 2 - 5 - 2;
        let ser_switch = room
            .switch_at((sx, ser_leg.1))
            .expect("the SER switch is where the station put it");
        room.set_switch(ser_switch, true);

        // One clock period per bit. At 2 Hz a period is 0.5 s, and the bit
        // advances on the RISING edge, so sampling just after each edge shows
        // the front of the pattern moving one place per period.
        let mut seen = Vec::new();
        for _ in 0..5 {
            room.run(0.5);
            seen.push((
                hi(room.volts(q0)),
                hi(room.volts(q1)),
                hi(room.volts(q2)),
                hi(room.volts(q3)),
            ));
        }
        // The register fills from Q0 upward: some sample must show Q0 high
        // while Q3 is still low, and a later one must show Q3 high.
        assert!(
            seen.iter().any(|s| s.0 && !s.3),
            "Q0 should light before Q3 does: {seen:?}"
        );
        assert!(
            seen.iter().any(|s| s.3),
            "with SER held high the 1 must reach Q3 within five clocks: {seen:?}"
        );
    }

    /// THE /RST LESSON, stated as a test. Cut the one wire that ties the
    /// reset pin to the supply — which is the state every player's first
    /// shift register is in, because nothing made them wire it — and the part
    /// goes completely dead: clock it all you like, every output stays low.
    ///
    /// This is the failure the room exists to explain, so it is asserted
    /// rather than merely written in a label box.
    #[test]
    fn without_the_reset_tie_it_is_dead() {
        let mut room = Room::template("logic");
        let p = shiftreg_pins();
        room.cut(p[4]);
        room.gate_ok();
        let ser_leg = p[3];
        let sx = X0 + DX * 2 - 5 - 2;
        if let Some(sw) = room.switch_at((sx, ser_leg.1)) {
            room.set_switch(sw, true);
        }
        room.run(3.0);
        for (k, q) in [p[5], p[6], p[7], p[8]].iter().enumerate() {
            assert!(
                room.volts(*q) < 2.5,
                "with /RST held low Q{k} must stay low, got {}",
                room.volts(*q)
            );
        }
    }
}
