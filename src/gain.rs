//! A level control, smoothed.

use crate::Result;
use crate::dsp::{decibels, one_pole};
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

/// The one-pole time a level change takes to cover 63% of the way: fast
/// enough to follow a knob, slow enough not to click.
const SMOOTH_MS: f32 = 5.0;

/// Multiply by a level, following changes with a 5 ms one-pole so a knob
/// never clicks. One smoother per channel, one level for all of them.
///
/// At 0 dB it is exact: the factor is `1.0` and the signal passes bit for
/// bit, which is what lets a rack of gain, capture and cabinet be tested
/// against the capture and cabinet run by hand. A glide lands on its target
/// rather than stalling short of it, so that holds again once a move back
/// to 0 dB has settled.
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
        self.coeff = one_pole::from_time(SMOOTH_MS, sample_rate as f32);
        self.current = vec![self.target(); channels];
        Ok(())
    }

    fn reset(&mut self) {
        let target = self.target();
        self.current.fill(target);
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let target = self.target();
        let (coeff, snap) = (self.coeff, one_pole::gain_snap(target));
        for ((x, y), g) in input.iter().zip(output.iter_mut()).zip(&mut self.current) {
            let mut gain = *g;
            for (o, &i) in y.iter_mut().zip(x.iter()) {
                gain = one_pole::step(gain, target, coeff, snap);
                *o = i * gain;
            }
            *g = gain;
        }
    }
}
