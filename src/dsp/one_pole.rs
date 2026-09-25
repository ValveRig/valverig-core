//! One-pole coefficients, and one step of a smoothed parameter.
//!
//! Every stage here smooths a moving setting, follows a level or rolls off
//! the highs with the same first-order filter, and this is the arithmetic
//! they share.
//!
//! # Two spellings, on purpose
//!
//! A one-pole is written two ways in this crate, and they are not
//! interchangeable at the level of the last bit:
//!
//! - the **increment** form, `now += k · (target − now)`, whose coefficient
//!   comes from [`from_time`] or [`from_corner`]. Nine stages use it;
//! - the **retention** form, `y = a · y + (1 − a) · target`, whose
//!   coefficient comes from [`retention`]. The compressor uses it, because
//!   that is how the dynamics literature writes ballistics and how its
//!   attack and release times are defined.
//!
//! `a` and `k` describe the same time constant and satisfy `a = 1 − k` in
//! exact arithmetic, but in `f32` the two orderings round differently, so
//! each stage keeps the form its numbers were measured against.

/// The increment-form coefficient that covers 63% of a step in
/// `milliseconds` at `sample_rate`, for a smoother written
/// `now += coefficient · (target − now)`.
///
/// Zero or less gives `1`: no smoothing, the target reached on the first
/// sample.
pub(crate) fn from_time(milliseconds: f32, sample_rate: f32) -> f32 {
    if milliseconds <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (milliseconds * 1e-3 * sample_rate)).exp()
    }
}

/// The increment-form coefficient of a lowpass whose corner is at `hz`, for
/// a filter written `state += coefficient · (x − state)`.
///
/// At a corner far below the sample rate this is close to `2π · hz / sr`;
/// at or above the Nyquist it saturates towards `1`, which passes the input
/// through, so the caller decides whether a corner that high means "no
/// filter" rather than relying on the coefficient to say so.
pub(crate) fn from_corner(hz: f32, sample_rate: f32) -> f32 {
    1.0 - (-2.0 * std::f32::consts::PI * hz / sample_rate).exp()
}

/// The retention-form coefficient for a time constant of `milliseconds` at
/// `sample_rate`, for a smoother written `y = a · y + (1 − a) · target`.
///
/// Zero or less gives `0`: no smoothing, the target reached on the first
/// sample.
pub(crate) fn retention(milliseconds: f32, sample_rate: f32) -> f32 {
    if milliseconds <= 0.0 {
        0.0
    } else {
        (-1.0 / (milliseconds * 1e-3 * sample_rate)).exp()
    }
}

/// One increment-form step of `now` towards `target`, landing exactly on it
/// once the two are within `snap`.
///
/// The snap matters more than it looks: a one-pole in `f32` stalls a few
/// ULPs short of its target once each step rounds to nothing, and how short
/// depends on the coefficient. Without it a parameter that has visibly
/// finished moving would still not be equal to its setting, and the stages
/// that copy their input through bit for bit at a particular setting, a mix
/// of zero or a flat tone control, would never reach it.
#[inline]
pub(crate) fn step(now: f32, target: f32, coefficient: f32, snap: f32) -> f32 {
    let next = now + coefficient * (target - now);
    if (target - next).abs() < snap {
        target
    } else {
        next
    }
}
