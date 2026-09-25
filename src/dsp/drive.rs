//! A drive: a clean boost at one end of the knob, a fuzz at the other.
//!
//! The signal loses what sits under 40 Hz to a one-pole highpass, because an
//! overdrive that clips the rumble along with the notes goes muddy. It is
//! then multiplied by the drive and shaped by `tanh` around a small
//! [`BIAS`], so the clipping is soft and slightly asymmetric: odd harmonics
//! from the curve and a little even from the offset, as a valve stage
//! makes. A blocker at 10 Hz removes the offset the bias leaves, a one-pole
//! lowpass at the tone setting darkens the result, and the level scales it.
//!
//! At 0 dB of drive the shaper is within a tenth of a decibel of a straight
//! line for a guitar-level signal, so the bottom of the knob is a boost; at
//! [`MAX_DRIVE_DB`] it is a square wave with rounded corners.
//!
//! # Why it runs at twice the rate
//!
//! A shaper makes harmonics above the input's own, and any that land past
//! the Nyquist fold back down to frequencies unrelated to the note, which
//! is what makes cheap distortion sound gritty. So the shaper runs at twice
//! the sample rate: the sample and a zero go through an eighth-order
//! Butterworth lowpass at 0.4 of the original rate, then the shaper, then a
//! fourth-order one, and one of every two results is kept. The harmonics
//! made between the original Nyquist and the doubled one are filtered out
//! before they could fold.
//!
//! It is not a cure. A hard square wave has harmonics beyond the doubled
//! Nyquist too, and those fold whatever the filter; on a 7 kHz tone at full
//! drive the fold-back left at 13 kHz is 15 dB below what a plain `tanh` at
//! 48 kHz puts there, and on a guitar, whose energy sits far lower, much
//! more. The fourth order on the way down is a choice: an eighth rings 20%
//! on a hard edge, a fourth 11%.
//!
//! The drive and the level follow a change with a 5 ms one-pole per sample;
//! the tone takes effect at once. Per sample, no lookahead, so the latency
//! is zero and the output does not depend on how frames are grouped into
//! blocks. Nothing to allocate.

use crate::dsp::biquad::Biquad;
use crate::dsp::tails;
use crate::dsp::{checked_sample_rate, decibels, one_pole};

/// The offset into the shaper: a touch of asymmetry.
pub const BIAS: f32 = 0.08;
/// At or above this tone setting, in Hz, the lowpass is out of the path.
pub const TONE_OPEN_HZ: f32 = 20_000.0;
/// The most drive, in dB.
pub const MAX_DRIVE_DB: f32 = 40.0;

/// The highpass before the shaper, in Hz.
const HIGHPASS_HZ: f32 = 40.0;
/// The blocker after it, in Hz.
const DC_BLOCK_HZ: f32 = 10.0;
/// The one-pole time the drive and the level take to cover 63% of a change.
const SMOOTH_MS: f32 = 5.0;
/// The oversampling lowpasses' corner, as a fraction of the doubled rate.
const OS_CORNER: f32 = 0.2;
/// The Q of each section of the eighth-order Butterworth on the way up.
const UP_Q: [f32; 4] = [0.509_80, 0.601_34, 0.899_98, 2.562_92];
/// The Q of each section of the fourth-order one on the way down.
const DOWN_Q: [f32; 2] = [0.541_20, 1.306_56];
/// Within this of its target, a smoothed gain lands on it.
const SNAP: f32 = 1e-6;

#[inline]
fn shape(x: f32) -> f32 {
    (x + BIAS).tanh() - BIAS.tanh()
}

/// A mono drive.
#[derive(Debug, Clone)]
pub struct Drive {
    sample_rate: f32,
    // Settings.
    drive_db: f32,
    tone_hz: f32,
    level_db: f32,
    // Derived.
    gain_target: f32,
    level_target: f32,
    coeff: f32,
    hp_coeff: f32,
    dc_coeff: f32,
    tone_open: bool,
    tone_coeff: f32,
    // State.
    gain: f32,
    level: f32,
    hp: f32,
    dc: f32,
    tone: f32,
    up: [Biquad; 4],
    down: [Biquad; 2],
}

impl Drive {
    /// A drive at `sample_rate`: 12 dB of drive, tone 6 kHz, level 0 dB.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut drive = Self {
            sample_rate: checked_sample_rate(sample_rate),
            drive_db: 12.0,
            tone_hz: 6000.0,
            level_db: 0.0,
            gain_target: 0.0,
            level_target: 0.0,
            coeff: 0.0,
            hp_coeff: 0.0,
            dc_coeff: 0.0,
            tone_open: false,
            tone_coeff: 0.0,
            gain: 0.0,
            level: 0.0,
            hp: 0.0,
            dc: 0.0,
            tone: 0.0,
            up: UP_Q.map(|q| Biquad::lowpass(OS_CORNER, q)),
            down: DOWN_Q.map(|q| Biquad::lowpass(OS_CORNER, q)),
        };
        drive.derive();
        drive.gain = drive.gain_target;
        drive.level = drive.level_target;
        drive
    }

    fn derive(&mut self) {
        let sr = self.sample_rate;
        self.gain_target = decibels::to_linear(self.drive_db);
        self.level_target = decibels::to_linear(self.level_db);
        self.coeff = one_pole::from_time(SMOOTH_MS, sr);
        self.hp_coeff = one_pole::from_corner(HIGHPASS_HZ, sr);
        self.dc_coeff = one_pole::from_corner(DC_BLOCK_HZ, sr);
        self.tone_open = self.tone_hz >= TONE_OPEN_HZ;
        self.tone_coeff = one_pole::from_corner(self.tone_hz.min(TONE_OPEN_HZ), sr);
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

    /// How hard the shaper is driven, in dB, `0` to [`MAX_DRIVE_DB`] and
    /// clamped to that range. A change glides over 5 ms.
    pub fn set_drive_db(&mut self, db: f32) {
        let db = db.clamp(0.0, MAX_DRIVE_DB);
        if db != self.drive_db {
            self.drive_db = db;
            self.gain_target = decibels::to_linear(db);
        }
    }

    /// The corner of the lowpass after the shaper, in Hz; [`TONE_OPEN_HZ`]
    /// or above takes it out of the path entirely. Takes effect at once.
    pub fn set_tone_hz(&mut self, hz: f32) {
        if hz != self.tone_hz {
            self.tone_hz = hz;
            self.tone_open = hz >= TONE_OPEN_HZ;
            self.tone_coeff = one_pole::from_corner(hz.min(TONE_OPEN_HZ), self.sample_rate);
        }
    }

    /// The output level, in dB. A change glides over 5 ms.
    pub fn set_level_db(&mut self, db: f32) {
        if db != self.level_db {
            self.level_db = db;
            self.level_target = decibels::to_linear(db);
        }
    }

    /// Delay the drive adds to the signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget the signal and land the drive and the level on their
    /// settings. Does not allocate.
    pub fn reset(&mut self) {
        self.gain = self.gain_target;
        self.level = self.level_target;
        self.hp = 0.0;
        self.dc = 0.0;
        self.tone = 0.0;
        for filter in self.up.iter_mut().chain(&mut self.down) {
            filter.clear();
        }
    }

    /// The shaper at twice the rate: the sample and a zero in, the second
    /// of the two results out.
    #[inline]
    fn oversampled(&mut self, v: f32) -> f32 {
        let (mut a, mut b) = (2.0 * v, 0.0f32);
        for filter in &mut self.up {
            a = filter.tick(a);
            b = filter.tick(b);
        }
        a = shape(a);
        b = shape(b);
        for filter in &mut self.down {
            a = filter.tick(a);
            b = filter.tick(b);
        }
        b
    }

    /// Drive `input` into `output`, sample for sample. Allocation-free, any
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
            self.gain = one_pole::step(self.gain, self.gain_target, self.coeff, SNAP);
            self.level = one_pole::step(self.level, self.level_target, self.coeff, SNAP);
            self.hp += self.hp_coeff * (x - self.hp);
            let v = (x - self.hp) * self.gain;
            let w = self.oversampled(v);
            self.dc += self.dc_coeff * (w - self.dc);
            let mut w = w - self.dc;
            if !self.tone_open {
                self.tone += self.tone_coeff * (w - self.tone);
                w = self.tone;
            }
            *o = w * self.level;
        }
        for filter in self.up.iter_mut().chain(&mut self.down) {
            filter.end_tail();
        }
        self.hp = tails::end(self.hp);
        self.dc = tails::end(self.dc);
        self.tone = tails::end(self.tone);
    }
}
