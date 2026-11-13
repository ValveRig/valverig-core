//! The compressor against its own definition: the static curve, the knee,
//! the times, transparency at rest, and independence from block grouping.

mod common;

use common::{SR, assert_same_bits, from_db, independent_of_block_grouping, ms, noise, run, to_db};
use valrig_core::compressor::Mono as Compressor;

/// A fresh compressor with a hard knee and fast times, so the static curve
/// can be read off a settled level.
fn hard(threshold_db: f32, ratio: f32) -> Compressor {
    let mut c = Compressor::new(SR);
    c.set_threshold_db(threshold_db);
    c.set_ratio(ratio);
    c.set_knee_db(0.0);
    c.set_attack_ms(1.0);
    c.set_release_ms(10.0);
    c
}

/// The reduction, in dB, a steady level settles at.
fn settled_reduction_db(c: &mut Compressor, level_db: f32) -> f32 {
    let x = vec![from_db(level_db); ms(500.0)];
    let y = run(&x, &[64], |i, o| c.process(i, o));
    -to_db(y[y.len() - 1] / x[0])
}

#[test]
fn below_the_threshold_it_is_transparent_bit_for_bit() {
    let mut c = Compressor::new(SR);
    c.set_threshold_db(-20.0);
    let x = noise(0x2468_ACE0, 4096, from_db(-40.0));
    let y = run(&x, &[128], |i, o| c.process(i, o));
    assert_same_bits(&y, &x, "below the threshold");
    assert_eq!(c.gain_reduction_db(), 0.0);
}

#[test]
fn the_static_curve_follows_threshold_and_ratio() {
    for (ratio, level, want) in [
        (4.0, -8.0, 9.0),
        (2.0, -8.0, 6.0),
        (20.0, 0.0, 19.0),
        (1.0, -8.0, 0.0),
    ] {
        let got = settled_reduction_db(&mut hard(-20.0, ratio), level);
        assert!(
            (got - want).abs() < 0.05,
            "ratio {ratio}, level {level} dB: {got} dB, wanted {want}"
        );
    }
    // Below the threshold, nothing.
    assert_eq!(settled_reduction_db(&mut hard(-20.0, 4.0), -25.0), 0.0);
}

#[test]
fn the_soft_knee_bends_the_curve_and_meets_the_hard_one() {
    let mut c = hard(-20.0, 4.0);
    c.set_knee_db(6.0);
    // slope 0.75; at the threshold the knee gives 0.75 · 3² / 12 dB.
    assert!((settled_reduction_db(&mut c, -20.0) - 0.5625).abs() < 0.02);
    // At the bottom of the knee, nothing yet.
    assert_eq!(settled_reduction_db(&mut c, -23.0), 0.0);
    // At the top, the knee meets the straight line: 0.75 · 3 dB either way.
    let soft = settled_reduction_db(&mut c, -17.0);
    let straight = settled_reduction_db(&mut hard(-20.0, 4.0), -17.0);
    assert!(
        (soft - 2.25).abs() < 0.02 && (straight - 2.25).abs() < 0.02,
        "{soft} vs {straight}"
    );
}

#[test]
fn attack_and_release_follow_their_times() {
    let mut c = hard(-20.0, 4.0);
    c.set_attack_ms(10.0);
    c.set_release_ms(100.0);
    let quiet = from_db(-30.0);
    let loud = from_db(-8.0); // 12 dB over: 9 dB of reduction at the ratio of 4
    let mut x = vec![quiet; ms(1100.0)];
    for v in &mut x[ms(50.0)..ms(250.0)] {
        *v = loud;
    }
    let y = run(&x, &[64], |i, o| c.process(i, o));
    let reduction = |t: f32| -to_db(y[ms(t)] / x[ms(t)]);

    assert_eq!(reduction(49.0), 0.0, "at rest before the step");
    let at_tau = reduction(60.0);
    assert!(
        (at_tau - 9.0 * (1.0 - (-1f32).exp())).abs() < 0.3,
        "63% of 9 dB one attack in: {at_tau}"
    );
    assert!(
        (reduction(249.0) - 9.0).abs() < 0.02,
        "settled at 9 dB: {}",
        reduction(249.0)
    );
    let after_tau = reduction(350.0);
    assert!(
        (after_tau - 9.0 * (-1f32).exp()).abs() < 0.3,
        "37% of 9 dB one release out: {after_tau}"
    );
    assert!(
        reduction(1000.0) < 0.01,
        "released by 1 s: {}",
        reduction(1000.0)
    );
    let mut previous = reduction(260.0);
    for t in 261..1100 {
        let now = reduction(t as f32);
        assert!(now <= previous + 1e-4, "the release is monotonic at {t} ms");
        previous = now;
    }
}

#[test]
fn makeup_is_exact_at_rest() {
    let mut c = Compressor::new(SR);
    c.set_threshold_db(-20.0);
    c.set_makeup_db(6.0);
    let x = vec![from_db(-40.0); 256];
    let y = run(&x, &[64], |i, o| c.process(i, o));
    let want = x[0] * from_db(6.0);
    assert!(y.iter().all(|v| v.to_bits() == want.to_bits()));
}

#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let x: Vec<f32> = (0..ms(1500.0))
        .map(|i| {
            let t = i as f32 / SR as f32;
            let envelope = ((t * std::f32::consts::TAU * 1.5).sin() * 0.5 + 0.5).powi(2);
            (t * std::f32::consts::TAU * 220.0).sin() * 0.7 * envelope
        })
        .collect();
    independent_of_block_grouping(
        &x,
        || {
            let mut c = Compressor::new(SR);
            c.set_threshold_db(-24.0);
            c.set_ratio(6.0);
            c.set_makeup_db(3.0);
            c
        },
        |c, i, o| c.process(i, o),
    );
}

#[test]
fn reset_forgets_the_reduction() {
    let mut c = hard(-20.0, 4.0);
    run(&vec![from_db(-6.0); ms(100.0)], &[64], |i, o| {
        c.process(i, o)
    });
    assert!(c.gain_reduction_db() > 8.0);
    c.reset();
    assert_eq!(c.gain_reduction_db(), 0.0);
    let x = vec![from_db(-40.0); 64];
    let y = run(&x, &[64], |i, o| c.process(i, o));
    assert_eq!(y[0].to_bits(), x[0].to_bits(), "transparent straight away");
}

#[test]
fn a_change_of_rate_keeps_the_times() {
    // The same 10 ms attack at twice the rate reaches the same reduction
    // after the same number of milliseconds, not the same number of frames.
    let reduction_after_10_ms = |rate: f64| {
        let mut c = Compressor::new(SR);
        c.set_threshold_db(-20.0);
        c.set_ratio(4.0);
        c.set_knee_db(0.0);
        c.set_attack_ms(10.0);
        c.set_sample_rate(rate);
        let frames = (rate / 100.0) as usize;
        let x = vec![from_db(-8.0); frames];
        run(&x, &[64], |i, o| c.process(i, o));
        c.gain_reduction_db()
    };
    let (a, b) = (
        reduction_after_10_ms(48_000.0),
        reduction_after_10_ms(96_000.0),
    );
    assert!(
        (a - b).abs() < 0.05,
        "{a} dB at 48 kHz against {b} dB at 96 kHz"
    );
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Compressor::new(f64::NAN);
}
