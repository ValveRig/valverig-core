//! What can go wrong opening a device.

use std::fmt;

/// Which side of the device a failure is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The device the chain is fed from.
    Input,
    /// The device the chain is played to.
    Output,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Input => "input",
            Direction::Output => "output",
        })
    }
}

/// Everything [`super::run`] and its configuration can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The block size asked for is zero, or larger than [`super::MAX_BLOCK`].
    #[error("a block of {0} frames, which is not between 1 and {max}", max = super::MAX_BLOCK)]
    Block(usize),
    /// The chain was said to produce a number of channels other than 1 or 2.
    #[error("{0} output channels, which is not 1 or 2")]
    OutputChannels(usize),
    /// The sample rate asked for is zero.
    #[error("a sample rate of zero")]
    SampleRate,
    /// The host has no default device of that kind.
    #[error("no default {0} device")]
    NoDevice(Direction),
    /// No device of that kind has the name asked for.
    #[error("no {0} device named {1:?}")]
    NoSuchDevice(Direction, String),
    /// The device offers no `f32` configuration at the requested rate.
    #[error("the {0} device offers no f32 configuration at {1} Hz")]
    NoConfig(Direction, u32),
    /// The device claims a configuration with no channels in it.
    #[error("the {0} device offers a configuration with no channels")]
    NoChannels(Direction),
    /// The host could not list its devices.
    #[error(transparent)]
    Devices(#[from] cpal::DevicesError),
    /// The device could not list its configurations.
    #[error(transparent)]
    Configs(#[from] cpal::SupportedStreamConfigsError),
    /// The stream could not be built.
    #[error(transparent)]
    Build(#[from] cpal::BuildStreamError),
    /// The stream could not be started.
    #[error(transparent)]
    Play(#[from] cpal::PlayStreamError),
}

/// What [`super::run`] and [`super::devices`] return. A device's failures are
/// not the engine's, and are only there with the `io` feature: they keep a
/// type of their own beside [`crate::Error`].
pub type Result<T> = std::result::Result<T, Error>;
