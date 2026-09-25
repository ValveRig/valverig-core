//! The ValveRig engine: a rack of modules, the controls on it, the DSP the
//! modules are made of, and the `.rig` presets that describe a rack.
//!
//! A plugin, an app or an audio worklet links this crate and gets
//! everything it needs:
//!
//! - the modules: a [`gate`], a [`compressor`], a tone control in [`eq`],
//!   a [`drive`], a [`chorus`], a [`phaser`], a [`delay`] and a [`reverb`],
//!   each beside the DSP it runs; a [`capture`] of `valverig-nam`'s models,
//!   a [`cabinet`], a smoothed [`gain`], a [`tap`] to the output, and a send
//!   and a return over the rack's buses in [`bus`];
//! - the [`node::Node`] contract every module answers to;
//! - the [`rack::Rack`] that runs them as stages in series and lanes in
//!   parallel panned into a stereo pair, and the [`rack::Controls`] a host
//!   sets parameters on from any thread while the rack runs on the audio
//!   one;
//! - [`preset`]: the `.rig` document, and the registry that builds a rack
//!   from one and writes a running rack back;
//! - [`dsp`]: the building blocks, among them the zero-latency convolver
//!   and the resampler a host needs at a rack's edges.
//!
//! [`rack::Rack::process`] allocates nothing, takes no lock and does no
//! I/O; it accepts any block length, and its output does not depend on how
//! frames are grouped. It works unchanged from a CoreAudio callback, a
//! plugin's process block, an AudioWorklet, or an offline loop.
//!
//! # A rack
//!
//! ```
//! use valverig_core::drive::Drive;
//! use valverig_core::eq::Tone;
//! use valverig_core::gain::Gain;
//! use valverig_core::rack::Rack;
//!
//! let (mut rack, controls) = Rack::builder()
//!     .push("input", Gain::new())?
//!     .push("drive", Drive::new())?
//!     .push("eq", Tone::new())?
//!     .push("output", Gain::new())?
//!     .build();
//!
//! rack.prepare(48_000.0, 128)?;             // allocates; before the audio thread starts
//! controls.set("output/level_db", -6.0)?;   // from any thread, any time
//!
//! let dry = [0.1f32; 128];
//! let mut wet = [0.0f32; 128];
//! rack.process(&dry, &mut [&mut wet]);      // on the audio thread; allocation-free
//! # Ok::<(), valverig_core::Error>(())
//! ```
//!
//! A capture and a cabinet join the same way, once the host has loaded
//! them: `Capture::new(Model::from_file("amp.nam")?)` and
//! `Cabinet::new(&ir_samples, Some(48_000.0))`. Nothing here reads a file
//! or opens a device, and nothing here resamples: a module that works at
//! one rate only refuses to be prepared at another, and says which.
//!
//! # The contract
//!
//! Every DSP stage in the crate, each module's own and the building blocks
//! in [`dsp`], holds to the same five rules, which are what make it safe to
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
//!    is [`dsp::resample::Resampler`], which is not a `process`-shaped
//!    stage at all and reports [`dsp::resample::Resampler::lookahead`]
//!    instead.
//!
//! A stage's settings are plain setters. They are cheap when the value has
//! not changed, so a module pushes its whole parameter set into its stages
//! every block, and anything that would click when it jumped is smoothed.
#![deny(missing_docs)]

pub mod bus;
pub mod cabinet;
pub mod capture;
pub mod channels;
pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod drive;
pub mod dsp;
pub mod eq;
pub mod error;
pub mod gain;
pub mod gate;
pub mod node;
pub mod phaser;
pub mod preset;
pub mod rack;
pub mod reverb;
pub mod tap;
mod worker;

pub use error::{Error, Result};
