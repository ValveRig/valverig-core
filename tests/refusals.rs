//! What a bad document earns, and that the message names the part at fault.
//!
//! A `.rig` is untrusted input: it arrives with a preset someone downloaded,
//! or from a text editor at two in the morning. Every case here must come
//! back as an [`Error`], never as a panic, an abort, or a silent acceptance.

use std::collections::BTreeMap;
use valrig_core::Error;
use valrig_core::preset::{Lane, Module, Preset, Stage};

const HEAD: &str = "format_version = 1\nname = \"x\"\n";
const HASH: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";

/// The error a document body earns, on top of a valid header.
fn refuse(body: &str) -> Error {
    let text = format!("{HEAD}{body}");
    Preset::parse(&text)
        .map(|_| ())
        .expect_err(&format!("should have been refused:\n{text}"))
}

/// A document body that must be accepted, so a refusal test cannot pass by
/// rejecting everything.
fn accept(body: &str) {
    let text = format!("{HEAD}{body}");
    Preset::parse(&text).unwrap_or_else(|e| panic!("should have been accepted: {e}\n{text}"));
}

#[test]
fn only_this_version_is_read_or_written() {
    for (found, document) in [
        (2u32, "format_version = 2\nname = \"x\"\n"),
        (0, "format_version = 0\nname = \"x\"\n"),
    ] {
        let e = Preset::parse(document).unwrap_err();
        assert!(
            matches!(e, Error::Version { found: f, supported: 1 } if f == found),
            "{e}"
        );
    }

    // A version the writer would refuse cannot escape through the writer.
    let mut p = Preset::new("from the future");
    p.format_version = 99;
    assert!(matches!(
        p.to_toml(),
        Err(Error::Version {
            found: 99,
            supported: 1
        })
    ));
    assert!(matches!(
        p.validate(),
        Err(Error::Version {
            found: 99,
            supported: 1
        })
    ));
}

#[test]
fn a_document_that_is_not_toml_is_told_where() {
    let e = Preset::parse("format_version = 1\nname = \"x\"\n[[stage]\n").unwrap_err();
    assert!(matches!(&e, Error::Toml(m) if m.contains('3')), "{e}");
    assert!(matches!(Preset::parse(""), Err(Error::Toml(_))));
    assert!(
        matches!(Preset::parse("name = \"x\"\n"), Err(Error::Toml(_))),
        "format_version is required"
    );
    assert!(
        matches!(Preset::parse("format_version = 1\n"), Err(Error::Toml(_))),
        "name is required"
    );
}

/// A key this reader does not know is refused, not dropped. Accepting it
/// would mean deleting it on the next save.
#[test]
fn a_key_this_reader_does_not_know_is_refused() {
    for body in [
        "something_new = 7\n",
        "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\nbipass = true\n",
        "[[stage]]\n[[stage.lane]]\nname = \"A\"\nwidth = 2.0\n",
        "[[stage]]\n[[stage.lane]]\nname = \"A\"\n[[stage.lane.module]]\ntype = \"std.gain\"\nname = \"m\"\ncolour = \"red\"\n",
        &format!("[assets.a]\nkind = \"nam\"\nhash = \"{HASH}\"\nsize = 12\n"),
    ] {
        assert!(matches!(refuse(body), Error::Toml(_)), "{body:?}");
    }
}

#[test]
fn a_stage_is_a_module_or_lanes_and_says_which() {
    let e = refuse("[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[[stage.lane]]\nname = \"A\"\n");
    assert!(
        matches!(&e, Error::Shape(m) if m.contains("stage 0") && m.contains("not both")),
        "{e}"
    );

    let e = refuse("[[stage]]\nbypass = true\n");
    assert!(
        matches!(&e, Error::Shape(m) if m.contains("stage 0") && m.contains("neither")),
        "{e}"
    );

    let e = refuse("[[stage]]\ntype = \"std.gate\"\n");
    assert!(
        matches!(&e, Error::Shape(m) if m.contains("needs a `name`")),
        "{e}"
    );

    let e = refuse("[[stage]]\nname = \"g\"\n");
    assert!(
        matches!(&e, Error::Shape(m) if m.contains("needs a `type`")),
        "{e}"
    );
}

#[test]
fn a_name_is_unique_and_can_be_addressed() {
    let e = refuse(
        "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[[stage]]\ntype = \"std.gain\"\nname = \"g\"\n",
    );
    assert!(matches!(&e, Error::DuplicateName(n) if n == "g"), "{e}");

    let e = refuse(
        "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[[stage]]\n[[stage.lane]]\nname = \"g\"\n",
    );
    assert!(
        matches!(&e, Error::DuplicateName(n) if n == "g"),
        "lanes share the namespace: {e}"
    );

    for bad in ["", "in/out", "a\\nb", "a\\u0000b", &"n".repeat(129)] {
        let e = refuse(&format!(
            "[[stage]]\ntype = \"std.gate\"\nname = \"{bad}\"\n"
        ));
        assert!(matches!(&e, Error::BadName(_)), "{bad:?}: {e}");
    }

    accept("[[stage]]\ntype = \"std.gate\"\nname = \"A.capture\"\n");
    accept("[[stage]]\ntype = \"std.gate\"\nname = \"Ampli·Té\"\n");
}

#[test]
fn a_module_type_is_namespaced_and_lowercase() {
    for bad in [
        "gate",
        "Std.Gate",
        "std.",
        ".gate",
        "std.gate.extra",
        "std gate",
        "",
    ] {
        let e = refuse(&format!("[[stage]]\ntype = \"{bad}\"\nname = \"g\"\n"));
        assert!(matches!(&e, Error::BadType(t) if t == bad), "{bad:?}: {e}");
    }
    accept("[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n");
}

#[test]
fn a_parameter_id_and_a_hint_key_are_identifiers() {
    for bad in ["", "a/b", "Threshold", "a.b"] {
        let e = refuse(&format!(
            "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.params]\n\"{bad}\" = 1.0\n"
        ));
        assert!(
            matches!(&e, Error::BadParamId { module, id } if module == "g" && id == bad),
            "{bad:?}: {e}"
        );

        let e = refuse(&format!(
            "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.ui]\n\"{bad}\" = \"v\"\n"
        ));
        assert!(
            matches!(&e, Error::BadUiKey { module, key } if module == "g" && key == bad),
            "{bad:?}: {e}"
        );
    }
    accept("[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.params]\nthreshold_db = -50.0\n");
    accept("[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.ui]\nlabel = \"Quiet Room\"\n");
}

#[test]
fn every_number_is_finite() {
    let e = refuse(
        "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.params]\nthreshold_db = nan\n",
    );
    assert!(
        matches!(&e, Error::NotFinite(a) if a == "g/threshold_db"),
        "{e}"
    );

    let e = refuse(
        "[[stage]]\ntype = \"std.gate\"\nname = \"g\"\n[stage.params]\nthreshold_db = inf\n",
    );
    assert!(
        matches!(&e, Error::NotFinite(a) if a == "g/threshold_db"),
        "{e}"
    );

    let e = refuse("[[stage]]\n[[stage.lane]]\nname = \"A\"\npan = inf\n");
    assert!(matches!(&e, Error::NotFinite(a) if a == "A/pan"), "{e}");

    let e = refuse("[[stage]]\n[[stage.lane]]\nname = \"A\"\nlevel_db = nan\n");
    assert!(
        matches!(&e, Error::NotFinite(a) if a == "A/level_db"),
        "{e}"
    );

    // A range is the engine's business, not the format's.
    accept("[[stage]]\n[[stage.lane]]\nname = \"A\"\npan = 7.0\n");
}

#[test]
fn an_asset_label_is_an_identifier_and_its_hash_is_an_address() {
    for bad in ["", "Plexi", "a/b", "a.b"] {
        let e = refuse(&format!(
            "[assets.\"{bad}\"]\nkind = \"nam\"\nhash = \"{HASH}\"\n"
        ));
        assert!(
            matches!(&e, Error::BadAssetLabel(l) if l == bad),
            "{bad:?}: {e}"
        );
    }

    for bad in [
        "sha256:00",
        "blake3:abc",
        "blake3:",
        &HASH.to_uppercase(),
        &HASH["blake3:".len()..],
    ] {
        let e = refuse(&format!("[assets.a]\nkind = \"nam\"\nhash = \"{bad}\"\n"));
        assert!(matches!(&e, Error::BadHash(h) if h == bad), "{bad:?}: {e}");
    }

    for bad in ["wav", "NAM", ""] {
        let e = refuse(&format!(
            "[assets.a]\nkind = \"{bad}\"\nhash = \"{HASH}\"\n"
        ));
        assert!(
            matches!(&e, Error::BadAssetKind(k) if k == bad),
            "{bad:?}: {e}"
        );
    }
}

/// A document says what an asset *is*, never where it lives. A hint that
/// could be a path would let a preset from the internet choose which file
/// every host that resolves it opens.
#[test]
fn a_file_hint_cannot_name_a_path() {
    for bad in [
        "/etc/passwd",
        "../../../etc/passwd",
        "sub/dir/a.nam",
        "a\\b.nam",
        "C:\\Windows\\System32",
        "",
        ".",
        "..",
    ] {
        let e = refuse(&format!(
            "[assets.a]\nkind = \"nam\"\nhash = \"{HASH}\"\nfile = {bad:?}\n"
        ));
        assert!(
            matches!(&e, Error::BadFile { asset, file } if asset == "a" && file == bad),
            "{bad:?}: {e}"
        );
    }
    accept(&format!(
        "[assets.a]\nkind = \"nam\"\nhash = \"{HASH}\"\nfile = \"brit-plexi-50-v2.nam\"\n"
    ));
    accept(&format!(
        "[assets.a]\nkind = \"ir\"\nhash = \"{HASH}\"\nfile = \"Cab 4x12.wav\"\n"
    ));
}

#[test]
fn a_stated_sample_rate_is_plausible_and_belongs_to_an_impulse_response() {
    for bad in ["0.0", "-48000.0", "nan", "inf", "1e300"] {
        let e = refuse(&format!(
            "[assets.a]\nkind = \"ir\"\nhash = \"{HASH}\"\nsample_rate = {bad}\n"
        ));
        assert!(
            matches!(&e, Error::BadSampleRate { asset, .. } if asset == "a"),
            "{bad}: {e}"
        );
    }

    let e = refuse(&format!(
        "[assets.a]\nkind = \"nam\"\nhash = \"{HASH}\"\nsample_rate = 48000.0\n"
    ));
    assert!(
        matches!(&e, Error::UnwantedSampleRate(l) if l == "a"),
        "a capture carries its own rate: {e}"
    );

    accept(&format!(
        "[assets.a]\nkind = \"ir\"\nhash = \"{HASH}\"\nsample_rate = 48000.0\n"
    ));
    accept(&format!("[assets.a]\nkind = \"nam\"\nhash = \"{HASH}\"\n"));
}

#[test]
fn a_module_can_only_name_an_asset_the_preset_carries() {
    let e = refuse("[[stage]]\ntype = \"std.capture\"\nname = \"amp\"\nasset = \"plexi\"\n");
    assert!(
        matches!(&e, Error::UnknownAsset { module, asset } if module == "amp" && asset == "plexi"),
        "{e}"
    );

    accept(&format!(
        "[assets.plexi]\nkind = \"nam\"\nhash = \"{HASH}\"\n\n[[stage]]\ntype = \"std.capture\"\nname = \"amp\"\nasset = \"plexi\"\n"
    ));
    accept(&format!(
        "[assets.plexi]\nkind = \"nam\"\nhash = \"{HASH}\"\n"
    ));
}

#[test]
fn a_parallel_stage_has_lanes_in_it() {
    let mut p = Preset::new("x");
    p.stages.push(valrig_core::preset::Stage::Lanes(Vec::new()));
    let e = p.validate().unwrap_err();
    assert!(
        matches!(&e, Error::Shape(m) if m.contains("at least one lane")),
        "{e}"
    );
}

#[test]
fn setting_a_value_refuses_what_it_cannot_reach() {
    let mut p = Preset::parse(include_str!("../assets/plexi-tweed-wide.rig")).unwrap();
    assert!(
        matches!(p.set_value("A/width", 1.0), Err(Error::Shape(m)) if m.contains("level_db and pan"))
    );
    assert!(
        matches!(p.set_value("nothing/here", 1.0), Err(Error::UnknownName(n)) if n == "nothing")
    );
    assert!(
        matches!(p.set_value("gate", 1.0), Err(Error::Shape(m)) if m.contains("name/parameter"))
    );
    assert!(matches!(
        p.set_value("gate/threshold_db", f64::NAN),
        Err(Error::NotFinite(_))
    ));
    assert!(matches!(
        p.set_value("gate/threshold_db", f64::INFINITY),
        Err(Error::NotFinite(_))
    ));
    assert!(matches!(
        p.set_bypass("nothing", true),
        Err(Error::UnknownName(_))
    ));
}

#[test]
fn a_preset_that_declares_more_than_a_reader_will_build_is_refused() {
    use valrig_core::preset::{MAX_LANES, MAX_MODULES, MAX_STAGES};
    let module = |name: String| Module {
        type_id: "std.gain".into(),
        name,
        asset: None,
        bypass: false,
        params: BTreeMap::new(),
        ui: BTreeMap::new(),
    };

    // Too many stages, each one a module.
    let mut p = Preset::new("wide");
    for i in 0..=MAX_STAGES {
        p.stages.push(Stage::Module(module(format!("g{i}"))));
    }
    assert!(
        matches!(p.validate(), Err(Error::TooMany { what: "stages", max, .. }) if max == MAX_STAGES),
        "{:?}",
        p.validate()
    );

    // Too many lanes in one stage.
    let mut p = Preset::new("fan");
    let lanes = (0..=MAX_LANES)
        .map(|i| Lane {
            name: format!("l{i}"),
            level_db: 0.0,
            pan: 0.0,
            bypass: false,
            modules: Vec::new(),
        })
        .collect();
    p.stages.push(Stage::Lanes(lanes));
    assert!(matches!(
        p.validate(),
        Err(Error::TooMany {
            what: "lanes in one stage",
            ..
        })
    ));

    // Few enough stages and lanes, but too many modules between them.
    let mut p = Preset::new("deep");
    let per_lane = MAX_MODULES / 8 + 1;
    let lanes = (0..8)
        .map(|l| Lane {
            name: format!("l{l}"),
            level_db: 0.0,
            pan: 0.0,
            bypass: false,
            modules: (0..per_lane).map(|i| module(format!("m{l}_{i}"))).collect(),
        })
        .collect();
    p.stages.push(Stage::Lanes(lanes));
    assert!(matches!(
        p.validate(),
        Err(Error::TooMany {
            what: "modules",
            ..
        })
    ));
}
