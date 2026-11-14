//! The pitch of a string: what a tuner needs.
//!
//! [`Detector`] finds the fundamental of a stretch of signal with YIN
//! (de Cheveigné and Kawahara, *YIN, a fundamental frequency estimator for
//! speech and music*, 2002): the difference between the signal and itself a
//! lag later, normalised by its running mean so that the first deep dip is
//! the period and not one of its multiples, then a parabola through the dip
//! for the fraction of a sample a tuner reads its cents from.
//!
//! It is not a `process`-shaped stage: it does not sit in the signal path,
//! and a host runs it beside the audio, on a copy of the input, a few times
//! a second. It holds to the rest of the contract: construction allocates,
//! [`Detector::detect`] does not, and a detector is built for one sample
//! rate.
//!
//! ```
//! use valverig_core::dsp::pitch::Detector;
//!
//! let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
//! let low_e: Vec<f32> = (0..detector.window())
//!     .map(|i| (i as f32 / 48_000.0 * std::f32::consts::TAU * 82.41).sin() * 0.3)
//!     .collect();
//!
//! let heard = detector.detect(&low_e).expect("a string");
//! assert!((heard.hz - 82.41).abs() < 0.05);
//! ```

use super::checked_sample_rate;

/// Under this dip a lag is taken for the period at once: the threshold the
/// paper settles on.
const DIP: f32 = 0.15;

/// With no dip that deep, the deepest one is still taken if it is under
/// this. Above it the signal has no pitch worth a needle: noise, a chord,
/// the hand moving on the strings.
const FAINT: f32 = 0.35;

/// A stretch quieter than this, in RMS, is silence: about -80 dBFS.
const SILENCE: f32 = 1.0e-4;

/// What a [`Detector`] heard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    /// The fundamental, in Hz.
    pub hz: f32,
    /// How periodic the signal was at that pitch: `1.0` for a pure tone,
    /// falling towards `0.65` as it gets harder to call.
    pub clarity: f32,
}

/// Finds the fundamental of a stretch of signal between two frequencies.
#[derive(Debug, Clone)]
pub struct Detector {
    sample_rate: f32,
    /// The shortest and the longest lag looked at, in samples.
    shortest: usize,
    longest: usize,
    /// The difference at each lag, then the same normalised.
    dips: Vec<f32>,
}

impl Detector {
    /// A detector at `sample_rate` for fundamentals from `low_hz` to
    /// `high_hz`. A guitar in standard tuning runs from 82 Hz to about
    /// 1.3 kHz; a low B is 62 Hz.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is outside the crate's band, or the two frequencies
    /// are not `0 < low_hz < high_hz` with at least two samples to a period
    /// of the highest.
    pub fn new(sample_rate: f64, low_hz: f32, high_hz: f32) -> Self {
        let sample_rate = checked_sample_rate(sample_rate);
        assert!(
            low_hz > 0.0 && low_hz < high_hz && high_hz <= sample_rate / 2.0,
            "a pitch is looked for between two frequencies under half the sample rate, got {low_hz} to {high_hz} Hz at {sample_rate} Hz"
        );
        let longest = (sample_rate / low_hz).ceil() as usize + 1;
        let shortest = ((sample_rate / high_hz).floor() as usize).max(2);
        Self {
            sample_rate,
            shortest,
            longest,
            dips: vec![0.0; longest + 1],
        }
    }

    /// How many samples [`Self::detect`] looks at: four periods of the
    /// lowest pitch. It reads the last that many of what it is given, and
    /// hears nothing in less.
    pub fn window(&self) -> usize {
        4 * self.longest
    }

    /// The fundamental of the end of `signal`, or nothing: silence, noise,
    /// or fewer than [`Self::window`] samples.
    pub fn detect(&mut self, signal: &[f32]) -> Option<Pitch> {
        let window = self.window();
        if signal.len() < window {
            return None;
        }
        let x = &signal[signal.len() - window..];
        let energy: f32 = x.iter().map(|v| v * v).sum();
        if !energy.is_finite() || (energy / window as f32).sqrt() < SILENCE {
            return None;
        }
        // The difference function over a stretch every lag fits in.
        let span = window - self.longest;
        self.dips[0] = 0.0;
        for lag in 1..=self.longest {
            let mut sum = 0.0f32;
            for j in 0..span {
                let d = x[j] - x[j + lag];
                sum += d * d;
            }
            self.dips[lag] = sum;
        }
        // Normalised by its mean so far: a lag of nothing no longer wins,
        // and the period's own dip is deeper than its multiples'.
        let mut running = 0.0f32;
        self.dips[0] = 1.0;
        for lag in 1..=self.longest {
            running += self.dips[lag];
            self.dips[lag] = if running > 0.0 {
                self.dips[lag] * lag as f32 / running
            } else {
                1.0
            };
        }
        // The first dip under the threshold, followed down to its bottom;
        // failing that the deepest dip there is, if it is deep enough.
        let mut found = None;
        let mut lag = self.shortest;
        while lag < self.longest {
            if self.dips[lag] < DIP {
                while lag + 1 < self.longest && self.dips[lag + 1] < self.dips[lag] {
                    lag += 1;
                }
                found = Some(lag);
                break;
            }
            lag += 1;
        }
        let lag = found.or_else(|| {
            let deepest = (self.shortest..self.longest)
                .min_by(|a, b| self.dips[*a].total_cmp(&self.dips[*b]))?;
            (self.dips[deepest] < FAINT).then_some(deepest)
        })?;
        // A parabola through the dip and its two neighbours: the period to
        // a fraction of a sample.
        let (before, at, after) = (self.dips[lag - 1], self.dips[lag], self.dips[lag + 1]);
        let bend = before - 2.0 * at + after;
        let shift = if bend > 0.0 {
            (0.5 * (before - after) / bend).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Some(Pitch {
            hz: self.sample_rate / (lag as f32 + shift),
            clarity: (1.0 - at).clamp(0.0, 1.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(rate: f32, hz: f32, frames: usize, harmonics: &[f32]) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate;
                harmonics
                    .iter()
                    .enumerate()
                    .map(|(n, a)| a * (t * TAU * hz * (n + 1) as f32).sin())
                    .sum::<f32>()
                    * 0.2
            })
            .collect()
    }

    /// How far off a reading is, in cents.
    fn cents(heard: f32, played: f32) -> f32 {
        1200.0 * (heard / played).log2()
    }

    #[test]
    fn every_open_string_is_read_to_a_cent_at_each_rate() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut detector = Detector::new(f64::from(rate), 60.0, 1_400.0);
            for hz in [
                61.74f32, 82.41, 110.0, 146.83, 196.0, 246.94, 329.63, 659.26, 1318.5,
            ] {
                let x = tone(rate, hz, detector.window(), &[1.0]);
                let heard = detector
                    .detect(&x)
                    .unwrap_or_else(|| panic!("{hz} Hz at {rate}"));
                assert!(
                    cents(heard.hz, hz).abs() < 1.0,
                    "{hz} Hz at {rate} read as {}",
                    heard.hz
                );
                assert!(heard.clarity > 0.95, "{hz} Hz at {rate}: {}", heard.clarity);
            }
        }
    }

    /// A string is mostly overtones: the note is the fundamental all the
    /// same, even where it is the weakest thing in the sound.
    #[test]
    fn a_weak_fundamental_under_strong_overtones_is_still_the_note() {
        let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
        for hz in [82.41f32, 110.0, 196.0] {
            let x = tone(48_000.0, hz, detector.window(), &[0.25, 1.0, 0.7, 0.5, 0.3]);
            let heard = detector.detect(&x).expect("a note");
            assert!(cents(heard.hz, hz).abs() < 2.0, "{hz} read as {}", heard.hz);
        }
    }

    #[test]
    fn a_string_a_few_cents_off_reads_that_many_cents_off() {
        let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
        for off in [-30.0f32, -5.0, 5.0, 30.0] {
            let hz = 110.0 * 2.0f32.powf(off / 1200.0);
            let x = tone(48_000.0, hz, detector.window(), &[1.0, 0.5, 0.25]);
            let heard = detector.detect(&x).expect("a note");
            assert!(
                (cents(heard.hz, 110.0) - off).abs() < 1.0,
                "{off} cents read as {}",
                cents(heard.hz, 110.0)
            );
        }
    }

    #[test]
    fn the_level_does_not_change_the_reading() {
        let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
        let loud = tone(48_000.0, 146.83, detector.window(), &[1.0, 0.4]);
        let quiet: Vec<f32> = loud.iter().map(|v| v * 0.01).collect();
        let (a, b) = (
            detector.detect(&loud).unwrap(),
            detector.detect(&quiet).unwrap(),
        );
        assert!((a.hz - b.hz).abs() < 0.01, "{} and {}", a.hz, b.hz);
    }

    #[test]
    fn silence_noise_and_too_little_have_no_pitch() {
        let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
        let window = detector.window();
        assert_eq!(detector.detect(&vec![0.0; window]), None);
        assert_eq!(detector.detect(&vec![1.0e-6; window]), None);
        // White noise, the same every run.
        let mut seed = 22_222u32;
        let noise: Vec<f32> = (0..window)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 9) as f32 / (1u32 << 23) as f32 - 1.0) * 0.3
            })
            .collect();
        assert_eq!(detector.detect(&noise), None);
        let short = tone(48_000.0, 110.0, window - 1, &[1.0]);
        assert_eq!(detector.detect(&short), None);
        let odd = vec![f32::NAN; window];
        assert_eq!(detector.detect(&odd), None);
    }

    #[test]
    fn only_the_end_of_what_it_is_given_is_heard() {
        let mut detector = Detector::new(48_000.0, 60.0, 1_400.0);
        let mut x = tone(48_000.0, 82.41, detector.window(), &[1.0]);
        x.extend(tone(48_000.0, 329.63, detector.window(), &[1.0]));
        let heard = detector.detect(&x).expect("a note");
        assert!(cents(heard.hz, 329.63).abs() < 1.0, "{}", heard.hz);
    }

    #[test]
    #[should_panic(expected = "between two frequencies")]
    fn a_range_upside_down_is_refused() {
        let _ = Detector::new(48_000.0, 400.0, 100.0);
    }
}
