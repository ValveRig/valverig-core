//! A rack from the wide-rig fixture, assets resolved by content;
//! the values landing on the controls; the state written back and read
//! again; and the refusals, each naming what is wrong.

use std::path::{Path, PathBuf};
use valrig_core::Error;
use valrig_core::preset::{
    self, Asset, AssetKind, BuildOptions, Loaded, Preset, ResolveError, Stage, asset_hash,
};
use valrig_nam::loader::Model;

const FIXTURE: &str = "assets/plexi-tweed-wide.rig";

/// Where the resolver looks for the capture files the preset's asset hints
/// name: the reference captures in `assets/models`.
fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models")
}

fn fixture() -> (Preset, PathBuf) {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)).unwrap();
    (Preset::parse(&text).unwrap(), models())
}

/// The resolver a host would write. A capture is the file the hint names,
/// accepted only if its bytes hash to the address. An IR here is
/// synthesised: the label is all the preset promises about it.
fn resolver(dir: PathBuf) -> impl FnMut(&str, &Asset) -> Result<Loaded, ResolveError> {
    move |label, asset| match asset.kind {
        AssetKind::Nam => {
            let file = asset.file.as_deref().ok_or("no file to look in")?;
            let bytes = std::fs::read(dir.join(file))?;
            if asset_hash(&bytes) != asset.hash {
                return Err(
                    format!("{file} is not {label}: its content hashes differently").into(),
                );
            }
            Ok(Loaded::Capture(Model::from_json(std::str::from_utf8(
                &bytes,
            )?)?))
        }
        AssetKind::Ir => Ok(Loaded::Ir {
            samples: (0..64).map(|i| 0.5f32.powi(i)).collect(),
            sample_rate: asset.sample_rate,
        }),
    }
}

#[test]
fn the_wide_rig_builds_from_its_preset() {
    let (p, dir) = fixture();
    let (mut rack, controls) =
        preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap();
    let modules = controls.modules();
    assert_eq!(modules.len(), 13, "eleven modules and two lanes");
    assert!(modules.contains(&("A.capture".to_string(), "std.capture")));
    assert!(modules.contains(&("A".to_string(), "std.lane")));
    assert!(modules.contains(&("B.cabinet".to_string(), "std.cabinet")));
    assert_eq!(controls.get("gate/threshold_db").unwrap(), -50.0);
    assert_eq!(controls.get("A.eq/bass_db").unwrap(), 1.5);
    assert_eq!(
        controls.get("B.eq/bass_db").unwrap(),
        0.0,
        "unset is the default"
    );
    assert_eq!(controls.get("A/pan").unwrap(), -1.0);
    assert_eq!(controls.get("B/pan").unwrap(), 1.0);
    assert_eq!(controls.get("B/level_db").unwrap(), -1.0);
    assert_eq!(controls.get("reverb/mix").unwrap(), 0.2);
    assert_eq!(controls.get("output/level_db").unwrap(), -6.0);
    assert!(controls.bypassed("compressor").unwrap());
    assert!(!controls.bypassed("reverb").unwrap());
    assert!(!controls.bypassed("A").unwrap());

    rack.prepare(48_000.0, 128).unwrap();
    assert_eq!(rack.channels_out(), 2);
    let mut input = vec![0.0f32; 128];
    input[0] = 0.5;
    let (mut left, mut right) = (vec![0.0f32; 128], vec![0.0f32; 128]);
    for _ in 0..8 {
        rack.process(&input, &mut [&mut left, &mut right]);
        input.fill(0.0);
    }
    assert!(left.iter().chain(&right).all(|v| v.is_finite()));
    assert!(
        left.iter().any(|v| *v != 0.0) || right.iter().any(|v| *v != 0.0),
        "something came out"
    );
}

#[test]
fn the_state_written_back_reads_again_as_the_same_rack() {
    let (mut p, dir) = fixture();
    let (_, controls) =
        preset::build(&p, &mut resolver(dir.clone()), BuildOptions::default()).unwrap();
    controls.set("gate/threshold_db", -35.0).unwrap();
    controls.set("B/pan", 0.5).unwrap();
    controls.set("B.eq/treble_db", 2.0).unwrap();
    controls.set_bypass("reverb", true).unwrap();
    controls.set_bypass("compressor", false).unwrap();
    preset::snapshot(&mut p, &controls).unwrap();

    let values = p.values();
    assert!(values.contains(&("gate/threshold_db".to_string(), -35.0)));
    assert!(values.contains(&("B/pan".to_string(), 0.5)));
    assert!(values.contains(&("B.eq/treble_db".to_string(), 2.0)));
    for (address, _, _) in controls.params() {
        assert!(
            values.iter().any(|(a, _)| *a == address),
            "{address} was not written back"
        );
    }
    assert!(p.bypasses().contains(&("reverb".to_string(), true)));
    assert!(p.bypasses().contains(&("compressor".to_string(), false)));

    let text = p.to_toml().unwrap();
    let again = Preset::parse(&text).unwrap();
    let (_, rebuilt) = preset::build(&again, &mut resolver(dir), BuildOptions::default()).unwrap();
    for (address, _, value) in controls.params() {
        assert_eq!(
            rebuilt.get(&address).unwrap(),
            value,
            "{address} through the file"
        );
    }
    for (name, _) in controls.modules() {
        assert_eq!(
            rebuilt.bypassed(&name).unwrap(),
            controls.bypassed(&name).unwrap(),
            "{name} bypass through the file"
        );
    }
}

#[test]
fn a_parameter_the_module_does_not_have_is_refused_by_address() {
    let (mut p, dir) = fixture();
    p.set_value("gate/knee", 1.0).unwrap();
    let e = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap_err();
    assert!(
        matches!(&e, Error::UnknownParam(a) if a.contains("knee")),
        "{e}"
    );
}

#[test]
fn a_value_out_of_range_is_clamped_as_any_set_is() {
    let (mut p, dir) = fixture();
    p.set_value("A/pan", 7.0).unwrap();
    p.set_value("gate/threshold_db", -500.0).unwrap();
    let (_, controls) = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap();
    assert_eq!(controls.get("A/pan").unwrap(), 1.0);
    assert_eq!(
        controls.get("gate/threshold_db").unwrap(),
        controls.param("gate/threshold_db").unwrap().spec().min
    );
}

#[test]
fn a_type_the_engine_does_not_have_is_refused_by_name() {
    let (mut p, dir) = fixture();
    if let Stage::Module(m) = &mut p.stages[1] {
        m.type_id = "std.wah".into();
    }
    let e = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap_err();
    assert!(
        matches!(&e, Error::UnknownType { module, type_id } if module == "gate" && type_id == "std.wah"),
        "{e}"
    );
}

#[test]
fn a_capture_wants_a_capture_and_a_cabinet_an_ir() {
    let (mut p, dir) = fixture();
    p.assets.get_mut("plexi").unwrap().kind = AssetKind::Ir;
    let e = preset::build(&p, &mut resolver(dir.clone()), BuildOptions::default()).unwrap_err();
    assert!(
        matches!(
            &e,
            Error::WrongAssetKind { module, asset, needs, found }
                if module == "A.capture"
                    && asset == "plexi"
                    && *needs == AssetKind::Nam
                    && *found == AssetKind::Ir
        ),
        "{e}"
    );

    let (mut p, _) = fixture();
    let cab = p.assets["plexi"].clone();
    p.assets.insert("cab".into(), cab);
    let e = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap_err();
    assert!(
        matches!(
            &e,
            Error::WrongAssetKind { module, needs, found, .. }
                if module == "A.cabinet"
                    && *needs == AssetKind::Ir
                    && *found == AssetKind::Nam
        ),
        "{e}"
    );
}

#[test]
fn a_wrong_file_is_caught_by_its_hash() {
    let (mut p, dir) = fixture();
    let tweed = p.assets["tweed"].file.clone();
    p.assets.get_mut("plexi").unwrap().file = tweed;
    let e = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap_err();
    assert!(
        matches!(
            &e,
            Error::Resolve { module, asset, source }
                if module == "A.capture"
                    && asset == "plexi"
                    && source.to_string().contains("hashes differently")
        ),
        "{e}"
    );
}

#[test]
fn a_resolver_that_cannot_names_the_module_and_the_label() {
    let (p, _) = fixture();
    let e = preset::build(
        &p,
        &mut |_: &str, _: &Asset| Err::<Loaded, ResolveError>("offline".into()),
        BuildOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(
            &e,
            Error::Resolve { module, asset, source }
                if module == "A.capture"
                    && asset == "plexi"
                    && source.to_string() == "offline"
        ),
        "{e}"
    );
}

#[test]
fn the_format_checks_run_first() {
    let (mut p, dir) = fixture();
    if let Stage::Module(m) = &mut p.stages[0] {
        m.name = "gate".into();
    }
    let e = preset::build(&p, &mut resolver(dir), BuildOptions::default()).unwrap_err();
    assert!(matches!(&e, Error::DuplicateName(n) if n == "gate"), "{e}");
}

#[test]
fn the_pedals_and_the_tap_build_from_their_types_and_write_back() {
    use std::collections::BTreeMap;
    use valrig_core::preset::Module;
    let module = |type_id: &str, name: &str| Module {
        type_id: type_id.into(),
        name: name.into(),
        asset: None,
        bypass: false,
        params: BTreeMap::new(),
        ui: BTreeMap::new(),
    };
    let mut p = Preset::new("pedals");
    for (t, n) in [
        ("std.drive", "drive"),
        ("std.send", "send"),
        ("std.tap", "tap"),
        ("std.chorus", "chorus"),
        ("std.phaser", "phaser"),
        ("std.delay", "delay"),
        ("std.return", "return"),
    ] {
        p.stages.push(Stage::Module(module(t, n)));
    }
    p.set_value("drive/drive_db", 20.0).unwrap();
    p.set_value("delay/time_ms", 250.0).unwrap();
    p.set_value("return/bus", 1.0).unwrap();
    let (mut rack, controls) = preset::build(
        &p,
        &mut |_, _| Err("no assets here".into()),
        BuildOptions::default(),
    )
    .unwrap();
    let modules = controls.modules();
    assert_eq!(modules.len(), 7);
    assert!(modules.contains(&("chorus".to_string(), "std.chorus")));
    assert!(modules.contains(&("phaser".to_string(), "std.phaser")));
    assert!(modules.contains(&("tap".to_string(), "std.tap")));
    assert!(modules.contains(&("send".to_string(), "std.send")));
    assert!(modules.contains(&("return".to_string(), "std.return")));
    assert_eq!(controls.get("drive/drive_db").unwrap(), 20.0);
    assert_eq!(controls.get("delay/time_ms").unwrap(), 250.0);
    assert_eq!(controls.get("chorus/mix").unwrap(), 0.5, "the default");
    assert_eq!(controls.get("tap/pan").unwrap(), 0.0);
    rack.prepare(48_000.0, 64).unwrap();
    let input = vec![0.2f32; 64];
    let mut out = vec![0.0f32; 64];
    for _ in 0..4 {
        rack.process(&input, &mut [&mut out]);
    }
    assert!(out.iter().all(|v| v.is_finite()) && out.iter().any(|v| *v != 0.0));
    controls.set("chorus/rate_hz", 2.5).unwrap();
    controls.set_bypass("tap", true).unwrap();
    preset::snapshot(&mut p, &controls).unwrap();
    let again = Preset::parse(&p.to_toml().unwrap()).unwrap();
    let values: std::collections::BTreeMap<String, f64> = again.values().into_iter().collect();
    assert_eq!(values["chorus/rate_hz"], 2.5);
    assert_eq!(values["drive/drive_db"], 20.0);
    assert!(again.bypasses().iter().any(|(n, on)| n == "tap" && *on));
}

#[test]
fn a_host_can_ask_for_every_capture_at_one_level() {
    let (p, dir) = fixture();
    let plain = preset::build(&p, &mut resolver(dir.clone()), BuildOptions::default()).unwrap();
    let normalized = preset::build(
        &p,
        &mut resolver(dir),
        BuildOptions {
            normalize_captures_to_dbfs: Some(-18.0),
        },
    )
    .unwrap();
    let (mut a, _) = plain;
    let (mut b, _) = normalized;
    a.prepare(48_000.0, 128).unwrap();
    b.prepare(48_000.0, 128).unwrap();
    let input: Vec<f32> = (0..128)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
        .collect();
    let (mut la, mut ra, mut lb, mut rb) = (
        vec![0.0f32; 128],
        vec![0.0f32; 128],
        vec![0.0f32; 128],
        vec![0.0f32; 128],
    );
    for _ in 0..8 {
        a.process(&input, &mut [&mut la, &mut ra]);
        b.process(&input, &mut [&mut lb, &mut rb]);
    }
    // A capture that states its loudness is scaled; one that does not is
    // left alone, and then the two racks agree exactly.
    let stated: Vec<Option<f64>> = p
        .assets
        .values()
        .filter(|a| a.kind == AssetKind::Nam)
        .map(|a| {
            Model::from_file(models().join(a.file.as_deref().unwrap()))
                .unwrap()
                .metadata
                .loudness
        })
        .collect();
    let same = la.iter().zip(&lb).all(|(x, y)| x.to_bits() == y.to_bits());
    assert_eq!(
        same,
        stated.iter().all(|l| l.is_none()),
        "stated loudness {stated:?}: the racks {}",
        if same { "agree" } else { "differ" }
    );
    assert!(lb.iter().all(|v| v.is_finite()));
    let mut c = valrig_core::capture::Capture::new(
        Model::from_file(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/wavenet.nam"))
            .unwrap(),
    );
    let stated = c.loudness();
    c = c.normalized(-18.0);
    let want = stated.map(|l| -18.0 - l as f32).unwrap_or(0.0);
    assert!(
        (c.gain_db() - want).abs() < 1e-3,
        "gain {} dB for a stated loudness of {stated:?}",
        c.gain_db()
    );
    c.set_gain_db(-6.0206);
    assert!((c.gain_db() + 6.0206).abs() < 1e-3);
}
