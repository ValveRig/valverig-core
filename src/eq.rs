//! The tone control, `valverig-dsp`'s [`ThreeBand`], as a module: one per
//! channel, one set of settings.

use std::sync::Arc;

use valverig_dsp::eq::ThreeBand;

use crate::channels::{Channels, PerChannel};
use crate::node::{Param, ParamSpec, Unit};

/// The low shelf.
pub const BASS: ParamSpec = ParamSpec {
    id: "bass_db",
    name: "Bass",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// The mid peak.
pub const MID: ParamSpec = ParamSpec {
    id: "mid_db",
    name: "Mid",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// The high shelf.
pub const TREBLE: ParamSpec = ParamSpec {
    id: "treble_db",
    name: "Treble",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// Bass, mid and treble in the rack, after the capture. Flat, it is
/// transparent bit for bit.
pub type Tone = Channels<ThreeBand, 3>;

impl PerChannel<3> for ThreeBand {
    const TYPE_ID: &'static str = "std.eq";
    const PARAMS: [ParamSpec; 3] = [BASS, MID, TREBLE];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 3]) {
        let [bass, mid, treble] = params;
        self.set_bass_db(bass.get());
        self.set_mid_db(mid.get());
        self.set_treble_db(treble.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
