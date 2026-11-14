//! The tone control against its own definition: transparency when flat, the
//! response of each band, the glide, and independence from block grouping.

mod common;

use common::{SR, assert_same_bits, independent_of_block_grouping, noise, rms, run, sine};
use valverig_core::eq::{BASS_HZ, MID_HZ, Mono as ThreeBand, TREBLE_HZ};

/// The steady-state gain at `hz`, in dB: the last 0.2 s of a 0.5 s sine, an
/// integer number of periods at every frequency used below.
fn gain_db_at(eq: &mut ThreeBand, hz: f32) -> f32 {
    let x = sine(hz, 0.5, 0.5);
    let y = run(&x, &[64], |i, o| eq.process(i, o));
    let tail = x.len() - (0.2 * SR as f32) as usize;
    20.0 * (rms(&y[tail..]) / rms(&x[tail..])).log10()
}

#[test]
fn flat_is_transparent_bit_for_bit() {
    let mut eq = ThreeBand::new(SR);
    // A negative zero every so often: the copy keeps its sign.
    let x: Vec<f32> = noise(0x1357_9BDF, 4096, 0.5)
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 100 == 0 { -0.0 } else { *v })
        .collect();
    let y = run(&x, &[128], |i, o| eq.process(i, o));
    assert_same_bits(&y, &x, "flat");
    // Back to flat after a boost, it is exact again once the tail has died.
    eq.set_bass_db(6.0);
    run(&x, &[128], |i, o| eq.process(i, o));
    eq.set_bass_db(0.0);
    run(&vec![0.0f32; 48_000], &[128], |i, o| eq.process(i, o));
    let y = run(&x, &[128], |i, o| eq.process(i, o));
    assert_same_bits(&y, &x, "after returning to flat")
}

#[test]
fn each_band_shapes_its_own_range() {
    // Bass: +6 dB well below the shelf, nothing above it.
    let mut eq = ThreeBand::new(SR);
    eq.set_bass_db(6.0);
    assert!(
        gain_db_at(&mut eq, 20.0) > 5.5,
        "bass at 20 Hz: {}",
        gain_db_at(&mut eq, 20.0)
    );
    assert!(
        (gain_db_at(&mut eq, BASS_HZ) - 3.0).abs() < 0.3,
        "half the gain at the corner"
    );
    assert!(
        gain_db_at(&mut eq, 1000.0).abs() < 0.5,
        "bass leaves 1 kHz alone"
    );
    // Treble: +6 dB well above the shelf, nothing below it.
    let mut eq = ThreeBand::new(SR);
    eq.set_treble_db(6.0);
    assert!(
        gain_db_at(&mut eq, 15_000.0) > 5.5,
        "treble at 15 kHz: {}",
        gain_db_at(&mut eq, 15_000.0)
    );
    assert!(
        (gain_db_at(&mut eq, TREBLE_HZ) - 3.0).abs() < 0.3,
        "half the gain at the corner"
    );
    assert!(
        gain_db_at(&mut eq, 500.0).abs() < 0.5,
        "treble leaves 500 Hz alone"
    );
    // Mid: +6 dB at the centre, little two octaves either side.
    let mut eq = ThreeBand::new(SR);
    eq.set_mid_db(6.0);
    assert!(
        (gain_db_at(&mut eq, MID_HZ) - 6.0).abs() < 0.1,
        "mid at its centre"
    );
    assert!(
        gain_db_at(&mut eq, 100.0).abs() < 0.5,
        "mid leaves 100 Hz alone"
    );
    assert!(
        gain_db_at(&mut eq, 6000.0).abs() < 0.5,
        "mid leaves 6 kHz alone"
    );
    // Cuts are cuts.
    let mut eq = ThreeBand::new(SR);
    eq.set_bass_db(-6.0);
    assert!(
        gain_db_at(&mut eq, 20.0) < -5.5,
        "bass cut at 20 Hz: {}",
        gain_db_at(&mut eq, 20.0)
    );
}

#[test]
fn a_change_of_gain_glides_and_settles() {
    let mut eq = ThreeBand::new(SR);
    let x = sine(40.0, 0.25, 0.5);
    let period = (SR / 40.0) as usize;
    run(&x, &[64], |i, o| eq.process(i, o));
    eq.set_bass_db(12.0);
    let y = run(&x, &[64], |i, o| eq.process(i, o));
    // Level per 40 Hz period: rising every period, never overshooting, and
    // within half a dB of the setting after 300 ms.
    let levels: Vec<f32> = y
        .chunks_exact(period)
        .zip(x.chunks_exact(period))
        .map(|(o, i)| 20.0 * (rms(o) / rms(i)).log10())
        .collect();
    for w in levels.windows(2) {
        assert!(w[1] >= w[0] - 0.01, "the glide is monotonic: {:?}", w);
    }
    assert!(levels.iter().all(|&l| l < 12.6), "no overshoot");
    assert!((levels[12] - 12.0).abs() < 0.5, "settled: {}", levels[12]);
}

#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let x: Vec<f32> = (0..48_000)
        .map(|i| {
            let t = i as f32 / SR as f32;
            ((t * std::f32::consts::TAU * 82.4).sin()
                + (t * std::f32::consts::TAU * 1234.5).sin() * 0.3)
                * 0.4
        })
        .collect();
    independent_of_block_grouping(
        &x,
        || {
            let mut eq = ThreeBand::new(SR);
            eq.set_bass_db(3.0);
            eq.set_mid_db(-4.0);
            eq.set_treble_db(5.0);
            eq.reset(); // lands the gains, so the knobs are still
            eq
        },
        |eq, i, o| eq.process(i, o),
    );
}

#[test]
fn reset_forgets_the_signal_and_lands_the_gains() {
    let mut used = ThreeBand::new(SR);
    used.set_treble_db(9.0);
    run(&sine(1000.0, 0.5, 0.1), &[64], |i, o| used.process(i, o));
    used.reset();
    let mut fresh = ThreeBand::new(SR);
    fresh.set_treble_db(9.0);
    fresh.reset();
    let x = sine(3000.0, 0.3, 0.05);
    let (a, b) = (
        run(&x, &[64], |i, o| used.process(i, o)),
        run(&x, &[64], |i, o| fresh.process(i, o)),
    );
    assert_eq!(a, b);
    assert_eq!(used.gains_db(), [0.0, 0.0, 9.0]);
}

#[test]
fn the_rate_can_change_without_losing_the_settings() {
    let mut eq = ThreeBand::new(44_100.0);
    eq.set_mid_db(6.0);
    eq.reset();
    eq.set_sample_rate(SR);
    assert!((gain_db_at(&mut eq, MID_HZ) - 6.0).abs() < 0.1);
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_sample_rate_is_refused() {
    let _ = ThreeBand::new(1_000_000.0);
}
