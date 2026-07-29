//! The schematic document model the engine compiles from.
//!
//! Elements are N-terminal parts placed on an integer grid. Coincident
//! endpoints are electrically connected; `Wire` elements additionally merge
//! their two endpoints into one node (Falstad-style wire closure), so wires
//! carry current but add no unknowns.

pub type Point = (i32, i32);

/// Largest pin count of any element (the 555 timer is 6-pin).
pub const MAX_PINS: usize = 6;

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
    /// A loudspeaker: electrically just its voice-coil resistance, so it is
    /// stamped exactly like a `Resistor`. `ohms` is the nominal impedance
    /// (8 Ω typical). Nothing here makes sound — the client's "listen"
    /// probe plays the solver's own node waveform through WebAudio, so what
    /// you hear is the simulation, not a sound effect.
    Speaker {
        ohms: f64,
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
    /// Momentary pushbutton: electrically a switch, but the client only
    /// holds it closed while the pointer is down (`SetSwitch`).
    Button {
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
    /// Operational transconductance amplifier (LM13700-style).
    /// Pins: [in+, in-, out, bias]. The bias pin is a diode junction to
    /// ground; the current injected into it sets Iabc, and the output
    /// sources Iout = Iabc * tanh(vd / 2Vt) — a current, not a voltage.
    /// gm = Iabc / 2Vt, output current saturates at ±Iabc.
    Ota,
    /// Bipolar 555 timer (RESET tied high, CTRL left at the internal
    /// divider). Pins: [vcc, gnd, trig, thr, out, dis].
    ///
    /// The internal divider sets the comparator thresholds at 1/3 and 2/3
    /// of the LIVE supply; an RS latch (discrete state) is set when
    /// v(trig) < vcc/3 and reset when v(thr) > 2·vcc/3, trigger winning.
    /// OUT is a totem-pole branch source: vcc - 1.2 V sourced from the
    /// VCC pin when the latch is high, 0.1 V sunk into GND when low. DIS
    /// is a saturated transistor to GND (10 Ω) while the latch is low and
    /// open otherwise. The supply pins draw ~3 mA of quiescent current.
    Timer555,
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
            Timer555 => 6,
            Ota => 4,
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
                | ElementKind::Button { closed: true }
                | ElementKind::OpAmp { .. }
                | ElementKind::Timer555
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
                | ElementKind::Ota
                | ElementKind::Timer555
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
