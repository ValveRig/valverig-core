//! Ending a decaying tail instead of letting it run for ever.
//!
//! A peak detector, a filter's ringing and a feedback delay line all decay
//! geometrically towards zero without arriving. Left alone, a reverb set to
//! a two-second decay is still producing non-zero samples twenty-three
//! seconds later, and a tone control returned to flat still has a fraction
//! of a signal in its state, so it cannot take the bit-exact copy-through
//! path that [`crate::eq::Mono`] is built around. So each stage cuts
//! its own tail once what is left cannot affect the output.
//!
//! # These floors are not about denormals
//!
//! The subnormal range begins around 1.18e-38, and arithmetic there costs
//! one to two orders of magnitude more per operation on some processors.
//! That is a real hazard, and it is **not what this module addresses**: it
//! belongs to whoever owns the audio thread, because flush-to-zero is a
//! setting on the thread's floating-point control register rather than
//! anything a stage can do to its own state. [`crate::dsp::denormals`] is how
//! that setting is reached, and says who sets it and for how long.
//!
//! The floors below sit eighteen and twenty-three decades *above* the
//! subnormal boundary, and that gap is the point. They end a tail while it
//! is still an ordinary float, seconds before a control register would have
//! anything to say about it, and they do it on any thread at all, including
//! an offline render, a test, or a consumer that never arms flush-to-zero
//! at all, because they are plain arithmetic rather than a machine mode.

/// Filter and detector state below this magnitude becomes zero.
///
/// State this small contributes under one part in 10^15 of a full-scale
/// sample, so cutting it there ends the tail rather than altering one.
pub(crate) const FILTER_FLOOR: f32 = 1e-20;

/// A sample written into a delay line below this magnitude becomes zero.
///
/// Higher than [`FILTER_FLOOR`] because what goes into a line is read back
/// and re-multiplied on every pass round a feedback loop, so a line is
/// reached sooner and more often than a filter's state.
pub(crate) const LINE_FLOOR: f32 = 1e-15;

/// `x`, or zero once a filter or detector's tail has run out.
#[inline]
pub(crate) fn end(x: f32) -> f32 {
    if x.abs() < FILTER_FLOOR { 0.0 } else { x }
}

/// `x`, or zero once it is too small to be worth putting back into a delay
/// line.
#[inline]
pub(crate) fn end_line(x: f32) -> f32 {
    if x.abs() < LINE_FLOOR { 0.0 } else { x }
}
