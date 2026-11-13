//! Real-time safety: nothing on the audio path allocates, frees, locks or
//! blocks, whatever a host does with the settings in between.
//!
//! A counting global allocator watches this thread over ragged schedules of
//! block sizes with every setting turning underneath: each DSP stage on its
//! own, then whole racks, mono and with lanes, sequential and parallel.
//! `realfft`'s `process_with_scratch` is allocation-free by contract; this is
//! what holds it to that.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

mod common;

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

impl Counting {
    fn note() {
        let _ = WATCHING.try_with(|w| {
            if w.get() {
                let _ = ALLOCS.try_with(|a| a.set(a.get() + 1));
            }
        });
    }
}

// SAFETY: every method forwards its arguments unchanged to the system
// allocator and returns what it returns, so the block a caller gets back
// comes from `System` and is freed by `System` under the same layout.
// Counting is a thread-local integer and allocates nothing itself, so it
// cannot re-enter the allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many times this thread allocated while `f` ran.
fn allocations_during<F: FnOnce()>(f: F) -> usize {
    ALLOCS.with(|a| a.set(0));
    WATCHING.with(|w| w.set(true));
    f();
    WATCHING.with(|w| w.set(false));
    ALLOCS.with(Cell::get)
}

/// Each DSP stage on its own, every setting turning.
mod stages {
    use super::allocations_during;
    use valrig_core::chorus::Mono as Chorus;
    use valrig_core::compressor::Mono as Compressor;
    use valrig_core::delay::Mono as Delay;
    use valrig_core::drive::Mono as Drive;
    use valrig_core::dsp::convolution::Convolver;
    use valrig_core::dsp::resample::Resampler;
    use valrig_core::eq::Mono as ThreeBand;
    use valrig_core::gate::Mono as Gate;
    use valrig_core::phaser::Mono as Phaser;
    use valrig_core::reverb::Fdn as Reverb;

    /// Block lengths that never repeat in step with anything.
    const SCHEDULE: [usize; 6] = [1, 7, 512, 3, 13, 64];

    #[test]
    fn process_does_not_allocate() {
        let ir: Vec<f32> = (0..24_000)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        // The default partition, and the smallest, which crosses a boundary many
        // times per block.
        for mut c in [Convolver::new(&ir), Convolver::with_partition(&ir, 16)] {
            let input = vec![0.1f32; 512];
            let mut output = vec![0.0f32; 512];
            // One untracked pass first: nothing is lazily initialised, but if it
            // were, blaming the first call would hide the steady-state result.
            for &n in &SCHEDULE {
                c.process(&input[..n], &mut output[..n]);
            }
            let allocs = allocations_during(|| {
                for round in 0..600 {
                    let n = SCHEDULE[round % SCHEDULE.len()];
                    c.process(&input[..n], &mut output[..n]);
                }
            });
            assert_eq!(
                allocs,
                0,
                "partition {}: process() allocated {allocs} time(s)",
                c.partition()
            );
            let resets = allocations_during(|| c.reset());
            assert_eq!(resets, 0, "reset() allocated {resets} time(s)");
        }
    }

    #[test]
    fn the_pedals_do_not_allocate_while_their_knobs_turn() {
        let input = vec![0.1f32; 512];
        let mut output = vec![0.0f32; 512];
        let mut drive = Drive::new(48_000.0);
        let mut chorus = Chorus::new(48_000.0);
        let mut delay = Delay::new(48_000.0);
        let mut phaser = Phaser::new(48_000.0);
        let mut up = Resampler::new(44_100.0, 48_000.0, 512);
        let mut down = Resampler::new(48_000.0, 44_100.0, 1024);
        down.prime(512 + 128);
        let mut go = |drive: &mut Drive, chorus: &mut Chorus, delay: &mut Delay, round: usize| {
            let n = SCHEDULE[round % SCHEDULE.len()];
            drive.set_drive_db((round % 40) as f32);
            drive.set_tone_hz(1000.0 + (round % 20) as f32 * 1000.0);
            drive.set_level_db(-((round % 12) as f32));
            chorus.set_rate_hz(0.5 + (round % 5) as f32);
            chorus.set_depth((round % 10) as f32 * 0.1);
            chorus.set_mix((round % 3) as f32 * 0.5);
            delay.set_time_ms(50.0 + (round % 30) as f32 * 60.0);
            delay.set_feedback((round % 9) as f32 * 0.1);
            delay.set_mix((round % 4) as f32 * 0.3);
            delay.set_tone_hz(500.0 + (round % 7) as f32 * 3000.0);
            phaser.set_rate_hz((round % 6) as f32);
            phaser.set_depth((round % 10) as f32 * 0.1);
            phaser.set_feedback((round % 9) as f32 * 0.1);
            phaser.set_mix((round % 3) as f32 * 0.5);
            drive.process(&input[..n], &mut output[..n]);
            chorus.process(&input[..n], &mut output[..n]);
            delay.process(&input[..n], &mut output[..n]);
            phaser.process(&input[..n], &mut output[..n]);
            // The detour a rack at another rate takes: in, through, out.
            up.push(&input[..n]);
            let got = up.pull(&mut output[..512]);
            down.push(&output[..got]);
            down.pull(&mut output[..n]);
        };
        for round in 0..SCHEDULE.len() {
            go(&mut drive, &mut chorus, &mut delay, round);
        }
        let allocs = allocations_during(|| {
            for round in 0..600 {
                go(&mut drive, &mut chorus, &mut delay, round);
            }
        });
        assert_eq!(allocs, 0, "process() allocated {allocs} time(s)");
        let resets = allocations_during(|| {
            drive.reset();
            chorus.reset();
            delay.reset();
            phaser.reset();
            up.reset();
            down.reset();
        });
        assert_eq!(resets, 0, "reset() allocated {resets} time(s)");
    }

    #[test]
    fn every_stage_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + std::fmt::Debug>() {}
        assert_send_sync::<Chorus>();
        assert_send_sync::<Compressor>();
        assert_send_sync::<Delay>();
        assert_send_sync::<Drive>();
        assert_send_sync::<Gate>();
        assert_send_sync::<Phaser>();
        assert_send_sync::<Resampler>();
        assert_send_sync::<Reverb>();
        assert_send_sync::<ThreeBand>();
        // The convolver holds `Arc<dyn ...>` transform plans and writes its
        // `Debug` by hand rather than deriving it, so it has the most to prove.
        assert_send_sync::<Convolver>();
    }

    /// The four stages that carry a signal level rather than a signal: the
    /// gate, the compressor, the tone control and the reverb, with every knob
    /// turning and the reverb driven through both of its entry points.
    #[test]
    fn the_chain_stages_do_not_allocate_while_their_knobs_turn() {
        let input = vec![0.1f32; 512];
        let mut output = vec![0.0f32; 512];
        let mut left = vec![0.0f32; 512];
        let mut right = vec![0.0f32; 512];
        let mut gate = Gate::new(48_000.0);
        let mut compressor = Compressor::new(48_000.0);
        let mut eq = ThreeBand::new(48_000.0);
        let mut reverb = Reverb::new(48_000.0);
        let mut go = |round: usize, output: &mut [f32], left: &mut [f32], right: &mut [f32]| {
            let n = SCHEDULE[round % SCHEDULE.len()];
            gate.set_threshold_db(-80.0 + (round % 60) as f32);
            gate.set_attack_ms(0.1 + (round % 9) as f32);
            gate.set_hold_ms((round % 100) as f32);
            gate.set_release_ms(10.0 + (round % 30) as f32 * 10.0);
            gate.set_range_db(-100.0 + (round % 5) as f32 * 20.0);
            compressor.set_threshold_db(-40.0 + (round % 35) as f32);
            compressor.set_ratio(1.0 + (round % 19) as f32);
            compressor.set_attack_ms(0.1 + (round % 20) as f32);
            compressor.set_release_ms(10.0 + (round % 40) as f32 * 10.0);
            compressor.set_knee_db((round % 24) as f32);
            compressor.set_makeup_db((round % 12) as f32);
            eq.set_bass_db(-12.0 + (round % 25) as f32);
            eq.set_mid_db(12.0 - (round % 25) as f32);
            eq.set_treble_db(((round % 9) as f32 - 4.0) * 3.0);
            reverb.set_mix((round % 11) as f32 * 0.1);
            reverb.set_decay_s(0.1 + (round % 20) as f32 * 0.5);
            reverb.set_damping_hz(500.0 + (round % 20) as f32 * 1000.0);
            reverb.set_predelay_ms((round % 100) as f32);
            gate.process(&input[..n], &mut output[..n]);
            compressor.process(&input[..n], &mut output[..n]);
            eq.process(&input[..n], &mut output[..n]);
            reverb.process(&input[..n], &mut output[..n]);
            reverb.process_pair(&input[..n], &input[..n], &mut left[..n], &mut right[..n]);
        };
        for round in 0..SCHEDULE.len() {
            go(round, &mut output, &mut left, &mut right);
        }
        let allocs = allocations_during(|| {
            for round in 0..600 {
                go(round, &mut output, &mut left, &mut right);
            }
        });
        assert_eq!(allocs, 0, "process() allocated {allocs} time(s)");
        let resets = allocations_during(|| {
            gate.reset();
            compressor.reset();
            eq.reset();
            reverb.reset();
        });
        assert_eq!(resets, 0, "reset() allocated {resets} time(s)");
    }
}

/// `Rack::process` and `Rack::reset`, in a mono rack and in one with lanes.
mod racks {
    use super::allocations_during;
    use super::common::{chain, lane_chain, model};
    use valrig_core::bus::{BusReturn, BusSend};
    use valrig_core::cabinet::Cabinet;
    use valrig_core::chorus::Chorus;
    use valrig_core::delay::Delay;
    use valrig_core::drive::Drive;
    use valrig_core::gain::Gain;
    use valrig_core::phaser::Phaser;
    use valrig_core::rack::{self, Lane, Rack};
    use valrig_core::tap::Tap;

    #[test]
    fn process_and_reset_do_not_allocate() {
        let ir: Vec<f32> = (0..2048)
            .map(|i| (-(i as f32) / 400.0).exp() * 0.2)
            .collect();
        for (wide, parallel) in [(false, false), (true, false), (true, true)] {
            let (mut rack, controls) = if wide {
                lane_chain(vec![
                    ("A", model("wavenet.nam"), Some(Cabinet::new(&ir, None))),
                    ("B", model("lstm.nam"), Some(Cabinet::new(&ir, None))),
                ])
            } else {
                chain(model("wavenet.nam"), Some(Cabinet::new(&ir, None)))
            };
            rack.set_parallel_lanes(parallel, None);
            rack.prepare(48_000.0, 128).unwrap();
            let channels = rack.channels_out();
            let input = vec![0.1f32; 128];
            let (mut left, mut right) = (vec![0.0f32; 128], vec![0.0f32; 128]);
            let schedule = [1usize, 7, 128, 3, 64, 17];
            let mut go = |rack: &mut rack::Rack, n: usize| {
                let mut outs = [&mut left[..n], &mut right[..n]];
                rack.process(&input[..n], &mut outs[..channels]);
            };
            for &n in &schedule {
                go(&mut rack, n);
            }
            let allocs = allocations_during(|| {
                for round in 0..300 {
                    let n = schedule[round % schedule.len()];
                    // A host turning knobs and switches while the audio runs.
                    controls
                        .set("output/level_db", -((round % 30) as f32))
                        .unwrap();
                    controls
                        .set("gate/threshold_db", -80.0 + (round % 7) as f32)
                        .unwrap();
                    controls
                        .set("compressor/threshold_db", -30.0 + (round % 5) as f32)
                        .unwrap();
                    controls
                        .set("reverb/mix", (round % 5) as f32 * 0.1)
                        .unwrap();
                    controls
                        .set("reverb/decay_s", 1.0 + (round % 4) as f32)
                        .unwrap();
                    controls.set_bypass("gate", round % 11 == 0).unwrap();
                    if wide {
                        controls.set("A/pan", -1.0 + (round % 3) as f32).unwrap();
                        controls.set("B/level_db", -((round % 12) as f32)).unwrap();
                        controls
                            .set("A.eq/bass_db", (round % 9) as f32 - 4.0)
                            .unwrap();
                        controls.set_bypass("B", round % 13 == 0).unwrap();
                        controls.set_bypass("A.cabinet", round % 5 == 0).unwrap();
                    } else {
                        controls
                            .set("eq/bass_db", (round % 9) as f32 - 4.0)
                            .unwrap();
                        controls.set_bypass("cabinet", round % 5 == 0).unwrap();
                    }
                    go(&mut rack, n);
                }
            });
            assert_eq!(
                allocs, 0,
                "wide {wide}, parallel {parallel}: process() allocated {allocs} time(s)"
            );
            let resets = allocations_during(|| rack.reset());
            assert_eq!(
                resets, 0,
                "wide {wide}, parallel {parallel}: reset() allocated {resets} time(s)"
            );
        }
    }

    #[test]
    fn the_pedals_and_the_taps_do_not_allocate_either() {
        for parallel in [false, true] {
            let (mut rack, controls) = Rack::builder()
                .push("input", Gain::new())
                .unwrap()
                .push("drive", Drive::new())
                .unwrap()
                .push("send", BusSend::new())
                .unwrap()
                .push("tap", Tap::new())
                .unwrap()
                .split(vec![
                    Lane::new("A")
                        .pan(-1.0)
                        .push("A.chorus", Chorus::new())
                        .unwrap()
                        .push("A.tap", Tap::new())
                        .unwrap()
                        .push("A.cab", Cabinet::new(&[1.0, 0.5, 0.25], None))
                        .unwrap(),
                    Lane::new("B")
                        .pan(1.0)
                        .push("B.delay", Delay::new())
                        .unwrap()
                        .push("B.phaser", Phaser::new())
                        .unwrap(),
                ])
                .unwrap()
                .push("return", BusReturn::new())
                .unwrap()
                .push("chorus", Chorus::new())
                .unwrap()
                .push("delay", Delay::new())
                .unwrap()
                .push("output", Gain::new())
                .unwrap()
                .build();
            rack.set_parallel_lanes(parallel, None);
            rack.prepare(48_000.0, 128).unwrap();
            let input = vec![0.1f32; 128];
            let (mut left, mut right) = (vec![0.0f32; 128], vec![0.0f32; 128]);
            let schedule = [1usize, 7, 128, 3, 64, 17];
            let mut go = |rack: &mut Rack, n: usize| {
                let mut outs = [&mut left[..n], &mut right[..n]];
                rack.process(&input[..n], &mut outs);
            };
            for &n in &schedule {
                go(&mut rack, n);
            }
            let allocs = allocations_during(|| {
                for round in 0..300 {
                    let n = schedule[round % schedule.len()];
                    controls.set("drive/drive_db", (round % 40) as f32).unwrap();
                    controls
                        .set("drive/tone_hz", 1000.0 + (round % 19) as f32 * 1000.0)
                        .unwrap();
                    controls
                        .set("A.chorus/depth", (round % 10) as f32 * 0.1)
                        .unwrap();
                    controls
                        .set("chorus/mix", (round % 3) as f32 * 0.5)
                        .unwrap();
                    controls
                        .set("B.delay/time_ms", 50.0 + (round % 30) as f32 * 60.0)
                        .unwrap();
                    controls
                        .set("delay/feedback", (round % 9) as f32 * 0.1)
                        .unwrap();
                    controls
                        .set("tap/pan", -1.0 + (round % 5) as f32 * 0.5)
                        .unwrap();
                    controls
                        .set("A.tap/level_db", -((round % 12) as f32))
                        .unwrap();
                    controls
                        .set("A.cab/low_cut_hz", 20.0 + (round % 8) as f32 * 40.0)
                        .unwrap();
                    controls
                        .set("A.cab/high_cut_hz", 20_000.0 - (round % 6) as f32 * 2000.0)
                        .unwrap();
                    controls
                        .set("B.phaser/rate_hz", 0.2 + (round % 4) as f32)
                        .unwrap();
                    controls
                        .set("send/level_db", -((round % 9) as f32))
                        .unwrap();
                    controls.set("send/bus", 1.0 + (round % 4) as f32).unwrap();
                    controls
                        .set("return/bus", 1.0 + (round % 3) as f32)
                        .unwrap();
                    controls.set_bypass("A.tap", round % 7 == 0).unwrap();
                    controls.set_bypass("B", round % 13 == 0).unwrap();
                    controls.set_bypass("delay", round % 5 == 0).unwrap();
                    go(&mut rack, n);
                }
            });
            assert_eq!(
                allocs, 0,
                "parallel {parallel}: process() allocated {allocs} time(s)"
            );
            let resets = allocations_during(|| rack.reset());
            assert_eq!(
                resets, 0,
                "parallel {parallel}: reset() allocated {resets} time(s)"
            );
        }
    }
}
