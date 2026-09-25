//! The ValveRig engine: a rack of modules and the controls on it.
//!
//! A plugin, an app or an audio worklet links this crate and gets
//! everything it needs: the [`node::Node`] contract every module
//! answers to, adapters wrapping `valverig-nam`'s capture and
//! `valverig-dsp`'s cabinet, gate, compressor, tone control, reverb, drive,
//! chorus, phaser and delay, a smoothed gain, a tap to the output, a send
//! and a return over the rack's buses, the [`rack::Rack`] that runs them
//! as stages in series and lanes in parallel panned into a stereo pair, and
//! the [`rack::Controls`] a host sets parameters on from any thread while
//! the rack runs on the audio one.
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
#![deny(missing_docs)]

pub mod bus;
pub mod cabinet;
pub mod capture;
pub mod channels;
pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod drive;
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
