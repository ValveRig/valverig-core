//! The noise gate, the dsp stage [`Gate`](crate::dsp::gate::Gate), as a
//! module: one gate per channel, one set of settings.

use std::sync::Arc;

use crate::dsp::gate::Gate as Inner;

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// The peak level at which the gate opens.
pub const THRESHOLD: ParamSpec = ParamSpec {
    id: "threshold_db",
    name: "Threshold",
    unit: Unit::Decibels,
    min: -100.0,
    max: 0.0,
    default: -60.0,
};

/// How quickly the gate opens.
pub const ATTACK: ParamSpec = ParamSpec {
    id: "attack_ms",
    name: "Attack",
    unit: Unit::Milliseconds,
    min: 0.01,
    max: 100.0,
    default: 0.5,
};

/// How long the gate stays open after the level falls.
pub const HOLD: ParamSpec = ParamSpec {
    id: "hold_ms",
    name: "Hold",
    unit: Unit::Milliseconds,
    min: 0.0,
    max: 1000.0,
    default: 50.0,
};

/// How quickly the gate closes once the hold has run out.
pub const RELEASE: ParamSpec = ParamSpec {
    id: "release_ms",
    name: "Release",
    unit: Unit::Milliseconds,
    min: 1.0,
    max: 2000.0,
    default: 100.0,
};

/// The attenuation when closed.
pub const RANGE: ParamSpec = ParamSpec {
    id: "range_db",
    name: "Range",
    unit: Unit::Decibels,
    min: -100.0,
    max: 0.0,
    default: -100.0,
};

/// A noise gate in the rack, ahead of the capture.
pub type Gate = Channels<Inner, 5>;

impl PerChannel<5> for Inner {
    const TYPE_ID: &'static str = "std.gate";
    const PARAMS: [ParamSpec; 5] = [THRESHOLD, ATTACK, HOLD, RELEASE, RANGE];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 5]) {
        let [threshold, attack, hold, release, range] = params;
        self.set_threshold_db(threshold.get());
        self.set_attack_ms(attack.get());
        self.set_hold_ms(hold.get());
        self.set_release_ms(release.get());
        self.set_range_db(range.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
