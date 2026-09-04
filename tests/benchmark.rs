//! What a rack costs per block, against the time a block has to be ready
//! in. Ignored by default, because it measures rather than checks:
//!
//! ```text
//! cargo test --release --test benchmark -- --ignored --nocapture
//! ```
//!
//! A debug build measures the debug build and tells you nothing about what
//! ships; always run this with `--release`.

mod common;

use common::{impulse_response, lane_chain, model};
use std::time::Instant;
use valverig_core::cabinet::Cabinet;
use valverig_core::rack::Rack;

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
