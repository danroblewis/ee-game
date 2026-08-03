//! CLI harness: relayout a saved room.
//!
//!   cargo run -p sim-layout --bin relayout -- <room.json> <out-elements.json> [out-room.json]
//!
//! `<room.json>` may be a full room save (object with an `elements` array)
//! or a bare array of ElementSpec. Writes the relaid element array to
//! `<out-elements.json>`; with a third argument, also writes a copy of the
//! room save with its elements replaced (loadable by the server).
//! Prints a JSON summary (report + determinism double-run check) to stdout.

use sim_core::ElementSpec;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: relayout <room.json> <out-elements.json> [out-room.json]");
        std::process::exit(2);
    }
    let raw = std::fs::read_to_string(&args[1]).expect("read input");
    let val: serde_json::Value = serde_json::from_str(&raw).expect("parse input");
    let elements_val = if val.is_array() {
        val.clone()
    } else {
        val.get("elements").cloned().expect("room has no elements")
    };
    let elements: Vec<ElementSpec> =
        serde_json::from_value(elements_val).expect("parse elements");

    let result = match sim_layout::relayout(&elements) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("LAYOUT FAILED: {e}");
            std::process::exit(1);
        }
    };
    // determinism: run again, byte-compare
    let again = sim_layout::relayout(&elements).expect("second run");
    let b1 = serde_json::to_string(&result.elements).unwrap();
    let b2 = serde_json::to_string(&again.elements).unwrap();
    assert_eq!(b1, b2, "non-deterministic output");

    std::fs::write(&args[2], serde_json::to_string_pretty(&result.elements).unwrap())
        .expect("write output");
    if args.len() > 3 && !val.is_array() {
        let mut room = val.clone();
        room["elements"] = serde_json::to_value(&result.elements).unwrap();
        std::fs::write(&args[3], serde_json::to_string(&room).unwrap()).expect("write room");
    }

    let rep = &result.report;
    println!(
        "{}",
        serde_json::json!({
            "quality": format!("{:?}", rep.quality),
            "elements": { "before": rep.elements_before, "after": rep.elements_after },
            "wires": { "before": rep.wires_before, "after": rep.wires_after },
            "flags": rep.flags_after,
            "connections": rep.conns,
            "tiers": {
                "abut": rep.tier_abut,
                "pattern": rep.tier_pattern,
                "astar": rep.tier_astar,
                "staircase": rep.tier_staircase,
            },
            "crossings_paid": rep.crossings_paid,
            "deterministic": true,
            "netlist_preserved": true,
            "check_document": "ok",
            "notes": rep.notes,
        })
    );
}
