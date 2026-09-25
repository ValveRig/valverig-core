//! The rack against its modules run by hand, its lanes and mix, and the
//! controls on it.

mod common;

use common::{
    assert_same_bits, impulse_response, lane_chain, model, moved_and_back, noise, run_rack,
};
use valverig_core::Error;
use valverig_core::cabinet::Cabinet;
use valverig_core::dsp::convolution::Convolver;
use valverig_core::gain::Gain;
use valverig_core::node::Node;
use valverig_core::rack::{Lane, Rack};

fn ir() -> Vec<f32> {
    impulse_response(2048, 99)
}

/// The capture and cabinet by hand, at 0 dB with the dynamics out and the
/// tone flat: what a lane or a mono rack must reproduce bit for bit.
fn by_hand(name: &str, ir: &[f32], x: &[f32], block: usize) -> Vec<f32> {
    let mut m = model(name);
    m.reset(48_000.0, block);
    let mut cab = Convolver::new(ir);
    let mut amp = vec![0.0f32; x.len()];
    let mut out = vec![0.0f32; x.len()];
    for ((xb, ab), ob) in x
        .chunks(block)
        .zip(amp.chunks_mut(block))
        .zip(out.chunks_mut(block))
    {
        m.process_mono(xb, ab);
        cab.process(ab, ob);
    }
    out
}

#[test]
fn the_standard_rack_is_the_capture_and_cabinet_run_by_hand() {
    let ir = ir();
    let (mut rack, controls) = common::chain(
        model("wavenet.nam"),
        Some(Cabinet::new(&ir, Some(48_000.0))),
    );
    // The gate would attenuate the first milliseconds while it opens and the
    // compressor would act on the peaks; with both out of the path the rack
    // is two gains at 0 dB, a flat tone control and a dry reverb around the
    // two modules, and flat and dry are exact.
    controls.set_bypass("gate", true).unwrap();
    controls.set_bypass("compressor", true).unwrap();
    controls.set("reverb/mix", 0.0).unwrap();
    rack.prepare(48_000.0, 64).unwrap();
    assert_eq!(rack.channels_out(), 1);
    let x = noise(1, 4096, 0.4);
    let got = run_rack(&mut rack, &x, &[64]);
    assert_same_bits(&got[0], &by_hand("wavenet.nam", &ir, &x, 64), "mono rack");
}

#[test]
fn two_lanes_hard_panned_are_two_rigs_side_by_side() {
    let ir = ir();
    let (mut rack, controls) = lane_chain(vec![
        (
            "A",
            model("wavenet.nam"),
            Some(Cabinet::new(&ir, Some(48_000.0))),
        ),
        (
            "B",
            model("lstm.nam"),
            Some(Cabinet::new(&ir, Some(48_000.0))),
        ),
    ]);
    controls.set_bypass("gate", true).unwrap();
    controls.set_bypass("compressor", true).unwrap();
    controls.set("reverb/mix", 0.0).unwrap();
    assert_eq!(controls.get("A/pan").unwrap(), -1.0);
    assert_eq!(controls.get("B/pan").unwrap(), 1.0);
    rack.prepare(48_000.0, 64).unwrap();
    assert_eq!(rack.channels_out(), 2);
    let x = noise(2, 4096, 0.4);
    let got = run_rack(&mut rack, &x, &[64]);
    // Hard pans are exact, and the other lane adds exactly nothing.
    assert_same_bits(
        &got[0],
        &by_hand("wavenet.nam", &ir, &x, 64),
        "left is lane A",
    );
    assert_same_bits(
        &got[1],
        &by_hand("lstm.nam", &ir, &x, 64),
        "right is lane B",
    );
}

#[test]
fn a_lane_in_the_centre_is_three_db_down_on_each_side() {
    let (mut rack, _) = Rack::builder()
        .split(vec![Lane::new("A").push("g", Gain::new()).unwrap()])
        .unwrap()
        .build();
    rack.prepare(48_000.0, 64).unwrap();
    let x = noise(3, 256, 0.5);
    let got = run_rack(&mut rack, &x, &[64]);
    assert_eq!(got[0], got[1]);
    let want = std::f32::consts::FRAC_1_SQRT_2;
    for (a, b) in got[0].iter().zip(&x) {
        assert!((a - b * want).abs() < 1e-6);
    }
}

#[test]
fn lane_level_and_bypass_control_the_mix() {
    let (mut rack, controls) = Rack::builder()
        .split(vec![
            Lane::new("A").pan(-1.0).push("ga", Gain::new()).unwrap(),
            Lane::new("B")
                .pan(1.0)
                .level_db(-6.0)
                .push("gb", Gain::new())
                .unwrap(),
        ])
        .unwrap()
        .build();
    rack.prepare(48_000.0, 64).unwrap();
    let x = noise(4, 256, 0.5);
    let got = run_rack(&mut rack, &x, &[64]);
    assert_same_bits(&got[0], &x, "A at 0 dB, hard left");
    let g = 10f32.powf(-6.0 / 20.0);
    for (a, b) in got[1].iter().zip(&x) {
        assert_eq!(a.to_bits(), (b * g).to_bits(), "B at -6 dB, hard right");
    }
    // A bypassed lane adds nothing.
    controls.set_bypass("B", true).unwrap();
    assert!(controls.bypassed("B").unwrap());
    let got = run_rack(&mut rack, &x, &[64]);
    assert!(got[1].iter().all(|&v| v == 0.0));
    assert_same_bits(&got[0], &x, "A untouched");
}

#[test]
fn the_reverb_after_the_lanes_keeps_the_pair() {
    let (mut rack, controls) = lane_chain(vec![
        ("A", model("wavenet.nam"), None),
        ("B", model("lstm.nam"), None),
    ]);
    controls.set("reverb/mix", 0.5).unwrap();
    rack.prepare(48_000.0, 64).unwrap();
    let mut x = vec![0.0f32; 48_000];
    x[0] = 0.5;
    let got = run_rack(&mut rack, &x, &[64]);
    let energy = |s: &[f32]| s.iter().map(|v| (v * v) as f64).sum::<f64>();
    assert!(
        energy(&got[0][24_000..]) > 0.0 && energy(&got[1][24_000..]) > 0.0,
        "a tail on both sides"
    );
    assert!(got[0] != got[1], "and they differ");
}

#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let x = noise(5, 3000, 0.4);
    for wide in [false, true] {
        let build = || {
            let (mut r, controls) = if wide {
                lane_chain(vec![
                    ("A", model("wavenet.nam"), Some(Cabinet::new(&ir(), None))),
                    ("B", model("lstm.nam"), None),
                ])
            } else {
                common::chain(model("wavenet.nam"), Some(Cabinet::new(&ir(), None)))
            };
            controls.set("reverb/mix", 0.3).unwrap();
            r.prepare(48_000.0, 128).unwrap();
            r
        };
        let reference = run_rack(&mut build(), &x, &[128]);
        for schedule in [
            vec![1usize],
            vec![7],
            vec![1, 2, 3, 5, 8, 13, 21, 34, 55, 89],
            vec![128, 1, 128, 2],
            vec![63, 65],
        ] {
            let got = run_rack(&mut build(), &x, &schedule);
            for (c, (a, b)) in reference.iter().zip(&got).enumerate() {
                assert_same_bits(
                    a,
                    b,
                    &format!("wide {wide}, schedule {schedule:?}, channel {c}"),
                );
            }
        }
    }
}

#[test]
fn names_and_addresses_are_stable() {
    let (mut rack, controls) = lane_chain(vec![
        ("A", model("wavenet.nam"), Some(Cabinet::new(&ir(), None))),
        ("B", model("lstm.nam"), None),
    ]);
    rack.prepare(48_000.0, 64).unwrap();
    let modules: Vec<(String, &str)> = controls.modules();
    assert_eq!(
        modules,
        vec![
            ("input".to_string(), "std.gain"),
            ("gate".to_string(), "std.gate"),
            ("compressor".to_string(), "std.compressor"),
            ("A".to_string(), "std.lane"),
            ("A.capture".to_string(), "std.capture"),
            ("A.eq".to_string(), "std.eq"),
            ("A.cabinet".to_string(), "std.cabinet"),
            ("B".to_string(), "std.lane"),
            ("B.capture".to_string(), "std.capture"),
            ("B.eq".to_string(), "std.eq"),
            ("reverb".to_string(), "std.reverb"),
            ("output".to_string(), "std.gain"),
        ]
    );
    let addresses: Vec<String> = controls.params().into_iter().map(|(a, _, _)| a).collect();
    assert!(addresses.contains(&"A/level_db".to_string()));
    assert!(addresses.contains(&"A/pan".to_string()));
    assert!(addresses.contains(&"B.eq/mid_db".to_string()));
    assert_eq!(addresses.first().unwrap(), "input/level_db");
    assert_eq!(addresses.last().unwrap(), "output/level_db");

    controls.set("output/level_db", 100.0).unwrap();
    assert_eq!(
        controls.get("output/level_db").unwrap(),
        24.0,
        "clamped to the spec's range"
    );
    assert!(matches!(
        controls.set("output/nope", 1.0),
        Err(Error::UnknownParam(_))
    ));
    assert!(matches!(
        controls.set("nope/level_db", 1.0),
        Err(Error::UnknownName(_))
    ));
    assert!(matches!(
        controls.set("level_db", 1.0),
        Err(Error::BadAddress(_))
    ));
    let level = controls.param("A/level_db").unwrap();
    level.set(-12.0);
    assert_eq!(controls.clone().get("A/level_db").unwrap(), -12.0);
}

#[test]
fn names_are_unique_across_modules_and_lanes() {
    assert!(matches!(
        Rack::builder()
            .push("g", Gain::new())
            .unwrap()
            .push("g", Gain::new()),
        Err(Error::DuplicateName(_))
    ));
    assert!(matches!(
        Rack::builder()
            .push("A", Gain::new())
            .unwrap()
            .split(vec![Lane::new("A")]),
        Err(Error::DuplicateName(_))
    ));
    assert!(matches!(
        Rack::builder()
            .push("x", Gain::new())
            .unwrap()
            .split(vec![Lane::new("A").push("x", Gain::new()).unwrap()]),
        Err(Error::DuplicateName(_))
    ));
    assert!(matches!(
        Lane::new("A").push("A", Gain::new()),
        Err(Error::DuplicateName(_))
    ));
    assert!(matches!(Rack::builder().split(vec![]), Err(Error::NoLanes)));
}

#[test]
fn a_capture_refuses_to_sit_after_a_mix() {
    let (mut rack, _) = Rack::builder()
        .split(vec![Lane::new("A").push("g", Gain::new()).unwrap()])
        .unwrap()
        .push(
            "amp",
            valverig_core::capture::Capture::new(model("wavenet.nam")),
        )
        .unwrap()
        .build();
    match rack.prepare(48_000.0, 64) {
        Err(Error::Channels { node, max, got }) => {
            assert_eq!((max, got), (1, 2));
            assert_eq!(node, "amp (std.capture)");
        }
        other => panic!("expected a channel refusal, got {other:?}"),
    }
}

#[test]
fn bypass_takes_a_module_out_of_the_path() {
    let x = noise(6, 1024, 0.4);
    let (mut rack, controls) = common::chain(model("wavenet.nam"), Some(Cabinet::new(&ir(), None)));
    rack.prepare(48_000.0, 128).unwrap();
    for name in ["gate", "compressor", "capture", "cabinet", "reverb"] {
        controls.set_bypass(name, true).unwrap();
    }
    let got = run_rack(&mut rack, &x, &[128]);
    assert_same_bits(&got[0], &x, "two gains at 0 dB and a flat tone control");
}

#[test]
fn a_gain_change_ramps_instead_of_stepping() {
    let (mut rack, controls) = Rack::builder().push("g", Gain::new()).unwrap().build();
    rack.prepare(48_000.0, 64).unwrap();
    let dc = vec![1.0f32; 64];
    let mut out = vec![0.0f32; 64];
    rack.process(&dc, &mut [&mut out]);
    assert!(out.iter().all(|&v| v == 1.0));
    controls.set("g/level_db", -20.0).unwrap();
    let mut previous = 1.0f32;
    let mut largest_step = 0.0f32;
    for _ in 0..(48_000 / 64 / 10) {
        rack.process(&dc, &mut [&mut out]);
        for &v in &out {
            assert!(v <= previous + 1e-7, "the ramp is monotonic");
            largest_step = largest_step.max(previous - v);
            previous = v;
        }
    }
    assert!(
        largest_step < 0.005,
        "largest per-sample step {largest_step}"
    );
    assert!(
        (previous - 0.1).abs() < 1e-3,
        "settled at -20 dB after 100 ms: {previous}"
    );
}

#[test]
fn a_module_refuses_the_wrong_sample_rate() {
    let (mut rack, _) = common::chain(model("wavenet.nam"), None);
    match rack.prepare(44_100.0, 64) {
        Err(Error::SampleRate { node, needs, got }) => {
            assert_eq!((needs, got), (48_000.0, 44_100.0));
            assert_eq!(node, "capture (std.capture)");
        }
        other => panic!("expected a sample-rate refusal, got {other:?}"),
    }
    let (mut rack, _) = Rack::builder()
        .push("cab", Cabinet::new(&ir(), Some(44_100.0)))
        .unwrap()
        .build();
    assert!(matches!(
        rack.prepare(48_000.0, 64),
        Err(Error::SampleRate { .. })
    ));
}

/// A module that declares latency, for the sums.
#[derive(Debug)]
struct Delayed(usize);

impl Node for Delayed {
    fn type_id(&self) -> &'static str {
        "test.delayed"
    }
    fn prepare(&mut self, _: f64, _: usize, _: usize) -> valverig_core::Result<()> {
        Ok(())
    }
    fn reset(&mut self) {}
    fn latency(&self) -> usize {
        self.0
    }
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        for (x, y) in input.iter().zip(output.iter_mut()) {
            y.copy_from_slice(x);
        }
    }
}

#[test]
fn latency_sums_the_stages_and_takes_the_longest_lane() {
    let (rack, controls) = Rack::builder()
        .push("a", Delayed(5))
        .unwrap()
        .split(vec![
            Lane::new("L").push("l", Delayed(7)).unwrap(),
            Lane::new("R")
                .push("r1", Delayed(2))
                .unwrap()
                .push("r2", Delayed(3))
                .unwrap(),
        ])
        .unwrap()
        .push("z", Delayed(1))
        .unwrap()
        .build();
    assert_eq!(rack.latency(), 5 + 7 + 1);
    controls.set_bypass("l", true).unwrap();
    assert_eq!(
        rack.latency(),
        13,
        "bypass must not move the latency under a host"
    );
}

#[test]
fn reset_returns_a_wavenet_rack_to_its_settled_state() {
    let x = noise(7, 2048, 0.4);
    let build = || {
        let (mut r, _) = lane_chain(vec![
            ("A", model("wavenet.nam"), Some(Cabinet::new(&ir(), None))),
            ("B", model("wavenet.nam"), None),
        ]);
        r.prepare(48_000.0, 64).unwrap();
        r
    };
    let fresh = run_rack(&mut build(), &x, &[64]);
    let mut used = build();
    run_rack(&mut used, &noise(8, 5000, 0.4), &[64]);
    used.reset();
    let after = run_rack(&mut used, &x, &[64]);
    for c in 0..2 {
        assert_same_bits(&fresh[c], &after[c], &format!("channel {c}"));
    }
}

#[test]
fn the_gate_closes_on_silence_and_opens_on_signal() {
    let (mut rack, controls) = common::chain(model("wavenet.nam"), None);
    rack.prepare(48_000.0, 64).unwrap();
    for name in ["capture", "compressor", "reverb"] {
        controls.set_bypass(name, true).unwrap();
    }
    controls.set("gate/threshold_db", -40.0).unwrap();
    let mut out = vec![0.0f32; 64];
    let hiss = vec![10f32.powf(-70.0 / 20.0); 64];
    for _ in 0..20 {
        rack.process(&hiss, &mut [&mut out]);
    }
    assert!(
        out.iter().all(|&v| v.abs() < 1e-7),
        "hiss is gated: {}",
        out[63]
    );
    let note = vec![10f32.powf(-10.0 / 20.0); 64];
    for _ in 0..40 {
        rack.process(&note, &mut [&mut out]);
    }
    assert_eq!(out[63].to_bits(), note[63].to_bits(), "open, and exact");
}

#[test]
fn the_compressor_evens_out_a_loud_note() {
    let (mut rack, controls) = common::chain(model("wavenet.nam"), None);
    rack.prepare(48_000.0, 64).unwrap();
    for name in ["gate", "capture", "reverb"] {
        controls.set_bypass(name, true).unwrap();
    }
    controls.set("compressor/threshold_db", -20.0).unwrap();
    controls.set("compressor/knee_db", 0.0).unwrap();
    controls.set("compressor/ratio", 4.0).unwrap();
    let loud = vec![10f32.powf(-6.0 / 20.0); 64];
    let mut out = vec![0.0f32; 64];
    for _ in 0..400 {
        rack.process(&loud, &mut [&mut out]);
    }
    let got = 20.0 * (out[63] / loud[63]).log10();
    assert!((got + 10.5).abs() < 0.05, "reduction {got} dB");
}

#[test]
fn the_tone_control_lifts_the_bass() {
    let (mut rack, controls) = common::chain(model("wavenet.nam"), None);
    rack.prepare(48_000.0, 64).unwrap();
    for name in ["gate", "compressor", "capture", "reverb"] {
        controls.set_bypass(name, true).unwrap();
    }
    controls.set("eq/bass_db", 6.0).unwrap();
    let x: Vec<f32> = (0..24_000)
        .map(|i| (i as f32 / 48_000.0 * 2.0 * std::f32::consts::PI * 40.0).sin() * 0.3)
        .collect();
    let y = run_rack(&mut rack, &x, &[64]);
    let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    let gain = 20.0 * (rms(&y[0][12_000..]) / rms(&x[12_000..])).log10();
    assert!(gain > 5.5 && gain < 6.5, "bass at 40 Hz: {gain} dB");
}

#[test]
fn a_block_larger_than_the_rack_was_prepared_for_is_split() {
    // CoreAudio can hand a callback more frames than it asked for. The rack
    // takes them, in blocks of at most what it was prepared for, and the
    // result is what the same frames give when the host splits them itself.
    let x = noise(9, 3000, 0.4);
    let prepared = 64;

    let (mut whole, _) = common::chain(model("wavenet.nam"), Some(Cabinet::new(&ir(), None)));
    whole.prepare(48_000.0, prepared).unwrap();
    let mut got = vec![0.0f32; x.len()];
    whole.process(&x, &mut [&mut got]);

    let (mut split, _) = common::chain(model("wavenet.nam"), Some(Cabinet::new(&ir(), None)));
    split.prepare(48_000.0, prepared).unwrap();
    let want = run_rack(&mut split, &x, &[prepared]);

    assert_same_bits(&got, &want[0], "3000 frames at once against 64 at a time");
}

#[test]
fn a_rack_and_its_controls_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Rack>();
    assert_send_sync::<valverig_core::rack::Controls>();
}

/// A wire that can be told to emit a NaN, for the output guard.
#[derive(Debug, Clone)]
struct Poison(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Node for Poison {
    fn type_id(&self) -> &'static str {
        "test.poison"
    }
    fn prepare(&mut self, _: f64, _: usize, _: usize) -> valverig_core::Result<()> {
        Ok(())
    }
    fn reset(&mut self) {}
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let poison = self.0.load(std::sync::atomic::Ordering::Relaxed);
        for (x, y) in input.iter().zip(output.iter_mut()) {
            y.copy_from_slice(x);
            if poison {
                y[0] = f32::NAN;
            }
        }
    }
}

#[test]
fn a_non_finite_block_is_silenced_and_the_feedback_path_is_cleared() {
    let poison = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (mut rack, controls) = Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("poison", Poison(poison.clone()))
        .unwrap()
        .push("reverb", valverig_core::reverb::Reverb::new())
        .unwrap()
        .build();
    rack.prepare(48_000.0, 64).unwrap();
    controls.set("reverb/mix", 0.5).unwrap();

    let x = noise(12, 64, 0.4);
    let mut out = vec![0.0f32; 64];
    rack.process(&x, &mut [&mut out]);
    assert!(!rack.faulted(), "nothing wrong yet");
    assert!(out.iter().all(|v| v.is_finite()));

    // One NaN into the reverb's recirculation.
    poison.store(true, std::sync::atomic::Ordering::Relaxed);
    rack.process(&x, &mut [&mut out]);
    assert!(
        out.iter().all(|&v| v == 0.0),
        "the block is silenced, not passed on"
    );
    assert!(rack.faulted(), "and the rack says so");

    // With the source of it gone, the rack comes back on its own: the
    // reverb was cleared inside the callback.
    poison.store(false, std::sync::atomic::Ordering::Relaxed);
    for _ in 0..4 {
        rack.process(&x, &mut [&mut out]);
    }
    assert!(out.iter().all(|v| v.is_finite()), "finite again");
    assert!(out.iter().any(|&v| v != 0.0), "and audible again");
    assert!(rack.faulted(), "the fault is remembered until a reset");
    rack.reset();
    assert!(!rack.faulted(), "which clears it");
}

#[test]
fn a_rate_outside_the_band_is_refused_before_any_module_sees_it() {
    let (mut rack, _) = Rack::builder()
        .push("drive", valverig_core::drive::Drive::new())
        .unwrap()
        .push("reverb", valverig_core::reverb::Reverb::new())
        .unwrap()
        .build();
    // Each of these would otherwise reach a module's own check and panic
    // there, on whichever thread prepared the rack.
    for rate in [
        0.0,
        500.0,
        valverig_core::MIN_SAMPLE_RATE - 1.0,
        valverig_core::MAX_SAMPLE_RATE + 1.0,
        f64::NAN,
        f64::INFINITY,
    ] {
        let e = rack.prepare(rate, 128).unwrap_err();
        assert!(
            matches!(e, Error::UnsupportedSampleRate(r) if r.to_bits() == rate.to_bits()),
            "{rate}: {e}"
        );
    }
    for rate in [
        valverig_core::MIN_SAMPLE_RATE,
        48_000.0,
        valverig_core::MAX_SAMPLE_RATE,
    ] {
        rack.prepare(rate, 128)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
    }
}

#[test]
fn a_gain_moved_and_brought_back_to_0_db_is_exact_again() {
    for away in [-60.0, -6.0, 24.0] {
        let (still, moved) = moved_and_back(
            || Rack::builder().push("g", Gain::new()).unwrap().build(),
            "g/level_db",
            away,
            0.0,
        );
        assert_same_bits(&moved[0], &still[0], &format!("back from {away} dB"));
    }
}

#[test]
fn a_hard_pan_moved_and_brought_back_is_exact_again() {
    let (still, moved) = moved_and_back(
        || {
            Rack::builder()
                .split(vec![
                    Lane::new("A").pan(-1.0).push("a", Gain::new()).unwrap(),
                ])
                .unwrap()
                .build()
        },
        "A/pan",
        0.0,
        -1.0,
    );
    assert_same_bits(&moved[0], &still[0], "left");
    assert_same_bits(&moved[1], &still[1], "right");
    assert!(moved[1].iter().all(|&v| v == 0.0), "nothing on the right");
}
