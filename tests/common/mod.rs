//! What the integration tests share: signals and the measurements taken of
//! them, block schedules and the runners that apply them to a stage or to a
//! rack, a bit-for-bit comparison, the reference captures, and the two
//! chains the rack tests are built from.
//!
//! Each test binary compiles the whole of this module and uses part of it,
//! which is what the allow below is for: an unused helper here means some
//! other test needs it, not that it is dead.
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

/// The rate the stage tests run at, unless a test says otherwise.
pub const SR: f64 = 48_000.0;

/// Block schedules that never line up with anything: single frames, a prime,
/// a Fibonacci run, a long-short alternation, and two either side of 64.
pub const SCHEDULES: [&[usize]; 5] = [
    &[1],
    &[7],
    &[1, 2, 3, 5, 8, 13, 21, 34, 55, 89],
    &[500, 1],
    &[63, 65],
];

/// Samples in `t` milliseconds at [`SR`].
pub fn ms(t: f32) -> usize {
    (t * (SR as f32) / 1000.0).round() as usize
}

/// The linear factor for `db` decibels.
pub fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// `ratio` as decibels.
pub fn to_db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}

/// A sine of `seconds` at [`SR`].
pub fn sine(hz: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
    sine_at(hz as f64, amplitude, seconds as f64, SR)
}

/// A sine of `seconds` at an arbitrary rate.
pub fn sine_at(hz: f64, amplitude: f32, seconds: f64, sample_rate: f64) -> Vec<f32> {
    (0..(seconds * sample_rate) as usize)
        .map(|i| ((i as f64 / sample_rate * std::f64::consts::TAU * hz).sin() as f32) * amplitude)
        .collect()
}

/// splitmix64 as `f32` in `[-scale, scale)`, exactly representable.
pub fn noise(seed: u64, n: usize, scale: f32) -> Vec<f32> {
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

/// A single full-scale sample, then silence.
pub fn impulse(n: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    x
}

/// A steady level of `level_db` dBFS, `n` frames long.
pub fn dc(level_db: f32, n: usize) -> Vec<f32> {
    vec![from_db(level_db); n]
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// The magnitude at `hz` over `x`, as a fraction of full scale, by
/// Goertzel's algorithm. `x` must hold a whole number of periods at `hz`.
pub fn goertzel(x: &[f32], hz: f64, sample_rate: f64) -> f32 {
    let w = std::f64::consts::TAU * hz / sample_rate;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - c * s1 * s2;
    (2.0 * power.max(0.0).sqrt() / x.len() as f64) as f32
}

/// Run `x` through `step` in blocks drawn cyclically from `blocks`, and
/// return everything it produced.
pub fn run(x: &[f32], blocks: &[usize], mut step: impl FnMut(&[f32], &mut [f32])) -> Vec<f32> {
    let mut y = vec![0.0f32; x.len()];
    let (mut pos, mut i) = (0usize, 0usize);
    while pos < x.len() {
        let n = blocks[i % blocks.len()].max(1).min(x.len() - pos);
        i += 1;
        step(&x[pos..pos + n], &mut y[pos..pos + n]);
        pos += n;
    }
    y
}

/// Every sample of `got` is `want` down to the last bit, `-0.0` and `0.0`
/// counted as different.
pub fn assert_same_bits(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(
        got.len(),
        want.len(),
        "{what}: {} samples against {}",
        got.len(),
        want.len()
    );
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!(
            g.to_bits(),
            w.to_bits(),
            "{what}: sample {i} is {g} not {w}"
        );
    }
}

/// Splitting `x` into blocks any way at all gives byte-identical output.
///
/// `fresh` must hand back a stage in the same state every time; every
/// schedule in [`SCHEDULES`] is compared against a run in blocks of 512.
pub fn independent_of_block_grouping<S>(
    x: &[f32],
    mut fresh: impl FnMut() -> S,
    mut process: impl FnMut(&mut S, &[f32], &mut [f32]),
) {
    let mut first = fresh();
    let reference = run(x, &[512], |i, o| process(&mut first, i, o));
    for blocks in SCHEDULES {
        let mut stage = fresh();
        let got = run(x, blocks, |i, o| process(&mut stage, i, o));
        assert_same_bits(&got, &reference, &format!("schedule {blocks:?}"));
    }
}

/// A cabinet-shaped IR: noise under an exponential decay.
pub fn cabinet(seed: u64, len: usize) -> Vec<f32> {
    let tau = (len as f32 / 4.0).max(1.0);
    noise(seed, len, 0.5)
        .into_iter()
        .enumerate()
        .map(|(i, v)| v * (-(i as f32) / tau).exp())
        .collect()
}

/// One of the reference captures in `assets/models`: copies of fixtures
/// from `valverig-nam`, whose README says where each one comes from.
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

/// A decaying noise burst, standing in for a cabinet's impulse response.
pub fn impulse_response(taps: usize, seed: u64) -> Vec<f32> {
    noise(seed, taps, 0.3)
        .iter()
        .enumerate()
        .map(|(i, v)| v * (-(i as f32) / 400.0).exp())
        .collect()
}

/// Run `input` through the rack in blocks drawn cyclically from
/// `schedule`, returning one output per channel. A ragged schedule is how
/// a test shows that the output does not depend on how frames are grouped.
pub fn run_rack(rack: &mut Rack, input: &[f32], schedule: &[usize]) -> Vec<Vec<f32>> {
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

/// The longest a rack's own gain glide takes to land exactly on its target,
/// with room to spare: from +24 dB to −60 dB takes 83 ms.
pub const LANDED_MS: f64 = 90.0;

/// Two copies of a rack in step, one with `address` moved to `away` and then
/// back to `home`, each given [`LANDED_MS`] to land, the other left alone.
/// Returns the next block from each, still first: a glide that lands on its
/// target leaves the two the same to the bit.
pub fn moved_and_back(
    build: impl Fn() -> (Rack, Controls),
    address: &str,
    away: f32,
    home: f32,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let block = 128;
    let x = noise(11, block, 0.5);
    let (mut still, _) = build();
    let (mut moved, controls) = build();
    still.prepare(SR, block).unwrap();
    moved.prepare(SR, block).unwrap();
    let landing = (LANDED_MS * 1e-3 * SR / block as f64).ceil() as usize;
    for value in [away, home] {
        controls.set(address, value).unwrap();
        for _ in 0..landing {
            run_rack(&mut still, &x, &[block]);
            run_rack(&mut moved, &x, &[block]);
        }
    }
    (
        run_rack(&mut still, &x, &[block]),
        run_rack(&mut moved, &x, &[block]),
    )
}
