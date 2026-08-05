//! A PT2399 wired the way the datasheet's own application circuit wires one,
//! and the proof that it echoes.
//!
//! This exists because the part was UNUSABLE before it did. Every plausible
//! naive wiring produced silence, and the reasons were all mine: the delay
//! output sits at whatever DC you feed it while the op-amps are referred to
//! the chip's half-supply, so a signal centred on 0 V drives them straight
//! into their rails; and the repeats path cannot sum into the delay's input
//! pin, because a source drives that node and an ideal source fixes it. It
//! has to sum into an op-amp's VIRTUAL GROUND, which is exactly what OP1 is
//! there for and exactly what the datasheet's circuit does with it.
//!
//! So the topology is not decoration. OP1 mixes dry with feedback, the delay
//! sits in the middle, OP2 is the output stage, and the repeats go from
//! OP2-OUT back to OP1's summing junction. A player who builds this has
//! built the real thing.

use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;
const DT: f64 = 20e-6;

#[test]
fn the_echo_recipe() {
    let mk = |dc: f64| vec![
        sim_core::ElementSpec { id: 1, kind: K::Pt2399,
            pins: vec![(0,0),(10,0),(0,2),(10,2),(0,5),(10,5),(0,7),(10,7)],
            ..Default::default() },
        spec(2, r(10_000.0), (0,2),(0,24)), gnd(3,(0,24)),      // delay time
        gnd(4,(10,2)),                                           // chip ground
        // Dry in, through a resistor into OP1's summing junction.
        spec(5, K::VoltageSource{dc,amp:0.0,hz:0.0,phase:0.0,wave:Wave::Sine}, (30,5),(30,20)),
        gnd(6,(30,20)),
        spec(7, r(10_000.0), (30,5),(0,5)),
        spec(8, r(10_000.0), (0,5),(10,5)),                      // OP1 feedback
        spec(9, K::Wire, (10,5),(0,0)),                          // OP1-OUT -> delay IN
        spec(10, r(10_000.0), (10,0),(0,7)),                     // delay OUT -> OP2
        spec(11, r(10_000.0), (0,7),(10,7)),                     // OP2 feedback
        spec(12, r(15_000.0), (10,7),(0,5)),                     // REPEATS -> OP1 junction
        spec(13, K::Capacitor{farads:220e-6}, (10,7),(14,7)),
        spec(14, K::Speaker{ohms:8.0}, (14,7),(14,20)), gnd(15,(14,20)),
    ];
    let mut d = mk(2.5);
    println!("gate: {:?}", sim_core::check_document(&d, DT));
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    eng.advance(20_000);
    // A 20 ms pulse, then listen for repeats.
    d = mk(4.0);
    eng.set_elements(&d);
    eng.advance(1_000);
    d = mk(2.5);
    eng.set_elements(&d);
    let t0 = eng.time();
    let (mut taps, mut above) = (Vec::new(), false);
    for _ in 0..60_000 {
        eng.advance(1);
        let v = eng.voltage_at((14,7)).unwrap();
        if v.abs() > 0.10 && !above { above = true; taps.push(((eng.time()-t0)*1000.0).round()); }
        if v.abs() < 0.04 { above = false; }
    }
    println!("quarantined={} echo taps at ms: {taps:?}", eng.is_quarantined());
    assert!(!eng.is_quarantined(), "the documented recipe must not quarantine a room");
    // Distinct repeats, spaced by the delay a 10 kΩ VCO resistor sets.
    // Grouped because each pulse EDGE crosses the threshold, so the test
    // looks at the gaps between groups rather than counting taps.
    let mut groups: Vec<f64> = Vec::new();
    for t in &taps {
        if groups.last().map_or(true, |g| t - g > 60.0) {
            groups.push(*t);
        }
    }
    assert!(
        groups.len() >= 4,
        "expected several repeats, got groups {groups:?} from taps {taps:?}"
    );
    for w in groups.windows(2).skip(1) {
        let gap = w[1] - w[0];
        assert!(
            (gap - 145.0).abs() < 40.0,
            "repeats should be one delay apart (~145 ms), got {gap} ms from {groups:?}"
        );
    }
}
