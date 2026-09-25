//! A three-band tone control: bass, mid and treble.
//!
//! Not a model of an amplifier's tone stack, since a neural capture already
//! carries the amp's, but the shaping a player reaches for after it: a low
//! shelf at 120 Hz, a peak at 750 Hz with a broad Q of 0.7, and a high
//! shelf at 3.2 kHz. Each is a second-order section from the RBJ Audio EQ
//! Cookbook, run in series in transposed direct form II. The corners are
//! fixed; only the three gains move.
//!
//! # Flat is exact
//!
//! At 0 dB every section's coefficients come out identical on both sides of
//! the difference equation, so with nothing in its state a section is
//! `y = x`. The tone control notices that and copies the block through,
//! which is bit for bit down to the sign of a zero. A flat tone control is
//! therefore transparent, and a host can leave one in a chain it wants to
//! compare against the same chain without it.
//!
//! # Moving a gain
//!
//! A gain glides towards its setting with a 20 ms one-pole, stepped once
//! per block by an amount that depends on the block's length, so the glide
//! takes the same time at every block size; the section's coefficients are
//! recomputed on every step until the gain lands. A moving knob is a run of
//! small coefficient steps rather than one jump, and a still knob costs
//! nothing but the comparison. With the gains still the output does not
//! depend on how frames are grouped into blocks; while one is gliding it
//! does, because the glide is stepped per block. Latency zero, nothing to
//! allocate.
//!
//! [`Tone`] is the rack's module: one [`Mono`] per channel, under one set
//! of parameters.

use std::f32::consts::PI;
use std::sync::Arc;

use crate::channels::{Channels, PerChannel};
use crate::dsp::biquad::Biquad;
use crate::dsp::checked_sample_rate;
use crate::node::{Param, ParamSpec, Unit};

/// The low shelf's corner, in Hz.
pub const BASS_HZ: f32 = 120.0;
/// The mid peak's centre, in Hz.
pub const MID_HZ: f32 = 750.0;
/// The high shelf's corner, in Hz.
pub const TREBLE_HZ: f32 = 3200.0;

/// The mid peak's width: broad, a tone knob rather than a notch.
const MID_Q: f32 = 0.7;
/// The one-pole time a gain takes to cover 63% of a change, in ms.
const SMOOTH_MS: f32 = 20.0;
/// The shelves' slope: `S = 1` in the cookbook's terms, as a `Q`.
const SHELF_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Within this many dB of its setting, a gain lands on it.
const SNAP_DB: f32 = 1e-3;

/// The cookbook's `A`: the square root of the linear gain, which is what
/// its shelf and peak formulae are written in terms of.
fn amplitude(db: f32) -> f32 {
    10f32.powf(db / 40.0)
}

fn low_shelf(b: &mut Biquad, db: f32, hz: f32, sample_rate: f32) {
    let a = amplitude(db);
    let w = 2.0 * PI * hz / sample_rate;
    let (s, c) = w.sin_cos();
    let alpha = s / (2.0 * SHELF_Q);
    let k = 2.0 * a.sqrt() * alpha;
    b.set(
        a * ((a + 1.0) - (a - 1.0) * c + k),
        2.0 * a * ((a - 1.0) - (a + 1.0) * c),
        a * ((a + 1.0) - (a - 1.0) * c - k),
        (a + 1.0) + (a - 1.0) * c + k,
        -2.0 * ((a - 1.0) + (a + 1.0) * c),
        (a + 1.0) + (a - 1.0) * c - k,
    );
}

fn high_shelf(b: &mut Biquad, db: f32, hz: f32, sample_rate: f32) {
    let a = amplitude(db);
    let w = 2.0 * PI * hz / sample_rate;
    let (s, c) = w.sin_cos();
    let alpha = s / (2.0 * SHELF_Q);
    let k = 2.0 * a.sqrt() * alpha;
    b.set(
        a * ((a + 1.0) + (a - 1.0) * c + k),
        -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
        a * ((a + 1.0) + (a - 1.0) * c - k),
        (a + 1.0) - (a - 1.0) * c + k,
        2.0 * ((a - 1.0) - (a + 1.0) * c),
        (a + 1.0) - (a - 1.0) * c - k,
    );
}

fn peak(b: &mut Biquad, db: f32, hz: f32, q: f32, sample_rate: f32) {
    let a = amplitude(db);
    let w = 2.0 * PI * hz / sample_rate;
    let (s, c) = w.sin_cos();
    let alpha = s / (2.0 * q);
    b.set(
        1.0 + alpha * a,
        -2.0 * c,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * c,
        1.0 - alpha / a,
    );
}

/// The low shelf.
pub const BASS: ParamSpec = ParamSpec {
    id: "bass_db",
    name: "Bass",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// The mid peak.
pub const MID: ParamSpec = ParamSpec {
    id: "mid_db",
    name: "Mid",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// The high shelf.
pub const TREBLE: ParamSpec = ParamSpec {
    id: "treble_db",
    name: "Treble",
    unit: Unit::Decibels,
    min: -12.0,
    max: 12.0,
    default: 0.0,
};

/// Bass, mid and treble in the rack, after the capture. Flat, it is
/// transparent bit for bit.
pub type Tone = Channels<Mono, 3>;

/// One channel of the tone control, which also runs on its own.
#[derive(Debug, Clone)]
pub struct Mono {
    sample_rate: f32,
    /// Settings, in dB: bass, mid, treble.
    target: [f32; 3],
    /// Where each gain is on its way there.
    current: [f32; 3],
    bands: [Biquad; 3],
    settled: bool,
}

impl Mono {
    /// A tone control at `sample_rate`, every band at its parameter's
    /// default: flat.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = Self {
            sample_rate: checked_sample_rate(sample_rate),
            target: [BASS.default, MID.default, TREBLE.default],
            current: [BASS.default, MID.default, TREBLE.default],
            bands: [Biquad::default(); 3],
            settled: true,
        };
        eq.recompute_all();
        eq
    }

    fn recompute(&mut self, band: usize) {
        let db = self.current[band];
        match band {
            0 => low_shelf(&mut self.bands[0], db, BASS_HZ, self.sample_rate),
            1 => peak(&mut self.bands[1], db, MID_HZ, MID_Q, self.sample_rate),
            _ => high_shelf(&mut self.bands[2], db, TREBLE_HZ, self.sample_rate),
        }
    }

    fn recompute_all(&mut self) {
        for band in 0..3 {
            self.recompute(band);
        }
    }

    fn set(&mut self, band: usize, db: f32) {
        if db != self.target[band] {
            self.target[band] = db;
            self.settled = false;
        }
    }

    /// Change the sample rate, keeping the settings and the signal.
    /// Does not allocate.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let rate = checked_sample_rate(sample_rate);
        if rate != self.sample_rate {
            self.sample_rate = rate;
            self.recompute_all();
        }
    }

    /// The low shelf's gain, in dB. A change glides.
    pub fn set_bass_db(&mut self, db: f32) {
        self.set(0, db);
    }

    /// The mid peak's gain, in dB. A change glides.
    pub fn set_mid_db(&mut self, db: f32) {
        self.set(1, db);
    }

    /// The high shelf's gain, in dB. A change glides.
    pub fn set_treble_db(&mut self, db: f32) {
        self.set(2, db);
    }

    /// The settings, in dB: bass, mid, treble. Where the gains are headed,
    /// not where a glide has reached.
    pub fn gains_db(&self) -> [f32; 3] {
        self.target
    }

    /// Delay the tone control adds to the signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget the signal and land every gain on its setting. Does not
    /// allocate.
    pub fn reset(&mut self) {
        self.current = self.target;
        self.settled = true;
        self.recompute_all();
        for band in &mut self.bands {
            band.clear();
        }
    }

    /// One step of every gain that is still on its way, sized for a block of
    /// `n` frames.
    fn glide(&mut self, n: usize) {
        let k = 1.0 - (-(n as f32) / (SMOOTH_MS * 1e-3 * self.sample_rate)).exp();
        let mut settled = true;
        for band in 0..3 {
            let (now, goal) = (self.current[band], self.target[band]);
            if now != goal {
                let next = now + k * (goal - now);
                self.current[band] = if (goal - next).abs() < SNAP_DB {
                    goal
                } else {
                    next
                };
                self.recompute(band);
                settled &= self.current[band] == goal;
            }
        }
        self.settled = settled;
    }

    /// Shape `input` into `output`, sample for sample. Allocation-free, any
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
        if !self.settled {
            self.glide(input.len());
        }
        if self.bands.iter().all(Biquad::is_identity) {
            output.copy_from_slice(input);
            return;
        }
        let [lo, mid, hi] = &mut self.bands;
        for (o, &x) in output.iter_mut().zip(input) {
            *o = hi.tick(mid.tick(lo.tick(x)));
        }
        for band in &mut self.bands {
            band.end_tail();
        }
    }
}

impl PerChannel<3> for Mono {
    const TYPE_ID: &'static str = "std.eq";
    const PARAMS: [ParamSpec; 3] = [BASS, MID, TREBLE];

    fn make(sample_rate: f64) -> Self {
        Self::new(sample_rate)
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.set_sample_rate(sample_rate);
    }

    fn apply(&mut self, params: &[Arc<Param>; 3]) {
        let [bass, mid, treble] = params;
        self.set_bass_db(bass.get());
        self.set_mid_db(mid.get());
        self.set_treble_db(treble.get());
    }

    fn reset(&mut self) {
        self.reset();
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process(input, output);
    }
}
