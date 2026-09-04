//! The compressor, `valverig-dsp`'s
//! [`Compressor`](valverig_dsp::compressor::Compressor), as a module: one
//! compressor per channel, one set of settings.

use std::sync::Arc;

use valverig_dsp::compressor::Compressor as Inner;

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// The level above which the signal is reduced.
pub const THRESHOLD: ParamSpec = ParamSpec {
    id: "threshold_db",
    name: "Threshold",
    unit: Unit::Decibels,
    min: -60.0,
    max: 0.0,
    default: -18.0,
};

/// Decibels in per decibel out above the threshold.
pub const RATIO: ParamSpec = ParamSpec {
    id: "ratio",
    name: "Ratio",
    unit: Unit::Ratio,
    min: 1.0,
    max: 20.0,
    default: 4.0,
};

/// How quickly the reduction follows a rising level.
pub const ATTACK: ParamSpec = ParamSpec {
    id: "attack_ms",
    name: "Attack",
    unit: Unit::Milliseconds,
    min: 0.1,
    max: 100.0,
    default: 5.0,
};

/// How quickly the reduction follows a falling level.
pub const RELEASE: ParamSpec = ParamSpec {
    id: "release_ms",
    name: "Release",
    unit: Unit::Milliseconds,
    min: 10.0,
    max: 2000.0,
    default: 100.0,
};

/// The width over which the ratio takes hold around the threshold.
pub const KNEE: ParamSpec = ParamSpec {
    id: "knee_db",
    name: "Knee",
    unit: Unit::Decibels,
    min: 0.0,
    max: 24.0,
    default: 6.0,
};

/// Gain after the reduction.
pub const MAKEUP: ParamSpec = ParamSpec {
    id: "makeup_db",
    name: "Makeup",
    unit: Unit::Decibels,
    min: 0.0,
    max: 24.0,
    default: 0.0,
};

/// A compressor in the rack, between the gate and the capture.
pub type Compressor = Channels<Inner, 6>;

impl PerChannel<6> for Inner {
    const TYPE_ID: &'static str = "std.compressor";
    const PARAMS: [ParamSpec; 6] = [THRESHOLD, RATIO, ATTACK, RELEASE, KNEE, MAKEUP];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 6]) {
        let [threshold, ratio, attack, release, knee, makeup] = params;
        self.set_threshold_db(threshold.get());
        self.set_ratio(ratio.get());
        self.set_attack_ms(attack.get());
        self.set_release_ms(release.get());
        self.set_knee_db(knee.get());
        self.set_makeup_db(makeup.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
