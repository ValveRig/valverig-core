//! The gate against its own definition: thresholds, times, hysteresis, the
//! floor, and independence from block grouping.

mod common;

use common::{
    SR, assert_same_bits, dc, from_db, independent_of_block_grouping, ms, noise, run, sine,
};
use valverig_core::gate::{HYSTERESIS_DB, Mono as Gate};

#[test]
fn stays_closed_below_the_threshold_and_opens_above_it() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-60.0);
    let y = run(&dc(-70.0, ms(200.0)), &[64], |i, o| g.process(i, o));
    assert!(!g.is_open());
    assert!(
        y.iter()
            .all(|&v| v.abs() <= from_db(-70.0) * from_db(-100.0)),
        "closed: the floor is -100 dB"
    );

    let x = dc(-50.0, ms(200.0));
    let y = run(&x, &[64], |i, o| g.process(i, o));
    assert!(g.is_open());
    assert_eq!(
        y.last().unwrap().to_bits(),
        x[0].to_bits(),
        "fully open, the gain is exactly 1"
    );
}

#[test]
fn an_open_gate_passes_the_signal_bit_for_bit() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-40.0);
    run(&dc(-6.0, ms(50.0)), &[64], |i, o| g.process(i, o));
    let x = noise(0x1234_5678, 4096, 0.5);
    let y = run(&x, &[128], |i, o| g.process(i, o));
    assert_same_bits(&y, &x, "an open gate");
}

/// A burst, then the gain measured as output over input on a carrier far
/// below the threshold, so the attack, the hold and the release are all
/// visible.
#[test]
fn attack_hold_and_release_follow_their_times() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-20.0);
    g.set_attack_ms(1.0);
    g.set_hold_ms(50.0);
    g.set_release_ms(100.0);
    let carrier = from_db(-80.0);
    let mut x = vec![carrier; ms(900.0)];
    for v in &mut x[ms(20.0)..ms(120.0)] {
        *v = 0.5;
    }
    let y = run(&x, &[64], |i, o| g.process(i, o));
    let gain = |t: f32| y[ms(t)] / x[ms(t)];

    assert!(gain(19.0) < 1e-4, "closed before the burst: {}", gain(19.0));
    assert!(
        gain(25.0) > 0.99,
        "open 5 ms into the burst with a 1 ms attack: {}",
        gain(25.0)
    );
    // The burst ends at 120 ms. The detector falls from -6 dB to the closing
    // mark of -24 dB in about 21 ms, then the 50 ms hold runs: fully open
    // until about 190 ms, and releasing after that.
    assert_eq!(gain(165.0), 1.0, "held open after the burst");
    assert!(gain(250.0) < 0.9, "releasing by 250 ms: {}", gain(250.0));
    assert!(gain(800.0) < 0.01, "closed by 800 ms: {}", gain(800.0));
    let mut previous = gain(200.0);
    for t in 201..900 {
        let now = gain(t as f32);
        assert!(now <= previous, "the release is monotonic at {t} ms");
        previous = now;
    }
}

#[test]
fn hysteresis_keeps_a_decaying_note_from_chattering() {
    let between = -20.0 - HYSTERESIS_DB / 2.0;
    // Opened by a burst, a level between the two marks keeps it open.
    let mut g = Gate::new(SR);
    g.set_threshold_db(-20.0);
    run(&dc(-6.0, ms(50.0)), &[64], |i, o| g.process(i, o));
    let x = dc(between, ms(500.0));
    let y = run(&x, &[64], |i, o| g.process(i, o));
    assert!(g.is_open());
    assert_eq!(y.last().unwrap().to_bits(), x[0].to_bits());
    // From closed, the same level does not open it.
    g.reset();
    run(&x, &[64], |i, o| g.process(i, o));
    assert!(!g.is_open());
}

#[test]
fn the_range_sets_the_closed_floor() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-60.0);
    g.set_range_db(-20.0);
    let x = dc(-80.0, ms(100.0));
    let y = run(&x, &[64], |i, o| g.process(i, o));
    let want = x[0] * from_db(-20.0);
    assert!(
        (y[y.len() - 1] - want).abs() < want * 1e-3,
        "{} vs {want}",
        y[y.len() - 1]
    );
}

/// 1 kHz bursts under a slow envelope, so the gate opens and closes a few
/// times over the signal, then silence so it ends closed.
fn opens_and_closes() -> Vec<f32> {
    let mut x: Vec<f32> = (0..ms(1500.0))
        .map(|i| {
            let t = i as f32 / SR as f32;
            let envelope = ((t * std::f32::consts::TAU * 1.5).sin() * 0.5 + 0.5).powi(4);
            (t * std::f32::consts::TAU * 1000.0).sin() * 0.5 * envelope
        })
        .collect();
    x.extend(std::iter::repeat_n(0.0, ms(300.0)));
    x
}

#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let x = opens_and_closes();
    independent_of_block_grouping(
        &x,
        || {
            let mut g = Gate::new(SR);
            g.set_threshold_db(-30.0);
            g.set_hold_ms(20.0);
            g.set_release_ms(30.0);
            g
        },
        |g, i, o| g.process(i, o),
    );
}

#[test]
fn a_signal_that_ends_in_silence_leaves_the_gate_closed() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-30.0);
    g.set_hold_ms(20.0);
    g.set_release_ms(30.0);
    run(&opens_and_closes(), &[64], |i, o| g.process(i, o));
    assert!(!g.is_open());
}

#[test]
fn reset_closes_the_gate_and_forgets_the_level() {
    let mut used = Gate::new(SR);
    run(&dc(-6.0, ms(50.0)), &[64], |i, o| used.process(i, o));
    assert!(used.is_open());
    used.reset();
    assert!(!used.is_open());
    let x = dc(-30.0, ms(20.0));
    let a = run(&x, &[64], |i, o| used.process(i, o));
    let mut fresh = Gate::new(SR);
    let b = run(&x, &[64], |i, o| fresh.process(i, o));
    assert_eq!(a, b);
}

#[test]
fn a_change_of_rate_keeps_the_settings_and_the_times() {
    let mut g = Gate::new(SR);
    g.set_threshold_db(-30.0);
    g.set_hold_ms(0.0);
    g.set_release_ms(10.0);
    g.set_sample_rate(96_000.0);
    // The hold is gone and the release is 10 ms whatever the rate: 100 ms
    // after a burst ends, the gate is shut at the new rate too.
    let mut x = sine(1000.0, 0.5, 0.05);
    x.extend(std::iter::repeat_n(0.0, 96_000 / 10));
    let y = run(&x, &[64], |i, o| g.process(i, o));
    assert!(!g.is_open(), "closed again after the burst");
    assert!(y.iter().all(|v| v.is_finite()));
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = Gate::new(0.0);
}
