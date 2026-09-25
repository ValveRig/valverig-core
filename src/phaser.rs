//! A phaser: the signal against a copy of itself shifted in phase by a
//! chain of allpasses whose corner sweeps, so a pair of notches slides up
//! and down the spectrum.
//!
//! Four first-order allpasses in series, each 90° round at the same corner,
//! so the chain is 360° round at the corner and 180° and 540° round either
//! side of it: added to the dry signal that is a notch at each of those and
//! a peak at the corner. A slow sine sweeps the corner around
//! [`CENTRE_HZ`] over up to 2.5 octaves each way at full depth; some of the
//! chain's output goes back into its input, which makes the peaks sharper.
//!
//! The two are blended by the mix: `0` the dry signal alone, which copies
//! through bit for bit; `0.5` the classic blend; `1` the chain alone, which
//! is an inaudible phase shift. The depth follows a change with a 20 ms
//! one-pole and the mix with a 5 ms one; the rate and the feedback take
//! effect at once. A second instance can run a fraction of a cycle along
//! ([`Mono::set_phase_offset`]) so that a stereo pair sways against
//! itself.
//!
//! Per sample, no lookahead: latency zero, output independent of how frames
//! are grouped. Nothing to allocate.
//!
//! [`Phaser`] is the rack's module: one [`Mono`] per channel, under one set
//! of parameters, each channel's sweep a quarter of a cycle behind the one
//! before it so a pair sways against itself.

use std::sync::Arc;

use crate::channels::{Channels, PerChannel};
use crate::chorus::CHANNEL_OFFSET;
use crate::dsp::tails;
use crate::dsp::{checked_sample_rate, one_pole};
use crate::node::{Param, ParamSpec, Unit};

/// Where the sweep is centred, in Hz.
pub const CENTRE_HZ: f32 = 700.0;
/// The most feedback.
pub const MAX_FEEDBACK: f32 = 0.9;

/// The allpasses in the chain.
const STAGES: usize = 4;
/// How far the sweep reaches each way at full depth.
const SWEEP_OCTAVES: f32 = 2.5;
/// The one-pole time the depth takes to cover 63% of a change.
const DEPTH_SMOOTH_MS: f32 = 20.0;
/// The one-pole time the mix takes to cover 63% of a change.
const MIX_SMOOTH_MS: f32 = 5.0;
const MIX_SNAP: f32 = 1e-4;
const DEPTH_SNAP: f32 = 1e-4;
/// The corner never reaches the Nyquist, where `tan` blows up.
const MAX_CORNER_FRACTION: f32 = 0.45;

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
pub type Phaser = Channels<Mono, 4>;

/// One channel of the phaser, which also runs on its own.
#[derive(Debug, Clone)]
pub struct Mono {
    sample_rate: f32,
    // Settings.
    rate_hz: f32,
    depth: f32,
    feedback: f32,
    mix: f32,
    offset: f32,
    // Derived.
    inc: f32,
    depth_coeff: f32,
    mix_coeff: f32,
    // State.
    phase: f32,
    depth_now: f32,
    mix_now: f32,
    /// Each stage's last input and last output.
    x1: [f32; STAGES],
    y1: [f32; STAGES],
    last: f32,
}

impl Mono {
    /// A phaser at `sample_rate`, every setting at its parameter's default.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut phaser = Self {
            sample_rate: checked_sample_rate(sample_rate),
            rate_hz: RATE.default,
            depth: DEPTH.default,
            feedback: FEEDBACK.default,
            mix: MIX.default,
            offset: 0.0,
            inc: 0.0,
            depth_coeff: 0.0,
            mix_coeff: 0.0,
            phase: 0.0,
            depth_now: DEPTH.default,
            mix_now: MIX.default,
            x1: [0.0; STAGES],
            y1: [0.0; STAGES],
            last: 0.0,
        };
        phaser.derive();
        phaser
    }

    fn derive(&mut self) {
        let sr = self.sample_rate;
        self.inc = self.rate_hz / sr;
        self.depth_coeff = one_pole::from_time(DEPTH_SMOOTH_MS, sr);
        self.mix_coeff = one_pole::from_time(MIX_SMOOTH_MS, sr);
    }

    /// Change the sample rate, keeping the settings; the signal is lost.
    /// Does not allocate.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let rate = checked_sample_rate(sample_rate);
        if rate != self.sample_rate {
            self.sample_rate = rate;
            self.derive();
            self.reset();
        }
    }

    /// The sweep's rate, in hertz; `0` holds the corner where it is, and
    /// negative is treated as `0`. Takes effect at once.
    pub fn set_rate_hz(&mut self, hz: f32) {
        let hz = hz.max(0.0);
        if hz != self.rate_hz {
            self.rate_hz = hz;
            self.inc = hz / self.sample_rate;
        }
    }

    /// How far the sweep reaches, `0` none to `1` two and a half octaves
    /// each way. Clamped to that range; a change glides over 20 ms.
    pub fn set_depth(&mut self, depth: f32) {
        self.depth = depth.clamp(0.0, 1.0);
    }

    /// How much of the chain's output goes back into its input, `0` to
    /// [`MAX_FEEDBACK`] and clamped to it. Takes effect at once.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback.clamp(0.0, MAX_FEEDBACK);
    }

    /// How much of the output is the shifted copy, `0` none to `1` all.
    /// Clamped to that range; a change glides over 5 ms.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Start the sweep this fraction of a cycle along, `0` to `1`, wrapping
    /// outside it.
    pub fn set_phase_offset(&mut self, turns: f32) {
        self.offset = turns.rem_euclid(1.0);
    }

    /// Delay the phaser adds to the dry signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget the signal, land the depth and the mix on their settings, and
    /// start the sweep again. Does not allocate.
    pub fn reset(&mut self) {
        self.x1 = [0.0; STAGES];
        self.y1 = [0.0; STAGES];
        self.last = 0.0;
        self.phase = 0.0;
        self.depth_now = self.depth;
        self.mix_now = self.mix;
    }

    /// Phase `input` into `output`, sample for sample. Allocation-free, any
    /// length.
    ///
    /// # Panics
    ///
    /// If `input` and `output` are different lengths.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        assert_eq!(
            input.len(),
            output.len(),
            "input and output must be the same length"
        );
        let sr = self.sample_rate;
        for (o, &x) in output.iter_mut().zip(input) {
            let lfo = (std::f32::consts::TAU * (self.phase + self.offset)).sin();
            self.phase += self.inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            self.depth_now =
                one_pole::step(self.depth_now, self.depth, self.depth_coeff, DEPTH_SNAP);
            let corner = (CENTRE_HZ * (SWEEP_OCTAVES * self.depth_now * lfo).exp2())
                .min(MAX_CORNER_FRACTION * sr);
            // The first-order allpass that is 90° round at the corner.
            let t = (std::f32::consts::PI * corner / sr).tan();
            let a = (t - 1.0) / (t + 1.0);
            let mut v = x + self.feedback * self.last;
            for stage in 0..STAGES {
                let y = a * v + self.x1[stage] - a * self.y1[stage];
                self.x1[stage] = v;
                self.y1[stage] = tails::end(y);
                v = y;
            }
            self.last = v;
            let m = one_pole::step(self.mix_now, self.mix, self.mix_coeff, MIX_SNAP);
            self.mix_now = m;
            *o = if m == 0.0 {
                x
            } else if m == 1.0 {
                v
            } else {
                x * (1.0 - m) + v * m
            };
        }
    }
}

impl PerChannel<4> for Mono {
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
