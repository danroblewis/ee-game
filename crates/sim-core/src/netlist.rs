//! The schematic document model the engine compiles from.
//!
//! Elements are N-terminal parts placed on an integer grid. Coincident
//! endpoints are electrically connected; `Wire` elements additionally merge
//! their two endpoints into one node (Falstad-style wire closure), so wires
//! carry current but add no unknowns.

pub type Point = (i32, i32);

/// Clamp a logic part's declared width into the range the model supports.
/// Used by `pin_count` and `logic_pins`, which must be TOTAL functions: they
/// run before validation on documents this build did not write.
#[inline]
const fn logic_width(n: u8, lo: u8, hi: u8) -> usize {
    if n < lo {
        lo as usize
    } else if n > hi {
        hi as usize
    } else {
        n as usize
    }
}

/// Largest pin count of any element.
///
/// 10, set by the widest members of the CMOS logic family: a 4-bit shift
/// register is `[VCC, GND, CLK, SER, RST, Q0..Q3]` = 9, and a 4:1 mux is
/// `[VCC, GND, I0..I3, S0, S1, Y]` = 9. The spare pin is headroom for an
/// output-enable, not slack.
///
/// It is deliberately the SMALLEST ceiling the part list fits in, because
/// the cost is paid by every element in the room and not just the wide
/// ones: `ElemFrame` is `2 · MAX_PINS` numbers per element per broadcast
/// tick, so a two-pin resistor ships eight unused voltage slots and eight
/// unused current slots at this setting. 8-bit shift registers are
/// therefore NOT a part — you cascade two 4-bit ones, which is how a real
/// 74HC595 chain is built and is a lesson rather than a compromise.
///
/// Raising it is now safe by construction: `state_hash` iterates
/// `pin_count()` rather than this constant, so no golden digest moves, and
/// `FRAME_STRIDE` is derived from it in one place that both transports
/// call.
pub const MAX_PINS: usize = 10;

/// Short-circuit output current a legacy (field-less) `OpAmp` deserialises
/// to: a 741/LM358-class jellybean. See `ElementKind::OpAmp`.
pub const DEFAULT_OPAMP_ISC: f64 = 0.025;

#[cfg(feature = "serde")]
fn default_opamp_isc() -> f64 {
    DEFAULT_OPAMP_ISC
}

/// The shape a source's AC component traces.
///
/// All four are PHASE-ALIGNED with the sine: each starts at 0 and rises, and
/// each has the same period and the same +/-`amp` extremes. Switching a
/// running source's waveform therefore changes its shape without jumping its
/// phase, so a scope trace stays put and a circuit built around the timing
/// keeps working.
///
/// Only `Sine` needs a transcendental. The other three are exact piecewise
/// arithmetic — cheaper than the sine and, unlike it, carrying no library
/// approximation at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Wave {
    /// The default, and what every document written before this existed is.
    #[default]
    Sine,
    /// +amp for the first half of the period, -amp for the second.
    /// DISCONTINUOUS: see `Wave::has_edges`.
    Square,
    /// 0 -> +amp -> 0 -> -amp -> 0. Continuous, with slope breaks the
    /// trapezoidal integrator handles without help.
    Triangle,
    /// Ramps 0 -> +amp across the first half, jumps to -amp, ramps back to 0.
    /// DISCONTINUOUS: see `Wave::has_edges`.
    Saw,
}

impl Wave {
    /// Does this shape JUMP — a step change of 2*amp in zero time?
    ///
    /// This is the load-bearing question, not a cosmetic one. Trapezoidal
    /// integration assumes the state moved smoothly across the step, so a
    /// genuine discontinuity into a reactive load makes it ring: the digital
    /// logic work measured an output driven to +5.53 V and -0.55 V on a 5 V
    /// rail from exactly this, and a pin a volt outside its rails is how a
    /// part gets destroyed. The engine already owns the cure — the backward-
    /// Euler steps it takes after a switch flip — and a waveform edge arms
    /// them the same way.
    pub fn has_edges(self) -> bool {
        matches!(self, Wave::Square | Wave::Saw)
    }

    /// Is `-f(u)` the same shape half a period along — `-f(u) == f(u + 1/2)`?
    ///
    /// This is what lets `Constraint::canonical` fold a NEGATIVE amplitude
    /// into a pi phase shift and keep amplitudes positive, so a source and
    /// its mirror image (drawn the other way round, amplitude negated) are
    /// recognised as the same net rather than as a conflict.
    ///
    /// It holds for sine, square and triangle. It is FALSE for the sawtooth,
    /// and that is not a detail: negating a ramp gives a REVERSE ramp, which
    /// is a different shape and not one this enum can even name. Folding it
    /// anyway would quietly merge two sources demanding genuinely different
    /// voltages onto one branch row and solve the wrong circuit. So an
    /// asymmetric wave keeps its amplitude SIGNED, its sign reaches the key,
    /// and a saw drawn against a saw is correctly reported as a conflict.
    pub fn is_half_wave_antisymmetric(self) -> bool {
        !matches!(self, Wave::Saw)
    }

    /// The shape at normalized phase `u` in `[0, 1)`, in `[-1, 1]`.
    ///
    /// Sine is NOT evaluated here: it keeps its original
    /// `libm::sin(2*pi*hz*t + phase)` call so that every document that
    /// predates this enum produces bit-identical numbers. Re-deriving it from
    /// `u` would round differently in the last place and move every golden
    /// digest for no reason.
    pub fn eval_unit(self, u: f64) -> f64 {
        match self {
            // Handled by the caller; see above.
            Wave::Sine => 0.0,
            Wave::Square => {
                if u < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Wave::Triangle => {
                if u < 0.25 {
                    4.0 * u
                } else if u < 0.75 {
                    2.0 - 4.0 * u
                } else {
                    4.0 * u - 4.0
                }
            }
            Wave::Saw => {
                if u < 0.5 {
                    2.0 * u
                } else {
                    2.0 * u - 2.0
                }
            }
        }
    }
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
    /// v(pin0) - v(pin1) = dc + amp * wave(2*pi*hz*t + phase). Pin 0 is +.
    VoltageSource {
        dc: f64,
        amp: f64,
        hz: f64,
        phase: f64,
        /// Defaulted, so a save written before waveforms existed loads as the
        /// sine it has always been.
        #[cfg_attr(feature = "serde", serde(default))]
        wave: Wave,
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
        #[cfg_attr(feature = "serde", serde(default))]
        wave: Wave,
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
    /// Light-dependent resistor (CdS cell). Stamps exactly like a `Resistor`;
    /// what makes it a different device is where its resistance comes from.
    ///
    /// `light` is NORMALIZED illumination in 0..1 — not lux, and nothing may
    /// ever print it as one. Resistance falls LOG-linearly from `r_dark` at
    /// light = 0 to `r_lit` at light = 1, which is how a real cell behaves
    /// (log R is linear in log E over the cell's decade range).
    ///
    /// `light` is an INPUT to the electrical model, exactly the way
    /// `Motor::bemf` is: the optical side lives outside sim-core and writes
    /// the fraction back through `Engine::write_param` at the tick boundary.
    /// **sim-core owns no optical state — it only ever sees a fraction.** It
    /// has no idea a camera, a microphone or a gamepad exists, which is the
    /// whole reason an external input cannot touch determinism.
    ///
    /// It is `serde(skip)` because a READING is world state, not document
    /// state: `r_dark`/`r_lit` are the part's calibration and persist, the
    /// illumination does not. A saved room loads DARK — a circuit that was
    /// lit is dark again until somebody points a camera at it, rather than
    /// loading with a light nobody is shining.
    Photocell {
        /// Resistance with no light on it. This is also the REST value: no
        /// camera, no claim, no player — the cell reads dark.
        r_dark: f64,
        /// Resistance under full illumination.
        r_lit: f64,
        #[cfg_attr(feature = "serde", serde(skip))]
        light: f64,
    },

    // ------------------------------------------------------ the CMOS family
    //
    // Five kinds, one model. A logic chip here is a PASSIVE CONDUCTANCE
    // NETWORK whose values are chosen by a discrete state — which is
    // literally what CMOS is, and is where everything else comes from:
    //
    // * an output pin is a pull-up to the VCC PIN and a pull-down to the GND
    //   PIN, one at 50 Ω and the other at 1 GΩ. The current a gate delivers
    //   therefore comes out of the player's battery instead of out of
    //   nowhere, which is the 555's lesson (`engine.rs`) reached without a
    //   branch row: `is_branch()` is false for the whole family;
    // * `Σ v·i` over the pins is exactly `Σ g·(Δv)²` over the internals, so
    //   it is provably non-negative and IS the chip's own dissipation. The
    //   op-amp's `elem_power` exception cannot recur here by construction;
    // * the incidence pattern is identical in every discrete state — both
    //   conductances are always stamped, only their values move — so
    //   `validate::probe_solvable`, which factors exactly one cold state,
    //   is sound for this family without being taught anything;
    // * levels are fractions of the LIVE supply, never a hard-coded 5 V.
    //
    // Timing: every logic element is a one-substep delay. Its state advances
    // in `accept()`, never during Newton. See `Engine::accept`.
    /// Combinational gate. Pins: `[VCC, GND, in0..in(ins-1), Y]`.
    ///
    /// One kind covers eight gates at four widths, which is what lets the
    /// family COMPOSE instead of enumerate: an SR latch is two cross-coupled
    /// `Nand`s, a self-correcting ring counter is a `Nor` fed from a shift
    /// register, and neither needs a part of its own.
    Gate {
        op: GateOp,
        /// Input count, 1..=4 (forced to 1 for `Buf`/`Not`).
        ins: u8,
    },
    /// D-type storage. Pins: `[VCC, GND, CLK, D, RST, Q, /Q]`. `RST` is
    /// asynchronous and ACTIVE LOW, like every real part's `/CLR`.
    ///
    /// `edge = true` is a rising-edge-triggered flip-flop; `edge = false` is
    /// a transparent latch (Q follows D while CLK is high, holds while it is
    /// low). Level-versus-edge is the single most confusing distinction in
    /// intro digital, and putting it behind one boolean means a player can
    /// flip it in the properties panel on a live circuit and watch the
    /// behaviour change.
    FlipFlop {
        edge: bool,
    },
    /// Serial-in, parallel-out shift register. Pins:
    /// `[VCC, GND, CLK, SER, RST, Q0..Q(bits-1)]`, `bits` in 2..=4.
    ///
    /// All stages move from ONE clock edge, evaluated in one place in one
    /// pass — there is no internal ripple, and that is the reason this is a
    /// single element rather than `bits` composed flip-flops: a global LU
    /// factorization is O(n³) over the whole room, so an internal ripple
    /// would cost `bits` of them per edge instead of one.
    ///
    /// Cascade two for 8 bits (Q3 of the first into SER of the second),
    /// which is how a real 74HC595 chain is built.
    ShiftReg {
        bits: u8,
    },
    /// BUCKET BRIGADE DEVICE: an analog delay line. Pins `[IN, OUT, CLK, GND]`.
    ///
    /// ## Why this is one element and not `stages` capacitors
    ///
    /// A real BBD is a chain of capacitors handing charge along under a
    /// two-phase clock, and the obvious model — `stages` capacitors and
    /// `2 * stages` MOSFET switches — is unaffordable at any useful length: a
    /// 4096-stage MN3005 would be tens of thousands of coupled unknowns
    /// against an O(n^3) factorization.
    ///
    /// It is also the WRONG model. A BBD is a SAMPLED-DATA device: charge
    /// moves between buckets only at a clock edge, and between edges each
    /// bucket is isolated from its neighbours. The chain therefore has no
    /// business in the MNA matrix at all. What the circuit can see is two
    /// terminals — a high-impedance input that samples, and an output that
    /// drives — which is exactly the shape `Motor` already has: the state
    /// evolves outside the matrix and writes the RHS only.
    ///
    /// So this costs ONE branch unknown and no Newton iterations, whatever
    /// `stages` is. The expensive-looking part is one of the cheapest in the
    /// engine, and none of that is a shortcut: a bucket chain really is a
    /// shift register of samples, no more behavioural than `Timer555`'s
    /// internal comparators. Every terminal voltage still comes from the
    /// solver.
    ///
    /// ## Delay
    ///
    /// One stage per clock TRANSITION, so a full cycle moves charge two
    /// half-stages exactly as a two-phase device does, and the delay is the
    /// datasheet formula:
    ///
    /// ```text
    ///     t_delay = stages / (2 * f_clock)
    /// ```
    ///
    /// The clock is a PIN, not a parameter, which is the whole point: delay
    /// time is set by whatever the player builds to drive it (a 555 and a
    /// pot), and modulating that clock is how chorus, flanger and vibrato
    /// were actually made. Echo needs no feature either — it is a wire from
    /// OUT back to IN through a resistor.
    ///
    /// ## What is NOT modelled, and must be said
    ///
    /// * CHARGE TRANSFER INEFFICIENCY. A real bucket loses a little on every
    ///   hand-off, which over thousands of stages is the BBD's characteristic
    ///   treble loss and noise. Modelling it per stage is O(stages) per edge
    ///   and is left out rather than approximated.
    /// * The sample lands on the substep where the edge is seen, so the delay
    ///   quantises to `dt`. At 20 us against delays of order 100 ms that is
    ///   ~0.01 %.
    /// * ALIASING IS MODELLED, deliberately. Sampling at f_clock folds
    ///   everything above f_clock/2, which is why every real BBD schematic is
    ///   wrapped in filters. Getting that wrong should sound wrong.
    Bbd {
        /// Bucket count, 2..=MAX_BBD_STAGES.
        stages: u16,
    },
    /// PT2399-class ECHO CHIP: the same delay line, but it brings its own
    /// clock AND its own two op-amps.
    ///
    /// Pins `[IN, OUT, VCO, GND, OP1-IN, OP1-OUT, OP2-IN, OP2-OUT]`.
    ///
    /// ## Why this exists next to `Bbd`
    ///
    /// A bucket brigade needs a clock, which means a 555 and a pot before it
    /// does anything. That is the right answer for a player who wants to
    /// modulate the delay — it is how a flanger is built — and the wrong one
    /// for a player who just wants an echo. The real PT2399 answers exactly
    /// that: one resistor on pin 6 and you have a delay.
    ///
    /// ## How the resistor sets the delay, honestly
    ///
    /// The VCO pin sits behind an internal reference. The player's resistor
    /// from it to ground therefore draws a current, and THAT CURRENT — which the
    /// solver computes, not us — sets the internal oscillator. So the delay
    /// really is a consequence of the circuit the player built: swap the
    /// resistor for a pot and the delay sweeps, exactly as it does on the
    /// bench. Nothing reads a resistance; nothing could, and it would be a
    /// lie if it did.
    ///
    /// ```text
    ///     f_clock = PT_HZ_PER_AMP * i_RT,     delay = PT_STAGES / f_clock
    /// ```
    ///
    /// with the constant fitted so the datasheet's own range falls out: about
    /// 30 ms at 5 kΩ, about 340 ms at 50 kΩ.
    ///
    /// ## Deviations, stated
    ///
    /// * The output has a real source impedance rather than being ideal, so
    ///   it costs no branch unknown — the single branch this part owns is
    ///   spent on the RT reference, because that current is the input to the
    ///   whole mechanism and reading it through a Norton equivalent would be
    ///   a small difference of two large numbers.
    /// * The clock cannot exceed one shift per substep, which caps the
    ///   shortest delay at `PT_STAGES * dt`. At the room's 20 µs that is
    ///   20 ms, just under the chip's own 30 ms minimum, so the whole useful
    ///   range is reachable.
    /// * The real chip's companding and its 1-bit converters — the reason a
    ///   long PT2399 delay degrades so musically — are not modelled.
    ///
    /// ## The two op-amps, and why they have no + input
    ///
    /// The real chip carries two op-amps, and you are meant to use them: one
    /// buffers and mixes the input, one sets the wet/dry blend and the
    /// feedback that makes repeats. They are on the package, so a circuit
    /// built around this part should use the ones it came with rather than
    /// bolting external ones on — which is the habit that transfers to a bench.
    ///
    /// Each exposes only its INVERTING input and its output, because on the
    /// real chip the non-inverting inputs are tied internally to the same
    /// half-supply reference RT sits on. That single fact is why every
    /// PT2399 circuit ever drawn uses them as INVERTING stages: it is not a
    /// stylistic choice, it is the only thing the pinout allows. Modelling
    /// it faithfully means a player who learns the topology here has learned
    /// the topology, not a convenience.
    ///
    /// They are transconductance macromodels — a `gm` stage into an output
    /// impedance, clamped to the supply — so they cost no branch unknown and
    /// no Newton iteration, exactly like the delay line they sit beside.
    Pt2399,
    /// Synchronous binary counter. Pins: `[VCC, GND, CLK, RST, Q0..]`,
    /// `bits` in 2..=4, `modulus` in 2..=2^bits.
    ///
    /// Synchronous rather than ripple, and that is a stated deviation from a
    /// 74HC393: a ripple counter's ~4 gate delays of inter-bit skew would
    /// cost four factorizations per edge instead of one, for a skew no
    /// player can use. What it earns over `ShiftReg` is BINARY WEIGHT —
    /// divide a clock by 2/4/8 for octaves, or address a `Mux`.
    Counter {
        bits: u8,
        modulus: u8,
    },
    /// Analog multiplexer (4051-class, not a 74HC153). Pins:
    /// `[VCC, GND, I0..I(2^sel - 1), S0..S(sel-1), Y]`, `sel` in 1..=2.
    ///
    /// The selected input is connected to Y through 50 Ω and the rest
    /// through 1 GΩ. Because that is a CONDUCTANCE and not a logic buffer it
    /// passes ANALOG in both directions — a pot's control voltage goes
    /// straight through it — which is worth far more in this game than a
    /// digital-only mux and costs nothing extra in the model.
    Mux {
        sel: u8,
    },
}

/// The combinational function a [`ElementKind::Gate`] computes.
///
/// Inversion is folded into the op rather than carried as a separate flag:
/// a player picks "NAND", not "AND with invert set", and the properties
/// panel gets one field instead of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GateOp {
    And,
    #[default]
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    /// Non-inverting buffer (`ins` forced to 1).
    Buf,
    /// Inverter (`ins` forced to 1).
    Not,
}

impl GateOp {
    /// Buffers and inverters take exactly one input whatever `ins` says.
    /// Applied in `pin_count` as well as in the evaluator, so the pinout a
    /// document declares and the pinout the solver expects cannot disagree.
    #[inline]
    pub fn fixed_ins(self) -> Option<u8> {
        matches!(self, GateOp::Buf | GateOp::Not).then_some(1)
    }

    /// Evaluate against `n_hi` inputs high out of `ins`, and the parity of
    /// the high count (which is what XOR/XNOR generalise to at width > 2).
    #[inline]
    pub fn eval(self, ins: usize, n_hi: usize) -> bool {
        match self {
            GateOp::And => n_hi == ins,
            GateOp::Nand => n_hi != ins,
            GateOp::Or | GateOp::Buf => n_hi > 0,
            GateOp::Nor | GateOp::Not => n_hi == 0,
            GateOp::Xor => n_hi % 2 == 1,
            GateOp::Xnor => n_hi % 2 == 0,
        }
    }
}

/// Which pins of a logic chip do what, so `build`, `update_guesses` and
/// `accept` cannot disagree about one part's pinout.
///
/// Three disjoint roles, and the split matters:
/// * **inputs** are Schmitt-sensed and get a symmetric high-resistance leak
///   to both rails;
/// * **outputs** are driven — a switched pull-up/pull-down pair;
/// * a `Mux`'s I pins are NEITHER. They are pass-gate terminals: analog,
///   bidirectional, and not thresholded at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogicPins {
    /// Schmitt-sensed input pins, contiguous from `in0`.
    pub n_in: usize,
    pub in0: usize,
    /// Driven output pins, contiguous from `out0`.
    pub n_out: usize,
    pub out0: usize,
    /// Which INPUT (index into `0..n_in`) is the clock, for the sequential
    /// parts. `None` for combinational ones.
    pub clk: Option<usize>,
}

/// A photocell's resistance at a given illumination: log-linear between
/// `r_dark` (light = 0) and `r_lit` (light = 1).
///
/// `libm` only — the same envelope the diode's `exp` already runs in — so
/// native and wasm32 produce bit-identical resistances. No `mul_add`.
pub fn photocell_ohms(r_dark: f64, r_lit: f64, light: f64) -> f64 {
    let l = if light.is_finite() {
        light.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = r_dark.max(MIN_PHOTOCELL_OHMS);
    let t = r_lit.max(MIN_PHOTOCELL_OHMS);
    if l == 0.0 {
        return d;
    }
    if l == 1.0 {
        return t;
    }
    let ln_d = libm::log(d);
    libm::exp(ln_d + l * (libm::log(t) - ln_d))
}

/// Floor for either end of a photocell's range. The gate enforces
/// `MIN_OHMS`; this is the belt-and-braces floor the stamp itself applies so
/// a hand-edited save can never put `1/0` into the matrix.
pub const MIN_PHOTOCELL_OHMS: f64 = 1e-6;

impl ElementKind {
    /// The document tag — the `t` field serde writes, and the same string the
    /// TypeScript client discriminates on.
    ///
    /// Exhaustive by design (no wildcard arm): a new variant will not compile
    /// until someone names it here, which is what lets other tables key off a
    /// tag instead of repeating the variant list.
    pub fn tag(&self) -> &'static str {
        use ElementKind::*;
        match self {
            Wire => "Wire",
            Ground => "Ground",
            Resistor { .. } => "Resistor",
            Lamp { .. } => "Lamp",
            Speaker { .. } => "Speaker",
            Capacitor { .. } => "Capacitor",
            Inductor { .. } => "Inductor",
            VoltageSource { .. } => "VoltageSource",
            CurrentSource { .. } => "CurrentSource",
            Rail { .. } => "Rail",
            Switch { .. } => "Switch",
            Button { .. } => "Button",
            Diode => "Diode",
            Zener { .. } => "Zener",
            Led { .. } => "Led",
            Npn { .. } => "Npn",
            Pnp { .. } => "Pnp",
            Nmos { .. } => "Nmos",
            Pmos { .. } => "Pmos",
            OpAmp { .. } => "OpAmp",
            Ota => "Ota",
            Timer555 => "Timer555",
            Potentiometer { .. } => "Potentiometer",
            Motor { .. } => "Motor",
            Noise { .. } => "Noise",
            Photocell { .. } => "Photocell",
            Gate { .. } => "Gate",
            FlipFlop { .. } => "FlipFlop",
            ShiftReg { .. } => "ShiftReg",
            Bbd { .. } => "BBD",
            Pt2399 => "PT2399",
            Counter { .. } => "Counter",
            Mux { .. } => "Mux",
        }
    }

    pub fn pin_count(&self) -> usize {
        use ElementKind::*;
        match self {
            Ground | Rail { .. } => 1,
            Timer555 => 6,
            Bbd { .. } => 4,
            Pt2399 => 8,
            Ota => 4,
            Npn { .. }
            | Pnp { .. }
            | Nmos { .. }
            | Pmos { .. }
            | OpAmp { .. }
            | Potentiometer { .. } => 3,
            // The logic family: two supply pins plus its own signals. Every
            // width here is CLAMPED rather than trusted, so `pin_count` stays
            // total — a document carrying `bits: 99` resolves to a pin count
            // this build can name, `set_elements` drops it for having the
            // wrong number of pins, and `check_kind` says why.
            Gate { op, ins } => 3 + logic_width(op.fixed_ins().unwrap_or(*ins), 1, 4),
            FlipFlop { .. } => 7,
            ShiftReg { bits } => 5 + logic_width(*bits, 2, 4),
            Counter { bits, .. } => 4 + logic_width(*bits, 2, 4),
            Mux { sel } => {
                let s = logic_width(*sel, 1, 2);
                3 + (1 << s) + s
            }
            _ => 2,
        }
    }

    /// The pin roles for a logic chip. `None` for everything else.
    pub fn logic_pins(&self) -> Option<LogicPins> {
        use ElementKind::*;
        Some(match self {
            // [VCC, GND, in0..in(ins-1), Y]
            Gate { op, ins } => {
                let n = logic_width(op.fixed_ins().unwrap_or(*ins), 1, 4);
                LogicPins {
                    n_in: n,
                    in0: 2,
                    n_out: 1,
                    out0: 2 + n,
                    clk: None,
                }
            }
            // [VCC, GND, CLK, D, RST, Q, /Q]
            FlipFlop { .. } => LogicPins {
                n_in: 3,
                in0: 2,
                n_out: 2,
                out0: 5,
                clk: Some(0),
            },
            // [VCC, GND, CLK, SER, RST, Q0..]
            ShiftReg { bits } => LogicPins {
                n_in: 3,
                in0: 2,
                n_out: logic_width(*bits, 2, 4),
                out0: 5,
                clk: Some(0),
            },
            // [VCC, GND, CLK, RST, Q0..]
            Counter { bits, .. } => LogicPins {
                n_in: 2,
                in0: 2,
                n_out: logic_width(*bits, 2, 4),
                out0: 4,
                clk: Some(0),
            },
            // [VCC, GND, I0.., S0.., Y] — the I pins are a pass gate, so
            // they are neither inputs nor outputs here. Only the select
            // lines are thresholded, and Y is driven by nothing.
            Mux { sel } => {
                let s = logic_width(*sel, 1, 2);
                LogicPins {
                    n_in: s,
                    in0: 2 + (1 << s),
                    n_out: 0,
                    out0: 0,
                    clk: None,
                }
            }
            _ => return None,
        })
    }

    /// The CMOS logic family: the parts that carry `ElemState::dstate` and
    /// advance it once per accepted substep.
    pub fn is_logic(&self) -> bool {
        self.logic_pins().is_some()
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
                | ElementKind::Bbd { .. }
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
        ) || self.is_logic()
    }

    /// The subset of `is_nonlinear` whose contribution to the MNA **matrix**
    /// is a function of a DISCRETE state (an op-amp's rail region, a 555's
    /// RS latch, a logic chip's `dstate`) rather than of the continuous
    /// operating point. Between two flips of that state the matrix is
    /// literally constant, so a factorization survives — see
    /// `Engine::reusable`.
    ///
    /// The invariant every member owes: every write this device makes into
    /// `a` in `Engine::build` depends only on node/branch indices, on
    /// compile-time constants, and on DISCRETE state (`ElemState::region`
    /// for the op-amp and the 555, `ElemState::dstate` for the logic
    /// family). Never on `x`, on `t`, or on continuous history (`v_prev`,
    /// `i_prev`, `vg1`, `vg2`).
    ///
    /// Getting this wrong for the logic family would be expensive rather
    /// than merely slow: the flag it feeds (`Engine::smooth_nonlinear`) is
    /// GLOBAL to the room, so one misclassified gate disarms reuse for every
    /// op-amp and 555 sharing the matrix with it.
    pub fn is_discrete_nonlinear(&self) -> bool {
        matches!(self, ElementKind::OpAmp { .. } | ElementKind::Timer555) || self.is_logic()
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
/// Longest bucket brigade a document may ask for.
///
/// 4096 is the MN3005, the longest BBD anyone actually shipped, and it costs
/// 32 kB of f64 per placed part — the chain is a plain buffer, not matrix
/// unknowns, so length is memory rather than solve time. The bound exists so
/// a hostile or stale document cannot ask for a gigabyte.
pub const MAX_BBD_STAGES: u16 = 4096;

/// The PT2399's internal delay-line depth, in samples.
///
/// Fixed, because the real chip's RAM is: only its clock varies. 1024 puts
/// the datasheet's 30 ms at 34 kHz and its 340 ms at 3 kHz, both of which
/// the engine's 50 kHz substep grid can clock without ever needing two
/// shifts in one substep.
pub const PT_STAGES: u16 = 1024;
/// The RT pin's internal reference, in volts.
pub const PT_V_RT: f64 = 2.5;
/// Internal series resistance in front of the VCO pin, in ohms.
///
/// FROM THE DATASHEET'S OWN TABLE 1, not invented. The table is not a 1/R
/// law: the clock tops out at 22 MHz as R goes to zero, which can only mean
/// there is resistance inside the pin. Fitting `f = K / (R + R0)` to the two
/// ends — 2.0 MHz at 27.6 kΩ and 22 MHz at ~0 — gives R0 = 2.76 kΩ, and that
/// same constant then predicts the MIDDLE of the table to about 3 %: 150.4 ms
/// against a printed 151 ms at 10.5 kΩ, 344 against 342 at 27.6 kΩ. A
/// constant fitted at the ends that lands on the middle is the check that the
/// SHAPE is right rather than the curve merely being bent to fit.
///
/// It also has a happy side effect the first guess at this needed a special
/// case for: the pin can be tied straight to ground and the chip simply runs
/// at its fastest, exactly as the real one does at R = 0.5 Ω.
pub const PT_R_RT: f64 = 2760.0;
/// Internal op-amp transconductance and output conductance.
///
/// Open-loop gain is their ratio, 1e5, which is an ordinary audio op-amp.
/// A `gm` stage into an output impedance is the standard macromodel and it
/// stamps entirely into the conductance matrix — no branch unknown, no
/// Newton iteration — which is what lets a part carry two of them for free.
pub const PT_OA_GM: f64 = 5e3;
/// 2 Ω of open-loop output impedance. Low for a macromodel, and chosen on
/// purpose: in this game a speaker is 8 Ω, so a stage that cannot drive one
/// is a stage nobody can hear. At 1 kΩ — the first value here — the chip's
/// own op-amp delivered 0.4 % of its signal into a speaker, which reads as a
/// broken part rather than a loaded one. A closed-loop op-amp's output
/// impedance is its open-loop value divided by the loop gain anyway, so the
/// honest figure for a stage in feedback is far below even this.
pub const PT_OA_GOUT: f64 = 0.5;
/// The rails an internal op-amp would clip at, on the chip's single 5 V
/// supply referred to PT_V_RT.
///
/// NOT ENFORCED YET, and that is a real gap rather than an oversight worth
/// hiding. Clipping is a discrete nonlinearity, so the region has to be
/// iterated to consistency inside Newton like `OpAmp`'s is — and at a gain
/// of 1e4 the inverting input only has to move half a millivolt to traverse
/// the whole rail range, so a first attempt at that region test CHATTERED
/// and quarantined the room after four rescues. A linear stage that always
/// converges beats a clipping one that can take a player's circuit down, so
/// these are documentation until the region test is done properly.
///
/// What it costs: an overdriven internal op-amp keeps amplifying instead of
/// clipping, so an echo with a feedback gain above unity grows without bound
/// rather than settling into distortion the way the real chip does.
pub const PT_OA_LO: f64 = 0.4;
pub const PT_OA_HI: f64 = 4.6;
/// Sample-clock frequency per amp drawn from the VCO pin.
///
/// Calibrated so `PT_STAGES / f` reproduces Table 1's DELAY column, which is
/// what a player experiences — the datasheet's own 22 MHz figure is the
/// chip's system clock, divided internally by about 667 before it reaches
/// the RAM. Modelling the divided rate directly keeps the shift cadence
/// inside the engine's substep grid and changes nothing anyone can hear.
///
/// Against Table 1: 31.3 ms at 0 Ω (printed 31.3), 150.4 ms at 10.5 kΩ
/// (printed 151), 344 ms at 27.6 kΩ (printed 342).
pub const PT_HZ_PER_AMP: f64 = 3.61e7;

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
    /// What a player calls this part: "TEMPO", "CUTOFF", "BEAT 1".
    ///
    /// A LABEL, and nothing else. It never reaches a stamp, never changes a
    /// node count and never moves a state hash, exactly like `tier` — two
    /// parts differing only in their names are the same circuit. It exists
    /// because a control panel listing "SW #431" tells a player nothing, and
    /// the only way to name a control used to be to wrap it in its own panel
    /// region just to borrow the region's name.
    ///
    /// Shared document state, not client-local: two players looking at one
    /// room must read the same labels on the same knobs.
    ///
    /// Empty means unnamed, and the UI falls back to the part's kind and id.
    /// Absent in old saves and old clients' ops, so serde defaults it empty
    /// and every existing part is unnamed.
    #[cfg_attr(feature = "serde", serde(default))]
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "String::is_empty"))]
    pub name: String,
}

/// Longest part name accepted. Generous for a knob label, short enough that
/// a hostile client cannot push a megabyte through the op pipeline and into
/// every other player's document.
pub const MAX_NAME: usize = 24;

impl ElementSpec {
    pub fn two(id: u32, kind: ElementKind, a: Point, b: Point) -> Self {
        debug_assert_eq!(kind.pin_count().min(2), 2);
        ElementSpec {
            id,
            kind,
            pins: vec![a, b],
            tier: 0,
            rot: 0,
            name: String::new(),
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
            name: String::new(),
        }
    }

    /// Any width. The 6-to-10-pin parts have no natural positional
    /// constructor, so they take their pin list directly.
    pub fn pins(id: u32, kind: ElementKind, pins: &[Point]) -> Self {
        debug_assert_eq!(kind.pin_count(), pins.len());
        ElementSpec {
            id,
            kind,
            pins: pins.to_vec(),
            tier: 0,
            rot: 0,
            name: String::new(),
        }
    }

    pub fn ground(id: u32, at: Point) -> Self {
        ElementSpec {
            id,
            kind: ElementKind::Ground,
            pins: vec![at],
            tier: 0,
            rot: 0,
            name: String::new(),
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
    /// Rename a part. Its own op rather than a field on `Move` or `SetKind`
    /// because renaming is neither a geometric nor an electrical change: it
    /// must not be able to move a pin or a value by accident, and it reads
    /// as one undo entry saying "renamed", which is what a player expects.
    SetName {
        id: u32,
        name: String,
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
    /// Photocell illumination (0..1). Changes a conductance, never the
    /// topology: invalidates the factorization only, and only when the value
    /// actually moves — a camera pointed at a still wall refactors nothing.
    ///
    /// It cannot make the matrix singular: R is finite and non-zero across
    /// the whole declared range, and the placement gate has already trialled
    /// BOTH ends of that range (`validate::pin_at_peak`). That is the same
    /// argument the hoist's `Wiper` write rests on — the guarantee is made at
    /// placement time, so the per-tick write needs no gate.
    Light { light: f64 },
}
