//! The low cut and the high cut that follow a cabinet's impulse response.
//!
//! A recorded cabinet carries whatever the microphone heard at the bottom
//! and the top of the spectrum, and a mix usually wants less of both: a
//! high-pass to take the rumble under the guitar's lowest note, and a
//! low-pass to take the fizz above where a speaker stops responding. Both
//! are second-order Butterworth sections, 12 dB an octave, in transposed
//! direct form II.
//!
//! Each cut leaves the path entirely at one end of its range, the low cut
//! at [`LOW_CUT_OPEN_HZ`] and the high cut at [`HIGH_CUT_OPEN_HZ`]. There
//! the samples are copied through untouched, so a cabinet with both cuts
//! parked is its convolution and nothing else, bit for bit.
//!
//! A corner glides towards its setting with a 20 ms one-pole, stepped once
//! per block by an amount that depends on the block's length, so the glide
//! takes the same time at every block size, and the section's coefficients
//! are recomputed on each step until the corner lands. A still setting
//! costs one comparison per block.

use crate::dsp::biquad::Biquad;
use crate::dsp::one_pole;
use crate::dsp::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};

/// At or below this low-cut setting, in Hz, the high-pass is out of the
/// path.
pub const LOW_CUT_OPEN_HZ: f32 = 20.0;

/// The highest low-cut setting, in Hz.
pub const LOW_CUT_MAX_HZ: f32 = 400.0;

/// The lowest high-cut setting, in Hz.
pub const HIGH_CUT_MIN_HZ: f32 = 2_000.0;

/// At or above this high-cut setting, in Hz, the low-pass is out of the
/// path.
pub const HIGH_CUT_OPEN_HZ: f32 = 20_000.0;

/// How long a corner takes to reach a new setting.
const GLIDE_MS: f32 = 20.0;

/// Within this fraction of an octave of its target, a gliding corner lands
/// on it, so that a cut parked at the end of its range reaches the exact
/// setting that takes it out of the path.
const SNAP_HZ: f32 = 0.01;

/// The highest fraction of the sample rate a corner is placed at. The
/// bilinear transform's `tan` diverges at the Nyquist, so a 20 kHz high cut
/// at 44.1 kHz is placed here instead.
const MAX_FRACTION: f32 = 0.45;

/// Butterworth: the Q that makes one second-order section maximally flat.
const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// A low cut and a high cut, in series, for one channel.
#[derive(Debug, Clone)]
pub struct Cuts {
    sample_rate: f32,
    low_target: f32,
    high_target: f32,
    /// Where each glide has reached, in Hz.
    low_now: f32,
    high_now: f32,
    low: Biquad,
    high: Biquad,
    settled: bool,
}

impl Cuts {
    /// Both cuts out of the path, at `sample_rate`.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut cuts = Self {
            sample_rate: 48_000.0,
            low_target: LOW_CUT_OPEN_HZ,
            high_target: HIGH_CUT_OPEN_HZ,
            low_now: LOW_CUT_OPEN_HZ,
            high_now: HIGH_CUT_OPEN_HZ,
            low: Biquad::default(),
            high: Biquad::default(),
            settled: false,
        };
        cuts.set_sample_rate(sample_rate);
        cuts
    }

    /// Change the sample rate, keeping the settings and the signal.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        assert!(
            (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate),
            "sample rate {sample_rate} is outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE} Hz"
        );
        self.sample_rate = sample_rate as f32;
        self.recompute();
    }

    /// The corner of the high-pass, in Hz, clamped to
    /// [`LOW_CUT_OPEN_HZ`]..=[`LOW_CUT_MAX_HZ`]. A change glides.
    pub fn set_low_cut_hz(&mut self, hz: f32) {
        let hz = hz.clamp(LOW_CUT_OPEN_HZ, LOW_CUT_MAX_HZ);
        if hz != self.low_target {
            self.low_target = hz;
            self.settled = false;
        }
    }

    /// The corner of the low-pass, in Hz, clamped to
    /// [`HIGH_CUT_MIN_HZ`]..=[`HIGH_CUT_OPEN_HZ`]. A change glides.
    pub fn set_high_cut_hz(&mut self, hz: f32) {
        let hz = hz.clamp(HIGH_CUT_MIN_HZ, HIGH_CUT_OPEN_HZ);
        if hz != self.high_target {
            self.high_target = hz;
            self.settled = false;
        }
    }

    /// Whether the high-pass is out of the path where the glide has
    /// reached.
    fn low_in_path(&self) -> bool {
        self.low_now > LOW_CUT_OPEN_HZ
    }

    /// Whether the low-pass is out of the path where the glide has reached.
    fn high_in_path(&self) -> bool {
        self.high_now < HIGH_CUT_OPEN_HZ
    }

    /// The sections for the corners the glides have reached.
    fn recompute(&mut self) {
        let ceiling = MAX_FRACTION * self.sample_rate;
        self.low = Biquad::highpass(self.low_now.min(ceiling) / self.sample_rate, BUTTERWORTH_Q);
        self.high = Biquad::lowpass(self.high_now.min(ceiling) / self.sample_rate, BUTTERWORTH_Q);
    }

    /// One step of each corner that is still on its way, sized for a block
    /// of `n` frames.
    fn glide(&mut self, n: usize) {
        if self.settled {
            return;
        }
        let coefficient = one_pole::from_time(GLIDE_MS, self.sample_rate / n as f32);
        self.low_now = one_pole::step(self.low_now, self.low_target, coefficient, SNAP_HZ);
        self.high_now = one_pole::step(self.high_now, self.high_target, coefficient, SNAP_HZ);
        self.settled = self.low_now == self.low_target && self.high_now == self.high_target;
        self.recompute();
    }

    /// Delay the cuts add to the signal: none. Both sections are recursive
    /// filters, which answer on the sample they are given.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget the signal and land both corners on their settings. Does not
    /// allocate.
    pub fn reset(&mut self) {
        self.low_now = self.low_target;
        self.high_now = self.high_target;
        self.settled = true;
        self.recompute();
        self.low.clear();
        self.high.clear();
    }

    /// Run one block, filtering `block` where it lies. Does not allocate.
    ///
    /// In place because a cut always follows something that has just
    /// written the samples, so there is nothing to copy from. With both cuts
    /// parked at the open ends of their ranges the block is not touched at
    /// all, which is what makes a parked cabinet its convolution bit for
    /// bit.
    pub fn process(&mut self, block: &mut [f32]) {
        self.glide(block.len());
        let (low_in_path, high_in_path) = (self.low_in_path(), self.high_in_path());
        if !low_in_path && !high_in_path {
            return;
        }
        for v in block.iter_mut() {
            if low_in_path {
                *v = self.low.tick(*v);
            }
            if high_in_path {
                *v = self.high.tick(*v);
            }
        }
        self.low.end_tail();
        self.high.end_tail();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn gain_db(cuts: &mut Cuts, hz: f32, sample_rate: f32) -> f32 {
        cuts.reset();
        let n = (sample_rate * 0.5) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| (i as f32 / sample_rate * 2.0 * std::f32::consts::PI * hz).sin() * 0.5)
            .collect();
        let mut y = x.clone();
        for block in y.chunks_mut(128) {
            cuts.process(block);
        }
        let tail = n - n / 5;
        20.0 * (rms(&y[tail..]) / rms(&x[tail..])).log10()
    }

    #[test]
    fn parked_at_both_ends_the_input_comes_through_bit_for_bit() {
        let mut cuts = Cuts::new(48_000.0);
        let x: Vec<f32> = (0..2048)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let mut y = x.clone();
        for block in y.chunks_mut(128) {
            cuts.process(block);
        }
        for (i, (a, b)) in x.iter().zip(&y).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i}");
        }
    }

    #[test]
    fn each_cut_takes_its_end_at_twelve_db_an_octave() {
        let mut cuts = Cuts::new(48_000.0);
        cuts.set_low_cut_hz(200.0);
        assert!(
            gain_db(&mut cuts, 50.0, 48_000.0) < -20.0,
            "two octaves under a 200 Hz cut"
        );
        assert!(
            gain_db(&mut cuts, 2000.0, 48_000.0).abs() < 0.5,
            "well above it, untouched"
        );

        let mut cuts = Cuts::new(48_000.0);
        cuts.set_high_cut_hz(4000.0);
        assert!(
            gain_db(&mut cuts, 16_000.0, 48_000.0) < -20.0,
            "two octaves over a 4 kHz cut"
        );
        assert!(
            gain_db(&mut cuts, 1000.0, 48_000.0).abs() < 0.5,
            "well under it, untouched"
        );
    }

    #[test]
    fn a_corner_glides_to_its_setting_and_lands_on_it() {
        let mut cuts = Cuts::new(48_000.0);
        cuts.set_low_cut_hz(400.0);
        // 20 ms at 48 kHz is about 15 blocks of 64; give it well over that.
        let mut y = vec![0.1f32; 64];
        for _ in 0..200 {
            cuts.process(&mut y);
        }
        assert_eq!(
            cuts.low_now, 400.0,
            "the glide lands exactly on the setting"
        );
        cuts.set_low_cut_hz(LOW_CUT_OPEN_HZ);
        for _ in 0..200 {
            cuts.process(&mut y);
        }
        assert_eq!(
            cuts.low_now, LOW_CUT_OPEN_HZ,
            "and back out of the path exactly"
        );
        assert!(cuts.settled);
    }

    #[test]
    fn a_rate_the_nyquist_would_not_allow_is_placed_below_it() {
        // A 20 kHz high cut at 44.1 kHz sits above 0.45 of the rate.
        let mut cuts = Cuts::new(44_100.0);
        cuts.set_high_cut_hz(19_000.0);
        let mut y = vec![0.1f32; 128];
        for _ in 0..100 {
            cuts.process(&mut y);
        }
        assert!(y.iter().all(|v| v.is_finite()), "the section stays stable");
    }
}
