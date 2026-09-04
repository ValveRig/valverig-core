//! The cabinet's cuts: out of the path at the ends of their ranges, where
//! the cabinet is the convolver alone bit for bit; 12 dB an octave inside.

use valverig_core::cabinet::Cabinet;
use valverig_core::gain::Gain;
use valverig_core::rack::Rack;

const SR: f64 = 48_000.0;

fn sine(hz: f32, seconds: f32) -> Vec<f32> {
    (0..(seconds * SR as f32) as usize)
        .map(|i| (i as f32 / SR as f32 * 2.0 * std::f32::consts::PI * hz).sin() * 0.5)
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn rack() -> (Rack, valverig_core::rack::Controls) {
    // A one-tap IR: the convolver is a wire, so what is left is the cuts.
    let (mut rack, controls) = Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("cab", Cabinet::new(&[1.0], Some(SR)))
        .unwrap()
        .build();
    rack.prepare(SR, 128).unwrap();
    (rack, controls)
}

fn gain_db(rack: &mut Rack, hz: f32) -> f32 {
    rack.reset();
    let x = sine(hz, 0.5);
    let mut y = vec![0.0f32; x.len()];
    for (i, o) in x.chunks(128).zip(y.chunks_mut(128)) {
        rack.process(i, &mut [o]);
    }
    let tail = x.len() - 9600;
    20.0 * (rms(&y[tail..]) / rms(&x[tail..])).log10()
}

#[test]
fn with_both_cuts_out_the_cabinet_is_the_convolver_bit_for_bit() {
    let (mut rack, _) = rack();
    let x: Vec<f32> = (0..2048)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
        .collect();
    let mut y = vec![0.0f32; x.len()];
    for (i, o) in x.chunks(128).zip(y.chunks_mut(128)) {
        rack.process(i, &mut [o]);
    }
    for (i, (a, b)) in x.iter().zip(&y).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "sample {i}");
    }
}

#[test]
fn the_low_cut_takes_the_bottom_and_the_high_cut_the_top() {
    let (mut rack, controls) = rack();
    controls.set("cab/low_cut_hz", 200.0).unwrap();
    let low = gain_db(&mut rack, 50.0);
    let mid = gain_db(&mut rack, 2000.0);
    assert!(
        low < -20.0,
        "50 Hz under a 200 Hz cut: {low} dB, two octaves at 12 dB each"
    );
    assert!(mid.abs() < 0.5, "2 kHz under a 200 Hz cut: {mid} dB");
    controls.set("cab/low_cut_hz", 20.0).unwrap();
    controls.set("cab/high_cut_hz", 4000.0).unwrap();
    let top = gain_db(&mut rack, 16_000.0);
    let mid = gain_db(&mut rack, 1000.0);
    assert!(top < -20.0, "16 kHz over a 4 kHz cut: {top} dB");
    assert!(mid.abs() < 0.5, "1 kHz over a 4 kHz cut: {mid} dB");
    // Back out of the path, exact again.
    controls.set("cab/high_cut_hz", 20_000.0).unwrap();
    rack.reset();
    let x: Vec<f32> = (0..512)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
        .collect();
    let mut y = vec![0.0f32; x.len()];
    for (i, o) in x.chunks(128).zip(y.chunks_mut(128)) {
        rack.process(i, &mut [o]);
    }
    for (i, (a, b)) in x.iter().zip(&y).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "sample {i} with the cuts back out"
        );
    }
}
