//! The tap to the output: a wire in the path whose copy of the block is
//! added to the output after every stage, levelled, and panned like a lane
//! when the output is a pair; nothing from one that did not run; in a lane
//! on a worker, bit for bit the sequential rack.

mod common;

use common::{assert_same_bits, from_db, moved_and_back, noise, run_rack};
use valrig_core::delay::Delay;
use valrig_core::gain::Gain;
use valrig_core::rack::{Lane, Rack};
use valrig_core::tap::Tap;

#[test]
fn a_tap_adds_its_point_in_the_chain_to_the_output() {
    let (mut rack, controls) = Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("tap", Tap::new())
        .unwrap()
        .push("output", Gain::new())
        .unwrap()
        .build();
    // Before prepare, so the gain starts there rather than gliding.
    controls.set("output/level_db", -6.0).unwrap();
    rack.prepare(48_000.0, 128).unwrap();
    assert_eq!(rack.channels_out(), 1);
    let x = noise(1, 4096, 0.5);
    let g = from_db(-6.0);
    let y = run_rack(&mut rack, &x, &[128, 1, 7, 64]);
    let want: Vec<f32> = x.iter().map(|v| v * g + v).collect();
    assert_same_bits(&y[0], &want, "the path at -6 dB plus the tap at unity");

    // A bypassed tap adds nothing.
    controls.set_bypass("tap", true).unwrap();
    let y = run_rack(&mut rack, &x, &[128]);
    let want: Vec<f32> = x.iter().map(|v| v * g).collect();
    assert_same_bits(&y[0], &want, "bypassed");

    // Back in, at its own level, with the path after it out.
    controls.set_bypass("tap", false).unwrap();
    controls.set_bypass("output", true).unwrap();
    controls.set("tap/level_db", -12.0).unwrap();
    rack.reset();
    let y = run_rack(&mut rack, &x, &[128]);
    let t = from_db(-12.0);
    let want: Vec<f32> = x.iter().map(|v| v + v * t).collect();
    assert_same_bits(&y[0], &want, "the tap at -12 dB over the bare path");
}

#[test]
fn a_tap_before_the_lanes_is_panned_into_the_pair_like_a_lane() {
    let build = || {
        Rack::builder()
            .push("input", Gain::new())
            .unwrap()
            .push("tap", Tap::new())
            .unwrap()
            .split(vec![
                Lane::new("A")
                    .pan(-1.0)
                    .push("A.gain", Gain::new())
                    .unwrap(),
                Lane::new("B").pan(1.0).push("B.gain", Gain::new()).unwrap(),
            ])
            .unwrap()
            .push("output", Gain::new())
            .unwrap()
            .build()
    };
    let (mut rack, controls) = build();
    controls.set("A.gain/level_db", -6.0).unwrap();
    controls.set("B.gain/level_db", 3.0).unwrap();
    rack.prepare(48_000.0, 128).unwrap();
    assert_eq!(rack.channels_out(), 2);
    let x = noise(2, 4096, 0.5);
    let y = run_rack(&mut rack, &x, &[128]);
    // A hard left at -6, B hard right at +3, the tap centred: a constant-
    // power pan puts it 3 dB down on each side.
    let (ga, gb) = (from_db(-6.0), from_db(3.0));
    let centre = std::f32::consts::FRAC_PI_4.cos();
    for i in 0..x.len() {
        let (wl, wr) = (x[i] * ga + x[i] * centre, x[i] * gb + x[i] * centre);
        assert!(
            (y[0][i] - wl).abs() < 1e-6 && (y[1][i] - wr).abs() < 1e-6,
            "sample {i}: {} {} against {wl} {wr}",
            y[0][i],
            y[1][i]
        );
    }
    // Panned hard right, it leaves the left side alone, exactly.
    controls.set("tap/pan", 1.0).unwrap();
    rack.reset();
    let y = run_rack(&mut rack, &x, &[128]);
    let left: Vec<f32> = x.iter().map(|v| v * ga).collect();
    assert_same_bits(&y[0], &left, "hard right leaves the left to lane A");
}

#[test]
fn a_tap_in_a_lane_on_a_worker_is_the_sequential_rack_bit_for_bit() {
    let build = || {
        Rack::builder()
            .push("input", Gain::new())
            .unwrap()
            .split(vec![
                Lane::new("A")
                    .pan(-0.5)
                    .push("A.tap", Tap::new())
                    .unwrap()
                    .push("A.delay", Delay::new())
                    .unwrap(),
                Lane::new("B")
                    .pan(0.5)
                    .push("B.gain", Gain::new())
                    .unwrap()
                    .push("B.tap", Tap::new())
                    .unwrap(),
                Lane::new("C").push("C.delay", Delay::new()).unwrap(),
            ])
            .unwrap()
            .push("tap", Tap::new())
            .unwrap()
            .push("output", Gain::new())
            .unwrap()
            .build()
    };
    let x = noise(3, 24_000, 0.4);
    let schedule = [128usize, 1, 7, 64, 3, 128, 33];
    let mut outs = Vec::new();
    for parallel in [false, true] {
        let (mut rack, controls) = build();
        rack.set_parallel_lanes(parallel, None);
        rack.prepare(48_000.0, 128).unwrap();
        assert_eq!(rack.parallel_lanes(), parallel);
        let mut y = vec![vec![0.0f32; x.len()]; 2];
        let (mut pos, mut i) = (0, 0);
        while pos < x.len() {
            let n = schedule[i % schedule.len()].min(x.len() - pos);
            // The host at the taps and the lanes while the rack runs.
            controls
                .set("A.tap/pan", -1.0 + (i % 5) as f32 * 0.5)
                .unwrap();
            controls.set("B.tap/level_db", -((i % 7) as f32)).unwrap();
            controls.set_bypass("B", i % 9 == 4).unwrap();
            controls.set_bypass("A.tap", i % 11 == 6).unwrap();
            controls
                .set("tap/level_db", -20.0 + (i % 3) as f32)
                .unwrap();
            let (l, r) = y.split_at_mut(1);
            let mut o = [&mut l[0][pos..pos + n], &mut r[0][pos..pos + n]];
            rack.process(&x[pos..pos + n], &mut o);
            pos += n;
            i += 1;
        }
        outs.push(y);
    }
    assert_same_bits(&outs[0][0], &outs[1][0], "left");
    assert_same_bits(&outs[0][1], &outs[1][1], "right");
    assert!(
        outs[0][0].iter().any(|v| v.abs() > 1e-3) && outs[0][1].iter().any(|v| v.abs() > 1e-3),
        "silence"
    );
    // And the taps are in there: the same rack with every tap bypassed is a
    // different signal.
    let (mut quiet, cq) = build();
    quiet.prepare(48_000.0, 128).unwrap();
    for name in ["A.tap", "B.tap", "tap"] {
        cq.set_bypass(name, true).unwrap();
    }
    let yq = run_rack(&mut quiet, &x, &[128]);
    assert!(
        yq[0]
            .iter()
            .zip(&outs[0][0])
            .any(|(a, b)| (a - b).abs() > 1e-3)
    );
}

#[test]
fn a_tap_into_a_mono_output_ramps_its_level_instead_of_stepping() {
    let (mut rack, controls) = Rack::builder()
        .push("tap", Tap::new())
        .unwrap()
        .push("output", Gain::new())
        .unwrap()
        .build();
    rack.prepare(48_000.0, 64).unwrap();
    assert_eq!(rack.channels_out(), 1);
    // The path silenced, so what comes out is the tap alone.
    controls.set_bypass("output", true).unwrap();
    let dc = vec![1.0f32; 64];
    let mut out = vec![0.0f32; 64];
    rack.process(&dc, &mut [&mut out]);
    assert!(
        out.iter().all(|&v| v == 2.0),
        "the wire at unity plus a tap at 0 dB, which a mono output does not pan down"
    );

    controls.set("tap/level_db", -20.0).unwrap();
    let mut previous = 2.0f32;
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
        (previous - 1.1).abs() < 1e-3,
        "the wire plus the tap at -20 dB after 100 ms: {previous}"
    );
}

#[test]
fn a_tap_moved_and_brought_back_to_unity_is_exact_again() {
    let build = || {
        Rack::builder()
            .push("t", Tap::new())
            .unwrap()
            .push("g", Gain::new())
            .unwrap()
            .build()
    };
    let (still, moved) = moved_and_back(build, "t/level_db", -12.0, 0.0);
    assert_same_bits(&moved[0], &still[0], "level");
    let lanes = || {
        Rack::builder()
            .push("t", Tap::new())
            .unwrap()
            .split(vec![Lane::new("A").push("a", Gain::new()).unwrap()])
            .unwrap()
            .build()
    };
    let (still, moved) = moved_and_back(lanes, "t/pan", 0.5, 0.0);
    assert_same_bits(&moved[0], &still[0], "pan, left");
    assert_same_bits(&moved[1], &still[1], "pan, right");
}
