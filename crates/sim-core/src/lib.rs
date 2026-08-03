//! sim-core: the authoritative circuit simulation. Pure computation — no
//! I/O, no threads, no clocks — so it compiles bit-identically for the
//! native server and the wasm32 client preview.

pub mod constraint;
mod engine;
mod netlist;
pub mod shape;
pub mod validate;

pub use constraint::{constraint_of, Constraint, ConstraintKey};
pub use engine::{
    AdvanceReport, ElemFrame, ElemTap, Engine, Island, Tuning, FRAME_STRIDE, GMIN,
};
pub use netlist::{
    Wave,
    photocell_ohms, DocOp, ElementKind, ElementSpec, GateOp, InteractOp, LogicPins, ParamWrite,
    Point,
    DEFAULT_OPAMP_ISC, MAX_PINS, MAX_TIER,
};
pub use shape::{is_rigid, Handle, Placement, Shape};
pub use validate::{check_document, check_edit, check_shapes, rigid_hint, Reject, SmallIds};

#[cfg(test)]
mod tests {
    use super::*;

    /// Battery -> switch -> lamp loop: the M1 demo circuit.
    fn demo_circuit(closed: bool) -> Vec<ElementSpec> {
        let dc9 = ElementKind::VoltageSource {
            wave: crate::netlist::Wave::Sine,
            dc: 9.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        };
        vec![
            ElementSpec::two(1, dc9, (0, 0), (0, 4)),
            ElementSpec::two(2, ElementKind::Wire, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Switch { closed }, (4, 0), (8, 0)),
            ElementSpec::two(
                4,
                ElementKind::Lamp {
                    ohms: 90.0,
                    rated_watts: 1.0,
                },
                (8, 0),
                (8, 4),
            ),
            ElementSpec::two(5, ElementKind::Wire, (8, 4), (0, 4)),
            ElementSpec::ground(6, (0, 4)),
        ]
    }

    #[test]
    fn lamp_lights_when_switch_closes() {
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&demo_circuit(false));
        eng.advance(100);
        let f = eng.frame();
        let lamp = f.iter().find(|e| e.id == 4).unwrap();
        assert!(
            lamp.power.abs() < 1e-9,
            "open switch: lamp dark, got {}",
            lamp.power
        );

        eng.interact(3, InteractOp::SetSwitch { closed: true });
        eng.advance(100);
        let f = eng.frame();
        let lamp = f.iter().find(|e| e.id == 4).unwrap();
        // 9 V across 90 ohms: 0.1 A, 0.9 W.
        assert!((lamp.i[0] - 0.1).abs() < 1e-6, "lamp current {}", lamp.i[0]);
        assert!((lamp.power - 0.9).abs() < 1e-5, "lamp power {}", lamp.power);

        // Wire current recovered by KCL propagation matches the loop
        // current (sign depends on orientation).
        let wire = f.iter().find(|e| e.id == 2).unwrap();
        assert!(
            (wire.i[0].abs() - 0.1).abs() < 1e-6,
            "wire current {}",
            wire.i[0]
        );
    }

    #[test]
    fn divider_is_exact() {
        // 10 V across 1k + 3k: midpoint at 7.5 V (pin-0 side is +).
        let dc10 = ElementKind::VoltageSource {
            wave: crate::netlist::Wave::Sine,
            dc: 10.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        };
        let elems = vec![
            ElementSpec::two(1, dc10, (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Resistor { ohms: 3000.0 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(1);
        // Exact up to the GMIN leak (1e-12 S per node pulls ~nV-µV level).
        let v_mid = eng.voltage_at((4, 0)).unwrap();
        assert!((v_mid - 7.5).abs() < 1e-6, "divider mid {v_mid}");
    }

    /// The audio tap must read the same terminal difference `pin_voltage`
    /// reports, in O(1), and must survive a stale handle without panicking.
    #[test]
    fn tap_reads_speaker_coil_drive() {
        // 5 V peak 440 Hz across an 8 Ω speaker in series with 8 Ω: the coil
        // sees exactly half the source, and the tap sees the coil.
        let ac = ElementKind::VoltageSource {
            wave: crate::netlist::Wave::Sine,
            dc: 0.0,
            amp: 5.0,
            hz: 440.0,
            phase: 0.0,
        };
        let elems = vec![
            ElementSpec::two(1, ac, (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 8.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Speaker { ohms: 8.0 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&elems);
        let tap = eng.tap(3).expect("speaker exists");
        assert_eq!(eng.tap_id(tap), Some(3));

        // A quarter period in: the source is at its peak, so the coil is at
        // half of it. Sample through the tap and cross-check pin_voltage.
        let mut peak = 0.0f64;
        for _ in 0..114 {
            eng.advance(1);
            let d = eng.tap_delta(tap, 0, 1);
            let slow = eng.pin_voltage(3, 0).unwrap() - eng.pin_voltage(3, 1).unwrap();
            assert_eq!(d, slow, "tap must agree with pin_voltage bit for bit");
            if d.abs() > peak.abs() {
                peak = d;
            }
        }
        assert!(
            (peak.abs() - 2.5).abs() < 0.02,
            "coil should see half of a 5 V source, got {peak}"
        );

        // Out-of-range pins and a handle whose element is gone read 0.
        assert_eq!(eng.tap_delta(tap, 0, 5), eng.pin_voltage(3, 0).unwrap());
        eng.set_elements(&[]);
        assert_eq!(eng.tap_delta(tap, 0, 1), 0.0);
        assert_eq!(eng.tap_id(tap), None);
        assert!(eng.tap(3).is_none());
    }

    /// The motor's armature history must be part of the state digest, or the
    /// S1 cross-target harness would happily miss a native/wasm32 divergence
    /// inside the new device.
    fn motor_loop(bemf: f64) -> Vec<ElementSpec> {
        vec![
            ElementSpec::two(
                1,
                ElementKind::VoltageSource {
                    wave: crate::netlist::Wave::Sine,
                    dc: 12.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 4),
            ),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1.0 }, (0, 0), (4, 0)),
            ElementSpec::two(
                3,
                ElementKind::Motor {
                    ohms: 2.0,
                    henries: 1.5e-3,
                    bemf,
                },
                (4, 0),
                (0, 4),
            ),
            ElementSpec::ground(4, (0, 4)),
        ]
    }

    #[test]
    fn motor_state_enters_the_hash() {
        let run = |bemf: f64| {
            let mut eng = Engine::new(10e-6);
            eng.set_elements(&motor_loop(bemf));
            eng.advance(500);
            eng.state_hash()
        };
        assert_eq!(run(0.0), run(0.0));
        assert_ne!(
            run(0.0),
            run(3.0),
            "armature current/history must reach the digest"
        );
    }

    #[test]
    fn write_param_changes_the_value_and_nothing_else() {
        // The machine writes at 1.5 kHz. If these writes behaved like
        // `interact()` they would re-arm the post-event backward-Euler steps
        // (never letting the integrator return to second order) and clear
        // `quarantined` (hiding a diverged circuit forever) — see
        // `Engine::write_param`, which carries both flags across the only
        // compile it can trigger. What is observable from outside is that a
        // no-op write disturbs no state at all, and that a real write moves
        // exactly the one number.
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&motor_loop(0.0));
        eng.advance(2000);
        let before = eng.state_hash();
        assert!(eng.write_param(3, ParamWrite::Bemf { volts: 0.0 }));
        assert_eq!(before, eng.state_hash(), "a no-op write must not disturb");
        // Same current for the same bemf: the RHS write is the only effect.
        assert!(eng.write_param(3, ParamWrite::Bemf { volts: 4.0 }));
        eng.advance(2000);
        let i = eng.pin_current(3, 0).unwrap();
        assert!((i - 8.0 / 3.0).abs() < 1e-6, "bemf 4 V -> 2.667 A, got {i}");
        assert!(!eng.is_quarantined());
    }

    /// The damage mechanism, from sim-core's side of the fence: a broken part
    /// is an open circuit whose pins are still junctions, and repairing it
    /// puts the current back. WHEN a part breaks is not decided here.
    #[test]
    fn a_broken_part_fails_open_and_repair_restores_it() {
        // Two resistors in parallel across 9 V: 90 Ω and 45 Ω.
        let elems = vec![
            ElementSpec::two(
                1,
                ElementKind::VoltageSource {
                    wave: crate::netlist::Wave::Sine,
                    dc: 9.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 8),
            ),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 90.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Wire, (4, 0), (0, 8)),
            ElementSpec::two(4, ElementKind::Resistor { ohms: 45.0 }, (0, 0), (4, 4)),
            ElementSpec::two(5, ElementKind::Wire, (4, 4), (0, 8)),
            ElementSpec::ground(6, (0, 8)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(10);
        assert!((eng.pin_current(2, 0).unwrap() - 0.1).abs() < 1e-9);
        assert!((eng.pin_current(4, 0).unwrap() - 0.2).abs() < 1e-9);

        assert!(eng.set_broken(2, true));
        assert!(eng.is_broken(2));
        assert!(!eng.set_broken(999, true), "unknown id");
        eng.advance(10);
        // Dead branch: exactly zero, and no power reported.
        assert_eq!(eng.pin_current(2, 0).unwrap(), 0.0);
        let f = eng.frame();
        let dead = f.iter().find(|e| e.id == 2).unwrap();
        assert_eq!(dead.power, 0.0);
        // Its pins are still junctions: pin 1 sits on the ground node.
        assert_eq!(eng.pin_voltage(2, 0).unwrap(), 9.0);
        assert!(eng.pin_voltage(2, 1).unwrap().abs() < 1e-6);
        // And the healthy branch beside it never noticed.
        assert!((eng.pin_current(4, 0).unwrap() - 0.2).abs() < 1e-9);

        // A document edit must not resurrect it (state survives by id).
        eng.set_elements(&elems);
        eng.advance(10);
        assert!(eng.is_broken(2));
        assert_eq!(eng.pin_current(2, 0).unwrap(), 0.0);

        assert!(eng.set_broken(2, false));
        eng.advance(10);
        assert!(!eng.is_broken(2));
        assert!((eng.pin_current(2, 0).unwrap() - 0.1).abs() < 1e-9);
        assert!(!eng.is_quarantined());
    }

    /// Breaking a branch device (its unknown disappears) must renumber the
    /// system without disturbing anything else, and a broken nonlinear device
    /// hands back the linear fast path.
    #[test]
    fn breaking_a_branch_device_renumbers_the_unknowns() {
        // 9 V through a closed switch into an LED + 330 Ω, plus a plain 90 Ω
        // load straight across the supply.
        let elems = vec![
            ElementSpec::two(
                1,
                ElementKind::VoltageSource {
                    wave: crate::netlist::Wave::Sine,
                    dc: 9.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 8),
            ),
            ElementSpec::two(2, ElementKind::Switch { closed: true }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Resistor { ohms: 330.0 }, (4, 0), (8, 0)),
            ElementSpec::two(4, ElementKind::Led { color: 0 }, (8, 0), (0, 8)),
            ElementSpec::two(5, ElementKind::Resistor { ohms: 90.0 }, (0, 0), (0, 8)),
            ElementSpec::ground(6, (0, 8)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(200);
        let i_led = eng.pin_current(4, 0).unwrap();
        assert!(i_led > 0.015 && i_led < 0.03, "LED current {i_led}");

        // Kill the switch: the LED branch goes dark, the 90 Ω load does not.
        assert!(eng.set_broken(2, true));
        eng.advance(200);
        assert_eq!(eng.pin_current(2, 0).unwrap(), 0.0);
        assert!(eng.pin_current(4, 0).unwrap().abs() < 1e-9);
        assert!((eng.pin_current(5, 0).unwrap() - 0.1).abs() < 1e-9);
        assert!(!eng.is_quarantined());

        // Kill the LED too and the whole circuit is linear again; the
        // remaining resistor still solves.
        assert!(eng.set_broken(4, true));
        eng.advance(200);
        assert!((eng.pin_current(5, 0).unwrap() - 0.1).abs() < 1e-9);
        assert!(!eng.is_quarantined());
    }

    /// The state digest must not grow when nothing is broken (the S1 golden
    /// hashes are a fixed contract), and must move when something is.
    #[test]
    fn broken_state_reaches_the_hash_only_when_it_exists() {
        let run = |break_id: Option<u32>| {
            let mut eng = Engine::new(10e-6);
            eng.set_elements(&demo_circuit(true));
            if let Some(id) = break_id {
                eng.set_broken(id, true);
            }
            eng.advance(200);
            eng.state_hash()
        };
        assert_eq!(run(None), run(None));
        assert_ne!(
            run(None),
            run(Some(4)),
            "a dead lamp must change the digest"
        );
        assert_ne!(
            run(Some(3)),
            run(Some(4)),
            "which part died must change the digest"
        );
    }

    /// A single-pin rail must behave exactly like a grounded battery: same
    /// node voltage, same loop current, and the branch current reported on
    /// its one pin.
    #[test]
    fn rail_is_a_grounded_battery() {
        let rail = ElementKind::Rail {
            wave: crate::netlist::Wave::Sine,
            dc: 5.0,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        };
        let elems = vec![
            ElementSpec {
                id: 1,
                kind: rail,
                pins: vec![(0, 0)],
                tier: 0,
                rot: 0,
            },
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (8, 0)),
            ElementSpec::ground(3, (8, 0)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(10);
        let v = eng.voltage_at((0, 0)).unwrap();
        assert!((v - 5.0).abs() < 1e-9, "rail node must sit at 5 V, got {v}");
        // The rail sources the loop current through its single pin.
        let i = eng.pin_current(1, 0).unwrap();
        assert!((i + 5.0 / 1000.0).abs() < 1e-9, "rail branch current {i}");
        // Configurable: SetValue retargets the DC level like a battery's.
        eng.interact(1, InteractOp::SetValue { value: 3.3 });
        eng.advance(10);
        let v = eng.voltage_at((0, 0)).unwrap();
        assert!((v - 3.3).abs() < 1e-9, "rail must follow SetValue, got {v}");
    }

    #[test]
    fn state_hash_is_reproducible() {
        let run = || {
            let mut eng = Engine::new(10e-6);
            eng.set_elements(&demo_circuit(true));
            eng.advance(1000);
            eng.state_hash()
        };
        assert_eq!(run(), run());
    }
    // ------------------------------------------------------------ islands

    fn dc(v: f64) -> ElementKind {
        ElementKind::VoltageSource {
            wave: crate::netlist::Wave::Sine,
            dc: v,
            amp: 0.0,
            hz: 0.0,
            phase: 0.0,
        }
    }

    /// A 9 V divider (1k + 3k) drawn `x` units to the right of the origin,
    /// with ids based at `id`. Two of these share nothing but ground.
    fn board(id: u32, x: i32) -> Vec<ElementSpec> {
        vec![
            ElementSpec::two(id, dc(9.0), (x, 0), (x, 8)),
            ElementSpec::two(
                id + 1,
                ElementKind::Resistor { ohms: 1000.0 },
                (x, 0),
                (x + 4, 0),
            ),
            ElementSpec::two(
                id + 2,
                ElementKind::Resistor { ohms: 3000.0 },
                (x + 4, 0),
                (x, 8),
            ),
            ElementSpec::ground(id + 3, (x, 8)),
        ]
    }

    /// Solving islands (the ones with unknowns), in island order.
    fn solving(eng: &Engine) -> Vec<&Island> {
        eng.islands().iter().filter(|i| i.unknowns() > 0).collect()
    }

    /// Two boards that share only ground are two independent systems, and
    /// each one solves exactly what it would solve alone.
    #[test]
    fn boards_that_share_only_ground_are_separate_islands() {
        let mut both = Vec::new();
        both.extend(board(1, 0));
        both.extend(board(10, 100));
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&both);
        eng.advance(10);

        let isl = solving(&eng);
        assert_eq!(isl.len(), 2, "one island per board");
        // Ground is a shared reference, not a coupling: nothing joins them.
        for i in &isl {
            assert_eq!(i.unknowns(), 3, "2 nodes + 1 source branch per board");
        }
        // The totals are exactly what one big matrix would have had — the
        // work simply stopped being one matrix.
        assert_eq!(eng.unknowns(), 6);
        assert_eq!(eng.node_count(), 4);
        assert_eq!(eng.branch_count(), 2);

        // ...and both boards read like the single-board circuit does alone.
        let mut alone = Engine::new(10e-6);
        alone.set_elements(&board(1, 0));
        alone.advance(10);
        for id in [1u32, 10] {
            let v = eng.pin_voltage(id + 1, 1).unwrap();
            assert_eq!(v, alone.pin_voltage(2, 1).unwrap(), "board {id} divider");
            assert!((v - 6.75).abs() < 1e-6, "{v}");
        }
    }

    /// Ideal-constraint merging must be PER ISLAND, and this is the test that
    /// says so.
    ///
    /// Merging groups two zero-impedance constraints that reduce to the same
    /// canonical form onto ONE branch row — two 5 V supplies on one node are
    /// one net, not two duplicate rows. The key is built from node indices,
    /// and islands renumber their nodes from 1 each, so node 1 exists in
    /// every island. Group per document instead of per island and two
    /// unrelated 5 V supplies on two unrelated boards produce the same key
    /// and get aliased onto one branch row spanning two matrices: a corrupt
    /// system, and a silent one — the LU still factors, the numbers are just
    /// wrong.
    ///
    /// So: two identical boards, drawn far apart, sharing nothing but ground.
    /// Each must own its own branch unknown and its own full source current.
    #[test]
    fn two_identical_boards_do_not_share_one_branch_row() {
        let mut world = Vec::new();
        world.extend(board(1, 0));
        world.extend(board(10, 100));
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&world);
        eng.advance(50);

        // Two islands, one branch each. A cross-island merge would show up
        // here as one island holding two branches and the other none — or,
        // worse, as the right shape with the wrong currents.
        let isl = solving(&eng);
        assert_eq!(isl.len(), 2);
        for i in &isl {
            assert_eq!(i.branch_count(), 1, "each board owns its own source row");
        }
        assert_eq!(eng.branch_count(), 2);

        // The physics: 9 V through 1k + 3k is 2.25 mA, out of EACH supply.
        // A merged pair would report half of one total apiece — the exact
        // symptom `share_n` produces when it groups across a partition.
        for id in [1u32, 10] {
            let i = eng.pin_current(id, 0).unwrap();
            assert!(
                (i + 9.0 / 4000.0).abs() < 1e-9,
                "board {id} source current {i}, expected {}",
                -9.0 / 4000.0
            );
        }

        // ...and each board reads exactly what it reads alone, bit for bit.
        let mut alone = Engine::new(10e-6);
        alone.set_elements(&board(1, 0));
        alone.advance(50);
        for id in [1u32, 10] {
            assert_eq!(
                eng.pin_current(id, 0).unwrap(),
                alone.pin_current(1, 0).unwrap(),
                "board {id} is not the same circuit it is on its own"
            );
        }
    }

    /// The other half: merging still HAPPENS, inside one island. Two 9 V
    /// supplies across one node pair are one net and one row, which is what
    /// makes two-way lighting placeable — partitioning must not have thrown
    /// that away by scoping the grouping too tightly.
    #[test]
    fn identical_supplies_on_one_board_still_merge() {
        let elems = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 8)),
            ElementSpec::two(2, dc(9.0), (0, 0), (0, 8)),
            ElementSpec::two(3, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(50);
        assert!(!eng.is_quarantined(), "duplicate ideal supplies must merge");
        let isl = solving(&eng);
        assert_eq!(isl.len(), 1);
        assert_eq!(isl[0].branch_count(), 1, "one net, one row");
        // 9 mA total, split symmetrically because the solver cannot pick the
        // split and permutation is the only symmetry the situation has.
        let (a, b) = (
            eng.pin_current(1, 0).unwrap(),
            eng.pin_current(2, 0).unwrap(),
        );
        assert!((a + b + 9e-3).abs() < 1e-9, "total {a} + {b}");
        assert_eq!(a, b, "the split is symmetric");
    }

    /// The point of per-island quarantine: an unsolvable build takes itself
    /// out, and the room keeps running around it.
    #[test]
    fn a_diverging_island_does_not_quarantine_the_room() {
        // An ideal 9 V source straight across an ideal LED — singular, and
        // the pre-partition engine used to freeze the whole world on it.
        let mut world = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Led { color: 0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Wire, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        world.extend(board(10, 100));
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&world);
        let r = eng.advance(200);

        assert!(eng.is_quarantined(), "the ideal loop must give up");
        assert_eq!(eng.quarantined_islands(), 1, "and only that one");
        assert!(r.quarantined);
        // The healthy board ran the whole time, and the world clock with it.
        assert_eq!(r.steps, 200);
        assert!((eng.time() - 200.0 * 10e-6).abs() < 1e-12);
        let v = eng.pin_voltage(11, 1).unwrap();
        assert!((v - 6.75).abs() < 1e-6, "healthy board divider {v}");
        assert!((eng.pin_current(11, 0).unwrap() - 9.0 / 4000.0).abs() < 1e-9);
    }

    /// The global `linear` flag was worth 60-95% of a substep: one diode
    /// anywhere re-stamped and re-factored the entire world on every NR
    /// iteration. It is per island now.
    #[test]
    fn a_diode_next_door_costs_a_linear_board_nothing() {
        let mut world = vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 330.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        world.extend(board(10, 100));
        let mut eng = Engine::new(10e-6);
        // This is a statement about the per-island `linear` flag, so it is
        // measured with the work-skipping levers off — otherwise the LED
        // board's dt dilation (which is a *different* win, measured in its
        // own tests) shows up as "fewer factorizations" here and hides the
        // one being asserted.
        eng.set_tuning(Tuning::off());
        eng.set_elements(&world);
        eng.advance(200);

        let isl = solving(&eng);
        assert_eq!(isl.len(), 2);
        assert!(!isl[0].is_linear(), "the LED board is nonlinear");
        assert!(isl[1].is_linear(), "the resistor board is not");
        assert!(
            isl[0].factorizations() >= 200,
            "the nonlinear board refactors every iteration: {}",
            isl[0].factorizations()
        );
        assert!(
            isl[1].factorizations() <= 4,
            "the linear board must factor once per event, not per iteration: {}",
            isl[1].factorizations()
        );
    }

    /// The contract `crates/server` needs to put rayon over the islands:
    /// stepping them in any order, through the public plan API, produces the
    /// same bits as the serial `advance()`.
    #[test]
    fn stepping_islands_out_of_order_is_bit_identical() {
        let mut world = Vec::new();
        for (k, x) in [0, 100, 200, 300].iter().enumerate() {
            world.extend(board(1 + 10 * k as u32, *x));
        }
        world.push(ElementSpec::two(
            99,
            ElementKind::Capacitor { farads: 1e-6 },
            (4, 0),
            (0, 8),
        ));

        let mut serial = Engine::new(10e-6);
        serial.set_elements(&world);
        let mut scattered = Engine::new(10e-6);
        scattered.set_elements(&world);

        for _ in 0..20 {
            serial.advance(50);
            // What a parallel scheduler does: take the plan, step the
            // islands in whatever order it likes, commit the clock.
            let (t0, dt, islands) = scattered.step_plan();
            let mut steps = 0;
            for island in islands.iter_mut().rev() {
                steps = steps.max(island.advance(t0, dt, 50).steps);
            }
            scattered.commit_advance(steps);
        }
        assert_eq!(
            serial.state_hash(),
            scattered.state_hash(),
            "island order must not change one bit"
        );
        assert_eq!(serial.time(), scattered.time());
    }

    // ==================================================================
    // Quiescence and local dt: the two multipliers on islands.
    // ==================================================================

    /// A DC board settles, goes static, and then costs nothing at all —
    /// while still reporting the exact divider voltage the solver produced.
    /// Skipping arithmetic is not the same as inventing a number.
    #[test]
    fn a_settled_dc_board_goes_static_and_still_tells_the_truth() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&board(1, 0));
        // 10 ms of quiet is the default hold; 25 ms is plenty for a purely
        // resistive divider, which is at its DC answer after one substep.
        let r = eng.advance(1250);
        assert_eq!(r.steps, 1250, "the world clock is not slowed by sleeping");
        assert_eq!(eng.static_islands(), 1, "the board went static");
        assert!(solving(&eng)[0].is_static());

        let v = eng.pin_voltage(2, 1).unwrap();
        assert!((v - 6.75).abs() < 1e-6, "held divider voltage {v}");
        let facs = eng.factorizations();

        // A whole tick of a static world: no solver work, no clock stall,
        // no drift in what it reports.
        let r = eng.advance(1667);
        assert_eq!(r.steps, 1667);
        assert_eq!(r.islands, 0, "no island ran the solver");
        assert_eq!(r.static_islands, 1);
        assert_eq!(r.nr_iters, 0);
        assert_eq!(
            eng.factorizations(),
            facs,
            "a sleeping island cannot factor"
        );
        assert_eq!(
            eng.pin_voltage(2, 1).unwrap(),
            v,
            "held state is held exactly"
        );
        assert!((eng.time() - 2917.0 * 20e-6).abs() < 1e-9);
    }

    /// The wake path that matters most: a player flips a switch on a board
    /// that has been asleep for a minute, and the lamp lights on the very
    /// next substep — not after the hold window expires again.
    #[test]
    fn a_static_board_wakes_immediately_on_a_switch_flip() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&demo_circuit(false));
        eng.advance(5000);
        assert_eq!(eng.static_islands(), 1, "an open switch is a static board");
        assert!(eng.pin_current(4, 0).unwrap().abs() < 1e-9, "dark");

        eng.interact(3, InteractOp::SetSwitch { closed: true });
        assert_eq!(eng.static_islands(), 0, "an edit wakes the island");
        eng.advance(1);
        let i = eng.pin_current(4, 0).unwrap();
        assert!((i - 0.1).abs() < 1e-6, "lit within one substep: {i}");
        assert_eq!(solving(&eng)[0].local_dt_k(), 1, "and back at the room dt");
    }

    /// The other wake path: a machine writing a parameter at kHz rates does
    /// not go through `compile()`, so it has to wake the island itself.
    #[test]
    fn a_param_write_wakes_a_static_island() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&vec![
            ElementSpec::two(1, dc(9.0), (0, 0), (0, 8)),
            ElementSpec::three(
                2,
                ElementKind::Potentiometer {
                    ohms: 1000.0,
                    wiper: 0.3,
                },
                (0, 0),
                (4, 0),
                (0, 8),
            ),
            ElementSpec::ground(3, (0, 8)),
        ]);
        eng.advance(2000);
        assert_eq!(eng.static_islands(), 1);
        assert!(eng.write_param(2, ParamWrite::Wiper { frac: 0.8 }));
        assert_eq!(eng.static_islands(), 0, "the write woke it");
        eng.advance(1);
        let v = eng.pin_voltage(2, 1).unwrap();
        assert!((v - 1.8).abs() < 0.01, "wiper written to 0.8: {v}");
    }

    /// The self-check the baseline sweep relies on: anything that keeps
    /// moving must read as moving. An AC island is also structurally barred
    /// from sleeping, because its equations depend on `t`.
    #[test]
    fn an_ac_island_is_never_declared_static() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&vec![
            ElementSpec::two(
                1,
                ElementKind::VoltageSource {
                    wave: crate::netlist::Wave::Sine,
                    dc: 0.0,
                    amp: 10.0,
                    hz: 60.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 8),
            ),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (0, 8)),
            ElementSpec::ground(3, (0, 8)),
        ]);
        assert!(
            !solving(&eng)[0].is_sleepable(),
            "a time-varying source bars sleep"
        );
        for _ in 0..50 {
            eng.advance(1000);
            assert_eq!(eng.static_islands(), 0, "an AC island never sleeps");
        }
        // ...and it is never integrated at fewer than the guaranteed
        // samples per cycle, whatever the error controller thinks.
        let k = solving(&eng)[0].local_dt_k();
        assert!(
            (k as f64) * 20e-6 <= 1.0 / (60.0 * 64.0) + 1e-12,
            "60 Hz island dilated to k={k}"
        );
    }

    /// The trap the measured criterion alone walks into. This board's
    /// unknowns move far less than 1 uV per substep — it passes the slew
    /// test from the first step — but it is crawling towards a DC point
    /// volts away, and freezing it would hold a number that is simply
    /// wrong. The window-drift guard is what catches it.
    #[test]
    fn a_slow_ramp_is_not_mistaken_for_static() {
        // 10 V through 10 MOhm into 10 uF: tau = 100 s. Per 20 us substep
        // the cap moves 10 V * 20e-6 / 100 = 2 nV, five hundred times under
        // the slew threshold.
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 10e6 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Capacitor { farads: 10e-6 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ]);
        for _ in 0..100 {
            eng.advance(1000);
            assert_eq!(
                eng.static_islands(),
                0,
                "a circuit still travelling volts must not be frozen"
            );
        }
        // It really was moving, and it really was moving slowly.
        let v = eng.pin_voltage(3, 0).unwrap();
        // 10 V through tau = 100 s, 2 s in: 10*(1 - e^-0.02) = 0.198 V.
        assert!((0.15..0.25).contains(&v), "slow ramp reached {v} V in 2 s");
    }

    /// The trap the drift guard alone walks into, ten million substeps
    /// later. The guard bounds the travel per 10 ms window at 1 uV, which
    /// is a bound of `1 uV/10 ms x tau` = 1e-4 V/s x tau on the travel a
    /// first-order tail still has LEFT — 1 mV at tau = 10 s, 10 mV at
    /// tau = 100 s. That residual is permanent: the island sleeps and the
    /// number never moves again, so a probe on it reads a lie forever.
    ///
    /// Measured before the fix, this exact circuit slept at t = 93 s
    /// holding 9.999025 V against a closed form of 10.000000 V and stayed
    /// there. The reason no existing test caught it: the trap is 4.6
    /// MILLION substeps in, and the rest of the suite runs 0.2 s of sim
    /// time.
    ///
    /// 1 kOhm into 10 mF (not 1 MOhm into 10 uF) for the same tau = 10 s,
    /// so that GMIN's 1e-12 S leak to ground shifts the DC point by 1e-9
    /// relative instead of 1e-6 and the closed form is the whole answer.
    #[test]
    fn a_long_tail_never_freezes_short_of_the_truth() {
        let dt = 20e-6;
        let tau = 10.0;
        let mut eng = Engine::new(dt);
        eng.set_elements(&vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Capacitor { farads: 10e-3 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ]);
        // Staleness is the only budget this trajectory can spend: it is
        // smooth, so its truncation error is orders below.
        let budget = 1.5 * eng.tuning().local_dt_slew * dt;
        // 300 s of sim time = 15 million substeps. The old criterion froze
        // at 93 s; the sleep that is actually justified lands at 161 s.
        let mut slept_at = None;
        for _ in 0..300 {
            eng.advance(50_000);
            let (t, v) = (eng.time(), eng.pin_voltage(3, 0).unwrap());
            let exact = 10.0 * (1.0 - libm::exp(-t / tau));
            assert!(
                (v - exact).abs() < budget,
                "t={t:.3} s: held {v} against a closed form of {exact} \
                 ({:.3e} over a budget of {budget:.3e})",
                (v - exact).abs()
            );
            if slept_at.is_none() && eng.static_islands() > 0 {
                slept_at = Some(t);
            }
        }
        // And when it does finally sleep, it sleeps AT the answer: the
        // settle test only lets it go once the travel it has LEFT is inside
        // the `quiescence_drift` budget. Measured: it sleeps at t = 161 s
        // holding 9.99999895 V, 1.05e-6 short of 10 V against a 1e-6
        // budget — the 5% excess is the estimator's own resolution slack,
        // and 1.05 uV is what a probe on this island reads forever.
        let t = slept_at.expect("a tail that has finished must sleep eventually");
        let v = eng.pin_voltage(3, 0).unwrap();
        assert!(
            (v - 10.0).abs() < 2.0 * eng.tuning().quiescence_drift,
            "slept at t={t:.1} s holding {v}, {:.3e} short of 10 V",
            (v - 10.0).abs()
        );

        // ...and the same circuit under the criterion that shipped before
        // the fix — the drift window with no decay test behind it — really
        // does walk into the trap, so this test is measuring the guard and
        // not the weather.
        // "any decay at all will do" — which is what no decay test means.
        let broken = Tuning {
            quiescence_decay: 1.0,
            ..Tuning::default()
        };
        let mut eng = Engine::new(dt);
        eng.set_tuning(broken);
        eng.set_elements(&vec![
            ElementSpec::two(1, dc(10.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Capacitor { farads: 10e-3 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ]);
        for _ in 0..300 {
            eng.advance(50_000);
        }
        let v = eng.pin_voltage(3, 0).unwrap();
        assert!(
            eng.static_islands() == 1 && (v - 10.0).abs() > 5e-4,
            "the pre-fix criterion no longer reproduces the freeze ({v} V), \
             so this test proves nothing"
        );
    }

    /// The same freeze, in the dimension the volt thresholds never covered.
    ///
    /// A branch current is an AMP, and every quiescence threshold was a
    /// VOLT applied to the whole unknown vector. This island's node voltage
    /// is a rock-steady 50 uV while the source's branch current ramps
    /// forever at 5e-5 A/s — a ramp the volt-shaped slew test reads as a
    /// 50 nV/s crawl and waves through. It slept in the first 60 ms holding
    /// 2.5 uA and was 1 mA off trajectory 20 s later.
    #[test]
    fn a_ramping_branch_current_is_never_declared_static() {
        let dt = 20e-6;
        let mut eng = Engine::new(dt);
        eng.set_elements(&vec![
            // 50 uV across 1 H: di/dt = V/L = 5e-5 A/s, exactly.
            ElementSpec::two(1, dc(50e-6), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Inductor { henries: 1.0 }, (0, 0), (0, 8)),
            ElementSpec::ground(3, (0, 8)),
        ]);
        for _ in 0..1000 {
            eng.advance(1_000);
            assert_eq!(
                eng.static_islands(),
                0,
                "a branch current still ramping must not be frozen at t={}",
                eng.time()
            );
        }
        let (t, i) = (eng.time(), eng.pin_current(2, 0).unwrap());
        assert!(
            (i - 5e-5 * t).abs() < 1e-9,
            "inductor current {i} A at t={t} s, expected {}",
            5e-5 * t
        );

        // The reproduction, kept: put the volt thresholds back on the amp
        // unknowns (and take the decay test away) and the island freezes in
        // the first 60 ms holding ~2.5 uA, 1 mA off trajectory by t = 20 s.
        let d = Tuning::default();
        let broken = Tuning {
            quiescence_decay: 1.0,
            quiescence_slew_i: d.quiescence_slew,
            quiescence_drift_i: d.quiescence_drift,
            ..d
        };
        let mut eng = Engine::new(dt);
        eng.set_tuning(broken);
        eng.set_elements(&vec![
            ElementSpec::two(1, dc(50e-6), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Inductor { henries: 1.0 }, (0, 0), (0, 8)),
            ElementSpec::ground(3, (0, 8)),
        ]);
        for _ in 0..1000 {
            eng.advance(1_000);
        }
        let i = eng.pin_current(2, 0).unwrap();
        assert!(
            eng.static_islands() == 1 && (i - 5e-5 * eng.time()).abs() > 1e-4,
            "the pre-fix thresholds no longer reproduce the frozen ramp \
             ({i} A), so this test proves nothing"
        );
    }

    /// Local dt: a slow island earns a bigger step, and gives it back the
    /// instant something happens. The world clock is unaffected either way.
    #[test]
    fn local_dt_dilates_a_slow_island_and_collapses_on_a_transient() {
        // 5 V into 1 kOhm / 100 uF: tau = 100 ms. The step may only grow
        // once the island is slow enough that the lag it buys is under
        // `local_dt_slew * dt` — 100 uV at this dt — which for this tail is
        // 0.3 s in, NOT the 40 ms the curvature test alone would have
        // allowed. That difference is the whole of defect 2: at 40 ms this
        // cap is still moving at 34 V/s, and a step that ends 20 us of world
        // time early reports 0.7 mV of pure staleness.
        let ramp = vec![
            ElementSpec::two(1, dc(5.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Capacitor { farads: 100e-6 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&ramp);
        eng.advance(2000);
        assert_eq!(
            solving(&eng)[0].local_dt_k(),
            1,
            "a cap moving at 34 V/s must not be dilated"
        );
        eng.advance(23_000); // out to 0.5 s: 50 e^-5 = 0.34 V/s
        let k = solving(&eng)[0].local_dt_k();
        assert!(k > 1, "a slow RC tail must earn a bigger step, got k={k}");

        // The same trajectory at the room dt, to within the error budget
        // the controller was told it could spend: the truncation budget
        // plus the staleness ceiling, and nothing else.
        let mut ref_eng = Engine::new(20e-6);
        ref_eng.set_tuning(Tuning::off());
        ref_eng.set_elements(&ramp);
        ref_eng.advance(25_000);
        assert_eq!(eng.time(), ref_eng.time(), "same world time either way");
        let (a, b) = (
            eng.pin_voltage(3, 0).unwrap(),
            ref_eng.pin_voltage(3, 0).unwrap(),
        );
        let budget = eng.tuning().local_dt_slew * eng.dt() + eng.tuning().local_dt_err;
        assert!(
            (a - b).abs() < budget,
            "dilated {a} vs room-dt {b}: {} over a budget of {budget}",
            (a - b).abs()
        );

        // A perturbation puts it straight back on the room dt.
        eng.interact(1, InteractOp::SetValue { value: 9.0 });
        assert_eq!(solving(&eng)[0].local_dt_k(), 1);
    }

    /// An island somebody is watching through an instrument is never
    /// dilated: a probe must see the waveform on the grid it sampled, not
    /// on a coarsened one.
    #[test]
    fn a_sampled_island_is_never_dilated() {
        let ramp = vec![
            ElementSpec::two(1, dc(5.0), (0, 0), (0, 8)),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            ElementSpec::two(3, ElementKind::Capacitor { farads: 100e-6 }, (4, 0), (0, 8)),
            ElementSpec::ground(4, (0, 8)),
        ];
        let mut eng = Engine::new(20e-6);
        eng.set_sampled(&[3]);
        eng.set_elements(&ramp);
        eng.advance(2000);
        assert!(solving(&eng)[0].is_pinned());
        assert_eq!(
            solving(&eng)[0].local_dt_k(),
            1,
            "a probed island stays at the room dt"
        );
        // The declaration survives an edit, because the partition does not.
        eng.interact(2, InteractOp::SetValue { value: 2000.0 });
        eng.advance(2000);
        assert_eq!(solving(&eng)[0].local_dt_k(), 1);
    }

    /// Nothing an island does to its own step size may move the world
    /// clock, and nothing may leave it more than one local step behind.
    #[test]
    fn dt_dilation_never_desynchronises_the_world() {
        let mut world = Vec::new();
        world.extend(board(1, 0));
        world.extend(vec![
            ElementSpec::two(10, dc(5.0), (100, 0), (100, 8)),
            ElementSpec::two(
                11,
                ElementKind::Resistor { ohms: 1000.0 },
                (100, 0),
                (104, 0),
            ),
            ElementSpec::two(
                12,
                ElementKind::Capacitor { farads: 100e-6 },
                (104, 0),
                (100, 8),
            ),
            ElementSpec::ground(13, (100, 8)),
        ]);
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&world);
        // Advance in ragged chunks that do not divide any k, which is the
        // case the pending-debt carry exists for.
        let mut expect = 0u32;
        for c in 1..=300u32 {
            let n = 1 + c % 7;
            let r = eng.advance(n);
            assert_eq!(r.steps, n, "every substep handed in is accounted for");
            expect += n;
        }
        assert!(
            (eng.time() - expect as f64 * 20e-6).abs() < 1e-9,
            "world clock {} vs {} substeps",
            eng.time(),
            expect
        );
        // Staleness bound: the debt an island may carry is under one local
        // step, and a local step is capped by Tuning::local_dt_max.
        for i in solving(&eng) {
            assert!(
                (i.local_dt_k() as f64) * 20e-6 <= eng.tuning().local_dt_max + 1e-12,
                "island h exceeds the staleness ceiling"
            );
        }
    }

    /// The interaction guard: with both levers on, every island's answer
    /// still agrees with the one the plain fixed-dt engine computes, and
    /// the two engines agree about what time it is.
    #[test]
    fn both_levers_together_do_not_change_what_a_player_sees() {
        let mut world = Vec::new();
        world.extend(board(1, 0)); // static resistive divider
        world.extend(vec![
            // slow RC, a dt-dilation case
            ElementSpec::two(10, dc(5.0), (100, 0), (100, 8)),
            ElementSpec::two(
                11,
                ElementKind::Resistor { ohms: 1000.0 },
                (100, 0),
                (104, 0),
            ),
            ElementSpec::two(
                12,
                ElementKind::Capacitor { farads: 100e-6 },
                (104, 0),
                (100, 8),
            ),
            ElementSpec::ground(13, (100, 8)),
        ]);
        world.extend(vec![
            // an LED board: nonlinear, settles
            ElementSpec::two(20, dc(9.0), (200, 0), (200, 8)),
            ElementSpec::two(
                21,
                ElementKind::Resistor { ohms: 330.0 },
                (200, 0),
                (204, 0),
            ),
            ElementSpec::two(22, ElementKind::Led { color: 0 }, (204, 0), (200, 8)),
            ElementSpec::ground(23, (200, 8)),
        ]);
        let mut fast = Engine::new(20e-6);
        fast.set_elements(&world);
        let mut slow = Engine::new(20e-6);
        slow.set_tuning(Tuning::off());
        slow.set_elements(&world);
        for _ in 0..40 {
            fast.advance(250);
            slow.advance(250);
        }
        assert_eq!(fast.time(), slow.time());
        assert!(
            fast.static_islands() > 0,
            "something should have gone static"
        );
        for f in fast.frame() {
            let s = slow.frame().into_iter().find(|s| s.id == f.id).unwrap();
            for p in 0..f.npins {
                assert!(
                    (f.v[p] - s.v[p]).abs() < 1e-3,
                    "elem {} pin {p}: levers {} vs plain {}",
                    f.id,
                    f.v[p],
                    s.v[p]
                );
                assert!(
                    (f.i[p] - s.i[p]).abs() < 1e-5,
                    "elem {} pin {p} current: levers {} vs plain {}",
                    f.id,
                    f.i[p],
                    s.i[p]
                );
            }
        }
    }

    /// Skipping and dilating are per island, exactly like quarantine: a
    /// static neighbour must not slow down, speed up, or freeze the one
    /// next to it.
    #[test]
    fn one_island_sleeping_does_not_touch_the_next() {
        let mut world = Vec::new();
        world.extend(board(1, 0)); // resistive: goes static
        world.extend(vec![
            // 1 kHz AC: structurally barred from sleeping.
            ElementSpec::two(
                10,
                ElementKind::VoltageSource {
                    wave: crate::netlist::Wave::Sine,
                    dc: 0.0,
                    amp: 5.0,
                    hz: 1000.0,
                    phase: 0.0,
                },
                (100, 0),
                (100, 8),
            ),
            ElementSpec::two(
                11,
                ElementKind::Resistor { ohms: 1000.0 },
                (100, 0),
                (100, 8),
            ),
            ElementSpec::ground(12, (100, 8)),
        ]);
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&world);
        eng.advance(2000);
        assert_eq!(eng.static_islands(), 1, "exactly one of the two sleeps");

        // The AC island keeps producing its waveform while its neighbour is
        // asleep, and it matches the same island simulated on its own.
        let mut alone = Engine::new(20e-6);
        alone.set_elements(&world[4..].to_vec());
        alone.advance(2000);
        assert_eq!(
            eng.pin_voltage(11, 0).unwrap(),
            alone.pin_voltage(11, 0).unwrap(),
            "a sleeping neighbour changed the live island's numbers"
        );
    }
}
