//! ADVERSARIAL verification of the CMOS logic family. Independent of the
//! author's own `logic.rs`: every expectation here is spelled out from first
//! principles, and the accident cases are the ones a PLAYER creates, not the
//! ones a designer plans for.

use sim_core::{ElementKind as K, ElementSpec, Engine, GateOp, InteractOp};
use sim_golden::*;

const DT: f64 = 20e-6;
const VCC: f64 = 5.0;

fn eng_with(elems: &[ElementSpec]) -> Engine {
    let mut e = Engine::new(DT);
    e.set_elements(elems);
    e
}

fn v_at(e: &Engine, p: (i32, i32)) -> f64 {
    e.voltage_at(p).unwrap_or_else(|| panic!("no junction {p:?}"))
}

fn frame_of(e: &Engine, id: u32) -> sim_core::ElemFrame {
    *e.frame().iter().find(|f| f.id == id).expect("id in frame")
}

// ============================================================ 1. TRUTH TABLES

/// An INDEPENDENT statement of what each gate is. Deliberately not
/// `GateOp::eval`.
fn reference(op: GateOp, bits: &[bool]) -> bool {
    let n = bits.iter().filter(|b| **b).count();
    let all = bits.len();
    match op {
        GateOp::And => n == all,
        GateOp::Nand => n != all,
        GateOp::Or => n >= 1,
        GateOp::Nor => n == 0,
        GateOp::Xor => n % 2 == 1,
        GateOp::Xnor => n % 2 == 0,
        GateOp::Buf => bits[0],
        GateOp::Not => !bits[0],
    }
}

/// gate id 3, switches 10+k, output at (14,6), 1 k load.
fn rig(op: GateOp, ins: u8) -> Vec<ElementSpec> {
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
        v.push(spec(10 + k as u32, K::Switch { closed: false }, (0, 0), inp));
        v.push(spec(30 + k as u32, r(10_000.0), inp, (0, 24)));
    }
    v
}

#[test]
fn adv_every_gate_every_width_every_row() {
    let ops = [
        GateOp::And,
        GateOp::Nand,
        GateOp::Or,
        GateOp::Nor,
        GateOp::Xor,
        GateOp::Xnor,
        GateOp::Buf,
        GateOp::Not,
    ];
    for op in ops {
        let widths: &[u8] = if op.fixed_ins().is_some() {
            &[1]
        } else {
            &[1, 2, 3, 4]
        };
        for &ins in widths {
            let mut e = eng_with(&rig(op, ins));
            for row in 0u32..(1 << ins) {
                let bits: Vec<bool> = (0..ins).map(|k| row >> k & 1 == 1).collect();
                for (k, b) in bits.iter().enumerate() {
                    e.interact(10 + k as u32, InteractOp::SetSwitch { closed: *b });
                }
                e.advance(20);
                assert!(!e.is_quarantined(), "{op:?}/{ins} row {row} quarantined");
                let vy = v_at(&e, (14, 6));
                let want = reference(op, &bits);
                // Refuse to round: a real 1 or a real 0 or the test fails.
                assert!(
                    vy > 0.9 * VCC || vy < 0.1 * VCC,
                    "{op:?}/{ins} row {row:b}: Y = {vy:.4} V, indeterminate"
                );
                assert_eq!(vy > 2.5, want, "{op:?}/{ins} row {row:b}: Y={vy:.4}");
            }
        }
    }
}

// ====================================================== 2. PLAYER ACCIDENTS

/// A completely unconnected input. Real CMOS floats; this model parks it at
/// vcc/2 by a symmetric leak, which sits inside the hysteresis band so the
/// Schmitt latch holds. Assert BOTH halves: the voltage, and that the gate
/// is stable rather than chattering.
#[test]
fn adv_floating_input_parks_midrail_and_holds() {
    // 2-input NAND, input A driven low, input B wired to nothing at all.
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        &[(0, 0), (0, 24), (6, 4), (6, 8), (14, 6)],
    ));
    v.push(spec(4, r(1000.0), (14, 6), (0, 24)));
    v.push(spec(5, r(10_000.0), (6, 4), (0, 24))); // A low
    let mut e = eng_with(&v);
    e.advance(50);
    let vb = v_at(&e, (6, 8));
    assert!(
        (vb - VCC / 2.0).abs() < 0.01,
        "floating input at {vb:.4} V, expected vcc/2"
    );
    // And it holds: sample the output over 2000 substeps, no change.
    let y0 = v_at(&e, (14, 6));
    for _ in 0..100 {
        e.advance(20);
        assert!(
            (v_at(&e, (14, 6)) - y0).abs() < 1e-9,
            "floating input made the gate chatter"
        );
    }
    assert!(y0 > 4.5, "A low => NAND high regardless of the float");
}

/// A chip with NO SUPPLY WIRED AT ALL. The single commonest beginner
/// mistake with a DIP. It must not quarantine and must not panic.
#[test]
fn adv_unsupplied_chip_does_not_break_the_room() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    // gate's VCC/GND pins go to two junctions connected to nothing else.
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(40, 0), (40, 24), (44, 4), (48, 4)],
    ));
    // a real load on the rail so the room is not empty
    v.push(spec(4, r(1000.0), (0, 0), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(200);
    assert!(!e.is_quarantined(), "an unsupplied chip quarantined the room");
    let f = frame_of(&e, 3);
    assert!(f.power.abs() < 1e-6, "unsupplied chip burns {} W", f.power);
    assert!(v_at(&e, (0, 0)) > 4.99, "the rest of the room still works");
}

/// TWO OUTPUTS TIED TOGETHER, fighting. The claim under audit: the node
/// parks at vcc/2, both chips survive, each burns ~125 mW, nothing
/// quarantines, and the result reads as INDETERMINATE rather than as a
/// confident lie.
#[test]
fn adv_two_outputs_fighting() {
    let y = (20, 6);
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    // BUF with input low -> drives Y low.
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), y],
    ));
    v.push(spec(4, r(10_000.0), (6, 4), (0, 24)));
    // NOT with input low -> drives Y high.
    v.push(logic(
        5,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 12), y],
    ));
    v.push(spec(6, r(10_000.0), (6, 12), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(500);
    assert!(!e.is_quarantined(), "a bus fight quarantined the room");
    let vy = v_at(&e, y);
    assert!(
        (vy - 2.5).abs() < 0.05,
        "two fighting outputs park at {vy:.4} V, expected vcc/2"
    );
    let p3 = frame_of(&e, 3).power;
    let p5 = frame_of(&e, 5).power;
    for (id, p) in [(3, p3), (5, p5)] {
        assert!(p > 0.0, "chip {id} power {p} must be dissipation, not gain");
        assert!(
            (p - 0.125).abs() < 0.01,
            "chip {id} burns {p:.4} W, expected ~0.125"
        );
    }
    // Damage must judge them: 0.125 W against a 0.35 W DIP-14 -> survives.
    let rat = damage::rating(
        &K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        0,
    )
    .unwrap();
    assert!(p3 < rat.limit, "0.125 W must be under the DIP-14 rating");
}

/// An output shorted straight to the opposite rail. 5 V / 50 Ω = 100 mA,
/// 500 mW: over a DIP-14 and the chip should die in seconds.
#[test]
fn adv_output_shorted_to_rail_burns_half_a_watt() {
    for (drive_high, short_to) in [(true, (0, 24)), (false, (0, 0))] {
        let y = (20, 6);
        let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        // NOT gate: input low -> Y high; input high -> Y low.
        v.push(logic(
            3,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), (6, 4), y],
        ));
        v.push(spec(4, r(10_000.0), (6, 4), (0, 24)));
        v.push(spec(
            5,
            K::Switch { closed: !drive_high },
            (0, 0),
            (6, 4),
        ));
        // The short: a 1 mΩ wire from Y to the rail it is fighting.
        v.push(spec(6, r(1e-3), y, short_to));
        let mut e = eng_with(&v);
        e.advance(500);
        assert!(!e.is_quarantined(), "a shorted output quarantined");
        let f = frame_of(&e, 3);
        assert!(
            (f.power - 0.5).abs() < 0.02,
            "shorted output (high={drive_high}) burns {:.4} W, expected 0.5",
            f.power
        );
        // and it really is moving that through a SUPPLY pin: a high output
        // sourcing into ground draws it from VCC, a low output sinking from
        // the rail dumps it out of GND.
        let supply = if drive_high { f.i[0] } else { f.i[1] };
        assert!(
            supply.abs() > 0.09,
            "high={drive_high}: supply pin only carries {supply:.4} A (vcc {:.4}, gnd {:.4})",
            f.i[0],
            f.i[1]
        );
    }
}

/// A 5 V part fed a 9 V input, which is what happens the moment a player
/// wires logic next to the hoist's motor rail. The VICTIM must latch up,
/// become a short, and be judged on its own dissipation.
#[test]
fn adv_overvoltage_input_latches_the_victim() {
    let mut v = vec![
        spec(1, dc(VCC), (0, 0), (0, 24)),
        gnd(2, (0, 24)),
        spec(7, dc(9.0), (40, 0), (0, 24)),
    ];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(1000.0), (14, 6), (0, 24)));
    // the mistake: 9 V straight onto a 5 V part's input, through a real wire
    v.push(spec(5, r(100.0), (40, 0), (6, 4)));
    let mut e = eng_with(&v);
    e.advance(200);
    assert!(!e.is_quarantined());
    let f = frame_of(&e, 3);
    // latch-up = 10 Ω across a 5 V supply = 2.5 W, plus the input path
    assert!(
        f.power > 1.0,
        "an overvolted chip only burns {:.4} W - latch-up did not fire",
        f.power
    );
    let rat = damage::rating(
        &K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        0,
    )
    .unwrap();
    assert!(f.power > rat.limit, "latch-up must exceed the package rating");
}

/// A chip run straight off a 9 V rail: over absolute maximum, latches.
#[test]
fn adv_nine_volt_rail_latches_and_kills() {
    let mut v = vec![spec(1, dc(9.0), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(10_000.0), (6, 4), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(100);
    let f = frame_of(&e, 3);
    assert!(
        f.power > 7.0,
        "9 V rail should latch and burn ~8.1 W, got {:.4}",
        f.power
    );
}

/// Reversed supply: VCC pin on ground, GND pin on the rail.
#[test]
fn adv_reversed_supply_is_not_silently_fine() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        // VCC pin -> ground, GND pin -> rail
        &[(0, 24), (0, 0), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(10_000.0), (6, 4), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(200);
    assert!(!e.is_quarantined(), "reversed supply quarantined");
    let f = frame_of(&e, 3);
    println!("REVERSED SUPPLY: power = {:.5} W, Y = {:.4} V", f.power, v_at(&e, (14, 6)));
    assert!(f.power >= 0.0, "reversed supply DELIVERS {} W", f.power);
}

/// Every player-visible number must be dissipation, never generation:
/// sweep a gate through every state under load and assert power >= 0 and
/// Σ pin currents = 0.
#[test]
fn adv_power_is_never_negative_and_kcl_closes() {
    let mut e = eng_with(&rig(GateOp::Xor, 3));
    for row in 0u32..8 {
        for k in 0..3 {
            e.interact(10 + k, InteractOp::SetSwitch { closed: row >> k & 1 == 1 });
        }
        for _ in 0..40 {
            e.advance(1);
            let f = frame_of(&e, 3);
            assert!(f.power >= 0.0, "row {row}: gate DELIVERED {} W", f.power);
            let sum: f64 = (0..f.npins).map(|p| f.i[p]).sum();
            assert!(sum.abs() < 1e-9, "row {row}: Σi = {sum:e}, KCL broken");
        }
    }
}

// ================================================== 3. SEQUENTIAL BEHAVIOUR

/// A /Q -> D flip-flop divides its clock by two. Sweep the clock against the
/// fixed 20 µs substep and find where that stops being true.
fn divider(hz: f64) -> Vec<ElementSpec> {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(spec(
        3,
        K::VoltageSource {
            wave: sim_core::Wave::Sine,
            dc: 2.5,
            amp: 2.5,
            hz,
            phase: 0.0,
        },
        (4, 12),
        (0, 24),
    ));
    v.push(logic(
        4,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (4, 12), (10, 12)],
    ));
    v.push(logic(
        5,
        K::FlipFlop { edge: true },
        &[(0, 0), (0, 24), (10, 12), (20, 16), (0, 0), (18, 8), (20, 16)],
    ));
    v.push(spec(6, r(1000.0), (18, 8), (0, 24)));
    v
}

/// Count rising edges of a node over `steps` substeps.
fn count_edges(e: &mut Engine, p: (i32, i32), steps: u32) -> u32 {
    let mut last = v_at(e, p) > 2.5;
    let mut n = 0;
    for _ in 0..steps {
        e.advance(1);
        let now = v_at(e, p) > 2.5;
        if now && !last {
            n += 1;
        }
        last = now;
    }
    n
}

#[test]
fn adv_flipflop_frequency_sweep() {
    println!("\n  clock Hz | expect Q edges | got | ratio");
    let mut last_good = 0.0;
    for hz in [
        100.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 8_000.0, 10_000.0, 12_500.0, 16_000.0, 20_000.0,
        25_000.0, 33_000.0, 50_000.0, 100_000.0,
    ] {
        let mut e = eng_with(&divider(hz));
        // settle, then measure over 20 ms of sim time
        e.advance(200);
        let steps = (0.02 / DT) as u32;
        let got = count_edges(&mut e, (18, 8), steps);
        let expect = hz * 0.02 / 2.0;
        let ratio = f64::from(got) / expect;
        println!(
            "  {hz:>8.0} | {expect:>14.1} | {got:>3} | {ratio:.4}{}",
            if e.is_quarantined() { "  QUARANTINED" } else { "" }
        );
        if (ratio - 1.0).abs() < 0.02 {
            last_good = hz;
        }
    }
    println!("  highest clock that still divides exactly by 2: {last_good} Hz");
    assert!(
        last_good >= 5000.0,
        "a flip-flop must be reliable to at least 5 kHz, got {last_good}"
    );
}

/// SETUP AND HOLD. D and CLK both come from logic, so both are quantized to
/// the substep grid; the question is what the part captures when D moves in
/// the SAME substep as the clock edge, one before, and one after.
#[test]
fn adv_setup_and_hold_at_the_substep_grid() {
    // CLK from switch id 10, D from switch id 11, both through a buffer so
    // they arrive on the same one-substep logic delay as any real signal.
    let build = || {
        let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        for (sw, pd, inp, out) in [
            (10u32, 20u32, (6, 0), (12, 0)),
            (11, 21, (6, 8), (12, 8)),
        ] {
            v.push(spec(sw, K::Switch { closed: false }, (0, 0), inp));
            v.push(spec(pd, r(10_000.0), inp, (0, 24)));
            v.push(logic(
                sw + 100,
                K::Gate {
                    op: GateOp::Buf,
                    ins: 1,
                },
                &[(0, 0), (0, 24), inp, out],
            ));
        }
        v.push(logic(
            5,
            K::FlipFlop { edge: true },
            &[(0, 0), (0, 24), (12, 0), (12, 8), (0, 0), (30, 4), (34, 4)],
        ));
        v.push(spec(6, r(1000.0), (30, 4), (0, 24)));
        v
    };
    // offset = substeps by which D leads the clock. Negative = D arrives
    // after the clock edge.
    println!("\n  D-vs-CLK offset (substeps) | Q captured");
    for offset in [-3i32, -2, -1, 0, 1, 2, 3] {
        let mut e = eng_with(&build());
        e.advance(100); // settle, Q = 0, both inputs low
        assert!(v_at(&e, (30, 4)) < 0.5);
        if offset > 0 {
            e.interact(11, InteractOp::SetSwitch { closed: true });
            e.advance(offset as u32);
            e.interact(10, InteractOp::SetSwitch { closed: true });
        } else {
            e.interact(10, InteractOp::SetSwitch { closed: true });
            e.advance((-offset) as u32);
            e.interact(11, InteractOp::SetSwitch { closed: true });
        }
        e.advance(60);
        let q = v_at(&e, (30, 4)) > 2.5;
        println!("  {offset:>26} | {}", if q { "1" } else { "0" });
    }
}

/// A CROSS-COUPLED NAND PAIR — the SR latch every beginner builds. Set,
/// reset, hold, and the forbidden 00 -> 11 race. Does it settle, oscillate,
/// or quarantine, and is the answer honest?
#[test]
fn adv_cross_coupled_nand_sr_latch() {
    // /S = switch 10 (closed = high), /R = switch 11. Pull-ups so an open
    // switch reads HIGH (inactive), which is what /S and /R want.
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let (sn, rn, q, qn) = ((6, 0), (6, 16), (20, 4), (20, 12));
    for (sw, n) in [(10u32, sn), (11u32, rn)] {
        v.push(spec(sw, K::Switch { closed: false }, n, (0, 24))); // closed = pull LOW
        v.push(spec(sw + 10, r(10_000.0), (0, 0), n)); // pull-up
    }
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        &[(0, 0), (0, 24), sn, qn, q],
    ));
    v.push(logic(
        4,
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        &[(0, 0), (0, 24), rn, q, qn],
    ));
    let mut e = eng_with(&v);
    e.advance(200);
    let read = |e: &Engine| (v_at(e, q) > 2.5, v_at(e, qn) > 2.5);

    // SET: /S low
    e.interact(10, InteractOp::SetSwitch { closed: true });
    e.advance(200);
    assert_eq!(read(&e), (true, false), "set failed");
    e.interact(10, InteractOp::SetSwitch { closed: false });
    e.advance(200);
    assert_eq!(read(&e), (true, false), "latch did not HOLD after set");

    // RESET: /R low
    e.interact(11, InteractOp::SetSwitch { closed: true });
    e.advance(200);
    assert_eq!(read(&e), (false, true), "reset failed");
    e.interact(11, InteractOp::SetSwitch { closed: false });
    e.advance(200);
    assert_eq!(read(&e), (false, true), "latch did not HOLD after reset");

    // THE FORBIDDEN STATE: both low -> both Q high, then release both at
    // once. A real latch races to an unpredictable side; a simulated one
    // must do something HONEST, not something silently confident.
    e.interact(10, InteractOp::SetSwitch { closed: true });
    e.interact(11, InteractOp::SetSwitch { closed: true });
    e.advance(200);
    assert_eq!(read(&e), (true, true), "both inputs low must force both high");
    e.interact(10, InteractOp::SetSwitch { closed: false });
    e.interact(11, InteractOp::SetSwitch { closed: false });
    // Watch what happens over 2000 substeps.
    let mut seen = std::collections::BTreeSet::new();
    let mut flips = 0;
    let mut last = read(&e);
    for _ in 0..2000 {
        e.advance(1);
        let now = read(&e);
        seen.insert(now);
        if now != last {
            flips += 1;
        }
        last = now;
    }
    println!(
        "\n  SR race: states seen = {seen:?}, transitions = {flips}, final = {last:?}, quarantined = {}",
        e.is_quarantined()
    );
    assert!(!e.is_quarantined(), "an SR race must not quarantine");
}

/// RING OSCILLATORS. An odd ring of inverters must oscillate. The claim
/// under audit is that the period is 2 substeps REGARDLESS of ring length,
/// which is an artifact of the one-substep-delay timing model and not a
/// frequency any player should be shown as real.
#[test]
fn adv_ring_oscillator_period_vs_length() {
    println!("\n  stages | measured period (substeps) | implied Hz | 1/(2·N·dt) would be");
    for n in [3usize, 5, 7, 9] {
        let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        let node = |k: usize| (10 + 6 * k as i32, 0);
        for k in 0..n {
            v.push(logic(
                (10 + k) as u32,
                K::Gate {
                    op: GateOp::Not,
                    ins: 1,
                },
                &[(0, 0), (0, 24), node(k), node((k + 1) % n)],
            ));
            v.push(spec((50 + k) as u32, r(100_000.0), node(k), (0, 24)));
        }
        let mut e = eng_with(&v);
        e.advance(200);
        let edges = count_edges(&mut e, node(0), 2000);
        assert!(edges > 0, "{n}-stage ring did not oscillate");
        let period = 2000.0 / f64::from(edges);
        println!(
            "  {n:>6} | {period:>26.3} | {:>10.0} | {:.0}",
            1.0 / (period * DT),
            1.0 / (2.0 * n as f64 * DT)
        );
        assert!(!e.is_quarantined());
    }
}

/// An EVEN ring of inverters is a LATCH in the world: there is no net
/// inversion around the loop, so it has two stable states and no
/// oscillation is possible. Report what this model does instead, with and
/// without the asymmetry a real board always has.
#[test]
fn adv_even_ring_should_be_bistable() {
    println!("\n  even ring | node cap | edges/4000 substeps | verdict");
    for n in [2usize, 4, 6] {
        for cap in [0.0f64, 1e-9, 1e-7] {
            let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
            let node = |k: usize| (10 + 6 * k as i32, 0);
            for k in 0..n {
                v.push(logic(
                    (10 + k) as u32,
                    K::Gate {
                        op: GateOp::Not,
                        ins: 1,
                    },
                    &[(0, 0), (0, 24), node(k), node((k + 1) % n)],
                ));
                v.push(spec((50 + k) as u32, r(100_000.0), node(k), (0, 24)));
            }
            if cap > 0.0 {
                // asymmetry: stray capacitance on ONE node only
                v.push(spec(90, K::Capacitor { farads: cap }, node(0), (0, 24)));
            }
            let mut e = eng_with(&v);
            e.advance(500);
            let edges = count_edges(&mut e, node(0), 4000);
            println!(
                "  {n:>9} | {cap:>8.0e} | {edges:>19} | {}",
                if edges == 0 { "bistable (correct)" } else { "OSCILLATES (wrong)" }
            );
        }
    }
}

/// A clock faster than the substep grid can carry. This must be reported
/// honestly rather than silently aliased into a plausible-looking number.
#[test]
fn adv_clock_above_nyquist_is_an_artifact_not_a_lie() {
    for hz in [25_000.0, 50_000.0, 100_000.0, 200_000.0] {
        let mut e = eng_with(&divider(hz));
        e.advance(200);
        let got = count_edges(&mut e, (18, 8), 5000);
        let implied = f64::from(got) / (5000.0 * DT);
        println!(
            "\n  clock {hz:>8.0} Hz (samples/cycle = {:.2}) -> Q at {implied:.0} Hz, want {:.0}",
            1.0 / (hz * DT),
            hz / 2.0
        );
    }
}

/// Does a latch stuck in the symmetric period-2 orbit ever RECOVER, and what
/// does the orbit cost the solver while it lasts?
#[test]
fn adv_symmetric_orbit_recovery_and_cost() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let (sn, rn, q, qn) = ((6, 0), (6, 16), (20, 4), (20, 12));
    for (sw, n) in [(10u32, sn), (11u32, rn)] {
        v.push(spec(sw, K::Switch { closed: false }, n, (0, 24)));
        v.push(spec(sw + 10, r(10_000.0), (0, 0), n));
    }
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        &[(0, 0), (0, 24), sn, qn, q],
    ));
    v.push(logic(
        4,
        K::Gate {
            op: GateOp::Nand,
            ins: 2,
        },
        &[(0, 0), (0, 24), rn, q, qn],
    ));
    let mut e = eng_with(&v);
    e.advance(200);
    // into the forbidden state and out of it together
    e.interact(10, InteractOp::SetSwitch { closed: true });
    e.interact(11, InteractOp::SetSwitch { closed: true });
    e.advance(200);
    e.interact(10, InteractOp::SetSwitch { closed: false });
    e.interact(11, InteractOp::SetSwitch { closed: false });
    e.advance(2000);
    let f0 = e.factorizations();
    e.advance(2000);
    let orbit_factors = e.factorizations() - f0;
    println!(
        "\n  stuck orbit: {orbit_factors} factorizations per 2000 substeps ({:.2}/substep)",
        orbit_factors as f64 / 2000.0
    );
    // now touch /S: does the player get their latch back?
    e.interact(10, InteractOp::SetSwitch { closed: true });
    e.advance(200);
    e.interact(10, InteractOp::SetSwitch { closed: false });
    let f1 = e.factorizations();
    e.advance(2000);
    let after = e.factorizations() - f1;
    let stable = v_at(&e, q) > 2.5 && v_at(&e, qn) < 2.5;
    println!(
        "  after a set pulse: Q={:.3} /Q={:.3} -> {}, {after} factorizations per 2000 substeps",
        v_at(&e, q),
        v_at(&e, qn),
        if stable { "RECOVERED" } else { "still stuck" }
    );
    assert!(stable, "a set pulse must rescue the latch");
}

/// The cross-coupled INVERTER pair: the simplest static memory cell there
/// is, and the one a player builds to store a bit. It has no set/reset
/// input to break the symmetry with.
#[test]
fn adv_cross_coupled_inverters_cannot_store_a_bit() {
    let (a, b) = ((20, 0), (20, 8));
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), a, b],
    ));
    v.push(logic(
        4,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), b, a],
    ));
    // a weak write port: a switch that can force A high
    v.push(spec(10, K::Switch { closed: false }, (0, 0), a));
    let mut e = eng_with(&v);
    e.advance(500);
    let cold = count_edges(&mut e, a, 2000);
    // write a 1 and let go
    e.interact(10, InteractOp::SetSwitch { closed: true });
    e.advance(500);
    e.interact(10, InteractOp::SetSwitch { closed: false });
    e.advance(500);
    let after = count_edges(&mut e, a, 2000);
    println!(
        "\n  cross-coupled inverters: cold {cold} edges/2000, after a forced write {after} edges/2000, A={:.3} B={:.3}",
        v_at(&e, a),
        v_at(&e, b)
    );
}

// ======================================================= 5. HONESTY AUDIT

/// WHERE DOES A LOGIC HIGH GET ITS ENERGY? Energy balance over the whole
/// room: what the sources deliver must equal what every other part burns,
/// to solver precision, in every logic state. If a gate invented current
/// this is the test that catches it.
#[test]
fn adv_room_energy_balances_in_every_logic_state() {
    let mut e = eng_with(&rig(GateOp::Nand, 3));
    for row in 0u32..8 {
        for k in 0..3 {
            e.interact(10 + k, InteractOp::SetSwitch { closed: row >> k & 1 == 1 });
        }
        e.advance(60);
        let (mut delivered, mut burned) = (0.0f64, 0.0f64);
        for f in e.frame() {
            // Sources report NEGATIVE power (delivering).
            if f.power < 0.0 {
                delivered -= f.power;
            } else {
                burned += f.power;
            }
        }
        assert!(
            (delivered - burned).abs() <= 1e-7 * delivered.max(1e-6) + 1e-12,
            "row {row}: sources delivered {delivered:.12} W, parts burned {burned:.12} W"
        );
        assert!(delivered > 0.0, "row {row}: nothing was delivered at all");
    }
}

/// A BROKEN logic chip must be an open circuit that stores nothing — not a
/// part that keeps clocking with its legs cut off.
#[test]
fn adv_a_broken_chip_is_open_and_silent() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(10_000.0), (6, 4), (0, 24)));
    v.push(spec(5, r(1000.0), (14, 6), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(100);
    assert!(v_at(&e, (14, 6)) > 4.5, "alive: Y should be high");
    e.set_broken(3, true);
    e.advance(100);
    assert!(!e.is_quarantined(), "breaking a chip quarantined the room");
    let f = frame_of(&e, 3);
    assert_eq!(f.power, 0.0, "a dead chip must burn nothing");
    for p in 0..f.npins {
        assert_eq!(f.i[p], 0.0, "dead chip pin {p} carries {} A", f.i[p]);
    }
    assert!(
        v_at(&e, (14, 6)).abs() < 1e-6,
        "a dead output must be open: the 1 k load pulls it to 0, got {:.6}",
        v_at(&e, (14, 6))
    );
}

/// A logic part must be PLACEABLE from any discrete state: `probe_solvable`
/// factors exactly one cold state, and the family's claim is that its
/// incidence pattern never moves. Run the probe with the chip in several
/// live states and demand the same answer.
#[test]
fn adv_probe_solvable_agrees_with_every_live_state() {
    let mut e = eng_with(&rig(GateOp::Xor, 2));
    for row in 0u32..4 {
        for k in 0..2 {
            e.interact(10 + k, InteractOp::SetSwitch { closed: row >> k & 1 == 1 });
        }
        e.advance(50);
        assert!(e.probe_solvable(), "row {row}: the live matrix is singular");
    }
}

/// The RESCUE LADDER halves the step and runs `accept` TWICE for one dt. A
/// logic element advances its state in `accept`, so a rescue must not clock
/// a register twice. Provoke rescues with an inductive kick on a gate output
/// and count the shifts against the clock.
#[test]
fn adv_a_rescue_must_not_double_clock() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(spec(
        3,
        K::VoltageSource {
            wave: sim_core::Wave::Sine,
            dc: 2.5,
            amp: 2.5,
            hz: 200.0,
            phase: 0.0,
        },
        (4, 12),
        (0, 24),
    ));
    v.push(logic(
        4,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (4, 12), (10, 12)],
    ));
    v.push(logic(
        5,
        K::ShiftReg { bits: 4 },
        &[
            (0, 0),
            (0, 24),
            (10, 12),
            (30, 12),
            (0, 0),
            (20, 4),
            (20, 8),
            (20, 12),
            (20, 16),
        ],
    ));
    v.push(logic(
        6,
        K::Gate {
            op: GateOp::Nor,
            ins: 3,
        },
        &[(0, 0), (0, 24), (20, 4), (20, 8), (20, 12), (30, 12)],
    ));
    for (id, j) in [(9u32, 4), (10, 8), (12, 12), (13, 16)] {
        v.push(spec(id, r(10_000.0), (20, j), (0, 24)));
    }
    // A buffer off Q0 driving the nastiest load in the parts bin: a 1 H
    // inductor and an LED, switched hard between the rails every four
    // clocks. Kept OFF the ring itself so the feedback stays clean.
    v.push(logic(
        14,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (20, 4), (60, 4)],
    ));
    v.push(spec(15, K::Inductor { henries: 1.0 }, (60, 4), (60, 12)));
    v.push(spec(16, K::Led { color: 0 }, (60, 12), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(2000);
    let steps = 50_000u32; // 1 s at 200 Hz = 200 clocks
    let rep = e.advance(steps);
    // Count Q0 rising edges: a one-hot ring pulses each output once per four
    // clocks, so 200 clocks/s must give 50 Q0 pulses.
    let mut e2 = eng_with(&v);
    e2.advance(2000);
    let q0 = count_edges(&mut e2, (20, 4), steps);
    println!(
        "\n  rescue test: {} rescues over {} substeps, Q0 pulsed {q0} times (want 50), quarantined={}",
        rep.rescues, rep.steps, e.is_quarantined()
    );
    assert!(!e.is_quarantined());
    assert_eq!(q0, 50, "the ring lost or gained a step");
}

/// THE FULL DAMAGE LOOP, exactly as the server runs it: sim -> frame ->
/// `DamageModel::tick` -> stamp the dead part open. Does a logic part
/// actually die of its own dissipation, and does a healthy one survive?
#[test]
fn adv_damage_breaks_an_abused_chip_and_spares_a_working_one() {
    // Chip 3 drives a 1 kΩ load (fine). Chip 5's output is shorted to the
    // opposite rail (0.5 W against a 0.35 W DIP-14).
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(1000.0), (14, 6), (0, 24)));
    v.push(logic(
        5,
        K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 12), (20, 6)],
    ));
    v.push(spec(6, r(1e-3), (20, 6), (0, 24))); // Y is high, shorted to GND
    v.push(spec(7, r(10_000.0), (6, 4), (0, 24)));
    v.push(spec(8, r(10_000.0), (6, 12), (0, 24)));
    let mut e = eng_with(&v);
    let mut dm = damage::DamageModel::new();
    dm.set_document(&v);
    let mut t = 0.0f64;
    let mut died: Option<(u32, f64)> = None;
    for k in 0..60_000 {
        e.advance(50);
        t += 50.0 * DT;
        let fr = e.frame();
        if k == 100 {
            let p5 = fr.iter().find(|f| f.id == 5).unwrap().power;
            let p3 = fr.iter().find(|f| f.id == 3).unwrap().power;
            println!("\n  chip 3 (1 k load) = {p3:.5} W, chip 5 (shorted) = {p5:.5} W");
        }
        for b in dm.tick(&fr, 50.0 * DT) {
            died.get_or_insert((b.id, t));
            e.set_broken(b.id, true);
        }
        if died.is_some() {
            break;
        }
    }
    let (id, when) = died.expect("the shorted chip never broke");
    println!("\n  damage: part {id} died at t = {when:.3} s; healthy chip stress = {:.4}", dm.stress(3));
    assert_eq!(id, 5, "the WRONG part broke");
    assert!(when < 10.0, "a 0.5 W short took {when:.2} s to kill a 0.35 W DIP");
    assert!(!dm.is_broken(3), "the loaded-but-fine chip was killed too");
    assert!(dm.stress(3) < 0.5, "a 1 k load must not stress the chip");
    assert!(!e.is_quarantined());
}

/// `accept` computes a chip's PIN CURRENTS from the discrete state it has
/// just advanced to, but from the node voltages the PREVIOUS state was
/// solved against. On the one substep where an output flips, the reported
/// current is therefore not the current the solver produced. Measure how big
/// that is, and whether the frame's KCL at the shared node survives it.
#[test]
fn adv_transition_substep_current_consistency() {
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    v.push(logic(
        3,
        K::Gate {
            op: GateOp::Buf,
            ins: 1,
        },
        &[(0, 0), (0, 24), (6, 4), (14, 6)],
    ));
    v.push(spec(4, r(1000.0), (14, 6), (0, 24)));
    v.push(spec(10, K::Switch { closed: false }, (0, 0), (6, 4)));
    v.push(spec(11, r(10_000.0), (6, 4), (0, 24)));
    let mut e = eng_with(&v);
    e.advance(200);
    e.interact(10, InteractOp::SetSwitch { closed: true });
    let (mut worst_p, mut worst_kcl) = (0.0f64, 0.0f64);
    println!("\n  substep | chip P (W) | Y (V) | chip i[Y] (A) | load i (A) | node KCL error (A)");
    for k in 0..8 {
        e.advance(1);
        let f3 = frame_of(&e, 3);
        let f4 = frame_of(&e, 4);
        // The gate's Y pin and the load's top pin are the SAME node.
        let kcl = f3.i[3] + f4.i[0];
        worst_p = worst_p.max(f3.power);
        worst_kcl = worst_kcl.max(kcl.abs());
        println!(
            "  {k:>7} | {:>10.5} | {:>5.3} | {:>13.6} | {:>10.6} | {kcl:.6}",
            f3.power,
            v_at(&e, (14, 6)),
            f3.i[3],
            f4.i[0]
        );
    }
    println!("  worst reported power {worst_p:.5} W, worst frame KCL error {worst_kcl:.6} A");
    // Steady state, for scale.
    e.advance(200);
    println!("  settled: chip P = {:.6} W", frame_of(&e, 3).power);
}

/// The worst case for that inconsistency: an output flipping EVERY substep
/// (a ring oscillator). The reported power is then the ARTIFACT at 100 %
/// duty. Measured against the package rating and against what the room's
/// source actually delivers — see
/// `adv_does_the_transition_artifact_kill_a_ring`, which is the same circuit
/// run to destruction.
#[test]
fn adv_a_ring_oscillator_reports_the_artifact_at_full_duty() {
    let n = 3usize;
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let node = |k: usize| (10 + 6 * k as i32, 0);
    for k in 0..n {
        v.push(logic(
            (10 + k) as u32,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), node(k), node((k + 1) % n)],
        ));
        v.push(spec((50 + k) as u32, r(100_000.0), node(k), (0, 24)));
    }
    let mut e = eng_with(&v);
    let mut dm = damage::DamageModel::new();
    dm.set_document(&v);
    e.advance(500);
    let mut peak = 0.0f64;
    let mut sum = 0.0f64;
    let mut n_s = 0.0f64;
    for _ in 0..20_000 {
        e.advance(1);
        let fr = e.frame();
        let p = fr.iter().find(|f| f.id == 10).unwrap().power;
        peak = peak.max(p);
        sum += p;
        n_s += 1.0;
        for b in dm.tick(&fr, DT) {
            panic!("the ring killed part {} - a gate driving 100 kΩ", b.id);
        }
    }
    let rat = damage::rating(
        &K::Gate {
            op: GateOp::Not,
            ins: 1,
        },
        0,
    )
    .unwrap();
    println!(
        "\n  3-ring at 25 kHz: peak reported P = {peak:.4} W, mean = {:.4} W, DIP rating = {:.2} W, stress = {:.4}",
        sum / n_s,
        rat.limit,
        dm.stress(10)
    );
}

/// Run the ring long enough to find out whether the reporting artifact
/// actually EXECUTES the gates, and compare the reported power with the
/// dissipation the network can physically contain.
#[test]
fn adv_does_the_transition_artifact_kill_a_ring() {
    let n = 3usize;
    let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
    let node = |k: usize| (10 + 6 * k as i32, 0);
    for k in 0..n {
        v.push(logic(
            (10 + k) as u32,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), node(k), node((k + 1) % n)],
        ));
        v.push(spec((50 + k) as u32, r(100_000.0), node(k), (0, 24)));
    }
    let mut e = eng_with(&v);
    let mut dm = damage::DamageModel::new();
    dm.set_document(&v);
    e.advance(500);
    let mut t = 0.0;
    let mut dead = None;
    for _ in 0..400_000 {
        e.advance(1);
        t += DT;
        for b in dm.tick(&e.frame(), DT) {
            dead = Some((b.id, t));
        }
        if dead.is_some() {
            break;
        }
    }
    // What the SOURCE actually delivers is the physical answer: it is what
    // the whole room draws, gates included.
    let src = e
        .frame()
        .iter()
        .find(|f| f.id == 1)
        .map(|f| -f.power)
        .unwrap_or(0.0);
    println!(
        "\n  3-ring: source delivers {src:.6} W to the WHOLE room; {:?}",
        dead.map(|(id, t)| format!("part {id} died at t = {t:.3} s"))
    );
    println!("  stress on the three gates: {:.3} {:.3} {:.3}", dm.stress(10), dm.stress(11), dm.stress(12));
}

/// The two findings COMPOUND, and this is the player-facing consequence.
/// A symmetric feedback loop sits in a substep-rate period-2 orbit
/// (`adv_even_ring_should_be_bistable`), and an output that flips every
/// substep reports the transition artifact at 100 % duty
/// (`adv_transition_substep_current_consistency`). So the two most ordinary
/// latches in digital electronics destroy themselves.
#[test]
fn adv_two_ordinary_latches_destroy_themselves() {
    // (a) cross-coupled inverters, straight from power-up.
    {
        let (a, b) = ((20, 0), (20, 8));
        let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        v.push(logic(
            3,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), a, b],
        ));
        v.push(logic(
            4,
            K::Gate {
                op: GateOp::Not,
                ins: 1,
            },
            &[(0, 0), (0, 24), b, a],
        ));
        let mut e = eng_with(&v);
        let mut dm = damage::DamageModel::new();
        dm.set_document(&v);
        let mut t = 0.0;
        let mut dead = None;
        for _ in 0..400_000 {
            e.advance(1);
            t += DT;
            for br in dm.tick(&e.frame(), DT) {
                dead = Some((br.id, t));
            }
            if dead.is_some() {
                break;
            }
        }
        let src = -e.frame().iter().find(|f| f.id == 1).unwrap().power;
        println!(
            "\n  (a) cross-coupled inverter latch, from cold: source delivers {src:.6} W; {:?}",
            dead.map(|(id, t)| format!("part {id} destroyed at t = {t:.3} s"))
        );
    }
    // (b) a cross-coupled NAND SR latch, after ONE forbidden-state race.
    {
        let mut v = vec![spec(1, dc(VCC), (0, 0), (0, 24)), gnd(2, (0, 24))];
        let (sn, rn, q, qn) = ((6, 0), (6, 16), (20, 4), (20, 12));
        for (sw, n) in [(10u32, sn), (11u32, rn)] {
            v.push(spec(sw, K::Switch { closed: false }, n, (0, 24)));
            v.push(spec(sw + 10, r(10_000.0), (0, 0), n));
        }
        v.push(logic(
            3,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), sn, qn, q],
        ));
        v.push(logic(
            4,
            K::Gate {
                op: GateOp::Nand,
                ins: 2,
            },
            &[(0, 0), (0, 24), rn, q, qn],
        ));
        let mut e = eng_with(&v);
        let mut dm = damage::DamageModel::new();
        dm.set_document(&v);
        e.advance(200);
        e.interact(10, InteractOp::SetSwitch { closed: true });
        e.interact(11, InteractOp::SetSwitch { closed: true });
        e.advance(200);
        e.interact(10, InteractOp::SetSwitch { closed: false });
        e.interact(11, InteractOp::SetSwitch { closed: false });
        let mut t = 0.0;
        let mut dead = None;
        for _ in 0..400_000 {
            e.advance(1);
            t += DT;
            for br in dm.tick(&e.frame(), DT) {
                dead = Some((br.id, t));
            }
            if dead.is_some() {
                break;
            }
        }
        let src = -e.frame().iter().find(|f| f.id == 1).unwrap().power;
        println!(
            "  (b) NAND SR latch after one illegal 00 input: source delivers {src:.6} W; {:?}",
            dead.map(|(id, t)| format!("part {id} destroyed at t = {t:.3} s"))
        );
    }
}
