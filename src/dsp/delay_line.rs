//! Two ring buffers: one read a fractional distance behind the write point,
//! one a whole number of samples behind it.
//!
//! Neither cuts a tail in what is written to it. A line's contents are the
//! signal, and which of them are worth rounding to zero depends on the
//! stage: the reverb ends the tail in its eight feedback lines but not in
//! the pre-delay or the diffusion allpasses, which see the input once and
//! pass it on. Callers that want the floor apply
//! [`crate::dsp::tails::end_line`] themselves, where it is visible.

/// A ring read a fractional distance behind the newest sample.
#[derive(Debug, Clone)]
pub(crate) struct Interpolated {
    buf: Vec<f32>,
    pos: usize,
}

impl Interpolated {
    /// A silent line of `len` samples, at least four so the interpolator
    /// always has the two samples either side of its read position.
    /// Allocates.
    pub fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(4)],
            pos: 0,
        }
    }

    /// The furthest distance [`Interpolated::read`] accepts, in samples:
    /// three short of the buffer, leaving room for the interpolator's two
    /// samples ahead of the read position and one behind it.
    pub fn max_read(&self) -> f32 {
        (self.buf.len() - 3) as f32
    }

    /// Write `x` as the newest sample, dropping the oldest.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
    }

    /// The sample exactly `d` frames behind the newest, `d` from `0` to
    /// `len - 1`.
    #[inline]
    fn at(&self, d: usize) -> f32 {
        let len = self.buf.len();
        self.buf[(self.pos + len - 1 - d) % len]
    }

    /// The signal `d` frames behind the newest by four-point Hermite
    /// interpolation between the samples either side: exact at an integer
    /// distance, and smooth as the distance sweeps.
    ///
    /// # Panics
    ///
    /// `d` must lie between `1.0` and [`Interpolated::max_read`]; the
    /// interpolator reads one sample further back than `d` and two further
    /// forward, and a `d` under `1.0` would run off the front. Debug builds
    /// catch it here; release builds read the wrong sample. Every caller in
    /// this crate clamps to that range before calling.
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        debug_assert!(
            d >= 1.0 && d <= self.max_read(),
            "read distance {d} outside 1.0..={}",
            self.max_read()
        );
        let i = d as usize;
        let t = d - i as f32;
        let x0 = self.at(i);
        if t == 0.0 {
            return x0;
        }
        let (xm1, x1, x2) = (self.at(i - 1), self.at(i + 1), self.at(i + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// Silence, the write point back at the start. Does not allocate.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// A ring read a whole number of samples behind the write point.
#[derive(Debug, Clone)]
pub(crate) struct Fixed {
    buf: Vec<f32>,
    pos: usize,
}

impl Fixed {
    /// A silent line of `len` samples, at least one. Allocates.
    pub fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
        }
    }

    /// The line's length in samples, which is also the delay
    /// [`Fixed::peek`] and [`Fixed::push`] give together.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// The sample about to leave the line: the one written `len` frames
    /// ago.
    #[inline]
    pub fn peek(&self) -> f32 {
        self.buf[self.pos]
    }

    /// Put `x` where the leaving sample was and step on, so a
    /// [`Fixed::peek`] then [`Fixed::push`] pair is a delay of exactly
    /// [`Fixed::len`] samples.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
    }

    /// Write `x` as the newest sample and read the one `behind` frames
    /// before it, in one pass over the ring: a delay whose length is a
    /// setting rather than the buffer's size.
    ///
    /// # Panics
    ///
    /// `behind` of zero returns `x` itself; `behind` must be at most
    /// `len - 1`, which is what the ring can look back over. Debug builds
    /// catch a larger one here.
    #[inline]
    pub fn push_and_read(&mut self, x: f32, behind: usize) -> f32 {
        let len = self.buf.len();
        debug_assert!(behind < len, "read {behind} frames behind a line of {len}");
        self.buf[self.pos] = x;
        let read = (self.pos + len - behind) % len;
        let y = self.buf[read];
        self.pos = (self.pos + 1) % len;
        y
    }

    /// Silence, the write point back at the start. Does not allocate.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}
