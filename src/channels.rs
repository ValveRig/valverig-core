//! One mono stage per channel, under one set of parameters.
//!
//! Most of what `valverig-dsp` offers is a mono primitive with settings: a
//! gate, a compressor, a tone control, a drive, a chorus, a phaser, a
//! delay. Making one of those into a module is always the same work - keep
//! one instance per channel, push the parameters into every instance before
//! each block, run them side by side - so it is written once here.
//!
//! A module says what it is by implementing [`PerChannel`]: its type id,
//! its parameters, how to build one, and where each parameter goes. Then
//! [`Channels`] is the [`Node`]. `valverig-dsp` knows nothing about any of
//! this; the trait is implemented here, on its types.

use std::sync::Arc;

use crate::Result;
use crate::node::{Node, Param, ParamSpec};

/// A mono stage that can be run once per channel.
///
/// `N` is how many parameters it has, so that [`PerChannel::apply`] can
/// take them as a fixed-size array and destructure it by name rather than
/// index into a slice.
pub trait PerChannel<const N: usize>: std::fmt::Debug + Send + Sync {
    /// The namespaced type id a preset names to ask for this stage.
    const TYPE_ID: &'static str;

    /// The parameters, in the order a host sees them. This is also the
    /// order [`PerChannel::apply`] receives them in.
    const PARAMS: [ParamSpec; N];

    /// One instance, ready to run at `sample_rate`. May allocate.
    fn make(sample_rate: f64) -> Self;

    /// Recompute whatever depends on the rate. May allocate.
    fn set_sample_rate(&mut self, sample_rate: f64);

    /// Settle this instance as the `channel`th of the set, once per
    /// [`Node::prepare`]. Nothing, for a stage where every channel is
    /// alike; a modulated stage offsets its sweep here so that a pair sways
    /// against itself. May allocate.
    fn prepared(&mut self, channel: usize) {
        let _ = channel;
    }

    /// Push the current values into this instance. Called before every
    /// block, so each setter must be cheap when nothing has changed.
    fn apply(&mut self, params: &[Arc<Param>; N]);

    /// Forget the signal, keeping every buffer. Must not allocate.
    fn reset(&mut self);

    /// Run one block of one channel. Must not allocate.
    fn process(&mut self, input: &[f32], output: &mut [f32]);
}

/// A [`PerChannel`] stage as a module: one instance per channel, one set of
/// parameters over all of them.
#[derive(Debug)]
pub struct Channels<T, const N: usize> {
    params: [Arc<Param>; N],
    inner: Vec<T>,
}

impl<T: PerChannel<N>, const N: usize> Channels<T, N> {
    /// The namespaced type id, the same one a preset names.
    pub const TYPE_ID: &'static str = T::TYPE_ID;

    /// One channel's worth, at its default settings. May allocate.
    pub fn new() -> Self {
        Self {
            params: T::PARAMS.map(Param::new),
            inner: vec![T::make(48_000.0)],
        }
    }

    fn apply(&mut self) {
        for one in &mut self.inner {
            one.apply(&self.params);
        }
    }
}

impl<T: PerChannel<N>, const N: usize> Default for Channels<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: PerChannel<N>, const N: usize> Node for Channels<T, N> {
    fn type_id(&self) -> &'static str {
        T::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, _max_block: usize, channels: usize) -> Result<()> {
        self.inner.resize_with(channels, || T::make(sample_rate));
        for (channel, one) in self.inner.iter_mut().enumerate() {
            one.set_sample_rate(sample_rate);
            one.prepared(channel);
        }
        self.apply();
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        for one in &mut self.inner {
            one.reset();
        }
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.apply();
        for ((x, y), one) in input.iter().zip(output.iter_mut()).zip(&mut self.inner) {
            one.process(x, y);
        }
    }
}
