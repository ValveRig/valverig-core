//! The reverb, `valverig-dsp`'s [`Reverb`](valverig_dsp::reverb::Reverb), as a
//! module: mono in a mono rack, a stereo pair after a mix.

use crate::node::{Node, Param, ParamSpec, Unit};
use crate::{Error, Result};
use std::sync::Arc;
use valverig_dsp::reverb::{MAX_PREDELAY_MS, Reverb as Inner};

/// How much of the output is tail.
pub const MIX: ParamSpec = ParamSpec {
    id: "mix",
    name: "Mix",
    unit: Unit::Ratio,
    min: 0.0,
    max: 1.0,
    default: 0.2,
};
/// How long the tail takes to fall by 60 dB.
pub const DECAY: ParamSpec = ParamSpec {
    id: "decay_s",
    name: "Decay",
    unit: Unit::Seconds,
    min: 0.1,
    max: 10.0,
    default: 2.0,
};
/// Above this the tail decays faster.
pub const DAMPING: ParamSpec = ParamSpec {
    id: "damping_hz",
    name: "Damping",
    unit: Unit::Hertz,
    min: 500.0,
    max: 20_000.0,
    default: 6000.0,
};
/// The gap before the tail begins.
pub const PREDELAY: ParamSpec = ParamSpec {
    id: "predelay_ms",
    name: "Pre-delay",
    unit: Unit::Milliseconds,
    min: 0.0,
    max: MAX_PREDELAY_MS,
    default: 10.0,
};

/// A reverb in the rack, after the cabinet or the mix. At a mix of zero it
/// is transparent bit for bit.
#[derive(Debug)]
pub struct Reverb {
    params: [Arc<Param>; 4],
    inner: Inner,
    channels: usize,
}

impl Reverb {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.reverb";

    /// A reverb at its defaults. Allocates.
    pub fn new() -> Self {
        Self {
            params: [
                Param::new(MIX),
                Param::new(DECAY),
                Param::new(DAMPING),
                Param::new(PREDELAY),
            ],
            inner: Inner::new(48_000.0),
            channels: 1,
        }
    }

    /// Push the parameters into the reverb. Each setter is a comparison when
    /// nothing changed, so this runs every block.
    fn apply(&mut self) {
        let [mix, decay, damping, predelay] = &self.params;
        self.inner.set_mix(mix.get());
        self.inner.set_decay_s(decay.get());
        self.inner.set_damping_hz(damping.get());
        self.inner.set_predelay_ms(predelay.get());
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Node for Reverb {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, _max_block: usize, channels: usize) -> Result<()> {
        if channels > 2 {
            return Err(Error::Channels {
                node: Self::TYPE_ID.into(),
                max: 2,
                got: channels,
            });
        }
        self.channels = channels;
        self.inner.set_sample_rate(sample_rate);
        self.apply();
        self.inner.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.apply();
        if self.channels == 1 {
            self.inner.process(input[0], output[0]);
        } else {
            let (left, right) = output.split_at_mut(1);
            self.inner
                .process_pair(input[0], input[1], left[0], right[0]);
        }
    }
}
