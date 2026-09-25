//! Lanes of unequal latency are lined up: at `prepare` the shorter lanes of
//! a stage are padded with a delay up to the longest, so the mix adds what
//! left the split together, on the caller's thread and on workers alike.

use valverig_core::Result;
mod common;

use common::noise;
use valverig_core::gain::Gain;
use valverig_core::node::Node;
use valverig_core::rack::{Lane, Rack};

/// A wire that is `k` frames late, and says so.
#[derive(Debug)]
struct Late {
    k: usize,
    lines: Vec<Vec<f32>>,
    pos: usize,
}

impl Late {
    fn new(k: usize) -> Self {
        Self {
            k,
            lines: Vec::new(),
            pos: 0,
        }
    }
}

impl Node for Late {
    fn type_id(&self) -> &'static str {
        "test.late"
    }
    fn prepare(&mut self, _sample_rate: f64, _max_block: usize, channels: usize) -> Result<()> {
        self.lines = vec![vec![0.0; self.k]; channels];
        self.pos = 0;
        Ok(())
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
        self.pos = 0;
    }
    fn latency(&self) -> usize {
        self.k
    }
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        if self.k == 0 {
            for (x, y) in input.iter().zip(output.iter_mut()) {
                y.copy_from_slice(x);
            }
            return;
        }
        let n = input[0].len();
        for ((x, y), line) in input.iter().zip(output.iter_mut()).zip(&mut self.lines) {
            let mut pos = self.pos;
            for i in 0..n {
                y[i] = line[pos];
                line[pos] = x[i];
                pos += 1;
                if pos == self.k {
                    pos = 0;
                }
            }
        }
        self.pos = (self.pos + n) % self.k;
    }
}

#[test]
fn the_shorter_lane_waits_for_the_longer() {
    let x = noise(1, 6000, 0.5);
    let schedule = [128usize, 1, 7, 64, 3, 128, 33];
    for late_in_a in [true, false] {
        for parallel in [false, true] {
            let (a, b) = if late_in_a { (100, 0) } else { (0, 100) };
            let (mut rack, _) = Rack::builder()
                .push("input", Gain::new())
                .unwrap()
                .split(vec![
                    Lane::new("A")
                        .pan(-1.0)
                        .push("A.wire", Late::new(a))
                        .unwrap(),
                    Lane::new("B")
                        .pan(1.0)
                        .push("B.wire", Late::new(b))
                        .unwrap(),
                ])
                .unwrap()
                .push("output", Gain::new())
                .unwrap()
                .build();
            rack.set_parallel_lanes(parallel, None);
            rack.prepare(48_000.0, 128).unwrap();
            assert_eq!(
                rack.latency(),
                100,
                "the stage is as late as its latest lane"
            );
            let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
            let (mut pos, mut i) = (0, 0);
            while pos < x.len() {
                let n = schedule[i % schedule.len()].min(x.len() - pos);
                let mut o = [&mut l[pos..pos + n], &mut r[pos..pos + n]];
                rack.process(&x[pos..pos + n], &mut o);
                pos += n;
                i += 1;
            }
            for n in 0..x.len() {
                assert_eq!(
                    l[n].to_bits(),
                    r[n].to_bits(),
                    "late in A {late_in_a}, parallel {parallel}: the sides differ at frame {n}"
                );
                let want = if n < 100 { 0.0 } else { x[n - 100] };
                assert_eq!(
                    l[n].to_bits(),
                    want.to_bits(),
                    "late in A {late_in_a}, parallel {parallel}: frame {n}"
                );
            }
        }
    }
}

#[test]
fn lanes_of_equal_latency_are_not_touched() {
    // No padding at all: the mix of two wires is the input on both sides,
    // bit for bit, which the rack tests already hold and this keeps.
    let (mut rack, _) = Rack::builder()
        .split(vec![
            Lane::new("A")
                .pan(-1.0)
                .push("A.wire", Late::new(0))
                .unwrap(),
            Lane::new("B")
                .pan(1.0)
                .push("B.wire", Late::new(0))
                .unwrap(),
        ])
        .unwrap()
        .build();
    rack.prepare(48_000.0, 64).unwrap();
    assert_eq!(rack.latency(), 0);
    let x = noise(2, 256, 0.5);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for c in 0..4 {
        let mut o = [&mut l[c * 64..(c + 1) * 64], &mut r[c * 64..(c + 1) * 64]];
        rack.process(&x[c * 64..(c + 1) * 64], &mut o);
    }
    for n in 0..256 {
        assert_eq!(l[n].to_bits(), x[n].to_bits());
        assert_eq!(r[n].to_bits(), x[n].to_bits());
    }
}
