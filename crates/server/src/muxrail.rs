//! WHY A LOGIC CHIP EXPLODES WHEN YOU RE-SUPPLY IT — and why it usually
//! should not.
//!
//! Daniel was tidying the TR-808: the 5 V logic supply is dragged around the
//! schematic on wires, and a single-pin rail at each chip would read better.
//! Doing that to a Mux destroyed it, "even though the voltage should be the
//! same". He was right that it should be, and right to be suspicious.
//!
//! Both halves are asserted here because the interesting answer is that the
//! first one is NOT the cause:
//!
//!   * swapping the supply wire for an equivalent rail changes NOTHING —
//!     same power, same behaviour, to four decimals. Tidying is safe;
//!   * feeding the same chip 9 V destroys it, because `LOGIC_V_ABSMAX` is
//!     7 V and CMOS above it latches up: an SCR fires and shorts VCC to GND
//!     through 10 ohms, which is 8.1 W the package cannot survive.
//!
//! And 9 V is not a strange thing to have reached for. The Battery part
//! DEFAULTS to 9 V while the V Rail part defaults to 5 V, and this room
//! carries a real 9 V analog supply alongside its 5 V logic one. Grabbing
//! the wrong one looks identical on the canvas.

#![cfg(test)]

use crate::e2e::Room;
use sim_core::{ElementKind as K, ElementSpec};

/// Feed the first Mux from its own single-pin rail at `volts`, with the old
/// supply wiring to that pin removed. Returns the chip's worst dissipation.
fn mux_on_its_own_rail(volts: f64) -> f64 {
    let r = Room::template("tr-808");
    let (mid, vcc) = r
        .elements
        .iter()
        .find(|e| matches!(e.kind, K::Mux { .. }))
        .map(|e| (e.id, e.pins[0]))
        .expect("the 808 has a mux");
    let mut d: Vec<ElementSpec> = r.elements.clone();
    d.retain(|e| !(matches!(e.kind, K::Wire) && e.pins.contains(&vcc)));
    d.push(ElementSpec {
        id: 900_001,
        kind: K::Rail { dc: volts, amp: 0.0, hz: 0.0, phase: 0.0, wave: sim_core::Wave::Sine },
        pins: vec![vcc],
        ..Default::default()
    });
    assert_eq!(
        sim_core::check_document(&d, crate::DT),
        Ok(()),
        "{volts} V: the gate should accept this — it is an ordinary circuit"
    );
    let mut eng = sim_core::Engine::new(crate::DT);
    eng.set_elements(&d);
    let mut worst = 0.0f64;
    for _ in 0..30 {
        eng.advance(2_000);
        for f in eng.frame() {
            if f.id == mid {
                worst = worst.max(f.power);
            }
        }
    }
    assert!(!eng.is_quarantined(), "{volts} V quarantined the room");
    worst
}

/// TIDYING IS SAFE. A rail at the pin is the same circuit as a wire to the
/// shared rail, and must behave identically — otherwise the schematic cannot
/// be cleaned up at all.
#[test]
fn a_rail_at_the_pin_is_the_same_as_a_wire_to_the_rail() {
    let mut base = Room::template("tr-808");
    base.run(1.0);
    let tidied = mux_on_its_own_rail(5.0);
    // The shipped room's mux, for comparison.
    let r = Room::template("tr-808");
    let mid = r
        .elements
        .iter()
        .find(|e| matches!(e.kind, K::Mux { .. }))
        .unwrap()
        .id;
    let mut eng = sim_core::Engine::new(crate::DT);
    eng.set_elements(&r.elements);
    let mut shipped = 0.0f64;
    for _ in 0..30 {
        eng.advance(2_000);
        for f in eng.frame() {
            if f.id == mid {
                shipped = shipped.max(f.power);
            }
        }
    }
    println!("  wired {shipped:.6} W   own rail {tidied:.6} W");
    assert!(
        (tidied - shipped).abs() < 1e-6,
        "re-supplying a chip from its own rail must change nothing: \
         {shipped:.6} W wired vs {tidied:.6} W on its own rail"
    );
}

/// AND THE THING THAT ACTUALLY DESTROYS IT. 7 V is the limit; the Battery
/// part defaults to 9.
#[test]
fn nine_volts_latches_a_logic_chip_and_five_does_not() {
    let ok = mux_on_its_own_rail(5.0);
    let edge = mux_on_its_own_rail(6.0);
    let dead = mux_on_its_own_rail(9.0);
    println!("  5 V {ok:.4} W   6 V {edge:.4} W   9 V {dead:.4} W");
    assert!(ok < 0.1 && edge < 0.1, "at or under the limit a mux idles");
    // 9^2 / 10 ohms = 8.1 W through the latch-up SCR.
    assert!(
        dead > 5.0,
        "9 V should latch the chip up and cook it, got {dead:.4} W"
    );
}
