//! A chorus: the signal against a copy of itself a few milliseconds behind,
//! the distance swept by a slow sine so the copy wavers in pitch around the
//! original.
//!
//! The copy sits [`BASE_MS`] behind, and the sweep reaches up to 5 ms
//! either side of that at full depth. It is read out of the line with
//! four-point Hermite interpolation, exact at an integer distance and
//! smooth between. The two are blended by the mix: `0` is the original
//! alone, which copies through bit for bit; `0.5` the classic equal blend;
//! `1` the copy alone, which is a vibrato.
//!
//! The depth follows a change with a 20 ms one-pole and the mix with a 5 ms
//! one; the rate takes effect at once, the sweep continuing from where it
//! was. A second instance can run a fraction of a cycle along
//! ([`Chorus::set_phase_offset`]) so that a stereo pair sways against
//! itself.
//!
//! Per sample, no lookahead: latency zero, output independent of how frames
//! are grouped. The line is sized at construction and at a change of rate,
//! which allocate; nothing else does.

use crate::dsp::checked_sample_rate;
use crate::dsp::delay_line::Interpolated;
use crate::dsp::one_pole;
use crate::dsp::tails;

/// Where the copy sits, in milliseconds.
pub const BASE_MS: f32 = 7.0;

/// How far the sweep reaches either side of [`BASE_MS`] at full depth.
const MAX_DEPTH_MS: f32 = 5.0;
/// The one-pole time the depth takes to cover 63% of a change.
const DEPTH_SMOOTH_MS: f32 = 20.0;
/// The one-pole time the mix takes to cover 63% of a change.
const MIX_SMOOTH_MS: f32 = 5.0;
const MIX_SNAP: f32 = 1e-4;
/// In samples.
const DEPTH_SNAP: f32 = 1e-3;

/// A mono chorus.
#[derive(Debug, Clone)]
pub struct Chorus {
    sample_rate: f32,
    // Settings.
    rate_hz: f32,
    depth: f32,
    mix: f32,
    offset: f32,
    // Derived.
    base: f32,
    depth_target: f32,
    inc: f32,
    depth_coeff: f32,
    mix_coeff: f32,
    // State.
    phase: f32,
    depth_now: f32,
    mix_now: f32,
    line: Interpolated,
}

impl Chorus {
    /// A chorus at `sample_rate`: rate 0.8 Hz, depth 0.5, mix 0.5.
    /// Allocates.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let sr = checked_sample_rate(sample_rate);
        let mut chorus = Self {
            sample_rate: sr,
            rate_hz: 0.8,
            depth: 0.5,
            mix: 0.5,
            offset: 0.0,
            base: 0.0,
            depth_target: 0.0,
            inc: 0.0,
            depth_coeff: 0.0,
            mix_coeff: 0.0,
            phase: 0.0,
            depth_now: 0.0,
            mix_now: 0.5,
            line: Interpolated::new(((BASE_MS + MAX_DEPTH_MS) * 1e-3 * sr).ceil() as usize + 8),
        };
        chorus.derive();
        chorus.depth_now = chorus.depth_target;
        chorus
    }

    fn derive(&mut self) {
        let sr = self.sample_rate;
        self.base = (BASE_MS * 1e-3 * sr).round();
        self.depth_target = self.depth * MAX_DEPTH_MS * 1e-3 * sr;
        self.inc = self.rate_hz / sr;
        self.depth_coeff = one_pole::from_time(DEPTH_SMOOTH_MS, sr);
        self.mix_coeff = one_pole::from_time(MIX_SMOOTH_MS, sr);
    }

    /// Change the sample rate, keeping the settings. The line is rebuilt,
    /// which allocates, and the signal is lost.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if checked_sample_rate(sample_rate) != self.sample_rate {
            let settings = (self.rate_hz, self.depth, self.mix, self.offset);
            *self = Self::new(sample_rate);
            (self.rate_hz, self.depth, self.mix, self.offset) = settings;
            self.derive();
            self.depth_now = self.depth_target;
            self.mix_now = self.mix;
        }
    }

    /// The sweep's rate, in hertz; negative is treated as `0`, which holds
    /// the copy where it is. Takes effect at once.
    pub fn set_rate_hz(&mut self, hz: f32) {
        if hz != self.rate_hz {
            self.rate_hz = hz.max(0.0);
            self.inc = self.rate_hz / self.sample_rate;
        }
    }

    /// How far the sweep reaches, `0` none to `1` five milliseconds either
    /// side. Clamped to that range; a change glides over 20 ms.
    pub fn set_depth(&mut self, depth: f32) {
        let depth = depth.clamp(0.0, 1.0);
        if depth != self.depth {
            self.depth = depth;
            self.depth_target = depth * MAX_DEPTH_MS * 1e-3 * self.sample_rate;
        }
    }

    /// How much of the output is the copy, `0` none to `1` all. Clamped to
    /// that range; a change glides over 5 ms.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Start the sweep this fraction of a cycle along, `0` to `1`, wrapping
    /// outside it: a stereo pair a quarter apart sways against itself.
    pub fn set_phase_offset(&mut self, turns: f32) {
        self.offset = turns.rem_euclid(1.0);
    }

    /// Delay the chorus adds to the dry signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Silence the copy and land the depth and the mix on their settings.
    /// The sweep goes back to its start. Does not allocate.
    pub fn reset(&mut self) {
        self.line.clear();
        self.phase = 0.0;
        self.depth_now = self.depth_target;
        self.mix_now = self.mix;
    }

    /// Chorus `input` into `output`, sample for sample. Allocation-free,
    /// any length.
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
        let max_read = self.line.max_read();
        for (o, &x) in output.iter_mut().zip(input) {
            self.line.push(tails::end_line(x));
            let lfo = (std::f32::consts::TAU * (self.phase + self.offset)).sin();
            self.phase += self.inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            self.depth_now = one_pole::step(
                self.depth_now,
                self.depth_target,
                self.depth_coeff,
                DEPTH_SNAP,
            );
            let distance = (self.base + self.depth_now * lfo).clamp(1.0, max_read);
            let w = self.line.read(distance);
            let m = one_pole::step(self.mix_now, self.mix, self.mix_coeff, MIX_SNAP);
            self.mix_now = m;
            *o = if m == 0.0 {
                x
            } else if m == 1.0 {
                w
            } else {
                x * (1.0 - m) + w * m
            };
        }
    }
}
