//! `Rack::process` and `Rack::reset` must not allocate, whatever the host
//! does with the controls in between, in a mono rack and in one with lanes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
mod common;

use common::{chain, lane_chain, model};
use valverig_core::bus::{BusReturn, BusSend};
use valverig_core::cabinet::Cabinet;
use valverig_core::chorus::Chorus;
use valverig_core::delay::Delay;
use valverig_core::drive::Drive;
use valverig_core::gain::Gain;
use valverig_core::phaser::Phaser;
use valverig_core::rack::{self, Lane, Rack};
use valverig_core::tap::Tap;

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

fn allocations_during<F: FnOnce()>(f: F) -> usize {
    ALLOCS.with(|a| a.set(0));
    WATCHING.with(|w| w.set(true));
    f();
    WATCHING.with(|w| w.set(false));
    ALLOCS.with(Cell::get)
}

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
