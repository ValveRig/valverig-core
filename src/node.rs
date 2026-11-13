//! What every module looks like to the rack.
//!
//! A node runs a signal of one or two channels into another of the same
//! width, with no allocation in `process`, at any block length up to the
//! ceiling it was prepared for, at a fixed sample rate, with a declared
//! latency and parameters addressed by stable string ids.
//!
//! The width is fixed at `prepare`, like the rate: the rack knows what the
//! signal is at every point and tells each module, so a mono primitive is
//! run once per channel by its adapter, and a module that cannot be
//! widened, a capture, refuses. A capture from `valrig-nam` and the
//! stages in [`dsp`](crate::dsp) keep their own mono APIs and are wrapped
//! by the modules in this crate, so `valrig-nam` does not have to depend
//! on `valrig-core` to be usable from it.

use crate::error::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// What a parameter's value means, so a host can label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A level or gain in decibels.
    Decibels,
    /// A frequency in hertz.
    Hertz,
    /// A time in milliseconds.
    Milliseconds,
    /// A time in seconds.
    Seconds,
    /// A dimensionless ratio, `0.0` to `1.0` unless the range says otherwise.
    Ratio,
    /// A whole number that picks something: a bus, a channel.
    Index,
}

/// A parameter's static description: everything a host needs to draw a
/// control for it, and the range the value is clamped to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// The stable id a preset and a host address it by: never an index.
    pub id: &'static str,
    /// A label for people.
    pub name: &'static str,
    /// What the value means.
    pub unit: Unit,
    /// The smallest value accepted.
    pub min: f32,
    /// The largest value accepted.
    pub max: f32,
    /// The value a fresh node starts at.
    pub default: f32,
}

/// A parameter's live value, shared between the node that reads it on the
/// audio thread and the host that sets it from anywhere else.
///
/// One atomic, so a set is lock-free and a get is a load. A node reads it
/// once per block and smooths towards it where a step would click.
#[derive(Debug)]
pub struct Param {
    spec: ParamSpec,
    bits: AtomicU32,
}

impl Param {
    /// A parameter at its default, shared.
    pub fn new(spec: ParamSpec) -> Arc<Self> {
        Arc::new(Self {
            spec,
            bits: AtomicU32::new(spec.default.to_bits()),
        })
    }

    /// Its description.
    pub fn spec(&self) -> &ParamSpec {
        &self.spec
    }

    /// The current value.
    pub fn get(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    /// Set the value, clamped to the range. A NaN is ignored.
    pub fn set(&self, value: f32) {
        if value.is_nan() {
            return;
        }
        self.bits.store(
            value.clamp(self.spec.min, self.spec.max).to_bits(),
            Ordering::Relaxed,
        );
    }
}

/// One module of a rack.
pub trait Node: Send + Sync + std::fmt::Debug {
    /// The namespaced type id, `std.gain` and the like: what a preset names
    /// to ask for a stage of this kind.
    fn type_id(&self) -> &'static str;

    /// The parameters, in a stable order. Empty for a stage that has none.
    fn params(&self) -> &[Arc<Param>] {
        &[]
    }

    /// Size everything for `max_block` frames of `channels` channels at
    /// `sample_rate`. Allocates; call it before the audio thread starts, and
    /// again on any change. A module that only works at one rate, or that
    /// cannot take that many channels, refuses here.
    fn prepare(&mut self, sample_rate: f64, max_block: usize, channels: usize) -> Result<()>;

    /// Forget the signal, keeping every buffer. Does not allocate, but may
    /// take as long as running audio: a capture settles itself on silence
    /// here. Call it from the thread that prepares the rack, not from the
    /// audio one.
    fn reset(&mut self);

    /// Drop whatever the module is holding, as cheaply as it can be
    /// dropped, so that a rack can recover inside its own callback when a
    /// non-finite sample appears and poisons a feedback path.
    ///
    /// Must not allocate and must not settle. The default is [`Node::reset`],
    /// which is already only buffer-clearing for every stage but one; a
    /// module whose reset would take real time overrides this to do the
    /// cheap part and leave the rest to `reset`.
    fn silence(&mut self) {
        self.reset();
    }

    /// The delay this stage adds, in frames, constant after `prepare`.
    fn latency(&self) -> usize {
        0
    }

    /// Run one block: one input and one output slice per prepared channel,
    /// all of one length of at most `max_block` frames. No allocation, no
    /// locks, no I/O.
    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]);

    /// Add whatever this module sends straight to the rack's output: called
    /// once per block after every stage has run, with the rack's output
    /// buffers, and only when the module ran this block. Nothing, for all
    /// but a tap. The same rules as `process`.
    fn tap(&mut self, _output: &mut [&mut [f32]]) {}

    /// What this module puts on a bus, if anything: the bus number and the
    /// block it just processed, one slice per channel, the second empty
    /// when it is mono. The rack reads it right after the module ran, or
    /// after the lanes stage it sits in has been joined. Nothing, for all
    /// but a send.
    fn sends(&self) -> Option<(usize, [&[f32]; 2])> {
        None
    }

    /// The bus this module reads, if any. Nothing, for all but a return.
    fn receives(&self) -> Option<usize> {
        None
    }

    /// The bus's block, handed over by the rack before the module runs:
    /// one slice per channel, the second empty when the bus is mono, both
    /// empty when nothing has been sent on it this block. The same rules
    /// as `process`.
    fn receive(&mut self, _bus: [&[f32]; 2]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_param_starts_at_its_default_and_clamps_to_its_range() {
        let p = Param::new(ParamSpec {
            id: "x",
            name: "X",
            unit: Unit::Ratio,
            min: 0.0,
            max: 1.0,
            default: 0.25,
        });
        assert_eq!(p.get(), 0.25);
        p.set(0.5);
        assert_eq!(p.get(), 0.5);
        p.set(7.0);
        assert_eq!(p.get(), 1.0);
        p.set(-7.0);
        assert_eq!(p.get(), 0.0);
        p.set(f32::NAN);
        assert_eq!(p.get(), 0.0, "a NaN is ignored");
    }
}
