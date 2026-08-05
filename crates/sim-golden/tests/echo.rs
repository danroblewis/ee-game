//! Does a BBD patched as an ECHO actually repeat? Measured on the solver.
use sim_core::{ElementKind as K, Engine, Wave};
use sim_golden::*;
const DT: f64 = 20e-6;

#[test]
fn a_bbd_with_feedback_repeats() {
    // IN node (0,0) is fed by a short pulse through a mixing resistor, and
    // ALSO by the delay's own output through a feedback resistor. That is
    // the whole echo circuit: one part, two resistors.
    let clk = 8000.0;
    let mut d = vec![
        sim_core::ElementSpec { id: 1, kind: K::Bbd { stages: 512 },
            pins: vec![(0,0),(8,0),(0,8),(8,8)], ..Default::default() },
        // Dry source: a single 5 V pulse, 10 ms wide, via a 10k mixer leg.
        spec(2, K::VoltageSource{dc:0.0,amp:0.0,hz:0.0,phase:0.0,wave:Wave::Sine}, (24,0),(24,8)),
        gnd(3,(24,8)),
        spec(4, r(10_000.0), (24,0),(0,0)),
        // FEEDBACK: output back into the input node. 10k against the 10k
        // mixer leg and the BBD's own 1M tether = a bit under unity.
        spec(5, r(12_000.0), (8,0),(0,0)),
        // Clock.
        spec(6, K::VoltageSource{dc:2.5,amp:2.5,hz:clk,phase:0.0,wave:Wave::Square}, (0,8),(0,24)),
        gnd(7,(0,24)),
        // Output load + the part's ground.
        spec(8, r(100_000.0), (8,0),(8,16)), gnd(9,(8,16)), gnd(10,(8,8)),
    ];
    let mut eng = Engine::new(DT);
    eng.set_elements(&d);
    let delay = 512.0 / (2.0 * clk);           // 32 ms
    // Fire the pulse.
    if let K::VoltageSource{ref mut dc, ..} = d[1].kind { *dc = 5.0; }
    eng.set_elements(&d);
    eng.advance((0.010 / DT) as u32);
    if let K::VoltageSource{ref mut dc, ..} = d[1].kind { *dc = 0.0; }
    eng.set_elements(&d);

    // Watch the output for repeats: count distinct bursts above a threshold.
    let mut peaks = Vec::new();
    let mut above = false;
    let t0 = eng.time();
    for _ in 0..((delay * 5.0) / DT) as u32 {
        eng.advance(1);
        let v = eng.voltage_at((8,0)).unwrap();
        if v > 0.15 && !above { above = true; peaks.push(eng.time() - t0); }
        if v < 0.08 { above = false; }
    }
    println!("echo taps at: {:?} (delay = {:.4} s)", peaks.iter().map(|x| (x*1000.0).round()/1000.0).collect::<Vec<_>>(), delay);
    assert!(peaks.len() >= 3, "expected repeats, got {} taps", peaks.len());
    // Successive taps must be one delay apart.
    for w in peaks.windows(2) {
        let gap = w[1] - w[0];
        assert!((gap - delay).abs() < delay * 0.25,
            "taps {:.4} s apart, delay is {:.4} s", gap, delay);
    }
}
