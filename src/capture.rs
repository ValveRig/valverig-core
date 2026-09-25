//! The neural capture, `valverig-nam`'s [`Model`], as a module.

use crate::dsp::decibels;
use crate::node::Node;
use crate::{Error, Result};
use valverig_nam::loader::Model;

/// A `.nam` capture in the rack.
///
/// The host loads the model - `valverig-nam` reads the file - and hands it
/// over. It runs at the rate it was trained at and no other: `prepare` at a
/// different rate is an error, because nothing in the rack resamples; a
/// host that runs at another rate converts at the rack's edges. And it is
/// mono: a capture sits in a lane or ahead of one, never after a mix, and
/// `prepare` for two channels is an error too.
///
/// A capture can carry a fixed gain after the model, for a host that wants
/// every capture at one level: [`Capture::normalized`] takes it from the
/// loudness the file states. At 0 dB the model's output passes bit for bit.
#[derive(Debug)]
pub struct Capture {
    model: Model,
    silence: Vec<f32>,
    scratch: Vec<f32>,
    gain: f32,
}

impl Capture {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.capture";

    /// Wrap a loaded model.
    pub fn new(model: Model) -> Self {
        Self {
            model,
            silence: Vec::new(),
            scratch: Vec::new(),
            gain: 1.0,
        }
    }

    /// A fixed gain after the model, in dB.
    pub fn set_gain_db(&mut self, db: f32) {
        self.gain = decibels::to_linear(db);
    }

    /// The fixed gain after the model, in dB.
    pub fn gain_db(&self) -> f32 {
        20.0 * self.gain.log10()
    }

    /// Bring the model's stated loudness to `target_dbfs`, when it states
    /// one: the reference plugin aims at -18 dBFS. A file that says nothing
    /// is left at 0 dB.
    pub fn normalized(mut self, target_dbfs: f32) -> Self {
        if let Some(loudness) = self.model.metadata.loudness {
            self.set_gain_db(target_dbfs - loudness as f32);
        }
        self
    }

    /// The loudness the file states for its output, in dBFS, when it states
    /// one. A host normalises against it; the reference plugin aims it at
    /// -18 dBFS.
    pub fn loudness(&self) -> Option<f64> {
        self.model.metadata.loudness
    }
}

impl Node for Capture {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn prepare(&mut self, sample_rate: f64, max_block: usize, channels: usize) -> Result<()> {
        if channels != 1 {
            return Err(Error::Channels {
                node: Self::TYPE_ID.into(),
                max: 1,
                got: channels,
            });
        }
        if let Some(needs) = self.model.expected_sample_rate
            && needs != sample_rate
        {
            return Err(Error::SampleRate {
                node: Self::TYPE_ID.into(),
                needs,
                got: sample_rate,
            });
        }
        // Sizes the model and settles it on silence.
        self.model.reset(sample_rate, max_block);
        self.silence = vec![0.0; max_block];
        self.scratch = vec![0.0; max_block];
        Ok(())
    }

    /// Settle the capture on silence again, as `prepare` did, but out of the
    /// module's own buffers so that nothing allocates: whole blocks until
    /// the model's own prewarm length has gone through, which is the
    /// reference's schedule too.
    fn reset(&mut self) {
        let block = self.silence.len();
        if block == 0 {
            return;
        }
        let target = self.model.prewarm_samples();
        let mut done = 0;
        while done < target {
            self.model.process_mono(&self.silence, &mut self.scratch);
            done += block;
        }
    }

    /// Nothing: settling a capture means running its prewarm length of
    /// silence through it, which is the one thing in a rack that cannot be
    /// done inside a callback. A capture that has latched a non-finite
    /// value needs [`Node::reset`], from the thread that prepared it, and
    /// the rack says so through [`crate::rack::Rack::faulted`].
    fn silence(&mut self) {}

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.model.process_mono(input[0], output[0]);
        if self.gain != 1.0 {
            for v in output[0].iter_mut() {
                *v *= self.gain;
            }
        }
    }
}
