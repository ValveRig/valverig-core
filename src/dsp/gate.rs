//! A noise gate: silence between the notes, without eating the notes.
//!
//! A guitar's noise floor is steady: pickup hum, amp hiss, a neural
//! capture's own rendering of both. The notes sit well above it. The gate
//! follows the input's peak envelope, opens when it rises above a
//! threshold, holds open for a while after it falls back below, then closes
//! towards a floor. Two things keep it from chattering on a decaying note:
//! it opens at the threshold but does not start closing until the level has
//! fallen [`HYSTERESIS_DB`] further, and the hold time restarts every time
//! the level climbs back over that lower mark.
//!
//! # What it computes
//!
//! Per sample, in this order:
//!
//! 1. the detector: `env = max(|x|, env · d)`, a peak follower whose
//!    release is 10 ms, short enough to track a note's decay and long enough
//!    that the troughs of a low string's waveform do not read as silence;
//! 2. the state: closed becomes open when `env` reaches the threshold; open
//!    restarts its hold timer whenever `env` is above the threshold less
//!    [`HYSTERESIS_DB`], and becomes closed when the timer runs out;
//! 3. the gain: a one-pole towards `1` when open or the range floor when
//!    closed, at the attack rate when rising and the release rate when
//!    falling, snapped to the target once within a thousandth of it
//!    (0.009 dB). The snap is *relative* rather than absolute because a
//!    one-pole in `f32` stalls a few ULPs short of its target once each
//!    step rounds to nothing, and how short depends on the rate;
//! 4. `y = x · gain`.
//!
//! An open gate therefore multiplies by exactly one and passes the signal
//! bit for bit.
//!
//! All of it is per sample with no lookahead, so the output does not depend
//! on how frames are grouped into blocks and the latency is zero. There is
//! nothing to allocate: the whole state is a handful of numbers.

use crate::dsp::tails;
use crate::dsp::{checked_sample_rate, decibels, one_pole};

/// How much further than the threshold the level must fall before the gate
/// starts to close, in dB.
pub const HYSTERESIS_DB: f32 = 4.0;

/// The peak detector's release time, in milliseconds.
const DETECTOR_RELEASE_MS: f32 = 10.0;

/// Within this fraction of the target, the gain snaps to it.
const SNAP: f32 = 1e-3;

/// A mono noise gate.
#[derive(Debug, Clone)]
pub struct Gate {
    sample_rate: f32,
    // Settings, kept so that a change of rate can rederive everything.
    threshold_db: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
    range_db: f32,
    // Derived.
    open_at: f32,
    close_below: f32,
    attack: f32,
    release: f32,
    hold_samples: usize,
    floor: f32,
    detector_decay: f32,
    // State.
    env: f32,
    gain: f32,
    open: bool,
    hold_left: usize,
}

impl Gate {
    /// A closed gate at `sample_rate`: threshold −60 dB, attack 0.5 ms,
    /// hold 50 ms, release 100 ms, range −100 dB.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not between 1 kHz and 768 kHz.
    pub fn new(sample_rate: f64) -> Self {
        let mut gate = Self {
            sample_rate: checked_sample_rate(sample_rate),
            threshold_db: -60.0,
            attack_ms: 0.5,
            hold_ms: 50.0,
            release_ms: 100.0,
            range_db: -100.0,
            open_at: 0.0,
            close_below: 0.0,
            attack: 1.0,
            release: 1.0,
            hold_samples: 0,
            floor: 0.0,
            detector_decay: 0.0,
            env: 0.0,
            gain: 0.0,
            open: false,
            hold_left: 0,
        };
        gate.derive();
        gate.reset();
        gate
    }

    fn derive(&mut self) {
        self.open_at = decibels::to_linear(self.threshold_db);
        self.close_below = decibels::to_linear(self.threshold_db - HYSTERESIS_DB);
        self.attack = one_pole::from_time(self.attack_ms, self.sample_rate);
        self.release = one_pole::from_time(self.release_ms, self.sample_rate);
        self.hold_samples = (self.hold_ms * 1e-3 * self.sample_rate).round() as usize;
        self.floor = decibels::to_linear(self.range_db);
        self.detector_decay = 1.0 - one_pole::from_time(DETECTOR_RELEASE_MS, self.sample_rate);
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

    /// The peak level, in dBFS, at which the gate opens. Finite.
    pub fn set_threshold_db(&mut self, db: f32) {
        if db != self.threshold_db {
            self.threshold_db = db;
            self.derive();
        }
    }

    /// How quickly the gate opens: the one-pole time to 63%, in ms. Zero or
    /// less opens it on the first sample above the threshold.
    pub fn set_attack_ms(&mut self, ms: f32) {
        if ms != self.attack_ms {
            self.attack_ms = ms;
            self.derive();
        }
    }

    /// How long the gate stays fully open after the level has fallen below
    /// the closing mark, in ms. Zero starts the release at once.
    pub fn set_hold_ms(&mut self, ms: f32) {
        if ms != self.hold_ms {
            self.hold_ms = ms;
            self.derive();
        }
    }

    /// How quickly the gate closes once the hold has run out: the one-pole
    /// time to 63%, in ms.
    pub fn set_release_ms(&mut self, ms: f32) {
        if ms != self.release_ms {
            self.release_ms = ms;
            self.derive();
        }
    }

    /// The attenuation when closed, in dB: the floor the gain settles at.
    /// `0` leaves the gate inaudible; −100 is effectively silence.
    ///
    /// Must be finite. Negative infinity makes the floor exactly zero,
    /// which the relative snap in step 3 above can never reach, leaving the
    /// gain to decay towards it for ever.
    pub fn set_range_db(&mut self, db: f32) {
        if db != self.range_db {
            self.range_db = db;
            self.derive();
        }
    }

    /// Whether the gate is open or holding.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Delay the gate adds to the signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Close the gate and forget the level. Does not allocate.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.gain = self.floor;
        self.open = false;
        self.hold_left = 0;
    }

    /// Gate `input` into `output`, sample for sample. Allocation-free, any
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
        let (mut env, mut gain, mut open, mut hold_left) =
            (self.env, self.gain, self.open, self.hold_left);
        for (o, &x) in output.iter_mut().zip(input) {
            let level = x.abs();
            env = if level > env {
                level
            } else {
                env * self.detector_decay
            };

            if open {
                if env >= self.close_below {
                    hold_left = self.hold_samples;
                } else if hold_left > 0 {
                    hold_left -= 1;
                } else {
                    open = false;
                }
            } else if env >= self.open_at {
                open = true;
                hold_left = self.hold_samples;
            }

            let target = if open { 1.0 } else { self.floor };
            let coeff = if target > gain {
                self.attack
            } else {
                self.release
            };
            gain += coeff * (target - gain);
            if (target - gain).abs() <= SNAP * target {
                gain = target;
            }
            *o = x * gain;
        }
        // The detector decays geometrically towards zero and never arrives.
        self.env = tails::end(env);
        self.gain = gain;
        self.open = open;
        self.hold_left = hold_left;
    }
}
