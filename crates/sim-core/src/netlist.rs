//! The schematic document model the engine compiles from.
//!
//! Elements are N-terminal parts placed on an integer grid. Coincident
//! endpoints are electrically connected; `Wire` elements additionally merge
//! their two endpoints into one node (Falstad-style wire closure), so wires
//! carry current but add no unknowns.

pub type Point = (i32, i32);

/// Largest pin count of any element (BJT/MOSFET/op-amp/pot are 3-pin).
pub const MAX_PINS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum ElementKind {
    Wire,
    /// Pins its single endpoint to node 0.
    Ground,
    Resistor {
        ohms: f64,
    },
    /// A resistor that renders as a lamp; glow is derived from dissipated
    /// power by the client. Electrically a resistor for now (filament R(T)
    /// arrives with the M6 damage/thermal pass).
    Lamp {
        ohms: f64,
        rated_watts: f64,
    },
    Capacitor {
        farads: f64,
    },
    Inductor {
        henries: f64,
    },
    /// v(pin0) - v(pin1) = dc + amp * sin(2*pi*hz*t + phase). Pin 0 is +.
    VoltageSource {
        dc: f64,
        amp: f64,
        hz: f64,
        phase: f64,
    },
    /// Constant current driven from pin 0 to pin 1 through the element.
    CurrentSource {
        amps: f64,
    },
    /// Closed switch stamps as a 0 V source (its branch current is an MNA
    /// unknown); open switch stamps nothing.
    Switch {
        closed: bool,
    },
    /// Shockley diode, anode = pin 0.
    Diode,
    /// Diode with reverse breakdown at `vz` volts (anode = pin 0).
    Zener {
        vz: f64,
    },
    /// Light-emitting diode (~2.1 V forward drop); the client renders glow
    /// from forward current against ~20 mA. `color` is render-only
    /// (0=red, 1=green, 2=blue, 3=yellow, 4=white).
    Led {
        color: u8,
    },
    /// Bipolar transistors, Ebers-Moll transport model.
    /// Pins: [base, collector, emitter].
    Npn {
        beta: f64,
    },
    Pnp {
        beta: f64,
    },
    /// Level-1 (Shichman-Hodges) MOSFETs. Pins: [gate, drain, source].
    /// `k` is the transconductance coefficient (A/V^2), `vt` the threshold.
    Nmos {
        vt: f64,
        k: f64,
    },
    Pmos {
        vt: f64,
        k: f64,
    },
    /// Ideal-ish op-amp: open-loop gain 1e5, output clamped to ±rail.
    /// Pins: [in+, in-, out]. Inputs draw no current.
    OpAmp {
        rail: f64,
    },
    /// Potentiometer. Pins: [end a, wiper, end b]; `wiper` in 0..1 is the
    /// fractional position from end a. `SetValue` moves the wiper.
    Potentiometer {
        ohms: f64,
        wiper: f64,
    },
}

impl ElementKind {
    pub fn pin_count(&self) -> usize {
        use ElementKind::*;
        match self {
            Ground => 1,
            Npn { .. }
            | Pnp { .. }
            | Nmos { .. }
            | Pmos { .. }
            | OpAmp { .. }
            | Potentiometer { .. } => 3,
            _ => 2,
        }
    }

    /// Devices whose branch current is an MNA unknown.
    pub fn is_branch(&self) -> bool {
        matches!(
            self,
            ElementKind::VoltageSource { .. }
                | ElementKind::Switch { closed: true }
                | ElementKind::OpAmp { .. }
        )
    }

    pub fn is_nonlinear(&self) -> bool {
        matches!(
            self,
            ElementKind::Diode
                | ElementKind::Zener { .. }
                | ElementKind::Led { .. }
                | ElementKind::Npn { .. }
                | ElementKind::Pnp { .. }
                | ElementKind::Nmos { .. }
                | ElementKind::Pmos { .. }
                | ElementKind::OpAmp { .. }
        )
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ElementSpec {
    pub id: u32,
    pub kind: ElementKind,
    pub pins: Vec<Point>,
}

impl ElementSpec {
    pub fn two(id: u32, kind: ElementKind, a: Point, b: Point) -> Self {
        debug_assert_eq!(kind.pin_count().min(2), 2);
        ElementSpec {
            id,
            kind,
            pins: vec![a, b],
        }
    }

    pub fn three(id: u32, kind: ElementKind, a: Point, b: Point, c: Point) -> Self {
        debug_assert_eq!(kind.pin_count(), 3);
        ElementSpec {
            id,
            kind,
            pins: vec![a, b, c],
        }
    }

    pub fn ground(id: u32, at: Point) -> Self {
        ElementSpec {
            id,
            kind: ElementKind::Ground,
            pins: vec![at],
        }
    }
}

/// Document edits: the schematic-drawing verbs. Applied server-side in
/// arrival order (M4 upgrades this to the full validated op pipeline).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum DocOp {
    Add {
        spec: ElementSpec,
    },
    Remove {
        id: u32,
    },
    Move {
        id: u32,
        pins: Vec<Point>,
    },
    /// Reconfigure a part's parameters (the properties-panel path). The
    /// new kind must keep the same pin count.
    SetKind {
        id: u32,
        kind: ElementKind,
    },
}

/// Interactions that mutate a live element without a recompile-worthy edit.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum InteractOp {
    SetSwitch {
        closed: bool,
    },
    /// Retarget the element's primary value (ohms, farads, henries, dc
    /// volts, amps, pot wiper) — the knob-drag path.
    SetValue {
        value: f64,
    },
}
