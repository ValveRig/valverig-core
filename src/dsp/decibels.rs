//! Decibels to the factor a sample is multiplied by.

/// The linear factor for `db` decibels: `10^(db/20)`.
///
/// `0` is unity, `-6` is close to a half, `-100` is 1e-5, and negative
/// infinity is zero. A non-finite `db` other than negative infinity gives a
/// non-finite factor: callers validate their settings, this does not.
#[inline]
pub(crate) fn to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
