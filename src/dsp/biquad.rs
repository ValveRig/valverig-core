//! One second-order section, in transposed direct form II.
//!
//! Transposed direct form II because it holds only two state values for a
//! second-order response and, unlike the direct form, keeps that state at
//! the filter's *output* scale rather than at an internal one, which is
//! what keeps a `f32` section well behaved at the low corner frequencies a
//! bass shelf asks for.

use crate::dsp::tails;

/// A second-order section: `y = b0·x + z1`, with the state carried forward.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Set the six coefficients of `b0 + b1·z⁻¹ + b2·z⁻² over a0 + a1·z⁻¹ +
    /// a2·z⁻²`, normalising by `a0` as the RBJ Audio EQ Cookbook's formulae
    /// leave it. The state is untouched, so a moving knob does not click.
    pub fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// A cookbook lowpass with its corner at `fraction` of the sample rate
    /// (so `0.25` is a quarter of the rate) and the given `q`.
    ///
    /// `fraction` must be under `0.5`; at the Nyquist the bilinear
    /// transform's `tan` diverges and the coefficients are meaningless.
    pub fn lowpass(fraction: f32, q: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * fraction;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let mut filter = Self::default();
        filter.set(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        );
        filter
    }

    /// A cookbook highpass with its corner at `fraction` of the sample rate
    /// and the given `q`, the mirror of [`Biquad::lowpass`].
    ///
    /// `fraction` must be under `0.5`, for the same reason.
    pub fn highpass(fraction: f32, q: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * fraction;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let mut filter = Self::default();
        filter.set(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        );
        filter
    }

    /// One sample through the section.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Cut the section's ringing once it is too small to affect the output,
    /// so that a filter left with no input reaches silence, and at a setting
    /// where the section is an identity reaches it in time to take the
    /// copy-through path. Cheap enough to call once per block; see
    /// [`crate::dsp::tails`].
    pub fn end_tail(&mut self) {
        self.z1 = tails::end(self.z1);
        self.z2 = tails::end(self.z2);
    }

    /// Forget the signal, keeping the coefficients.
    pub fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Whether this section would pass a signal through unchanged, bit for
    /// bit: the numerator equal to the denominator, and no signal in the
    /// state to come out first.
    pub fn is_identity(&self) -> bool {
        self.b0 == 1.0
            && self.b1 == self.a1
            && self.b2 == self.a2
            && self.z1 == 0.0
            && self.z2 == 0.0
    }
}
