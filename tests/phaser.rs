//! The phaser against its own definition: the dry copy at mix zero, two
//! notches and a peak at the corner with the sweep held, the sweep moving
//! them, the pair's offset, block-grouping independence and `reset`.

mod common;

use common::{SR, assert_same_bits, independent_of_block_grouping, noise, rms, run, sine};
use valverig_core::phaser::{CENTRE_HZ, Mono as Phaser};

/// The steady-state gain at `hz`, in dB, with the sweep held at the centre.
fn gain_db(hz: f32) -> f32 {
    let mut p = Phaser::new(SR);
    p.set_rate_hz(0.0);
    p.set_depth(0.0);
    p.set_feedback(0.0);
    p.set_mix(0.5);
    p.reset();
    let x = sine(hz, 0.5, 0.5);
    let y = run(&x, &[128], |i, o| p.process(i, o));
    let tail = x.len() - (0.2 * SR as f32) as usize;
    // The mix is a half, so a perfectly added copy is unity, not two.
    20.0 * (rms(&y[tail..]) / rms(&x[tail..])).log10()
}

#[test]
fn a_mix_of_zero_is_the_input_bit_for_bit() {
    let mut p = Phaser::new(SR);
    p.set_mix(0.0);
    p.set_feedback(0.5);
    p.reset();
    let x: Vec<f32> = noise(1, 4096, 0.5)
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 97 == 0 { -0.0 } else { *v })
        .collect();
    let y = run(&x, &[128], |i, o| p.process(i, o));
    assert_same_bits(&y, &x, "a mix of zero");
}

#[test]
fn held_at_the_centre_it_is_two_notches_around_a_peak() {
    // Four allpasses, 90° each at the corner: 360° there, so dry and wet add;
    // 180° and 540° at 0.414 and 2.414 times the corner, where they cancel.
    let at_centre = gain_db(CENTRE_HZ);
    assert!(at_centre.abs() < 0.5, "at the corner: {at_centre} dB");
    let lower = gain_db(CENTRE_HZ * 0.414);
    let upper = gain_db(CENTRE_HZ * 2.414);
    assert!(lower < -20.0, "the lower notch: {lower} dB");
    assert!(upper < -20.0, "the upper notch: {upper} dB");
    // Far below the corner the chain has hardly turned: four stages at 20 Hz
    // are 13° round, a twentieth of a decibel against the dry signal.
    let far = gain_db(20.0);
    assert!(far.abs() < 0.2, "far below the corner: {far} dB");
}

#[test]
fn the_sweep_moves_the_notches_over_a_tone() {
    let mut p = Phaser::new(SR);
    p.set_rate_hz(1.0);
    p.set_depth(1.0);
    p.set_feedback(0.3);
    p.set_mix(0.5);
    p.reset();
    let x = sine(1000.0, 0.5, 3.0);
    let y = run(&x, &[128], |i, o| p.process(i, o));
    // The level of a 1 kHz tone rises and falls as the notches pass it.
    let windows: Vec<f32> = (10..30)
        .map(|w| rms(&y[w * 4800..(w + 1) * 4800]))
        .collect();
    let (lo, hi) = windows
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
    assert!(
        20.0 * (hi / lo).log10() > 6.0,
        "the tone swings {} dB under the sweep",
        20.0 * (hi / lo).log10()
    );
    assert!(y.iter().all(|v| v.is_finite()));
}

#[test]
fn a_phase_offset_makes_a_pair_sway_against_itself() {
    let mut left = Phaser::new(SR);
    let mut right = Phaser::new(SR);
    right.set_phase_offset(0.25);
    let x = sine(800.0, 0.5, 1.0);
    let l = run(&x, &[128], |i, o| left.process(i, o));
    let r = run(&x, &[128], |i, o| right.process(i, o));
    let differ = l
        .iter()
        .zip(&r)
        .skip(1000)
        .filter(|(a, b)| (*a - *b).abs() > 1e-3)
        .count();
    assert!(
        differ > 10_000,
        "{differ} samples differ between the two sides"
    );
}

#[test]
fn output_does_not_depend_on_block_grouping() {
    let x = noise(5, 20_000, 0.4);
    independent_of_block_grouping(
        &x,
        || {
            let mut p = Phaser::new(SR);
            p.set_rate_hz(2.3);
            p.set_depth(0.8);
            p.set_feedback(0.6);
            p.set_mix(0.5);
            p
        },
        |p, i, o| p.process(i, o),
    );
}

#[test]
fn reset_forgets_the_signal_and_a_change_of_rate_keeps_the_settings() {
    let mut p = Phaser::new(SR);
    p.set_feedback(0.8);
    let _ = run(&noise(9, 2000, 0.5), &[128], |i, o| p.process(i, o));
    p.reset();
    let y = run(&vec![0.0f32; 256], &[64], |i, o| p.process(i, o));
    assert!(
        y.iter().all(|v| v.to_bits() == 0),
        "silence in, silence out after reset"
    );
    p.set_sample_rate(96_000.0);
    let y = run(&sine(1000.0, 0.5, 0.1), &[128], |i, o| p.process(i, o));
    assert!(y.iter().all(|v| v.is_finite()) && y.iter().any(|v| v.abs() > 0.1));
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Phaser::new(-48_000.0);
}
