//! A tap to the output: the signal at this point in the chain, at its own
//! level and pan, summed into the rack's output alongside whatever the rest
//! of the chain makes of it. A send, on a mixing desk.
//!
//! In the path the tap is a wire: its output is its input, bit for bit.
//! What it keeps is a copy of the block, and after every stage has run the
//! rack calls [`Node::tap`] on it, which adds that copy - levelled, and
//! panned into a pair the way a lane is mixed - to the output buffers. A
//! tap in a lane runs on the lane's thread and is tapped on the rack's,
//! after the lane has been joined, so nothing is shared while it runs.

use crate::Result;
use crate::gain::db_to_linear;
use crate::node::{Node, Param, ParamSpec};
use crate::rack::{LANE_LEVEL, LANE_PAN, pan_gains};
use std::sync::Arc;

/// The level into the output.
pub const LEVEL: ParamSpec = LANE_LEVEL;
/// The place in the pair, when the output is one.
pub const PAN: ParamSpec = LANE_PAN;

/// The gains follow a level or pan change with this one-pole time, as the
/// mix does.
const SMOOTH_MS: f32 = 5.0;

/// A tap to the output.
#[derive(Debug)]
pub struct Tap {
    params: [Arc<Param>; 2],
    buf: [Vec<f32>; 2],
    width: usize,
    held: usize,
    /// The smoothed gains into the left and right of a stereo output.
    gain: [f32; 2],
    /// The smoothed gain into a mono output, where there is no pair to pan
    /// across and the level is the whole of it.
    mono_gain: f32,
    coeff: f32,
}

impl Tap {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.tap";

    /// A tap at 0 dB, centred.
    pub fn new() -> Self {
        Self {
            params: [Param::new(LEVEL), Param::new(PAN)],
            buf: [Vec::new(), Vec::new()],
            width: 1,
            held: 0,
            gain: [0.0; 2],
            mono_gain: 0.0,
            coeff: 1.0,
        }
    }

    /// The gains the tap is heading for into a stereo output: a
    /// constant-power pan of a mono tap, a balance of a stereo one, times
    /// the level.
    fn targets(&self) -> [f32; 2] {
        pan_gains(
            self.width,
            self.params[1].get(),
            db_to_linear(self.params[0].get()),
        )
    }

    /// The gain the tap is heading for into a mono output: the level, with
    /// no pan law, so a centred tap is at unity rather than 3 dB down.
    fn mono_target(&self) -> f32 {
        db_to_linear(self.params[0].get())
    }
}

impl Default for Tap {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for Tap {
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
        self.coeff = 1.0 - (-1.0 / (SMOOTH_MS * 1e-3 * sample_rate as f32)).exp();
        self.gain = self.targets();
        self.mono_gain = self.mono_target();
        Ok(())
    }

    fn reset(&mut self) {
        self.held = 0;
        self.gain = self.targets();
        self.mono_gain = self.mono_target();
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let n = input[0].len();
        for ((x, y), b) in input.iter().zip(output.iter_mut()).zip(&mut self.buf) {
            y.copy_from_slice(x);
            b[..n].copy_from_slice(x);
        }
        self.held = n;
    }

    fn tap(&mut self, output: &mut [&mut [f32]]) {
        let n = self.held.min(output[0].len());
        let coeff = self.coeff;
        match output {
            // One channel out, which the rack only has when it has no lanes
            // stage, so the tap is mono here too: the level and no pan law,
            // followed as smoothly as the pair is.
            [mono] => {
                let target = self.mono_target();
                let mut gain = self.mono_gain;
                for (out, &held) in mono[..n].iter_mut().zip(&self.buf[0][..n]) {
                    gain += coeff * (target - gain);
                    *out += held * gain;
                }
                self.mono_gain = gain;
            }
            [left, right] => {
                let target = self.targets();
                let [mut gl, mut gr] = self.gain;
                let right_source = if self.width == 1 { 0 } else { 1 };
                for i in 0..n {
                    gl += coeff * (target[0] - gl);
                    gr += coeff * (target[1] - gr);
                    left[i] += self.buf[0][i] * gl;
                    right[i] += self.buf[right_source][i] * gr;
                }
                self.gain = [gl, gr];
            }
            _ => {}
        }
    }
}
