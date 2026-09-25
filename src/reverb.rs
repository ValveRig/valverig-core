//! A reverb whose decay time is a setting rather than a knob.
//!
//! A feedback delay network: eight delay lines of prime lengths between 29
//! and 74 ms, mixed on every pass through an 8-point Hadamard transform,
//! which is orthonormal and so neither adds nor loses energy. The only
//! losses are the ones asked for. Each line's feedback gain is set so that
//! the line loses 60 dB in exactly [`Fdn::set_decay_s`] seconds whatever
//! its length, so every mode of the tail decays at the same rate and the
//! decay time means what it says.
//!
//! Around the tank: a one-pole lowpass in every loop takes the highs down
//! faster ([`Fdn::set_damping_hz`]); four series allpasses in front, with
//! Dattorro's lengths and coefficients, smear the input so the tail builds
//! up densely rather than as discrete echoes; a blocker at 20 Hz keeps DC
//! out of the loop; and a pre-delay of up to [`MAX_PREDELAY_MS`] sits
//! between the two.
//!
//! # Mixing
//!
//! The dry signal is mixed in after the tank, so a mix of exactly zero
//! copies it through bit for bit while the tank keeps running: turning the
//! mix back up reveals a tail that is already there. The mix follows a
//! change with a 5 ms one-pole. The other settings take effect at once, and
//! a change of pre-delay while sound is passing is a jump, as it is on the
//! hardware.
//!
//! [`Fdn::process`] is mono in and out, the tail being the mean of two
//! decorrelated taps; [`Fdn::process_pair`] is a stereo pair in and out,
//! the tank hearing the mean of the two sides and each side keeping its own
//! dry signal and its own tap. For a mono source with a stereo tail, pass
//! the same slice as both sides.
//!
//! Everything is per sample with no lookahead, so the latency is zero and
//! the output does not depend on how frames are grouped. The lines are
//! sized at construction and at a change of sample rate, which allocate;
//! nothing else does.
//!
//! [`Fdn`] is the network itself, and runs on its own. [`Reverb`] is the
//! rack's module: it runs the network mono in a mono rack and as a stereo
//! pair after a mix.

use std::sync::Arc;

use crate::dsp::checked_sample_rate;
use crate::dsp::delay_line::Fixed;
use crate::dsp::one_pole;
use crate::dsp::tails;
use crate::node::{Node, Param, ParamSpec, Unit};
use crate::{Error, Result};

/// The longest pre-delay, in milliseconds.
pub const MAX_PREDELAY_MS: f32 = 100.0;

/// The delay lines in the tank.
const LINES: usize = 8;
/// Line lengths at 48 kHz, in samples: primes, so no two lines share a
/// period, from 29 to 74 ms. Scaled to the sample rate in use.
const LINE_SAMPLES_48K: [usize; LINES] = [1409, 1783, 2099, 2459, 2731, 2999, 3307, 3547];
/// The input diffusion allpasses at 48 kHz: Dattorro's lengths and gains,
/// scaled from his 29.7 kHz.
const DIFFUSION_SAMPLES_48K: [usize; 4] = [229, 173, 611, 447];
const DIFFUSION_GAIN: [f32; 4] = [0.75, 0.75, 0.625, 0.625];
/// Where the input enters and the two outputs leave: three distinct rows of
/// the Hadamard matrix, so that each sums to zero and none is the other.
const IN_SIGNS: [f32; LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const LEFT_SIGNS: [f32; LINES] = [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
const RIGHT_SIGNS: [f32; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// `1 / sqrt(8)`: the Hadamard scale, and the injection and tap scale, so
/// that the tank holds the input's energy and hands it back the same way.
const NORM: f32 = 0.353_553_4;
const DC_BLOCK_HZ: f32 = 20.0;
const MIX_SMOOTH_MS: f32 = 5.0;
const MIX_SNAP: f32 = 1e-4;

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
    inner: Fdn,
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
            inner: Fdn::new(48_000.0),
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

#[derive(Debug, Clone)]
struct Allpass {
    delay: Fixed,
    gain: f32,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let d = self.delay.peek();
        let y = d - self.gain * x;
        self.delay.push(x + self.gain * y);
        y
    }
}

fn scaled(samples_at_48k: usize, sample_rate: f32) -> usize {
    (samples_at_48k as f32 * sample_rate / 48_000.0).round() as usize
}

/// An 8-point Walsh–Hadamard transform in place, unscaled.
#[inline]
fn hadamard(v: &mut [f32; LINES]) {
    let mut span = 1;
    while span < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + span {
                let (a, b) = (v[j], v[j + span]);
                v[j] = a + b;
                v[j + span] = a - b;
            }
            i += 2 * span;
        }
        span *= 2;
    }
}

/// The reverb's feedback delay network, with a mono or stereo tail.
#[derive(Debug, Clone)]
pub struct Fdn {
    sample_rate: f32,
    // Settings.
    mix: f32,
    decay_s: f32,
    damping_hz: f32,
    predelay_ms: f32,
    // Derived.
    mix_coeff: f32,
    dc_coeff: f32,
    damp_coeff: f32,
    gains: [f32; LINES],
    // State.
    mix_now: f32,
    dc_lp: f32,
    diffusion: [Allpass; 4],
    predelay: Fixed,
    predelay_samples: usize,
    lines: [Fixed; LINES],
    damp: [f32; LINES],
}

impl Fdn {
    /// A reverb at `sample_rate`, every setting at its parameter's default. Allocates.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let sr = checked_sample_rate(sample_rate);
        let allpass = |i: usize| Allpass {
            delay: Fixed::new(scaled(DIFFUSION_SAMPLES_48K[i], sr)),
            gain: DIFFUSION_GAIN[i],
        };
        let mut reverb = Self {
            sample_rate: sr,
            mix: MIX.default,
            decay_s: DECAY.default,
            damping_hz: DAMPING.default,
            predelay_ms: PREDELAY.default,
            mix_coeff: 0.0,
            dc_coeff: 0.0,
            damp_coeff: 0.0,
            gains: [0.0; LINES],
            mix_now: MIX.default,
            dc_lp: 0.0,
            diffusion: [allpass(0), allpass(1), allpass(2), allpass(3)],
            predelay: Fixed::new((MAX_PREDELAY_MS * 1e-3 * sr) as usize + 1),
            predelay_samples: 0,
            lines: LINE_SAMPLES_48K.map(|n| Fixed::new(scaled(n, sr))),
            damp: [0.0; LINES],
        };
        reverb.derive();
        reverb
    }

    fn derive(&mut self) {
        self.mix_coeff = one_pole::from_time(MIX_SMOOTH_MS, self.sample_rate);
        self.dc_coeff = one_pole::from_corner(DC_BLOCK_HZ, self.sample_rate);
        self.damp_coeff = one_pole::from_corner(self.damping_hz, self.sample_rate);
        // 60 dB over `decay_s`, per line, whatever its length.
        for (gain, line) in self.gains.iter_mut().zip(&self.lines) {
            let passes_per_second = self.sample_rate / line.len() as f32;
            *gain = 10f32.powf(-3.0 / (self.decay_s.max(0.01) * passes_per_second));
        }
        let longest = self.predelay.len() - 1;
        self.predelay_samples =
            ((self.predelay_ms * 1e-3 * self.sample_rate).round() as usize).min(longest);
    }

    /// Change the sample rate, keeping the settings. The lines are rebuilt,
    /// which allocates, and the tail is lost.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if checked_sample_rate(sample_rate) != self.sample_rate {
            let settings = (self.mix, self.decay_s, self.damping_hz, self.predelay_ms);
            *self = Self::new(sample_rate);
            (self.mix, self.decay_s, self.damping_hz, self.predelay_ms) = settings;
            self.mix_now = self.mix;
            self.derive();
        }
    }

    /// How much of the output is tail, `0` none to `1` all. Clamped to that
    /// range. A change glides over 5 ms.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// How long the tail takes to fall by 60 dB, in seconds. Anything under
    /// 0.01 s is treated as 0.01 s. Takes effect at once.
    pub fn set_decay_s(&mut self, seconds: f32) {
        if seconds != self.decay_s {
            self.decay_s = seconds;
            self.derive();
        }
    }

    /// The corner of the lowpass in every loop, in Hz: above it the tail
    /// decays faster. Takes effect at once.
    pub fn set_damping_hz(&mut self, hz: f32) {
        if hz != self.damping_hz {
            self.damping_hz = hz;
            self.derive();
        }
    }

    /// The gap before the tail begins, in milliseconds, up to
    /// [`MAX_PREDELAY_MS`] and clamped to it. Takes effect at once, so a
    /// change while sound is passing is a jump.
    pub fn set_predelay_ms(&mut self, ms: f32) {
        if ms != self.predelay_ms {
            self.predelay_ms = ms;
            self.derive();
        }
    }

    /// The gap before the tail begins, in samples: the pre-delay plus the
    /// shortest line.
    pub fn onset_samples(&self) -> usize {
        self.predelay_samples + self.lines.iter().map(Fixed::len).min().unwrap_or(0)
    }

    /// Delay the reverb adds to the dry signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Silence the tail and land the mix on its setting. Does not allocate.
    pub fn reset(&mut self) {
        self.mix_now = self.mix;
        self.dc_lp = 0.0;
        for allpass in &mut self.diffusion {
            allpass.delay.clear();
        }
        self.predelay.clear();
        for line in &mut self.lines {
            line.clear();
        }
        self.damp = [0.0; LINES];
    }

    /// One sample through the tank; returns the two wet taps.
    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32) {
        self.dc_lp += self.dc_coeff * (x - self.dc_lp);
        let mut v = x - self.dc_lp;
        for allpass in &mut self.diffusion {
            v = allpass.tick(v);
        }
        let p = self.predelay.push_and_read(v, self.predelay_samples) * NORM;

        let mut out = [0.0f32; LINES];
        for (o, line) in out.iter_mut().zip(&self.lines) {
            *o = line.peek();
        }
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for i in 0..LINES {
            left += LEFT_SIGNS[i] * out[i];
            right += RIGHT_SIGNS[i] * out[i];
        }
        hadamard(&mut out);
        for i in 0..LINES {
            let mixed = out[i] * NORM;
            self.damp[i] += self.damp_coeff * (mixed - self.damp[i]);
            let w = p * IN_SIGNS[i] + self.damp[i] * self.gains[i];
            self.lines[i].push(tails::end_line(w));
        }
        (left * NORM, right * NORM)
    }

    #[inline]
    fn step_mix(&mut self) -> f32 {
        self.mix_now = one_pole::step(self.mix_now, self.mix, self.mix_coeff, MIX_SNAP);
        self.mix_now
    }

    /// Reverberate `input` into `output`, the tail being the mean of the
    /// two taps. Allocation-free, any length.
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
            let (l, r) = self.tick(x);
            let m = self.step_mix();
            *o = if m == 0.0 {
                x
            } else {
                x * (1.0 - m) + 0.5 * (l + r) * m
            };
        }
    }

    /// Reverberate a stereo pair: the tank hears the mean of the two sides,
    /// and each side keeps its own dry signal and gets its own tail.
    /// Allocation-free, any length.
    ///
    /// Passing the same slice as both sides is how a mono source gets a
    /// stereo tail.
    ///
    /// # Panics
    ///
    /// If the four slices are not all the same length.
    pub fn process_pair(
        &mut self,
        in_left: &[f32],
        in_right: &[f32],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        assert!(
            in_left.len() == in_right.len()
                && in_left.len() == left.len()
                && in_left.len() == right.len(),
            "all four must be the same length"
        );
        for i in 0..in_left.len() {
            let (xl, xr) = (in_left[i], in_right[i]);
            let (wl, wr) = self.tick(0.5 * (xl + xr));
            let m = self.step_mix();
            if m == 0.0 {
                left[i] = xl;
                right[i] = xr;
            } else {
                left[i] = xl * (1.0 - m) + wl * m;
                right[i] = xr * (1.0 - m) + wr * m;
            }
        }
    }
}
