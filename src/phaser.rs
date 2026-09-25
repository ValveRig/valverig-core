//! The phaser, the dsp stage [`Phaser`](crate::dsp::phaser::Phaser), as
//! a module: one per channel, one set of settings, each channel's sweep a
//! quarter of a cycle behind the one before it so a pair sways against
//! itself.

use std::sync::Arc;

use crate::dsp::phaser::{MAX_FEEDBACK, Phaser as Inner};

use crate::channels::{Channels, PerChannel};
use crate::chorus::CHANNEL_OFFSET;
use crate::node::{Param, ParamSpec, Unit};

/// The sweep's rate.
pub const RATE: ParamSpec = ParamSpec {
    id: "rate_hz",
    name: "Rate",
    unit: Unit::Hertz,
    min: 0.05,
    max: 10.0,
    default: 0.5,
};

/// How far the sweep reaches.
pub const DEPTH: ParamSpec = ParamSpec {
    id: "depth",
    name: "Depth",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.7,
};

/// How much of the chain's output goes round again: sharper peaks.
pub const FEEDBACK: ParamSpec = ParamSpec {
    id: "feedback",
    name: "Feedback",
    unit: Unit::Ratio,
    min: 0.0,
    max: MAX_FEEDBACK,
    default: 0.3,
};

/// How much of the output is the shifted copy: `0.5` is the effect.
pub const MIX: ParamSpec = ParamSpec {
    id: "mix",
    name: "Mix",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.5,
};

/// A phaser in the rack. At a mix of zero it is transparent bit for bit.
pub type Phaser = Channels<Inner, 4>;

impl PerChannel<4> for Inner {
    const TYPE_ID: &'static str = "std.phaser";
    const PARAMS: [ParamSpec; 4] = [RATE, DEPTH, FEEDBACK, MIX];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn prepared(&mut self, channel: usize) {
        self.set_phase_offset(channel as f32 * CHANNEL_OFFSET);
    }

    fn apply(&mut self, params: &[Arc<Param>; 4]) {
        let [rate, depth, feedback, mix] = params;
        self.set_rate_hz(rate.get());
        self.set_depth(depth.get());
        self.set_feedback(feedback.get());
        self.set_mix(mix.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
