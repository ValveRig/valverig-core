//! A send and a return: the signal at one point in the chain put on a bus,
//! and added back in at another, so that a stretch of the chain runs in
//! parallel with a dry copy of itself - an effects loop, on a mixing desk.
//!
//! The rack keeps [`crate::rack::BUSES`] buses. A [`BusSend`] is a
//! wire in the path that keeps a levelled copy of the block and names the
//! bus it goes on; the rack takes the copy right after the send ran, or
//! after the lanes stage it sits in has been joined. A [`BusReturn`] names
//! the bus it reads; the rack hands it the bus's block before it runs, and
//! it adds that block to its input at its own level. Buses are emptied at
//! the start of every block, so a return hears the sends that ran before it
//! in rack order - a send later in the chain, or in the same lanes stage,
//! is silent to it - and there is no feedback and no latency. A mono bus
//! into a stereo return goes to both sides; a stereo bus into a mono return
//! is the mean of the two.

use crate::Result;
use crate::gain::db_to_linear;
use crate::node::{Node, Param, ParamSpec, Unit};
use crate::rack::{BUSES, LANE_LEVEL};
use std::sync::Arc;

/// The level onto, or back from, the bus.
pub const LEVEL: ParamSpec = LANE_LEVEL;
/// Which bus, counted from one.
pub const BUS: ParamSpec = ParamSpec {
    id: "bus",
    name: "Bus",
    unit: Unit::Index,
    min: 1.0,
    max: BUSES as f32,
    default: 1.0,
};

/// The gains follow a level change with this one-pole time.
const SMOOTH_MS: f32 = 5.0;

fn bus_index(p: &Param) -> usize {
    (p.get().round() as usize).clamp(1, BUSES) - 1
}

#[inline]
fn coeff(sample_rate: f64) -> f32 {
    1.0 - (-1.0 / (SMOOTH_MS * 1e-3 * sample_rate as f32)).exp()
}

/// A send onto a bus.
#[derive(Debug)]
pub struct BusSend {
    params: [Arc<Param>; 2],
    buf: [Vec<f32>; 2],
    width: usize,
    held: usize,
    gain: f32,
    coeff: f32,
}

impl BusSend {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.send";

    /// A send at 0 dB onto bus 1.
    pub fn new() -> Self {
        Self {
            params: [Param::new(LEVEL), Param::new(BUS)],
            buf: [Vec::new(), Vec::new()],
            width: 1,
            held: 0,
            gain: 1.0,
            coeff: 1.0,
        }
    }
}

impl Default for BusSend {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for BusSend {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, max_block: usize, channels: usize) -> Result<()> {
        self.width = channels.min(2);
        self.buf = [vec![0.0; max_block], vec![0.0; max_block]];
        self.held = 0;
        self.coeff = coeff(sample_rate);
        self.gain = db_to_linear(self.params[0].get());
        Ok(())
    }

    fn reset(&mut self) {
        self.held = 0;
        self.gain = db_to_linear(self.params[0].get());
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let n = input[0].len();
        let target = db_to_linear(self.params[0].get());
        let (coeff, mut g) = (self.coeff, self.gain);
        for i in 0..n {
            g += coeff * (target - g);
            for (c, x) in input.iter().enumerate().take(self.width) {
                self.buf[c][i] = x[i] * g;
            }
        }
        self.gain = g;
        for (x, y) in input.iter().zip(output.iter_mut()) {
            y.copy_from_slice(x);
        }
        self.held = n;
    }

    fn sends(&self) -> Option<(usize, [&[f32]; 2])> {
        let n = self.held;
        Some((
            bus_index(&self.params[1]),
            [
                &self.buf[0][..n],
                &self.buf[1][..if self.width == 2 { n } else { 0 }],
            ],
        ))
    }
}

/// A return from a bus.
#[derive(Debug)]
pub struct BusReturn {
    params: [Arc<Param>; 2],
    buf: [Vec<f32>; 2],
    width: usize,
    /// Frames received this block, and how wide the bus was.
    got: usize,
    bus_width: usize,
    gain: f32,
    coeff: f32,
}

impl BusReturn {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.return";

    /// A return at 0 dB from bus 1.
    pub fn new() -> Self {
        Self {
            params: [Param::new(LEVEL), Param::new(BUS)],
            buf: [Vec::new(), Vec::new()],
            width: 1,
            got: 0,
            bus_width: 1,
            gain: 1.0,
            coeff: 1.0,
        }
    }
}

impl Default for BusReturn {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for BusReturn {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, max_block: usize, channels: usize) -> Result<()> {
        self.width = channels.min(2);
        self.buf = [vec![0.0; max_block], vec![0.0; max_block]];
        self.got = 0;
        self.coeff = coeff(sample_rate);
        self.gain = db_to_linear(self.params[0].get());
        Ok(())
    }

    fn reset(&mut self) {
        self.got = 0;
        self.gain = db_to_linear(self.params[0].get());
    }

    fn receives(&self) -> Option<usize> {
        Some(bus_index(&self.params[1]))
    }

    fn receive(&mut self, bus: [&[f32]; 2]) {
        let n = bus[0].len().min(self.buf[0].len());
        self.buf[0][..n].copy_from_slice(&bus[0][..n]);
        if bus[1].len() >= n && n > 0 && !bus[1].is_empty() {
            self.buf[1][..n].copy_from_slice(&bus[1][..n]);
            self.bus_width = 2;
        } else {
            self.bus_width = 1;
        }
        self.got = n;
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let n = input[0].len();
        let target = db_to_linear(self.params[0].get());
        let got = self.got.min(n);
        if got == 0 {
            // Nothing on the bus this block: a wire.
            for (x, y) in input.iter().zip(output.iter_mut()) {
                y.copy_from_slice(x);
            }
            self.gain = target;
            self.got = 0;
            return;
        }
        let (coeff, mut g) = (self.coeff, self.gain);
        for c in 0..self.width.min(input.len()) {
            g = self.gain;
            let x = input[c];
            let y = &mut output[c];
            for i in 0..n {
                g += coeff * (target - g);
                let b = if i >= got {
                    0.0
                } else if self.bus_width == 2 {
                    if self.width == 2 {
                        self.buf[c.min(1)][i]
                    } else {
                        0.5 * (self.buf[0][i] + self.buf[1][i])
                    }
                } else {
                    self.buf[0][i]
                };
                y[i] = x[i] + b * g;
            }
        }
        self.gain = g;
        self.got = 0;
    }
}
