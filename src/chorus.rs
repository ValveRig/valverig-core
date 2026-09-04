//! The chorus, `valverig-dsp`'s [`Chorus`](valverig_dsp::chorus::Chorus), as
//! a module: one per channel, one set of settings, each channel's sweep a
//! quarter of a cycle behind the one before it so a pair sways against
//! itself.

use std::sync::Arc;

use valverig_dsp::chorus::Chorus as Inner;

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// The sweep's rate.
pub const RATE: ParamSpec = ParamSpec {
    id: "rate_hz",
    name: "Rate",
    unit: Unit::Hertz,
    min: 0.1,
    max: 10.0,
    default: 0.8,
};

/// How far the sweep reaches.
pub const DEPTH: ParamSpec = ParamSpec {
    id: "depth",
    name: "Depth",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.5,
};

/// How much of the output is the swept copy: `0` none, `1` all, a vibrato.
pub const MIX: ParamSpec = ParamSpec {
    id: "mix",
    name: "Mix",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.5,
};

/// How far along its cycle each channel after the first starts, in turns.
pub(crate) const CHANNEL_OFFSET: f32 = 0.25;

/// A chorus in the rack. At a mix of zero it is transparent bit for bit.
pub type Chorus = Channels<Inner, 3>;

impl PerChannel<3> for Inner {
    const TYPE_ID: &'static str = "std.chorus";
    const PARAMS: [ParamSpec; 3] = [RATE, DEPTH, MIX];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn prepared(&mut self, channel: usize) {
        self.set_phase_offset(channel as f32 * CHANNEL_OFFSET);
    }

    fn apply(&mut self, params: &[Arc<Param>; 3]) {
        let [rate, depth, mix] = params;
        self.set_rate_hz(rate.get());
        self.set_depth(depth.get());
        self.set_mix(mix.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
