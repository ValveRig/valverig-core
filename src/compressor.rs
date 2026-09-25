//! A compressor: less loud when loud, so the notes sit evenly into the amp.
//!
//! Feed-forward, in the log domain, after Giannoulis, Massberg and Reiss,
//! *Digital Dynamic Range Compressor Design: A Tutorial and Analysis*
//! (2012): the level in decibels goes through a gain computer with a soft
//! knee, the reduction it asks for is smoothed with separate attack and
//! release times, and the smoothed reduction, less the makeup gain, is what
//! the sample is multiplied by. Smoothing the *reduction* rather than the
//! level is what makes the attack and release behave the same way at every
//! ratio and knee.
//!
//! # What it computes
//!
//! Per sample, in this order:
//!
//! 1. the level, `20 · log10 |x|`. Below the bottom of the knee the
//!    reduction is zero, so nothing is computed there at all;
//! 2. the gain computer, with `over = level − threshold` and
//!    `slope = 1 − 1/ratio`: a reduction of `slope · over` above the knee,
//!    `slope · (over + knee/2)² / (2 · knee)` inside it, and zero below;
//! 3. the smoothing: `y = a·y + (1 − a)·r`, with the attack coefficient
//!    while `r` is above `y` and the release coefficient otherwise, flushed
//!    to zero under a millionth of a decibel so that a compressor at rest
//!    multiplies by exactly one;
//! 4. `out = x · 10^(−y/20) · makeup`.
//!
//! Below the threshold with no makeup, the whole of that is a multiply by
//! one and the signal comes through bit for bit.
//!
//! All of it is per sample with no lookahead, so the latency is zero, there
//! is nothing to allocate, and the output does not depend on how frames are
//! grouped into blocks.
//!
//! [`Compressor`] is the rack's module: one [`Mono`] per channel, under one
//! set of parameters.

use std::sync::Arc;

use crate::channels::{Channels, PerChannel};
use crate::dsp::{checked_sample_rate, decibels, one_pole};
use crate::node::{Param, ParamSpec, Unit};

/// `ln(10) / 20`: decibels to a natural-log exponent.
const DB_TO_NEPER: f32 = 0.115_129_25;

/// The reduction, in dB, below which it is flushed to zero.
const FLUSH_DB: f32 = 1e-6;

/// The level above which the signal is reduced.
pub const THRESHOLD: ParamSpec = ParamSpec {
    id: "threshold_db",
    name: "Threshold",
    unit: Unit::Decibels,
    min: -60.0,
    max: 0.0,
    default: -18.0,
};

/// Decibels in per decibel out above the threshold.
pub const RATIO: ParamSpec = ParamSpec {
    id: "ratio",
    name: "Ratio",
    unit: Unit::Ratio,
    min: 1.0,
    max: 20.0,
    default: 4.0,
};

/// How quickly the reduction follows a rising level.
pub const ATTACK: ParamSpec = ParamSpec {
    id: "attack_ms",
    name: "Attack",
    unit: Unit::Milliseconds,
    min: 0.1,
    max: 100.0,
    default: 5.0,
};

/// How quickly the reduction follows a falling level.
pub const RELEASE: ParamSpec = ParamSpec {
    id: "release_ms",
    name: "Release",
    unit: Unit::Milliseconds,
    min: 10.0,
    max: 2000.0,
    default: 100.0,
};

/// The width over which the ratio takes hold around the threshold.
pub const KNEE: ParamSpec = ParamSpec {
    id: "knee_db",
    name: "Knee",
    unit: Unit::Decibels,
    min: 0.0,
    max: 24.0,
    default: 6.0,
};

/// Gain after the reduction.
pub const MAKEUP: ParamSpec = ParamSpec {
    id: "makeup_db",
    name: "Makeup",
    unit: Unit::Decibels,
    min: 0.0,
    max: 24.0,
    default: 0.0,
};

/// A compressor in the rack, between the gate and the capture.
pub type Compressor = Channels<Mono, 6>;

/// One channel of the compressor, which also runs on its own.
#[derive(Debug, Clone)]
pub struct Mono {
    sample_rate: f32,
    // Settings.
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    makeup_db: f32,
    // Derived.
    slope: f32,
    /// The linear level at the bottom of the knee; below it nothing happens.
    knee_floor: f32,
    attack: f32,
    release: f32,
    makeup: f32,
    // State: the smoothed reduction, in dB, never negative.
    reduction: f32,
}

impl Mono {
    /// A compressor at `sample_rate`, every setting at its parameter's default.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut compressor = Self {
            sample_rate: checked_sample_rate(sample_rate),
            threshold_db: THRESHOLD.default,
            ratio: RATIO.default,
            attack_ms: ATTACK.default,
            release_ms: RELEASE.default,
            knee_db: KNEE.default,
            makeup_db: MAKEUP.default,
            slope: 0.0,
            knee_floor: 0.0,
            attack: 0.0,
            release: 0.0,
            makeup: 1.0,
            reduction: 0.0,
        };
        compressor.derive();
        compressor
    }

    fn derive(&mut self) {
        self.slope = 1.0 - 1.0 / self.ratio.max(1.0);
        self.knee_floor = decibels::to_linear(self.threshold_db - 0.5 * self.knee_db.max(0.0));
        self.attack = one_pole::retention(self.attack_ms, self.sample_rate);
        self.release = one_pole::retention(self.release_ms, self.sample_rate);
        self.makeup = decibels::to_linear(self.makeup_db);
    }

    /// Change the sample rate, keeping the settings and the state.
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
        }
    }

    /// The level, in dBFS, above which the signal is reduced. Finite.
    pub fn set_threshold_db(&mut self, db: f32) {
        if db != self.threshold_db {
            self.threshold_db = db;
            self.derive();
        }
    }

    /// How many decibels of input make one decibel of output above the
    /// threshold. `1` is no compression; anything under `1` is treated as
    /// `1`.
    pub fn set_ratio(&mut self, ratio: f32) {
        if ratio != self.ratio {
            self.ratio = ratio;
            self.derive();
        }
    }

    /// How quickly the reduction follows a rising level: the time constant
    /// in ms. Zero or less applies the full reduction on the first sample.
    pub fn set_attack_ms(&mut self, ms: f32) {
        if ms != self.attack_ms {
            self.attack_ms = ms;
            self.derive();
        }
    }

    /// How quickly the reduction follows a falling level, in ms.
    pub fn set_release_ms(&mut self, ms: f32) {
        if ms != self.release_ms {
            self.release_ms = ms;
            self.derive();
        }
    }

    /// The width, in dB, over which the ratio takes hold around the
    /// threshold. `0` is a hard knee; a negative width is treated as `0`.
    pub fn set_knee_db(&mut self, db: f32) {
        if db != self.knee_db {
            self.knee_db = db;
            self.derive();
        }
    }

    /// Gain applied after the reduction, in dB.
    ///
    /// Applied as a plain multiply with no smoothing of its own, so a host
    /// that sweeps it fast enough to hear will hear the steps: it is a
    /// trim, not a fader.
    pub fn set_makeup_db(&mut self, db: f32) {
        if db != self.makeup_db {
            self.makeup_db = db;
            self.derive();
        }
    }

    /// The reduction the last sample got, in dB, for a meter. Never
    /// negative; zero when the compressor is at rest.
    pub fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }

    /// Delay the compressor adds to the signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget the reduction. Does not allocate.
    pub fn reset(&mut self) {
        self.reduction = 0.0;
    }

    /// Compress `input` into `output`, sample for sample. Allocation-free,
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
        let mut y = self.reduction;
        for (o, &x) in output.iter_mut().zip(input) {
            let level = x.abs();
            let wanted = if level <= self.knee_floor {
                0.0
            } else {
                let over = 20.0 * level.log10() - self.threshold_db;
                if self.knee_db > 0.0 && 2.0 * over.abs() <= self.knee_db {
                    let t = over + 0.5 * self.knee_db;
                    self.slope * t * t / (2.0 * self.knee_db)
                } else if over > 0.0 {
                    self.slope * over
                } else {
                    0.0
                }
            };
            let a = if wanted > y {
                self.attack
            } else {
                self.release
            };
            y = a * y + (1.0 - a) * wanted;
            if y < FLUSH_DB {
                y = 0.0;
            }
            *o = x * ((-y * DB_TO_NEPER).exp() * self.makeup);
        }
        self.reduction = y;
    }
}

impl PerChannel<6> for Mono {
    const TYPE_ID: &'static str = "std.compressor";
    const PARAMS: [ParamSpec; 6] = [THRESHOLD, RATIO, ATTACK, RELEASE, KNEE, MAKEUP];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 6]) {
        let [threshold, ratio, attack, release, knee, makeup] = params;
        self.set_threshold_db(threshold.get());
        self.set_ratio(ratio.get());
        self.set_attack_ms(attack.get());
        self.set_release_ms(release.get());
        self.set_knee_db(knee.get());
        self.set_makeup_db(makeup.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
