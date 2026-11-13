//! What the engine costs, against the time it has: the convolver on its own
//! at every partition size, and a whole rack per block. Ignored by default,
//! because it measures rather than checks:
//!
//! ```text
//! cargo test --release --test benchmark -- --ignored --nocapture
//! ```
//!
//! A debug build measures the debug build and tells you nothing about what
//! ships; always run this with `--release`.

mod common;

use std::hint::black_box;
use std::time::Instant;

use common::{cabinet, impulse_response, lane_chain, model, noise};
use valrig_core::cabinet::Cabinet;
use valrig_core::dsp::convolution::{Convolver, partition_for};
use valrig_core::rack::Rack;

/// The measurement behind `partition_for`: steady-state throughput over
/// cabinet-sized IRs at every partition size, as multiples of real time at
/// 48 kHz. Construction is outside the clock; `*` marks what
/// `Convolver::new` picks.
///
/// It measures rather than asserts, so it is not part of a normal run:
///
/// ```text
/// cargo test --release --test benchmark -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measures rather than checks; run with --release --ignored"]
fn partition_sweep() {
    if cfg!(debug_assertions) {
        eprintln!("warning: built without optimisations; these numbers mean nothing.");
    }
    let sample_rate = 48_000.0;
    let seconds = 2.0;
    let frames = (sample_rate * seconds) as usize;
    let input = noise(0xC0FFEE, frames, 0.3);
    let mut output = vec![0.0f32; frames];
    let partitions = [32usize, 64, 128, 256, 512];

    print!("{:>8} {:>6} |", "ir taps", "block");
    for p in partitions {
        print!(" {:>8}", format!("P={p}"));
    }
    println!();
    for &len in &[512usize, 2048, 8192, 24_000, 96_000] {
        let ir = cabinet(0xAB, len);
        for &block in &[64usize, 512] {
            print!("{len:>8} {block:>6} |");
            for p in partitions {
                let mut c = Convolver::with_partition(&ir, p);
                let mut sweep = || {
                    for pos in (0..frames).step_by(block) {
                        let n = block.min(frames - pos);
                        c.process(
                            black_box(&input[pos..pos + n]),
                            black_box(&mut output[pos..pos + n]),
                        );
                    }
                };
                sweep();
                let mut times = Vec::with_capacity(5);
                for _ in 0..5 {
                    let start = Instant::now();
                    sweep();
                    times.push(start.elapsed().as_secs_f64());
                }
                times.sort_by(f64::total_cmp);
                let times_real_time = seconds / times[times.len() / 2];
                let mark = if p == partition_for(len) { "*" } else { " " };
                print!(" {times_real_time:>7.0}{mark}");
            }
            println!();
        }
    }
}

const RATE: f64 = 48_000.0;
const BLOCK: usize = 128;
/// Blocks run before timing starts, so the caches and the branch
/// predictors are in the state they would be in during playback.
const WARMUP: usize = 200;
const ROUNDS: usize = 3000;

/// Microseconds per block, and the worst single block seen.
fn measure(rack: &mut Rack, input: &[f32]) -> (f64, f64) {
    let (mut left, mut right) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let channels = rack.channels_out();
    let mut go = |rack: &mut Rack| {
        let mut outs = [&mut left[..], &mut right[..]];
        rack.process(input, &mut outs[..channels]);
    };
    for _ in 0..WARMUP {
        go(rack);
    }
    let mut worst = 0.0f64;
    let start = Instant::now();
    for _ in 0..ROUNDS {
        let one = Instant::now();
        go(rack);
        worst = worst.max(one.elapsed().as_secs_f64() * 1e6);
    }
    (start.elapsed().as_secs_f64() * 1e6 / ROUNDS as f64, worst)
}

#[test]
#[ignore = "measures rather than checks; run with --release --ignored"]
fn two_captures_in_lanes_sequential_against_workers() {
    let budget_us = BLOCK as f64 / RATE * 1e6;
    let ir = impulse_response(2048, 99);
    let input: Vec<f32> = (0..BLOCK).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
    println!("block {BLOCK} at {RATE} Hz: {budget_us:.0} us budget");
    for parallel in [false, true] {
        let (mut rack, _) = lane_chain(vec![
            ("A", model("a2.nam"), Some(Cabinet::new(&ir, None))),
            (
                "B",
                model("wavenet_a1_standard.nam"),
                Some(Cabinet::new(&ir, None)),
            ),
        ]);
        rack.set_parallel_lanes(parallel, None);
        rack.prepare(RATE, BLOCK).unwrap();
        let (per_block, worst) = measure(&mut rack, &input);
        println!(
            "{:10}  {per_block:7.1} us per block ({:.0}% of budget), worst {worst:.0} us",
            if parallel { "workers" } else { "sequential" },
            per_block / budget_us * 100.0
        );
    }
}
