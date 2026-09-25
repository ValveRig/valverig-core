//! A sample-rate converter for the edges of a rack that runs at one rate in
//! a host that runs at another.
//!
//! Windowed-sinc interpolation, polyphase: the impulse response of a
//! lowpass at 0.46 of the lower of the two rates, [`TAPS`] long under a
//! Kaiser window of β 8 for about 80 dB of image rejection, tabulated at
//! 256 fractional positions and read with a linear blend between
//! neighbouring positions. Each output frame is the dot product of the taps
//! at its fractional position with the input frames around it, so the
//! converter needs [`Resampler::lookahead`] frames of input past a
//! position before it can make the frame there, and nothing else: no filter
//! state, no block, no dependence on how the input arrives.
//!
//! # Pushing and pulling
//!
//! This is not a `process`-shaped stage, because the number of frames out
//! is not the number of frames in. Push input frames with
//! [`Resampler::push`], ask [`Resampler::available`] how many output frames
//! can be made from what has arrived, and make them with
//! [`Resampler::pull`]. The read position is kept as an integer frame and a
//! fraction, so it neither drifts nor loses precision however long the
//! stream runs. Nothing allocates after construction.
//!
//! A rack runs between two of these, the device's rate in and the rack's
//! rate out and back again, and hears the device a fraction of a millisecond
//! late; for a 48 kHz capture on a 44.1 kHz interface the whole detour
//! costs about 1.4 ms.

use crate::dsp::checked_sample_rate;

/// The taps of the windowed sinc.
pub const TAPS: usize = 64;

/// The fractional positions tabulated per input frame.
const PHASES: usize = 256;
/// The lowpass corner as a fraction of the lower of the two rates: the top
/// of the band, less the transition width the window needs.
const CUTOFF: f32 = 0.46;
/// The Kaiser window's shape: about 80 dB of rejection.
const BETA: f64 = 8.0;
const HALF: usize = TAPS / 2;

/// The most input frames a converter will make room for between pulls.
///
/// The ring is the next power of two at or above `4 · max_in + 4 · TAPS`,
/// so this bounds it at four million samples, sixteen megabytes, and keeps
/// that sizing far away from the point where the multiplication would
/// overflow and wrap to a zero-length buffer. Twenty seconds of audio at
/// 48 kHz, and four orders of magnitude above any host's block size.
const MAX_INPUT_FRAMES: usize = 1 << 20;

/// The modified Bessel function of the first kind, order zero.
fn i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// A converter from one rate to another.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input frames per output frame, as an integer and a fraction.
    ratio_int: usize,
    ratio_frac: f64,
    /// `PHASES + 1` rows of `TAPS`.
    table: Vec<f32>,
    ring: Vec<f32>,
    mask: usize,
    /// Frames pushed, ever.
    written: usize,
    /// The input frame the next output frame sits at or after, and how far
    /// after.
    read: usize,
    frac: f64,
}

impl Resampler {
    /// A converter from `from_hz` to `to_hz` with room for `max_in` input
    /// frames between pulls. Allocates.
    ///
    /// # Panics
    ///
    /// If either rate is outside 1 kHz to 768 kHz, which rejects a NaN, an
    /// infinity and a zero along with them, or if `max_in` is above
    /// 1,048,576 frames. Both bounds exist so that the ring buffer's size
    /// cannot be computed from a number large enough to overflow it.
    pub fn new(from_hz: f64, to_hz: f64, max_in: usize) -> Self {
        checked_sample_rate(from_hz);
        checked_sample_rate(to_hz);
        assert!(
            max_in <= MAX_INPUT_FRAMES,
            "resampler: room for {max_in} input frames asked for, at most {MAX_INPUT_FRAMES}"
        );
        let ratio = from_hz / to_hz;
        // The corner relative to the input rate: the lower rate's band.
        let corner = CUTOFF as f64 * (1.0f64).min(1.0 / ratio);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        let norm = i0(BETA);
        for phase in 0..=PHASES {
            let frac = phase as f64 / PHASES as f64;
            let row = &mut table[phase * TAPS..(phase + 1) * TAPS];
            let mut sum = 0.0f64;
            for (k, slot) in row.iter_mut().enumerate() {
                // Tap k sits at input offset `k - HALF + 1` from the frame
                // at or before the read position; the sinc is centred on
                // the position itself.
                let t = (k as f64 - (HALF as f64 - 1.0)) - frac;
                let x = 2.0 * corner * t;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let w = t / HALF as f64;
                let window = if w.abs() >= 1.0 {
                    0.0
                } else {
                    i0(BETA * (1.0 - w * w).sqrt()) / norm
                };
                let h = 2.0 * corner * sinc * window;
                sum += h;
                *slot = h as f32;
            }
            // Unity at DC whatever the phase.
            for slot in row.iter_mut() {
                *slot = (*slot as f64 / sum) as f32;
            }
        }
        let want = 4 * max_in.max(1) + 4 * TAPS;
        let size = want.next_power_of_two();
        Self {
            ratio_int: ratio.floor() as usize,
            ratio_frac: ratio - ratio.floor(),
            table,
            ring: vec![0.0; size],
            mask: size - 1,
            written: 0,
            read: 0,
            frac: 0.0,
        }
    }

    /// The look-ahead, in input frames: an output frame needs this many
    /// input frames past its own position before it can be made.
    ///
    /// This is not latency in the sense the `process`-shaped stages use the
    /// word. An output frame carries the input from the instant it sits at,
    /// not from 32 frames earlier, so a round trip out and back comes home
    /// on its own frames. What the look-ahead costs is *when* a frame can be
    /// made, which is what a host budgets for.
    pub fn lookahead(&self) -> usize {
        HALF
    }

    /// Forget everything: the input, the read position and the fraction.
    /// Does not allocate.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.written = 0;
        self.read = 0;
        self.frac = 0.0;
    }

    /// How many more input frames fit before a pull is needed.
    pub fn room(&self) -> usize {
        // Frames behind the read position must stay for the taps behind it.
        let keep = self.read.saturating_sub(HALF);
        (self.ring.len() - HALF).saturating_sub(self.written - keep)
    }

    /// Feed input frames.
    ///
    /// # Panics
    ///
    /// If they do not fit: pull first, and see [`Resampler::room`].
    pub fn push(&mut self, input: &[f32]) {
        assert!(
            input.len() <= self.room(),
            "resampler: {} frames pushed with room for {}",
            input.len(),
            self.room()
        );
        for &x in input {
            self.ring[self.written & self.mask] = x;
            self.written += 1;
        }
    }

    /// Push this many frames of silence: what a converter on a rack's
    /// output is started with, so that a pull for a block always has one.
    ///
    /// # Panics
    ///
    /// If they do not fit; see [`Resampler::room`].
    pub fn prime(&mut self, frames: usize) {
        assert!(
            frames <= self.room(),
            "resampler: no room to prime {frames} frames"
        );
        for _ in 0..frames {
            self.ring[self.written & self.mask] = 0.0;
            self.written += 1;
        }
    }

    #[inline]
    fn can_read(&self, read: usize) -> bool {
        read + HALF < self.written
    }

    /// How many output frames can be made from what has arrived.
    pub fn available(&self) -> usize {
        let (mut read, mut frac, mut n) = (self.read, self.frac, 0usize);
        while self.can_read(read) {
            n += 1;
            frac += self.ratio_frac;
            read += self.ratio_int;
            if frac >= 1.0 {
                frac -= 1.0;
                read += 1;
            }
        }
        n
    }

    /// Make output frames into `output`, as many as it holds or as many as
    /// can be made, whichever is fewer, and say how many.
    /// Allocation-free.
    pub fn pull(&mut self, output: &mut [f32]) -> usize {
        let mut made = 0;
        for o in output.iter_mut() {
            if !self.can_read(self.read) {
                break;
            }
            let p = self.frac * PHASES as f64;
            let row = p as usize;
            let blend = (p - row as f64) as f32;
            let (a, b) = (
                &self.table[row * TAPS..(row + 1) * TAPS],
                &self.table[(row + 1) * TAPS..(row + 2) * TAPS],
            );
            let start = self.read + 1 + self.ring.len() - HALF;
            let mut acc = 0.0f32;
            for k in 0..TAPS {
                let x = self.ring[(start + k) & self.mask];
                acc += x * (a[k] + blend * (b[k] - a[k]));
            }
            *o = acc;
            made += 1;
            self.frac += self.ratio_frac;
            self.read += self.ratio_int;
            if self.frac >= 1.0 {
                self.frac -= 1.0;
                self.read += 1;
            }
        }
        made
    }
}

/// Resample a whole signal at once, from `from_hz` to `to_hz`.
///
/// For a signal that is already in memory and is converted off the audio
/// thread: an impulse response recorded at one rate and played at another.
/// [`Resampler`] is the streaming form, for a callback; this is the same
/// filter with the bookkeeping done for you.
///
/// The output is aligned with the input: frame `n` out carries the input at
/// `n * to_hz / from_hz`, and the length is what that ratio implies. An
/// empty input gives an empty output, and equal rates give the input back.
/// Allocates.
///
/// # Panics
///
/// If either rate is outside 1 kHz to 768 kHz, as [`Resampler::new`] does.
pub fn resample(input: &[f32], from_hz: f64, to_hz: f64) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if from_hz == to_hz {
        return input.to_vec();
    }
    let want = (input.len() as f64 * to_hz / from_hz).round() as usize;
    let room = MAX_INPUT_FRAMES.min(input.len().next_power_of_two().max(2 * TAPS));
    let mut resampler = Resampler::new(from_hz, to_hz, room);
    let mut out: Vec<f32> = Vec::with_capacity(want + TAPS);
    let mut scratch = vec![0.0f32; 1024];
    // Silence after the input so that the last output frame, which needs
    // input past its own position, can be made.
    let tail = vec![0.0f32; resampler.lookahead() + 1];
    for block in [input, &tail[..]] {
        let mut rest = block;
        loop {
            let take = rest.len().min(resampler.room());
            if take > 0 {
                resampler.push(&rest[..take]);
                rest = &rest[take..];
            }
            let made = resampler.pull(&mut scratch);
            out.extend_from_slice(&scratch[..made]);
            if rest.is_empty() {
                break;
            }
            if take == 0 && made == 0 {
                break;
            }
        }
    }
    loop {
        let made = resampler.pull(&mut scratch);
        if made == 0 {
            break;
        }
        out.extend_from_slice(&scratch[..made]);
    }
    out.resize(want, 0.0);
    out
}

#[cfg(test)]
mod one_shot_tests {
    use super::*;

    /// A sine resampled to another rate is the same sine, sampled there.
    #[test]
    fn a_sine_comes_out_at_the_new_rate() {
        for (from, to) in [
            (44_100.0, 48_000.0),
            (48_000.0, 44_100.0),
            (96_000.0, 48_000.0),
        ] {
            let hz = 440.0;
            let n = (from as usize) / 2;
            let input: Vec<f32> = (0..n)
                .map(|i| (i as f64 / from * 2.0 * std::f64::consts::PI * hz).sin() as f32)
                .collect();
            let out = resample(&input, from, to);
            let want = (n as f64 * to / from).round() as usize;
            assert_eq!(out.len(), want, "{from} -> {to}");
            // Past the filter's edges, every sample is the sine at that
            // instant: the conversion keeps the signal in place.
            let mut worst = 0.0f32;
            for (i, v) in out.iter().enumerate().take(want - 200).skip(200) {
                let expected = (i as f64 / to * 2.0 * std::f64::consts::PI * hz).sin() as f32;
                worst = worst.max((v - expected).abs());
            }
            assert!(worst < 0.02, "{from} -> {to}: worst error {worst}");
        }
    }

    #[test]
    fn equal_rates_and_an_empty_signal_are_answered_without_filtering() {
        let x: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        assert_eq!(
            resample(&x, 48_000.0, 48_000.0),
            x,
            "equal rates pass through"
        );
        assert!(resample(&[], 44_100.0, 48_000.0).is_empty());
    }

    #[test]
    fn a_signal_longer_than_the_ring_is_converted_whole() {
        // Longer than any one push can hold, so the loop has to take turns.
        let n = 300_000;
        let input: Vec<f32> = (0..n).map(|i| ((i % 97) as f32 / 97.0) - 0.5).collect();
        let out = resample(&input, 96_000.0, 48_000.0);
        assert_eq!(out.len(), n / 2);
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(out.iter().any(|v| v.abs() > 0.1), "not silence");
    }
}
