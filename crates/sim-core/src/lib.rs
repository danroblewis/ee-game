//! sim-core: the authoritative circuit simulation. Pure computation — no
//! I/O, no threads, no clocks — so it compiles bit-identically for the
//! native server and the wasm32 client preview.

pub mod constraint;
mod engine;
mod netlist;
pub mod validate;

pub use constraint::{constraint_of, Constraint, ConstraintKey};
pub use engine::{AdvanceReport, ElemFrame, ElemTap, Engine, GMIN};
pub use netlist::{
    photocell_ohms, DocOp, ElementKind, ElementSpec, InteractOp, ParamWrite, Point,
    DEFAULT_OPAMP_ISC, MAX_PINS, MAX_TIER,
};
pub use validate::{check_document, Reject, SmallIds};

#[cfg(test)]
mod tests {
    use super::*;

    /// Battery -> switch -> lamp loop: the M1 demo circuit.
    fn demo_circuit(closed: bool) -> Vec<ElementSpec> {
        let dc9 = ElementKind::VoltageSource {
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
}
