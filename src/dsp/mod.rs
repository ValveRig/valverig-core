//! Real-time audio DSP stages, one per box in a guitar signal chain.
//!
//! [`convolution::Convolver`] runs a cabinet impulse response with no
//! latency at all. [`gate::Gate`], [`compressor::Compressor`],
//! [`eq::ThreeBand`] and [`reverb::Reverb`] are the noise gate, the
//! compressor, the tone control and the reverb around it;
//! [`drive::Drive`], [`chorus::Chorus`], [`phaser::Phaser`] and
//! [`delay::Delay`] are the pedals in front of it and after it; and
//! [`resample::Resampler`] converts between an interface's sample rate and
//! the rate a capture was made at.
//!
//! Every stage is mono and independent: no shared bus, no trait to
//! implement, no framework. The rack's modules run one per channel under a
//! single set of parameters, and a stage runs just as well on its own.
//!
//! # The contract
//!
//! Each stage holds to the same five rules, which are what make it safe to
//! call from an audio callback:
//!
//! 1. **Construction allocates; `process` does not.** Build a stage, and
//!    change its sample rate, off the audio thread. Nothing in `process`
//!    allocates, frees, locks, blocks, logs or touches a file.
//! 2. **Any block length, in any order.** A host may hand over one frame,
//!    then 512, then three. Zero-length blocks are fine, and no stage asks
//!    for a maximum.
//! 3. **The output does not depend on how frames are grouped.** With the
//!    settings still, splitting a stream into blocks any way at all gives
//!    byte-identical output. Where a stage schedules work on its own grid,
//!    as the convolver does with its partitions, that grid is anchored to
//!    the absolute sample count and never to a block boundary.
//! 4. **One sample rate, fixed until it is changed.** Every rate-dependent
//!    coefficient is recomputed by `set_sample_rate`; no stage resamples
//!    on its own, and none silently runs coefficients built for another
//!    rate.
//! 5. **`latency()` reports the delay a stage adds, in frames.** Every
//!    stage here reports zero: none of them looks ahead. The one exception
//!    is [`resample::Resampler`], which is not a `process`-shaped stage at
//!    all and reports [`resample::Resampler::lookahead`] instead.
//!
//! Settings are plain setters. They are cheap when the value has not
//! changed, so a host can push the whole parameter set every block from
//! wherever it keeps them, and anything that would click when it jumped is
//! smoothed.
//!
//! # Running a cabinet
//!
//! ```
//! use valverig_core::dsp::convolution::Convolver;
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
//!
//! # Running a pedal
//!
//! ```
//! use valverig_core::dsp::drive::Drive;
//!
//! let mut drive = Drive::new(48_000.0);
//! drive.set_drive_db(18.0);
//! drive.set_tone_hz(4_000.0);
//!
//! let mut out = [0.0f32; 128];
//! for block in [64, 33, 31] {
//!     let dry = vec![0.05f32; block];
//!     drive.process(&dry, &mut out[..block]);
//! }
//! ```

mod biquad;
mod decibels;
mod delay_line;
mod one_pole;
mod tails;

pub mod chorus;
pub mod compressor;
pub mod convolution;
pub mod cuts;
pub mod delay;
pub mod denormals;
pub mod drive;
pub mod eq;
pub mod gate;
pub mod phaser;
pub mod resample;
pub mod reverb;

/// The lowest sample rate any stage accepts.
const MIN_SAMPLE_RATE: f64 = 1_000.0;

/// The highest sample rate any stage accepts.
///
/// Well above every rate an audio interface offers, and low enough that the
/// longest buffer a stage sizes from a rate, the delay's two seconds, stays
/// under six megabytes. The bound exists so that a rate arriving from
/// a device query, a preset or a test cannot turn into an allocation of an
/// arbitrary size.
const MAX_SAMPLE_RATE: f64 = 768_000.0;

/// A sample rate as the `f32` the coefficients are computed in.
///
/// # Panics
///
/// If `sample_rate` is not between [`MIN_SAMPLE_RATE`] and
/// [`MAX_SAMPLE_RATE`]. A NaN or an infinity fails the comparison and
/// panics with the rest: without this every buffer sized from a rate is
/// sized from whatever `as usize` makes of it, which is a silently wrong
/// buffer rather than a refusal.
fn checked_sample_rate(sample_rate: f64) -> f32 {
    assert!(
        (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate),
        "sample rate must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz, got {sample_rate}"
    );
    sample_rate as f32
}
