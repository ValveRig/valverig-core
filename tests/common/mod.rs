//! What the integration tests share: the reference captures, a repeatable
//! signal, a block-schedule runner, a bit-for-bit comparison, and the two
//! chains the tests exercise the rack with.
//!
//! Each test binary uses a part of this, so unused items here are expected.
#![allow(dead_code)]

use std::path::Path;

use valverig_core::cabinet::Cabinet;
use valverig_core::capture::Capture;
use valverig_core::compressor::Compressor;
use valverig_core::eq::Tone;
use valverig_core::gain::Gain;
use valverig_core::gate::Gate;
use valverig_core::rack::{Controls, Lane, Rack};
use valverig_core::reverb::Reverb;
use valverig_nam::loader::Model;

/// The reference captures live in `valverig-nam`, which is where they are
/// documented and checked; the tests here read them from the sibling
/// checkout rather than keeping a second copy.
pub fn model(name: &str) -> Model {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/models")
        .join(name);
    Model::from_file(&path).unwrap_or_else(|e| {
        panic!(
            "the reference capture {} would not load: {e}",
            path.display()
        )
    })
}

/// splitmix64 rendered as `f32`, so every value is exactly representable
/// and a comparison can be bit for bit. The same seed gives the same
/// signal on every machine.
pub fn signal(seed: u64, n: usize, scale: f32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            (((z >> 40) as u32) as f32 / 8_388_608.0 - 1.0) * scale
        })
        .collect()
}

/// A decaying noise burst, standing in for a cabinet's impulse response.
pub fn impulse_response(taps: usize, seed: u64) -> Vec<f32> {
    signal(seed, taps, 0.3)
        .iter()
        .enumerate()
        .map(|(i, v)| v * (-(i as f32) / 400.0).exp())
        .collect()
}

/// Run `input` through the rack in blocks drawn cyclically from
/// `schedule`, returning one output per channel. A ragged schedule is how
/// a test shows that the output does not depend on how frames are grouped.
pub fn run(rack: &mut Rack, input: &[f32], schedule: &[usize]) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; input.len()]; rack.channels_out()];
    let (mut pos, mut i) = (0, 0);
    while pos < input.len() {
        let n = schedule[i % schedule.len()].max(1).min(input.len() - pos);
        i += 1;
        let mut slices: Vec<&mut [f32]> = out.iter_mut().map(|c| &mut c[pos..pos + n]).collect();
        rack.process(&input[pos..pos + n], &mut slices);
        pos += n;
    }
    out
}

/// Fail on the first sample whose bits differ, naming it.
pub fn assert_bits(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: lengths differ");
    for (i, (a, b)) in got.iter().zip(want).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "{what}: sample {i}");
    }
}

/// A mono chain around one capture: `input` gain, `gate`, `compressor`,
/// `capture`, `eq`, `cabinet` when there is one, `reverb`, `output` gain.
pub fn chain(model: Model, cabinet: Option<Cabinet>) -> (Rack, Controls) {
    let mut builder = Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("gate", Gate::new())
        .unwrap()
        .push("compressor", Compressor::new())
        .unwrap()
        .push("capture", Capture::new(model))
        .unwrap()
        .push("eq", Tone::new())
        .unwrap();
    if let Some(cabinet) = cabinet {
        builder = builder.push("cabinet", cabinet).unwrap();
    }
    builder
        .push("reverb", Reverb::new())
        .unwrap()
        .push("output", Gain::new())
        .unwrap()
        .build()
}

/// The same front end, then one lane per capture, spread evenly from hard
/// left to hard right, then `reverb` and `output` on the pair. A lane is
/// `<name>.capture`, `<name>.eq`, and `<name>.cabinet` when there is one.
pub fn lane_chain(lanes: Vec<(&str, Model, Option<Cabinet>)>) -> (Rack, Controls) {
    let count = lanes.len();
    let mut built = Vec::with_capacity(count);
    for (i, (name, model, cabinet)) in lanes.into_iter().enumerate() {
        let pan = if count < 2 {
            0.0
        } else {
            -1.0 + 2.0 * i as f32 / (count - 1) as f32
        };
        let mut lane = Lane::new(name)
            .pan(pan)
            .push(&format!("{name}.capture"), Capture::new(model))
            .unwrap()
            .push(&format!("{name}.eq"), Tone::new())
            .unwrap();
        if let Some(cabinet) = cabinet {
            lane = lane.push(&format!("{name}.cabinet"), cabinet).unwrap();
        }
        built.push(lane);
    }
    Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("gate", Gate::new())
        .unwrap()
        .push("compressor", Compressor::new())
        .unwrap()
        .split(built)
        .unwrap()
        .push("reverb", Reverb::new())
        .unwrap()
        .push("output", Gain::new())
        .unwrap()
        .build()
}
