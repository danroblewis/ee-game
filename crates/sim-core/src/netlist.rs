//! The schematic document model the engine compiles from.
//!
//! Elements are N-terminal parts placed on an integer grid. Coincident
//! endpoints are electrically connected; `Wire` elements additionally merge
//! their two endpoints into one node (Falstad-style wire closure), so wires
//! carry current but add no unknowns.

pub type Point = (i32, i32);

/// Largest pin count of any element (the 555 timer is 6-pin).
pub const MAX_PINS: usize = 6;

/// Short-circuit output current a legacy (field-less) `OpAmp` deserialises
/// to: a 741/LM358-class jellybean. See `ElementKind::OpAmp`.
pub const DEFAULT_OPAMP_ISC: f64 = 0.025;

#[cfg(feature = "serde")]
fn default_opamp_isc() -> f64 {
    DEFAULT_OPAMP_ISC
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum ElementKind {
    #[default]
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
    /// Single-pin voltage rail: v(pin0) = dc + amp * sin(2*pi*hz*t + phase),
    /// referenced to ground. The return path is implicit — the branch current
    /// flows through node 0 — so a rail powers anything that has its own
    /// ground reference, without a wire back to a battery.
    Rail {
        dc: f64,
        amp: f64,
        hz: f64,
        phase: f64,
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
    /// Op-amp: open-loop gain 1e5, output clamped to ±`rail`, output
    /// current clamped to ±`isc`. Pins: [in+, in-, out]. Inputs draw no
    /// current.
    ///
    /// `isc` is the short-circuit output current, and it is the honest half
    /// of the model: a real op-amp's output stage folds back to `I_sc` and
    /// sits there indefinitely (741 ≈ 25 mA, TL07x/LM358 ≈ 40 mA), which is
    /// why shorting an op-amp's output does not destroy it. Without it the
    /// part is an unlimited current source and "op-amp straight into a
    /// motor" works, which it does not in the world.
    ///
    /// Old documents have no `isc` field; serde defaults them to the 741's
    /// 25 mA, which is what they were implicitly promising to be.
    OpAmp {
        rail: f64,
        #[cfg_attr(feature = "serde", serde(default = "default_opamp_isc"))]
        isc: f64,
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
    /// DC motor armature. Pins: [+, -]; the branch current is an unknown
    /// and is defined as the current INTO pin 0.
    /// Law: v(pin0) - v(pin1) = ohms·i + henries·di/dt + bemf.
    ///
    /// `bemf` is an INPUT to the electrical model: the mechanical side
    /// (rotor speed) lives outside sim-core and writes K·ω back through
    /// `Engine::write_param` every machine tick. sim-core owns no
    /// mechanical state — it only ever sees a voltage.
    Motor {
        ohms: f64,
        henries: f64,
        bemf: f64,
    },
    /// White-noise generator: an EMF of `volts` peak behind `ohms` of
    /// output resistance, redrawn once per timestep. Pins: [out, ref];
    /// pin 0 is +.
    ///
    /// The stream is a *pure function* of (`seed`, sample index) evaluated
    /// with integer arithmetic only, so it is bit-identical on native and
    /// wasm32, survives save/reload, and never touches a clock, an OS
    /// entropy source or the `rand` crate. Two sources with different
    /// seeds are independent; two with the SAME seed are the same signal.
    ///
    /// Uniform on [-volts, +volts): RMS = volts/sqrt(3) = 0.5774·volts,
    /// mean 0. One fresh sample per substep is a zero-order hold at the
    /// substep rate (50 kHz at the standard 20 µs dt), so the spectrum is
    /// flat to well past audio — the hiss is deliberately unfiltered.
    /// Band-limit it in the circuit (an RC pole, a gm-C filter), which is
    /// what a drum voice wants anyway and what keeps content out of the
    /// fold-back band above the audio tap's Nyquist.
    Noise {
        volts: f64,
        ohms: f64,
        seed: u32,
    },
}

impl ElementKind {
    pub fn pin_count(&self) -> usize {
        use ElementKind::*;
        match self {
            Ground | Rail { .. } => 1,
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
                | ElementKind::Rail { .. }
                | ElementKind::Switch { closed: true }
                | ElementKind::Button { closed: true }
                | ElementKind::OpAmp { .. }
                | ElementKind::Timer555
                | ElementKind::Motor { .. }
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

    /// The subset of `is_nonlinear` whose contribution to the MNA **matrix**
    /// is a function of a DISCRETE state (an op-amp's rail region, a 555's
    /// RS latch) rather than of the continuous operating point. Between two
    /// flips of that state the matrix is literally constant, so a
    /// factorization survives — see `Engine::reusable`.
    ///
    /// The invariant every member owes: every write this device makes into
    /// `a` in `Engine::build` depends only on node/branch indices, on
    /// compile-time constants, and on `ElemState::region`. Never on `x`, on
    /// `t`, or on continuous history (`v_prev`, `i_prev`, `vg1`, `vg2`).
    pub fn is_discrete_nonlinear(&self) -> bool {
        matches!(self, ElementKind::OpAmp { .. } | ElementKind::Timer555)
    }

    /// Devices that genuinely need Newton iteration: their conductance is a
    /// smooth function of the operating point (`libm::exp`/`tanh`), so the
    /// matrix moves on every NR pass and no factorization can be reused.
    ///
    /// Deliberately DERIVED from `is_nonlinear` rather than written as a
    /// third whitelist: a new nonlinear device that its author forgets to
    /// classify lands on the safe side (treated as smooth, forfeiting reuse,
    /// costing only speed) instead of silently reusing a stale matrix.
    pub fn needs_newton(&self) -> bool {
        self.is_nonlinear() && !self.is_discrete_nonlinear()
    }
}

/// Highest tier index a document may carry. sim-core owns only the
/// SYNTACTIC bound (so a hostile or stale client cannot smuggle a `tier:
/// 200` past `check_document`); what a tier MEANS is the `damage` crate's
/// table, which clamps anything it has no row for down to its top row.
/// Raise this when the tech tree needs more headroom than four rungs.
pub const MAX_TIER: u8 = 3;

/// One placed part.
///
/// `kind` is the ELECTRICAL model — everything the matrix sees. `tier` and
/// `rot` are deliberately outside it: neither may ever reach a stamp, so
/// neither can change a state hash, a node count or a matrix entry.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ElementSpec {
    pub id: u32,
    pub kind: ElementKind,
    pub pins: Vec<Point>,
    /// Which RATING this instance carries: 0 is the starting kit, higher
    /// tiers are the same device in a bigger package (a 5 W wirewound
    /// resistor is `Resistor` at tier 1). The tech tree gates which tiers a
    /// player may place; the damage crate owns what each tier can take. The
    /// solver ignores this field completely — a 5 W and a 0.25 W resistor of
    /// the same ohms are the same circuit, which is exactly why headroom can
    /// be progression instead of a rewrite.
    ///
    /// Absent in old saves and old client ops: serde defaults it to 0, so
    /// every part in an existing room is a starting-kit part.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tier: u8,
    /// Quarter-turn symbol rotation, 0..3 clockwise. RENDER ONLY, and it
    /// exists for the parts whose pins cannot express an orientation:
    /// `Ground` and `Rail` have a single pin, so rotating their pins is a
    /// no-op and the symbol would always point the same way. Multi-pin parts
    /// take their orientation from their pin geometry and ignore this.
    ///
    /// It is in the shared document (not client-local) because two players
    /// looking at one room must see one schematic. It is not in
    /// `ElementKind` because it must cost the netlist nothing.
    #[cfg_attr(feature = "serde", serde(default))]
    pub rot: u8,
}

impl ElementSpec {
    pub fn two(id: u32, kind: ElementKind, a: Point, b: Point) -> Self {
        debug_assert_eq!(kind.pin_count().min(2), 2);
        ElementSpec {
            id,
            kind,
            pins: vec![a, b],
            tier: 0,
            rot: 0,
        }
    }

    pub fn three(id: u32, kind: ElementKind, a: Point, b: Point, c: Point) -> Self {
        debug_assert_eq!(kind.pin_count(), 3);
        ElementSpec {
            id,
            kind,
            pins: vec![a, b, c],
            tier: 0,
            rot: 0,
        }
    }

    pub fn ground(id: u32, at: Point) -> Self {
        ElementSpec {
            id,
            kind: ElementKind::Ground,
            pins: vec![at],
            tier: 0,
            rot: 0,
        }
    }

    /// Same part, one tier up the tech tree.
    pub fn at_tier(mut self, tier: u8) -> Self {
        self.tier = tier;
        self
    }

    /// Same part, symbol turned `rot` quarter-turns clockwise.
    pub fn rotated(mut self, rot: u8) -> Self {
        self.rot = rot & 3;
        self
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
    /// Reposition a part's pins, and (optionally) turn its symbol.
    ///
    /// Rotation rides the Move because rotation IS a geometric transform:
    /// for a two-pin part the client rotates the pins and `rot` stays None,
    /// for a one-pin part the pins are unchanged and `rot` carries the whole
    /// of the turn. One op, one undo entry, either way. `None` means "leave
    /// the symbol as it is", which is also what an old client's Move
    /// deserialises to.
    Move {
        id: u32,
        pins: Vec<Point>,
        #[cfg_attr(feature = "serde", serde(default))]
        rot: Option<u8>,
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

/// Machine-side parameter writes: how a co-simulated mechanism (a hoist's
/// rotor, a limit switch, a position sensor) pushes its state back into the
/// live circuit between substeps. These run at kHz rates, so each variant
/// declares the cheapest correct invalidation — see `Engine::write_param`.
///
/// Unlike `InteractOp` these are NOT player edits: they never clear the
/// quarantine flag and never re-arm the post-event backward-Euler steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamWrite {
    /// Motor back-EMF (volts). RHS-only: the b vector is rebuilt every
    /// step, so this costs nothing and never refactors.
    Bemf { volts: f64 },
    /// Potentiometer wiper (0..1). Changes conductances but not the
    /// topology: invalidates the factorization only.
    Wiper { frac: f64 },
    /// Switch position. Changes the branch-unknown count, so it needs the
    /// full compile path — but only when the position actually differs.
    Switch { closed: bool },
}
