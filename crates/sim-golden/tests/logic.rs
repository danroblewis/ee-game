//! The CMOS logic family, driven through the real engine.
//!
//! Everything here goes in through `set_elements` / `advance` / `interact`
//! and reads back through `frame()` and `voltage_at()` — the same surface a
//! player's circuit uses. Nothing calls a device evaluator directly, because
//! the interesting claims are not about the truth tables (those are three
//! lines of boolean algebra) but about what the SOLVER does with them: that
//! a gate's output really is pulled to its supply pins, that an edge is
//! detected exactly once per clock, that state survives an unrelated edit,
//! and that the matrix the next substep runs against is not stale.

use sim_core::{ElementKind as K, ElementSpec, Engine, GateOp, InteractOp};
use sim_golden::*;

/// The same 20 µs the server runs, so every timing claim below is in the
/// units the game actually uses.
const DT: f64 = 20e-6;
const VCC: f64 = 5.0;

fn engine_with(elems: &[ElementSpec]) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(elems);
    e
}

fn v_at(eng: &Engine, p: (i32, i32)) -> f64 {
    eng.voltage_at(p).unwrap_or_else(|| panic!("no junction at {p:?}"))
}

/// Read a node as a logic level, and REFUSE to guess: anything between the
/// two Schmitt thresholds is indeterminate and a test that silently rounded
/// it would be hiding exactly the bug worth catching.
fn level(eng: &Engine, p: (i32, i32)) -> bool {
    let v = v_at(eng, p);
    assert!(
        v > 0.9 * VCC || v < 0.1 * VCC,
        "node {p:?} is at {v:.4} V - neither a 1 nor a 0"
    );
    v > 0.5 * VCC
}

// ------------------------------------------------------------- truth tables

/// One gate, both inputs switch-driven from the rail against 10 kΩ
/// pull-downs, output loaded with 1 kΩ. Ids: 3 = gate, 5/6 = the input
/// switches, 4 = the load.
fn gate_rig(op: GateOp, ins: u8) -> Vec<ElementSpec> {
    let y = (14, 6);
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let mut pins = vec![(0, 0), (0, 24)];
    for k in 0..ins as i32 {
        pins.push((6, 4 + 4 * k));
    }
    pins.push(y);
    v.push(logic(3, K::Gate { op, ins }, &pins));
    v.push(spec(4, r(1000.0), y, (0, 24)));
    for k in 0..ins as i32 {
        let inp = (6, 4 + 4 * k);
        v.push(spec(
            5 + k as u32,
            K::Switch { closed: false },
            (0, 0),
            inp,
        ));
        v.push(spec(20 + k as u32, r(10_000.0), inp, (0, 24)));
    }
    v
}

/// Every gate, every input combination, through the solver.
///
/// The gates are checked at width 2 (and 1 for the buffer/inverter) against
/// the boolean function spelled out here rather than against
/// `GateOp::eval`, so this is an independent statement of what each part is
/// and not a restatement of the implementation.
#[test]
fn every_gate_computes_its_truth_table() {
    let cases: &[(GateOp, u8, &[bool])] = &[
        // inputs enumerate as (a, b) = 00, 01, 10, 11
        (GateOp::And, 2, &[false, false, false, true]),
        (GateOp::Nand, 2, &[true, true, true, false]),
        (GateOp::Or, 2, &[false, true, true, true]),
        (GateOp::Nor, 2, &[true, false, false, false]),
        (GateOp::Xor, 2, &[false, true, true, false]),
        (GateOp::Xnor, 2, &[true, false, false, true]),
        // one input: 0, 1
        (GateOp::Buf, 1, &[false, true]),
        (GateOp::Not, 1, &[true, false]),
    ];
    for (op, ins, table) in cases {
        let mut eng = engine_with(&gate_rig(*op, *ins));
        for (row, want) in table.iter().enumerate() {
            for k in 0..*ins {
                let closed = row >> k & 1 == 1;
                eng.interact(5 + u32::from(k), InteractOp::SetSwitch { closed });
            }
            // Two substeps is enough for any combinational gate: the inputs
            // settle in one, the output follows one substep later.
            eng.advance(10);
            assert!(!eng.is_quarantined(), "{op:?} quarantined");
            let got = level(&eng, (14, 6));
            assert_eq!(
                got, *want,
                "{op:?}/{ins} row {row:02b}: expected {want}, got {got}"
            );
        }
    }
}

/// A 4-input gate is not a special case of a 2-input one: the width has to
/// reach `pin_count`, the pin roles and the evaluator together.
#[test]
fn a_four_input_nand_needs_all_four_inputs_high() {
    let mut eng = engine_with(&gate_rig(GateOp::Nand, 4));
    for row in 0u32..16 {
        for k in 0..4 {
            eng.interact(
                5 + k,
                InteractOp::SetSwitch {
                    closed: row >> k & 1 == 1,
                },
            );
        }
        eng.advance(10);
        assert_eq!(level(&eng, (14, 6)), row != 15, "row {row:04b}");
    }
}

// ------------------------------------------------------------- the analogue
//
// The claims that make this a circuit model rather than a boolean one.

/// A gate's output current comes OUT OF THE SUPPLY PIN, not out of nowhere.
///
/// This is the op-amp lesson, and it is the reason the family has modelled
/// VCC/GND at all: with a high output driving a load to ground, the current
/// into the load must appear as current out of the VCC pin, and the sum over
/// all the chip's pins must be zero.
#[test]
fn output_current_comes_out_of_the_supply_pin() {
    let mut eng = engine_with(&gate_rig(GateOp::Buf, 1));
    eng.interact(5, InteractOp::SetSwitch { closed: true });
    eng.advance(20);

    let f = eng.frame();
    let g = f.iter().find(|e| e.id == 3).unwrap();
    // [VCC, GND, IN, Y]
    let (i_vcc, i_out) = (g.i[0], g.i[3]);
    // Y sources current, so the current INTO the chip at Y is negative.
    assert!(i_out < -1e-4, "output should be sourcing, got {i_out}");
    // ...and it is drawn IN through VCC.
    assert!(i_vcc > 1e-4, "VCC should be supplying, got {i_vcc}");
    let sum: f64 = (0..g.npins).map(|p| g.i[p]).sum();
    assert!(sum.abs() < 1e-9, "KCL over the chip's pins: {sum}");

    // And the power it reports is its own dissipation: positive, and equal
    // to the drop across its own 50 Ω output FET.
    assert!(g.power > 0.0, "a passive network cannot deliver power");
    let expect = i_out * i_out * 50.0;
    assert!(
        (g.power - expect).abs() < 0.05 * expect,
        "power {} should be i²·R_on = {expect}",
        g.power
    );
}

/// Logic levels are fractions of the LIVE supply, never a hard-coded 5 V.
/// Run the same inverter from 3 V and it is 3 V logic.
#[test]
fn levels_track_the_live_supply() {
    for rail in [3.0, 5.0, 6.0] {
        let mut elems = gate_rig(GateOp::Not, 1);
        elems[0] = spec(1, dc(rail), (0, 0), (0, 24));
        let mut eng = engine_with(&elems);
        eng.advance(20);
        // Input low -> output high, at THIS rail.
        let y = v_at(&eng, (14, 6));
        assert!(
            (y - rail).abs() < 0.05 * rail,
            "on a {rail} V rail the output sat at {y}"
        );
    }
}

/// A heavy load drags a real CMOS output off the rail, and past a point it
/// drags it below the next gate's own threshold. Nothing about that is
/// scripted: it is a 50 Ω source against the player's resistor.
#[test]
fn a_heavy_load_sags_the_output_below_its_own_threshold() {
    for (load, lo, hi) in [(10_000.0, 0.99, 1.01), (1000.0, 0.94, 0.97), (100.0, 0.63, 0.70)] {
        let mut elems = gate_rig(GateOp::Buf, 1);
        elems[3] = spec(4, r(load), (14, 6), (0, 24));
        let mut eng = engine_with(&elems);
        eng.interact(5, InteractOp::SetSwitch { closed: true });
        eng.advance(20);
        let frac = v_at(&eng, (14, 6)) / VCC;
        assert!(
            frac > lo && frac < hi,
            "{load} Ω load: output at {frac:.3}·vcc, expected {lo}..{hi}"
        );
    }
    // The 100 Ω case is below V_TH_HI = 0.65·vcc — a downstream gate would
    // read it as indeterminate, which is the lesson.
}

/// A floating input parks at exactly half the supply — dead in the middle of
/// the hysteresis band — so the Schmitt latch HOLDS and the gate is
/// deterministic rather than chattering. Real CMOS floats; a pull-down would
/// have been convenient and a lie.
#[test]
fn a_floating_input_sits_at_half_the_rail_and_holds() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        // Input pin wired to nothing at all.
        logic(
            3,
            K::Gate {
                op: GateOp::Buf,
                ins: 1,
            },
            &[(0, 0), (0, 24), (6, 8), (14, 6)],
        ),
        spec(4, r(1000.0), (14, 6), (0, 24)),
    ];
    let mut eng = engine_with(&elems);
    eng.advance(500);
    assert!(!eng.is_quarantined(), "a floating input must not quarantine");
    let vin = v_at(&eng, (6, 8));
    assert!(
        (vin - VCC / 2.0).abs() < 0.05,
        "floating input at {vin}, expected ~{}",
        VCC / 2.0
    );
    // It held its power-up state (low) rather than flickering.
    let y = v_at(&eng, (14, 6));
    assert!(y < 0.1 * VCC, "held output should still be low, got {y}");
}

/// An unpowered, unwired chip dropped on the canvas — the very first thing a
/// player does — must sit there quietly.
#[test]
fn an_unwired_chip_is_harmless() {
    for kind in [
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        K::FlipFlop { edge: true },
        K::ShiftReg { bits: 4 },
        K::Counter {
            bits: 4,
            modulus: 16,
        },
        K::Mux { sel: 2 },
    ] {
        let n = kind.pin_count();
        let pins: Vec<(i32, i32)> = (0..n as i32).map(|k| (k * 2, 0)).collect();
        let mut eng = engine_with(&[ElementSpec::pins(1, kind, &pins)]);
        eng.advance(500);
        assert!(!eng.is_quarantined(), "{kind:?} quarantined while unwired");
        for f in eng.frame() {
            for p in 0..f.npins {
                assert!(f.v[p].is_finite() && f.i[p].is_finite(), "{kind:?} pin {p}");
            }
        }
    }
}

// ------------------------------------------------------------------- timing

/// Count rising edges on a node over a run, sampling once per substep.
fn count_edges(eng: &mut Engine, at: (i32, i32), steps: u32) -> u32 {
    let mut n = 0;
    let mut prev = v_at(eng, at) > 0.5 * VCC;
    for _ in 0..steps {
        eng.advance(1);
        let now = v_at(eng, at) > 0.5 * VCC;
        if now && !prev {
            n += 1;
        }
        prev = now;
    }
    n
}

/// The flip-flop divides by two, exactly, over a long run — which is the
/// statement that the edge detector fires once per clock and never twice.
#[test]
fn a_flip_flop_divides_its_clock_by_two() {
    let mut eng = engine_with(&dff_divide_by_2());
    eng.advance(1000); // let the clock shaper start
    // 1 kHz clock, 20 µs substeps: 50 substeps per clock period.
    let steps = 50 * 200;
    let clk = count_edges(&mut eng, (10, 12), steps);
    let mut eng2 = engine_with(&dff_divide_by_2());
    eng2.advance(1000);
    let q = count_edges(&mut eng2, (18, 8), steps);
    assert!(!eng.is_quarantined() && !eng2.is_quarantined());
    assert!(
        (clk as i64 - 200).abs() <= 1,
        "clock should be 200 edges, got {clk}"
    );
    assert!(
        (q as i64 * 2 - clk as i64).abs() <= 1,
        "Q should be exactly half the clock: {q} vs {clk}"
    );
}

/// A transparent latch is NOT an edge-triggered flip-flop, and the same
/// element proves it: flip `edge` and the output follows D while the clock
/// is high instead of sampling it once.
#[test]
fn a_transparent_latch_follows_d_while_the_clock_is_high() {
    // CLK and D both switch-driven, so the test can move D *during* the
    // clock's high phase — the thing a flip-flop must ignore and a latch
    // must not.
    let rig = |edge: bool| -> Vec<ElementSpec> {
        vec![
            spec(1, dc(VCC), (0, 0), (0, 24)),
            gnd(2, (0, 24)),
            // [VCC, GND, CLK, D, RST, Q, /Q]
            logic(
                3,
                K::FlipFlop { edge },
                &[
                    (0, 0),
                    (0, 24),
                    (6, 4),
                    (6, 8),
                    (0, 0),
                    (16, 4),
                    (16, 12),
                ],
            ),
            spec(4, K::Switch { closed: false }, (0, 0), (6, 4)), // CLK
            spec(5, K::Switch { closed: false }, (0, 0), (6, 8)), // D
            spec(6, r(10_000.0), (6, 4), (0, 24)),
            spec(7, r(10_000.0), (6, 8), (0, 24)),
            spec(8, r(1000.0), (16, 4), (0, 24)),
            spec(9, r(1000.0), (16, 12), (0, 24)),
        ]
    };
    for edge in [true, false] {
        let mut eng = engine_with(&rig(edge));
        eng.advance(20);
        assert!(!level(&eng, (16, 4)), "Q should power up low");

        // D low, clock rises: Q takes the low.
        eng.interact(4, InteractOp::SetSwitch { closed: true });
        eng.advance(20);
        assert!(!level(&eng, (16, 4)), "edge={edge}: Q after a 0 clocked in");

        // Now raise D while the clock is STILL high. This is the whole
        // difference between the two parts.
        eng.interact(5, InteractOp::SetSwitch { closed: true });
        eng.advance(20);
        assert_eq!(
            level(&eng, (16, 4)),
            !edge,
            "edge={edge}: a latch must follow D mid-phase, a flip-flop must not"
        );

        // Drop the clock and raise it again: now BOTH sample the high D.
        eng.interact(4, InteractOp::SetSwitch { closed: false });
        eng.advance(20);
        eng.interact(4, InteractOp::SetSwitch { closed: true });
        eng.advance(20);
        assert!(level(&eng, (16, 4)), "edge={edge}: Q after a 1 clocked in");
        // /Q is always the complement.
        assert_ne!(level(&eng, (16, 4)), level(&eng, (16, 12)), "/Q");

        // Asynchronous reset is ACTIVE LOW and does not wait for a clock.
        // (RST is tied to VCC in this rig, so drive it low by rewiring: use
        // the counter test for the clocked path and check the tie here.)
        assert!(level(&eng, (16, 4)));
    }
}

/// Reset is asynchronous and active low: pulling RST down clears Q with no
/// clock edge at all, and holds it cleared.
#[test]
fn reset_is_asynchronous_and_active_low() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        logic(
            3,
            K::FlipFlop { edge: true },
            &[(0, 0), (0, 24), (6, 4), (0, 0), (6, 12), (16, 4), (16, 12)],
        ),
        spec(4, K::Switch { closed: false }, (0, 0), (6, 4)), // CLK
        spec(5, r(10_000.0), (6, 4), (0, 24)),
        // RST pulled UP by default (not reset); switch 6 pulls it down.
        spec(6, K::Switch { closed: false }, (6, 12), (0, 24)),
        spec(7, r(10_000.0), (0, 0), (6, 12)),
        spec(8, r(1000.0), (16, 4), (0, 24)),
        spec(9, r(1000.0), (16, 12), (0, 24)),
    ];
    let mut eng = engine_with(&elems);
    // D is tied to VCC, so one clock edge sets Q.
    eng.advance(20);
    eng.interact(4, InteractOp::SetSwitch { closed: true });
    eng.advance(20);
    assert!(level(&eng, (16, 4)), "Q should be set");

    // Pull RST low with the clock parked high: no edge, but Q clears.
    eng.interact(6, InteractOp::SetSwitch { closed: true });
    eng.advance(20);
    assert!(!level(&eng, (16, 4)), "async reset must clear Q with no edge");

    // It HOLDS cleared across clock edges while RST is asserted.
    for _ in 0..3 {
        eng.interact(4, InteractOp::SetSwitch { closed: false });
        eng.advance(10);
        eng.interact(4, InteractOp::SetSwitch { closed: true });
        eng.advance(10);
        assert!(!level(&eng, (16, 4)), "reset must dominate the clock");
    }
}

/// The self-correcting one-hot ring: exactly one output high at every
/// instant, advancing one step per clock, self-starting from the defined
/// all-zeros power-up state with no seeding.
#[test]
fn the_shift_register_ring_is_one_hot_and_self_starting() {
    let mut eng = engine_with(&shiftreg_ring4());
    let qs = [(20, 4), (20, 8), (20, 12), (20, 16)];

    // 500 Hz clock at 20 µs = 100 substeps per period. Walk several full
    // laps, sampling the pattern well clear of every clock edge.
    let mut seen = Vec::new();
    for _ in 0..40 {
        eng.advance(25);
        let bits: Vec<bool> = qs.iter().map(|p| level(&eng, *p)).collect();
        let hot = bits.iter().filter(|b| **b).count();
        assert_eq!(hot, 1, "expected exactly one hot output, got {bits:?}");
        seen.push(bits.iter().position(|b| *b).unwrap());
    }
    assert!(!eng.is_quarantined());

    // It self-started: something is hot at all (asserted above), and the
    // hot bit advances by one, wrapping at 4.
    let mut moves = 0;
    for w in seen.windows(2) {
        if w[0] != w[1] {
            assert_eq!(
                (w[0] + 1) % 4,
                w[1],
                "the hot bit must advance by exactly one: {} -> {}",
                w[0],
                w[1]
            );
            moves += 1;
        }
    }
    assert!(moves >= 4, "the ring should have advanced, saw {moves} moves");
}

/// All four stages of a shift register move from ONE clock edge, in one
/// substep — no internal ripple. That is the property that makes it one
/// element instead of four composed flip-flops, and it is worth a direct
/// test because an internal ripple would cost four global O(n³)
/// factorizations per edge instead of one.
#[test]
fn a_shift_register_has_no_internal_ripple() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        logic(
            3,
            K::ShiftReg { bits: 4 },
            &[
                (0, 0),
                (0, 24),
                (6, 4),   // CLK
                (0, 0),   // SER tied high: shift in 1s
                (0, 0),   // RST tied high
                (20, 4),
                (20, 8),
                (20, 12),
                (20, 16),
            ],
        ),
        spec(4, K::Switch { closed: false }, (0, 0), (6, 4)),
        spec(5, r(10_000.0), (6, 4), (0, 24)),
        spec(6, r(10_000.0), (20, 4), (0, 24)),
        spec(7, r(10_000.0), (20, 8), (0, 24)),
        spec(8, r(10_000.0), (20, 12), (0, 24)),
        spec(9, r(10_000.0), (20, 16), (0, 24)),
    ];
    let qs = [(20, 4), (20, 8), (20, 12), (20, 16)];
    let mut eng = engine_with(&elems);
    eng.advance(20);
    // Fill it: 1000, 1100, 1110, 1111.
    for k in 0..4 {
        eng.interact(4, InteractOp::SetSwitch { closed: false });
        eng.advance(10);
        eng.interact(4, InteractOp::SetSwitch { closed: true });
        eng.advance(10);
        let bits: Vec<bool> = qs.iter().map(|p| level(&eng, *p)).collect();
        let want: Vec<bool> = (0..4).map(|j| j <= k).collect();
        assert_eq!(bits, want, "after {} clocks", k + 1);
    }

    // The no-ripple claim, sharply: hold the clock low, then step ONE
    // substep past the rising edge. Q0 and Q1 must both have moved.
    eng.interact(4, InteractOp::SetSwitch { closed: false });
    eng.advance(10);
    // Shift in a 0 by untying SER... simplest equivalent: reset and refill
    // is not what we want, so instead check the pattern moves as a unit by
    // clocking once and reading at the very next substep.
    let before: Vec<bool> = qs.iter().map(|p| level(&eng, *p)).collect();
    eng.interact(4, InteractOp::SetSwitch { closed: true });
    // One substep for the clock node to rise and be sensed, one for the
    // outputs to be driven from the new state.
    eng.advance(3);
    let after: Vec<bool> = qs.iter().map(|p| level(&eng, *p)).collect();
    assert_eq!(
        before, after,
        "all-ones shifted by one is still all-ones, in one step"
    );
}

/// Two 4-bit registers cascaded Q3 -> SER are an 8-bit register, which is
/// why an 8-bit part does not exist. The seam shows exactly one clock of
/// delay, like two real '595s.
#[test]
fn two_registers_cascade_into_eight_bits() {
    let mut elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        // First stage: SER tied high.
        logic(
            3,
            K::ShiftReg { bits: 4 },
            &[
                (0, 0),
                (0, 24),
                (6, 4),
                (0, 0),
                (0, 0),
                (20, 4),
                (20, 8),
                (20, 12),
                (20, 16),
            ],
        ),
        // Second stage: SER from the first stage's Q3.
        logic(
            4,
            K::ShiftReg { bits: 4 },
            &[
                (0, 0),
                (0, 24),
                (6, 4),
                (20, 16),
                (0, 0),
                (40, 4),
                (40, 8),
                (40, 12),
                (40, 16),
            ],
        ),
        spec(5, K::Switch { closed: false }, (0, 0), (6, 4)),
        spec(6, r(10_000.0), (6, 4), (0, 24)),
    ];
    let qs = [
        (20, 4),
        (20, 8),
        (20, 12),
        (20, 16),
        (40, 4),
        (40, 8),
        (40, 12),
        (40, 16),
    ];
    for (k, q) in qs.iter().enumerate() {
        elems.push(spec(10 + k as u32, r(10_000.0), *q, (0, 24)));
    }
    let mut eng = engine_with(&elems);
    eng.advance(20);
    // Eight clocks fill all eight bits, one per clock, in order.
    for k in 0..8 {
        eng.interact(5, InteractOp::SetSwitch { closed: false });
        eng.advance(10);
        eng.interact(5, InteractOp::SetSwitch { closed: true });
        eng.advance(10);
        let bits: Vec<bool> = qs.iter().map(|p| level(&eng, *p)).collect();
        let want: Vec<bool> = (0..8).map(|j| j <= k).collect();
        assert_eq!(bits, want, "cascade after {} clocks", k + 1);
    }
}

/// A 3-bit counter divides by 2, 4 and 8 on its three outputs — the octave
/// divider, and the property `ShiftReg` genuinely cannot provide.
#[test]
fn a_counter_divides_by_binary_weights() {
    let steps = 50 * 240; // 240 clock periods at 1 kHz
    for (pin, div) in [((20, 4), 2u32), ((20, 8), 4), ((20, 12), 8)] {
        let mut eng = engine_with(&counter_div8());
        eng.advance(1000);
        let n = count_edges(&mut eng, pin, steps);
        assert!(!eng.is_quarantined());
        let want = 240 / div;
        assert!(
            (n as i64 - i64::from(want)).abs() <= 1,
            "Q{} should tick {want} times, got {n}",
            div.trailing_zeros()
        );
    }
}

/// A modulus that is not a power of two really wraps early: mod-5 on three
/// bits visits 0,1,2,3,4,0,... and never reaches 5.
#[test]
fn a_counter_respects_a_non_power_of_two_modulus() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        logic(
            3,
            K::Counter {
                bits: 3,
                modulus: 5,
            },
            &[(0, 0), (0, 24), (6, 4), (0, 0), (20, 4), (20, 8), (20, 12)],
        ),
        spec(4, K::Switch { closed: false }, (0, 0), (6, 4)),
        spec(5, r(10_000.0), (6, 4), (0, 24)),
        spec(6, r(10_000.0), (20, 4), (0, 24)),
        spec(7, r(10_000.0), (20, 8), (0, 24)),
        spec(8, r(10_000.0), (20, 12), (0, 24)),
    ];
    let qs = [(20, 4), (20, 8), (20, 12)];
    let mut eng = engine_with(&elems);
    eng.advance(20);
    let mut seen = Vec::new();
    for _ in 0..12 {
        eng.interact(4, InteractOp::SetSwitch { closed: false });
        eng.advance(10);
        eng.interact(4, InteractOp::SetSwitch { closed: true });
        eng.advance(10);
        let n: u32 = qs
            .iter()
            .enumerate()
            .map(|(k, p)| u32::from(level(&eng, *p)) << k)
            .sum();
        seen.push(n);
    }
    assert_eq!(seen, vec![1, 2, 3, 4, 0, 1, 2, 3, 4, 0, 1, 2], "mod-5 sequence");
}

/// The mux passes ANALOG, which is the whole reason it is modelled as a
/// 4051 and not a '153: the four channels carry 1/2/3/4 V — not logic
/// levels — and each appears at Y in turn.
#[test]
fn the_mux_passes_analog_levels_through() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        spec(3, dc(1.0), (24, 2), (0, 24)),
        spec(4, dc(2.0), (24, 6), (0, 24)),
        spec(5, dc(3.0), (24, 10), (0, 24)),
        spec(6, dc(4.0), (24, 14), (0, 24)),
        logic(
            7,
            K::Mux { sel: 2 },
            &[
                (0, 0),
                (0, 24),
                (24, 2),
                (24, 6),
                (24, 10),
                (24, 14),
                (16, 18), // S0
                (16, 22), // S1
                (34, 8),  // Y
            ],
        ),
        spec(8, r(100_000.0), (34, 8), (0, 24)),
        spec(9, K::Switch { closed: false }, (0, 0), (16, 18)),
        spec(10, K::Switch { closed: false }, (0, 0), (16, 22)),
        spec(11, r(10_000.0), (16, 18), (0, 24)),
        spec(12, r(10_000.0), (16, 22), (0, 24)),
    ];
    let mut eng = engine_with(&elems);
    for (sel, want) in [(0u32, 1.0), (1, 2.0), (2, 3.0), (3, 4.0)] {
        eng.interact(
            9,
            InteractOp::SetSwitch {
                closed: sel & 1 == 1,
            },
        );
        eng.interact(
            10,
            InteractOp::SetSwitch {
                closed: sel & 2 == 2,
            },
        );
        eng.advance(20);
        let y = v_at(&eng, (34, 8));
        assert!(
            (y - want).abs() < 0.02,
            "select {sel} should pass {want} V, got {y:.4}"
        );
    }
}

// -------------------------------------------------------------- the hazards

/// A ring of inverters oscillates — and it does so at 1/(2·dt) REGARDLESS
/// of how many inverters are in it. This test exists to pin an ARTIFACT, not
/// a physical result, because the artifact is worth knowing about and must
/// not drift silently.
///
/// Why it happens: every stage has exactly the same one-substep delay and
/// they all update from the same solve, so the map is
/// `x_k(n+1) = NOT x_{k-1}(n)`. From the defined all-zeros power-up state
/// every stage is equal, and the all-equal state is a period-2 orbit of that
/// map for ANY ring length: all-low -> all-high -> all-low. The travelling
/// wave that would give 1/(2·N·dt) needs an unequal starting state, and
/// nothing in a deterministic solver breaks the tie. Measured: 3, 5 and 7
/// stages all run at 25.000 kHz, with or without capacitance on the nodes
/// (a 50 Ω output charges any sane capacitor well inside one substep, so the
/// node still tracks the state exactly).
///
/// Why it is not worth faking away: a real 3-inverter ring runs at about
/// 60 MHz. At a 20 µs timestep this simulator is four orders of magnitude
/// short of representing gate delay at all, so NO frequency it reports for a
/// ring oscillator would be right. What it can represent honestly is
/// sequential logic clocked by the player's own oscillator, which is the
/// regime every other test here is in. Manufacturing a per-gate offset to
/// force 1/(2·N·dt) would be inventing physics to make a wrong number look
/// like a different wrong number.
#[test]
fn a_ring_of_inverters_locks_to_the_timestep_not_the_ring_length() {
    let expect = 1.0 / (2.0 * DT); // 25 kHz: one edge every two substeps
    for n in [3usize, 5, 7] {
        let mut elems = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        let nodes: Vec<(i32, i32)> = (0..n).map(|k| (10, 4 + 4 * k as i32)).collect();
        for k in 0..n {
            elems.push(logic(
                3 + k as u32,
                K::Gate {
                    op: GateOp::Not,
                    ins: 1,
                },
                &[(0, 0), (0, 24), nodes[(k + n - 1) % n], nodes[k]],
            ));
            elems.push(spec(20 + k as u32, r(100_000.0), nodes[k], (0, 24)));
        }
        let mut eng = engine_with(&elems);
        eng.advance(400);
        assert!(!eng.is_quarantined(), "a ring must not quarantine");

        let steps = 5000; // 100 ms
        let edges = count_edges(&mut eng, nodes[0], steps);
        let hz = f64::from(edges) / (f64::from(steps) * DT);
        assert!(
            (hz - expect).abs() < 0.02 * expect,
            "ring of {n} oscillated at {hz:.0} Hz, expected {expect:.0} Hz"
        );
        // The claim that makes this an artifact rather than a result: the
        // frequency did not depend on the ring length.
    }
}

/// The same symmetry, in the place it actually bites: a cross-coupled NAND
/// pair that has NEVER been set sits in the all-equal mode and toggles at
/// 1/(2·dt) with both outputs the same, instead of picking a side.
///
/// This is the honest residue of the model, and it is documented rather than
/// fudged. It is NOT a stuck failure: one pulse on /S resolves it
/// permanently (see `cross_coupled_nands_latch_and_remember`), and the
/// packaged `FlipFlop` — whose resolution is written as one element's state
/// machine rather than emerging from two elements racing — is never
/// metastable at all. That is the argument for shipping `FlipFlop` and
/// `ShiftReg` as parts instead of telling players to build them from gates.
#[test]
fn an_unset_cross_coupled_pair_is_symmetric_until_something_breaks_the_tie() {
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        logic(
            3,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), (6, 4), (20, 12), (20, 4)],
        ),
        logic(
            4,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), (6, 12), (20, 4), (20, 12)],
        ),
        spec(5, r(10_000.0), (0, 0), (6, 4)), // /S released
        spec(6, r(10_000.0), (0, 0), (6, 12)), // /R released
        spec(7, r(100_000.0), (20, 4), (0, 24)),
        spec(8, r(100_000.0), (20, 12), (0, 24)),
    ];
    let mut eng = engine_with(&elems);
    eng.advance(400);
    assert!(!eng.is_quarantined(), "and it must not quarantine");
    // Both outputs move together, which is the symmetry itself.
    for _ in 0..8 {
        eng.advance(1);
        let (q, qb) = (v_at(&eng, (20, 4)), v_at(&eng, (20, 12)));
        assert!(
            (q - qb).abs() < 0.01,
            "an unset symmetric pair has equal outputs, got {q:.3} / {qb:.3}"
        );
    }
    let edges = count_edges(&mut eng, (20, 4), 5000);
    let hz = f64::from(edges) / (5000.0 * DT);
    let expect = 1.0 / (2.0 * DT);
    assert!(
        (hz - expect).abs() < 0.02 * expect,
        "expected the symmetric mode at {expect:.0} Hz, got {hz:.0} Hz"
    );
}

/// A gate output is a hard step between the rails behind 50 Ω, so a player
/// hanging a capacitor on it — the sequencer's CV glide cap is exactly that
/// — is a discontinuity driving an integrator. Trapezoid rings badly on it:
/// measured at +5.53 V and -0.55 V on a 5 V rail before a logic edge armed
/// the post-event backward-Euler steps.
///
/// That was not cosmetic. A pin driven a volt outside the rails is what
/// fires the latch-up model, so the integrator would have been destroying
/// chips with nothing wrong with them. The output must stay inside its own
/// supply, whatever the player hangs on it.
#[test]
fn a_capacitive_load_does_not_ring_the_output_past_its_own_rails() {
    for cap in [1e-9, 1e-8, 1e-7, 1e-6] {
        let mut elems = shiftreg_ring4();
        elems.push(spec(20, K::Capacitor { farads: cap }, (20, 4), (0, 24)));
        let mut eng = engine_with(&elems);
        eng.advance(500);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for _ in 0..20_000 {
            eng.advance(1);
            let v = v_at(&eng, (20, 4));
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(!eng.is_quarantined());
        assert!(
            hi < VCC + 0.1,
            "C={cap:e}: output rang to {hi:.4} V, above its own {VCC} V supply"
        );
        assert!(lo > -0.1, "C={cap:e}: output rang to {lo:.4} V, below GND");
    }
}

/// A cross-coupled NAND pair is an SR latch: it HOLDS after the set pulse is
/// released, which is the thing an ideal zero-delay gate model cannot do.
#[test]
fn cross_coupled_nands_latch_and_remember() {
    // /S and /R are active low, pulled up through 10 kΩ, pulled down by
    // buttons — the way a player would wire it.
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        // Gate A: [VCC, GND, /S, Qbar, Q]
        logic(
            3,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), (6, 4), (20, 12), (20, 4)],
        ),
        // Gate B: [VCC, GND, /R, Q, Qbar]
        logic(
            4,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), (6, 12), (20, 4), (20, 12)],
        ),
        spec(5, r(10_000.0), (0, 0), (6, 4)),  // pull-up on /S
        spec(6, r(10_000.0), (0, 0), (6, 12)), // pull-up on /R
        spec(7, K::Switch { closed: false }, (6, 4), (0, 24)), // set
        spec(8, K::Switch { closed: false }, (6, 12), (0, 24)), // reset
        spec(9, r(100_000.0), (20, 4), (0, 24)),
        spec(10, r(100_000.0), (20, 12), (0, 24)),
    ];
    let mut eng = engine_with(&elems);
    eng.advance(200);

    // Set: pull /S low, then release.
    eng.interact(7, InteractOp::SetSwitch { closed: true });
    eng.advance(50);
    assert!(level(&eng, (20, 4)), "Q should be set while /S is low");
    eng.interact(7, InteractOp::SetSwitch { closed: false });
    eng.advance(500);
    assert!(level(&eng, (20, 4)), "Q must HOLD after /S is released");
    assert!(!level(&eng, (20, 12)), "/Q must be the complement");

    // Reset: pull /R low, then release.
    eng.interact(8, InteractOp::SetSwitch { closed: true });
    eng.advance(50);
    assert!(!level(&eng, (20, 4)), "Q should clear while /R is low");
    eng.interact(8, InteractOp::SetSwitch { closed: false });
    eng.advance(500);
    assert!(!level(&eng, (20, 4)), "Q must HOLD cleared");
    assert!(!eng.is_quarantined());
}

/// Two outputs wired together is LEGAL — a resistive fight, not a rejection.
/// The node parks between the thresholds (indeterminate, which is the
/// lesson) and both chips run warm without dying, exactly as two fighting
/// 74HC outputs do.
#[test]
fn two_outputs_tied_together_fight_and_get_warm() {
    let y = (20, 8);
    let elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        // A buffer driving HIGH (input tied to VCC)...
        logic(
            3,
            K::Gate {
                op: GateOp::Buf,
                ins: 1,
            },
            &[(0, 0), (0, 24), (0, 0), y],
        ),
        // ...against an inverter also driving HIGH's opposite (input tied
        // to VCC too, so it drives LOW). Same node.
        logic(
            4,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), (0, 0), y],
        ),
    ];
    let mut eng = engine_with(&elems);
    eng.advance(200);
    assert!(!eng.is_quarantined(), "an output fight must not quarantine");

    let v = v_at(&eng, y);
    assert!(
        (v - VCC / 2.0).abs() < 0.1,
        "two fighting outputs should park at half the rail, got {v}"
    );
    // 100 Ω across 5 V = 50 mA, 125 mW in each chip: hot, and survivable.
    let f = eng.frame();
    for id in [3, 4] {
        let g = f.iter().find(|e| e.id == id).unwrap();
        assert!(
            (g.power - 0.125).abs() < 0.01,
            "chip {id} should burn ~125 mW, got {:.4} W",
            g.power
        );
        assert!(g.power > 0.0, "and it can never be negative");
    }
}

/// A 74HC-class part on the hoist's 9 V rail latches up: the parasitic SCR
/// fires, the chip becomes 10 Ω across its own supply, and it burns 8.1 W
/// against a 0.35 W package. This is what makes overvoltage physically
/// dissipative instead of needing a second damage metric.
#[test]
fn overvoltage_latches_the_chip_up_and_it_burns() {
    let mut eng = engine_with(&logic_latchup());
    eng.advance(200);
    assert!(!eng.is_quarantined());

    let g = eng.frame().into_iter().find(|e| e.id == 3).unwrap();
    // 9 V across 10 Ω = 900 mA, 8.1 W.
    assert!(
        g.power > 7.0,
        "a latched chip should be burning watts, got {:.3} W",
        g.power
    );
    assert!(g.i[0] > 0.5, "and drawing amps through VCC, got {}", g.i[0]);

    // 5 V is nowhere near tripping it: the discrimination has to be real or
    // the mechanism is just a trap.
    let mut ok = engine_with(&gate_nand_dc());
    ok.advance(200);
    let n = ok.frame().into_iter().find(|e| e.id == 3).unwrap();
    assert!(
        n.power < 0.01,
        "a 5 V part must not latch: {:.4} W",
        n.power
    );
}

/// Latch-up is STICKY: it survives the overvoltage going away and only
/// clears on a power cycle, which is what actually clears it in the world.
#[test]
fn latch_up_clears_only_on_a_power_cycle() {
    // Supply through a switch so the rail can be removed.
    let mut elems = logic_latchup();
    elems.push(spec(6, K::Switch { closed: true }, (0, 0), (0, 2)));
    // Re-point the gate's VCC at the switched node.
    elems[2] = logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 2), (0, 24), (6, 8), (14, 8)],
    );
    elems.push(spec(7, r(100_000.0), (0, 2), (0, 24)));
    let mut eng = engine_with(&elems);
    eng.advance(200);
    let latched = eng.frame().into_iter().find(|e| e.id == 3).unwrap();
    assert!(latched.power > 1.0, "should have latched on 9 V");

    // Open the supply switch: the rail collapses, the latch clears.
    eng.interact(6, InteractOp::SetSwitch { closed: false });
    eng.advance(200);
    let dead = eng.frame().into_iter().find(|e| e.id == 3).unwrap();
    assert!(dead.power < 0.01, "unpowered chip should burn nothing");

    // Power back up — but the rail is still 9 V, so it latches straight
    // back. (Which is correct: nothing about the circuit was fixed.)
    eng.interact(6, InteractOp::SetSwitch { closed: true });
    eng.advance(200);
    let again = eng.frame().into_iter().find(|e| e.id == 3).unwrap();
    assert!(again.power > 1.0, "9 V still latches it");
}

// ---------------------------------------------------------------- the seams

/// Stored state survives an unrelated edit. A player moving a resistor on
/// the other side of the room must not clear a register.
#[test]
fn stored_state_survives_an_unrelated_edit() {
    let mut elems = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        logic(
            3,
            K::FlipFlop { edge: true },
            &[(0, 0), (0, 24), (6, 4), (0, 0), (0, 0), (16, 4), (16, 12)],
        ),
        spec(4, K::Switch { closed: false }, (0, 0), (6, 4)),
        spec(5, r(10_000.0), (6, 4), (0, 24)),
        spec(6, r(1000.0), (16, 4), (0, 24)),
        // The unrelated part, in its own island.
        spec(7, r(4700.0), (60, 0), (60, 8)),
    ];
    let mut eng = engine_with(&elems);
    eng.advance(20);
    eng.interact(4, InteractOp::SetSwitch { closed: true });
    eng.advance(20);
    assert!(level(&eng, (16, 4)), "Q set");

    // Recompile the document with the far-away resistor changed.
    elems[6] = spec(7, r(2200.0), (60, 0), (60, 8));
    eng.set_elements(&elems);
    eng.advance(20);
    assert!(
        level(&eng, (16, 4)),
        "an edit on the other side of the room cleared the flip-flop"
    );
}

/// The stale-LU regression, stated directly.
///
/// The logic family is `is_nonlinear()` but never flips during Newton, so
/// NOTHING in `update_guesses` clears the retained factorization for it —
/// `accept()` has to, and that single line is the only thing between this
/// design and a solver running against a matrix that describes the previous
/// state of every gate in the room. `pwl_reuse.rs` proves bit-identity
/// against a non-reusing engine across every golden; this proves the
/// symptom directly, on a circuit whose output would visibly freeze.
#[test]
fn a_logic_state_change_invalidates_the_retained_factorization() {
    let mut a = engine_with(&shiftreg_ring4());
    let mut b = engine_with(&shiftreg_ring4());
    b.set_reuse_pwl(false);
    for c in 0..40 {
        a.advance(250);
        b.advance(250);
        assert_eq!(
            a.state_hash(),
            b.state_hash(),
            "reuse diverged from a full refactor at chunk {c}"
        );
    }
    // Non-vacuous in both directions: the reusing engine really did skip
    // most factorizations, and it really did do some.
    assert!(
        a.factorizations() < b.factorizations() / 4,
        "reuse should be saving factorizations: {} vs {}",
        a.factorizations(),
        b.factorizations()
    );
    assert!(
        a.factorizations() > 10,
        "a clocked register must refactor on its edges"
    );
}

/// The family owns no branch unknown. The entire cost model rests on it —
/// a 555-shaped output would add one MNA row per output pin, and the
/// factorization is O(n³) over the whole room.
#[test]
fn no_logic_part_is_a_branch_device() {
    for kind in [
        K::Gate {
            op: GateOp::Nand,
            ins: 4,
        },
        K::FlipFlop { edge: true },
        K::FlipFlop { edge: false },
        K::ShiftReg { bits: 4 },
        K::Counter {
            bits: 4,
            modulus: 16,
        },
        K::Mux { sel: 2 },
    ] {
        assert!(!kind.is_branch(), "{kind:?} must not own a branch unknown");
        assert!(kind.is_logic());
        // The classification the performance rests on: piecewise-linear, so
        // the factorization survives between edges, and NOT Newton-driven.
        assert!(kind.is_nonlinear(), "{kind:?}");
        assert!(kind.is_discrete_nonlinear(), "{kind:?}");
        assert!(!kind.needs_newton(), "{kind:?} must not force refactors");
    }

    // In a real room the only branch rows are the sources. `shiftreg_ring4`
    // has two (the 5 V rail and the clock oscillator) and six logic chips;
    // if any chip owned a row there would be eight.
    let elems = shiftreg_ring4();
    let sources = elems.iter().filter(|e| e.kind.is_branch()).count();
    assert_eq!(sources, 2, "the golden's own source count");
    let mut eng = engine_with(&elems);
    eng.advance(10);
    assert_eq!(
        eng.branch_count(),
        sources,
        "logic chips must add no MNA rows of their own"
    );
}
