//! The send and the return: what a send puts on a bus, a return later in
//! rack order adds to its input; a return before it, or a bypassed send,
//! hears nothing; a send in a lane reaches a return after the mix, on
//! workers too, bit for bit.

mod common;

use common::{assert_bits, run, signal};
use valverig_core::bus::{BusReturn, BusSend};
use valverig_core::gain::{Gain, db_to_linear};
use valverig_core::rack::{Lane, Rack};

#[test]
fn a_return_adds_what_a_send_before_it_put_on_the_bus() {
    let (mut rack, controls) = Rack::builder()
        .push("input", Gain::new())
        .unwrap()
        .push("send", BusSend::new())
        .unwrap()
        .push("loop", Gain::new())
        .unwrap()
        .push("return", BusReturn::new())
        .unwrap()
        .push("output", Gain::new())
        .unwrap()
        .build();
    controls.set("loop/level_db", -6.0).unwrap();
    rack.prepare(48_000.0, 128).unwrap();
    let x = signal(1, 4096, 0.5);
    let g = db_to_linear(-6.0);
    let y = run(&mut rack, &x, &[128, 1, 7, 64]);
    let want: Vec<f32> = x.iter().map(|v| v * g + v).collect();
    assert_bits(&y[0], &want, "the loop at -6 dB plus the dry bus");

    // A bypassed send puts nothing on the bus; the return is a wire.
    controls.set_bypass("send", true).unwrap();
    let y = run(&mut rack, &x, &[128]);
    let want: Vec<f32> = x.iter().map(|v| v * g).collect();
    assert_bits(&y[0], &want, "bypassed send");

    // The send at its own level, the return at its own.
    controls.set_bypass("send", false).unwrap();
    controls.set("send/level_db", -12.0).unwrap();
    controls.set("return/level_db", -6.0).unwrap();
    rack.reset();
    let y = run(&mut rack, &x, &[128]);
    let (s, r) = (db_to_linear(-12.0), db_to_linear(-6.0));
    let want: Vec<f32> = x.iter().map(|v| v * g + (v * s) * r).collect();
    assert_bits(&y[0], &want, "send -12, return -6");

    // Another bus is another bus.
    controls.set("send/bus", 2.0).unwrap();
    rack.reset();
    let y = run(&mut rack, &x, &[128]);
    let want: Vec<f32> = x.iter().map(|v| v * g).collect();
    assert_bits(&y[0], &want, "send on bus 2, return on bus 1");
}

#[test]
fn a_return_before_its_send_hears_nothing() {
    let (mut rack, controls) = Rack::builder()
        .push("return", BusReturn::new())
        .unwrap()
        .push("loop", Gain::new())
        .unwrap()
        .push("send", BusSend::new())
        .unwrap()
        .build();
    controls.set("loop/level_db", -6.0).unwrap();
    rack.prepare(48_000.0, 128).unwrap();
    let x = signal(2, 2048, 0.5);
    let y = run(&mut rack, &x, &[128]);
    let g = db_to_linear(-6.0);
    let want: Vec<f32> = x.iter().map(|v| v * g).collect();
    assert_bits(&y[0], &want, "the bus is emptied every block");
}

#[test]
fn a_send_in_a_lane_reaches_a_return_after_the_mix_and_the_workers_agree() {
    let build = || {
        Rack::builder()
            .push("input", Gain::new())
            .unwrap()
            .split(vec![
                Lane::new("A")
                    .pan(-1.0)
                    .push("A.send", BusSend::new())
                    .unwrap()
                    .push("A.gain", Gain::new())
                    .unwrap(),
                Lane::new("B")
                    .pan(1.0)
                    .push("B.gain", Gain::new())
                    .unwrap()
                    .push("B.return", BusReturn::new())
                    .unwrap(),
            ])
            .unwrap()
            .push("return", BusReturn::new())
            .unwrap()
            .push("output", Gain::new())
            .unwrap()
            .build()
    };
    let x = signal(3, 8192, 0.5);
    let (ga, gb) = (db_to_linear(-6.0), db_to_linear(3.0));
    let mut outs = Vec::new();
    for parallel in [false, true] {
        let (mut rack, controls) = build();
        controls.set("A.gain/level_db", -6.0).unwrap();
        controls.set("B.gain/level_db", 3.0).unwrap();
        rack.set_parallel_lanes(parallel, None);
        rack.prepare(48_000.0, 128).unwrap();
        let y = run(&mut rack, &x, &[128, 1, 7, 64, 3]);
        // The mono bus goes to both sides of the stereo return; the return
        // inside lane B, in the same stage as the send, hears nothing.
        let left: Vec<f32> = x.iter().map(|v| v * ga + v).collect();
        let right: Vec<f32> = x.iter().map(|v| v * gb + v).collect();
        assert_bits(&y[0], &left, &format!("parallel {parallel}: left"));
        assert_bits(&y[1], &right, &format!("parallel {parallel}: right"));
        outs.push(y);
    }
    assert_bits(
        &outs[0][0],
        &outs[1][0],
        "left, sequential against parallel",
    );
    assert_bits(
        &outs[0][1],
        &outs[1][1],
        "right, sequential against parallel",
    );
}
