//! The convolver against the definition: a direct convolution in `f64`.
//!
//! Every IR length that matters to the partitioning, from one tap through
//! one short of a partition, one over, several partitions and a full
//! cabinet, at the
//! partition size chosen for it and at fixed ones either side, fed in single
//! samples, in steady blocks, and in a ragged schedule that never lines up
//! with a partition boundary.

mod common;

use common::{cabinet, noise};
use valrig_core::dsp::convolution::{Convolver, partition_for};

fn direct_f64(ir: &[f32], x: &[f32]) -> Vec<f64> {
    let mut y = vec![0.0f64; x.len()];
    for (t, out) in y.iter_mut().enumerate() {
        let mut acc = 0.0f64;
        for (i, &h) in ir.iter().enumerate().take(t + 1) {
            acc += h as f64 * x[t - i] as f64;
        }
        *out = acc;
    }
    y
}

fn build(ir: &[f32], partition: Option<usize>) -> Convolver {
    match partition {
        Some(p) => Convolver::with_partition(ir, p),
        None => Convolver::new(ir),
    }
}

/// Run `x` through a fresh convolver in blocks drawn cyclically from
/// `schedule`.
fn run(ir: &[f32], x: &[f32], partition: Option<usize>, schedule: &[usize]) -> Vec<f32> {
    let mut c = build(ir, partition);
    common::run(x, schedule, |i, o| c.process(i, o))
}

fn worst_relative_error(got: &[f32], want: &[f64]) -> f64 {
    let peak = want
        .iter()
        .fold(0.0f64, |m, v| m.max(v.abs()))
        .max(f64::MIN_POSITIVE);
    got.iter()
        .zip(want)
        .fold(0.0f64, |m, (&g, &w)| m.max((g as f64 - w).abs()))
        / peak
}

const PARTITIONS: [Option<usize>; 4] = [None, Some(16), Some(64), Some(256)];

#[test]
fn the_partition_follows_the_ir_length_within_bounds() {
    assert_eq!(partition_for(0), 16);
    assert_eq!(partition_for(100), 16);
    assert_eq!(partition_for(512), 32);
    assert_eq!(partition_for(2048), 64);
    assert_eq!(partition_for(8192), 128);
    assert_eq!(partition_for(24_000), 256);
    assert_eq!(partition_for(1_000_000), 256);
}

#[test]
fn an_empty_ir_is_silence_and_a_one_tap_ir_is_a_gain() {
    let x: Vec<f32> = (0..100).map(|i| (i as f32 * 0.1).sin()).collect();
    let mut y = vec![1.0f32; 100];
    Convolver::new(&[]).process(&x, &mut y);
    assert!(y.iter().all(|&v| v == 0.0));
    Convolver::new(&[0.5]).process(&x, &mut y);
    for (a, b) in x.iter().zip(&y) {
        assert_eq!(b.to_bits(), (0.5 * a).to_bits());
    }
}

/// The direct head reproduces the first `P` taps of the IR exactly: an
/// impulse in, and every tap comes out as the single non-zero product.
#[test]
fn an_impulse_returns_the_head_exactly_and_the_tail_closely() {
    let ir: Vec<f32> = (0..1000)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
        .collect();
    let mut c = Convolver::new(&ir);
    let mut input = vec![0.0f32; 1000];
    input[0] = 1.0;
    let mut out = vec![0.0f32; 1000];
    for (x, y) in input.chunks(64).zip(out.chunks_mut(64)) {
        c.process(x, y);
    }
    let p = c.partition();
    for i in 0..p {
        assert_eq!(out[i].to_bits(), ir[i].to_bits(), "tap {i}");
    }
    for i in p..1000 {
        assert!(
            (out[i] - ir[i]).abs() < 1e-5,
            "tap {i}: {} vs {}",
            out[i],
            ir[i]
        );
    }
}

#[test]
fn reset_forgets_the_input() {
    let ir: Vec<f32> = (0..300).map(|i| 1.0 / (i as f32 + 1.0)).collect();
    let mut fresh = Convolver::new(&ir);
    let mut used = fresh.clone();
    let x: Vec<f32> = (0..96).map(|i| (i as f32 * 0.3).cos()).collect();
    let mut scratch = vec![0.0f32; 96];
    used.process(&x, &mut scratch);
    used.reset();
    let (mut a, mut b) = (vec![0.0f32; 96], vec![0.0f32; 96]);
    fresh.process(&x, &mut a);
    used.process(&x, &mut b);
    assert_eq!(a, b);
}

#[test]
fn the_ir_length_is_reported_as_given() {
    assert_eq!(Convolver::new(&[]).ir_len(), 0);
    assert_eq!(Convolver::new(&vec![0.1; 4097]).ir_len(), 4097);
}

#[test]
#[should_panic(expected = "partition must be a power of two")]
fn a_partition_that_is_not_a_power_of_two_is_refused() {
    let _ = Convolver::with_partition(&[0.1, 0.2], 100);
}

#[test]
#[should_panic(expected = "partition must be a power of two")]
fn a_partition_large_enough_to_exhaust_memory_is_refused() {
    let _ = Convolver::with_partition(&[0.1, 0.2], 1 << 40);
}

#[test]
fn matches_a_double_precision_direct_convolution() {
    // Single-precision transforms over a 24,000-tap IR land a few parts in
    // 10^7 from the double-precision definition; this is that, with headroom.
    const TOL: f64 = 1e-5;
    let lengths = [
        1usize, 2, 15, 16, 17, 63, 64, 65, 100, 255, 256, 257, 1000, 4097, 24_000,
    ];
    let mut worst = (0.0f64, String::new());
    let mut failures = Vec::new();
    for &len in &lengths {
        let ir = cabinet(0xAB ^ len as u64, len);
        let x = noise(
            0xC0FFEE ^ len as u64,
            if len > 5000 { 2048 } else { 4096 },
            0.5,
        );
        let want = direct_f64(&ir, &x);
        for partition in PARTITIONS {
            for schedule in [vec![1usize], vec![64], vec![512], vec![1, 7, 64, 3, 13, 2]] {
                let got = run(&ir, &x, partition, &schedule);
                let err = worst_relative_error(&got, &want);
                let what = format!("ir {len}, partition {partition:?}, schedule {schedule:?}");
                if err > worst.0 {
                    worst = (err, what.clone());
                }
                if err > TOL {
                    failures.push(format!("{what}: relative error {err:.3e}"));
                }
            }
        }
    }
    eprintln!("worst relative error {:.3e} at {}", worst.0, worst.1);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Splitting the stream into blocks any way at all gives byte-identical
/// output.
#[test]
fn output_is_independent_of_how_frames_are_grouped() {
    let ir = cabinet(7, 24_000);
    let x = noise(11, 3000, 0.5);
    for partition in PARTITIONS {
        let reference = run(&ir, &x, partition, &[512]);
        for schedule in [
            vec![1usize],
            vec![7],
            vec![1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233],
            vec![500, 1, 500, 2],
            vec![63, 65],
        ] {
            let got = run(&ir, &x, partition, &schedule);
            common::assert_same_bits(
                &got,
                &reference,
                &format!("partition {partition:?}, schedule {schedule:?}"),
            );
        }
    }
}

/// No latency: the response to an impulse starts on the impulse, whatever
/// the block size, and the first partition of it is exact.
#[test]
fn latency_is_zero() {
    let ir = cabinet(3, 5000);
    let mut x = vec![0.0f32; 1024];
    x[0] = 1.0;
    for partition in PARTITIONS {
        for block in [1usize, 64, 512] {
            let y = run(&ir, &x, partition, &[block]);
            let c = build(&ir, partition);
            assert_eq!(c.latency(), 0);
            for i in 0..c.partition() {
                assert_eq!(
                    y[i].to_bits(),
                    ir[i].to_bits(),
                    "partition {partition:?}, tap {i}"
                );
            }
            assert!((y[c.partition()] - ir[c.partition()]).abs() < 1e-6);
        }
    }
}
