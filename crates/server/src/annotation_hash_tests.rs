//! NEITHER ANNOTATION PRIMITIVE TOUCHES THE CIRCUIT — asserted on the STATE
//! HASH, not on the shape of the code.
//!
//! The rest of the suite proves the pieces (a name joins nothing, a label
//! resolves to a node, a detached label survives). These three prove the
//! whole claim end to end and are the ones that fail loudly if anybody ever
//! wires an annotation into the solver: same document, same 20 000 steps,
//! same `state_hash`, same bits in every junction voltage, with
//! `derive_net_map` running every tick exactly as the room task runs it.

use super::*;

/// The whole claim of primitive #1 and #2, measured with a STATE HASH.
///
/// Same document, run 20 000 steps (0.4 s of sim at DT = 20 µs), once with no
/// annotation at all and once with 13 label boxes and 64 net labels present
/// AND `derive_net_map` called every tick the way the room task calls it.
/// The hash and every junction voltage must be bit-identical.
#[test]
fn av_annotation_cannot_move_the_state_hash() {
    // The synth: the biggest shipped document, and the one this feature
    // actually annotates.
    let build = crate::templates::BUILTINS
        .iter()
        .find(|b| b.id == "synth")
        .unwrap()
        .build;
    let ann = build().normalize().unwrap();
    assert_eq!(ann.label_boxes.len(), 13, "the synth ships 13 headings");

    let mut plain = build().normalize().unwrap();
    plain.label_boxes.clear();
    plain.net_labels.clear();
    plain.panels.clear();

    // The elements are the only thing the solver ever sees, so first: the two
    // documents are the SAME BYTES.
    assert_eq!(
        serde_json::to_string(&plain.elements).unwrap(),
        serde_json::to_string(&ann.elements).unwrap(),
        "annotation changed the document"
    );

    // 64 net labels, scattered over the pins of the first 64 elements, so
    // most of them resolve to real nets and the derivation does real work.
    let mut labels: Vec<NetLabel> = Vec::new();
    let next = AtomicU32::new(1);
    for e in ann.elements.iter() {
        if labels.len() >= MAX_NET_LABELS {
            break;
        }
        let p = e.pins[0];
        apply_net_label_op(
            &mut labels,
            &next,
            &NetLabelOp::Add {
                x: p.0,
                y: p.1,
                name: Some(format!("NET{}", e.id)),
            },
        );
    }
    assert_eq!(labels.len(), MAX_NET_LABELS, "64 labels on real pins");

    let probes: Vec<Probe> = ann
        .elements
        .iter()
        .take(16)
        .map(|e| Probe {
            pid: e.id,
            elem: e.id,
            pin: 0,
            kind: ProbeKind::V,
            r: None,
        })
        .collect();

    let mut a = Engine::new(DT);
    a.set_elements(&plain.elements);
    let mut b = Engine::new(DT);
    b.set_elements(&ann.elements);

    let mut named_seen = 0usize;
    for _ in 0..200 {
        a.advance(100);
        b.advance(100);
        // Exactly what the room task does every tick when a room has labels.
        let (live, named) = derive_net_map(&b, &labels, &probes);
        named_seen = named_seen.max(named.len());
        assert!(!live.is_empty(), "labels resolve to nets");
        assert_eq!(
            a.state_hash(),
            b.state_hash(),
            "deriving the net map moved the state hash"
        );
    }
    assert!(named_seen > 0, "probes were actually named");

    // Every junction voltage, bit for bit.
    let mut checked = 0usize;
    for e in ann.elements.iter() {
        for p in e.pins.iter() {
            let (va, vb) = (a.voltage_at(*p), b.voltage_at(*p));
            assert_eq!(
                va.map(f64::to_bits),
                vb.map(f64::to_bits),
                "voltage at {p:?} differs"
            );
            checked += 1;
        }
    }
    assert!(checked > 300, "checked {checked} pins");
    eprintln!(
        "AV: hash {:016x} == {:016x}, {checked} pin voltages bit-identical",
        a.state_hash(),
        b.state_hash()
    );
}

/// TWO NETS, ONE NAME, PROBED. Not `node_at` this time — the actual solved
/// voltage each probe reports, taken through the same accessor the frame
/// broadcast uses.
#[test]
fn av_two_nets_one_name_probe_different_voltages() {
    let elems = vec![
        // Island 1: 12 V across a divider.
        ElementSpec {
            id: 1,
            kind: ElementKind::VoltageSource {
                dc: 12.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
                wave: Default::default(),
            },
            pins: vec![(0, 0), (0, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 2,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(0, 0), (0, 4)],
            ..Default::default()
        },
        ElementSpec {
            id: 3,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(0, 4), (0, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 4,
            kind: ElementKind::Ground,
            pins: vec![(0, 8)],
            ..Default::default()
        },
        // Island 2, 100 units east: 3 V across a divider.
        ElementSpec {
            id: 5,
            kind: ElementKind::VoltageSource {
                dc: 3.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
                wave: Default::default(),
            },
            pins: vec![(100, 0), (100, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 6,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(100, 0), (100, 4)],
            ..Default::default()
        },
        ElementSpec {
            id: 7,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(100, 4), (100, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 8,
            kind: ElementKind::Ground,
            pins: vec![(100, 8)],
            ..Default::default()
        },
    ];
    let mut eng = Engine::new(DT);
    eng.set_elements(&elems);
    eng.advance(200);

    let labels = vec![
        NetLabel {
            nlid: 1,
            x: 0,
            y: 4,
            name: "MID".into(),
        },
        NetLabel {
            nlid: 2,
            x: 100,
            y: 4,
            name: "MID".into(),
        },
    ];
    let probes = vec![
        Probe {
            pid: 1,
            elem: 2,
            pin: 1,
            kind: ProbeKind::V,
            r: None,
        },
        Probe {
            pid: 2,
            elem: 6,
            pin: 1,
            kind: ProbeKind::V,
            r: None,
        },
    ];
    let (live, named) = derive_net_map(&eng, &labels, &probes);
    assert_eq!(live.len(), 2);
    assert_eq!(named.len(), 2, "both probes are named");
    assert_eq!(named[0].1, 1);
    assert_eq!(named[1].1, 2, "each probe gets ITS OWN net's label");

    let v1 = eng.voltage_at((0, 4)).unwrap();
    let v2 = eng.voltage_at((100, 4)).unwrap();
    eprintln!("AV: MID = {v1} V and MID = {v2} V — two nets, one name");
    assert!((v1 - 6.0).abs() < 1e-6, "{v1}");
    assert!((v2 - 1.5).abs() < 1e-6, "{v2}");
    assert_ne!(v1.to_bits(), v2.to_bits());
    assert_ne!(eng.node_at((0, 4)), eng.node_at((100, 4)));
}

/// SPLIT a named net and JOIN two named nets, and read what actually happens
/// against what `NetLabel` says should happen.
#[test]
fn av_split_and_join_named_nets() {
    // A ---wire--- B, both ends named, one source, one load.
    let mut elems = vec![
        ElementSpec {
            id: 1,
            kind: ElementKind::VoltageSource {
                dc: 10.0,
                amp: 0.0,
                hz: 0.0,
                phase: 0.0,
                wave: Default::default(),
            },
            pins: vec![(0, 0), (0, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 2,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(0, 0), (0, 4)],
            ..Default::default()
        },
        ElementSpec {
            id: 3,
            kind: ElementKind::Resistor { ohms: 1000.0 },
            pins: vec![(10, 4), (10, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 4,
            kind: ElementKind::Ground,
            pins: vec![(0, 8)],
            ..Default::default()
        },
        ElementSpec {
            id: 5,
            kind: ElementKind::Wire,
            pins: vec![(0, 8), (10, 8)],
            ..Default::default()
        },
        // THE BRIDGE: joins the two mid-points into one net.
        ElementSpec {
            id: 6,
            kind: ElementKind::Wire,
            pins: vec![(0, 4), (10, 4)],
            ..Default::default()
        },
    ];
    let labels = vec![
        NetLabel {
            nlid: 1,
            x: 0,
            y: 4,
            name: "LEFT".into(),
        },
        NetLabel {
            nlid: 2,
            x: 10,
            y: 4,
            name: "RIGHT".into(),
        },
    ];
    let probes = vec![
        Probe {
            pid: 1,
            elem: 2,
            pin: 1,
            kind: ProbeKind::V,
            r: None,
        },
        Probe {
            pid: 2,
            elem: 3,
            pin: 0,
            kind: ProbeKind::V,
            r: None,
        },
    ];
    let mut eng = Engine::new(DT);
    eng.set_elements(&elems);
    eng.advance(200);

    // JOINED: one net, two names. Both live; both probes get the SAME name,
    // and it is the lowest (y, x) one.
    let (live, named) = derive_net_map(&eng, &labels, &probes);
    assert_eq!(live.len(), 2, "both names are real");
    assert_eq!(eng.node_at((0, 4)), eng.node_at((10, 4)), "one net");
    assert_eq!(named.len(), 2);
    assert_eq!(named[0].1, named[1].1, "both probes show the same string");
    assert_eq!(named[0].1, 1, "lowest (y,x) is LEFT at x=0");
    eprintln!("AV joined: live={live:?} named={named:?}");

    // SPLIT: cut the bridge. Two nets, one name each, nothing deleted.
    elems.retain(|e| e.id != 6);
    eng.set_elements(&elems);
    eng.advance(200);
    let (live2, named2) = derive_net_map(&eng, &labels, &probes);
    assert_eq!(live2.len(), 2, "both labels still name something");
    assert_ne!(eng.node_at((0, 4)), eng.node_at((10, 4)), "two nets now");
    assert_eq!(named2.len(), 2);
    assert_eq!(named2[0].1, 1);
    assert_eq!(named2[1].1, 2, "each half now reports its OWN name");
    eprintln!("AV split: live={live2:?} named={named2:?}");
    assert_eq!(labels.len(), 2, "a split never deletes a label");
}
