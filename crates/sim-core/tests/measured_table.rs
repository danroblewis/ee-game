//! The measured table, pinned as a test.
//!
//! Every line the design brief measured against the pre-change tree, in one
//! place, with the verdict this change is supposed to produce. Printed as a
//! table with `--nocapture` so the before/after is readable at a glance, and
//! asserted so it cannot silently drift.

use sim_core::{check_document, ElementKind, ElementSpec, Reject};

const DT: f64 = 20e-6;

fn dc(v: f64) -> ElementKind {
    ElementKind::VoltageSource {
        wave: sim_core::Wave::Sine,
        dc: v,
        amp: 0.0,
        hz: 0.0,
        phase: 0.0,
    }
}

fn ac(dc: f64, amp: f64, hz: f64, phase: f64) -> ElementKind {
    ElementKind::VoltageSource { dc, amp, hz, phase, wave: sim_core::Wave::Sine }
}

fn rail(v: f64) -> ElementKind {
    ElementKind::Rail {
        wave: sim_core::Wave::Sine,
        dc: v,
        amp: 0.0,
        hz: 0.0,
        phase: 0.0,
    }
}

fn rail_at(id: u32, v: f64, at: (i32, i32)) -> ElementSpec {
    ElementSpec {
        id,
        kind: rail(v),
        pins: vec![at],
        ..Default::default()
    }
}

fn r(ohms: f64) -> ElementKind {
    ElementKind::Resistor { ohms }
}

fn load(id: u32) -> ElementSpec {
    ElementSpec::two(id, r(1000.0), (0, 0), (0, 6))
}

fn gnd(id: u32) -> ElementSpec {
    ElementSpec::ground(id, (0, 6))
}

/// What a verdict is, coarsely, for the table.
fn verdict(res: &Result<(), Reject>) -> String {
    match res {
        Ok(()) => "ACCEPTED".to_string(),
        Err(e) => format!("REJECTED({})", e.code()),
    }
}

#[test]
fn the_measured_table() {
    type Case = (&'static str, Vec<ElementSpec>, bool, &'static str);

    let sw = |closed| ElementKind::Switch { closed };

    let cases: Vec<Case> = vec![
        // ---- the owner's original seven ----
        (
            "1V and 5V directly connected",
            vec![
                ElementSpec::two(1, dc(1.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
                gnd(3),
            ],
            false,
            "conflicting_sources",
        ),
        (
            "two 5V sources directly connected",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
                load(3),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "two 5V Rails on the same node",
            vec![rail_at(1, 5.0, (0, 0)), rail_at(2, 5.0, (0, 0)), load(3), gnd(4)],
            true,
            "",
        ),
        (
            "5V and 12V Rails on the same node",
            vec![rail_at(1, 5.0, (0, 0)), rail_at(2, 12.0, (0, 0)), load(3), gnd(4)],
            false,
            "conflicting_sources",
        ),
        (
            "5V source shorted by a wire",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Wire, (0, 0), (0, 6)),
                gnd(3),
            ],
            false,
            "shorted_source",
        ),
        (
            "two 5V sources in series",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 6), (0, 12)),
                ElementSpec::two(3, r(1000.0), (0, 0), (0, 12)),
                ElementSpec::ground(4, (0, 12)),
            ],
            true,
            "",
        ),
        (
            "damage-exploit repro (lamp + conflicting source)",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(
                    2,
                    ElementKind::Lamp {
                        ohms: 90.0,
                        rated_watts: 1.0,
                    },
                    (0, 0),
                    (0, 6),
                ),
                ElementSpec::two(3, dc(240.0), (0, 0), (0, 6)),
                gnd(4),
            ],
            false,
            "conflicting_sources",
        ),
        // ---- the ten the design added ----
        (
            "two 5V sources ANTI-parallel (pins flipped)",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 6), (0, 0)),
                gnd(3),
            ],
            false,
            "conflicting_sources",
        ),
        (
            "THREE 5V sources all parallel",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(3, dc(5.0), (0, 0), (0, 6)),
                load(8),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "5V DC + {dc:5,amp:1,hz:50} parallel",
            vec![
                ElementSpec::two(1, dc(5.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ac(5.0, 1.0, 50.0, 0.0), (0, 0), (0, 6)),
                gnd(3),
            ],
            false,
            "conflicting_sources",
        ),
        (
            "two identical AC sources parallel",
            vec![
                ElementSpec::two(1, ac(0.0, 5.0, 50.0, 0.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ac(0.0, 5.0, 50.0, 0.0), (0, 0), (0, 6)),
                load(3),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "5V source (to gnd) + 5V Rail on same node",
            vec![
                rail_at(1, 5.0, (0, 0)),
                ElementSpec::two(2, dc(5.0), (0, 0), (0, 6)),
                load(3),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "Rail 5V + VSource -5V drawn ground->rail",
            vec![
                rail_at(1, 5.0, (0, 0)),
                ElementSpec::two(2, dc(-5.0), (0, 6), (0, 0)),
                load(3),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "two CLOSED switches in parallel",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, sw(true), (0, 0), (4, 0)),
                ElementSpec::two(3, sw(true), (0, 0), (4, 0)),
                ElementSpec::two(4, r(90.0), (4, 0), (0, 6)),
                gnd(5),
            ],
            true,
            "",
        ),
        (
            "two OPEN switches in parallel",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, sw(false), (0, 0), (4, 0)),
                ElementSpec::two(3, sw(false), (0, 0), (4, 0)),
                ElementSpec::two(4, r(90.0), (4, 0), (0, 6)),
                gnd(5),
            ],
            true,
            "",
        ),
        (
            "two Motors in parallel",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(
                    2,
                    ElementKind::Motor {
                        ohms: 2.0,
                        henries: 1.5e-3,
                        bemf: 0.0,
                    },
                    (0, 0),
                    (0, 6),
                ),
                ElementSpec::two(
                    3,
                    ElementKind::Motor {
                        ohms: 2.0,
                        henries: 1.5e-3,
                        bemf: 0.0,
                    },
                    (0, 0),
                    (0, 6),
                ),
                gnd(4),
            ],
            true,
            "",
        ),
        (
            "two OpAmps sharing all three pins",
            vec![
                ElementSpec::three(1, ElementKind::OpAmp { rail: 9.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 0), (4, 0), (8, 0)),
                ElementSpec::three(2, ElementKind::OpAmp { rail: 9.0, isc: sim_core::DEFAULT_OPAMP_ISC }, (0, 0), (4, 0), (8, 0)),
                ElementSpec::two(3, r(1000.0), (8, 0), (0, 6)),
                gnd(4),
            ],
            false,
            "unsolvable",
        ),
        (
            "9V straight across the shipped LED",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, ElementKind::Led { color: 0 }, (0, 0), (0, 6)),
                gnd(3),
            ],
            false,
            "will_not_converge",
        ),
        // ---- the classic V-loop, and the rail on ground ----
        (
            "a loop of three ideal sources",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, dc(5.0), (0, 6), (6, 6)),
                ElementSpec::two(3, dc(3.0), (6, 6), (0, 0)),
            ],
            false,
            "source_loop",
        ),
        (
            "Rail 0V + Ground on the same point",
            vec![rail_at(1, 0.0, (0, 0)), ElementSpec::ground(2, (0, 0))],
            false,
            "shorted_source",
        ),
        (
            "two 5V Rails on DIFFERENT nodes, each own load",
            vec![
                rail_at(1, 5.0, (0, 0)),
                rail_at(2, 5.0, (8, 0)),
                ElementSpec::two(3, r(1000.0), (0, 0), (0, 6)),
                ElementSpec::two(4, r(1000.0), (8, 0), (0, 6)),
                gnd(5),
            ],
            true,
            "",
        ),
        (
            "two Grounds at different points",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                load(2),
                gnd(3),
                ElementSpec::ground(4, (20, 20)),
            ],
            true,
            "",
        ),
        (
            "the shipped LED WITH a 330 ohm series resistor",
            vec![
                ElementSpec::two(1, dc(9.0), (0, 0), (0, 6)),
                ElementSpec::two(2, r(330.0), (0, 0), (4, 0)),
                ElementSpec::two(3, ElementKind::Led { color: 0 }, (4, 0), (0, 6)),
                gnd(4),
            ],
            true,
            "",
        ),
    ];

    println!(
        "\n{:<48} {:<28} {}",
        "CASE", "VERDICT", "WHAT THE PLAYER IS TOLD"
    );
    println!("{}", "-".repeat(140));
    let mut bad = Vec::new();
    for (why, specs, want_ok, want_code) in &cases {
        let res = check_document(specs, DT);
        let hint = match &res {
            Ok(()) => "-".to_string(),
            Err(e) => e.hint(),
        };
        println!("{:<48} {:<28} {}", why, verdict(&res), hint);
        match (&res, want_ok) {
            (Ok(()), true) => {}
            (Err(e), false) if e.code() == *want_code => {}
            _ => bad.push(format!(
                "{why}: wanted {}, got {}",
                if *want_ok {
                    "ACCEPTED".to_string()
                } else {
                    format!("REJECTED({want_code})")
                },
                verdict(&res)
            )),
        }
    }
    println!();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
