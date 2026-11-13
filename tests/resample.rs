//! The converter against its definition: a tone comes through at its level
//! and at the right pitch, the images and what is above the lower Nyquist
//! are rejected, a round trip is the input on its own frames, and the
//! output does not depend on how the input arrives.

mod common;

use common::{goertzel, rms, sine_at as sine, to_db};
use valrig_core::dsp::resample::{Resampler, TAPS};

/// Everything `x` makes, pushed in `chunks` and pulled after each.
fn convert(r: &mut Resampler, x: &[f32], chunks: &[usize]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut buf = vec![0.0f32; 4096];
    let (mut pos, mut i) = (0, 0);
    while pos < x.len() {
        let n = chunks[i % chunks.len()].max(1).min(x.len() - pos);
        r.push(&x[pos..pos + n]);
        pos += n;
        i += 1;
        loop {
            let got = r.pull(&mut buf);
            out.extend_from_slice(&buf[..got]);
            if got < buf.len() {
                break;
            }
        }
    }
    out
}

#[test]
fn a_tone_comes_through_at_its_level_and_pitch_both_ways() {
    for (from, to) in [
        (44_100.0, 48_000.0),
        (48_000.0, 44_100.0),
        (96_000.0, 48_000.0),
        (48_000.0, 96_000.0),
    ] {
        let mut r = Resampler::new(from, to, 512);
        let x = sine(1000.0, 0.5, 1.0, from);
        let y = convert(&mut r, &x, &[512]);
        // All of it but the look-ahead's worth at the end.
        let expect = (x.len() as f64 * to / from) as usize;
        assert!(
            (y.len() as isize - expect as isize).abs() <= 2 * TAPS as isize,
            "{from}→{to}: {} frames out of {} in",
            y.len(),
            x.len()
        );
        let tail = &y[y.len() - (to as usize / 5)..];
        let level = to_db(rms(tail) / rms(&x[x.len() / 2..]));
        assert!(level.abs() < 0.05, "{from}→{to}: 1 kHz at {level} dB");
        // At the pitch it went in at: the whole tail is on that line.
        let on = goertzel(tail, 1000.0, to);
        assert!((on - 0.5).abs() < 0.01, "{from}→{to}: {on} on 1 kHz");
        // And the top of the band survives.
        let mut r = Resampler::new(from, to, 512);
        let x = sine(18_000.0, 0.5, 1.0, from);
        let y = convert(&mut r, &x, &[512]);
        let tail = &y[y.len() - (to as usize / 5)..];
        let top = to_db(goertzel(tail, 18_000.0, to) / 0.5);
        assert!(top > -1.0, "{from}→{to}: 18 kHz at {top} dB");
    }
}

#[test]
fn what_lies_above_the_lower_nyquist_is_rejected() {
    // 48 kHz into 44.1: a 23 kHz tone has nowhere to go and must not fold.
    let mut r = Resampler::new(48_000.0, 44_100.0, 512);
    let x = sine(23_000.0, 0.5, 1.0, 48_000.0);
    let y = convert(&mut r, &x, &[512]);
    let peak = y[y.len() - 8820..]
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        to_db(peak / 0.5) < -60.0,
        "23 kHz leaks through at {} dB",
        to_db(peak / 0.5)
    );
    // 96 into 48: 30 kHz likewise.
    let mut r = Resampler::new(96_000.0, 48_000.0, 512);
    let x = sine(30_000.0, 0.5, 1.0, 96_000.0);
    let y = convert(&mut r, &x, &[512]);
    let peak = y[y.len() - 9600..]
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        to_db(peak / 0.5) < -60.0,
        "30 kHz leaks through at {} dB",
        to_db(peak / 0.5)
    );
}

#[test]
fn a_round_trip_is_the_input_on_the_same_frames() {
    // The sinc is centred on the position it reads, so an output frame is
    // the input at that instant: the look-ahead is when a frame can be
    // made, not where it sits. Out and back, the input comes back on its
    // own frames.
    let (a, b) = (44_100.0, 48_000.0);
    let mut up = Resampler::new(a, b, 512);
    let mut down = Resampler::new(b, a, 1024);
    assert_eq!(up.lookahead(), TAPS / 2);
    let x = sine(440.0, 0.5, 1.0, a);
    let mid = convert(&mut up, &x, &[512]);
    let y = convert(&mut down, &mid, &[600]);
    let mut worst = 0.0f32;
    for (v, want) in y.iter().zip(&x).skip(4000) {
        worst = worst.max((v - want).abs());
    }
    assert!(
        to_db(worst / 0.5) < -60.0,
        "worst error {} dB",
        to_db(worst / 0.5)
    );
}

#[test]
fn output_does_not_depend_on_how_the_input_arrives() {
    let x: Vec<f32> = (0..20_000)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
        .collect();
    let mut a = Resampler::new(48_000.0, 44_100.0, 1024);
    let mut b = Resampler::new(48_000.0, 44_100.0, 1024);
    let ya = convert(&mut a, &x, &[256]);
    let yb = convert(&mut b, &x, &[1, 7, 300, 3, 64, 17, 5, 1024]);
    assert_eq!(ya.len(), yb.len());
    for (i, (p, q)) in ya.iter().zip(&yb).enumerate() {
        assert_eq!(p.to_bits(), q.to_bits(), "frame {i}");
    }
}

#[test]
fn priming_lets_a_block_be_pulled_at_once_and_reset_forgets() {
    let mut r = Resampler::new(48_000.0, 44_100.0, 256);
    assert_eq!(r.available(), 0);
    r.prime(256 + 2 * TAPS);
    assert!(
        r.available() >= 256,
        "{} frames after priming",
        r.available()
    );
    let mut out = vec![1.0f32; 256];
    assert_eq!(r.pull(&mut out), 256);
    assert!(out.iter().all(|v| *v == 0.0));
    r.push(&sine(1000.0, 0.5, 0.01, 48_000.0));
    r.reset();
    assert_eq!(r.available(), 0);
    assert!(r.room() > 1000);
}

#[test]
#[should_panic(expected = "sample rate must be between")]
fn an_impossible_rate_is_refused() {
    let _ = Resampler::new(48_000.0, 0.0, 512);
}

#[test]
#[should_panic(expected = "input frames asked for")]
fn asking_for_more_room_than_the_ring_can_hold_is_refused() {
    // Without the cap the size computation overflows and wraps to a
    // zero-length ring, and the first push indexes off the end of it.
    let _ = Resampler::new(48_000.0, 44_100.0, usize::MAX / 8);
}
