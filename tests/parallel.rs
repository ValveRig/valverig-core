//! Lanes on threads of their own: bit for bit the sequential rack, in ragged
//! blocks, with lanes bypassed and unbypassed under it, and with three lanes
//! so two workers are in play.

mod common;

use common::{lane_chain, model, noise};
use valverig_core::cabinet::Cabinet;
use valverig_core::rack::{self, Rack};

fn rig(three: bool) -> (Rack, rack::Controls) {
    let ir: Vec<f32> = (0..1024)
        .map(|i| (-(i as f32) / 300.0).exp() * 0.3)
        .collect();
    let mut lanes = vec![
        ("A", model("wavenet.nam"), Some(Cabinet::new(&ir, None))),
        ("B", model("lstm.nam"), Some(Cabinet::new(&ir, None))),
    ];
    if three {
        lanes.push(("C", model("wavenet.nam"), None));
    }
    lane_chain(lanes)
}

fn run(
    rack: &mut Rack,
    controls: &rack::Controls,
    input: &[f32],
    blocks: &[usize],
) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![0.0f32; input.len()], vec![0.0f32; input.len()]);
    let (mut pos, mut i) = (0, 0);
    while pos < input.len() {
        let n = blocks[i % blocks.len()].min(input.len() - pos);
        // The host changing the mix and bypassing a lane under the rack.
        controls.set("A/pan", -1.0 + (i % 5) as f32 * 0.5).unwrap();
        controls.set("B/level_db", -((i % 7) as f32)).unwrap();
        controls.set_bypass("B", i % 9 == 4).unwrap();
        let mut outs = [&mut left[pos..pos + n], &mut right[pos..pos + n]];
        rack.process(&input[pos..pos + n], &mut outs);
        pos += n;
        i += 1;
    }
    (left, right)
}

#[test]
fn parallel_lanes_are_the_sequential_rack_bit_for_bit() {
    let input = noise(1, 48_000, 0.3);
    let blocks = [128usize, 1, 7, 64, 3, 128, 33];
    for three in [false, true] {
        let (mut seq, cs) = rig(three);
        seq.prepare(48_000.0, 128).unwrap();
        let (mut par, cp) = rig(three);
        par.set_parallel_lanes(true, None);
        par.prepare(48_000.0, 128).unwrap();
        assert!(par.parallel_lanes() && !seq.parallel_lanes());
        let want = run(&mut seq, &cs, &input, &blocks);
        let got = run(&mut par, &cp, &input, &blocks);
        for (side, (w, g)) in [("left", (&want.0, &got.0)), ("right", (&want.1, &got.1))] {
            let first = w
                .iter()
                .zip(g.iter())
                .position(|(a, b)| a.to_bits() != b.to_bits());
            assert!(
                first.is_none(),
                "three {three}, {side}: first difference at frame {}",
                first.unwrap()
            );
            assert!(
                w.iter().any(|v| v.abs() > 1e-4),
                "three {three}, {side}: silence"
            );
        }
    }
}

#[test]
fn a_hook_runs_once_per_worker_and_workers_stop_with_the_rack() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut rack, _) = rig(true);
    let c = calls.clone();
    rack.set_parallel_lanes(
        true,
        Some(Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        })),
    );
    rack.prepare(48_000.0, 64).unwrap();
    // Two workers for three lanes; the first lane runs on the caller.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Preparing again restarts them; dropping stops them.
    rack.prepare(48_000.0, 64).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    drop(rack);
}
