//! sim-core: the authoritative circuit simulation. Pure computation — no
//! I/O, no threads, no clocks — so it compiles bit-identically for the
//! native server and the wasm32 client preview.

mod engine;
mod netlist;

pub use engine::{AdvanceReport, ElemFrame, Engine, GMIN};
pub use netlist::{ElementKind, ElementSpec, InteractOp, Point};

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: u32, kind: ElementKind, a: Point, b: Point) -> ElementSpec {
        ElementSpec { id, kind, a, b }
    }

    /// Battery -> switch -> lamp loop: the M1 demo circuit.
    fn demo_circuit(closed: bool) -> Vec<ElementSpec> {
        vec![
            spec(
                1,
                ElementKind::VoltageSource {
                    dc: 9.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 4),
            ),
            spec(2, ElementKind::Wire, (0, 0), (4, 0)),
            spec(3, ElementKind::Switch { closed }, (4, 0), (8, 0)),
            spec(
                4,
                ElementKind::Lamp {
                    ohms: 90.0,
                    rated_watts: 1.0,
                },
                (8, 0),
                (8, 4),
            ),
            spec(5, ElementKind::Wire, (8, 4), (0, 4)),
            spec(6, ElementKind::Ground, (0, 4), (0, 4)),
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
        assert!(
            (lamp.current - 0.1).abs() < 1e-6,
            "lamp current {}",
            lamp.current
        );
        assert!((lamp.power - 0.9).abs() < 1e-5, "lamp power {}", lamp.power);

        // Wire current recovered by KCL propagation matches the loop
        // current (direction depends on wire orientation).
        let wire = f.iter().find(|e| e.id == 2).unwrap();
        assert!(
            (wire.current.abs() - 0.1).abs() < 1e-6,
            "wire current {}",
            wire.current
        );
    }

    #[test]
    fn divider_is_exact() {
        // 10 V across 1k + 3k: midpoint at 7.5 V (a-side is +).
        let elems = vec![
            spec(
                1,
                ElementKind::VoltageSource {
                    dc: 10.0,
                    amp: 0.0,
                    hz: 0.0,
                    phase: 0.0,
                },
                (0, 0),
                (0, 8),
            ),
            spec(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (4, 0)),
            spec(3, ElementKind::Resistor { ohms: 3000.0 }, (4, 0), (0, 8)),
            spec(4, ElementKind::Ground, (0, 8), (0, 8)),
        ];
        let mut eng = Engine::new(10e-6);
        eng.set_elements(&elems);
        eng.advance(1);
        // Exact up to the GMIN leak (1e-12 S per node pulls ~nV-µV level).
        let v_mid = eng.voltage_at((4, 0)).unwrap();
        assert!((v_mid - 7.5).abs() < 1e-6, "divider mid {v_mid}");
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
