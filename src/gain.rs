//! A level control, smoothed.

use crate::Result;
use crate::dsp::decibels;
use crate::node::{Node, Param, ParamSpec, Unit};
use std::sync::Arc;

/// The one parameter: level in decibels.
pub const LEVEL: ParamSpec = ParamSpec {
    id: "level_db",
    name: "Level",
    unit: Unit::Decibels,
    min: -60.0,
    max: 24.0,
    default: 0.0,
};

/// Multiply by a level, following changes with a 5 ms one-pole so a knob
/// never clicks. One smoother per channel, one level for all of them.
///
/// At 0 dB it is exact: the factor is `1.0` and the signal passes bit for
/// bit, which is what lets a rack of gain, capture and cabinet be tested
/// against the capture and cabinet run by hand.
#[derive(Debug)]
pub struct Gain {
    params: [Arc<Param>; 1],
    /// The linear factor each channel's last sample was multiplied by.
    current: Vec<f32>,
    coeff: f32,
}

impl Gain {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.gain";

    /// A gain at 0 dB.
    pub fn new() -> Self {
        Self {
            params: [Param::new(LEVEL)],
            current: vec![1.0],
            coeff: 1.0,
        }
    }

    fn target(&self) -> f32 {
        decibels::to_linear(self.params[0].get())
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for Gain {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, _max_block: usize, channels: usize) -> Result<()> {
        // 5 ms: fast enough to follow a knob, slow enough not to click.
        self.coeff = 1.0 - (-1.0 / (0.005 * sample_rate as f32)).exp();
        self.current = vec![self.target(); channels];
        Ok(())
    }

    fn reset(&mut self) {
        let target = self.target();
        self.current.fill(target);
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let target = self.target();
        let coeff = self.coeff;
        for ((x, y), g) in input.iter().zip(output.iter_mut()).zip(&mut self.current) {
            let mut gain = *g;
            for (o, &i) in y.iter_mut().zip(x.iter()) {
                gain += coeff * (target - gain);
                *o = i * gain;
            }
            *g = gain;
        }
    }
}
