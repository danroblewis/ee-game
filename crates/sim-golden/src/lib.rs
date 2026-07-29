//! Golden circuits: known-good netlists with closed-form expected
//! behavior. The library exposes builders so the determinism harness, the
//! benchmarks, and the tests all exercise the exact same circuits.

use sim_core::{ElementKind, ElementSpec, Point};

pub fn spec(id: u32, kind: ElementKind, a: Point, b: Point) -> ElementSpec {
    ElementSpec { id, kind, a, b }
}

pub fn dc(volts: f64) -> ElementKind {
    ElementKind::VoltageSource {
        dc: volts,
        amp: 0.0,
        hz: 0.0,
        phase: 0.0,
    }
}

pub fn sine(amp: f64, hz: f64) -> ElementKind {
    ElementKind::VoltageSource {
        dc: 0.0,
        amp,
        hz,
        phase: 0.0,
    }
}

/// 10 V source, 1 kΩ, 1 µF: v_c(t) = 10 (1 - e^(-t/τ)), τ = 1 ms.
pub fn rc_step() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(10.0), (0, 0), (0, 8)),
        spec(2, ElementKind::Resistor { ohms: 1000.0 }, (0, 0), (8, 0)),
        spec(3, ElementKind::Capacitor { farads: 1e-6 }, (8, 0), (0, 8)),
        spec(4, ElementKind::Ground, (0, 8), (0, 8)),
    ]
}

/// 5 V source, 100 Ω, 10 mH: i(t) = 0.05 (1 - e^(-t/τ)), τ = 100 µs.
pub fn rl_step() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(5.0), (0, 0), (0, 8)),
        spec(2, ElementKind::Resistor { ohms: 100.0 }, (0, 0), (8, 0)),
        spec(3, ElementKind::Inductor { henries: 10e-3 }, (8, 0), (0, 8)),
        spec(4, ElementKind::Ground, (0, 8), (0, 8)),
    ]
}

/// Lightly damped series RLC (1 Ω, 1 mH, 1 µF) driven by a 1 V step:
/// f0 ≈ 5.03 kHz, Q ≈ 31.6.
pub fn rlc_ring() -> Vec<ElementSpec> {
    vec![
        spec(1, dc(1.0), (0, 0), (0, 8)),
        spec(2, ElementKind::Resistor { ohms: 1.0 }, (0, 0), (4, 0)),
        spec(3, ElementKind::Inductor { henries: 1e-3 }, (4, 0), (8, 0)),
        spec(4, ElementKind::Capacitor { farads: 1e-6 }, (8, 0), (0, 8)),
        spec(5, ElementKind::Ground, (0, 8), (0, 8)),
    ]
}

/// Half-wave rectifier: 10 V / 60 Hz sine, diode, 1 kΩ ∥ 100 µF load.
pub fn half_wave_rectifier() -> Vec<ElementSpec> {
    vec![
        spec(1, sine(10.0, 60.0), (0, 0), (0, 8)),
        spec(2, ElementKind::Diode, (0, 0), (8, 0)),
        spec(3, ElementKind::Resistor { ohms: 1000.0 }, (8, 0), (0, 8)),
        spec(4, ElementKind::Capacitor { farads: 100e-6 }, (8, 0), (0, 8)),
        spec(5, ElementKind::Ground, (0, 8), (0, 8)),
    ]
}

/// The M1 demo: battery -> switch -> lamp.
pub fn demo_lamp(closed: bool) -> Vec<ElementSpec> {
    vec![
        spec(1, dc(9.0), (0, 0), (0, 4)),
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
