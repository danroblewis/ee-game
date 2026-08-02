//! The hoist model against closed forms.
//!
//! These tests drive the mechanism with the QUASI-STATIC armature law
//! i = (v_terminal - K·ω)/R_loop, which is exact once the electrical pole
//! (L/R = 0.75 ms) has settled — 33× faster than τ_mech. The full
//! solver-in-the-loop numbers (where i really is an MNA unknown) are
//! asserted in the server's integration test; here we are pinning the
//! mechanical model and the goal arithmetic.

use machine::*;

/// The shipped machine tick: 32 substeps of the server's 20 µs dt.
const H: f64 = 32.0 * 20e-6;

/// Armature current for a terminal voltage across a total loop resistance.
fn armature_i(v_terminal: f64, r_loop: f64, omega: f64) -> f64 {
    (v_terminal - K * omega) / r_loop
}

/// Steady-state rotor speed from torque balance:
/// K·(V - K·ω)/R = m·g·r + b·ω.
fn omega_steady(v_terminal: f64, r_loop: f64) -> f64 {
    (K * v_terminal / r_loop - LOAD_TORQUE) / (K * K / r_loop + VISCOUS_B)
}

#[test]
fn twelve_volts_lifts_at_the_closed_form_speed() {
    let expect_omega = omega_steady(12.0, R_ARM);
    // The contract's number, independently: (0.25·12/2 - 0.23544)/0.03145.
    assert!(
        (expect_omega - 40.2).abs() < 0.01,
        "closed form ω_ss = {expect_omega}, contract says 40.2 rad/s"
    );

    let mut h = Hoist::default();
    let mut t = 0.0;
    let mut omega_free = 0.0; // rotor speed while still travelling
    let mut t_top = f64::NAN;
    for _ in 0..2000 {
        let i = armature_i(12.0, R_ARM, h.omega);
        h.tick(i, H);
        t += H;
        if h.y < SHAFT_H {
            omega_free = h.omega;
        } else if t_top.is_nan() {
            t_top = t;
        }
    }

    // Free-running speed: 19 τ_mech of travel, so the transient is gone.
    assert!(
        (omega_free - expect_omega).abs() < 1e-6,
        "ω = {omega_free}, closed form {expect_omega}"
    );
    assert!(
        (omega_free - 40.2).abs() < 0.05,
        "ω = {omega_free} rad/s, contract says 40.2"
    );
    let vel = DRUM_R * omega_free;
    assert!(
        (vel - 0.804).abs() < 0.001,
        "lift speed {vel} m/s, want 0.804"
    );

    // Full lift = the steady-state travel time plus one mechanical time
    // constant of spin-up: 0.40/0.80417 + 0.0248 = 0.5222 s.
    let expect_top = SHAFT_H / vel + TAU_MECH;
    assert!(
        (t_top - expect_top).abs() < 0.005,
        "reached the head stop at {t_top} s, closed form {expect_top} s"
    );
    assert_eq!(h.y, SHAFT_H, "must be parked against the head stop");
    assert!(
        h.omega <= 0.0,
        "upward motion must be arrested: ω = {}",
        h.omega
    );

    // Voltage buys SPEED: 12 V commands 0.80 m/s, so the crate flies through
    // the band and parks out of it. Scoped to 12 V on purpose — the balance
    // voltage m·g·r·R/K holds the band open loop (see HOLD_CURRENT's docs and
    // the server's `the_balance_voltage_holds_the_band_open_loop`).
    assert!(!h.win, "constant 12 V must never satisfy the hold");
    assert_eq!(h.hold, 0.0, "hold must have drained at the head stop");
    eprintln!("12 V: ω={omega_free:.4} rad/s v={vel:.4} m/s t_top={t_top:.4} s");
}

/// Descend from the head stop against a resistive brake loop for `n_taus`
/// mechanical time constants. Returns (measured speed, the exponential tail
/// still left in it) — the tail is theory, not a tuned tolerance:
/// v(t) = v_ss·(1 - e^(-t/τ)), so the residual is |v_ss|·e^(-n).
fn descend_speed(r_loop: f64, n_taus: f64) -> (f64, f64) {
    let v_ss = DRUM_R * omega_steady(0.0, r_loop);
    let tau = J_EFF / (K * K / r_loop + VISCOUS_B);
    let mut h = Hoist::default();
    h.y = SHAFT_H;
    let ticks = (n_taus * tau / H).round() as u32;
    for _ in 0..ticks {
        let i = armature_i(0.0, r_loop, h.omega);
        h.tick(i, H);
    }
    assert!(
        h.y > 0.0,
        "descent must not reach the floor during the test"
    );
    (h.velocity(), v_ss.abs() * libm::exp(-n_taus))
}

/// The closed-form terminal speed must be an exact fixed point of the
/// integrator: start the rotor there and it must not move.
fn assert_terminal_speed_is_fixed(r_loop: f64) {
    let mut h = Hoist::default();
    h.y = 0.5 * SHAFT_H;
    h.omega = omega_steady(0.0, r_loop);
    let before = h.omega;
    for _ in 0..100 {
        let i = armature_i(0.0, r_loop, h.omega);
        h.tick(i, H);
    }
    assert!(
        (h.omega - before).abs() < 1e-9,
        "{r_loop} Ω loop: ω drifted from {before} to {}",
        h.omega
    );
}

#[test]
fn shorted_leads_brake_the_descent() {
    // 0 V terminal: i = -K·ω/R, so ω·(K²/R + b) = -m·g·r.
    let expect = DRUM_R * omega_steady(0.0, R_ARM);
    assert!(
        (expect + 0.150).abs() < 0.001,
        "closed form {expect} m/s, contract says -0.150"
    );
    assert_terminal_speed_is_fixed(R_ARM);
    let (measured, tail) = descend_speed(R_ARM, 8.0);
    assert!(
        (measured - expect).abs() < tail,
        "shorted descent {measured} m/s, closed form {expect} (tail {tail})"
    );
    eprintln!("shorted: {measured:.6} m/s");
}

#[test]
fn ballast_resistance_lets_it_descend_faster() {
    // The contract's "-0.30 m/s through an external 4 ohm" is a 4 Ω TOTAL
    // loop (a 2 Ω ballast in series with the 2 Ω armature). Both cases are
    // asserted so neither reading can rot.
    let four_total = DRUM_R * omega_steady(0.0, 4.0);
    assert!(
        (four_total + 0.30).abs() < 0.005,
        "4 Ω loop gives {four_total} m/s, contract says -0.30"
    );
    assert_terminal_speed_is_fixed(4.0);
    let (measured, tail) = descend_speed(4.0, 6.0);
    assert!(
        (measured - four_total).abs() < tail,
        "4 Ω loop descent {measured} m/s, closed form {four_total} (tail {tail})"
    );

    // A 4 Ω ballast ON TOP of the armature brakes less, so it falls faster.
    let six_total = DRUM_R * omega_steady(0.0, R_ARM + 4.0);
    assert_terminal_speed_is_fixed(R_ARM + 4.0);
    let (measured6, tail6) = descend_speed(R_ARM + 4.0, 5.0);
    assert!(
        (measured6 - six_total).abs() < tail6,
        "6 Ω loop descent {measured6} m/s, closed form {six_total} (tail {tail6})"
    );
    assert!(
        measured6 < measured,
        "less braking must descend faster: {measured6} vs {measured}"
    );
    eprintln!("4 Ω loop: {measured:.6} m/s   6 Ω loop: {measured6:.6} m/s");
}

#[test]
fn open_leads_free_fall_lands_hard() {
    // Open circuit: no armature current, so the only torques are gravity
    // and viscous drag. The drag-free acceleration is
    // a = m·g·r²/J_eff = m·g/(m + J_rotor/r²) = 6.037 m/s²,
    // giving sqrt(2·a·H) = 2.198 m/s at the floor.
    let a_dragfree = LOAD_TORQUE * DRUM_R / J_EFF;
    assert!(
        (a_dragfree - 6.04).abs() < 0.01,
        "free-fall a = {a_dragfree} m/s², contract says 6.04"
    );
    let v_dragfree = (2.0 * a_dragfree * SHAFT_H).sqrt();
    assert!(
        (v_dragfree - 2.20).abs() < 0.01,
        "drag-free landing {v_dragfree} m/s, contract says 2.20"
    );

    // With b = 2e-4 the same fall is a damped ramp:
    //   ω(t)  = -Ω_t·(1 - e^(-t/τ_v)),  Ω_t = m·g·r/b, τ_v = J/b
    //   y(t)  = H - r·Ω_t·[t - τ_v·(1 - e^(-t/τ_v))]
    let omega_t = LOAD_TORQUE / VISCOUS_B;
    let tau_v = J_EFF / VISCOUS_B;
    let drop = |t: f64| DRUM_R * omega_t * (t - tau_v * (1.0 - libm::exp(-t / tau_v)));
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if drop(mid) < SHAFT_H {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t_land = 0.5 * (lo + hi);
    let v_expect = DRUM_R * omega_t * (1.0 - libm::exp(-t_land / tau_v));

    let mut h = Hoist::default();
    h.y = SHAFT_H;
    let mut impact = 0.0;
    for _ in 0..2000 {
        h.tick(0.0, H);
        if h.impact > 0.0 {
            impact = h.impact;
            break;
        }
    }

    assert!(
        (impact - v_expect).abs() < 2e-3,
        "landing {impact} m/s, closed form with drag {v_expect} m/s (t_land {t_land} s)"
    );
    // Viscous drag costs ~3% of the drag-free 2.20 m/s.
    assert!(
        (impact - v_dragfree).abs() < 0.08,
        "landing {impact} m/s vs drag-free {v_dragfree} m/s"
    );
    assert!(impact > HARD_LANDING, "a 0.40 m drop must count as hard");
    assert_eq!(h.landings, 1, "exactly one landing");
    assert_eq!(h.y, 0.0);
    assert_eq!(h.omega, 0.0, "the rope goes slack on the floor");
    eprintln!("free fall: {impact:.6} m/s (closed form {v_expect:.6}, drag-free {v_dragfree:.6})");

    // Resting on the floor is not a landing: no further impacts, no count.
    for _ in 0..100 {
        h.tick(0.0, H);
        assert_eq!(h.impact, 0.0, "resting on the floor must not re-trigger");
    }
    assert_eq!(h.landings, 1);
}

#[test]
fn hold_current_is_a_static_equilibrium() {
    assert!(
        (HOLD_CURRENT - 0.94).abs() < 0.005,
        "hold current {HOLD_CURRENT} A, contract says m·g·r/K = 0.94"
    );

    // Park the crate mid-band and feed exactly m·g·r/K: the net torque is
    // identically zero, so it must not move at all — and the goal must fill
    // to 5.0 s in exactly 5.0 s of ticks.
    let mut h = Hoist::default();
    h.y = 0.320;
    let start = h.y;
    let ticks = (HOLD_NEED / H).ceil() as u32; // 5.0 s is 7812.5 ticks
    for _ in 0..ticks {
        h.tick(HOLD_CURRENT, H);
    }
    assert_eq!(h.y, start, "static hold must not drift: y = {}", h.y);
    assert_eq!(h.omega, 0.0);
    assert!(h.win, "5.0 s in band must win, hold = {}", h.hold);
    assert_eq!(h.landings, 0);
    eprintln!("static hold: i={HOLD_CURRENT:.6} A, hold={:.4} s", h.hold);
}

#[test]
fn hold_timer_fills_drains_and_clamps() {
    let mut h = Hoist::default();
    h.y = 0.320; // in band

    // Fill: exactly h per tick.
    for n in 1..=10u32 {
        h.tick(HOLD_CURRENT, H);
        let expect = n as f64 * H;
        assert!(
            (h.hold - expect).abs() < 1e-12,
            "tick {n}: hold {} want {expect}",
            h.hold
        );
    }

    // Drain: 3× as fast, out of band (band edges are inclusive, so 0.340 is
    // still in and 0.341 is out). The hold current keeps y exactly put, so
    // the arithmetic is the only thing under test.
    h.y = 0.341;
    let before = h.hold;
    h.tick(HOLD_CURRENT, H);
    let expect = before - HOLD_DRAIN * H;
    assert!(
        (h.hold - expect).abs() < 1e-12,
        "drain: hold {} want {expect}",
        h.hold
    );

    // Clamp at zero, never negative.
    for _ in 0..100 {
        h.tick(HOLD_CURRENT, H);
    }
    assert_eq!(h.hold, 0.0, "hold must clamp at 0");
    assert!(!h.win);

    // Clamp at HOLD_NEED, and the win latches.
    h.y = 0.300; // the low band edge is in band
    let ticks = (2.0 * HOLD_NEED / H).round() as u32;
    for _ in 0..ticks {
        h.tick(HOLD_CURRENT, H);
    }
    assert_eq!(h.hold, HOLD_NEED, "hold must clamp at 5.0");
    assert!(h.win);

    // A bang-bang controller that overshoots out of band 1 tick in 5 still
    // wins: 4h - 3h = +h net per 5 ticks. (At 3 in / 1 out it is exactly
    // break-even, which is what "drain at 3x" buys: sloppy control still
    // converges, but only if it is in band most of the time.)
    let mut b = Hoist::default();
    for n in 0..45_000u32 {
        b.y = if n % 5 == 1 { 0.290 } else { 0.320 };
        b.tick(HOLD_CURRENT, H);
    }
    assert!(
        b.win,
        "3x drain must not starve bang-bang control: hold={}",
        b.hold
    );
    eprintln!("bang-bang 4/5 duty: hold={:.4} s after 45000 ticks", b.hold);
}

#[test]
fn limit_switches_latch_with_release_hysteresis() {
    // Every tick here runs at the static hold current with ω = 0, so y is
    // exactly the value placed on it and the thresholds are testable to the
    // last bit.
    let mut h = Hoist::default();
    // Fresh at the floor: LIM-BOT closed, LIM-TOP open.
    let w = h.tick(HOLD_CURRENT, H);
    assert!(w.lim_bot && !w.lim_top);

    // Just below the threshold: still open.
    h.y = LIM_TOP_Y - 1e-9;
    assert!(!h.tick(HOLD_CURRENT, H).lim_top);
    // At the threshold: closed.
    h.y = LIM_TOP_Y;
    assert!(h.tick(HOLD_CURRENT, H).lim_top);
    // Inside the 2 mm release window: stays closed.
    h.y = LIM_TOP_Y - LIM_HYST + 1e-9;
    assert!(
        h.tick(HOLD_CURRENT, H).lim_top,
        "must hold through the release window"
    );
    // Past it: opens.
    h.y = LIM_TOP_Y - LIM_HYST - 1e-9;
    assert!(!h.tick(HOLD_CURRENT, H).lim_top);

    h.y = LIM_BOT_Y + LIM_HYST; // above the close point, below release
    assert!(
        !h.tick(HOLD_CURRENT, H).lim_bot,
        "opened on the way up, stays open"
    );
    h.y = LIM_BOT_Y;
    assert!(h.tick(HOLD_CURRENT, H).lim_bot);
    h.y = LIM_BOT_Y + LIM_HYST;
    assert!(
        h.tick(HOLD_CURRENT, H).lim_bot,
        "must hold through the release window"
    );
    h.y = LIM_BOT_Y + LIM_HYST + 1e-9;
    assert!(!h.tick(HOLD_CURRENT, H).lim_bot);
}

#[test]
fn sensor_wiper_tracks_height_and_stays_off_its_ends() {
    let mut h = Hoist::default();
    for (y, expect) in [
        (0.0, WIPER_MAX),
        (SHAFT_H, WIPER_MIN),
        (0.320, 1.0 - 0.320 / SHAFT_H),
        (0.200, 0.5),
    ] {
        h.y = y;
        let w = h.tick(HOLD_CURRENT, H); // static: y does not move
        assert!(
            (w.wiper - expect).abs() < 1e-12,
            "y={y}: wiper {} want {expect}",
            w.wiper
        );
        assert!((WIPER_MIN..=WIPER_MAX).contains(&w.wiper));
    }
}

#[test]
fn reset_rearms_the_goal_but_keeps_the_energy_meter() {
    let mut h = Hoist::default();
    h.y = SHAFT_H;
    for _ in 0..2000 {
        h.tick(0.0, H); // fall, land hard, count it
    }
    h.accumulate_joules(12.0 * 0.9, 1.0);
    assert_eq!(h.landings, 1);
    assert!(h.joules > 0.0);
    let joules = h.joules;

    h.reset();
    assert_eq!(h.y, 0.0);
    assert_eq!(h.omega, 0.0);
    assert_eq!(h.hold, 0.0);
    assert_eq!(h.landings, 0);
    assert_eq!(h.impact, 0.0);
    assert!(!h.win);
    assert_eq!(
        h.joules, joules,
        "the energy meter is not part of the attempt"
    );
}

#[test]
fn back_emf_write_is_the_rotor_speed() {
    let mut h = Hoist::default();
    let w = h.tick(2.0, H); // some lifting current
    assert!(h.omega > 0.0);
    assert_eq!(w.bemf, K * h.omega, "bemf must be exactly K·ω");
}
