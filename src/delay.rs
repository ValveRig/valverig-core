//! The delay, the dsp stage [`Delay`](crate::dsp::delay::Delay), as a
//! module: one per channel, one set of settings.

use std::sync::Arc;

use crate::dsp::delay::{Delay as Inner, MAX_FEEDBACK, MAX_TIME_MS, TONE_OPEN_HZ};

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// The time between repeats. A change glides, as a tape echo's does.
pub const TIME: ParamSpec = ParamSpec {
    id: "time_ms",
    name: "Time",
    unit: Unit::Milliseconds,
    min: 10.0,
    max: MAX_TIME_MS,
    default: 380.0,
};

/// How much of each repeat goes round again.
pub const FEEDBACK: ParamSpec = ParamSpec {
    id: "feedback",
    name: "Feedback",
    unit: Unit::Ratio,
    min: 0.0,
    max: MAX_FEEDBACK,
    default: 0.35,
};

/// How loud the repeats are against the dry signal.
pub const MIX: ParamSpec = ParamSpec {
    id: "mix",
    name: "Mix",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.3,
};

/// The lowpass every repeat goes through; at the top it is out of the path.
pub const TONE: ParamSpec = ParamSpec {
    id: "tone_hz",
    name: "Tone",
    unit: Unit::Hertz,
    min: 500.0,
    max: TONE_OPEN_HZ,
    default: 4000.0,
};

/// A delay in the rack, after the cabinet or the mix. At a mix of zero it is
/// transparent bit for bit.
pub type Delay = Channels<Inner, 4>;

impl PerChannel<4> for Inner {
    const TYPE_ID: &'static str = "std.delay";
    const PARAMS: [ParamSpec; 4] = [TIME, FEEDBACK, MIX, TONE];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 4]) {
        let [time, feedback, mix, tone] = params;
        self.set_time_ms(time.get());
        self.set_feedback(feedback.get());
        self.set_mix(mix.get());
        self.set_tone_hz(tone.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
