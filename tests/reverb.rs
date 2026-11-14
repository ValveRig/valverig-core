//! The reverb against its own definition: the mix, the onset, the decay
//! time read off the impulse response, damping, stability, and independence
//! from block grouping.

mod common;

use std::f32::consts::PI;

use common::{SR, assert_same_bits, impulse, independent_of_block_grouping, ms, noise, rms, run};
use valverig_core::reverb::Fdn as Reverb;

/// A wet-only reverb with the damping out of the way.
fn wet(decay_s: f32) -> Reverb {
    let mut r = Reverb::new(SR);
    r.set_mix(1.0);
    r.set_decay_s(decay_s);
    r.set_damping_hz(20_000.0);
    r.set_predelay_ms(0.0);
    r.reset();
    r
}

/// The decay time, by Schroeder's backward integration of the energy of the
/// impulse response, between -5 and -35 dB. On a 300 Hz lowpass of the
/// response, so that the damping, even at the top of its range, plays no
/// part.
fn t60(h: &[f32]) -> f32 {
    let c = 1.0 - (-2.0 * PI * 300.0 / SR as f32).exp();
    let mut lp = 0.0f32;
    let low: Vec<f64> = h
        .iter()
        .map(|&v| {
            lp += c * (v - lp);
            lp as f64
        })
        .collect();
    let mut energy = vec![0.0f64; low.len()];
    let mut acc = 0.0f64;
    for i in (0..low.len()).rev() {
        acc += low[i] * low[i];
        energy[i] = acc;
    }
    let total = energy[0];
    let at = |db: f64| {
        energy
            .iter()
            .position(|&e| 10.0 * (e / total).log10() <= db)
            .unwrap()
    };
    (at(-35.0) - at(-5.0)) as f32 / SR as f32 * 2.0
}

#[test]
fn a_mix_of_zero_is_the_dry_signal_bit_for_bit() {
    let mut r = Reverb::new(SR);
    r.set_mix(0.0);
    r.reset();
    let mut x = noise(1, 8192, 0.5);
    x[100] = -0.0;
    let y = run(&x, &[128], |i, o| r.process(i, o));
    assert_same_bits(&y, &x, "a mix of zero");
}

#[test]
fn the_mix_blends_dry_and_tail() {
    // Half and half: the impulse itself comes through at exactly half,
    // since the tail has not started yet.
    let mut r = Reverb::new(SR);
    r.set_mix(0.5);
    r.reset();
    let y = run(&impulse(ms(500.0)), &[64], |i, o| r.process(i, o));
    assert_eq!(y[0].to_bits(), 0.5f32.to_bits());
    assert!(
        y[ms(200.0)..].iter().any(|&v| v != 0.0),
        "and there is a tail"
    );
    // All tail: nothing of the impulse itself.
    let mut only_tail = wet(2.0);
    let y = run(&impulse(ms(100.0)), &[64], |i, o| only_tail.process(i, o));
    assert_eq!(y[0], 0.0);
}

#[test]
fn the_tail_starts_after_the_predelay_and_the_shortest_line() {
    for predelay in [0.0f32, 20.0, 100.0] {
        let mut r = wet(2.0);
        r.set_predelay_ms(predelay);
        r.reset();
        let y = run(&impulse(ms(300.0)), &[64], |i, o| r.process(i, o));
        let first = y.iter().position(|&v| v != 0.0).unwrap();
        assert_eq!(first, r.onset_samples(), "pre-delay {predelay} ms");
        assert_eq!(first, ms(predelay) + 1409);
    }
}

#[test]
fn the_decay_time_is_the_decay_time() {
    for decay in [0.5f32, 2.0, 5.0] {
        let mut r = wet(decay);
        let x = impulse((decay * 2.0 * SR as f32) as usize);
        let y = run(&x, &[64], |i, o| r.process(i, o));
        let got = t60(&y);
        assert!(
            (got - decay).abs() < decay * 0.1,
            "decay {decay} s: measured {got} s"
        );
    }
}

#[test]
fn damping_takes_the_tail_down_faster() {
    let energy_after = |damping: f32| {
        let mut r = wet(3.0);
        r.set_damping_hz(damping);
        r.reset();
        let y = run(&impulse(ms(2000.0)), &[64], |i, o| r.process(i, o));
        y[ms(1000.0)..].iter().map(|v| (v * v) as f64).sum::<f64>()
    };
    let (open, damped) = (energy_after(20_000.0), energy_after(1_000.0));
    assert!(damped < open * 0.5, "damped {damped} vs open {open}");
}

#[test]
fn it_is_stable_at_the_longest_decay() {
    let mut r = wet(10.0);
    let mut x = noise(7, ms(2000.0), 0.9);
    x.extend(std::iter::repeat_n(0.0, ms(6000.0)));
    let y = run(&x, &[64], |i, o| r.process(i, o));
    assert!(y.iter().all(|v| v.is_finite()));
    let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 4.0, "peak {peak}");
    let (early, late) = (rms(&y[ms(2000.0)..ms(3000.0)]), rms(&y[ms(7000.0)..]));
    assert!(late < early, "decaying: {early} then {late}");
    assert!(late > 0.0, "and still ringing at ten seconds");
}

#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let mut x = noise(3, ms(400.0), 0.5);
    x.extend(std::iter::repeat_n(0.0, ms(600.0)));
    independent_of_block_grouping(
        &x,
        || {
            let mut r = Reverb::new(SR);
            r.set_mix(0.3);
            r.set_decay_s(1.0);
            r.reset();
            r
        },
        |r, i, o| r.process(i, o),
    );
}

#[test]
fn one_signal_on_both_sides_gets_two_different_tails() {
    let mut r = Reverb::new(SR);
    r.set_mix(0.5);
    r.reset();
    let x = impulse(ms(300.0));
    let (mut l, mut rr) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    r.process_pair(&x, &x, &mut l, &mut rr);
    assert_eq!(l[0], 0.5);
    assert_eq!(rr[0], 0.5);
    assert!(
        l[ms(100.0)..] != rr[ms(100.0)..],
        "the two tails are not the same"
    );
}

#[test]
fn each_side_keeps_its_own_dry_signal() {
    let x = noise(9, ms(400.0), 0.5);
    let y = noise(10, x.len(), 0.5);
    let (mut left, mut right) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut r = Reverb::new(SR);
    r.set_mix(0.0);
    r.reset();
    r.process_pair(&x, &y, &mut left, &mut right);
    assert_same_bits(&left, &x, "the left side at a mix of zero");
    assert_same_bits(&right, &y, "the right side at a mix of zero");
}

#[test]
fn reset_silences_the_tail() {
    let mut r = wet(2.0);
    run(&impulse(ms(100.0)), &[64], |i, o| r.process(i, o));
    r.reset();
    let y = run(&vec![0.0f32; ms(500.0)], &[64], |i, o| r.process(i, o));
    assert!(y.iter().all(|&v| v == 0.0));
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Reverb::new(f64::INFINITY);
}
