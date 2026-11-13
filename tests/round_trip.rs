//! Reading a document, walking it, changing it, and writing it back.

use std::collections::BTreeMap;
use valrig_core::Error;
use valrig_core::preset::{Asset, AssetKind, Lane, MAX_STAGES, Module, Preset, Stage, asset_hash};

const WIDE: &str = include_str!("../assets/plexi-tweed-wide.rig");

fn module(type_id: &str, name: &str) -> Module {
    Module {
        type_id: type_id.into(),
        name: name.into(),
        asset: None,
        bypass: false,
        params: BTreeMap::new(),
        ui: BTreeMap::new(),
    }
}

#[test]
fn the_wide_rig_parses() {
    let p = Preset::parse(WIDE).unwrap();
    assert_eq!(p.name, "Plexi + Tweed · Wide");
    assert_eq!(p.author.as_deref(), Some("Valrig"));
    assert_eq!(p.assets.len(), 3);
    assert_eq!(p.assets["plexi"].kind, AssetKind::Nam);
    assert_eq!(p.assets["plexi"].file.as_deref(), Some("a2.nam"));
    assert_eq!(
        p.assets["plexi"].sample_rate, None,
        "a capture carries its own rate"
    );
    assert_eq!(p.assets["cab"].sample_rate, Some(48_000.0));
    assert_eq!(p.stages.len(), 6);

    let names: Vec<&str> = p.modules().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "input",
            "gate",
            "compressor",
            "A.capture",
            "A.eq",
            "A.cabinet",
            "B.capture",
            "B.eq",
            "B.cabinet",
            "reverb",
            "output"
        ]
    );

    assert!(matches!(&p.stages[2], Stage::Module(m) if m.bypass && m.type_id == "std.compressor"));
    let Stage::Lanes(lanes) = &p.stages[3] else {
        panic!("stage 3 is the parallel one")
    };
    assert_eq!(lanes.len(), 2);
    assert_eq!((lanes[0].pan, lanes[0].level_db), (-1.0, 0.0));
    assert_eq!((lanes[1].pan, lanes[1].level_db), (1.0, -1.0));
    assert_eq!(lanes[0].modules[0].asset.as_deref(), Some("plexi"));
    assert_eq!(lanes[0].modules[1].params["bass_db"], 1.5);
    assert!(
        lanes[1].modules[1].params.is_empty(),
        "B's tone control is at its defaults"
    );
}

#[test]
fn every_capture_hash_is_a_content_address() {
    let p = Preset::parse(WIDE).unwrap();
    for (label, asset) in &p.assets {
        let hash = asset.hash.as_str();
        assert!(hash.starts_with("blake3:"), "{label}");
        assert_eq!(hash.len(), "blake3:".len() + 64, "{label}");
    }
}

#[test]
fn values_are_addresses_in_rack_order() {
    let p = Preset::parse(WIDE).unwrap();
    let values = p.values();
    let at = |address: &str| {
        values
            .iter()
            .position(|(a, _)| a == address)
            .unwrap_or_else(|| panic!("{address} missing"))
    };

    assert_eq!(values[0], ("input/level_db".to_string(), 0.0));
    assert_eq!(values[at("A/pan")].1, -1.0);
    assert_eq!(values[at("A.eq/bass_db")].1, 1.5);
    assert_eq!(values[at("B/level_db")].1, -1.0);
    assert!(at("gate/threshold_db") < at("A/level_db"));
    assert!(at("A/level_db") < at("A/pan") && at("A/pan") < at("A.eq/bass_db"));
    assert!(
        at("A.eq/bass_db") < at("B/pan")
            && at("B/pan") < at("reverb/mix")
            && at("reverb/mix") < at("output/level_db")
    );
    assert!(
        !values.iter().any(|(a, _)| a.starts_with("B.eq/")),
        "unset parameters are not values"
    );
}

#[test]
fn bypasses_list_every_module_and_lane() {
    let p = Preset::parse(WIDE).unwrap();
    let b = p.bypasses();
    assert_eq!(b.len(), 13, "eleven modules and two lanes");
    assert!(b.contains(&("compressor".to_string(), true)));
    assert!(b.contains(&("A".to_string(), false)));
    assert!(b.contains(&("B.cabinet".to_string(), false)));
}

#[test]
fn what_is_written_reads_back_the_same() {
    let p = Preset::parse(WIDE).unwrap();
    let text = p.to_toml().unwrap();
    assert!(
        text.contains("[[stage]]")
            && text.contains("[[stage.lane]]")
            && text.contains("[[stage.lane.module]]")
    );
    assert!(!text.contains("bypass = false"), "defaults are not written");
    assert_eq!(Preset::parse(&text).unwrap(), p);
}

#[test]
fn presentation_hints_ride_along_and_are_not_values() {
    let p = Preset::parse(WIDE).unwrap();
    let capture = p.modules().find(|m| m.name == "A.capture").unwrap();
    assert_eq!(capture.ui["label"], "Plexi");
    assert!(
        !p.values().iter().any(|(a, _)| a.contains("label")),
        "hints are not values"
    );
    assert_eq!(Preset::parse(&p.to_toml().unwrap()).unwrap(), p);
}

#[test]
fn an_empty_preset_round_trips() {
    let p = Preset::new("nothing yet");
    let text = p.to_toml().unwrap();
    assert_eq!(Preset::parse(&text).unwrap(), p);
    assert!(p.values().is_empty() && p.bypasses().is_empty() && p.modules().next().is_none());
}

#[test]
fn a_preset_built_in_code_round_trips() {
    let mut p = Preset::new("Just an amp");
    p.assets.insert(
        "amp".into(),
        Asset {
            kind: AssetKind::Nam,
            hash: asset_hash(b"pretend this is a .nam"),
            file: Some("amp.nam".into()),
            sample_rate: None,
        },
    );
    let mut capture = module("std.capture", "capture");
    capture.asset = Some("amp".into());
    p.stages.push(Stage::Module(module("std.gain", "input")));
    p.stages.push(Stage::Lanes(vec![Lane {
        name: "A".into(),
        level_db: 0.0,
        pan: 0.0,
        bypass: false,
        modules: vec![capture],
    }]));
    p.set_value("input/level_db", 3.0).unwrap();
    p.set_value("A/pan", 0.25).unwrap();
    p.set_bypass("A", true).unwrap();

    let again = Preset::parse(&p.to_toml().unwrap()).unwrap();
    assert_eq!(again, p);
    assert_eq!(
        again.values(),
        [
            ("input/level_db".to_string(), 3.0),
            ("A/level_db".to_string(), 0.0),
            ("A/pan".to_string(), 0.25)
        ]
    );
    assert_eq!(
        again.bypasses(),
        [
            ("input".to_string(), false),
            ("A".to_string(), true),
            ("capture".to_string(), false)
        ]
    );
}

#[test]
fn set_value_and_set_bypass_reach_modules_and_lanes() {
    let mut p = Preset::parse(WIDE).unwrap();
    p.set_value("gate/threshold_db", -40.0).unwrap();
    p.set_value("B.eq/treble_db", 2.0).unwrap();
    p.set_value("A/level_db", -3.0).unwrap();
    p.set_bypass("B", true).unwrap();

    assert!(
        p.values()
            .contains(&("gate/threshold_db".to_string(), -40.0))
    );
    assert!(
        p.values().contains(&("B.eq/treble_db".to_string(), 2.0)),
        "a parameter the module did not have is added"
    );
    assert!(p.values().contains(&("A/level_db".to_string(), -3.0)));
    assert!(p.bypasses().contains(&("B".to_string(), true)));
    assert_eq!(
        Preset::parse(&p.to_toml().unwrap()).unwrap(),
        p,
        "still writable after being changed"
    );
}

#[test]
fn every_finite_value_survives_the_writer_exactly() {
    let hard = [
        0.1f64 + 0.2,
        1.0 / 3.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        -0.0,
        5e-324,
        -1e-14,
        1e16,
        -0.5,
    ];
    let mut m = module("std.gain", "m");
    for (i, v) in hard.iter().enumerate() {
        m.params.insert(format!("p{i}"), *v);
    }
    let mut p = Preset::new("precision");
    p.stages.push(Stage::Module(m));

    let again = Preset::parse(&p.to_toml().unwrap()).unwrap();
    assert_eq!(again, p);
    for (i, v) in hard.iter().enumerate() {
        let (_, written) = again
            .values()
            .into_iter()
            .find(|(a, _)| a == &format!("m/p{i}"))
            .unwrap();
        assert_eq!(written.to_bits(), v.to_bits(), "p{i}");
    }
}

/// The duplicate-name check sees every name in the largest document a
/// reader will accept. A linear scan per name would make this quadratic; a
/// set makes it not. Anything larger is refused outright, so this is the
/// worst case there is.
#[test]
fn the_largest_allowed_document_is_checked_without_going_quadratic() {
    let mut text = String::from("format_version = 1\nname = \"big\"\n");
    for i in 0..MAX_STAGES {
        text.push_str(&format!(
            "[[stage]]\ntype = \"std.gain\"\nname = \"m{i}\"\n"
        ));
    }
    let started = std::time::Instant::now();
    let p = Preset::parse(&text).unwrap();
    let took = started.elapsed();
    assert_eq!(p.modules().count(), MAX_STAGES);
    assert!(
        took < std::time::Duration::from_millis(200),
        "the largest allowed document took {took:?} to parse and check"
    );

    text.push_str("[[stage]]\ntype = \"std.gain\"\nname = \"m0\"\n");
    let e = Preset::parse(&text).unwrap_err();
    assert!(
        matches!(e, Error::TooMany { what: "stages", .. }),
        "one stage over the limit is refused before the duplicate is reached: {e}"
    );
}
