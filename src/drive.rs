//! The drive, `valverig-dsp`'s [`Drive`](valverig_dsp::drive::Drive), as a
//! module: one per channel, one set of settings.

use std::sync::Arc;

use valverig_dsp::drive::{Drive as Inner, MAX_DRIVE_DB, TONE_OPEN_HZ};

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// How hard the shaper is driven: a boost at `0`, a fuzz at the top.
pub const DRIVE: ParamSpec = ParamSpec {
    id: "drive_db",
    name: "Drive",
    unit: Unit::Decibels,
    min: 0.0,
    max: MAX_DRIVE_DB,
    default: 12.0,
};

/// The lowpass after the shaper; at the top it is out of the path.
pub const TONE: ParamSpec = ParamSpec {
    id: "tone_hz",
    name: "Tone",
    unit: Unit::Hertz,
    min: 500.0,
    max: TONE_OPEN_HZ,
    default: 6000.0,
};

/// The output level.
pub const LEVEL: ParamSpec = ParamSpec {
    id: "level_db",
    name: "Level",
    unit: Unit::Decibels,
    min: -24.0,
    max: 24.0,
    default: 0.0,
};

/// A drive in the rack, in front of the amp.
pub type Drive = Channels<Inner, 3>;

impl PerChannel<3> for Inner {
    const TYPE_ID: &'static str = "std.drive";
    const PARAMS: [ParamSpec; 3] = [DRIVE, TONE, LEVEL];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 3]) {
        let [drive, tone, level] = params;
        self.set_drive_db(drive.get());
        self.set_tone_hz(tone.get());
        self.set_level_db(level.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
