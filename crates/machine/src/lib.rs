//! `machine`: the mechanical half of a co-simulated device — one degree of
//! freedom, integrated between solver substeps.
//!
//! This crate holds THE HOIST: a crate on a platform in a vertical shaft,
//! lifted by a drum on a DC motor whose armature is a real element in the
//! circuit document. The electrical side lives in `sim-core`
//! (`ElementKind::Motor`); the only things that cross the boundary are
//! numbers the solver produced (`i_motor`) and numbers the solver consumes
//! (back-EMF, sensor wiper, limit-switch positions).
//!
//! Two state variables — rotor speed and platform height — and nothing
//! else: there is no physics engine here and none is wanted. `y` and
//! `omega` are integrals of a solver unknown, which is what makes the goal
//! MEASURED rather than asserted.
//!
//! Purity rules (mirrored from `sim-core`): no I/O, no clocks, no random,
//! f64 throughout, no `mul_add`, no transcendentals. Everything is `+ - * /`
//! plus `min`/`max`/`abs`, all of which are exact IEEE-754 operations, so
//! native and wasm32 agree bit-for-bit.

// ---------------------------------------------------------------- constants

/// Armature resistance (Ω).
pub const R_ARM: f64 = 2.0;
/// Armature inductance (H).
pub const L_ARM: f64 = 1.5e-3;
/// Motor constant: V·s/rad as back-EMF, N·m/A as torque (they are the same
/// number in SI for an ideal machine).
pub const K: f64 = 0.25;
/// Drum radius (m): rope speed = r·ω.
pub const DRUM_R: f64 = 0.02;
/// Crate + platform mass (kg).
pub const CRATE_M: f64 = 1.2;
/// Viscous damping at the drum (N·m·s).
pub const VISCOUS_B: f64 = 2e-4;
/// Effective inertia at the drum (kg·m²): rotor 3.0e-4 plus the hanging
/// mass reflected through the drum, m·r² = 4.8e-4.
pub const J_EFF: f64 = 7.8e-4;
/// Shaft height (m): the platform travels 0 ..= H.
pub const SHAFT_H: f64 = 0.40;
/// Standard gravity (m/s²).
pub const G: f64 = 9.81;

/// The green band painted across the shaft (m).
pub const BAND_LO: f64 = 0.300;
pub const BAND_HI: f64 = 0.340;
/// Continuous in-band seconds required to win.
pub const HOLD_NEED: f64 = 5.0;
/// Out-of-band drain multiplier. Draining 3× faster than it fills still
/// leaves bang-bang control a comfortable win.
pub const HOLD_DRAIN: f64 = 3.0;

/// LIM-TOP closes at/above this height (m), LIM-BOT at/below its own, both
/// releasing 2 mm later so a hovering crate cannot chatter the topology.
pub const LIM_TOP_Y: f64 = 0.36;
pub const LIM_BOT_Y: f64 = 0.04;
pub const LIM_HYST: f64 = 0.002;

/// Impact speed above which a landing counts as hard (m/s).
pub const HARD_LANDING: f64 = 0.8;

/// Sensor wiper travel limits: the pot never reaches its own ends.
pub const WIPER_MIN: f64 = 0.02;
pub const WIPER_MAX: f64 = 0.98;

/// Gravity load torque at the drum (N·m) = m·g·r.
pub const LOAD_TORQUE: f64 = CRATE_M * G * DRUM_R;
/// Armature current that exactly balances gravity (A) = m·g·r/K. Holding
/// the crate still requires this current and therefore feedback: a constant
/// voltage sets a speed, not a position.
pub const HOLD_CURRENT: f64 = LOAD_TORQUE / K;
/// Mechanical time constant (s) = J/(K²/R + b) — the electrical brake
/// dominates the viscous term. 24.8 ms, i.e. 39× the 640 µs machine tick,
/// which is why explicit Euler is provably stable here.
pub const TAU_MECH: f64 = J_EFF / (K * K / R_ARM + VISCOUS_B);

// -------------------------------------------------------------------- model

/// What the mechanism writes back into the circuit each machine tick.
/// Every field is a real electrical quantity: nothing here is a display
/// value, and nothing here bypasses the solver.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Writes {
    /// Motor back-EMF (V) = K·ω, opposing the applied terminal voltage.
    pub bemf: f64,
    /// Position-sensor potentiometer wiper (0..1), clamped off its ends.
    pub wiper: f64,
    /// LIM-TOP / LIM-BOT switch positions (hysteretic).
    pub lim_top: bool,
    pub lim_bot: bool,
}

/// The hoist: rotor speed and platform height, plus the goal's own
/// measurements. All fields are public — the server broadcasts them and
/// checkpoints them, and tests set up initial conditions with them.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Hoist {
    /// Platform height (m), 0 ..= SHAFT_H.
    pub y: f64,
    /// Drum angular velocity (rad/s); positive lifts.
    pub omega: f64,
    /// Continuous in-band seconds accumulated, 0 ..= HOLD_NEED.
    pub hold: f64,
    /// Landings harder than HARD_LANDING since the last reset.
    pub landings: u32,
    /// Energy the player's sources have delivered (J).
    pub joules: f64,
    /// Latches once `hold` reaches HOLD_NEED.
    pub win: bool,
    /// One-shot: the touchdown speed (m/s) of the tick that just ran, and
    /// zero on every other tick — a landing is the transition from airborne
    /// to floor, not the state of resting on it.
    pub impact: f64,
    /// Latched switch positions, so hysteresis has something to latch on and
    /// the server can write only on change.
    lim_top: bool,
    lim_bot: bool,
}

impl Default for Hoist {
    /// Crate on the floor, still, goal armed. LIM-BOT starts closed because
    /// y = 0 is below its threshold.
    fn default() -> Self {
        Hoist {
            y: 0.0,
            omega: 0.0,
            hold: 0.0,
            landings: 0,
            joules: 0.0,
            win: false,
            impact: 0.0,
            lim_top: false,
            lim_bot: true,
        }
    }
}

impl Hoist {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rope/platform speed (m/s), positive upward.
    pub fn velocity(&self) -> f64 {
        DRUM_R * self.omega
    }

    /// Is the platform inside the painted band right now?
    pub fn in_band(&self) -> bool {
        self.y >= BAND_LO && self.y <= BAND_HI
    }

    /// Advance the mechanism by `h` seconds using `i_motor`, the armature
    /// current the solver just produced (A, into the motor's pin 0), and
    /// return what the circuit must be told.
    ///
    /// Explicit Euler: h/τ_mech = 0.026 at the shipped 640 µs tick, so the
    /// integrator is unconditionally stable and the loop gain through
    /// back-EMF (h·K²/(R·J) = 0.026) cannot wind up.
    pub fn tick(&mut self, i_motor: f64, h: f64) -> Writes {
        let airborne = self.y > 0.0;
        self.impact = 0.0;

        // Rotor: J·dω/dt = K·i - m·g·r - b·ω. Gravity is a constant torque
        // (a hanging load, not a spring), so it never changes sign.
        self.omega += h * (K * i_motor - LOAD_TORQUE - VISCOUS_B * self.omega) / J_EFF;
        self.y += h * DRUM_R * self.omega;

        if self.y < 0.0 {
            // Floor: inelastic stop. The rope goes slack, the platform sits.
            let touchdown = (DRUM_R * self.omega).abs();
            self.y = 0.0;
            self.omega = 0.0;
            if airborne {
                self.impact = touchdown;
                if touchdown > HARD_LANDING {
                    self.landings += 1;
                }
            }
        } else if self.y > SHAFT_H {
            // Head stop: upward motion is arrested, downward is free.
            self.y = SHAFT_H;
            self.omega = self.omega.min(0.0);
        }

        // Goal: fill while in band, drain 3× as fast outside it.
        if self.in_band() {
            self.hold += h;
        } else {
            self.hold -= HOLD_DRAIN * h;
        }
        self.hold = self.hold.clamp(0.0, HOLD_NEED);
        if self.hold >= HOLD_NEED {
            self.win = true;
        }

        self.writes()
    }

    /// Accumulate the energy the player's sources delivered over `h`
    /// seconds. `watts` is Σ max(-power, 0) across sources, straight from
    /// the solver — sinking sources (a charging battery) do not refund.
    pub fn accumulate_joules(&mut self, watts: f64, h: f64) {
        if watts > 0.0 {
            self.joules += h * watts;
        }
    }

    /// Lower the crate to the floor, zero the rotor, and re-arm the goal.
    /// `joules` keeps counting: it is the room's lifetime energy meter, not
    /// part of the attempt.
    pub fn reset(&mut self) {
        self.y = 0.0;
        self.omega = 0.0;
        self.hold = 0.0;
        self.landings = 0;
        self.win = false;
        self.impact = 0.0;
    }

    /// Recompute the writes, latching the hysteretic limit switches.
    fn writes(&mut self) -> Writes {
        self.lim_top = if self.lim_top {
            self.y >= LIM_TOP_Y - LIM_HYST
        } else {
            self.y >= LIM_TOP_Y
        };
        self.lim_bot = if self.lim_bot {
            self.y <= LIM_BOT_Y + LIM_HYST
        } else {
            self.y <= LIM_BOT_Y
        };
        Writes {
            bemf: K * self.omega,
            // Wiper runs from 1 at the floor to 0 at the head: wire SENSE-A
            // to the supply and SENSE-B to ground and the wiper voltage
            // rises with height.
            wiper: (1.0 - self.y / SHAFT_H).clamp(WIPER_MIN, WIPER_MAX),
            lim_top: self.lim_top,
            lim_bot: self.lim_bot,
        }
    }
}
