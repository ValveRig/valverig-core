//! The chorus against its own definition: the dry copy when the mix is
//! zero, the plain delay when the depth is zero, the sweep, the stereo
//! offset, block-grouping independence and `reset`.

mod common;

use common::{SR, assert_same_bits, goertzel, independent_of_block_grouping, noise, run, sine};
use valrig_core::chorus::{BASE_MS, Mono as Chorus};

#[test]
fn a_mix_of_zero_is_the_input_bit_for_bit() {
    let mut c = Chorus::new(SR);
    c.set_mix(0.0);
    c.set_depth(1.0);
    // The mix glides to a setting; reset lands it there.
    c.reset();
    // Negative zeros salted through, so "bit for bit" means the sign too.
    let x: Vec<f32> = noise(1, 4096, 0.5)
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 97 == 0 { -0.0 } else { *v })
        .collect();
    let y = run(&x, &[128], |i, o| c.process(i, o));
    assert_same_bits(&y, &x, "a mix of zero");
}

#[test]
fn no_depth_and_all_copy_is_the_input_delayed_by_the_base() {
    let mut c = Chorus::new(SR);
    c.set_depth(0.0);
    c.set_mix(1.0);
    c.reset();
    let base = (BASE_MS * 1e-3 * SR as f32).round() as usize;
    assert_eq!(base, 336);
    let x = noise(2, 4096, 0.5);
    let y = run(&x, &[128], |i, o| c.process(i, o));
    for (i, v) in y.iter().enumerate().take(base) {
        assert_eq!(*v, 0.0, "sample {i}: the line starts silent");
    }
    for i in base..x.len() {
        assert_eq!(y[i].to_bits(), x[i - base].to_bits(), "sample {i}");
    }
}

#[test]
fn the_sweep_bends_the_pitch_of_the_copy() {
    // A vibrato at 2 Hz, full depth: ±240 samples at 48 kHz, so the copy of
    // a 1 kHz sine swings some ±60 Hz and the energy that was on the 1 kHz
    // line is spread across sidebands.
    let mut c = Chorus::new(SR);
    c.set_rate_hz(2.0);
    c.set_depth(1.0);
    c.set_mix(1.0);
    let x = sine(1000.0, 0.5, 2.0);
    let y = run(&x, &[128], |i, o| c.process(i, o));
    let window = &y[48_000..];
    let on_line = goertzel(window, 1000.0, SR) / goertzel(&x[48_000..], 1000.0, SR);
    assert!(
        on_line < 0.5,
        "the copy keeps {on_line} of its energy on 1 kHz"
    );
    let peak = window.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak < 0.55 && peak > 0.45,
        "peak {peak}: the copy is the input's level, interpolated"
    );
    assert!(window.iter().all(|v| v.is_finite()));
}

#[test]
fn a_phase_offset_makes_a_pair_sway_against_itself() {
    let mut left = Chorus::new(SR);
    let mut right = Chorus::new(SR);
    right.set_phase_offset(0.25);
    for c in [&mut left, &mut right] {
        c.set_depth(0.7);
        c.set_mix(0.5);
    }
    let x = sine(440.0, 0.5, 1.0);
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
            let mut c = Chorus::new(SR);
            c.set_rate_hz(1.3);
            c.set_depth(0.8);
            c.set_mix(0.4);
            c
        },
        |c, i, o| c.process(i, o),
    );
}

#[test]
fn reset_silences_the_copy_and_a_change_of_rate_keeps_the_settings() {
    let mut c = Chorus::new(SR);
    c.set_mix(1.0);
    c.set_depth(0.0);
    let _ = run(&noise(9, 2000, 0.5), &[128], |i, o| c.process(i, o));
    c.reset();
    let y = run(&vec![0.0f32; 400], &[64], |i, o| c.process(i, o));
    assert!(y.iter().all(|v| *v == 0.0), "the line is clear after reset");
    c.set_sample_rate(96_000.0);
    let x = noise(10, 4000, 0.5);
    let y = run(&x, &[128], |i, o| c.process(i, o));
    let base = (BASE_MS * 1e-3 * 96_000.0f32).round() as usize;
    assert_eq!(
        y[base + 100].to_bits(),
        x[100].to_bits(),
        "still the plain delay at the new rate"
    );
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Chorus::new(f64::INFINITY);
}
