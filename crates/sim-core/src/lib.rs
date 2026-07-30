//! sim-core: the authoritative circuit simulation. Pure computation — no
//! I/O, no threads, no clocks — so it compiles bit-identically for the
//! native server and the wasm32 client preview.

mod engine;
mod netlist;

pub use engine::{AdvanceReport, ElemFrame, Engine, GMIN};
pub use netlist::{DocOp, ElementKind, ElementSpec, InteractOp, ParamWrite, Point, MAX_PINS};

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
