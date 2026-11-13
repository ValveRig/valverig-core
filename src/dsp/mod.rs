//! The DSP building blocks the modules are made of.
//!
//! [`convolution::Convolver`] runs an impulse response with no latency at
//! all, and [`cuts::Cuts`] is the pair of filters after it: the cabinet is
//! the two together. [`resample::Resampler`] converts between an
//! interface's sample rate and the rate a capture was made at, which is how
//! a host runs a rack at a rate its device does not offer. [`denormals`]
//! sets the thread's flush-to-zero mode around the rack, and
//! [`pitch::Detector`] reads the pitch of a string for a tuner, beside the
//! signal path and not in it. Inside the crate,
//! the filters, delay lines, one-pole smoothers and tail floors here are
//! what each module's own DSP is built from.
//!
//! Every stage here is mono and independent, holds to the contract in the
//! [crate documentation](crate), and runs just as well without a rack.
//!
//! # Running a cabinet
//!
//! ```
//! use valrig_core::dsp::convolution::Convolver;
//!
//! let ir: Vec<f32> = (0..2048).map(|i| (-(i as f32) / 300.0).exp() * 0.1).collect();
//! let mut cab = Convolver::new(&ir);   // allocates; before the audio thread starts
//!
//! let dry = [0.25f32; 64];
//! let mut wet = [0.0f32; 64];
//! cab.process(&dry, &mut wet);         // allocation-free, any block length, no latency
//!
//! assert_eq!(cab.latency(), 0);
//! ```

pub(crate) mod biquad;
pub(crate) mod decibels;
pub(crate) mod delay_line;
pub(crate) mod one_pole;
pub(crate) mod tails;

pub mod convolution;
pub mod cuts;
pub mod denormals;
pub mod pitch;
pub mod resample;

use crate::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};

/// A sample rate as the `f32` the coefficients are computed in.
///
/// # Panics
///
/// If `sample_rate` is not between [`MIN_SAMPLE_RATE`] and
/// [`MAX_SAMPLE_RATE`]. A NaN or an infinity fails the comparison and
/// panics with the rest: without this every buffer sized from a rate is
/// sized from whatever `as usize` makes of it, which is a silently wrong
/// buffer rather than a refusal.
pub(crate) fn checked_sample_rate(sample_rate: f64) -> f32 {
    assert!(
        (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate),
        "sample rate must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz, got {sample_rate}"
    );
    sample_rate as f32
}
