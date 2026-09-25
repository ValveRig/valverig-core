//! The drive against its own definition: a boost at the bottom of the knob,
//! a clipper at the top, the alias the oversampling takes out, the tone,
//! the level, block-grouping independence and `reset`.

mod common;

use common::{SR, goertzel, independent_of_block_grouping, noise, rms, run, sine, to_db};
use valverig_core::dsp::drive::{BIAS, Drive, TONE_OPEN_HZ};

/// The last tenth of a second: 4,800 samples, a whole number of periods at
/// every multiple of 10 Hz.
fn tail(x: &[f32]) -> &[f32] {
    &x[x.len() - 4800..]
}

/// The magnitude at `hz` over the tail of `x`, as a fraction of full scale.
fn tone_at(x: &[f32], hz: f64) -> f32 {
    goertzel(tail(x), hz, SR)
}

/// The drive with nothing after the shaper: tone open, level at unity.
fn open(d: &mut Drive) {
    d.set_tone_hz(TONE_OPEN_HZ);
    d.set_level_db(0.0);
}

#[test]
fn no_drive_is_a_boost_within_a_tenth_of_a_decibel() {
    let mut d = Drive::new(SR);
    open(&mut d);
    d.set_drive_db(0.0);
    let x = sine(1000.0, 0.05, 0.5);
    let y = run(&x, &[128], |i, o| d.process(i, o));
    let gain = to_db(rms(tail(&y)) / rms(tail(&x)));
    assert!(gain.abs() < 0.12, "gain {gain} dB at 0 dB of drive");
    // Most of it is the shaper: its slope at the bias.
    let slope = 1.0 - BIAS.tanh().powi(2);
    assert!(
        (gain - to_db(slope)).abs() < 0.06,
        "gain {gain} dB, the shaper's slope is {} dB",
        to_db(slope)
    );
    // And the low E is still there.
    let x = sine(80.0, 0.05, 0.5);
    d.reset();
    let y = run(&x, &[128], |i, o| d.process(i, o));
    let gain = to_db(rms(tail(&y)) / rms(tail(&x)));
    assert!(gain > -1.2, "gain {gain} dB at 80 Hz");
}

#[test]
fn full_drive_clips_to_a_rounded_square_with_odd_harmonics() {
    let mut d = Drive::new(SR);
    open(&mut d);
    d.set_drive_db(40.0);
    let x = sine(1000.0, 0.5, 0.5);
    let y = run(&x, &[64], |i, o| d.process(i, o));
    let peak = tail(&y).iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak <= 1.3,
        "peak {peak}: the shaper tops out at one, the filter after it rings a little on the edges"
    );
    assert!(peak > 0.9, "peak {peak}: not clipping");
    let fundamental = tone_at(&y, 1000.0);
    let third = tone_at(&y, 3000.0);
    let fifth = tone_at(&y, 5000.0);
    assert!(
        third / fundamental > 0.25,
        "third harmonic {third} against {fundamental}: a square wave has a third"
    );
    assert!(
        fifth / fundamental > 0.12,
        "fifth harmonic {fifth} against {fundamental}"
    );
}

#[test]
fn the_oversampling_keeps_the_harmonics_from_folding_back() {
    // A 7 kHz tone at full drive: the fifth harmonic at 35 kHz would fold to
    // 13 kHz at 48 kHz, and a plain tanh at the sample rate puts it there at
    // a fifth of the fundamental. Oversampled, that one is filtered out;
    // what is left on 13 kHz is the square's 43rd harmonic, which folds
    // inside the doubled rate and no filter reaches.
    let x = sine(7000.0, 0.5, 0.5);
    let naive: Vec<f32> = x.iter().map(|v| (100.0 * v).tanh()).collect();
    let mut d = Drive::new(SR);
    open(&mut d);
    d.set_drive_db(40.0);
    let y = run(&x, &[128], |i, o| d.process(i, o));
    let alias_naive = tone_at(&naive, 13_000.0);
    let alias = tone_at(&y, 13_000.0);
    let fundamental = tone_at(&y, 7000.0);
    assert!(
        alias_naive > 0.05,
        "the naive shaper aliases: {alias_naive}"
    );
    assert!(
        to_db(alias / alias_naive) < -15.0,
        "the alias at 13 kHz: {} dB against the naive shaper's",
        to_db(alias / alias_naive)
    );
    assert!(
        fundamental > 0.5,
        "the fundamental is still there: {fundamental}"
    );
}

#[test]
fn the_tone_darkens_and_the_level_scales() {
    let x1 = sine(1000.0, 0.05, 0.5);
    let x8 = sine(8000.0, 0.05, 0.5);
    let gain_at = |d: &mut Drive, x: &[f32]| {
        d.reset();
        let y = run(x, &[128], |i, o| d.process(i, o));
        to_db(rms(tail(&y)) / rms(tail(x)))
    };
    let mut d = Drive::new(SR);
    open(&mut d);
    d.set_drive_db(0.0);
    d.set_tone_hz(2000.0);
    let low = gain_at(&mut d, &x1);
    let high = gain_at(&mut d, &x8);
    assert!(
        high - low < -9.0,
        "8 kHz sits {} dB from 1 kHz with the tone at 2 kHz; a one-pole gives -11",
        high - low
    );

    let mut d = Drive::new(SR);
    open(&mut d);
    d.set_drive_db(12.0);
    let full = run(&x1, &[128], |i, o| d.process(i, o));
    d.reset();
    d.set_level_db(-6.0206);
    let half = run(&x1, &[128], |i, o| d.process(i, o));
    let ratio = rms(tail(&half)) / rms(tail(&full));
    assert!((ratio - 0.5).abs() < 1e-3, "-6 dB of level: {ratio}");
}

#[test]
fn output_does_not_depend_on_block_grouping() {
    let x = noise(7, 20_000, 0.4);
    independent_of_block_grouping(
        &x,
        || {
            let mut d = Drive::new(SR);
            d.set_drive_db(20.0);
            d.set_tone_hz(4000.0);
            d.set_level_db(-3.0);
            d
        },
        |d, i, o| d.process(i, o),
    );
}

#[test]
fn reset_forgets_the_signal() {
    let mut d = Drive::new(SR);
    d.set_drive_db(30.0);
    let x = noise(3, 2000, 0.5);
    let _ = run(&x, &[128], |i, o| d.process(i, o));
    d.reset();
    let y = run(&vec![0.0f32; 256], &[64], |i, o| d.process(i, o));
    assert!(
        y.iter().all(|v| v.to_bits() == 0),
        "silence in, silence out after reset"
    );
    // A change of rate keeps the settings.
    d.set_sample_rate(96_000.0);
    let y = run(&sine(1000.0, 0.5, 0.1), &[128], |i, o| d.process(i, o));
    assert!(y.iter().all(|v| v.is_finite()) && y.iter().any(|v| v.abs() > 0.5));
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Drive::new(0.0);
}
