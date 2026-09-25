//! A delay: the signal again some time later, and again fainter, the highs
//! going first.
//!
//! One line of up to [`MAX_TIME_MS`], read a fractional distance behind the
//! write point with four-point Hermite interpolation. What is read goes
//! through a one-pole lowpass at the tone setting, or straight through at
//! [`TONE_OPEN_HZ`] and above, and that is both what is added to the
//! output, scaled by the mix, and what goes back into the line, scaled by
//! the feedback, so every repeat is darker than the last as it is on tape.
//! The dry signal passes at unity whatever the mix, and a mix of exactly
//! zero copies it through bit for bit while the line keeps running.
//!
//! A change of time is a glide, not a jump: the read distance moves towards
//! the new setting at half a sample per sample, which bends the pitch of
//! what is in the line the way a tape echo's motor does, by an octave at
//! most, and lands exactly. The mix follows a change with a 5 ms one-pole; the
//! feedback and the tone take effect at once.
//!
//! Per sample, no lookahead: latency zero, output independent of how frames
//! are grouped. The line is sized at construction and at a change of rate,
//! which allocate; nothing else does.
//!
//! [`Delay`] is the rack's module: one [`Mono`] per channel, under one set
//! of parameters.

use std::sync::Arc;

use crate::channels::{Channels, PerChannel};
use crate::dsp::checked_sample_rate;
use crate::dsp::delay_line::Interpolated;
use crate::dsp::one_pole;
use crate::dsp::tails;
use crate::node::{Param, ParamSpec, Unit};

/// The longest time between repeats, in milliseconds.
pub const MAX_TIME_MS: f32 = 2000.0;
/// The shortest, in milliseconds.
pub const MIN_TIME_MS: f32 = 1.0;
/// At or above this tone setting, in Hz, the lowpass is out of the path.
pub const TONE_OPEN_HZ: f32 = 20_000.0;
/// The most feedback.
pub const MAX_FEEDBACK: f32 = 0.95;

/// How fast the read distance moves towards a new time, in samples per
/// sample.
const GLIDE: f32 = 0.5;
/// The one-pole time the mix takes to cover 63% of a change.
const MIX_SMOOTH_MS: f32 = 5.0;
const MIX_SNAP: f32 = 1e-4;

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
pub type Delay = Channels<Mono, 4>;

/// One channel of the delay, which also runs on its own.
#[derive(Debug, Clone)]
pub struct Mono {
    sample_rate: f32,
    // Settings.
    time_ms: f32,
    feedback: f32,
    mix: f32,
    tone_hz: f32,
    // Derived.
    /// The read distance the time asks for, in samples.
    time_target: f32,
    tone_open: bool,
    tone_coeff: f32,
    mix_coeff: f32,
    // State.
    time_now: f32,
    mix_now: f32,
    lp: f32,
    line: Interpolated,
}

impl Mono {
    /// A delay at `sample_rate`, every setting at its parameter's default. Allocates.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let sr = checked_sample_rate(sample_rate);
        let mut delay = Self {
            sample_rate: sr,
            time_ms: TIME.default,
            feedback: FEEDBACK.default,
            mix: MIX.default,
            tone_hz: TONE.default,
            time_target: 0.0,
            tone_open: false,
            tone_coeff: 0.0,
            mix_coeff: 0.0,
            time_now: 0.0,
            mix_now: MIX.default,
            lp: 0.0,
            line: Interpolated::new((MAX_TIME_MS * 1e-3 * sr).ceil() as usize + 8),
        };
        delay.derive();
        delay.time_now = delay.time_target;
        delay
    }

    /// The read distance, in samples, for a time of `ms`. Read before
    /// write, so a time of `T` samples is a distance of `T - 1`.
    fn distance_for(&self, ms: f32) -> f32 {
        ((ms * 1e-3 * self.sample_rate).round() - 1.0).clamp(1.0, self.line.max_read())
    }

    fn derive(&mut self) {
        self.time_target = self.distance_for(self.time_ms);
        self.tone_open = self.tone_hz >= TONE_OPEN_HZ;
        self.tone_coeff = one_pole::from_corner(self.tone_hz.min(TONE_OPEN_HZ), self.sample_rate);
        self.mix_coeff = one_pole::from_time(MIX_SMOOTH_MS, self.sample_rate);
    }

    /// Change the sample rate, keeping the settings. The line is rebuilt,
    /// which allocates, and the repeats are lost.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if checked_sample_rate(sample_rate) != self.sample_rate {
            let settings = (self.time_ms, self.feedback, self.mix, self.tone_hz);
            *self = Self::new(sample_rate);
            (self.time_ms, self.feedback, self.mix, self.tone_hz) = settings;
            self.derive();
            self.time_now = self.time_target;
            self.mix_now = self.mix;
        }
    }

    /// The time between repeats, in milliseconds, [`MIN_TIME_MS`] to
    /// [`MAX_TIME_MS`] and clamped to that range. A change glides.
    pub fn set_time_ms(&mut self, ms: f32) {
        let ms = ms.clamp(MIN_TIME_MS, MAX_TIME_MS);
        if ms != self.time_ms {
            self.time_ms = ms;
            self.time_target = self.distance_for(ms);
        }
    }

    /// How much of each repeat goes round again, `0` to [`MAX_FEEDBACK`]
    /// and clamped to it. Takes effect at once.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback.clamp(0.0, MAX_FEEDBACK);
    }

    /// How loud the repeats are against the dry signal, `0` none to `1` as
    /// loud. Clamped to that range; a change glides over 5 ms.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// The corner of the lowpass every repeat goes through, in Hz;
    /// [`TONE_OPEN_HZ`] or above takes it out of the path entirely. Takes
    /// effect at once.
    pub fn set_tone_hz(&mut self, hz: f32) {
        if hz != self.tone_hz {
            self.tone_hz = hz;
            self.tone_open = hz >= TONE_OPEN_HZ;
            self.tone_coeff = one_pole::from_corner(hz.min(TONE_OPEN_HZ), self.sample_rate);
        }
    }

    /// Delay the delay adds to the dry signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Silence the repeats and land the time and the mix on their settings.
    /// Does not allocate.
    pub fn reset(&mut self) {
        self.line.clear();
        self.lp = 0.0;
        self.time_now = self.time_target;
        self.mix_now = self.mix;
    }

    /// Delay `input` into `output`, sample for sample. Allocation-free, any
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
        for (o, &x) in output.iter_mut().zip(input) {
            let diff = self.time_target - self.time_now;
            self.time_now = if diff.abs() <= GLIDE {
                self.time_target
            } else {
                self.time_now + diff.signum() * GLIDE
            };
            let r = self.line.read(self.time_now);
            let wet = if self.tone_open {
                r
            } else {
                self.lp += self.tone_coeff * (r - self.lp);
                self.lp
            };
            self.line.push(tails::end_line(x + self.feedback * wet));
            let m = one_pole::step(self.mix_now, self.mix, self.mix_coeff, MIX_SNAP);
            self.mix_now = m;
            *o = if m == 0.0 { x } else { x + wet * m };
        }
        self.lp = tails::end(self.lp);
    }
}

impl PerChannel<4> for Mono {
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
