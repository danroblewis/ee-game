//! The MNA engine: compile a netlist into independent per-island systems,
//! advance them in fixed timesteps, keep every displayed number honest.
//!
//! ## Islands
//!
//! A document is partitioned into **islands**: connected components of the
//! element graph with the ground node cut out. Every `Ground` ties into
//! node 0, which is eliminated from the system, so two boards that share
//! nothing but ground share nothing at all — they were already
//! block-diagonal blocks of one matrix, and the coupling was a fiction of
//! the data layout, not physics.
//!
//! Each [`Island`] owns its own unknown vector, dense matrix, LU
//! factorization, `linear` flag, piecewise-linear factorization reuse,
//! Newton-Raphson loop and convergence test, post-event backward-Euler
//! counter, rescue ladder and quarantine flag. Consequences, all of them
//! measured in `docs/scale-baseline.md`:
//!
//! - a diode in district 7 no longer costs district 3 a refactorization;
//! - NR convergence stops being all-or-nothing (2.09 -> 1.03 iterations);
//! - fill-in stays near 1.0x because island `n` stays small;
//! - a diverging island quarantines alone. The world keeps running.
//!
//! sim-core owns no threads and no clock. `Engine::advance` steps the
//! islands in order; a caller that wants them in parallel takes
//! [`Engine::step_plan`], steps the slice however it likes (rayon lives in
//! `crates/server`), and commits the world clock with
//! [`Engine::commit_advance`]. The two are arithmetically identical: island
//! state is disjoint memory, so the operand of every flop is the same in
//! either order.
//!
//! ## Piecewise-linear factorization reuse
//!
//! An island whose matrix is constant between discrete events keeps its LU
//! across substeps instead of refactoring: either the island is linear, or
//! its only nonlinearities are piecewise-linear (op-amp rail regions, 555
//! latches) — see [`Island::reusable`] and `ElementKind::is_discrete_nonlinear`.
//! Reuse is EXACT: on a hit the retained L/U is bit-for-bit what a refactor
//! would have recomputed, which `crates/sim-golden/tests/pwl_reuse.rs`
//! asserts per island, on every golden, on the state hash AND on the raw
//! matrix bits.
//!
//! Partitioning makes this strictly better rather than merely compatible.
//! The disarming condition ("some device's conductance is a smooth function
//! of the operating point") used to be one global flag, so one diode
//! anywhere in the room forced every 555 and every op-amp in it to refactor
//! on every Newton pass. It is per island now: a diode district disarms only
//! itself, and the op-amp district next door keeps its factorization.
//!
//! ## Quiescence and local dt — the two multipliers on islands
//!
//! Islands cut the work *per substep*. Two further levers cut how much of
//! the world is visited *at all*, and they only exist because islands do
//! (you cannot skip part of one matrix).
//!
//! **Quiescence.** An island that has FINISHED MOVING is asleep: it stamps
//! nothing, factors nothing and solves nothing, and every number read out of
//! it is the DC state its last real solve produced. Nothing is invented — a
//! held solution of a circuit that is not moving *is* the solution of the
//! next substep. Three conditions, all of them held continuously for
//! [`Tuning::quiescence_hold`] seconds of sim time, and each applied to the
//! unknowns whose dimension it is stated in (node unknowns are volts, branch
//! unknowns are amps):
//!
//! 1. a per-step slew under [`Tuning::quiescence_slew`] / `_slew_i` (the
//!    criterion measured in `docs/scale-baseline.md`);
//! 2. a total excursion across the whole window under
//!    [`Tuning::quiescence_drift`] / `_drift_i` — a slow ramp passes the
//!    per-step test forever while travelling arbitrarily far, and only the
//!    window test catches it;
//! 3. a bound on the travel it has LEFT, [`Tuning::quiescence_decay`]: the
//!    decay between two consecutive windows says how fast this island is
//!    converging, so `m1²/(m0-m1)` is everything still to come, and that —
//!    not the travel already done — must be under the drift bound. (1) and
//!    (2) together only say the island has gone quiet *lately*. Without (3)
//!    a τ = 10 s tail sleeps 1 mV short of its own DC point and holds that
//!    number forever.
//!
//! So the residual an island can freeze with is under `quiescence_drift`
//! (1 µV) per node and `quiescence_drift_i` (1 nA) per branch — for any time
//! constant, not just short ones. An island that cannot demonstrate the
//! decay keeps solving: it is still moving, and local dt is what makes that
//! cheap. An island holding any time-varying source is structurally
//! ineligible — its equations depend on `t`, so freezing it would be a lie
//! the moment the clock moves. So is an island holding a `Noise` source:
//! its stream position is discrete state that only a solve advances.
//!
//! **Local dt.** Each island integrates at `h = k * dt`, `k` a power of two
//! inside a configurable band, raised only from estimates computed from the
//! island's own state — never from CPU load or a wall clock, which would
//! destroy determinism. Two budgets, bounding the two ways a bigger step
//! costs accuracy:
//!
//! - CURVATURE, [`Tuning::local_dt_err`] / `_err_i`: the second difference
//!   of the unknown vector, which is the leading term of the BE/TR local
//!   truncation error. This bounds the error made *inside* the step.
//! - MOTION, [`Tuning::local_dt_slew`] / `_slew_i`: an island's step ends on
//!   a world substep boundary, but the caller can stop the world anywhere,
//!   so the island can be up to `(k-1)·dt` behind the number it reports.
//!   That lag is first order in `h`, so it dominates everything the
//!   curvature budget bounds, and capping the travel per local step at
//!   `local_dt_slew · dt` is what holds it under `local_dt_slew · dt` volts
//!   — a bound that shrinks with the room dt, which is what makes accuracy
//!   monotone in dt.
//!
//! `k` collapses to 1 immediately on any perturbation: an edit, an interact,
//! a parameter write, a discrete device transition (op-amp rail region, 555
//! latch), an NR rescue, or either budget going over. An island sampled at a
//! rate the player can see — a scope probe, a speaker tap, a co-simulated
//! motor, a noise source feeding an audio worklet — is pinned to `k = 1`, so
//! dt dilation can never coarsen a waveform anyone is looking at.
//!
//! Both levers are deterministic functions of deterministic state, so the
//! cross-target harness is unaffected in kind (the hashes themselves move,
//! because the trajectory legitimately changes — this is a documented
//! tolerance-defined semantics, exactly as `docs/scale-parallelism.md` §5.2
//! required). [`Tuning::off`] is the yardstick: with both levers off the
//! engine is the pre-lever engine, bit for bit, on every golden.
//!
//! Unknown vector layout, per island: `[v_node1 .. v_nodeN, i_branch1 ..
//! i_branchM]` where branches are voltage-source-like elements (sources,
//! closed switches, op-amp outputs). Node 0 is ground, is shared by all
//! islands, and is not an unknown anywhere. Every node gets a `GMIN` leak to
//! ground so floating circuits stay solvable (beginner-tolerant solver).
//!
//! Sign conventions used throughout:
//! - `pin_i[p]` is the current flowing INTO the element at pin `p`.
//! - A constant current I into pin p stamps `b[p] -= I`.
//! - A dependence dI_p/dV_n stamps `a[p][n] += g`.

use crate::constraint::{constraint_of, Constraint, ConstraintKey};
use crate::netlist::{
    Wave,
    ElementKind, ElementSpec, InteractOp, LogicPins, ParamWrite, Point, MAX_PINS,
};
use sim_math::DenseLu;
use std::collections::BTreeMap;

pub const GMIN: f64 = 1e-12;

/// Thermal voltage at room temperature.
const VT: f64 = 0.025865;
/// Default diode: n = 2 emission (Falstad-family, NR-friendly).
const DIODE_IS: f64 = 1.7143528192808883e-7;
const DIODE_NVT: f64 = 2.0 * VT;
/// LED: tuned for a ~2.1 V forward drop at 20 mA.
const LED_IS: f64 = 1e-20;
const LED_NVT: f64 = 0.05;
/// Zener: n = 1 junction both directions; knee offset places 5 mA at -vz.
const ZENER_IS: f64 = 1e-14;
const ZENER_NVT: f64 = VT;
/// BJT Ebers-Moll.
const BJT_IS: f64 = 1e-14;
const BJT_BETA_R: f64 = 1.0;
/// MOSFET off-state drain-source leak, and per-iteration voltage damping.
const MOS_LEAK: f64 = 1e-8;
const MOS_DAMP: f64 = 0.5;
/// Drain-source avalanche breakdown: every real power MOSFET is
/// avalanche-rated (2N7000 and IRLZ44N are both 55-60 V), and the clamp is
/// not decoration — it is what stops an inductive turn-off from having no
/// solution at all. Switch a motor off with an off-state FET presenting
/// `MOS_LEAK` and the winding's stored current has nowhere to go: NR
/// diverges and the whole room quarantines with no diagnosis. With the
/// clamp the energy goes where it goes in the world — into the FET, which
/// gets hot and eventually lets go, and the fix a player discovers is the
/// real fix (fit a freewheel diode).
///
/// Structurally this is the `Zener` branch applied across drain-source, and
/// it is gated to be EXACTLY zero more than 40·nVt below breakdown, so no
/// circuit that never approaches 60 V changes by one bit.
const MOS_BV: f64 = 55.0;
const MOS_BV_IS: f64 = 1e-3;
const MOS_BV_NVT: f64 = 0.15;
/// Voltage below breakdown at which the avalanche term is forced to exactly
/// zero. 40 e-foldings down is 4e-18 of the knee current — arithmetically
/// nothing, and making it structurally nothing is what guarantees that no
/// circuit which stays clear of `MOS_BV` changes by a single bit.
const MOS_BV_MARGIN: f64 = 40.0 * MOS_BV_NVT;
/// Op-amp open-loop gain and input offset voltage. The offset is a real
/// device property, and it matters here: an ideal offset-free op-amp in a
/// positive-feedback loop has an exact metastable solution that a
/// noiseless deterministic solver would sit on forever — the offset is
/// what lets relaxation oscillators and flip-flops self-start.
const OPAMP_GAIN: f64 = 1e5;
const OPAMP_VOFF: f64 = 1e-4;
/// OTA bias-pin diode (LM13700-style: Iabc injected into a junction).
const OTA_IS: f64 = 1e-14;
/// Bipolar 555: totem-pole output drops (sourcing from VCC / sinking to
/// GND), the saturated discharge transistor's conductance (10 Ω), and the
/// quiescent supply conductance (~3 mA across a 9 V rail).
const T555_VDROP_HIGH: f64 = 1.2;
const T555_VSAT_LOW: f64 = 0.1;
const T555_G_DIS: f64 = 0.1;
const T555_G_QUIESCENT: f64 = 3.3e-4;
/// Comparator thresholds as fractions of the live supply, from the
/// internal 3-resistor divider.
const T555_THR_FRAC: f64 = 2.0 / 3.0;
const T555_TRIG_FRAC: f64 = 1.0 / 3.0;

// ------------------------------------------------------- CMOS logic family
//
// The output stage is a PAIR OF SWITCHED CONDUCTANCES to the two supply
// pins — literally what CMOS is — so a logic chip is a passive network whose
// values are picked by discrete state. It owns no branch unknown and writes
// nothing into `b`: it is a switch network, not a source. Three things fall
// out of that, and they are the reasons the model is shaped this way:
//
//  * it cannot create energy, so `elem_power`'s default `Σ v·i` is exactly
//    its own dissipation and this family needs no exception the way `OpAmp`
//    does (`Σ v·i` = `Σ g·(Δv)²` ≥ 0, provably);
//  * the a-matrix INCIDENCE PATTERN is identical in every discrete state
//    (both conductances are always stamped, only the values move), which is
//    what makes `validate::probe_solvable`'s single cold factorization sound
//    for this family — the 555's region-dependent branch row is not;
//  * no branch unknown means one fewer row per output than a 555-shaped
//    model, and an O(n³) factorization is the family's real cost.
/// On-resistance of one output FET, and the off-state / leakage pair.
///
/// 50 Ω is chosen against real failure modes rather than a datasheet
/// typical: an output shorted to a rail passes 100 mA at 5 V and burns
/// 500 mW, which is over the DIP tier and kills the part in a couple of
/// seconds (a real 74HC sources 25-50 mA into a short and a sustained short
/// does destroy it). A 1 kΩ load sags the output to 0.971·vcc; a 100 Ω load
/// sags it to 0.667·vcc, below the receiver's own threshold — a genuine
/// design lesson delivered entirely by the solver.
const LOGIC_R_ON: f64 = 50.0;
const LOGIC_G_ON: f64 = 1.0 / LOGIC_R_ON;
const LOGIC_G_OFF: f64 = 1e-9;
/// Input leak to EACH rail. Two structural jobs: a floating input node can
/// never be singular, and a floating input parks at exactly vcc/2 — dead in
/// the middle of the hysteresis band, so the Schmitt latch HOLDS and the
/// gate deterministically ignores it.
///
/// Symmetric on purpose. A pull-down would make a floating input read LOW,
/// which is convenient and is a lie: real CMOS floats, and "floating inputs
/// are the #1 CMOS beginner bug" is exactly the lesson this family should
/// teach. The honest version needs a DIAGNOSTIC, not a fudge — the client
/// can see from the solver's own node voltage that a pin is sitting in the
/// indeterminate band and say so.
const LOGIC_G_IN: f64 = 1e-9;
/// Static supply current: 1 µA at 5 V. Honest quiescent CMOS, and the same
/// role `T555_G_QUIESCENT` plays — the rails carry current with every output
/// unloaded, so KCL stays sane.
///
/// NOT modelled, and stated rather than hidden: dynamic `C·V·f` supply
/// current, which dominates a real CMOS part above a few kHz. These chips
/// run cooler than real ones at high clock rates. Charging for it would mean
/// inventing a number no solver produced; doing it honestly needs real
/// internal node capacitance.
const LOGIC_G_QUIESCENT: f64 = 2e-7;
/// Schmitt thresholds as fractions of the LIVE supply, so a 3 V rail is 3 V
/// logic and a rail sagging under load drags the logic levels down with it.
///
/// Hysteresis on EVERY input is a deliberate deviation from a plain 74HC00,
/// which has a single ~0.5·VCC threshold. Without it a gate inside any real
/// feedback loop degenerates into substep-rate chatter that neither diverges
/// nor quarantines — it silently produces a plausible-looking wrong
/// waveform, which is the worst failure mode available. The 555's 1/3-2/3
/// divider is the same trick.
///
/// What the deviation costs: the model is optimistic about noise. A slow,
/// noisy ramp into a real HC gate produces a burst of edges; here it
/// produces one clean edge, so this family will never teach "add a Schmitt
/// trigger to clean up a slow edge" — every input already is one.
const LOGIC_TH_HI: f64 = 0.65;
const LOGIC_TH_LO: f64 = 0.35;
/// CMOS latch-up. A parasitic SCR fires when a pin is driven far outside the
/// chip's own rails, or the supply exceeds absolute maximum, and the part
/// becomes a short across its supply until power is removed.
///
/// Modelling it is what makes overvoltage PHYSICALLY DISSIPATIVE instead of
/// judged by a second damage metric, and that matters because a `Tier`
/// carries one metric per rung: without latch-up the family would have to
/// choose between catching a shorted output and catching a 9 V rail. With
/// it, `Metric::Power` catches both. A 74HC on the hoist's 9 V rail latches,
/// burns 8.1 W against a 0.35 W package, and dies — a good game moment, and
/// the correct one.
///
/// 7.0 V is the 74HC/74HCT absolute maximum supply. It is a property of the
/// DIE, so it is a constant of the modelled family and not a `tier` lookup:
/// `ElementSpec::tier` must never reach a stamp (it would make `state_hash`
/// depend on a render/damage field), and `dstate` is hashed. A CD4000B-class
/// 18 V part is a different die and would be a new field on the kind, not a
/// rung of this ladder.
const LOGIC_V_ABSMAX: f64 = 7.0;
/// How far outside its own rails a pin may be driven before the SCR fires.
const LOGIC_V_LATCH_MARGIN: f64 = 1.0;
/// The latched short: 10 Ω across VCC-GND.
const LOGIC_G_LATCHUP: f64 = 0.1;
/// Supply below this clears the latch — i.e. a power cycle, which is what
/// actually clears latch-up in the world.
const LOGIC_V_UNLATCH: f64 = 1.0;

const NR_MAX_ITERS: usize = 100;
const NR_ABSTOL: f64 = 1e-6;
const NR_RELTOL: f64 = 1e-3;
/// dt-halving rescue depth: 2^4 = 16x finer before quarantine.
const RESCUE_DEPTH: u32 = 4;
/// Steps integrated with backward Euler after a discontinuity (edit,
/// switch flip) to kill trapezoidal ringing.
const BE_STEPS_AFTER_EVENT: u32 = 2;

const TWO_PI: f64 = core::f64::consts::TAU;
/// 1/2pi, so a phase in radians becomes a phase in TURNS with one multiply.
/// Precomputed as a constant rather than divided at each use so that every
/// source in the room does the identical operation, in the identical order.
const INV_TWO_PI: f64 = 1.0 / core::f64::consts::TAU;

/// Tunable thresholds for the two work-skipping levers. Every field is a
/// property of the *state*, never of the machine: nothing here may ever be
/// derived from a wall clock, a load average or a thread count, because the
/// simulation has to produce identical numbers on a phone and on a server.
///
/// The defaults are the criteria measured in `docs/scale-baseline.md`,
/// tightened where the measurement could not rule out a slow drift.
///
/// ## Volts and amps are different quantities
///
/// The unknown vector holds node voltages AND branch currents. Every
/// threshold below therefore comes in a pair: the `_i` field is the same
/// criterion for the branch (current) unknowns. A volts threshold applied
/// to an amp is not a conservative approximation, it is a category error —
/// 0.05 "V/s" against a current ramp is 0.05 A/s, a thousand times looser
/// than intended, and it is what let a source branch current ramp straight
/// through the sleep test. The `_i` defaults are the voltage figure over a
/// 1 kΩ reference impedance, which is the scale of the shipped catalogue's
/// circuits (a 10 V rail through a kΩ-order resistor is a mA-order
/// current).
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Skip islands that have gone electrically static.
    pub quiescence: bool,
    /// Volts per second. An island step whose largest NODE unknown moved
    /// faster than this is moving. 0.05 V/s = 1 µV per 20 µs substep, the
    /// figure the baseline sweep validated (0 exceptions across four
    /// sweeps).
    pub quiescence_slew: f64,
    /// Amps per second: the same test for the BRANCH unknowns.
    pub quiescence_slew_i: f64,
    /// Volts. Total excursion of any node unknown allowed across a whole
    /// `quiescence_hold` window. This is the guard the per-step slew test
    /// cannot provide: a monotone crawl of 0.9 µV per substep passes the
    /// slew test indefinitely while travelling half a volt.
    ///
    /// On its own this bound is NOT enough to make sleeping safe: a
    /// first-order tail that is inside it can still be hiding
    /// `quiescence_drift × τ / quiescence_hold` volts of unfinished travel,
    /// which is 10 mV for τ = 100 s. [`Tuning::quiescence_decay`] is what
    /// closes that gap; this field then sets the scale of the residual.
    pub quiescence_drift: f64,
    /// Amps: the same window test for the BRANCH unknowns.
    pub quiescence_drift_i: f64,
    /// Seconds of sim time both conditions must hold before a window
    /// completes. 10 ms = the 500-substep window of the baseline
    /// measurement.
    pub quiescence_hold: f64,
    /// The largest per-window decay ratio the settle test will trust, and
    /// the switch that turns the test off (`>= 1.0` disables it, which is
    /// exactly the criterion that shipped before it existed — the
    /// regression tests use that to reproduce the trap).
    ///
    /// An island may sleep only once the travel it has LEFT is under
    /// `quiescence_drift`. Slew and drift only bound the travel it has
    /// recently DONE, which is a different quantity and a far weaker one:
    /// a first-order tail inside the drift bound is still hiding
    /// `drift × τ / quiescence_hold` volts of unfinished travel, 1 mV at
    /// τ = 10 s and 10 mV at τ = 100 s, and sleeping makes that permanent.
    ///
    /// The travel left is measurable. Across two consecutive hold windows a
    /// relaxing unknown travels `m0` then `m1 = ρ·m0`, and everything after
    /// this window sums to `m1·ρ/(1-ρ) = m1²/(m0-m1)` — no model fitted,
    /// just the island's own decay read off its own state. Two whole
    /// windows is what makes it resolvable: a 10 ms baseline against a 20 µs
    /// step is five hundred times more signal, and `m0-m1` is checked
    /// against the f64 noise floor of the differences it came from before it
    /// is believed.
    ///
    /// A ramp (`ρ = 1`), and any tail whose τ dwarfs the window badly enough
    /// that `m0-m1` drowns in rounding, can never satisfy that — so they
    /// keep solving. They are still moving; local dt is what makes it cheap.
    pub quiescence_decay: f64,
    /// Let islands integrate at multiples of the room dt.
    pub local_dt: bool,
    /// Volts of local truncation error per island step that the dt
    /// controller is allowed to spend. Estimated by the second difference
    /// of the unknown vector, which is `h² y''` — exactly the leading BE
    /// error term and an upper bound on the TR one.
    pub local_dt_err: f64,
    /// Amps: the same budget for the BRANCH unknowns.
    pub local_dt_err_i: f64,
    /// Volts per second. The staleness governor, and the reason accuracy is
    /// monotone in the room dt.
    ///
    /// An island that integrates at `h = k·dt` finishes its step on a world
    /// substep boundary, but the caller can stop the world anywhere: the
    /// island is then up to `(k-1)·dt` of world time behind what it
    /// reports. That lag is a FIRST-order error (`slew × lag`), so it dwarfs
    /// the truncation error the curvature controller bounds, and — because
    /// the curvature budget is absolute — a finer room dt used to buy a
    /// bigger `k` and leave the lag exactly where it was. Refining dt made
    /// the answer worse.
    ///
    /// So `k` may only rise while the island's motion across one local step
    /// stays under `local_dt_slew · dt`, i.e. while `slew · k ≤
    /// local_dt_slew`. The read-out lag is then under `local_dt_slew · dt`
    /// volts *whatever `k` is*: it shrinks linearly with the room dt and
    /// vanishes with it, which is what makes total error monotone in dt.
    /// At the shipped 20 µs that ceiling is 100 µV.
    ///
    /// The test is on the step just taken, so an island that accelerates
    /// can overshoot it once, by whatever the acceleration was worth —
    /// bounded in turn by `local_dt_err`, since curvature is what
    /// acceleration is. One step later `k` is back at 1. The honest bound
    /// on the lag is therefore `local_dt_slew · dt + local_dt_err`.
    pub local_dt_slew: f64,
    /// Amps per second: the same governor for the BRANCH unknowns.
    pub local_dt_slew_i: f64,
    /// Consecutive island steps comfortably inside budget before `k`
    /// doubles. Deliberately slow to rise and instant to fall.
    pub local_dt_hold: u32,
    /// Hard ceiling on `k`.
    pub local_dt_max_k: u32,
    /// Seconds. Hard ceiling on any island's `h`, whatever `k` says. Bounds
    /// how far behind world time an island may sit in TIME, where
    /// `local_dt_slew` bounds it in volts: at 500 µs an island is at most 3%
    /// of a 60 Hz frame stale.
    pub local_dt_max: f64,
    /// Samples per cycle guaranteed for an island holding a time-varying
    /// source. The error controller would catch aliasing anyway; this is a
    /// structural belt to its braces.
    pub local_dt_min_samples: f64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            quiescence: true,
            quiescence_slew: 0.05,
            quiescence_slew_i: 5e-5,
            quiescence_drift: 1e-6,
            quiescence_drift_i: 1e-9,
            quiescence_hold: 10e-3,
            quiescence_decay: 0.9999,
            local_dt: true,
            local_dt_err: 1e-4,
            local_dt_err_i: 1e-7,
            local_dt_slew: 5.0,
            local_dt_slew_i: 5e-3,
            local_dt_hold: 64,
            local_dt_max_k: 32,
            local_dt_max: 500e-6,
            local_dt_min_samples: 64.0,
        }
    }
}

impl Tuning {
    /// Both levers off: the engine steps every island at the room dt, every
    /// substep. The yardstick every measurement of the levers is taken
    /// against, and the configuration an observational experiment must use
    /// so it measures the world instead of its own skipping.
    pub fn off() -> Self {
        Tuning {
            quiescence: false,
            local_dt: false,
            ..Tuning::default()
        }
    }
}

// ---------------------------------------------------------- noise stream
//
// A deterministic solver has no thermal agitation of its own, so a noise
// source has to carry its own. The requirement that makes this delicate is
// the project's determinism invariant: native and wasm32 must agree BIT FOR
// BIT, forever, across saves. That rules out anything seeded from a clock or
// the OS, and it rules out float-state generators (a float recurrence is
// exactly reproducible in principle but leaves no margin, and nothing here
// needs one). What follows is integer-only.

/// SplitMix64 finalizer over `(seed, n)`. Counter-based on purpose: the
/// word is a pure function of its inputs, so nothing has to be carried
/// forward except an integer index, and any state rollback (`step()`'s
/// rescue path, a save/reload) reproduces the stream exactly.
///
/// `wrapping_mul`/`wrapping_add`/xor/shift on `u64` are exact on every
/// target — no FMA, no libm, no float rounding anywhere in the advance.
#[inline]
fn noise_word(seed: u32, n: u64) -> u64 {
    // The trailing constant breaks the finalizer's fixed point: without it
    // seed 0 at n = 0 hashes to 0, i.e. the default noise source would open
    // with one sample pinned at exactly -volts.
    let mut z = (seed as u64)
        .wrapping_mul(0xD1B5_4A32_D192_ED03)
        .wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0xA076_1D64_78BD_642F);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform on [-1, 1). The top 32 bits of the word scaled by 2/2^32 — an
/// exact power of two — then shifted by 1: `u32 -> f64` is exact, the
/// multiply is exact, the subtraction is exact. The map introduces no
/// rounding at all, so it cannot differ between targets even in principle.
///
/// Mean 0, RMS 1/sqrt(3) = 0.577350, peak just under 1.
#[inline]
fn noise_unit(seed: u32, n: u64) -> f64 {
    ((noise_word(seed, n) >> 32) as u32 as f64) * (2.0 / 4_294_967_296.0) - 1.0
}

#[derive(Clone, Copy, Default)]
struct ElemState {
    /// Companion history: capacitor voltage / inductor current at the
    /// previous accepted step. A `Noise` source borrows `v_prev` for the
    /// EMF it holds constant across the current step's NR iterations (it
    /// has no companion model of its own, and reusing the slot keeps it
    /// in the state digest and in `step()`'s snapshot for free).
    v_prev: f64,
    i_prev: f64,
    /// Junction-voltage NR guesses: diode vd / BJT (vbe, vbc) — stored
    /// polarity-normalized so PNP shares the NPN code path.
    vg1: f64,
    vg2: f64,
    /// Op-amp rail region: -1, 0 (linear), +1. Doubles as the 555's RS
    /// latch: 0 = output low, 1 = output high.
    region: i8,
    /// Damped per-pin voltages for MOSFET NR stabilization.
    lastv: [f64; MAX_PINS],
    /// Currents INTO the element per pin, from the last accepted step.
    pin_i: [f64; MAX_PINS],
    /// A `Noise` source's position in its own PRNG stream. Counter-based,
    /// so the sample is a pure function of (seed, n) and restoring this
    /// integer restores the generator exactly — `Default` (0) is a valid
    /// start and there is no "uninitialized" sentinel to get wrong.
    noise_n: u64,
    /// Discrete state for the CMOS logic family. `region` is an `i8` and a
    /// 4-bit shift register needs four data bits plus up to four input
    /// hysteresis latches plus a clock-history bit plus latch-up, so the
    /// family gets a word of its own rather than overloading `region` a
    /// third time.
    ///
    /// ```text
    ///   0..8    data      Q0..Q3 / gate output / decoded mux select
    ///   8..16   schmitt   per-input hysteresis latch, in pin order
    ///   16      clk_prev  the clock's Schmitt level at the last accepted step
    ///   17      latched   CMOS latch-up: sticky until the supply is removed
    ///   18..32  reserved
    /// ```
    ///
    /// All-zeros is the DEFINED power-up state — every bit low, nothing
    /// latched — which is what makes a shift register read a real 0 V on
    /// every output from the first substep instead of sitting at an
    /// indeterminate half-rail until something writes it.
    dstate: u32,
}

// `dstate` bit positions. See `ElemState::dstate`.
const D_DATA: usize = 0;
const D_SCHMITT: usize = 8;
const D_CLK_PREV: usize = 16;
const D_LATCHED: usize = 17;
/// Base of the PT2399's two internal op-amp clamp states, two bits each
/// (bits 18..22). They live here rather than in `region` because `region` is
/// one `i8` and this part has two of them — and because a chip's internal
/// stage states belong with the other discrete state, where the digest and
/// the rescue snapshot already carry them.
const D_PT_OA: usize = 18;

#[inline]
fn dbit(d: u32, i: usize) -> bool {
    (d >> i) & 1 != 0
}

#[inline]
fn dset(d: &mut u32, i: usize, v: bool) {
    if v {
        *d |= 1 << i;
    } else {
        *d &= !(1 << i);
    }
}

/// Read an `n`-bit field starting at `lo`.
#[inline]
fn dfield(d: u32, lo: usize, n: usize) -> u32 {
    (d >> lo) & ((1u32 << n) - 1)
}

/// Write an `n`-bit field starting at `lo`.
#[inline]
fn dset_field(d: &mut u32, lo: usize, n: usize, v: u32) {
    let mask = ((1u32 << n) - 1) << lo;
    *d = (*d & !mask) | ((v << lo) & mask);
}

struct CompiledElem {
    spec: ElementSpec,
    /// Electrical node index per pin, LOCAL to the owning island (0 =
    /// ground, which every island shares; unused pins 0).
    node: [usize; MAX_PINS],
    /// Index into the branch-current unknowns for branch devices. Members of
    /// a merged ideal-constraint group all point at the SAME index.
    branch: Option<usize>,
    /// How many elements share this branch unknown (1 = sole owner). A merged
    /// member reports `i / share_n` — see `accept`.
    share_n: u32,
    /// ±1: the sign this element reads its share of the branch current with.
    /// −1 only for a merged member drawn with its pins in the opposite order
    /// to the group's leader.
    share_sign: f64,
    /// This element writes the branch row. False only for a non-leader member
    /// of a merged group: the leader already wrote it, and stamping again
    /// would accumulate the ±1 incidence to ±N.
    stamps: bool,
    state: ElemState,
    /// The part has failed OPEN: it stamps nothing, owns no branch unknown
    /// and carries no current. Its pins remain junction points, so anything
    /// else wired to them keeps working — a dead part is a gap, not a hole
    /// in the netlist.
    ///
    /// This is the ONLY damage mechanism inside sim-core. Ratings, thermal
    /// accumulators and the decision to break live outside the solve path
    /// (see `crates/damage`), because none of that is numerics and none of
    /// it may perturb the golden state hashes.
    broken: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AdvanceReport {
    /// Substeps of world time taken: the most any island advanced.
    pub steps: u32,
    /// Newton-Raphson iterations summed over every island.
    pub nr_iters: u32,
    pub rescues: u32,
    /// True when at least one island is quarantined.
    pub quarantined: bool,
    /// Islands that actually ran the solver.
    pub islands: u32,
    /// Islands that were asleep for this whole call and did no work at all.
    /// Their reported state is the DC solution they last solved for.
    pub static_islands: u32,
    /// Island-level integration steps executed, summed over islands. With
    /// local dt one such step covers `k` world substeps, so this — not
    /// `steps` — is the divisor that turns `nr_iters` into iterations per
    /// solve.
    pub island_steps: u32,
}

impl AdvanceReport {
    fn merge(&mut self, other: AdvanceReport) {
        self.steps = self.steps.max(other.steps);
        self.nr_iters += other.nr_iters;
        self.rescues += other.rescues;
        self.quarantined |= other.quarantined;
        self.islands += other.islands;
        self.static_islands += other.static_islands;
        self.island_steps += other.island_steps;
    }
}

/// An O(1) handle to one compiled element, for callers that sample the same
/// element far more often than once per tick (audio taps). Obtained from
/// [`Engine::tap`]; invalidated by [`Engine::set_elements`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElemTap {
    island: usize,
    slot: usize,
}

/// Per-element view of the live simulation for rendering: everything the
/// client paints comes from here and nowhere else.
#[derive(Clone, Copy, Debug, Default)]
pub struct ElemFrame {
    pub id: u32,
    pub npins: usize,
    /// Voltage at each pin.
    pub v: [f64; MAX_PINS],
    /// Current INTO the element at each pin (wires included, via KCL
    /// propagation).
    pub i: [f64; MAX_PINS],
    /// Dissipated power (negative = delivering power).
    pub power: f64,
    /// The island this element sits on has been quarantined: these numbers
    /// are the last ones its solver produced and they are not moving again
    /// until the build changes. A consumer that INTEGRATES a frame — the
    /// damage model integrates dissipated power over sim time — must skip
    /// these, or it is cooking a part on stale numbers. A consumer that
    /// merely draws them is fine: they are the truth as far as it goes.
    ///
    /// Per element, because quarantine is per island: one diverging build in
    /// the corner of the room must not switch anything off for the rest of
    /// it.
    pub quarantined: bool,
}

/// Numbers per element in the flat wire layout: `[id, npins, v0..vN, i0..iN,
/// power]`. Lives HERE, next to `MAX_PINS`, because it is a function of it —
/// it used to be a `const` in `sim-wasm` with a compile-time assert, a
/// spelled-out `[f64; 15]` in the server and a `15` in TypeScript, and the
/// server's copy was the dangerous one: an array literal with `f.v[0]…f.v[5]`
/// written out compiles at ANY ceiling and simply stops sending the pins past
/// six. Every renderer would draw the extra legs dead and nothing anywhere
/// would go red.
pub const FRAME_STRIDE: usize = 3 + 2 * MAX_PINS;

impl ElemFrame {
    /// Push this frame's `FRAME_STRIDE` numbers in wire order. The one
    /// implementation both transports use, so the browser build and the
    /// server build cannot disagree about the layout.
    pub fn pack(&self, mut push: impl FnMut(f64)) {
        push(f64::from(self.id));
        push(self.npins as f64);
        for v in self.v {
            push(v);
        }
        for i in self.i {
            push(i);
        }
        push(self.power);
    }
}

/// One electrically independent circuit and everything needed to solve it.
///
/// Islands share nothing: no matrix entry, no unknown, no factorization, no
/// convergence verdict, no quarantine. That is what makes stepping them in
/// any order — or on any number of threads — bit-identical to stepping them
/// in this one.
/// Clock thresholds for a BBD's CLK pin, in volts above its GND pin.
///
/// Absolute rather than supply-relative because the part has no supply pin:
/// a bucket brigade's clock is an external square wave, and these are the
/// levels a CMOS driver running on 5 V comfortably clears. The gap is the
/// hysteresis, so a slow or noisy clock edge still produces exactly one
/// transition instead of a burst of them.
const BBD_CLK_HI: f64 = 2.0;
const BBD_CLK_LO: f64 = 1.0;
/// Tether on a BBD's IN and CLK pins, in siemens (1 uS = 1 MΩ).
///
/// These pins draw essentially nothing — a real bucket brigade's input is a
/// MOS gate — but "essentially nothing" and "no path at all" are different
/// things to a matrix: an unconnected input with no route anywhere makes the
/// row singular. Same tether an op-amp input carries, and small enough that
/// it never loads what drives it.
const BBD_G_IN: f64 = 1e-6;
/// PT2399 output source impedance, as a conductance (1 mS = 1 kΩ).
///
/// A real buffered chip output is not ideal, and here that is load-bearing
/// rather than pedantry: giving the output a source impedance frees this
/// part's single branch unknown for the RT reference, where the current
/// being measured is the input to everything else.
const PT_G_OUT: f64 = 1e-3;

/// A bucket chain: the samples, where the next one lands, and enough to undo
/// the most recent write.
#[derive(Clone, Debug)]
struct DelayLine {
    /// One entry per bucket. Power-up state is all zeros, which is a real,
    /// defined state (an uncharged chain), not a sentinel.
    buf: Vec<f64>,
    /// Where the NEXT sample is written, which is also where the OLDEST
    /// sample currently sits — that coincidence is the whole trick, and it
    /// is why the read costs nothing.
    w: usize,
    /// `(index, value)` of the last write, so a rejected step can be undone
    /// in O(1). Without this the rescue ladder would have to snapshot the
    /// whole buffer every step, which is the one thing that would make a
    /// long delay expensive.
    undo: Option<(usize, f64)>,
}

#[cfg(test)]
mod delay_line_tests {
    use super::DelayLine;

    /// The chain is FIFO and the read costs nothing because `w` is both the
    /// write slot and the oldest sample.
    #[test]
    fn a_chain_is_first_in_first_out() {
        let mut d = DelayLine::new(3, 0.0);
        // A fresh chain is full of its power-up zeros, so the first three
        // pushes get those back.
        assert_eq!(d.shift(1.0), 0.0);
        assert_eq!(d.shift(2.0), 0.0);
        assert_eq!(d.shift(3.0), 0.0);
        // ...and then what was put in, in order.
        assert_eq!(d.shift(4.0), 1.0);
        assert_eq!(d.shift(5.0), 2.0);
        assert_eq!(d.shift(6.0), 3.0);
    }

    /// Rollback restores the exact pre-shift chain, which is what a rejected
    /// step needs. Without it a rescue would leave a sample in the line and
    /// the delay would lengthen by one bucket every time the solver
    /// struggled.
    #[test]
    fn rollback_undoes_exactly_one_shift() {
        let mut d = DelayLine::new(4, 0.0);
        for v in [1.0, 2.0, 3.0, 4.0] {
            d.shift(v);
        }
        let before = (d.buf.clone(), d.w);
        d.shift(99.0);
        assert_ne!((d.buf.clone(), d.w), before, "the shift must have done something");
        d.rollback();
        assert_eq!((d.buf.clone(), d.w), before, "rollback must restore it exactly");
        // Idempotent: a second rollback is not a second step backwards, so a
        // rescue that recurses cannot walk the chain backwards.
        d.rollback();
        assert_eq!((d.buf, d.w), before);
    }
}

impl DelayLine {
    /// `rest` is what the chain holds before anything has been put in it.
    ///
    /// Zero for a bucket brigade, which has no reference of its own — but
    /// the PT2399's whole signal path sits at REF, and a chain that powers
    /// up at 0 V hands the stage after it a 2.5 V step it never asked for.
    /// That slams the output op-amp into its rail for a full delay period at
    /// power-up, which was invisible while those op-amps could swing
    /// anywhere and became a quarantined room the moment they could not.
    fn new(stages: usize, rest: f64) -> Self {
        DelayLine {
            buf: vec![rest; stages.max(2)],
            w: 0,
            undo: None,
        }
    }

    /// Push one sample in, return the one that falls out the far end.
    fn shift(&mut self, v: f64) -> f64 {
        let i = self.w;
        self.undo = Some((i, self.buf[i]));
        // Read BEFORE the write: `w` holds the oldest sample, and the new one
        // takes its place.
        let out = self.buf[i];
        self.buf[i] = v;
        self.w = if i + 1 == self.buf.len() { 0 } else { i + 1 };
        out
    }

    /// Undo the most recent `shift`. Idempotent: a step that shifted nothing
    /// has nothing to undo, and undoing twice is not a second rollback.
    fn rollback(&mut self) {
        if let Some((i, v)) = self.undo.take() {
            self.buf[i] = v;
            self.w = i;
        }
    }
}

pub struct Island {
    /// Elements that take part in the solve occupy `0..active`; the rest
    /// (wires, grounds, broken parts, parts with every pin on ground) are
    /// parked behind them. Parked elements are pure no-ops in stamping, NR
    /// and accept, so the hot loops never visit them — in a real, wire-heavy
    /// document that is about half the document.
    elems: Vec<CompiledElem>,
    active: usize,
    num_nodes: usize,
    num_branches: usize,
    n: usize,
    a: Vec<f64>,
    b: Vec<f64>,
    x: Vec<f64>,
    lu: DenseLu,
    /// Per-substep `ElemState` snapshots for the rescue ladder, one reusable
    /// buffer per recursion depth so a substep allocates nothing.
    saved: Vec<Vec<ElemState>>,
    /// Linear circuits factor once per edit and reuse. The factorization
    /// is only valid for the (step size, integration mode) it was stamped
    /// with — companion conductances depend on both. This is a property of
    /// THIS island: a diode next door is not this island's problem.
    linear: bool,
    /// True if ANY live element IN THIS ISLAND needs Newton iteration, i.e.
    /// writes into `a` as a smooth function of the operating point (diode,
    /// BJT, MOS, OTA). An island free of those has a matrix that is constant
    /// between discrete events even when it is not linear — see `reusable`.
    ///
    /// Per island is what makes it useful. As one global flag, a single
    /// diode anywhere in the room disarmed factorization reuse for every
    /// 555 and every op-amp in it.
    smooth_nonlinear: bool,
    factor_valid: bool,
    factored_h: f64,
    factored_be: bool,
    /// Copy of [`Engine`]'s instrumentation knob, so `build` can read it
    /// without reaching back out of the island. Written by `rebuild` and by
    /// `Engine::set_reuse_pwl`, never by the solve.
    reuse_pwl: bool,
    /// Indices into `elems` of the sources whose waveform JUMPS (square and
    /// sawtooth). Almost always empty — a room with no such source pays one
    /// `is_empty()` per step and nothing else.
    ///
    /// They are tracked because a jump is an EVENT, and the trapezoidal rule
    /// assumes the state moved smoothly across the step it is integrating.
    /// Drive a capacitor from a square wave and trapezoid will ring on every
    /// edge — the same failure the logic family hit, where an output reached
    /// +5.53 V and -0.55 V on a 5 V rail and would have destroyed healthy
    /// parts through the damage model. The engine already owns the cure: the
    /// backward-Euler steps it takes after a switch flip. A waveform edge
    /// arms them exactly the same way, and for exactly the same reason.
    edge_sources: Vec<usize>,
    /// Bucket chains, keyed by ELEMENT ID rather than by document index.
    ///
    /// Deliberately a side table and not a field on `CompiledElem`:
    ///
    /// * keying by id means a chain survives `compile()` for free, exactly
    ///   as `old_state` does. Editing anything else in the room must not
    ///   empty a delay line that is mid-echo;
    /// * `CompiledElem` stays small and `Copy`-ish, so the rescue ladder's
    ///   per-step state snapshot does not clone thousands of samples. A
    ///   delay only ever writes ONE sample per clock edge, so its rollback
    ///   is the single overwritten value (see `DelayLine::undo`) rather than
    ///   a copy of the buffer.
    delays: BTreeMap<u32, DelayLine>,
    be_steps: u32,
    quarantined: bool,
    /// Count of numeric factorizations since construction (instrumentation
    /// only; never read by the solver, never hashed).
    factorizations: u64,

    // ------------------------------------------- quiescence and local dt
    tuning: Tuning,
    /// `x` at the previous / second-previous accepted island step. The
    /// first difference is the motion test, the second difference is the
    /// integration-error estimate.
    x_prev: Vec<f64>,
    x_prev2: Vec<f64>,
    /// `x` when the current run of quiet steps began: the window-drift test
    /// measures against this, not against the previous step.
    x_mark: Vec<f64>,
    /// Per-unknown travel across the PREVIOUS completed quiet window. The
    /// ratio between this and the current window's travel is the island's
    /// own decay rate, which is what bounds the travel it has left — see
    /// [`Tuning::quiescence_decay`].
    win_prev: Vec<f64>,
    /// Is `win_prev` a real measurement? False until one full quiet window
    /// has completed, and again after anything breaks the run of quiet.
    have_win: bool,
    /// Accepted steps since the last event that invalidated the history
    /// (compile, wake, `k` change, rescue). `x_prev`/`x_prev2` only mean
    /// what they claim at 1 and 2.
    hist: u8,
    /// Sim seconds for which both quiescence conditions have held.
    quiet_t: f64,
    asleep: bool,
    /// No time-varying source and no noise source in this island, so its
    /// equations do not depend on `t` and freezing it cannot lose anything.
    sleepable: bool,
    /// Largest source frequency in this island, 0 when it is DC-only.
    ac_hz: f64,
    /// Structurally sampled faster than the tick: holds a speaker (audio
    /// rate), a co-simulated motor (machine rate) or a noise source (one
    /// fresh sample per substep, by specification).
    pinned: bool,
    /// A caller has told us a player is watching this island through an
    /// instrument. Cleared and re-set by [`Engine::set_sampled`].
    sampled: bool,
    /// Local dt multiple: this island integrates at `h = k * dt`.
    k: u32,
    /// Consecutive accepted steps whose error estimate was comfortably
    /// inside budget, counted towards doubling `k`.
    good: u32,
    /// World substeps this island owes. Non-zero only between calls, and
    /// only when `k > 1`: an island that cannot afford a whole local step
    /// yet carries the debt rather than taking a short one.
    pending: u32,
    /// A discrete device transition (op-amp rail region, 555 latch) happened
    /// in the step just solved. Not a numeric quantity — a topology change,
    /// and the sharpest "this island is not resting" signal there is.
    discrete_moved: bool,
}

pub struct Engine {
    islands: Vec<Island>,
    /// Document order -> `(element id, island, slot)`. Everything the
    /// outside world sees — `frame()`, `state_hash()`, id lookups — walks
    /// this, so partitioning never reorders the document.
    order: Vec<(u32, usize, usize)>,
    /// Junction (geometric point) -> `(island, island-local node)`. Node 0
    /// is ground in every island, so its island index is not meaningful.
    junctions: Vec<(Point, usize, usize)>,
    /// Factorizations performed by islands that no longer exist, so the
    /// instrumentation counter keeps meaning "since construction" across
    /// edits (which rebuild the partition from scratch).
    retired_factorizations: u64,
    /// Bucket chains in transit between `take_doc` (which dismantles the
    /// islands) and `rebuild` (which re-homes them). Empty at every other
    /// moment; anything still in here after a rebuild belonged to a part
    /// that no longer exists, and is dropped with it.
    orphan_delays: BTreeMap<u32, DelayLine>,
    dt: f64,
    time: f64,
    tuning: Tuning,
    /// Instrumentation/test knob: when false the piecewise-linear
    /// factorization reuse is disabled and every substep refactors, exactly
    /// as before the event-driven path existed. Solver output is identical
    /// either way — `crates/sim-golden/tests/pwl_reuse.rs` asserts that on
    /// every golden, per island, both on the state hash and on the raw
    /// matrix bits, which is what keeps the `is_discrete_nonlinear`
    /// classification honest as devices are added.
    ///
    /// Authority lives here and is copied into every island at `rebuild`,
    /// exactly like `tuning`.
    reuse_pwl: bool,
    /// Element ids a caller has declared it samples faster than the tick.
    /// Kept on the engine (not the islands) because the partition is rebuilt
    /// from scratch on every edit and the declaration must survive that.
    sampled: Vec<u32>,
}

impl Engine {
    pub fn new(dt: f64) -> Self {
        Engine {
            islands: Vec::new(),
            order: Vec::new(),
            junctions: Vec::new(),
            retired_factorizations: 0,
            orphan_delays: BTreeMap::new(),
            dt,
            time: 0.0,
            tuning: Tuning::default(),
            reuse_pwl: true,
            sampled: Vec::new(),
        }
    }

    // -------------------------------------------------------------- tuning

    pub fn tuning(&self) -> Tuning {
        self.tuning
    }

    /// Replace the quiescence / local-dt thresholds. Wakes every island and
    /// resets every local dt, so the new settings take effect from a clean
    /// state rather than from decisions the old ones made.
    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.tuning = tuning;
        for island in self.islands.iter_mut() {
            island.tuning = tuning;
            island.wake();
        }
    }

    /// Declare the elements a player-visible instrument is reading faster
    /// than the tick — scope probes, measurement chips, audio taps. The
    /// islands that own them are pinned to `k = 1`, so no waveform anyone is
    /// looking at is ever integrated on a coarsened grid. Replaces the
    /// previous declaration; survives recompilation.
    ///
    /// Sleeping is deliberately *not* disabled for a sampled island: a
    /// static island reports the DC state its last real solve produced, which
    /// is exactly what an instrument on a circuit that is not moving must
    /// show. Skipping arithmetic is not the same as inventing a number.
    ///
    /// That holds only because sleeping is gated on a bound, not on a
    /// vibe: an island may sleep only once the travel it has LEFT is under
    /// [`Tuning::quiescence_drift`] (1 µV / 1 nA), so the worst a probe on a
    /// sleeping island can read is 1 µV short of the answer, forever. The
    /// criterion that shipped before bounded the travel it had recently
    /// DONE, which for a τ = 100 s tail let a probe sit 10 mV short of the
    /// truth permanently — a solver number, but not the solver's answer.
    pub fn set_sampled(&mut self, ids: &[u32]) {
        self.sampled.clear();
        self.sampled.extend_from_slice(ids);
        self.apply_sampled();
    }

    fn apply_sampled(&mut self) {
        for island in self.islands.iter_mut() {
            island.sampled = false;
        }
        for id in self.sampled.clone() {
            if let Some(&(_, isl, _)) = self.order.iter().find(|(eid, _, _)| *eid == id) {
                self.islands[isl].sampled = true;
            }
        }
    }

    /// Wake every island: the next substep is solved in full, at the room
    /// dt. The escape hatch for any coupling sim-core does not model yet —
    /// when Bergeron corridors land (plan resolution 3), a corridor whose
    /// boundary state moved calls this on the islands it joins.
    pub fn wake_all(&mut self) {
        for island in self.islands.iter_mut() {
            island.wake();
        }
    }

    /// Wake one island by index. Out-of-range indices are ignored.
    pub fn wake_island(&mut self, island: usize) {
        if let Some(i) = self.islands.get_mut(island) {
            i.wake();
        }
    }

    /// Wake the island owning an element. The hook every perturbation that
    /// does not go through `compile()` must use.
    pub fn wake_element(&mut self, id: u32) -> bool {
        let Some(&(_, isl, _)) = self.order.iter().find(|(eid, _, _)| *eid == id) else {
            return false;
        };
        self.islands[isl].wake();
        true
    }

    /// Islands currently asleep.
    pub fn static_islands(&self) -> usize {
        self.islands.iter().filter(|i| i.asleep).count()
    }

    /// Sum of `k` over the islands that are awake and solving, and the count
    /// of them: `(sum_k, islands)`. A cheap way for instrumentation to state
    /// the mean dt dilation without walking the islands itself.
    pub fn local_dt_spread(&self) -> (u64, usize) {
        let mut sum = 0u64;
        let mut live = 0usize;
        for i in self.islands.iter() {
            if i.n > 0 && !i.asleep && !i.quarantined {
                sum += i.k as u64;
                live += 1;
            }
        }
        (sum, live)
    }

    // ------------------------------------------------------ instrumentation
    // Read-only views for the scale benchmark (`sim-golden`, bin `scale`).
    // None of these participate in the solve or in `state_hash`.

    /// Total MNA unknowns across every island. Partitioning does not change
    /// this sum — it changes how many matrices it is spread over, which is
    /// the whole point: cost is superlinear in the size of one matrix.
    pub fn unknowns(&self) -> usize {
        self.islands.iter().map(|i| i.n).sum()
    }

    /// Unknowns in the largest island: the number that actually sets the
    /// per-substep cost.
    pub fn max_island_unknowns(&self) -> usize {
        self.islands.iter().map(|i| i.n).max().unwrap_or(0)
    }

    pub fn island_count(&self) -> usize {
        self.islands.len()
    }

    /// The islands, for instrumentation and for schedulers that want to
    /// step them themselves (see [`Engine::step_plan`]).
    pub fn islands(&self) -> &[Island] {
        &self.islands
    }

    pub fn node_count(&self) -> usize {
        self.islands.iter().map(|i| i.num_nodes).sum()
    }

    pub fn branch_count(&self) -> usize {
        self.islands.iter().map(|i| i.num_branches).sum()
    }

    pub fn element_count(&self) -> usize {
        self.order.len()
    }

    /// False if ANY island is nonlinear. The flag that matters is
    /// [`Island::is_linear`]: refactorization is decided island by island.
    pub fn is_linear(&self) -> bool {
        self.islands.iter().all(|i| i.linear)
    }

    /// Test/instrumentation knob: disable piecewise-linear factorization
    /// reuse and refactor unconditionally, as the solver did before it
    /// existed. Both settings must produce bit-identical results.
    #[doc(hidden)]
    pub fn set_reuse_pwl(&mut self, on: bool) {
        self.reuse_pwl = on;
        for island in self.islands.iter_mut() {
            island.reuse_pwl = on;
            island.factor_valid = false;
        }
    }

    /// Structural nonzeros summed over every island's last stamped matrix.
    pub fn matrix_nnz(&self) -> usize {
        self.islands.iter().map(|i| i.matrix_nnz()).sum()
    }

    /// Numeric factorizations performed since construction, all islands.
    pub fn factorizations(&self) -> u64 {
        self.retired_factorizations + self.islands.iter().map(|i| i.factorizations).sum::<u64>()
    }

    /// Prefix sums of the islands' node counts: `base[i]` is the first
    /// GLOBAL node index island `i` owns, minus one.
    ///
    /// Islands number their nodes from 1 independently, so node 1 exists in
    /// every island. Anything that reasons about the document as a whole —
    /// `crate::validate`'s union-finds, its conflict reports — needs one
    /// namespace, and this is the map into it. It is a bijection onto
    /// `1..=node_count()`, which is all those consumers require.
    fn node_base(&self) -> Vec<usize> {
        let mut base = Vec::with_capacity(self.islands.len());
        let mut acc = 0usize;
        for i in self.islands.iter() {
            base.push(acc);
            acc += i.num_nodes;
        }
        base
    }

    /// Island-local node -> document-global node. Ground (0) stays 0.
    #[inline]
    fn global_node(base: &[usize], island: usize, local: usize) -> usize {
        if local == 0 {
            0
        } else {
            base[island] + local
        }
    }

    /// `(element id, island, GLOBAL node index per pin)` in document order —
    /// lets a caller map unknowns back to the part of the world that owns
    /// them.
    ///
    /// The node indices are globalised (see [`Engine::node_base`]) so a
    /// caller comparing two elements' nodes gets the right answer whether or
    /// not they landed on the same island. The island index is handed over
    /// beside them for callers that want the partition itself.
    pub fn element_nodes(&self) -> Vec<(u32, usize, [usize; MAX_PINS])> {
        let base = self.node_base();
        self.doc()
            .map(|(isl, e)| {
                let mut nd = [0usize; MAX_PINS];
                for (k, v) in nd.iter_mut().enumerate() {
                    *v = Self::global_node(&base, isl, e.node[k]);
                }
                (e.spec.id, isl, nd)
            })
            .collect()
    }

    /// Every ideal zero-impedance constraint the compiled document imposes,
    /// in document order: `(element id, canonical constraint)`.
    ///
    /// This is the compiled truth — real node numbers after wire closure and
    /// ground merging — so `crate::validate` can say WHICH parts conflict and
    /// WHICH ones close a loop, instead of watching the LU return one
    /// anonymous "singular". Broken parts are skipped: they impose nothing.
    ///
    /// Node indices are GLOBALISED. Islands renumber from 1 each, so a 1 V
    /// source on one board and a 5 V source on another would both report
    /// `(1, 0)` and be reported to the player as conflicting supplies. The
    /// constraint is still computed from the island-local nodes the solver
    /// actually stamps — which is what keeps the grouping in `rebuild`
    /// island-local — and only the indices handed out are lifted.
    pub fn ideal_constraints(&self) -> Vec<(u32, Constraint)> {
        let base = self.node_base();
        self.doc()
            .filter(|(_, e)| !e.broken)
            .filter_map(|(isl, e)| {
                constraint_of(&e.spec.kind, &e.node).map(|c| {
                    (
                        e.spec.id,
                        Constraint {
                            a: Self::global_node(&base, isl, c.a),
                            b: Self::global_node(&base, isl, c.b),
                            ..c
                        },
                    )
                })
            })
            .collect()
    }

    pub fn dt(&self) -> f64 {
        self.dt
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    /// True when at least one island has been quarantined. One bad build
    /// does not stop the room: the other islands keep solving, the world
    /// clock keeps advancing, and every number outside that island keeps
    /// being produced by a solver that is still running.
    ///
    /// Which is exactly why this is almost never the question a caller
    /// wants. "Is anything in the room broken" is a status line; "is THIS
    /// part's circuit still being solved" is
    /// [`Engine::element_quarantined`], and that is the one that decides
    /// whether a consequence — damage, a machine step — may fire. Gating a
    /// consequence on this flag makes one diverging build in the corner of
    /// the room switch that consequence off for everybody.
    pub fn is_quarantined(&self) -> bool {
        self.islands.iter().any(|i| i.quarantined)
    }

    /// How many islands are down. The honest player-facing number: "3 of 51
    /// builds have stopped solving" is actionable, "the room is quarantined"
    /// has not been true since islands landed.
    pub fn quarantined_islands(&self) -> usize {
        self.islands.iter().filter(|i| i.quarantined).count()
    }

    /// Is the island this element sits on quarantined? Unknown ids read
    /// false. The per-element form of the question every consumer of
    /// [`Engine::frame`] is really asking; `ElemFrame::quarantined` carries
    /// the same answer for a whole sweep without the id lookup.
    pub fn element_quarantined(&self, id: u32) -> bool {
        self.find(id)
            .map(|(isl, _)| self.islands[isl].quarantined)
            .unwrap_or(false)
    }

    // ---------------------------------------------------------- document

    /// Elements in document order, with the island each belongs to.
    fn doc(&self) -> impl Iterator<Item = (usize, &CompiledElem)> + '_ {
        self.order
            .iter()
            .map(move |&(_, i, s)| (i, &self.islands[i].elems[s]))
    }

    fn find(&self, id: u32) -> Option<(usize, &CompiledElem)> {
        let &(_, i, s) = self.order.iter().find(|(eid, _, _)| *eid == id)?;
        Some((i, &self.islands[i].elems[s]))
    }

    fn find_mut(&mut self, id: u32) -> Option<(usize, &mut CompiledElem)> {
        let &(_, i, s) = self.order.iter().find(|(eid, _, _)| *eid == id)?;
        Some((i, &mut self.islands[i].elems[s]))
    }

    /// Move every element out of its island, back into document order. The
    /// islands themselves are dropped, so their factorization counters are
    /// retired into the engine's running total first.
    fn take_doc(&mut self) -> Vec<CompiledElem> {
        let mut islands = core::mem::take(&mut self.islands);
        // Rescue the bucket chains on the way past. This has to happen HERE
        // and not in `rebuild`: by the time rebuild runs, the line above has
        // already emptied `self.islands`, so a harvest there reads nothing
        // and every edit silently drops every delay line. (Measured: an edit
        // mid-flight pushed a 25.6 ms delay's arrival from 768 substeps to
        // 1285 — a full refill — which is exactly what that looks like.)
        self.orphan_delays
            .extend(islands.iter_mut().flat_map(|i| core::mem::take(&mut i.delays)));
        self.retired_factorizations += islands.iter().map(|i| i.factorizations).sum::<u64>();
        let mut slots: Vec<Vec<Option<CompiledElem>>> = islands
            .iter_mut()
            .map(|i| {
                core::mem::take(&mut i.elems)
                    .into_iter()
                    .map(Some)
                    .collect()
            })
            .collect();
        core::mem::take(&mut self.order)
            .into_iter()
            .filter_map(|(_, i, s)| slots[i][s].take())
            .collect()
    }

    /// Replace the document and recompile. Continuous state (cap voltage,
    /// inductor current) and the broken flag survive for elements whose id
    /// persists: moving a dead resistor does not repair it.
    pub fn set_elements(&mut self, specs: &[ElementSpec]) {
        // Keyed by id, not scanned for it: `set_elements` is called on every
        // edit and on every block the placement gate trials, and the scan this
        // replaces was O(elements²). First writer wins, which is what
        // `Vec::find` did, so a document that repeats an id carries the same
        // state across as before. Ordered map for the same reason `rebuild`
        // uses one: no hasher, no platform RNG, identical on wasm32.
        let old = self.take_doc();
        let old_state: BTreeMap<u32, (ElemState, bool)> = old
            .iter()
            .rev()
            .map(|e| (e.spec.id, (e.state, e.broken)))
            .collect();
        let mut doc = Vec::with_capacity(specs.len());
        for s in specs {
            if s.pins.len() != s.kind.pin_count() {
                continue; // malformed element: drop rather than panic
            }
            let (mut state, broken) = old_state.get(&s.id).copied().unwrap_or_default();
            // A fresh echo chip's HELD OUTPUT starts at its own reference,
            // for the same reason its delay chain does: the whole signal path
            // rests at REF, and an output that starts at 0 V hands the stage
            // after it a 2.5 V step at power-up. `v_prev` is what the output
            // stamps, so seeding the chain alone was not enough — the chain
            // rested correctly and the pin still read zero.
            if matches!(s.kind, ElementKind::Pt2399) && !old_state.contains_key(&s.id) {
                state.v_prev = crate::PT_V_RT;
            }
            doc.push(CompiledElem {
                spec: s.clone(),
                node: [0; MAX_PINS],
                branch: None,
                share_n: 1,
                share_sign: 1.0,
                stamps: true,
                state,
                broken,
            });
        }
        self.rebuild(doc);
    }

    /// Break a part open, or repair it. Returns false when the id is unknown.
    ///
    /// A break/repair is a world EVENT, not a numeric write: it changes the
    /// unknown count (a broken source or switch loses its branch) so it goes
    /// through the full compile path exactly like a switch flip, which also
    /// re-arms the post-event backward-Euler steps and clears `quarantined`.
    /// That is correct for both directions: the circuit really did change, and
    /// a solver that diverged on the old topology deserves a fresh start on
    /// the new one. (Contrast `write_param`, which fires at kHz rates and must
    /// therefore carry those flags across untouched.)
    ///
    /// The part's continuous state is reset both ways: a part that has just
    /// released its magic smoke has no charge or flux left, and a repaired one
    /// is a new part out of the drawer.
    pub fn set_broken(&mut self, id: u32, broken: bool) -> bool {
        let Some((_, e)) = self.find_mut(id) else {
            return false;
        };
        if e.broken == broken {
            return true; // idempotent, and free: no recompile
        }
        e.broken = broken;
        e.state = ElemState::default();
        self.compile();
        true
    }

    /// Has this part failed open? Unknown ids read false.
    pub fn is_broken(&self, id: u32) -> bool {
        self.find(id).map(|(_, e)| e.broken).unwrap_or(false)
    }

    pub fn interact(&mut self, id: u32, op: InteractOp) {
        let Some((_, e)) = self.find_mut(id) else {
            return;
        };
        match (op, &mut e.spec.kind) {
            (InteractOp::SetSwitch { closed }, ElementKind::Switch { closed: c })
            | (InteractOp::SetSwitch { closed }, ElementKind::Button { closed: c }) => *c = closed,
            (InteractOp::SetValue { value }, k) => match k {
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => *ohms = value.max(1e-6),
                ElementKind::Capacitor { farads } => *farads = value.max(1e-15),
                ElementKind::Inductor { henries } => *henries = value.max(1e-12),
                ElementKind::VoltageSource { dc, .. } | ElementKind::Rail { dc, .. } => *dc = value,
                ElementKind::CurrentSource { amps } => *amps = value,
                ElementKind::Potentiometer { wiper, .. } => *wiper = value.clamp(0.01, 0.99),
                // The noise knob is its level, not its seed: dragging it
                // must change how loud the hiss is, never which hiss it is.
                ElementKind::Noise { volts, .. } => *volts = value,
                _ => return,
            },
            _ => return,
        }
        // Switch flips change topology (branch count); value changes only
        // invalidate the factorization. Recompiling handles both and is
        // cheap at current scale.
        self.compile();
    }

    /// Write a live element's parameter from a co-simulated machine, at the
    /// cheapest correct cost (see `ParamWrite`). Returns false when the id
    /// or the parameter/device pairing does not exist.
    ///
    /// This is deliberately NOT `interact()`: machine writes land at kHz
    /// rates, and `interact()`/`compile()` both clear `quarantined` and
    /// re-arm `be_steps`. Clearing quarantine that often would resurrect a
    /// diverged circuit every 640 µs and hide the failure forever; re-arming
    /// BE would silently keep the integrator in first order.
    pub fn write_param(&mut self, id: u32, write: ParamWrite) -> bool {
        let Some((island, e)) = self.find_mut(id) else {
            return false;
        };
        let mut invalidate = false;
        let mut topology = false;
        let mut changed = false;
        match (write, &mut e.spec.kind) {
            (ParamWrite::Bemf { volts }, ElementKind::Motor { bemf, .. }) => {
                // RHS only: `build()` rewrites b[branch] every step.
                if *bemf != volts {
                    *bemf = volts;
                    changed = true;
                }
            }
            (ParamWrite::Wiper { frac }, ElementKind::Potentiometer { wiper, .. }) => {
                let new = frac.clamp(0.01, 0.99);
                if *wiper != new {
                    *wiper = new;
                    invalidate = true;
                    changed = true;
                }
            }
            (ParamWrite::Light { light: new }, ElementKind::Photocell { light, .. }) => {
                // Conductance only. The `!=` guard is what makes a still
                // scene free: an unchanged reading refactors nothing.
                let new = if new.is_finite() { new.clamp(0.0, 1.0) } else { 0.0 };
                if *light != new {
                    *light = new;
                    invalidate = true;
                    // AND WAKE THE ISLAND, exactly as `Wiper` does one arm
                    // up. A DC circuit goes quiet within a second of being
                    // drawn, and refactoring a SLEEPING island changes
                    // nothing a player can see: it stamps no `build()` and
                    // runs no solve, so every reading after the circuit
                    // settled was discarded in silence. A camera could only
                    // ever move a circuit that something else was already
                    // keeping awake.
                    //
                    // This is a perturbation from outside, which is the one
                    // thing `wake()` is for — and it is still not an EDIT:
                    // `wake()` touches neither `quarantined` nor
                    // `be_steps`, so a driven part cannot resurrect a
                    // diverged room 30 times a second. The `!=` guard above
                    // keeps a still scene free, so an unchanged reading
                    // still wakes nothing.
                    changed = true;
                }
            }
            (ParamWrite::Switch { closed }, ElementKind::Switch { closed: c }) => {
                if *c != closed {
                    *c = closed;
                    topology = true;
                    changed = true;
                }
            }
            _ => return false,
        }
        // A machine write that moved a number is a perturbation: the island
        // must be solving again on the very next substep, at full
        // resolution. This is the wake path for co-simulation — `interact()`
        // and `set_broken()` get theirs for free from `compile()`, which
        // rebuilds the partition and so hands back islands that are awake by
        // construction.
        //
        // A write that moved nothing wakes nothing, which is the same
        // promise the rest of this method already makes: a machine mirroring
        // an unchanged back-EMF at 1.5 kHz must cost exactly what silence
        // costs, or a stalled motor could never let its island go still.
        if changed {
            self.islands[island].wake();
        }
        if invalidate {
            // Only the island that owns the part loses its factorization.
            self.islands[island].factor_valid = false;
        }
        if topology {
            // A branch appears/disappears: only the compile path can
            // renumber the unknowns. Carry every island's solver health
            // flags across it untouched.
            let flags: Vec<(u32, bool)> = self
                .islands
                .iter()
                .map(|i| (i.be_steps, i.quarantined))
                .collect();
            self.compile();
            // The partition itself cannot move: an element ties its own
            // nodes into one island whatever its switch is doing, so the
            // island order is a function of the document's geometry alone
            // and the flags land back where they came from. If that ever
            // stops holding, they stay at their post-compile defaults
            // rather than being attached to the wrong island.
            if flags.len() == self.islands.len() {
                for (island, (be, q)) in self.islands.iter_mut().zip(flags) {
                    island.be_steps = be;
                    island.quarantined = q;
                }
            }
        }
        true
    }

    /// Recompile the current document (an in-place edit changed a value, a
    /// switch or a broken flag).
    fn compile(&mut self) {
        let doc = self.take_doc();
        self.rebuild(doc);
    }

    /// Wire closure, node numbering, island partition and per-island unknown
    /// layout. Everything downstream of this is per island.
    fn rebuild(&mut self, mut doc: Vec<CompiledElem>) {
        // Bucket chains are keyed by id and must OUTLIVE a recompile: wiring
        // a resistor in somewhere else must not empty a delay line that is
        // mid-echo. `take_doc` stashed them here on its way past, because it
        // is the thing that dismantles the islands; whatever is left over
        // after this belonged to parts that are gone, and is dropped.
        let mut old_delays = core::mem::take(&mut self.orphan_delays);
        // 1. Junctions: unique endpoints, interned in first-seen order.
        //
        //    The index a point gets is exactly the one a linear scan would
        //    give it — first seen, first numbered — so the compiled netlist is
        //    byte-for-byte what it was before this was an ordered map. The map
        //    only decides how fast the lookup finds it. `BTreeMap`, not a hash
        //    map: no hasher seed, no platform RNG, nothing that could differ
        //    between native and wasm32.
        //
        //    It matters because `rebuild` is not a rare event. Every edit,
        //    every switch flip, every knob turn, every machine move and every
        //    trial the placement gate runs recompiles the document, and the
        //    scan this replaces was O(points²): 69 us on a 400-element room,
        //    against 25 us here (measured, release, this tree).
        let mut points: Vec<Point> = Vec::new();
        let mut index_of: BTreeMap<Point, usize> = BTreeMap::new();
        let mut ends: Vec<Vec<usize>> = Vec::with_capacity(doc.len());
        for e in &doc {
            ends.push(
                e.spec
                    .pins
                    .iter()
                    .map(|p| {
                        *index_of.entry(*p).or_insert_with(|| {
                            points.push(*p);
                            points.len() - 1
                        })
                    })
                    .collect(),
            );
        }

        // 1b. LOOSE ENDS COST AN UNKNOWN, so give them away.
        //
        //     A resistor with one end connected to nothing carries no current
        //     — KCL at a point nothing else touches leaves no other option —
        //     and a pure conductance with no current through it has NO
        //     VOLTAGE ACROSS IT. Its free end is therefore at exactly the far
        //     end's potential, always, and giving it a row of its own asks
        //     the solver to rediscover that on every substep forever.
        //
        //     Measured before this: ten dangling resistors took a room from
        //     4.3 ms to 16.7 ms per 50k steps and its node count from 2 to
        //     12, while ten resistors connected at BOTH ends — same stamping,
        //     no new nodes — cost less than half as much. Newton iterations
        //     and factorization counts were identical across all three, so
        //     it was never solve work: it was matrix size, on every step, for
        //     the life of the room.
        //
        //     ONLY PURE CONDUCTANCES, and that restriction is the whole
        //     safety argument. Zero current means zero drop for a resistor,
        //     and does NOT for anything else: a capacitor holds its charge,
        //     an inductor its history, and an ideal SOURCE with a loose end
        //     would become `0 = dc` — a singular row — the moment its two
        //     terminals were made one. Those keep their unknown.
        let mut degree: Vec<u32> = vec![0; points.len()];
        for (e, je) in doc.iter().zip(ends.iter()) {
            if e.broken {
                continue;
            }
            for &j in je {
                degree[j] += 1;
            }
        }
        // NOTE for anyone tempted to special-case labels here: do not. A
        // label is an ELEMENT, so its pin is already counted in the loop
        // above like every other pin, and a point carrying a resistor and a
        // label has degree 2 — not a loose end. When labels were room state
        // instead of parts, this needed an explicit correction, and without
        // it the loose-end merge folded every named net into whatever its
        // resistor's far end was (ground, in the case that found it). Being
        // a part deletes the special case rather than fixing it.

        // 2. Union-find: wires merge their endpoints; grounds pin to a
        //    virtual ground root.
        let ground_root = points.len();
        let mut parent: Vec<usize> = (0..=points.len()).collect();
        // LABELS MERGE BY NAME, exactly where a wire merges by geometry and
        // in the same pass. Every `Label` sharing a name becomes one node,
        // however far apart its copies are drawn.
        //
        // Read straight off the DOCUMENT, because a label is a part. There
        // is no list to push in, no second source of truth, and nothing the
        // gate and the engine can disagree about — they are handed the same
        // `specs` and reach the same conclusion.
        //
        // Names compare trimmed and case-insensitively: "+5V" and "+5v" are
        // the same net to everyone except a string comparison. A BLANK name
        // joins nothing, because an unnamed label is one somebody has not
        // finished typing, not an instruction to short the room together.
        {
            // First anchor seen for each name; everything later joins it.
            // Owned keys, because `rebuild` runs on every edit and anything
            // borrowed-and-leaked here would leak once per keystroke.
            let mut first_of: BTreeMap<String, usize> = BTreeMap::new();
            for (e, je) in doc.iter().zip(ends.iter()) {
                if !matches!(e.spec.kind, ElementKind::Label) || e.broken {
                    continue;
                }
                let key = e.spec.name.trim().to_lowercase();
                if key.is_empty() {
                    continue;
                }
                match first_of.get(&key) {
                    Some(&j) => {
                        let (ra, rb) = (find(&mut parent, je[0]), find(&mut parent, j));
                        if ra != rb {
                            parent[ra] = rb;
                        }
                    }
                    None => {
                        first_of.insert(key, je[0]);
                    }
                }
            }
        }
        for (e, je) in doc.iter().zip(ends.iter()) {
            // A broken part is an OPEN circuit — it must not merge nodes any
            // more than it stamps. `damage::rating` returns None for Wire and
            // Ground today so nothing here can break yet; the guard goes in
            // now, while it is still free, rather than the day wires become
            // breakable and a "broken" wire silently keeps shorting.
            if e.broken {
                continue;
            }
            // A loose end of a pure conductance joins the node it hangs
            // off: same potential, one unknown instead of two.
            if matches!(
                e.spec.kind,
                ElementKind::Resistor { .. }
                    | ElementKind::Lamp { .. }
                    | ElementKind::Speaker { .. }
                    | ElementKind::Photocell { .. }
            ) && je.len() == 2
            {
                for (a, b) in [(0usize, 1usize), (1, 0)] {
                    if degree[je[a]] == 1 {
                        let (ra, rb) = (find(&mut parent, je[a]), find(&mut parent, je[b]));
                        if ra != rb {
                            parent[ra] = rb;
                        }
                    }
                }
            }
            match e.spec.kind {
                ElementKind::Wire => {
                    let (ra, rb) = (find(&mut parent, je[0]), find(&mut parent, je[1]));
                    parent[ra] = rb;
                }
                ElementKind::Ground => {
                    let (ra, rg) = (find(&mut parent, je[0]), find(&mut parent, ground_root));
                    parent[ra] = rg;
                }
                _ => {}
            }
        }

        // 3. Number electrical nodes: ground set -> 0, others 1..=N.
        //    Same first-seen numbering as the scan it replaces, so node
        //    indices — and therefore the matrix, and therefore every hash —
        //    are unchanged. Roots are junction indices, so a flat vector
        //    indexed by root does the lookup in O(1) with no map at all.
        let groot = find(&mut parent, ground_root);
        let mut node_of_root: Vec<usize> = vec![usize::MAX; points.len() + 1];
        node_of_root[groot] = 0;
        let mut num_nodes = 0usize;
        let node_of_junction: Vec<usize> = (0..points.len())
            .map(|j| {
                let r = find(&mut parent, j);
                if node_of_root[r] == usize::MAX {
                    num_nodes += 1;
                    node_of_root[r] = num_nodes;
                }
                node_of_root[r]
            })
            .collect();

        // 4. Global node per pin, and the island partition: an element ties
        //    all of ITS non-ground nodes into one component. Ground is not a
        //    coupling — it is the reference every island shares.
        //
        //    A broken part still ties its nodes together even though it
        //    stamps nothing. That is conservative (it can only make an island
        //    larger than physics requires) and it keeps every pin of a part in
        //    one island, which is what `pin_voltage` on a dead part needs.
        let mut np: Vec<usize> = (0..=num_nodes).collect();
        for (e, je) in doc.iter_mut().zip(ends.iter()) {
            e.node = [0; MAX_PINS];
            let mut first = 0usize;
            for (k, j) in je.iter().enumerate() {
                let nd = node_of_junction[*j];
                e.node[k] = nd; // global for now; localized in step 6
                if nd == 0 {
                    continue;
                }
                if first == 0 {
                    first = nd;
                } else {
                    let (ra, rb) = (find(&mut np, first), find(&mut np, nd));
                    np[ra] = rb;
                }
            }
        }

        // 5. Island ids and island-local node numbers, both in ascending
        //    global-node order — so island membership and the layout inside
        //    an island are a deterministic function of the document alone.
        //
        //    A one-island document therefore gets exactly the numbering the
        //    unpartitioned engine gave it: `local_of_node[nd] == nd`. That is
        //    what makes partitioning bit-exact on every golden circuit.
        let mut island_of_root: Vec<usize> = vec![usize::MAX; num_nodes + 1];
        let mut island_of_node: Vec<usize> = vec![usize::MAX; num_nodes + 1];
        let mut local_of_node: Vec<usize> = vec![0; num_nodes + 1];
        let mut island_nodes: Vec<usize> = Vec::new();
        for nd in 1..=num_nodes {
            let r = find(&mut np, nd);
            if island_of_root[r] == usize::MAX {
                island_of_root[r] = island_nodes.len();
                island_nodes.push(0);
            }
            let isl = island_of_root[r];
            island_of_node[nd] = isl;
            island_nodes[isl] += 1;
            local_of_node[nd] = island_nodes[isl];
        }

        // 6. Assign every element to an island, localize its nodes, and split
        //    the document into the parts that solve and the parts that do not.
        //    Wires and grounds stamp nothing, accept nothing and converge
        //    nothing; a broken part is an open circuit; a part with every pin
        //    on ground has no unknown to influence. All three are parked.
        let mut ground_island = usize::MAX;
        let mut island_count = island_nodes.len();
        let mut owner: Vec<usize> = Vec::with_capacity(doc.len());
        let mut solves: Vec<bool> = Vec::with_capacity(doc.len());
        for e in doc.iter_mut() {
            let mut isl = usize::MAX;
            for k in 0..e.spec.pins.len() {
                let nd = e.node[k];
                if nd != 0 && isl == usize::MAX {
                    isl = island_of_node[nd];
                }
                e.node[k] = if nd == 0 { 0 } else { local_of_node[nd] };
            }
            let wiring = matches!(e.spec.kind, ElementKind::Wire | ElementKind::Ground);
            let active = !e.broken && !wiring && isl != usize::MAX;
            if isl == usize::MAX {
                // Nothing but ground: park it in the (zero-unknown) island
                // that collects the document's pure wiring.
                if ground_island == usize::MAX {
                    ground_island = island_count;
                    island_count += 1;
                }
                isl = ground_island;
            }
            if !active {
                // No solve will ever write to it, so it must not keep
                // reporting what the last one did.
                e.state.v_prev = 0.0;
                e.state.i_prev = 0.0;
                e.state.pin_i = [0.0; MAX_PINS];
            }
            owner.push(isl);
            solves.push(active);
        }

        // 7. Bucket the document per island, solving elements first (so the
        //    hot loops are a prefix), each group in document order.
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); island_count];
        for (di, isl) in owner.iter().enumerate() {
            if solves[di] {
                buckets[*isl].push(di);
            }
        }
        let actives: Vec<usize> = buckets.iter().map(|b| b.len()).collect();
        for (di, isl) in owner.iter().enumerate() {
            if !solves[di] {
                buckets[*isl].push(di);
            }
        }

        // 8. Branch unknowns, numbered PER ISLAND in document order — and
        //    ideal-constraint merging, grouped per island for the same
        //    reason.
        //
        //    Ideal, zero-impedance constraints (sources, rails, closed
        //    switches — see `crate::constraint`) that reduce to the SAME
        //    canonical constraint share ONE branch unknown. Two 5 V supplies
        //    on one node are one net, not two duplicate rows; so are a 5 V
        //    supply and a 5 V rail, and so are two closed switches in
        //    parallel. Without this they are a singular matrix and the whole
        //    document is unplaceable — which is why two-way lighting used to
        //    be refused.
        //
        //    The grouping runs INSIDE the per-island loop, and that is not a
        //    tidiness choice. A constraint's key is built from node indices,
        //    and islands renumber from 1 each: two unrelated 5 V supplies on
        //    two unrelated boards would produce the same key and get merged
        //    onto one branch row spanning two matrices. That is a corrupt
        //    system, not a slow one, and grouping per island makes it
        //    structurally impossible rather than defended against. With one
        //    island this is bit-identical to the unpartitioned engine,
        //    because the bucket IS the document.
        //
        //    Grouping is by exact equality of an integer key, found with a
        //    linear scan in DOCUMENT ORDER. No hashing and no float
        //    comparison anywhere: the group a member joins, the member that
        //    leads it, and the branch index every group gets depend only on
        //    the document, never on iteration order. Same on native and on
        //    wasm32, byte for byte.
        //
        //    Motors, op-amps and 555s never participate (they are not ideal
        //    constraints), so this cannot merge anything that was well-posed
        //    before: every document it changes is one the placement gate
        //    rejected as `Unsolvable`.
        let mut branches = vec![0usize; island_count];
        let mut branch_of: Vec<Option<usize>> = vec![None; doc.len()];
        let mut share_n: Vec<u32> = vec![1; doc.len()];
        let mut share_sign: Vec<f64> = vec![1.0; doc.len()];
        let mut stamps: Vec<bool> = vec![true; doc.len()];
        for (isl, bucket) in buckets.iter().enumerate() {
            // (canonical key, branch index, leader's drawn orientation)
            let mut groups: Vec<(ConstraintKey, usize, bool)> = Vec::new();
            // (document index, group slot) for every member that joined one.
            let mut group_of: Vec<(usize, usize)> = Vec::new();
            for &di in bucket[..actives[isl]].iter() {
                if !doc[di].spec.kind.is_branch() {
                    continue;
                }
                let Some(c) = constraint_of(&doc[di].spec.kind, &doc[di].node) else {
                    // A branch device that is not an ideal constraint (motor,
                    // op-amp, 555): always its own unknown.
                    branch_of[di] = Some(branches[isl]);
                    branches[isl] += 1;
                    continue;
                };
                let key = c.key();
                match groups.iter().position(|(k, _, _)| *k == key) {
                    Some(g) => {
                        // Same net as an earlier element: alias onto its row
                        // and stay silent. Reading the shared current needs
                        // the sign of this member's drawn orientation
                        // relative to the leader's, because the leader's row
                        // defines which way the branch current is positive.
                        let (_, bi, leader_flipped) = groups[g];
                        branch_of[di] = Some(bi);
                        stamps[di] = false;
                        share_sign[di] = if c.flipped == leader_flipped { 1.0 } else { -1.0 };
                        group_of.push((di, g));
                    }
                    None => {
                        branch_of[di] = Some(branches[isl]);
                        groups.push((key, branches[isl], c.flipped));
                        group_of.push((di, groups.len() - 1));
                        branches[isl] += 1;
                    }
                }
            }
            // How many members each group ended up with, for the current split.
            if !groups.is_empty() {
                let mut counts = vec![0u32; groups.len()];
                for (_, g) in group_of.iter() {
                    counts[*g] += 1;
                }
                for (di, g) in group_of.iter() {
                    share_n[*di] = counts[*g];
                }
            }
        }

        // 9. Move the elements into their islands and record document order.
        let mut cells: Vec<Option<CompiledElem>> = doc.into_iter().map(Some).collect();
        let mut order = vec![(0u32, 0usize, 0usize); cells.len()];
        let mut islands: Vec<Island> = Vec::with_capacity(island_count);
        for (isl, bucket) in buckets.into_iter().enumerate() {
            let mut elems: Vec<CompiledElem> = Vec::with_capacity(bucket.len());
            for (slot, di) in bucket.into_iter().enumerate() {
                let mut e = cells[di].take().expect("each element lands in one island");
                e.branch = branch_of[di];
                e.share_n = share_n[di];
                e.share_sign = share_sign[di];
                e.stamps = stamps[di];
                order[di] = (e.spec.id, isl, slot);
                elems.push(e);
            }
            let active = actives[isl];
            let num_nodes = if isl < island_nodes.len() {
                island_nodes[isl]
            } else {
                0
            };
            let num_branches = branches[isl];
            let n = num_nodes + num_branches;
            // A broken nonlinear device stamps nothing, so it cannot make the
            // system nonlinear: the last dead LED in a room hands the solver
            // its single-pass linear path back. (Broken parts are parked
            // outside the active prefix, so they are already excluded.)
            let linear = !elems[..active].iter().any(|e| e.spec.kind.is_nonlinear());
            // ...and the last dead diode also hands back cross-substep
            // factorization reuse to an island that still has op-amps and
            // 555s in it, whose matrix only moves when a region or a latch
            // flips. Per island, so a diode next door never disarms it.
            let smooth_nonlinear = elems[..active].iter().any(|e| e.spec.kind.needs_newton());
            // The two structural properties the levers key off. Both are
            // functions of the netlist alone, so they cost one pass per
            // edit and nothing per substep.
            //
            // `ac_hz`: a source whose value depends on `t`. `hz == 0` is a
            // constant `dc + amp·sin(phase)`, which is DC.
            // `pinned`: a device somebody samples faster than the tick — a
            // speaker feeds an audio worklet, a motor is co-simulated by the
            // machine model at its own fixed rate, a noise source is
            // specified as one fresh sample per substep. None may be handed
            // a coarsened integration grid.
            let mut ac_hz = 0.0f64;
            let mut pinned = false;
            let mut has_noise = false;
            for e in elems[..active].iter() {
                match e.spec.kind {
                    ElementKind::VoltageSource { amp, hz, .. }
                    | ElementKind::Rail { amp, hz, .. } => {
                        if amp != 0.0 && hz != 0.0 {
                            ac_hz = ac_hz.max(hz.abs());
                        }
                    }
                    ElementKind::Speaker { .. } | ElementKind::Motor { .. } => pinned = true,
                    // A noise source carries DISCRETE state — its position in
                    // its own PRNG stream — that only a solve advances, and
                    // `state_hash` watches it. Sleeping would stop the hiss
                    // and freeze a counter two engines could then disagree
                    // about; `k > 1` would decimate a stream specified as one
                    // sample per substep (at k = 8 a 25 kHz generator becomes
                    // a 3 kHz zero-order hold, which is a player-audible
                    // number no longer coming from the model it was written
                    // as). So: pinned, and never sleepable.
                    ElementKind::Noise { .. } => {
                        pinned = true;
                        has_noise = true;
                    }
                    _ => {}
                }
            }
            let tuning = self.tuning;
            let reuse_pwl = self.reuse_pwl;
            // Only the ACTIVE prefix can stamp, so only it can produce an
            // edge; a parked or broken source drives nothing.
            let edge_sources: Vec<usize> = (0..active)
                .filter(|&i| match elems[i].spec.kind {
                    ElementKind::VoltageSource { amp, hz, wave, .. }
                    | ElementKind::Rail { amp, hz, wave, .. } => {
                        amp != 0.0 && hz != 0.0 && wave.has_edges()
                    }
                    _ => false,
                })
                .collect();
            // Re-home this island's chains. A part whose `stages` CHANGED
            // gets a fresh chain rather than a reinterpreted one: the old
            // samples were taken at a different tap position and replaying
            // them at a new length would be audible nonsense.
            let delays: BTreeMap<u32, DelayLine> = elems
                .iter()
                .filter_map(|e| {
                    // Both delay parts draw from the same chain. Only where
                    // the clock comes from differs: the BBD takes it from a
                    // pin, the PT2399 makes its own — and the PT's depth is
                    // fixed, because a real chip's RAM is.
                    let (stages, rest) = match e.spec.kind {
                        ElementKind::Bbd { stages } => (stages, 0.0),
                        ElementKind::Pt2399 => (crate::PT_STAGES, crate::PT_V_RT),
                        _ => return None,
                    };
                    {
                        let want = (stages as usize).max(2);
                        let dl = old_delays
                            .remove(&e.spec.id)
                            .filter(|d| d.buf.len() == want)
                            .unwrap_or_else(|| DelayLine::new(want, rest));
                        Some((e.spec.id, dl))
                    }
                })
                .collect();
            islands.push(Island {
                elems,
                delays,
                active,
                num_nodes,
                num_branches,
                n,
                a: vec![0.0; n * n],
                b: vec![0.0; n],
                x: vec![0.0; n],
                lu: DenseLu::new(n),
                saved: vec![Vec::new(); RESCUE_DEPTH as usize + 1],
                linear,
                smooth_nonlinear,
                factor_valid: false,
                factored_h: 0.0,
                factored_be: false,
                reuse_pwl,
                edge_sources,
                be_steps: BE_STEPS_AFTER_EVENT,
                quarantined: false,
                factorizations: 0,
                tuning,
                x_prev: vec![0.0; n],
                x_prev2: vec![0.0; n],
                x_mark: vec![0.0; n],
                win_prev: vec![0.0; n],
                have_win: false,
                hist: 0,
                quiet_t: 0.0,
                asleep: false,
                sleepable: ac_hz == 0.0 && !has_noise,
                ac_hz,
                pinned,
                sampled: false,
                k: 1,
                good: 0,
                pending: 0,
                discrete_moved: false,
            });
        }

        self.junctions = points
            .iter()
            .zip(node_of_junction.iter())
            .map(|(p, nd)| {
                if *nd == 0 {
                    (*p, 0, 0)
                } else {
                    (*p, island_of_node[*nd], local_of_node[*nd])
                }
            })
            .collect();
        self.order = order;
        self.islands = islands;
        // The partition just changed, so which island owns a sampled element
        // may have too. Every island is born awake at k = 1, which is also
        // the correct answer to "the document was edited".
        self.apply_sampled();
    }

    /// Stamp and LU-factor the system for the current document WITHOUT
    /// advancing any state: true when the MNA matrix is nonsingular, i.e.
    /// the next step could at least factor. This is the structural half of
    /// placement validation (`crate::validate`) — a `false` here is exactly
    /// the condition that would quarantine the engine one step later.
    ///
    /// Backward Euler on purpose: it is what the post-edit event steps run,
    /// and structural singularity (dependent source rows, zero rows) is
    /// independent of `h` and the integration mode anyway. Device state and
    /// the solution vector are untouched; the trial factorization is
    /// discarded so a subsequent step re-stamps from scratch.
    pub fn probe_solvable(&mut self) -> bool {
        let (t, dt) = (self.time, self.dt);
        let mut ok = true;
        for island in self.islands.iter_mut() {
            if island.n == 0 {
                continue; // an empty island is trivially solvable
            }
            // Clear BEFORE as well as after. `build()` skips stamping AND
            // factoring when a retained factorization is still valid, so
            // probing with one armed would answer `true` without testing
            // anything — the probe must always do the work it claims to cost.
            island.factor_valid = false;
            ok &= island.build(t + dt, dt, true).is_ok();
            island.factor_valid = false;
        }
        ok
    }

    /// Advance up to `max_steps` fixed-dt substeps. The caller owns the
    /// wall-clock budget (Falstad's rule: heavy circuits slow sim time,
    /// never the UI).
    ///
    /// Islands are stepped one after another here. They are independent, so
    /// this is exactly the serial spelling of [`Engine::step_plan`] — see
    /// that method for the parallel one.
    pub fn advance(&mut self, max_steps: u32) -> AdvanceReport {
        let (t0, dt) = (self.time, self.dt);
        let mut report = AdvanceReport::default();
        for island in self.islands.iter_mut() {
            report.merge(island.advance(t0, dt, max_steps));
        }
        self.commit_advance(report.steps);
        report
    }

    /// The islands and the clock to step them against: `(world time, dt,
    /// islands)`. sim-core spawns no threads and reads no clock, so a caller
    /// that wants per-island parallelism (rayon lives in `crates/server`)
    /// takes this, calls [`Island::advance`] on each element of the slice in
    /// any order or on any thread, and then calls [`Engine::commit_advance`]
    /// with the largest `steps` any island reported.
    ///
    /// Bit-identical to `advance()`: island state is disjoint memory, so
    /// every flop has the same operands in the same order either way.
    pub fn step_plan(&mut self) -> (f64, f64, &mut [Island]) {
        (self.time, self.dt, &mut self.islands)
    }

    /// Move the world clock on by `steps` substeps, after stepping the
    /// islands externally. `advance()` does this for you.
    pub fn commit_advance(&mut self, steps: u32) {
        for _ in 0..steps {
            self.time += self.dt;
        }
    }
}

/// `dc + amp · wave(2π·hz·t + phase)`.
///
/// SINE IS EVALUATED EXACTLY AS IT ALWAYS WAS. Re-deriving it from the
/// normalized phase would round differently in the last place and move every
/// golden digest for a refactor, so the original expression is kept verbatim
/// and only the other three shapes go through `eval_unit`.
///
/// The normalized phase is `hz·t + phase/2π`, taken to its fractional part.
/// `libm::floor` is exact — no approximation enters — and every operation
/// here is plain f64, so this is bit-identical on native and wasm32 like the
/// rest of the kernel. Nothing is reduced modulo 2π: the division by the
/// constant happens once, on the phase alone.
#[inline]
fn source_value(dc: f64, amp: f64, hz: f64, phase: f64, wave: Wave, t: f64) -> f64 {
    if amp == 0.0 {
        return dc;
    }
    match wave {
        Wave::Sine => dc + amp * libm::sin(TWO_PI * hz * t + phase),
        w => {
            let x = hz * t + phase * INV_TWO_PI;
            let u = x - libm::floor(x);
            dc + amp * w.eval_unit(u)
        }
    }
}

/// Does the source's shape JUMP somewhere in `(t0, t1]`?
///
/// Both answers are one integer comparison on the phase counter, which is
/// what makes them exact at any step size — including a step that straddles
/// several edges at once (a source far above the sample rate, where the
/// answer is "yes" regardless).
///
/// With `x = hz·t + phase/2π` the normalized phase is `frac(x)`, so:
///   * SQUARE jumps at `u = 0` and `u = 1/2`, i.e. whenever `floor(2x)` moves;
///   * SAW jumps at `u = 1/2` only, i.e. whenever `floor(x − 1/2)` moves.
#[inline]
fn crosses_edge(hz: f64, phase: f64, wave: Wave, t0: f64, t1: f64) -> bool {
    if !wave.has_edges() || hz == 0.0 {
        return false;
    }
    let x = |t: f64| hz * t + phase * INV_TWO_PI;
    match wave {
        Wave::Square => libm::floor(2.0 * x(t0)) != libm::floor(2.0 * x(t1)),
        _ => libm::floor(x(t0) - 0.5) != libm::floor(x(t1) - 0.5),
    }
}

impl Island {
    /// Does any source jump inside `(t0, t1]`?
    ///
    /// The `is_empty()` short-circuit is the point: rooms without a square or
    /// sawtooth source — which is nearly all of them — never look at a single
    /// element.
    #[inline]
    fn step_has_edge(&self, t0: f64, t1: f64) -> bool {
        if self.edge_sources.is_empty() {
            return false;
        }
        self.edge_sources.iter().any(|&i| {
            match self.elems[i].spec.kind {
                ElementKind::VoltageSource { hz, phase, wave, .. }
                | ElementKind::Rail { hz, phase, wave, .. } => {
                    crosses_edge(hz, phase, wave, t0, t1)
                }
                _ => false,
            }
        })
    }

    // ------------------------------------------------------ instrumentation

    /// MNA unknowns in this island: `nodes + branches`.
    pub fn unknowns(&self) -> usize {
        self.n
    }

    pub fn node_count(&self) -> usize {
        self.num_nodes
    }

    pub fn branch_count(&self) -> usize {
        self.num_branches
    }

    /// Elements in this island, including the parked ones (wires, grounds,
    /// broken parts) that never reach the solve.
    pub fn element_count(&self) -> usize {
        self.elems.len()
    }

    /// Elements this island actually visits every substep.
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// False if any element IN THIS ISLAND is nonlinear. A diode next door
    /// costs this island nothing.
    pub fn is_linear(&self) -> bool {
        self.linear
    }

    pub fn is_quarantined(&self) -> bool {
        self.quarantined
    }

    /// The last stamped MNA matrix, row-major `n x n`.
    pub fn matrix(&self) -> &[f64] {
        &self.a
    }

    /// Structural nonzeros in the last stamped matrix.
    pub fn matrix_nnz(&self) -> usize {
        self.a.iter().filter(|v| **v != 0.0).count()
    }

    /// The last solved unknown vector.
    pub fn solution(&self) -> &[f64] {
        &self.x
    }

    /// Numeric factorizations performed since construction.
    pub fn factorizations(&self) -> u64 {
        self.factorizations
    }

    /// True when THIS island's stamped matrix is constant between discrete
    /// events, so an LU factorization can outlive the substep that produced
    /// it: either the island is linear, or its only nonlinearities are
    /// piecewise-linear (op-amp rail regions, 555 latches). One diode in
    /// this island disarms it — and, since islands landed, only in this one.
    ///
    /// This does not weaken anything: on a reuse hit the solver runs against
    /// an L/U that a refactor would have recomputed to the same bits.
    #[inline]
    fn reusable(&self) -> bool {
        self.linear || (self.reuse_pwl && !self.smooth_nonlinear)
    }

    // ----------------------------------------- quiescence and local dt

    /// Is this island asleep? A sleeping island does no arithmetic; the
    /// values it reports are the ones its last real solve produced.
    pub fn is_static(&self) -> bool {
        self.asleep
    }

    /// This island's local dt multiple: it integrates at `k * dt`.
    pub fn local_dt_k(&self) -> u32 {
        self.k
    }

    /// Whether an instrument or a co-simulated machine samples this island
    /// faster than the tick, pinning it to `k = 1`.
    pub fn is_pinned(&self) -> bool {
        self.pinned || self.sampled
    }

    /// Can this island ever go static? False when it holds a time-varying
    /// source, whose equations depend on `t`, or a noise source, whose
    /// stream only a solve advances.
    pub fn is_sleepable(&self) -> bool {
        self.sleepable
    }

    /// Return to "just perturbed": awake, at the room dt, with no history to
    /// draw conclusions from. Everything that can change this island's
    /// trajectory from outside routes through here.
    ///
    /// Deliberately does NOT clear `factor_valid`: a sleeping island ran no
    /// `build()`, so its retained matrix and LU are still exactly what its
    /// last real solve stamped, and nothing about the circuit changed while
    /// it slept. If it slept at `k > 1` the reset to `k = 1` moves `h`, and
    /// `build`'s `factored_h == h` test refactors on its own.
    pub fn wake(&mut self) {
        self.asleep = false;
        self.quiet_t = 0.0;
        self.have_win = false;
        self.k = 1;
        self.good = 0;
        self.hist = 0;
        self.x_mark.copy_from_slice(&self.x);
    }

    /// Back to the room dt, with the error history invalidated: the next
    /// two steps re-establish an evenly spaced triple before the controller
    /// is allowed another opinion.
    fn drop_to_room_dt(&mut self) {
        self.good = 0;
        if self.k > 1 {
            self.k = 1;
            self.hist = 1;
        }
    }

    /// The largest `k` this island may use at room step `dt`. Powers of two
    /// only, so every island's step boundary is also a world substep
    /// boundary and no island can ever land between two of them.
    fn k_cap(&self, dt: f64) -> u32 {
        if !self.tuning.local_dt || self.pinned || self.sampled || dt <= 0.0 {
            return 1;
        }
        let mut cap = self.tuning.local_dt_max_k.max(1);
        // Absolute ceiling on h: bounds how stale an island may be.
        let by_time = (self.tuning.local_dt_max / dt) as u32;
        cap = cap.min(by_time.max(1));
        // Nyquist-with-margin for a time-varying source.
        if self.ac_hz > 0.0 {
            let by_hz = (1.0 / (self.ac_hz * self.tuning.local_dt_min_samples * dt)) as u32;
            cap = cap.min(by_hz.max(1));
        }
        // Round down to a power of two.
        1u32 << (31 - cap.max(1).leading_zeros())
    }

    /// Has every unknown finished travelling, to within the residual
    /// [`Tuning::quiescence_drift`] allows?
    ///
    /// This is the question sleeping actually turns on, and it is NOT the
    /// question the slew and drift tests answer. Those bound the travel an
    /// island has recently done; freezing it makes the travel it has LEFT
    /// permanent, and for a long tail those two numbers differ by `τ /
    /// quiescence_hold` — four orders of magnitude for a 100 s time
    /// constant.
    ///
    /// Called only at the end of a completed quiet window, with `x_mark` the
    /// state one whole window ago and `win_prev` the travel of the window
    /// before that. For an unknown relaxing towards a DC point the
    /// per-window travel is geometric — `m1 = ρ·m0` — so everything still
    /// to come sums to `m1·ρ/(1-ρ)`, which in terms of what we measured is
    /// `m1² / (m0 - m1)`. Nothing is fitted and nothing is assumed about the
    /// circuit: the island's own two windows say how fast it is converging
    /// and therefore how far it has left to go.
    fn window_settled(&self) -> bool {
        let t = self.tuning;
        if t.quiescence_decay >= 1.0 {
            return true; // test disabled (the pre-fix criterion)
        }
        for i in 0..self.n {
            let (x, mark) = (self.x[i], self.x_mark[i]);
            let m1 = (x - mark).abs();
            let scale = x.abs().max(mark.abs());
            // Below a few ulps the trajectory has stopped moving in f64 at
            // all: a BE/TR update whose increment is under half an ulp
            // returns the same bits forever, so continuing to solve would
            // hold this number exactly as firmly as sleeping does.
            if m1 <= 4.0 * f64::EPSILON * scale {
                continue;
            }
            if !self.have_win {
                return false;
            }
            let m0 = self.win_prev[i];
            let d = m0 - m1;
            // `m0` and `m1` are differences of numbers of size `scale`, so
            // they each carry ~eps·scale of rounding and their difference
            // carries a few of those. Believing a `d` at that level would be
            // reading an extrapolation off noise, so an unknown whose decay
            // is not resolvable simply does not qualify — it keeps solving.
            let noise = 64.0 * f64::EPSILON * (scale + m0);
            if m1 > t.quiescence_decay * m0 || d <= noise {
                return false;
            }
            // Travel remaining, `m1²/d`, against the residual budget for
            // this unknown's dimension. Multiplied out: no division, and no
            // way to trip over a zero `d` that the guard above already
            // excluded.
            let tol = if i < self.num_nodes {
                t.quiescence_drift
            } else {
                t.quiescence_drift_i
            };
            if m1 * m1 > tol * d {
                return false;
            }
        }
        true
    }

    /// Post-step bookkeeping for both levers: one pass over `x` produces the
    /// motion, the window drift and the integration-error estimate — each
    /// split by the KIND of unknown, because the node unknowns are volts and
    /// the branch unknowns are amps — and the decisions fall out of them.
    ///
    /// `h` is the step just taken, `dt` the room substep the caller is
    /// counting in. Both matter: the quiescence tests are rates over the
    /// island's own step, while the staleness governor is a bound on how far
    /// the island may be from WORLD time, which is measured in `dt`.
    fn after_step(&mut self, h: f64, dt: f64, nr: u32) {
        // |x - x_prev|, motion this step; |x - x_mark|, motion across the
        // quiet window; |x - 2x_prev + x_prev2| ~= h² y'', the LTE. Volts
        // (node unknowns) and amps (branch unknowns) never mix.
        let (mut dv, mut mv, mut cv) = (0.0f64, 0.0f64, 0.0f64);
        let (mut di, mut mi, mut ci) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..self.n {
            let (x, p1, p2, m) = (self.x[i], self.x_prev[i], self.x_prev2[i], self.x_mark[i]);
            let (d, mm, c) = ((x - p1).abs(), (x - m).abs(), (x - p1 - p1 + p2).abs());
            if i < self.num_nodes {
                dv = dv.max(d);
                mv = mv.max(mm);
                cv = cv.max(c);
            } else {
                di = di.max(d);
                mi = mi.max(mm);
                ci = ci.max(c);
            }
        }
        let t = self.tuning;
        // Three points equally spaced by the CURRENT `h` are what makes the
        // second difference an error estimate. `hist` counts how many of
        // them we have; below two, `cmax` is a mixture of step sizes and
        // means nothing, and acting on it is what makes a naive multirate
        // controller oscillate between k and 1 forever.
        let warm = self.hist >= 2;
        // A discrete transition or a rescue means the trajectory just did
        // something the previous two samples know nothing about. That is a
        // fact about the devices, not an estimate, so it always counts.
        let disturbed = self.discrete_moved || nr > 1;

        // --- local dt. Raise only on a sustained, comfortable margin;
        //     collapse to the room dt the instant either estimate says the
        //     error would not fit, so a transient is never integrated
        //     coarsely. `4·c` is the curvature estimate at twice the step
        //     (the error term is h², so doubling h quadruples it) and `2·d`
        //     the motion at twice the step (first order, so it doubles).
        //
        //     TWO budgets, and the motion one is not redundant: curvature
        //     bounds the truncation error inside the step, motion bounds the
        //     read-out lag at the end of it. A constant-slope ramp has zero
        //     curvature and unbounded lag, which is exactly the shape the
        //     curvature test alone waves through.
        if t.local_dt {
            let curvy = warm && (cv > t.local_dt_err || ci > t.local_dt_err_i);
            let racing = dv > t.local_dt_slew * dt || di > t.local_dt_slew_i * dt;
            let room = 4.0 * cv <= t.local_dt_err
                && 4.0 * ci <= t.local_dt_err_i
                && 2.0 * dv <= t.local_dt_slew * dt
                && 2.0 * di <= t.local_dt_slew_i * dt;
            if disturbed || curvy || racing {
                self.drop_to_room_dt();
            } else if warm && room {
                self.good += 1;
            } else if warm {
                self.good = 0;
            }
        }

        // --- quiescence. Every condition in physical units, so the verdict
        //     does not change when `h` does, and every condition applied to
        //     the unknowns whose dimension it is stated in.
        if t.quiescence && self.sleepable {
            let quiet = !disturbed
                && dv <= t.quiescence_slew * h
                && di <= t.quiescence_slew_i * h
                && mv <= t.quiescence_drift
                && mi <= t.quiescence_drift_i;
            if quiet && warm {
                self.quiet_t += h;
                if self.quiet_t >= t.quiescence_hold {
                    if self.window_settled() {
                        self.asleep = true;
                    } else {
                        // Quiet, but not yet demonstrably finished. Close
                        // this window, remember how far it travelled, and
                        // let the next one measure the decay against it.
                        for i in 0..self.n {
                            self.win_prev[i] = (self.x[i] - self.x_mark[i]).abs();
                        }
                        self.have_win = true;
                        self.x_mark.copy_from_slice(&self.x);
                        self.quiet_t = 0.0;
                    }
                }
            } else {
                self.quiet_t = 0.0;
                self.have_win = false;
                self.x_mark.copy_from_slice(&self.x);
            }
        }

        // Roll the history forward.
        self.x_prev2.copy_from_slice(&self.x_prev);
        self.x_prev.copy_from_slice(&self.x);
        self.hist = self.hist.saturating_add(1);
        self.discrete_moved = false;
    }

    // ------------------------------------------------------------ stepping

    /// Advance this island by `max_steps` substeps of the room `dt`, from
    /// world time `t0`. The island holds no clock of its own: `t0` is what
    /// time-varying sources are evaluated against.
    ///
    /// The island may execute those substeps as fewer, larger integration
    /// steps (`h = k·dt`), or — if it has gone static — as none at all. It
    /// still *consumes* every substep it was given either way, so the world
    /// clock never depends on which islands were busy. The only state it
    /// carries between calls is `pending`: world substeps it owes because
    /// they did not add up to a whole local step yet, bounded by `k` and so
    /// by `Tuning::local_dt_max`.
    ///
    /// A failure here quarantines THIS island and nothing else.
    pub fn advance(&mut self, t0: f64, dt: f64, max_steps: u32) -> AdvanceReport {
        let mut report = AdvanceReport::default();
        if self.n == 0 || self.quarantined {
            report.quarantined = self.quarantined;
            return report;
        }
        // Where this island's own clock actually stands: behind world time
        // by whatever it still owes. At k = 1 the debt is always zero and
        // this is `t0` exactly, bit for bit.
        let mut t = t0 - self.pending as f64 * dt;
        self.pending += max_steps;

        if self.asleep {
            // Nothing is moving, so the held solution IS the solution of
            // every substep skipped here. No stamping, no factor, no solve,
            // no invented number: `x` and every device's state stay exactly
            // as the last real solve left them.
            self.pending = 0;
            report.steps = max_steps;
            report.static_islands = 1;
            return report;
        }

        report.islands = 1;
        let cap = self.k_cap(dt);
        if self.k > cap {
            // The caller changed `dt`, or an instrument was pointed at this
            // island since the last call. Same history rule as any other
            // step-size change.
            self.k = cap;
            self.good = 0;
            self.hist = 1;
        }
        let mut advanced = 0u32;
        while self.pending >= self.k {
            let k = self.k;
            let h = dt * k as f64;
            // A waveform edge is an EVENT, and it arms the same two backward-
            // Euler steps a switch flip does rather than just one. One is not
            // enough: the step containing the jump is integrated cleanly, but
            // the transient it kicks off is still moving when trapezoid
            // resumes, and it rings on that. Measured on a 5 V square into an
            // RC at tau = dt/10 — peak node voltage 5.163 V with no arming,
            // 5.082 V arming only the edge step, 5.028 V with the full two.
            // The 0.5% that remains is not ringing: it is the edge landing
            // between two samples, which is inherent to sampling a
            // discontinuity at finite dt and cannot be integrated away.
            if self.step_has_edge(t, t + h) {
                self.be_steps = self.be_steps.max(BE_STEPS_AFTER_EVENT);
            }
            let be = self.be_steps > 0;
            let nr0 = report.nr_iters;
            let resc0 = report.rescues;
            match self.step(t, h, be, 0, &mut report) {
                Ok(()) => {
                    t += h;
                    self.pending -= k;
                    advanced += k;
                    self.be_steps = self.be_steps.saturating_sub(1);
                    report.island_steps += 1;
                    // A step that only survived the dt-halving ladder is the
                    // loudest possible "this step was too big".
                    if report.rescues > resc0 {
                        self.drop_to_room_dt();
                        self.quiet_t = 0.0;
                    }
                    self.after_step(h, dt, report.nr_iters - nr0);
                    if self.asleep {
                        break;
                    }
                    // Doubling is decided here, not in `after_step`, because
                    // only the caller's `dt` knows the cap. `build()` refactors
                    // on its own when `h` changes (the companion conductances
                    // depend on it), so nothing else has to be invalidated.
                    if self.good >= self.tuning.local_dt_hold && self.k < cap {
                        self.k *= 2;
                        self.good = 0;
                        // One banked point: the step just taken ended where
                        // the first step at the new `h` will begin, so two
                        // more steps make three evenly spaced samples.
                        self.hist = 1;
                    }
                }
                Err(()) => {
                    self.quarantined = true;
                    break;
                }
            }
        }
        report.steps = if self.quarantined {
            advanced.min(max_steps)
        } else {
            // Every substep handed in has been accounted for: executed, or
            // carried as debt that is smaller than one local step.
            max_steps
        };
        if self.asleep {
            self.pending = 0;
        }
        report.quarantined = self.quarantined;
        report
    }

    /// One accepted step of size `h` from time `t`, recursing into halved BE
    /// steps if NR fails. On success, device history state has been advanced.
    fn step(
        &mut self,
        t: f64,
        h: f64,
        be: bool,
        depth: u32,
        report: &mut AdvanceReport,
    ) -> Result<(), ()> {
        // One reusable snapshot buffer per recursion level: the rescue ladder
        // is 4 deep, and a substep must not allocate.
        let mut saved = core::mem::take(&mut self.saved[depth as usize]);
        saved.clear();
        saved.extend(self.elems[..self.active].iter().map(|e| e.state));
        let out = match self.solve_step(t, h, be, report) {
            Ok(()) => Ok(()),
            Err(()) => {
                for (e, s) in self.elems[..self.active].iter_mut().zip(saved.iter()) {
                    e.state = *s;
                }
                // ...and un-shift any bucket chain that moved during the step
                // being thrown away. Without this a rejected step leaves its
                // sample in the line permanently and the delay lengthens by
                // one bucket per rescue — a slow, silent drift that no
                // voltage assertion would ever notice. O(1) per chain and
                // idempotent, so the half-steps the ladder takes next record
                // and undo their own writes independently.
                for line in self.delays.values_mut() {
                    line.rollback();
                }
                // The rollback restores discrete state (`region` lives in
                // `ElemState`), so the retained factorization may now
                // describe a region set that no longer exists. Drop it
                // before anything can look at it — including on the
                // give-up path below, where quarantine follows but a live
                // `write_param` could still resume stepping.
                self.factor_valid = false;
                if depth >= RESCUE_DEPTH {
                    Err(())
                } else {
                    report.rescues += 1;
                    // Backward Euler at half the step, twice: robust against
                    // both nonconvergence and trapezoidal ringing.
                    self.step(t, h * 0.5, true, depth + 1, report)
                        .and_then(|()| self.step(t, h * 0.5, true, depth + 1, report))
                        .map(|()| self.factor_valid = false)
                }
            }
        };
        self.saved[depth as usize] = saved;
        out
    }

    /// Newton-Raphson (single pass for linear circuits) at t + h.
    fn solve_step(
        &mut self,
        t: f64,
        h: f64,
        be: bool,
        report: &mut AdvanceReport,
    ) -> Result<(), ()> {
        let iters = if self.linear { 1 } else { NR_MAX_ITERS };
        let mut converged = self.linear;
        // Reset per-pass discrete-state-change budgets for the op-amp rail
        // region and the 555 latch (lastv[0] doubles as the counter for
        // both; neither has MOS damping state).
        // The same pre-pass draws each noise source's sample for this step.
        for e in self.elems[..self.active].iter_mut() {
            let kind = e.spec.kind;
            if matches!(kind, ElementKind::OpAmp { .. } | ElementKind::Timer555) {
                e.state.lastv[0] = 0.0;
            }
            // Drawn ONCE, before the NR loop, and held constant through it:
            // a source that moved under Newton's feet would never converge.
            // `step()` snapshots every ElemState before calling us and
            // restores it if we fail, so a rescued step rewinds the counter
            // and its two half-size backward-Euler retries each draw their
            // own sample — deterministic on every path through the ladder.
            // A part that has failed open freezes its stream, matching the
            // way `accept()` freezes a broken part's history.
            if let ElementKind::Noise { volts, seed, .. } = kind {
                if !e.broken {
                    e.state.v_prev = volts * noise_unit(seed, e.state.noise_n);
                    e.state.noise_n = e.state.noise_n.wrapping_add(1);
                }
            }
        }
        for _ in 0..iters {
            report.nr_iters += 1;
            self.build(t + h, h, be)?;
            self.x.copy_from_slice(&self.b);
            self.lu.solve(&mut self.x);
            if self.x.iter().any(|v| !v.is_finite()) {
                return Err(());
            }
            if self.linear {
                break;
            }
            converged = self.update_guesses();
            if converged {
                break;
            }
        }
        if !converged {
            return Err(());
        }
        self.accept(h, be);
        Ok(())
    }

    #[inline]
    fn xv(&self, node: usize) -> f64 {
        if node == 0 {
            0.0
        } else {
            self.x[node - 1]
        }
    }

    // ------------------------------------------------------------ stamping

    #[inline]
    fn stamp_g(&mut self, p: usize, q: usize, g: f64) {
        let n = self.n;
        if p > 0 {
            self.a[(p - 1) * n + (p - 1)] += g;
        }
        if q > 0 {
            self.a[(q - 1) * n + (q - 1)] += g;
        }
        if p > 0 && q > 0 {
            self.a[(p - 1) * n + (q - 1)] -= g;
            self.a[(q - 1) * n + (p - 1)] -= g;
        }
    }

    /// dI(into element at pin-node p)/dV(node q).
    #[inline]
    fn stamp_partial(&mut self, p: usize, q: usize, g: f64) {
        if p > 0 && q > 0 {
            self.a[(p - 1) * self.n + (q - 1)] += g;
        }
    }

    /// Constant current I INTO the element at pin-node p.
    #[inline]
    fn stamp_i_into(&mut self, p: usize, i: f64) {
        if p > 0 {
            self.b[p - 1] -= i;
        }
    }

    /// Stamp this island's system for time `t_new` and step `h`. Factors the
    /// matrix unless a valid retained factorization is being reused.
    fn build(&mut self, t_new: f64, h: f64, be: bool) -> Result<(), ()> {
        let need_factor = !(self.reusable()
            && self.factor_valid
            && self.factored_h == h
            && self.factored_be == be);
        if need_factor {
            self.a.iter_mut().for_each(|v| *v = 0.0);
        }
        self.b.iter_mut().for_each(|v| *v = 0.0);

        if need_factor {
            let n = self.n;
            for k in 0..self.num_nodes {
                self.a[k * n + k] += GMIN;
            }
        }

        for ei in 0..self.active {
            let (kind, node, branch, state, stamps) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch, e.state, e.stamps)
            };
            if !stamps {
                // A merged member of an ideal-constraint group. The leader
                // already wrote the row and the RHS; writing again would
                // accumulate the ±1 incidence to ±N. The group's voltage is
                // the leader's — deterministic, and by construction the two
                // agree to within the merge tolerance.
                continue;
            }
            let n = self.n;
            match kind {
                // A label stamps NOTHING. It merged its nodes back in `rebuild`,
                // which is the whole of what it does — same as a wire.
                ElementKind::Wire | ElementKind::Ground | ElementKind::Label => {}
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => {
                    if need_factor {
                        self.stamp_g(node[0], node[1], 1.0 / ohms);
                    }
                }
                ElementKind::Potentiometer { ohms, wiper } => {
                    if need_factor {
                        let r1 = (ohms * wiper).max(1e-3);
                        let r2 = (ohms * (1.0 - wiper)).max(1e-3);
                        self.stamp_g(node[0], node[1], 1.0 / r1);
                        self.stamp_g(node[1], node[2], 1.0 / r2);
                    }
                }
                // A photocell IS a resistor here. The only difference is
                // where the ohms came from, and the matrix cannot tell.
                ElementKind::Photocell {
                    r_dark,
                    r_lit,
                    light,
                } => {
                    if need_factor {
                        self.stamp_g(
                            node[0],
                            node[1],
                            1.0 / crate::photocell_ohms(r_dark, r_lit, light),
                        );
                    }
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let ieq = if be {
                        -geq * state.v_prev
                    } else {
                        -(geq * state.v_prev + state.i_prev)
                    };
                    if need_factor {
                        self.stamp_g(node[0], node[1], geq);
                    }
                    self.stamp_i_into(node[0], ieq);
                    self.stamp_i_into(node[1], -ieq);
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let ieq = if be {
                        state.i_prev
                    } else {
                        state.i_prev + geq * state.v_prev
                    };
                    if need_factor {
                        self.stamp_g(node[0], node[1], geq);
                    }
                    self.stamp_i_into(node[0], ieq);
                    self.stamp_i_into(node[1], -ieq);
                }
                ElementKind::CurrentSource { amps } => {
                    self.stamp_i_into(node[0], amps);
                    self.stamp_i_into(node[1], -amps);
                }
                ElementKind::Noise { ohms, .. } => {
                    // Norton form of (EMF in series with `ohms`): a fixed
                    // conductance plus an injected current. The conductance
                    // never changes, so it sits under `need_factor` exactly
                    // like a resistor's — a noise source is RHS-only and
                    // forces no refactorization, which is what makes a
                    // linear noise circuit stay on the reused factorization.
                    if need_factor {
                        self.stamp_g(node[0], node[1], 1.0 / ohms);
                    }
                    let i = state.v_prev / ohms;
                    self.stamp_i_into(node[0], -i);
                    self.stamp_i_into(node[1], i);
                }
                ElementKind::VoltageSource {
                    dc,
                    amp,
                    hz,
                    phase,
                    wave,
                } => {
                    let v = source_value(dc, amp, hz, phase, wave, t_new);
                    let bi = self.num_nodes + branch.ok_or(())?;
                    if need_factor {
                        for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                            if pin > 0 {
                                self.a[bi * n + (pin - 1)] += sgn;
                                self.a[(pin - 1) * n + bi] += sgn;
                            }
                        }
                    }
                    self.b[bi] = v;
                }
                ElementKind::Rail {
                    dc,
                    amp,
                    hz,
                    phase,
                    wave,
                } => {
                    // A voltage source whose far terminal IS ground: only the
                    // one pin stamps, and node 0 has no row to receive the
                    // return current (exactly like a grounded two-pin source).
                    let v = source_value(dc, amp, hz, phase, wave, t_new);
                    let bi = self.num_nodes + branch.ok_or(())?;
                    if need_factor && node[0] > 0 {
                        self.a[bi * n + (node[0] - 1)] += 1.0;
                        self.a[(node[0] - 1) * n + bi] += 1.0;
                    }
                    self.b[bi] = v;
                }
                ElementKind::Pt2399 => {
                    // Pins [IN, OUT, RT, GND].
                    //
                    // The BRANCH is spent on the RT reference, not on the
                    // output. That is deliberate: the current the player's
                    // resistor draws from RT is the input to the entire
                    // mechanism, and reading it out of a Norton equivalent
                    // would make it a small difference between two large
                    // numbers. As an ideal source it IS the branch unknown,
                    // exactly.
                    //
                    // The output pays for that by having a real source
                    // impedance instead of being ideal — which a buffered
                    // chip output has anyway, so the model gets more honest
                    // rather than less.
                    let (vin, vout, vrt, vgnd) = (node[0], node[1], node[2], node[3]);
                    let g_rt = 1.0 / crate::PT_R_RT;
                    if need_factor {
                        self.stamp_g(vin, vgnd, BBD_G_IN);
                        self.stamp_g(vout, vgnd, PT_G_OUT);
                        self.stamp_g(vrt, vgnd, g_rt);
                    }
                    // Two Thevenin sources as Nortons: the held sample behind
                    // PT_R_OUT at the output, and the reference behind
                    // PT_R_RT at RT. Neither is ideal, so this part owns no
                    // branch unknown at all — and, more to the point, RT tied
                    // to ground is now an ORDINARY circuit rather than a
                    // contradiction the gate has to refuse.
                    //
                    // It also makes a FLOATING RT mean exactly zero current
                    // instead of the trickle the input tether used to leak,
                    // so an unwired delay pin is honest silence rather than a
                    // six-second delay that reads as a broken part.
                    // Collected, not applied inline: the stamps below need
                    // `self` mutably too, and a closure holding it would lock
                    // them out.
                    let mut inj: [(usize, usize, f64); 6] = [
                        (vout, vgnd, state.v_prev * PT_G_OUT),
                        (vrt, vgnd, crate::PT_V_RT * g_rt),
                        (0, 0, 0.0),
                        (0, 0, 0.0),
                        (0, 0, 0.0),
                        (0, 0, 0.0),
                    ];

                    // ------------------------------------- the two op-amps
                    //
                    // Each is a transconductance into an output impedance:
                    //     i_out = gm * (VREF - v(inv))
                    // with the + input pinned internally at VREF, which is
                    // the real chip's pinout and the reason these are always
                    // used as inverting stages. Open-loop gain is gm/gout.
                    //
                    // The gm term is a VOLTAGE-CONTROLLED CURRENT SOURCE and
                    // stamps straight into the conductance matrix, so neither
                    // op-amp needs a branch unknown. The VREF part of it is a
                    // constant and rides the RHS.
                    //
                    // `region` carries both clamp states, two bits each, in
                    // the spare half of `dstate`: 0 linear, 1 pinned low,
                    // 2 pinned high. A pinned op-amp stops being a gm stage
                    // and becomes a stiff source at the rail, which is what
                    // makes a runaway feedback loop CLIP instead of diverge.
                    for (k, (inv, out)) in [
                        (node[4], node[5]),   // OP1
                        (node[6], node[7]),   // OP2
                        (node[8], node[9]),   // LPF1
                        (node[10], node[11]), // LPF2
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        // 0 linear, 1 pinned low, 2 pinned high. Decided from
                        // the INPUT in `update_guesses` — see there for why
                        // that is the only version of this test that settles.
                        let reg = (state.dstate >> (D_PT_OA + 2 * k)) & 3;
                        if need_factor {
                            self.stamp_g(out, vgnd, crate::PT_OA_GOUT);
                            // A gm stage's input is a pure control terminal
                            // and draws nothing — but "nothing" and "no path
                            // at all" are different to a matrix, and an
                            // op-amp nobody wired would be a singular row.
                            // Same tether the delay input carries.
                            self.stamp_g(inv, vgnd, BBD_G_IN);
                            // -gm * v(inv) into `out`, referred to GND —
                            // but ONLY while the stage is linear. Saturated,
                            // the transconductance stops responding to its
                            // input and becomes a constant current, which is
                            // what a real output stage does when it runs out
                            // of swing.
                            if reg == 0 {
                                let gm = crate::PT_OA_GM;
                                for (r, sgn) in [(out, 1.0), (vgnd, -1.0)] {
                                    if r > 0 && inv > 0 {
                                        self.a[(r - 1) * n + (inv - 1)] += sgn * gm;
                                    }
                                }
                            }
                        }
                        let drive = if reg != 0 {
                            // SATURATED: gm delivers its limit current, so
                            // the open-circuit output sits exactly on the
                            // rail — and UNDER LOAD IT SAGS, because a
                            // saturated stage has finite drive. That sag is
                            // free here and is the honest behaviour; a hard
                            // voltage clamp would have pretended otherwise.
                            let d = if reg == 2 {
                                crate::PT_OA_HI - crate::PT_V_RT
                            } else {
                                crate::PT_OA_LO - crate::PT_V_RT
                            };
                            crate::PT_V_RT + d
                        } else {
                            // The output is
                            //     v_out = VREF + A * (VREF - v_inv)
                            // and BOTH constant terms have to be here. The
                            // first version dropped the leading VREF, which
                            // put the stage's rest point at 0 V instead of at
                            // the half-supply everything else is referred to —
                            // so a unity-gain buffer delivered 0.046 V for a
                            // 1 V input and looked like a loading problem.
                            crate::PT_V_RT * (1.0 + crate::PT_OA_GM / crate::PT_OA_GOUT)
                        };
                        inj[2 + k] = (out, vgnd, drive * crate::PT_OA_GOUT);
                    }
                    for (p, q, i) in inj {
                        if p > 0 {
                            self.b[p - 1] += i;
                        }
                        if q > 0 {
                            self.b[q - 1] -= i;
                        }
                    }
                }
                ElementKind::Bbd { .. } => {
                    // Pins [IN, OUT, CLK, GND].
                    //
                    // Nothing about the bucket chain appears here. What the
                    // matrix sees is one ideal voltage source holding the
                    // sample that fell out of the far end, plus two
                    // high-impedance inputs — the same shape as the motor,
                    // and for the same reason: the state lives outside the
                    // matrix and only ever writes the RHS.
                    //
                    // The held sample rides in `v_prev`, which puts it in the
                    // state digest and in the rescue ladder's snapshot for
                    // free.
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let (vin, vout, vclk, vgnd) = (node[0], node[1], node[2], node[3]);
                    if need_factor {
                        // OUT is driven against GND, not against node 0: the
                        // output current has to come out of the part's own
                        // ground pin, the way the 555's does, or it appears
                        // from nowhere and KCL stops meaning anything.
                        for (pin, sgn) in [(vout, 1.0), (vgnd, -1.0)] {
                            if pin > 0 {
                                self.a[bi * n + (pin - 1)] += sgn;
                                self.a[(pin - 1) * n + bi] += sgn;
                            }
                        }
                        // IN and CLK draw ~nothing, but they may not FLOAT:
                        // an unconnected input with no path to anywhere is a
                        // singular row. This is the same 1 MΩ-ish tether an
                        // op-amp input gets.
                        self.stamp_g(vin, vgnd, BBD_G_IN);
                        self.stamp_g(vclk, vgnd, BBD_G_IN);
                    }
                    self.b[bi] = state.v_prev;
                }
                ElementKind::Motor {
                    ohms,
                    henries,
                    bemf,
                } => {
                    // v0 - v1 = R·i + L·di/dt + bemf with i the branch
                    // unknown (current INTO pin 0). Backward Euler on the
                    // inductive term — di/dt ≈ (i - i_prev)/h — gives the
                    // row  v0 - v1 - (R + L/h)·i = bemf - (L/h)·i_prev.
                    // BE unconditionally: the armature pole (L/R = 0.75 ms
                    // for the shipped hoist motor) is stiff next to the
                    // machine tick, and BE cannot ring against it.
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let gl = henries / h;
                    if need_factor {
                        for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                            if pin > 0 {
                                self.a[bi * n + (pin - 1)] += sgn;
                                self.a[(pin - 1) * n + bi] += sgn;
                            }
                        }
                        self.a[bi * n + bi] -= ohms + gl;
                    }
                    self.b[bi] = bemf - gl * state.i_prev;
                }
                ElementKind::Switch { closed } | ElementKind::Button { closed } => {
                    if closed {
                        let bi = self.num_nodes + branch.ok_or(())?;
                        if need_factor {
                            for (pin, sgn) in [(node[0], 1.0), (node[1], -1.0)] {
                                if pin > 0 {
                                    self.a[bi * n + (pin - 1)] += sgn;
                                    self.a[(pin - 1) * n + bi] += sgn;
                                }
                            }
                        }
                        self.b[bi] = 0.0;
                    }
                }
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let vg = state.vg1;
                    let ef = libm::exp(vg / nvt);
                    let mut g = is / nvt * ef;
                    let mut i0 = is * (ef - 1.0);
                    if let Some(voff) = voff {
                        // Reverse breakdown branch (Zener).
                        let er = libm::exp(-(vg + voff) / nvt);
                        g += is / nvt * er;
                        i0 -= is * er;
                    }
                    let i_lin = i0 - g * vg;
                    self.stamp_g(node[0], node[1], g);
                    self.stamp_i_into(node[0], i_lin);
                    self.stamp_i_into(node[1], -i_lin);
                }
                ElementKind::Npn { beta } | ElementKind::Pnp { beta } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let (b_, c, e) = (node[0], node[1], node[2]);
                    let (vbe, vbc) = (state.vg1, state.vg2);
                    let ebe = libm::exp(vbe / VT);
                    let ebc = libm::exp(vbc / VT);
                    let gbe = BJT_IS / VT * ebe;
                    let gbc = BJT_IS / VT * ebc;
                    let i_f = BJT_IS * (ebe - 1.0);
                    let i_r = BJT_IS * (ebc - 1.0);
                    // Currents into collector/base (polarity-normalized).
                    let ic = i_f - i_r * (1.0 + 1.0 / BJT_BETA_R);
                    let ib = i_f / beta + i_r / BJT_BETA_R;
                    let d_ic = (gbe, -gbc * (1.0 + 1.0 / BJT_BETA_R)); // (d/dvbe, d/dvbc)
                    let d_ib = (gbe / beta, gbc / BJT_BETA_R);
                    // Conductance stamps are polarity-independent (pol^2).
                    for (pin, cur, d) in [(c, ic, d_ic), (b_, ib, d_ib)] {
                        self.stamp_partial(pin, b_, d.0 + d.1);
                        self.stamp_partial(pin, e, -d.0);
                        self.stamp_partial(pin, c, -d.1);
                        self.stamp_i_into(pin, pol * (cur - d.0 * vbe - d.1 * vbc));
                    }
                    // Emitter = -(collector + base).
                    let d_ie = (-(d_ic.0 + d_ib.0), -(d_ic.1 + d_ib.1));
                    self.stamp_partial(e, b_, d_ie.0 + d_ie.1);
                    self.stamp_partial(e, e, -d_ie.0);
                    self.stamp_partial(e, c, -d_ie.1);
                    let ie = -(ic + ib);
                    self.stamp_i_into(e, pol * (ie - d_ie.0 * vbe - d_ie.1 * vbc));
                }
                ElementKind::Nmos { vt, k } | ElementKind::Pmos { vt, k } => {
                    let pol = if matches!(kind, ElementKind::Nmos { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let m = mos_eval(pol, vt, k, &state.lastv);
                    // Currents into effective drain d / source s.
                    let (g_, d, s) = (node[0], m.d_pin(node), m.s_pin(node));
                    let i0 = pol * (m.id - m.gm * m.vgs - m.gds * m.vds);
                    self.stamp_partial(d, g_, m.gm);
                    self.stamp_partial(d, d, m.gds);
                    self.stamp_partial(d, s, -(m.gm + m.gds));
                    self.stamp_i_into(d, i0);
                    self.stamp_partial(s, g_, -m.gm);
                    self.stamp_partial(s, d, -m.gds);
                    self.stamp_partial(s, s, m.gm + m.gds);
                    self.stamp_i_into(s, -i0);
                }
                ElementKind::Ota => {
                    let (p, m, out, bias) = (node[0], node[1], node[2], node[3]);
                    // Bias pin: diode junction to ground; injected current
                    // is Iabc.
                    let vb = state.vg2;
                    let eb = libm::exp(vb / VT);
                    let g_b = OTA_IS / VT * eb;
                    let i_b = OTA_IS * (eb - 1.0);
                    self.stamp_partial(bias, bias, g_b);
                    self.stamp_i_into(bias, i_b - g_b * vb);
                    // Output: Iout = Iabc * tanh(vd / 2Vt) flowing OUT of
                    // the out pin. Linearize in vd AND vbias.
                    let iabc = i_b.max(0.0);
                    let vd = state.vg1;
                    let th = libm::tanh(vd / (2.0 * VT));
                    let gm_eff = iabc / (2.0 * VT) * (1.0 - th * th);
                    let d_ib = if i_b > 0.0 { g_b } else { 0.0 };
                    let iout = iabc * th;
                    // I_into(out) = -Iout; partials are negated.
                    self.stamp_partial(out, p, -gm_eff);
                    self.stamp_partial(out, m, gm_eff);
                    self.stamp_partial(out, bias, -d_ib * th);
                    self.stamp_i_into(out, -iout + gm_eff * vd + d_ib * th * vb);
                }
                ElementKind::Timer555 => {
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let (vcc, gp, out, dis) = (node[0], node[1], node[4], node[5]);
                    // Quiescent supply current: the chip's own bias
                    // network, so the rails carry current even with the
                    // output unloaded and KCL stays sane.
                    // NOTE: every write into `a` below is guarded by
                    // `need_factor`. The matrix is only re-zeroed when it is
                    // about to be refactored, so an unguarded `+= 1.0` would
                    // accumulate into a retained matrix on every reuse hit.
                    // The RHS (`b`) is re-zeroed every pass and must always
                    // be written.
                    if need_factor {
                        self.stamp_g(vcc, gp, T555_G_QUIESCENT);
                        // Discharge pin: saturated transistor to GND while
                        // the latch is low, open circuit while it is high.
                        if state.region == 0 {
                            self.stamp_g(dis, gp, T555_G_DIS);
                        }
                    }
                    // Totem-pole output as a branch voltage source, referred
                    // to the rail it is working against: high sources from
                    // the VCC pin at vcc - 1.2 V, low sinks into the GND pin
                    // at 0.1 V. Tying the return to a supply pin is what
                    // makes the output current actually come out of the
                    // battery instead of appearing from nowhere.
                    let (ret, drop) = if state.region != 0 {
                        (vcc, -T555_VDROP_HIGH)
                    } else {
                        (gp, T555_VSAT_LOW)
                    };
                    if need_factor {
                        if out > 0 {
                            self.a[(out - 1) * n + bi] += 1.0;
                            self.a[bi * n + (out - 1)] += 1.0;
                        }
                        if ret > 0 {
                            self.a[(ret - 1) * n + bi] -= 1.0;
                            self.a[bi * n + (ret - 1)] -= 1.0;
                        }
                    }
                    self.b[bi] = drop;
                }
                ElementKind::OpAmp { rail, isc } => {
                    let bi = self.num_nodes + branch.ok_or(())?;
                    let (p, m, out) = (node[0], node[1], node[2]);
                    // Output branch current column. The branch unknown is
                    // the current INTO the out pin, so an op-amp SOURCING
                    // current carries a negative branch value. Guarded like
                    // the 555's stamps: `a` survives a reuse hit, `b` does not.
                    if need_factor && out > 0 {
                        self.a[(out - 1) * n + bi] += 1.0;
                    }
                    // Constraint row depends on the output-stage region:
                    //   0   linear     vout = A·(vp - vm + Voff)
                    //  ±1   railed     vout = ±rail
                    //  ±2   limited    i_out = ±isc, vout free
                    match state.region {
                        0 => {
                            if need_factor {
                                if p > 0 {
                                    self.a[bi * n + (p - 1)] += OPAMP_GAIN;
                                }
                                if m > 0 {
                                    self.a[bi * n + (m - 1)] -= OPAMP_GAIN;
                                }
                                if out > 0 {
                                    self.a[bi * n + (out - 1)] -= 1.0;
                                }
                            }
                            // vout = A(vp - vm + Voff)
                            self.b[bi] = -OPAMP_GAIN * OPAMP_VOFF;
                        }
                        r if r.abs() == 1 => {
                            if need_factor && out > 0 {
                                self.a[bi * n + (out - 1)] += 1.0;
                            }
                            self.b[bi] = r as f64 * rail;
                        }
                        r => {
                            // Folded back: the output stage is a current
                            // source of ±isc and the node voltage is
                            // whatever the load makes it. r = +2 means the
                            // amp is driving high, i.e. SOURCING isc, i.e. a
                            // branch (into-pin) current of -isc.
                            self.a[bi * n + bi] += 1.0;
                            self.b[bi] = -(r.signum() as f64) * isc;
                        }
                    }
                }
                ElementKind::Gate { .. }
                | ElementKind::FlipFlop { .. }
                | ElementKind::ShiftReg { .. }
                | ElementKind::Counter { .. }
                | ElementKind::Mux { .. } => {
                    // The whole family, one arm, and NOTHING is written into
                    // `b`: a CMOS chip is a switch network, not a source. It
                    // owns no branch unknown either, so every write below is
                    // a symmetric conductance and every one of them is
                    // guarded by `need_factor` — the matrix survives a reuse
                    // hit, so an unguarded `+=` would accumulate on every
                    // reuse.
                    //
                    // THE PROPERTY THE WHOLE DESIGN TURNS ON: the incidence
                    // pattern is identical in every discrete state. Both
                    // output conductances are always stamped and only their
                    // VALUES move, so a positive-conductance network is all
                    // this contributes in any state — it cannot induce a
                    // singularity, which is what makes `probe_solvable`'s
                    // single cold factorization sound for this family.
                    if need_factor {
                        let (vcc, gnd) = (node[0], node[1]);
                        let lp = kind.logic_pins().ok_or(())?;
                        let d = state.dstate;

                        self.stamp_g(vcc, gnd, LOGIC_G_QUIESCENT);
                        // Latch-up is a hard short across the supply. Set in
                        // `accept`, never here.
                        if dbit(d, D_LATCHED) {
                            self.stamp_g(vcc, gnd, LOGIC_G_LATCHUP);
                        }
                        // Inputs: symmetric leak to both rails.
                        for k in 0..lp.n_in {
                            let p = node[lp.in0 + k];
                            self.stamp_g(p, vcc, LOGIC_G_IN);
                            self.stamp_g(p, gnd, LOGIC_G_IN);
                        }
                        // Outputs: the totem pole, referred to the supply
                        // PINS. This is what makes the current a gate
                        // delivers actually come out of the player's battery
                        // instead of appearing from nowhere — the 555's
                        // lesson, reached without a branch row.
                        for k in 0..lp.n_out {
                            let p = node[lp.out0 + k];
                            let (g_pu, g_pd) = if dbit(d, D_DATA + k) {
                                (LOGIC_G_ON, LOGIC_G_OFF)
                            } else {
                                (LOGIC_G_OFF, LOGIC_G_ON)
                            };
                            self.stamp_g(p, vcc, g_pu);
                            self.stamp_g(p, gnd, g_pd);
                        }
                        // The mux is the one part whose signal path is a PASS
                        // GATE rather than a driver: the selected channel is
                        // connected to Y through 50 Ω and the rest through
                        // 1 GΩ, in both directions. That is a 4051, not a
                        // '153 — and because it is a conductance it passes
                        // analog, which costs nothing extra here.
                        if let ElementKind::Mux { .. } = kind {
                            let chans = 1usize << lp.n_in;
                            let y = node[lp.in0 + lp.n_in];
                            let on = dfield(d, D_DATA, lp.n_in) as usize;
                            for j in 0..chans {
                                let g = if j == on { LOGIC_G_ON } else { LOGIC_G_OFF };
                                self.stamp_g(node[2 + j], y, g);
                            }
                        }
                    }
                }
            }
        }

        if need_factor {
            self.factorizations += 1;
            let ok = {
                let a = core::mem::take(&mut self.a);
                let ok = self.lu.factor(&a);
                self.a = a;
                ok
            };
            #[cfg(feature = "dump-matrix")]
            {
                std::eprintln!("t={t_new} n={}", self.n);
                for r in 0..self.n {
                    let row: Vec<f64> = (0..self.n).map(|c| self.a[r * self.n + c]).collect();
                    std::eprintln!("  {row:?} | {}", self.b[r]);
                }
            }
            if !ok {
                return Err(());
            }
            if self.reusable() {
                self.factor_valid = true;
                self.factored_h = h;
                self.factored_be = be;
            }
        }
        let _ = t_new;
        Ok(())
    }

    /// Post-solve NR bookkeeping: limit and store each nonlinear device's
    /// new operating-point guess. Returns true when every device in THIS
    /// island agrees with its previous guess (converged) — convergence is no
    /// longer all-or-nothing across the world.
    fn update_guesses(&mut self) -> bool {
        let mut converged = true;
        // Set when a piecewise-linear device ACTUALLY moves its discrete
        // state. That, and only that, is what makes a retained
        // factorization stale: this is the "event" in event-driven. It is
        // also the sharpest "this island is not resting" signal there is,
        // and the two levers read it as one — a rail flip is both.
        let mut discrete_flip = false;
        let close = |a: f64, b: f64| (a - b).abs() < NR_ABSTOL + NR_RELTOL * a.abs().max(b.abs());
        for ei in 0..self.active {
            let (kind, node) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node)
            };
            match kind {
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let vd = self.xv(node[0]) - self.xv(node[1]);
                    let old = self.elems[ei].state.vg1;
                    let vcrit = nvt * libm::log(nvt / (core::f64::consts::SQRT_2 * is));
                    let new = if let (Some(voff), true) = (voff, vd < 0.0) {
                        // Limit the reverse junction like a forward one.
                        -(pnjlim(-(vd + voff), -(old + voff), nvt, vcrit)) - voff
                    } else {
                        pnjlim(vd, old, nvt, vcrit)
                    };
                    if !close(new, old) {
                        converged = false;
                    }
                    self.elems[ei].state.vg1 = new;
                }
                ElementKind::Npn { .. } | ElementKind::Pnp { .. } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let vcrit = VT * libm::log(VT / (core::f64::consts::SQRT_2 * BJT_IS));
                    let vbe = pol * (self.xv(node[0]) - self.xv(node[2]));
                    let vbc = pol * (self.xv(node[0]) - self.xv(node[1]));
                    let st = &mut self.elems[ei].state;
                    let nbe = pnjlim(vbe, st.vg1, VT, vcrit);
                    let nbc = pnjlim(vbc, st.vg2, VT, vcrit);
                    if !close(nbe, st.vg1) || !close(nbc, st.vg2) {
                        converged = false;
                    }
                    st.vg1 = nbe;
                    st.vg2 = nbc;
                }
                ElementKind::Nmos { .. } | ElementKind::Pmos { .. } => {
                    let mut vs = [self.xv(node[0]), self.xv(node[1]), self.xv(node[2])];
                    // Breakdown limiting — the MOSFET's version of what
                    // `pnjlim` does for a diode junction.
                    //
                    // A solved drain voltage past the avalanche knee is an
                    // extrapolation off a nearly vertical curve, so the
                    // honest next guess is the knee itself. And because
                    // that is a BOUND rather than an extrapolation it can
                    // be taken in one step, instead of crawling there half
                    // a volt an iteration: an inductive turn-off has to
                    // move the drain fifty-odd volts inside one timestep,
                    // which at `MOS_DAMP` costs more NR passes than there
                    // are — that is precisely why an unclamped turn-off
                    // used to diverge and freeze the whole room.
                    //
                    // Everything below `MOS_BV` is untouched, damping and
                    // all, so no sub-breakdown circuit moves by one bit.
                    let st = &mut self.elems[ei].state;
                    let vds = vs[1] - vs[2];
                    let sgn = if vds >= 0.0 { 1.0 } else { -1.0 };
                    let now = sgn * vds;
                    let was = sgn * (st.lastv[1] - st.lastv[2]);
                    let breaking = now > MOS_BV - MOS_BV_MARGIN || was > MOS_BV - MOS_BV_MARGIN;
                    if breaking {
                        // The exponential is limited the way every other
                        // junction in this engine is limited, so it steps
                        // ONTO the curve from below in one pass and falls
                        // off it freely in one pass — instead of crawling
                        // at MOS_DAMP in both directions.
                        let vcrit =
                            MOS_BV_NVT * libm::log(MOS_BV_NVT / (core::f64::consts::SQRT_2 * MOS_BV_IS));
                        let lim = MOS_BV + pnjlim(now - MOS_BV, was - MOS_BV, MOS_BV_NVT, vcrit);
                        if (lim - now).abs() > 0.01 {
                            converged = false;
                        }
                        vs[1] = vs[2] + sgn * lim;
                    }
                    for (p, v) in vs.iter().enumerate() {
                        let last = &mut st.lastv[p];
                        let delta = v - *last;
                        if delta.abs() > 0.01 {
                            converged = false;
                        }
                        *last += if breaking && p == 1 {
                            delta
                        } else {
                            delta.clamp(-MOS_DAMP, MOS_DAMP)
                        };
                    }
                }
                ElementKind::Ota => {
                    let vd = self.xv(node[0]) - self.xv(node[1]);
                    let vb = self.xv(node[3]);
                    let vcrit = VT * libm::log(VT / (core::f64::consts::SQRT_2 * OTA_IS));
                    let st = &mut self.elems[ei].state;
                    let nb = pnjlim(vb, st.vg2, VT, vcrit);
                    if !close(vd, st.vg1) || !close(nb, st.vg2) {
                        converged = false;
                    }
                    st.vg1 = vd; // tanh is safe at any argument; no limiting
                    st.vg2 = nb;
                }
                ElementKind::Timer555 => {
                    // Thresholds track the LIVE supply through the internal
                    // divider: a sagging rail moves both comparators.
                    let vg = self.xv(node[1]);
                    let vcc = self.xv(node[0]) - vg;
                    let vtrig = self.xv(node[2]) - vg;
                    let vthr = self.xv(node[3]) - vg;
                    let st = &mut self.elems[ei].state;
                    // RS latch. Trigger below vcc/3 sets (output high) and
                    // dominates — holding TRIG low pins the output high on
                    // a real 555 too; threshold above 2·vcc/3 resets.
                    let latch = if vtrig < vcc * T555_TRIG_FRAC {
                        1
                    } else if vthr > vcc * T555_THR_FRAC {
                        0
                    } else {
                        st.region
                    };
                    // At most 2 latch changes per NR pass, exactly like the
                    // op-amp rail regions: right at a comparator crossing
                    // the two states can point at each other forever, and
                    // holding the current one yields a consistent solve that
                    // the next substep's capacitor motion resolves.
                    if latch != st.region && st.lastv[0] < 2.0 {
                        st.lastv[0] += 1.0;
                        converged = false;
                        st.region = latch;
                        discrete_flip = true;
                    }
                }
                ElementKind::Pt2399 => {
                    // The two internal op-amps pick their region HERE, inside
                    // the Newton loop, from their INPUTS. See PT_OA_VLIN for
                    // why the input and not the output: an output-side test
                    // cannot settle at this gain, and the first version of
                    // this chattered until the solver quarantined the room.
                    let vgnd = self.xv(node[3]);
                    for (k, inv) in [4usize, 6, 8, 10].into_iter().enumerate() {
                        let d = crate::PT_V_RT - (self.xv(node[inv]) - vgnd);
                        let reg = (self.elems[ei].state.dstate >> (D_PT_OA + 2 * k)) & 3;
                        // HYSTERESIS, and it is not a nicety. The linear
                        // window is +/-0.2 mV wide (the gain is 1e4 and the
                        // swing is 2.1 V), so Newton lands inside it only
                        // approximately — and a bare threshold flips the
                        // region on the approach, every pass, until the
                        // solver gives up and quarantines the room. That is
                        // exactly what happened: a working echo died the
                        // moment clipping was switched on.
                        //
                        // Leaving saturation needs the input to come WELL
                        // back inside the window; entering needs it clearly
                        // outside. The same trick `OpAmp` plays with its
                        // 1.000001 factors, for the same reason.
                        const OUT: f64 = 1.0; // enter at the window edge
                        const BACK: f64 = 0.5; // leave at half of it
                        let edge = crate::PT_OA_VLIN * if reg == 0 { OUT } else { BACK };
                        let want: u32 = if d > edge {
                            2 // driven past the top of its swing
                        } else if d < -edge {
                            1
                        } else {
                            0
                        };
                        // A SATURATED STAGE MAY NOT TELEPORT TO THE OTHER
                        // RAIL. Going 2 -> 1 in one pass moves the output
                        // 4.2 V in a single discontinuity, and the solver
                        // cannot follow it: measured, a stage that had been
                        // resting on its top rail for thousands of steps
                        // flipped straight to the bottom one and took four
                        // rescues and then the room with it.
                        //
                        // A real output stage slews THROUGH its linear
                        // region, so this does too: the opposite rail is
                        // reachable only by way of 0, which costs one extra
                        // Newton pass and turns the jump into two steps the
                        // integrator can take.
                        let new_region = if reg != 0 && want != 0 && want != reg {
                            0
                        } else {
                            want
                        };
                        let sh = D_PT_OA + 2 * k;
                        let st = &mut self.elems[ei].state;
                        if reg != new_region {
                            converged = false;
                            st.dstate = (st.dstate & !(3 << sh)) | (new_region << sh);
                            discrete_flip = true;
                        }
                    }
                }
                ElementKind::OpAmp { rail, isc } => {
                    let target = OPAMP_GAIN * (self.xv(node[0]) - self.xv(node[1]) + OPAMP_VOFF);
                    let vout = self.xv(node[2]);
                    // Current OUT of the output pin: the branch unknown is
                    // the current in.
                    let i_out = -self.elems[ei]
                        .branch
                        .map(|b| self.x[self.num_nodes + b])
                        .unwrap_or(0.0);
                    let over = isc * 1.000001;
                    let st = &mut self.elems[ei].state;
                    let new_region = match st.region {
                        0 => {
                            if node[2] > 0 && vout.abs() > rail * 1.000001 {
                                if vout > 0.0 {
                                    1
                                } else {
                                    -1
                                }
                            } else if i_out > over {
                                // The load wants more than the output stage
                                // can give even without saturating: fold
                                // back. This is what makes a follower into
                                // 10 Ω sag instead of delivering an amp.
                                2
                            } else if i_out < -over {
                                -2
                            } else {
                                0
                            }
                        }
                        r if r.abs() == 2 => {
                            let s = r.signum();
                            if (s as f64) * target < 0.0 {
                                // The amplifier now wants the other way.
                                // Relax to linear and let the next pass pick
                                // the region the load actually implies —
                                // jumping straight to the opposite limit
                                // would chatter forever in a follower whose
                                // load is only marginally too heavy.
                                0
                            } else if (s as f64) * vout > rail * 1.000001 {
                                // Pushing isc took the output past its own
                                // rail: the load is lighter than the limit
                                // after all, so it is the RAIL that binds.
                                s
                            } else {
                                r
                            }
                        }
                        r if (r as f64) * i_out > over => {
                            // Railed, and the load is dragging more than the
                            // output stage can supply. A real op-amp does not
                            // die here, it stops delivering: the output sags
                            // off the rail at constant current.
                            2 * r
                        }
                        r => {
                            // Any opposing drive flips DIRECTLY to the
                            // other rail: positive-feedback circuits leave
                            // a rail hard, and routing through the linear
                            // region would chatter (its solution sits back
                            // on the old rail) until NR gives up —
                            // Schmitt triggers and relaxation oscillators
                            // with slow RC vs dt hit this exactly at the
                            // threshold crossing. Only a weakening
                            // same-sign drive relaxes to linear (negative
                            // feedback coming out of saturation).
                            let drive = (r as f64) * target;
                            if drive >= rail {
                                r
                            } else if drive < 0.0 {
                                -r
                            } else {
                                0
                            }
                        }
                    };
                    if new_region != st.region {
                        // At most 2 region changes per NR pass. At the
                        // exact threshold crossing (within the offset
                        // window, microvolts wide) the railed and linear
                        // regions can point at each other indefinitely;
                        // holding the current region yields a consistent
                        // solve, and the next substep's capacitor motion
                        // resolves the ambiguity cleanly.
                        if st.lastv[0] < 2.0 {
                            st.lastv[0] += 1.0;
                            converged = false;
                            st.region = new_region;
                            discrete_flip = true;
                        }
                    }
                }
                ElementKind::Gate { .. }
                | ElementKind::FlipFlop { .. }
                | ElementKind::ShiftReg { .. }
                | ElementKind::Counter { .. }
                | ElementKind::Mux { .. } => {
                    // DELIBERATELY NOTHING, and this arm exists rather than
                    // falling into `_ => {}` below so that the emptiness is
                    // visibly intentional and survives someone "fixing" it.
                    //
                    // The logic family advances its discrete state once per
                    // ACCEPTED substep, in `accept`. That is what gives it a
                    // real one-substep propagation delay — a ring of
                    // inverters oscillates instead of sitting on its DC fixed
                    // point, and a cross-coupled pair settles instead of
                    // chattering. Doing it here would instead:
                    //   (a) let a chip see its own clock move under Newton's
                    //       feet within one substep, and self-trigger;
                    //   (b) be silently rolled back by the rescue path, which
                    //       snapshots `ElemState` before every solve;
                    //   (c) collapse a D-deep ripple into D factorizations
                    //       inside ONE substep instead of one factorization
                    //       in each of D substeps — same total work, piled
                    //       into a single 20 µs budget instead of spread.
                    //
                    // For the same reason these kinds are deliberately NOT
                    // added to the `lastv[0]` flip-budget reset in
                    // `solve_step`: they never flip during NR, so they need
                    // no budget. Adding them there would be harmless but
                    // misleading; omitting them is the statement.
                }
                _ => {}
            }
        }
        if discrete_flip {
            // Two consumers, one event, neither derived from the other: the
            // retained LU no longer describes the region set that is now in
            // force, and an island whose comparator just tripped is not
            // resting.
            self.factor_valid = false;
            self.discrete_moved = true;
        }
        converged
    }

    /// Commit device history and pin currents from the solved unknowns.
    fn accept(&mut self, h: f64, be: bool) {
        // (element slot, id, freshly sampled input). Collected rather than
        // applied inline because the chain lives in `self.delays` while the
        // loop below holds `self.elems` mutably — and because a shift is a
        // once-per-step event that has no business happening inside a
        // per-element borrow.
        let mut bbd_shifts: Vec<(usize, u32, f64)> = Vec::new();
        // The logic family's discrete state moves HERE rather than in
        // `update_guesses`, so it needs the `accept`-time analogue of
        // `discrete_flip`: a chip that changed state has changed the matrix,
        // and the next substep must not run against the retained LU. This
        // flag is the ONLY thing standing between the family and a stale-LU
        // correctness bug, because these parts are `is_nonlinear()` but
        // never flip during Newton.
        let mut logic_changed = false;
        // `0..self.active`, not `0..self.elems.len()`. The family was written
        // against the pre-island engine, which had no partition and so had to
        // carry a `broken` flag through this destructure and test it by hand.
        // An island parks wires, grounds, broken parts and all-pins-on-ground
        // parts BEHIND `active`, so an element reached here is live by
        // construction and the flag has nothing left to say.
        for ei in 0..self.active {
            let (kind, node, branch, share_n, share_sign) = {
                let e = &self.elems[ei];
                (e.spec.kind, e.node, e.branch, e.share_n, e.share_sign)
            };
            let v01 = self.xv(node[0]) - self.xv(node[1]);
            // The branch unknown, oriented and split for THIS member.
            //
            // Merged ideal sources share one row, so the solver produces one
            // total current and — this is the honest part — it CANNOT pick
            // the split: every division of the total satisfies the same
            // physics, because the merged constraint has a one-dimensional
            // null space in branch coordinates. The symmetric point is the
            // unique choice invariant under permuting the members, and
            // permutation is the only symmetry the situation has (it is also
            // what real supplies with matched internal impedance do). The
            // TOTAL — the number the energy meter and `source_watts` care
            // about — is exactly the solver's.
            //
            // For an unmerged element this is `× 1.0 / 1.0`: bit-exact
            // identity, so no existing document's state hash can move.
            let bi_val = branch.map(|b| self.x[self.num_nodes + b] * share_sign / f64::from(share_n));
            let mut vs = [0.0; MAX_PINS];
            for (k, v) in vs.iter_mut().enumerate() {
                *v = self.xv(node[k]);
            }
            let eid = self.elems[ei].spec.id;
            let st = &mut self.elems[ei].state;
            let mut two = |i: f64| {
                st.pin_i = [0.0; MAX_PINS];
                st.pin_i[0] = i;
                st.pin_i[1] = -i;
            };
            match kind {
                // A label stamps NOTHING. It merged its nodes back in `rebuild`,
                // which is the whole of what it does — same as a wire.
                ElementKind::Wire | ElementKind::Ground | ElementKind::Label => {}
                ElementKind::Resistor { ohms }
                | ElementKind::Lamp { ohms, .. }
                | ElementKind::Speaker { ohms } => two(v01 / ohms),
                ElementKind::Photocell {
                    r_dark,
                    r_lit,
                    light,
                } => two(v01 / crate::photocell_ohms(r_dark, r_lit, light)),
                ElementKind::Potentiometer { ohms, wiper } => {
                    let r1 = (ohms * wiper).max(1e-3);
                    let r2 = (ohms * (1.0 - wiper)).max(1e-3);
                    let ia = (vs[0] - vs[1]) / r1;
                    let ib = (vs[2] - vs[1]) / r2;
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = ia;
                    st.pin_i[1] = -(ia + ib);
                    st.pin_i[2] = ib;
                }
                ElementKind::Capacitor { farads } => {
                    let geq = if be { farads / h } else { 2.0 * farads / h };
                    let i = if be {
                        geq * (v01 - st.v_prev)
                    } else {
                        geq * (v01 - st.v_prev) - st.i_prev
                    };
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::Inductor { henries } => {
                    let geq = if be { h / henries } else { h / (2.0 * henries) };
                    let i = if be {
                        st.i_prev + geq * v01
                    } else {
                        st.i_prev + geq * (v01 + st.v_prev)
                    };
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::CurrentSource { amps } => two(amps),
                // Current into pin 0 across the internal resistance: zero
                // on open circuit, -v_emf/R into a short. (`v_prev` is this
                // step's held EMF, drawn in `solve_step`'s pre-pass.)
                ElementKind::Noise { ohms, .. } => two((v01 - st.v_prev) / ohms),
                ElementKind::VoltageSource { .. } => two(bi_val.unwrap_or(0.0)),
                ElementKind::Rail { .. } => {
                    // One real pin: its current is the branch unknown; the
                    // return leg lives in ground and has no pin to report.
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = bi_val.unwrap_or(0.0);
                }
                ElementKind::Pt2399 => {
                    let vgnd = vs[3];
                    // What the player's resistor is drawing, from the solver's
                    // own answer for the RT node. Zero when nothing is wired
                    // there, which is the honest reading of an open pin.
                    let i_rt = ((crate::PT_V_RT - (vs[2] - vgnd)) / crate::PT_R_RT).max(0.0);
                    // The chip's own oscillator: a phase accumulator, so
                    // nothing has to be wired to a clock pin. Clamped at one
                    // shift per substep — the engine cannot move a sample
                    // faster than it steps, and pretending otherwise would
                    // silently shorten the delay instead of refusing it.
                    let f = (crate::PT_HZ_PER_AMP * i_rt).clamp(0.0, 1.0 / h);
                    st.vg1 += f * h;
                    if st.vg1 >= 1.0 {
                        st.vg1 -= 1.0;
                        bbd_shifts.push((ei, eid, vs[0] - vgnd));
                    }
                    // The clamp regions are NOT decided here — see
                    // `update_guesses`. Deciding them after the solve made
                    // them one step stale, and with a gain of 1e4 that is not
                    // a small error: the stage flip-flopped between its rails
                    // every step and delivered the same railed output whatever
                    // went in. All that is left here is reporting current.
                    // What each stage delivers into whatever hangs on it.
                    let mut oa_i = [0.0f64; 4];
                    for (k, (inv, out)) in
                        [(4usize, 5usize), (6, 7), (8, 9), (10, 11)].into_iter().enumerate()
                    {
                        let want = crate::PT_V_RT
                            + (crate::PT_V_RT - (vs[inv] - vgnd)) * crate::PT_OA_GM
                                / crate::PT_OA_GOUT;
                        oa_i[k] = (want - (vs[out] - vgnd)) * crate::PT_OA_GOUT;
                    }
                    let i_out = (st.v_prev - (vs[1] - vgnd)) * PT_G_OUT;
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[1] = i_out;
                    st.pin_i[2] = i_rt;
                    st.pin_i[5] = oa_i[0];
                    st.pin_i[7] = oa_i[1];
                    st.pin_i[9] = oa_i[2];
                    st.pin_i[11] = oa_i[3];
                    // Everything the chip sources leaves through its own
                    // ground pin, or KCL stops meaning anything.
                    st.pin_i[3] = -(i_out + i_rt + oa_i.iter().sum::<f64>());
                }
                ElementKind::Bbd { .. } => {
                    // Pins [IN, OUT, CLK, GND]. Runs ONCE PER ACCEPTED STEP,
                    // which is the whole reason it is here and not in the
                    // stamp: a shift is a state change, and Newton may visit
                    // a stamp several times for one step in a room that also
                    // contains a diode.
                    let i = bi_val.unwrap_or(0.0);
                    let vgnd = vs[3];
                    // Schmitt on the clock, so one slow or noisy edge is one
                    // transition and not a burst.
                    let hi = if dbit(st.dstate, D_CLK_PREV) {
                        vs[2] - vgnd > BBD_CLK_LO
                    } else {
                        vs[2] - vgnd > BBD_CLK_HI
                    };
                    if hi != dbit(st.dstate, D_CLK_PREV) {
                        // EVERY transition moves one stage, so a full cycle
                        // moves two — which is what a real two-phase device
                        // does, and is what makes the delay the datasheet's
                        //     t = stages / (2 * f_clock).
                        bbd_shifts.push((ei, eid, vs[0] - vgnd));
                        dset(&mut st.dstate, D_CLK_PREV, hi);
                    }
                    // Current flows at the OUT pin (sourced from GND); the
                    // two high-impedance inputs carry ~nothing worth naming.
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[1] = i;
                    st.pin_i[3] = -i;
                }
                ElementKind::Motor { .. } => {
                    // The armature current is the branch unknown; it is also
                    // the inductive history for the next step (same slot the
                    // plain inductor uses).
                    let i = bi_val.unwrap_or(0.0);
                    st.v_prev = v01;
                    st.i_prev = i;
                    two(i);
                }
                ElementKind::Switch { closed } | ElementKind::Button { closed } => {
                    two(if closed { bi_val.unwrap_or(0.0) } else { 0.0 })
                }
                ElementKind::Diode | ElementKind::Led { .. } | ElementKind::Zener { .. } => {
                    let (is, nvt, voff) = diode_params(&kind);
                    let mut i = is * (libm::exp(v01 / nvt) - 1.0);
                    if let Some(voff) = voff {
                        i -= is * libm::exp(-(v01 + voff) / nvt);
                    }
                    st.vg1 = v01;
                    two(i);
                }
                ElementKind::Npn { beta } | ElementKind::Pnp { beta } => {
                    let pol = if matches!(kind, ElementKind::Npn { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let vbe = pol * (vs[0] - vs[2]);
                    let vbc = pol * (vs[0] - vs[1]);
                    let i_f = BJT_IS * (libm::exp(vbe / VT) - 1.0);
                    let i_r = BJT_IS * (libm::exp(vbc / VT) - 1.0);
                    let ic = i_f - i_r * (1.0 + 1.0 / BJT_BETA_R);
                    let ib = i_f / beta + i_r / BJT_BETA_R;
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[0] = pol * ib;
                    st.pin_i[1] = pol * ic;
                    st.pin_i[2] = -pol * (ib + ic);
                    st.vg1 = vbe;
                    st.vg2 = vbc;
                }
                ElementKind::Nmos { vt, k } | ElementKind::Pmos { vt, k } => {
                    let pol = if matches!(kind, ElementKind::Nmos { .. }) {
                        1.0
                    } else {
                        -1.0
                    };
                    let m = mos_eval(pol, vt, k, &vs);
                    let id = pol * m.id;
                    st.pin_i = [0.0; MAX_PINS];
                    // Current enters the effective drain, leaves the source.
                    st.pin_i[m.d_index] = id;
                    st.pin_i[m.s_index] = -id;
                }
                ElementKind::OpAmp { .. } => {
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[2] = bi_val.unwrap_or(0.0);
                }
                ElementKind::Timer555 => {
                    st.pin_i = [0.0; MAX_PINS];
                    // Quiescent rail current.
                    let iq = (vs[0] - vs[1]) * T555_G_QUIESCENT;
                    st.pin_i[0] = iq;
                    st.pin_i[1] = -iq;
                    // Discharge transistor (only conducting when low).
                    if st.region == 0 {
                        let idis = (vs[5] - vs[1]) * T555_G_DIS;
                        st.pin_i[5] = idis;
                        st.pin_i[1] -= idis;
                    }
                    // Output branch: sourced from VCC when high, sunk into
                    // GND when low.
                    let io = bi_val.unwrap_or(0.0);
                    st.pin_i[4] = io;
                    st.pin_i[if st.region != 0 { 0 } else { 1 }] -= io;
                }
                ElementKind::Ota => {
                    let eb = libm::exp(vs[3] / VT);
                    let iabc = (OTA_IS * (eb - 1.0)).max(0.0);
                    let iout = iabc * libm::tanh((vs[0] - vs[1]) / (2.0 * VT));
                    st.pin_i = [0.0; MAX_PINS];
                    st.pin_i[2] = -iout;
                    st.pin_i[3] = OTA_IS * (eb - 1.0);
                    st.vg1 = vs[0] - vs[1];
                    st.vg2 = vs[3];
                }
                ElementKind::Gate { .. }
                | ElementKind::FlipFlop { .. }
                | ElementKind::ShiftReg { .. }
                | ElementKind::Counter { .. }
                | ElementKind::Mux { .. } => {
                    let Some(lp) = kind.logic_pins() else { continue };
                    let npins = kind.pin_count();
                    let (vv, vg) = (vs[0], vs[1]);
                    let vcc_v = vv - vg;
                    let d0 = st.dstate;
                    let mut d = d0;

                    // 1. Latch-up, before anything else: a latched chip is a
                    //    short, not a gate, and it stops functioning.
                    if dbit(d, D_LATCHED) {
                        // Sticky until the supply goes away — a power cycle,
                        // which is exactly what clears real latch-up.
                        if vcc_v < LOGIC_V_UNLATCH {
                            dset(&mut d, D_LATCHED, false);
                        }
                    } else {
                        let mut trip = vcc_v > LOGIC_V_ABSMAX;
                        for p in 0..npins {
                            if vs[p] > vv + LOGIC_V_LATCH_MARGIN
                                || vs[p] < vg - LOGIC_V_LATCH_MARGIN
                            {
                                trip = true;
                            }
                        }
                        if trip {
                            dset(&mut d, D_LATCHED, true);
                        }
                    }

                    // 2. Input Schmitt latches, thresholds on the LIVE
                    //    supply. A pin between the two thresholds HOLDS,
                    //    which is what makes a floating input (parked at
                    //    vcc/2 by the symmetric leak) deterministic rather
                    //    than chattering.
                    let th_hi = vg + vcc_v * LOGIC_TH_HI;
                    let th_lo = vg + vcc_v * LOGIC_TH_LO;
                    for k in 0..lp.n_in {
                        let v = vs[lp.in0 + k];
                        let cur = dbit(d, D_SCHMITT + k);
                        let new = if v > th_hi {
                            true
                        } else if v < th_lo {
                            false
                        } else {
                            cur
                        };
                        dset(&mut d, D_SCHMITT + k, new);
                    }

                    // 3. The state machine, from the NEW Schmitt bits.
                    if !dbit(d, D_LATCHED) {
                        logic_eval(&kind, &lp, &mut d);
                    }

                    // 4. Clock history LAST, so an edge is exactly one
                    //    substep wide and the state machine above saw the
                    //    PREVIOUS accepted level to compare against.
                    if let Some(c) = lp.clk {
                        let level = dbit(d, D_SCHMITT + c);
                        dset(&mut d, D_CLK_PREV, level);
                    }

                    // 5. Pin currents, from the same conductances `build`
                    //    stamped. Every internal branch contributes ±, so
                    //    Σ i = 0 exactly and Σ v·i = Σ g·(Δv)² ≥ 0: honest
                    //    dissipation by construction, with no `elem_power`
                    //    exception needed.
                    let mut pi = [0.0f64; MAX_PINS];
                    {
                        // REPORT THE STATE THAT WAS SOLVED, NOT THE ONE JUST
                        // DECIDED. `vs` came out of a solve stamped with `d0`;
                        // `logic_eval` above may already have advanced `d` for
                        // the NEXT substep. Pairing the new conductances with
                        // the old voltages invents current out of nothing: on a
                        // transition substep an output reported 0.500 W where
                        // the settled value is 0.001 W, the frame KCL error at
                        // the driven node reached 0.1 A, and the damage model —
                        // which judges parts from exactly these numbers —
                        // destroyed a 3-inverter ring whose supply was
                        // delivering 765 µW to the entire room.
                        //
                        // `pin_i` is reporting only and is read after the solve,
                        // so this cannot change any answer: no golden digest
                        // moves. The next substep stamps `d` and reports `d`.
                        let d = d0;
                        let mut add = |a: usize, b: usize, g: f64| {
                            let i = (vs[a] - vs[b]) * g;
                            pi[a] += i;
                            pi[b] -= i;
                        };
                        add(0, 1, LOGIC_G_QUIESCENT);
                        if dbit(d, D_LATCHED) {
                            add(0, 1, LOGIC_G_LATCHUP);
                        }
                        for k in 0..lp.n_in {
                            add(lp.in0 + k, 0, LOGIC_G_IN);
                            add(lp.in0 + k, 1, LOGIC_G_IN);
                        }
                        for k in 0..lp.n_out {
                            let hi = dbit(d, D_DATA + k);
                            add(lp.out0 + k, 0, if hi { LOGIC_G_ON } else { LOGIC_G_OFF });
                            add(lp.out0 + k, 1, if hi { LOGIC_G_OFF } else { LOGIC_G_ON });
                        }
                        if let ElementKind::Mux { .. } = kind {
                            let chans = 1usize << lp.n_in;
                            let y = lp.in0 + lp.n_in;
                            let on = dfield(d, D_DATA, lp.n_in) as usize;
                            for j in 0..chans {
                                add(2 + j, y, if j == on { LOGIC_G_ON } else { LOGIC_G_OFF });
                            }
                        }
                    }
                    st.pin_i = pi;
                    if d != d0 {
                        st.dstate = d;
                        logic_changed = true;
                    }
                }
            }
        }
        if logic_changed {
            self.factor_valid = false;
            // A logic edge is a DISCONTINUITY, in exactly the sense a switch
            // flip is, and it gets the same treatment: a couple of backward-
            // Euler substeps to kill trapezoidal ringing.
            //
            // This is not decoration. A gate output is a hard step between
            // the rails behind 50 Ω, and a player WILL hang a capacitor on
            // it — the sequencer's glide cap is exactly that. Measured on the
            // shift-register ring at a sequencer clock rate with 100 nF on
            // Q0, trapezoid rings the output to +5.53 V and -0.55 V on a 5 V
            // rail: over half a volt outside the chip's own supply, on the
            // target circuit. That is wrong on its face, and here it is also
            // dangerous, because a pin driven a volt outside the rails is
            // what fires the latch-up model — the integrator would have been
            // destroying chips that nothing was wrong with. With BE armed
            // the same node stays inside 0..5 V.
            //
            // The cost is bounded by construction: at a sequencer clock the
            // edges are tens of substeps apart, so this is 2 substeps in 50.
            // A circuit switching every substep runs entirely in BE, which
            // is the correct integrator for it anyway.
            self.be_steps = self.be_steps.max(BE_STEPS_AFTER_EVENT);
        }

        // Bucket chains move last, once the loop has released `self.elems`.
        //
        // The sample that falls out becomes the OUTPUT the next stamp holds,
        // and it lands in `v_prev` — which is why the chain itself never has
        // to be in the state digest for the output to be reproducible, and
        // why the rescue ladder already snapshots the thing that matters.
        for (ei, id, sample) in bbd_shifts.drain(..) {
            let Some(line) = self.delays.get_mut(&id) else {
                continue;
            };
            let out = line.shift(sample);
            self.elems[ei].state.v_prev = out;
            // A shifted chain has moved the RHS, so the next substep must
            // re-solve rather than coast on a retained factorization. The
            // matrix itself is untouched (this is an RHS-only device, like
            // the motor), so this does NOT force a refactorization.
            self.discrete_moved = true;
        }
    }

    /// Deterministic digest of this island's state, for a caller checking
    /// that a parallel run matches a serial one island by island.
    pub fn state_hash(&self) -> u64 {
        use xxhash_rust::xxh3::Xxh3;
        let mut h = Xxh3::new();
        let mut put = |x: f64| h.update(&sim_math::canon(x).to_bits().to_le_bytes());
        for v in &self.x {
            put(*v);
        }
        for e in &self.elems {
            put(e.state.v_prev);
            put(e.state.i_prev);
            put(e.state.vg1);
            put(e.state.vg2);
            put(e.state.region as f64);
            // Only the pins the part HAS. The slots past `pin_count()` are
            // always zero, so hashing them said nothing — but it made every
            // golden digest a function of `MAX_PINS`, which meant raising the
            // ceiling for a 9-pin shift register would move all 18 of them and
            // destroy their provenance as a comparison against the pre-island
            // engine. Scoping the loop decouples the two permanently: this is
            // the LAST time a ceiling change moves a hash.
            let np = e.spec.kind.pin_count();
            for p in 0..np {
                put(e.state.lastv[p]);
                put(e.state.pin_i[p]);
            }
            // The bucket chain. A delay line whose CONTENTS differ is a
            // different machine even when every terminal voltage currently
            // agrees: the divergence surfaces seconds later, when those
            // samples reach the output, which is exactly the kind of drift
            // the cross-target harness exists to catch. O(stages), and only
            // ever called by the harness — never in a tick.
            if let Some(line) = self.delays.get(&e.spec.id) {
                put(line.w as f64);
                for v in &line.buf {
                    put(*v);
                }
            }
            if matches!(e.spec.kind, ElementKind::Noise { .. }) {
                put((e.state.noise_n >> 32) as u32 as f64);
                put((e.state.noise_n & 0xffff_ffff) as u32 as f64);
            }
        }
        h.digest()
    }
}

impl Engine {
    // ---------------------------------------------------------- read-out

    /// Voltage at an island-local node from the last solve.
    pub fn node_voltage(&self, island: usize, node: usize) -> f64 {
        self.islands.get(island).map(|i| i.xv(node)).unwrap_or(0.0)
    }

    /// GLOBAL node index at a geometric point, if that point is a junction of
    /// the compiled document. None = nothing connects there.
    ///
    /// The same lookup `voltage_at` does, stopping one step earlier. It exists
    /// so a caller can ask "are these two places the same net?" — which is an
    /// integer compare on the answer — WITHOUT a second union-find over the
    /// document, and without node numbers leaving as anything but an opaque
    /// token. They are not stable identities: they are a function of document
    /// order, so an answer is only good for the compile that produced it. The
    /// server re-derives per tick for exactly that reason.
    ///
    /// Read-only. Nothing here stamps, integrates or touches state, so it
    /// cannot move a state hash.
    pub fn node_at(&self, p: Point) -> Option<usize> {
        let base = self.node_base();
        self.junctions
            .iter()
            .find(|(q, _, _)| *q == p)
            .map(|(_, isl, node)| Self::global_node(&base, *isl, *node))
    }

    /// GLOBAL node index of one pin of an element. Same namespace as
    /// [`Engine::node_at`], so the two compare directly.
    pub fn pin_node(&self, id: u32, pin: usize) -> Option<usize> {
        let (isl, e) = self.find(id)?;
        if pin >= e.spec.pins.len() {
            return None;
        }
        Some(Self::global_node(&self.node_base(), isl, e.node[pin]))
    }

    /// Voltage at a geometric point, if it is a junction.
    pub fn voltage_at(&self, p: Point) -> Option<f64> {
        self.junctions
            .iter()
            .find(|(q, _, _)| *q == p)
            .map(|(_, isl, node)| self.islands[*isl].xv(*node))
    }

    /// Voltage at one pin of an element, from the last solve.
    pub fn pin_voltage(&self, id: u32, pin: usize) -> Option<f64> {
        let (isl, e) = self.find(id)?;
        if pin >= e.spec.pins.len() {
            return None;
        }
        Some(self.islands[isl].xv(e.node[pin]))
    }

    /// Resolve an element id to a handle for repeated sampling. `pin_voltage`
    /// scans the document per call, which is fine once a tick and ruinous at
    /// audio rates (hundreds of samples per tick per tap), so a high-rate
    /// sampler resolves once and then reads through the handle.
    ///
    /// The handle is INVALIDATED by `set_elements` — resolve it again after
    /// any document edit. A stale handle reads 0, it never panics.
    pub fn tap(&self, id: u32) -> Option<ElemTap> {
        let &(_, island, slot) = self.order.iter().find(|(eid, _, _)| *eid == id)?;
        Some(ElemTap { island, slot })
    }

    /// `v(pin a) - v(pin b)` at a tap, from the last accepted step, in O(1).
    /// This is the quantity a voltage-driven device follows: the drive across
    /// a loudspeaker's voice coil is exactly its terminal difference.
    /// Out-of-range slots/pins read 0 so a tap on a deleted element goes
    /// silent instead of panicking.
    pub fn tap_delta(&self, t: ElemTap, a: usize, b: usize) -> f64 {
        let Some(island) = self.islands.get(t.island) else {
            return 0.0;
        };
        let Some(e) = island.elems.get(t.slot) else {
            return 0.0;
        };
        let n = e.spec.pins.len();
        let va = if a < n { island.xv(e.node[a]) } else { 0.0 };
        let vb = if b < n { island.xv(e.node[b]) } else { 0.0 };
        va - vb
    }

    /// The element id a tap currently points at, for callers that want to
    /// confirm a handle still means what they resolved it from.
    pub fn tap_id(&self, t: ElemTap) -> Option<u32> {
        self.islands
            .get(t.island)
            .and_then(|i| i.elems.get(t.slot))
            .map(|e| e.spec.id)
    }

    /// Current into one pin of an element, from the last accepted step.
    /// NOTE: wires get their current from KCL propagation, which only runs
    /// in `frame()` — for a wire probe, sample via `frame()` instead.
    pub fn pin_current(&self, id: u32, pin: usize) -> Option<f64> {
        let (_, e) = self.find(id)?;
        if pin >= e.spec.pins.len() {
            return None;
        }
        Some(e.state.pin_i[pin])
    }

    pub fn is_wire(&self, id: u32) -> bool {
        self.find(id)
            .map(|(_, e)| matches!(e.spec.kind, ElementKind::Wire))
            .unwrap_or(false)
    }

    /// Per-element render frame, in document order. Wire currents are
    /// recovered by KCL propagation over junctions (wires are node-merged so
    /// they have no unknown of their own).
    pub fn frame(&self) -> Vec<ElemFrame> {
        let mut out: Vec<ElemFrame> = self
            .doc()
            .map(|(isl, e)| {
                let npins = e.spec.pins.len();
                let mut v = [0.0; MAX_PINS];
                for (i, val) in v.iter_mut().enumerate().take(npins) {
                    *val = self.islands[isl].xv(e.node[i]);
                }
                let i = e.state.pin_i;
                let power = elem_power(&e.spec.kind, npins, &v, &i);
                ElemFrame {
                    id: e.spec.id,
                    npins,
                    v,
                    i,
                    power,
                    quarantined: self.islands[isl].quarantined,
                }
            })
            .collect();
        self.solve_wire_currents(&mut out);
        out
    }

    /// KCL relaxation: a wire whose endpoint junction has exactly one
    /// unknown incident current gets solved by that junction's balance.
    /// Pure-wire loops are ambiguous and settle at 0 (harmless: dots just
    /// don't move there).
    fn solve_wire_currents(&self, frames: &mut [ElemFrame]) {
        // Point -> junction index. `BTreeMap`, not a hash map, for the same
        // reason `rebuild` uses one: no hasher seed, no platform RNG. Only
        // lookups happen through it, so it is a pure speed-up over the linear
        // `position()` scan it replaces.
        let mut jix: BTreeMap<Point, usize> = BTreeMap::new();
        for (j, (p, _, _)) in self.junctions.iter().enumerate() {
            jix.insert(*p, j);
        }
        let kinds: Vec<ElementKind> = self.doc().map(|(_, e)| e.spec.kind).collect();
        let mut incident: Vec<Vec<(usize, usize)>> = vec![Vec::new(); self.junctions.len()];
        for (i, (_, e)) in self.doc().enumerate() {
            for (t, p) in e.spec.pins.iter().enumerate() {
                if let Some(j) = jix.get(p) {
                    incident[*j].push((i, t));
                }
            }
        }
        let is_wire = |i: usize| matches!(kinds[i], ElementKind::Wire);
        let mut known: Vec<bool> = (0..kinds.len()).map(|i| !is_wire(i)).collect();
        loop {
            let mut progressed = false;
            for inc in incident.iter() {
                // Grounds can sink arbitrary current; their junctions are
                // not solvable by balance.
                if inc
                    .iter()
                    .any(|(i, _)| matches!(kinds[*i], ElementKind::Ground))
                {
                    continue;
                }
                let unknowns: Vec<&(usize, usize)> =
                    inc.iter().filter(|(i, _)| !known[*i]).collect();
                if unknowns.len() != 1 {
                    continue;
                }
                let &&(wi, wt) = unknowns.first().unwrap();
                // Total current flowing from this junction into all known
                // elements; the unknown wire pin must supply it.
                let mut into_known = 0.0;
                for &(i, t) in inc {
                    if i != wi && known[i] {
                        into_known += frames[i].i[t];
                    }
                }
                let pin_current = -into_known;
                frames[wi].i[wt] = pin_current;
                frames[wi].i[1 - wt] = -pin_current;
                known[wi] = true;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }

    /// Deterministic digest of all continuous + discrete state; the S1
    /// cross-target harness asserts these match bit-for-bit.
    /// The bucket chain belonging to an element, wherever its island is.
    /// Linear in island count and used only by the digest.
    fn delay_of(&self, id: u32) -> Option<&DelayLine> {
        self.islands.iter().find_map(|i| i.delays.get(&id))
    }

    pub fn state_hash(&self) -> u64 {
        use xxhash_rust::xxh3::Xxh3;
        let mut h = Xxh3::new();
        let mut put = |x: f64| h.update(&sim_math::canon(x).to_bits().to_le_bytes());
        put(self.time);
        for island in &self.islands {
            for v in &island.x {
                put(*v);
            }
        }
        for (_, e) in self.doc() {
            put(e.state.v_prev);
            put(e.state.i_prev);
            put(e.state.vg1);
            put(e.state.vg2);
            put(e.state.region as f64);
            // Only the pins the part HAS. The slots past `pin_count()` are
            // structurally dead — nothing ever writes them, every device
            // clears the whole array before filling its own prefix — so
            // hashing them fed a run of guaranteed zeros into the digest and
            // made every golden hash a function of MAX_PINS. Scoping the
            // loop moves every hash exactly ONCE and then decouples the
            // digest from the ceiling permanently, which is what lets the
            // logic family raise MAX_PINS without touching provenance again.
            let np = e.spec.kind.pin_count();
            for p in 0..np {
                put(e.state.lastv[p]);
                put(e.state.pin_i[p]);
            }
            // Broken parts are discrete state and must reach the digest — but
            // only when one EXISTS. An unconditional field would feed one more
            // f64 per element into the hash and change every golden hash in
            // the S1 cross-target harness, for a feature no golden circuit
            // uses. Nothing broken => not one byte differs.
            if e.broken {
                put(e.spec.id as f64);
            }
            // A noise source's stream POSITION is discrete state: two
            // engines can agree on every voltage in the circuit and still
            // diverge on the very next step if they disagree about where
            // they are in the sequence, and the cross-target harness would
            // never see it. Conditional for the same reason `broken` is —
            // a world with no noise source hashes exactly as it did before
            // this device existed, so no golden hash moved.
            // The bucket chain. A delay line whose CONTENTS differ is a
            // different machine even when every terminal voltage currently
            // agrees: the divergence surfaces seconds later, when those
            // samples reach the output, which is exactly the kind of drift
            // the cross-target harness exists to catch. O(stages), and only
            // ever called by the harness — never in a tick.
            if let Some(line) = self.delay_of(e.spec.id) {
                put(line.w as f64);
                for v in &line.buf {
                    put(*v);
                }
            }
            if matches!(e.spec.kind, ElementKind::Noise { .. }) {
                // Two exact halves: every u32 is exactly representable in
                // f64, so this is a lossless view of the counter through
                // the f64-shaped `put`.
                put((e.state.noise_n >> 32) as u32 as f64);
                put((e.state.noise_n & 0xffff_ffff) as u32 as f64);
            }
            // A logic chip's whole memory — its stored bits, its input
            // hysteresis latches, its clock history and whether it has
            // latched up — is `dstate`, and two engines agreeing on every
            // voltage can still disagree about the next edge if they
            // disagree about it. Conditional for the same reason `broken`
            // and `noise_n` are: a world with no logic part hashes exactly
            // as it did before this family existed, so no golden digest
            // moved when it landed. Every u32 is exact in f64.
            if e.spec.kind.is_logic() {
                put(f64::from(e.state.dstate));
            }
        }
        h.digest()
    }
}

/// Union-find with path halving, over a parent array.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// What a part DISSIPATES this instant, in watts.
///
/// For every part whose terminals are all modelled this is just `Σ v·i` over
/// the pins — conservation does the rest, and a part that is delivering
/// power reads negative.
///
/// The op-amp is the one exception, and it is an exception about honesty
/// rather than convenience. Its model has no supply terminals: the return
/// current for whatever the output drives vanishes into node 0, so `Σ v·i`
/// is the power it DELIVERS to the load, not the power it burns. Worse, the
/// number is not even recoverable from the sum — a railed output sits at
/// exactly ±rail, so the output transistor's own drop is identically zero
/// there. What a real output stage burns is the difference between the
/// supply it works against and the pin it lands on, times the current it
/// passes, and that IS computable from solved quantities:
///
/// ```text
///   P = |i_out| · (rail - sign(i_out)·vout)
/// ```
///
/// sourcing 25 mA into a dead short on a ±5 V part = 0.125 W (hot, and
/// survivable forever, which is why shorting an op-amp output does not
/// destroy it); the same short on a ±100 V part is 2.5 W and kills it.
/// Quiescent supply current is deliberately NOT added: the model does not
/// draw it from any node, so charging the player for it would be inventing
/// a number no solver produced.
fn elem_power(kind: &ElementKind, npins: usize, v: &[f64; MAX_PINS], i: &[f64; MAX_PINS]) -> f64 {
    if let ElementKind::OpAmp { rail, .. } = kind {
        let i_out = -i[2];
        let drop = rail - if i_out >= 0.0 { v[2] } else { -v[2] };
        return i_out.abs() * drop.max(0.0);
    }
    (0..npins).map(|p| v[p] * i[p]).sum()
}

/// Advance one logic chip's data bits from its (already updated) input
/// Schmitt bits and its previous clock level.
///
/// Called once per ACCEPTED substep and nowhere else, which is the whole of
/// the family's timing model: every logic element is a one-substep delay, so
/// a signal driven by logic is piecewise-constant across a whole substep and
/// setup/hold between two logic signals is structurally satisfied. It also
/// means an edge is quantized to the 20 µs substep grid, capping the honest
/// clock rate at 1/(2·dt) = 25 kHz — a sequencer clock in the low kHz has
/// 20x margin, and past the cap the edge count silently aliases, which is
/// why the client should count solver edges rather than trust the clock.
///
/// Reads and writes only `d`. No solved voltages, no time, no history.
fn logic_eval(kind: &ElementKind, lp: &LogicPins, d: &mut u32) {
    match *kind {
        ElementKind::Gate { op, .. } => {
            let ins = lp.n_in;
            let n_hi = (0..ins).filter(|k| dbit(*d, D_SCHMITT + k)).count();
            dset(d, D_DATA, op.eval(ins, n_hi));
        }
        // [CLK, D, RST] -> [Q, /Q]. RST is asynchronous and active low.
        ElementKind::FlipFlop { edge } => {
            let clk = dbit(*d, D_SCHMITT);
            let din = dbit(*d, D_SCHMITT + 1);
            let rst = dbit(*d, D_SCHMITT + 2);
            let held = dbit(*d, D_DATA);
            let q = if !rst {
                false
            } else if edge {
                // Rising edge = high now, low at the last accepted substep.
                // Comparing against the PREVIOUS ACCEPTED level (rather than
                // anything Newton saw) is what makes this a real edge
                // detector that also rewinds correctly on a rescue.
                if clk && !dbit(*d, D_CLK_PREV) {
                    din
                } else {
                    held
                }
            } else if clk {
                din // transparent while the clock is high
            } else {
                held
            };
            dset(d, D_DATA, q);
            dset(d, D_DATA + 1, !q);
        }
        // [CLK, SER, RST] -> Q0..Q(bits-1). Every stage moves from ONE edge,
        // here, in one pass: no internal ripple, and therefore one
        // factorization per clock edge instead of `bits` of them.
        ElementKind::ShiftReg { .. } => {
            let bits = lp.n_out;
            let clk = dbit(*d, D_SCHMITT);
            let ser = dbit(*d, D_SCHMITT + 1);
            let rst = dbit(*d, D_SCHMITT + 2);
            if !rst {
                dset_field(d, D_DATA, bits, 0);
            } else if clk && !dbit(*d, D_CLK_PREV) {
                let cur = dfield(*d, D_DATA, bits);
                dset_field(d, D_DATA, bits, (cur << 1) | u32::from(ser));
            }
        }
        // [CLK, RST] -> Q0..Q(bits-1), synchronous: all bits from one edge.
        ElementKind::Counter { modulus, .. } => {
            let bits = lp.n_out;
            let clk = dbit(*d, D_SCHMITT);
            let rst = dbit(*d, D_SCHMITT + 1);
            // Clamped rather than trusted: `check_kind` refuses a bad
            // modulus, but this must stay total for a document written by a
            // build that allowed one.
            let m = u32::from(modulus).clamp(2, 1 << bits);
            if !rst {
                dset_field(d, D_DATA, bits, 0);
            } else if clk && !dbit(*d, D_CLK_PREV) {
                let cur = dfield(*d, D_DATA, bits);
                let next = if cur + 1 >= m { 0 } else { cur + 1 };
                dset_field(d, D_DATA, bits, next);
            }
        }
        // The select lines decode straight into the data field, which is
        // what `build` reads to pick the conducting channel.
        ElementKind::Mux { .. } => {
            let sel = lp.n_in;
            let mut v = 0u32;
            for k in 0..sel {
                if dbit(*d, D_SCHMITT + k) {
                    v |= 1 << k;
                }
            }
            dset_field(d, D_DATA, sel, v);
        }
        _ => {}
    }
}

/// (saturation current, n·Vt, reverse-breakdown offset).
fn diode_params(kind: &ElementKind) -> (f64, f64, Option<f64>) {
    match kind {
        ElementKind::Led { .. } => (LED_IS, LED_NVT, None),
        ElementKind::Zener { vz } => {
            // Offset the reverse exponent so |i| = 5 mA exactly at -vz.
            let knee = ZENER_NVT * libm::log(0.005 / ZENER_IS);
            (ZENER_IS, ZENER_NVT, Some(vz - knee))
        }
        _ => (DIODE_IS, DIODE_NVT, None),
    }
}

/// Level-1 MOSFET evaluation on polarity-normalized, drain/source-swapped
/// voltages. `id` flows into the effective drain.
struct MosOp {
    id: f64,
    gm: f64,
    gds: f64,
    vgs: f64,
    vds: f64,
    d_index: usize,
    s_index: usize,
}

impl MosOp {
    fn d_pin(&self, node: [usize; MAX_PINS]) -> usize {
        node[self.d_index]
    }
    fn s_pin(&self, node: [usize; MAX_PINS]) -> usize {
        node[self.s_index]
    }
}

fn mos_eval(pol: f64, vt: f64, k: f64, v: &[f64]) -> MosOp {
    let vg = pol * v[0];
    let (vd, vs) = (pol * v[1], pol * v[2]);
    // The terminal at lower (normalized) potential acts as the source.
    let (d_index, s_index, vdn, vsn) = if vd >= vs {
        (1, 2, vd, vs)
    } else {
        (2, 1, vs, vd)
    };
    let vgs = vg - vsn;
    let vds = vdn - vsn;
    let vgst = vgs - vt;
    let (id, gm, gds) = if vgst <= 0.0 {
        (0.0, 0.0, MOS_LEAK)
    } else if vds < vgst {
        (
            k * (vgst * vds - vds * vds * 0.5),
            k * vds,
            k * (vgst - vds) + MOS_LEAK,
        )
    } else {
        (0.5 * k * vgst * vgst, k * vgst, MOS_LEAK)
    };
    // Drain-source avalanche. `vds` is normalized non-negative (the higher
    // terminal is always the effective drain), so this is a one-sided
    // exponential. Below the guard the term underflows to nothing anyway;
    // making it structurally zero is what keeps every sub-60 V circuit
    // bit-identical to before the clamp existed.
    let (i_av, g_av) = if vds > MOS_BV - MOS_BV_MARGIN {
        let i = MOS_BV_IS * libm::exp((vds - MOS_BV) / MOS_BV_NVT);
        (i, i / MOS_BV_NVT)
    } else {
        (0.0, 0.0)
    };
    MosOp {
        id: id + MOS_LEAK * vds + i_av,
        gm,
        gds: gds + g_av,
        vgs,
        vds,
        d_index,
        s_index,
    }
}

/// SPICE-style junction voltage limiting: keeps NR from exponent overflow
/// by pulling large forward-bias steps back onto the exponential.
fn pnjlim(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > vt + vt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                vold + vt * libm::log(arg)
            } else {
                vcrit
            }
        } else {
            vt * libm::log(vnew.max(vt) / vt)
        }
    } else {
        vnew
    }
}

/// Noise-source tests. These live inside `engine` rather than in the crate's
/// public test module because two of them have to reach the private
/// `ElemState` — the whole point is that the generator's state snapshots and
/// restores correctly, and that is not observable from the public API.
#[cfg(test)]
mod noise_tests {
    use super::*;

    /// Noise source -> 1 MΩ to ground: the loaded node sits within 0.1 % of
    /// the raw EMF, so `voltage_at` reads the generator almost directly.
    fn open_noise(volts: f64, seed: u32) -> Vec<ElementSpec> {
        vec![
            ElementSpec::two(
                1,
                ElementKind::Noise {
                    volts,
                    ohms: 1000.0,
                    seed,
                },
                (0, 0),
                (0, 8),
            ),
            ElementSpec::two(2, ElementKind::Resistor { ohms: 1e6 }, (0, 0), (0, 8)),
            ElementSpec::ground(3, (0, 8)),
        ]
    }

    fn samples(volts: f64, seed: u32, n: usize) -> Vec<f64> {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&open_noise(volts, seed));
        (0..n)
            .map(|_| {
                eng.advance(1);
                eng.voltage_at((0, 0)).unwrap()
            })
            .collect()
    }

    /// The stream is a pure function of (seed, index): same seed, same
    /// sequence, every time and on every target. Different seeds must be
    /// genuinely independent, or two "independent" hiss sources in one patch
    /// would be the same signal played twice.
    #[test]
    fn noise_is_reproducible_from_its_seed() {
        assert_eq!(samples(1.0, 7, 256), samples(1.0, 7, 256));
        let a = samples(1.0, 7, 4096);
        let b = samples(1.0, 8, 4096);
        assert_ne!(a, b, "different seeds must give different noise");
        // Correlation between two seeds should be ~1/sqrt(N) = 0.016.
        let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
        let corr = dot / (na * nb);
        assert!(
            corr.abs() < 0.1,
            "two seeds must be uncorrelated, got r = {corr}"
        );
        // ...and so must consecutive samples of ONE stream: a generator with
        // lag-1 correlation is not white noise, it is a coloured rumble.
        let lag: f64 = a.windows(2).map(|w| w[0] * w[1]).sum::<f64>() / (na * na);
        assert!(lag.abs() < 0.05, "stream must be white, lag-1 r = {lag}");
    }

    /// Exactly what `step()` does on its rescue path: snapshot every
    /// ElemState, roll it back, re-run. The stream counter lives in
    /// ElemState, so the replayed steps must reproduce the same samples —
    /// otherwise one dt-halving rescue would silently fork the generator and
    /// two engines could diverge from an identical starting state.
    #[test]
    fn snapshot_restore_replays_the_same_stream() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&open_noise(1.0, 4242));
        eng.advance(100);
        let saved: Vec<ElemState> = eng.islands[0].elems.iter().map(|e| e.state).collect();
        let saved_time = eng.time;
        let hash = eng.state_hash();
        let first: Vec<f64> = (0..200)
            .map(|_| {
                eng.advance(1);
                eng.voltage_at((0, 0)).unwrap()
            })
            .collect();
        assert_ne!(eng.state_hash(), hash, "200 steps must move the digest");
        for (e, s) in eng.islands[0].elems.iter_mut().zip(saved.iter()) {
            e.state = *s;
        }
        // The rescue path restores state and re-solves; a sleeping island
        // would skip the re-solve and the replay would prove nothing.
        eng.islands[0].wake();
        eng.time = saved_time;
        let again: Vec<f64> = (0..200)
            .map(|_| {
                eng.advance(1);
                eng.voltage_at((0, 0)).unwrap()
            })
            .collect();
        assert_eq!(first, again, "restored state must replay identically");
    }

    /// A recompile (any unrelated edit anywhere in the document) must not
    /// restart the stream: `set_elements` carries ElemState across by id, and
    /// the counter has to ride along with it.
    #[test]
    fn an_edit_elsewhere_does_not_restart_the_stream() {
        let specs = open_noise(1.0, 900);
        let mut a = Engine::new(20e-6);
        a.set_elements(&specs);
        let mut b = Engine::new(20e-6);
        b.set_elements(&specs);
        for _ in 0..50 {
            a.advance(1);
            b.advance(1);
        }
        b.set_elements(&specs); // recompile, same document
        let ta: Vec<f64> = (0..50)
            .map(|_| {
                a.advance(1);
                a.voltage_at((0, 0)).unwrap()
            })
            .collect();
        let tb: Vec<f64> = (0..50)
            .map(|_| {
                b.advance(1);
                b.voltage_at((0, 0)).unwrap()
            })
            .collect();
        assert_eq!(ta, tb, "a recompile must not rewind the generator");
    }

    /// Uniform on [-volts, volts): mean 0, RMS volts/sqrt(3), peak < volts.
    /// A biased or mis-scaled generator is a DC offset or a drum at the wrong
    /// level, and both are silent failures without this.
    #[test]
    fn noise_statistics_are_sane() {
        const N: usize = 20_000;
        let s = samples(2.0, 99, N);
        let mean = s.iter().sum::<f64>() / N as f64;
        let rms = (s.iter().map(|x| x * x).sum::<f64>() / N as f64).sqrt();
        let peak = s.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        // sigma of the mean is 2/sqrt(3)/sqrt(N) = 0.0082; 0.05 is 6 sigma.
        assert!(mean.abs() < 0.05, "mean must be ~0, got {mean}");
        // Expected 2/sqrt(3) = 1.1547, less the 0.1 % divider loss.
        let want = 2.0 / 3.0f64.sqrt() * (1e6 / 1.001e6);
        assert!(
            (rms / want - 1.0).abs() < 0.03,
            "RMS must be volts/sqrt(3) = {want}, got {rms}"
        );
        assert!(peak <= 2.0, "amplitude must not exceed volts, got {peak}");
        assert!(peak > 1.9, "a full-scale stream must reach its peak: {peak}");
    }

    /// The generator is RHS-only: its conductance is constant, so a linear
    /// noise circuit must keep reusing one factorization. If this regresses,
    /// every noise source costs an LU per step and the synth stops holding
    /// real time.
    #[test]
    fn noise_never_forces_a_refactorization() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&open_noise(1.0, 1));
        eng.advance(1);
        assert!(
            eng.islands[0].linear,
            "a noise source must not make a circuit nonlinear"
        );
        eng.advance(5000);
        assert!(
            eng.islands[0].factor_valid,
            "the factorization must survive the stream"
        );
        assert!(!eng.is_quarantined(), "noise must never diverge the solver");
        // ...and the island never went to sleep on the way: a noise source
        // is discrete state that only a solve advances, so it pins its
        // island awake and at the room dt.
        assert!(!eng.islands[0].is_sleepable());
        assert!(eng.islands[0].is_pinned());
        assert_eq!(eng.static_islands(), 0);
        assert_eq!(eng.islands[0].local_dt_k(), 1);
    }

    /// Nothing in the advance may touch a float, and the [-1, 1) map must be
    /// exactly representable, or the two targets have room to disagree.
    #[test]
    fn noise_unit_is_exact_and_bounded() {
        for n in 0..1000u64 {
            let x = noise_unit(31, n);
            assert!((-1.0..1.0).contains(&x), "out of range: {x}");
            // Every sample is an exact multiple of 2^-31 offset by -1, so
            // reconstructing the integer must be lossless.
            let k = (x + 1.0) * 2147483648.0;
            assert_eq!(k, k.floor(), "sample {x} is not on the 2^-31 grid");
        }
        // Pinned vectors: changing the generator changes every saved world
        // that contains one, so it has to be a deliberate act.
        assert_eq!(noise_word(0, 0), 0x7DE5_3DE7_72EA_694C);
        assert_eq!(noise_word(1, 0), 0x38DD_62C4_22DA_381F);
        assert_eq!(noise_word(0, 1), 0x4396_D60D_BD85_37AF);
    }

    /// A part that has failed open stamps nothing and its stream stops:
    /// a dead noise source is silent, and it does not quietly keep burning
    /// through samples where the digest cannot see the effect.
    #[test]
    fn a_broken_noise_source_is_silent_and_frozen() {
        let mut eng = Engine::new(20e-6);
        eng.set_elements(&open_noise(1.0, 5));
        eng.advance(10);
        eng.set_broken(1, true);
        eng.advance(10);
        let v = eng.voltage_at((0, 0)).unwrap();
        assert!(v.abs() < 1e-9, "a dead source must stop driving, got {v}");
        // A broken part is parked behind its island's active prefix, so the
        // counter is read through the document order rather than off slot 0.
        let stream = |eng: &Engine| eng.find(1).unwrap().1.state.noise_n;
        let n_after = stream(&eng);
        eng.advance(100);
        assert_eq!(
            stream(&eng),
            n_after,
            "a dead source must not advance its stream"
        );
    }
}
