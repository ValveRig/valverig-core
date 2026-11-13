//! The delay against its own definition: the dry copy when the mix is
//! zero, repeats at the exact time with the exact feedback, the tone on
//! each repeat, the glide to a new time, block-grouping independence and
//! `reset`.

mod common;

use common::{SR, assert_same_bits, impulse, independent_of_block_grouping, noise, run};
use valrig_core::delay::{Mono as Delay, TONE_OPEN_HZ};

/// Half a second, feedback a half, the repeats as loud as the dry signal,
/// nothing in the loop.
fn plain(d: &mut Delay) {
    d.set_time_ms(500.0);
    d.set_feedback(0.5);
    d.set_mix(1.0);
    d.set_tone_hz(TONE_OPEN_HZ);
}

#[test]
fn a_mix_of_zero_is_the_input_bit_for_bit() {
    let mut d = Delay::new(SR);
    d.set_mix(0.0);
    // The mix glides to a setting; reset lands it there.
    d.reset();
    let x: Vec<f32> = noise(1, 4096, 0.5)
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 97 == 0 { -0.0 } else { *v })
        .collect();
    let y = run(&x, &[128], |i, o| d.process(i, o));
    assert_same_bits(&y, &x, "a mix of zero");
}

#[test]
fn the_repeats_land_on_the_sample_at_the_feedback_exactly() {
    let mut d = Delay::new(SR);
    plain(&mut d);
    let y = run(&impulse(80_000), &[128], |i, o| d.process(i, o));
    assert_eq!(y[0], 1.0, "the dry impulse");
    assert_eq!(y[24_000], 1.0, "the first repeat, half a second on");
    assert_eq!(y[48_000], 0.5, "the second, at the feedback");
    assert_eq!(y[72_000], 0.25, "the third");
    let elsewhere = y
        .iter()
        .enumerate()
        .filter(|(i, v)| i % 24_000 != 0 && **v != 0.0)
        .count();
    assert_eq!(elsewhere, 0, "nothing between the repeats");
}

#[test]
fn the_tone_darkens_every_repeat() {
    let mut d = Delay::new(SR);
    plain(&mut d);
    d.set_feedback(0.0);
    d.set_tone_hz(1000.0);
    let y = run(&impulse(30_000), &[128], |i, o| d.process(i, o));
    // The repeat is the impulse response of the one-pole: it peaks at the
    // coefficient and sums to one.
    let peak = y[24_000];
    assert!(
        peak > 0.1 && peak < 0.2,
        "the first sample of the repeat: {peak}, a one-pole at 1 kHz starts at 0.12"
    );
    let sum: f32 = y[24_000..].iter().sum();
    assert!((sum - 1.0).abs() < 1e-3, "the repeat sums to {sum}");
    assert!(y[24_001] > 0.0 && y[24_001] < peak, "then it decays");
}

#[test]
fn a_change_of_time_glides_and_lands() {
    let mut d = Delay::new(SR);
    plain(&mut d);
    d.set_feedback(0.0);
    let _ = run(&impulse(30_000), &[128], |i, o| d.process(i, o));
    // Halve the time: the read point walks 12,000 samples closer at half a
    // sample per sample, a quarter of a second, and stays finite doing it.
    d.set_time_ms(250.0);
    let y = run(&vec![0.0f32; 24_000], &[128], |i, o| d.process(i, o));
    assert!(y.iter().all(|v| v.is_finite()));
    let y = run(&impulse(20_000), &[128], |i, o| d.process(i, o));
    assert_eq!(y[12_000], 1.0, "the repeat at the new time, exactly");
    assert!(y[1..12_000].iter().all(|v| *v == 0.0) && y[12_001..].iter().all(|v| *v == 0.0));
}

#[test]
fn output_does_not_depend_on_block_grouping() {
    let x = noise(5, 40_000, 0.4);
    independent_of_block_grouping(
        &x,
        || {
            let mut d = Delay::new(SR);
            d.set_time_ms(123.4);
            d.set_feedback(0.6);
            d.set_mix(0.5);
            d.set_tone_hz(3000.0);
            d
        },
        |d, i, o| d.process(i, o),
    );
}

#[test]
fn reset_silences_the_repeats_and_a_change_of_rate_keeps_the_settings() {
    let mut d = Delay::new(SR);
    plain(&mut d);
    let _ = run(&noise(9, 30_000, 0.5), &[128], |i, o| d.process(i, o));
    d.reset();
    let y = run(&vec![0.0f32; 60_000], &[128], |i, o| d.process(i, o));
    assert!(
        y.iter().all(|v| *v == 0.0),
        "nothing comes back after reset"
    );
    d.set_sample_rate(96_000.0);
    let y = run(&impulse(60_000), &[128], |i, o| d.process(i, o));
    assert_eq!(y[48_000], 1.0, "half a second at 96 kHz");
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Delay::new(f64::INFINITY);
}
