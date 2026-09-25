//! Zero-latency partitioned convolution, for cabinet impulse responses.
//!
//! # The shape of the problem
//!
//! A cabinet IR is 20 to 500 ms long: 1,000 to 24,000 taps at 48 kHz. Direct
//! convolution costs one multiply-add per tap per sample, which is 1.2
//! GMAC/s at the long end and comparable to the neural model itself, so the
//! bulk of the work has to go through the FFT. Plain FFT convolution, though, needs a whole
//! block of input before it can produce any output, and a block of latency is
//! exactly what a guitarist notices.
//!
//! # How it works
//!
//! The IR is cut into partitions of `P` taps. The first partition is applied
//! in the time domain, straight from the input history, so every output
//! sample is available the moment its input sample is. Every later partition
//! is applied in the frequency domain by uniformly partitioned overlap-save:
//! once per `P` input samples the newest `2P` samples are transformed, the
//! spectrum is pushed onto a frequency-domain delay line, and the sum of
//! `H_k · X_{m-k}` over the tail partitions is transformed back to give the
//! next `P` output samples of the tail. Those depend only on input that is
//! already complete, so they are ready before they are needed.
//!
//! Output for a sample is therefore `head + tail`: a `P`-tap direct form plus
//! one pre-computed value. Latency is zero for any block size, and the output
//! is bit-identical however the input is split into blocks, because both
//! halves are computed on the absolute sample count rather than on block
//! boundaries. Nothing about the convolver depends on a block ceiling, so it
//! does not ask for one.
//!
//! # Cost
//!
//! Per sample, about `2P` flops for the direct head and `4L/P` for the
//! frequency-domain sum over an `L`-tap IR, plus two real FFTs of size `2P`
//! per `P` samples. [`partition_for`] picks `P` from the IR length to balance
//! the two.
//!
//! Multiples of real time at 48 kHz on one Apple M3 Max core, median of five
//! runs of two seconds in blocks of 64; `*` is what [`Convolver::new`] picks:
//!
//! | IR taps | P=32 | P=64 | P=128 | P=256 | P=512 |
//! |---:|---:|---:|---:|---:|---:|
//! | 512 | **1619*** | 1403 | 966 | 552 | 302 |
//! | 2,048 | 737 | **990*** | 807 | 489 | 277 |
//! | 8,192 | 233 | 459 | **572*** | 437 | 267 |
//! | 24,000 | 84 | 189 | 315 | **335*** | 241 |
//! | 96,000 | 21 | 51 | 107 | **158*** | 173 |
//!
//! The marked column is fastest at every length but the last, and blocks of
//! 512 measure within noise of blocks of 64. At 96,000 taps, two seconds, the
//! cap at 256 gives up about a tenth of the speed of 512 for a bound on the
//! work done at any one partition boundary: one transform of 512 plus the
//! tail sum, a few percent of a 32-frame block's deadline.
//!
//! # Accuracy
//!
//! Single-precision transforms. Against a double-precision direct convolution
//! the worst error over every partition-relevant IR length is 3.9e-7 of the
//! signal peak; `tests/convolution.rs` measures it and holds it under 1e-5.
//! The inverse transform's `1/N` scale is folded into the partition spectra
//! at construction, so the process path does no scaling.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::fmt;
use std::sync::Arc;

/// Smallest partition [`partition_for`] will choose. Below this the
/// transforms are too short to pay for their own bookkeeping, and the direct
/// head is trivially cheap anyway.
const MIN_PARTITION: usize = 16;

/// Largest partition [`partition_for`] will choose. Bounds the direct head at
/// 256 taps and the work done at a partition boundary at one transform of 512
/// plus the tail sum, however long the IR: under 32-sample blocks that burst
/// is a few percent of one block's deadline.
const MAX_PARTITION: usize = 256;

/// The largest partition [`Convolver::with_partition`] accepts.
///
/// Far above the range [`partition_for`] chooses from, because that method
/// exists to measure the choice and so has to be able to try partitions the
/// policy rejects. The limit is here only so that a partition size cannot
/// turn into an allocation of an arbitrary size: at this bound the buffers a
/// convolver holds come to a few megabytes.
const PARTITION_LIMIT: usize = 1 << 16;

/// The partition size a convolver uses for an IR of `ir_len` taps: the power
/// of two at or above `sqrt(2 · ir_len)`, clamped to `16..=256`.
///
/// Per sample the direct head costs about `2P` flops and the tail about
/// `4L / P`, so the two balance near `P = sqrt(2L)`. Measured over IRs from
/// 512 to 96,000 taps, the fastest partition is that power of two for every
/// length, and the choice does not depend on the block size: the
/// frequency-domain work is done once per `P` samples wherever the block
/// boundaries fall. `partition_sweep` in `tests/benchmark.rs` is the
/// measurement.
pub fn partition_for(ir_len: usize) -> usize {
    (2 * ir_len)
        .isqrt()
        .next_power_of_two()
        .clamp(MIN_PARTITION, MAX_PARTITION)
}

/// A mono convolver with a fixed impulse response and no latency.
///
/// Built once, off the audio thread, from the IR; [`Convolver::process`] then
/// allocates nothing and takes blocks of any length. To change the IR, build
/// a new one and swap it in.
#[derive(Clone)]
pub struct Convolver {
    p: usize,
    ir_len: usize,
    /// Taps `0..P`, reversed, so the direct head is a forward dot product.
    head_rev: Vec<f32>,
    /// One spectrum of `P + 1` bins per tail partition (taps `P..`), each
    /// scaled by `1 / 2P` so the inverse transform needs no scaling. Real and
    /// imaginary parts in separate planes, so the tail sum vectorises as plain
    /// lanes of multiplies and adds rather than as interleaved pairs.
    parts_re: Vec<f32>,
    parts_im: Vec<f32>,
    nparts: usize,
    /// The frequency-domain delay line: the last `nparts` input spectra, a
    /// ring whose newest entry is at `fdl_pos`, in the same split layout.
    fdl_re: Vec<f32>,
    fdl_im: Vec<f32>,
    fdl_pos: usize,
    /// The last `2P` input samples, a ring written at `ring_pos`.
    ring: Vec<f32>,
    ring_pos: usize,
    /// Samples into the current partition, `0..P`.
    phase: usize,
    /// The tail's contribution to the `P` output samples of the current
    /// partition, computed at the boundary before it.
    pending: Vec<f32>,
    // Scratch, sized once.
    window: Vec<f32>,
    fft_in: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
    acc: Vec<Complex<f32>>,
    fft_out: Vec<f32>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
}

impl Convolver {
    /// Build for `ir`, at the partition size [`partition_for`] picks.
    /// Allocates.
    ///
    /// The IR is taken as samples, not as a path, and at whatever rate it
    /// was recorded at: a convolver has no sample rate of its own and cannot
    /// tell that an IR does not match the signal. A host that cares checks
    /// that before building one.
    ///
    /// An empty IR is a valid convolver whose output is silence.
    pub fn new(ir: &[f32]) -> Self {
        Self::with_partition(ir, partition_for(ir.len()))
    }

    /// [`Convolver::new`] with the partition size chosen by the caller. For
    /// measuring the choice; the default is [`partition_for`].
    ///
    /// # Panics
    ///
    /// If `p` is not a power of two between 16 and 65,536.
    pub fn with_partition(ir: &[f32], p: usize) -> Self {
        assert!(
            p.is_power_of_two() && (MIN_PARTITION..=PARTITION_LIMIT).contains(&p),
            "partition must be a power of two between {MIN_PARTITION} and {PARTITION_LIMIT}, got {p}"
        );
        let n = 2 * p;
        let bins = p + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);

        let mut head_rev = vec![0.0f32; p];
        for (i, &h) in ir.iter().take(p).enumerate() {
            head_rev[p - 1 - i] = h;
        }

        let nparts = ir.len().saturating_sub(p).div_ceil(p);
        let mut parts_re = vec![0.0f32; nparts * bins];
        let mut parts_im = vec![0.0f32; nparts * bins];
        let mut fft_in = r2c.make_input_vec();
        let mut spectrum = r2c.make_output_vec();
        let mut scratch_fwd = r2c.make_scratch_vec();
        let scale = 1.0 / n as f32;
        for k in 0..nparts {
            let start = p * (k + 1);
            let chunk = &ir[start..(start + p).min(ir.len())];
            fft_in.fill(0.0);
            fft_in[..chunk.len()].copy_from_slice(chunk);
            r2c.process_with_scratch(&mut fft_in, &mut spectrum, &mut scratch_fwd)
                .expect("planned for this length");
            for (b, c) in spectrum.iter().enumerate() {
                parts_re[k * bins + b] = c.re * scale;
                parts_im[k * bins + b] = c.im * scale;
            }
        }
        let scratch_inv = c2r.make_scratch_vec();

        Self {
            p,
            ir_len: ir.len(),
            head_rev,
            parts_re,
            parts_im,
            nparts,
            fdl_re: vec![0.0; nparts * bins],
            fdl_im: vec![0.0; nparts * bins],
            fdl_pos: 0,
            ring: vec![0.0; n],
            ring_pos: 0,
            phase: 0,
            pending: vec![0.0; p],
            window: vec![0.0; n],
            fft_in,
            spectrum,
            acc_re: vec![0.0; bins],
            acc_im: vec![0.0; bins],
            acc: vec![Complex::new(0.0, 0.0); bins],
            fft_out: vec![0.0; n],
            scratch_fwd,
            scratch_inv,
            r2c,
            c2r,
        }
    }

    /// Taps in the impulse response.
    pub fn ir_len(&self) -> usize {
        self.ir_len
    }

    /// The partition size in use; see [`partition_for`].
    pub fn partition(&self) -> usize {
        self.p
    }

    /// Delay the convolver adds to the signal: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// Forget all input, keeping the IR and every buffer. Does not allocate.
    pub fn reset(&mut self) {
        self.fdl_re.fill(0.0);
        self.fdl_im.fill(0.0);
        self.fdl_pos = 0;
        self.ring.fill(0.0);
        self.ring_pos = 0;
        self.phase = 0;
        self.pending.fill(0.0);
    }

    /// Convolve `input` into `output`, sample for sample. Allocation-free.
    ///
    /// Any length, in any sequence of calls; the output does not depend on how
    /// frames are grouped.
    ///
    /// # Panics
    ///
    /// If `input` and `output` are different lengths.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        assert_eq!(
            input.len(),
            output.len(),
            "input and output must be the same length"
        );
        let p = self.p;
        let mut done = 0;
        while done < input.len() {
            // Up to the next partition boundary, so that the boundaries fall
            // on the absolute sample count whatever the block size is.
            let seg = (input.len() - done).min(p - self.phase);
            let x = &input[done..done + seg];
            let y = &mut output[done..done + seg];
            self.push(x);
            self.head(y);
            for (o, tail) in y
                .iter_mut()
                .zip(&self.pending[self.phase..self.phase + seg])
            {
                *o += *tail;
            }
            self.phase += seg;
            done += seg;
            if self.phase == p {
                self.phase = 0;
                self.advance_tail();
            }
        }
    }

    /// Append a segment of at most `P` samples to the input ring.
    fn push(&mut self, x: &[f32]) {
        let n = self.ring.len();
        let first = (n - self.ring_pos).min(x.len());
        self.ring[self.ring_pos..self.ring_pos + first].copy_from_slice(&x[..first]);
        self.ring[..x.len() - first].copy_from_slice(&x[first..]);
        self.ring_pos = (self.ring_pos + x.len()) % n;
    }

    /// The direct head over the segment just pushed:
    /// `y[i] = Σ_{k < P} h[k] · x[t_i - k]`.
    fn head(&mut self, y: &mut [f32]) {
        let p = self.p;
        let n = self.ring.len();
        // The window is the last `seg + P - 1` samples, ending at the newest.
        let len = y.len() + p - 1;
        let start = (self.ring_pos + n - len) % n;
        let first = (n - start).min(len);
        self.window[..first].copy_from_slice(&self.ring[start..start + first]);
        self.window[first..len].copy_from_slice(&self.ring[..len - first]);

        let h = &self.head_rev;
        let w = &self.window[..len];
        for (i, out) in y.iter_mut().enumerate() {
            // Eight independent lanes, summed in a fixed order at the end:
            // vectorisable, and the same association for every sample, so
            // the result cannot depend on where the block boundaries fell.
            // `as_chunks` drops a remainder, and there is never one: the
            // constructor accepts only a power-of-two partition of 16 or
            // more, so `p` is always a multiple of 8.
            let mut acc = [0.0f32; 8];
            let (hc, _) = h.as_chunks::<8>();
            let (wc, _) = w[i..i + p].as_chunks::<8>();
            for (hc, wc) in hc.iter().zip(wc) {
                for l in 0..8 {
                    acc[l] += hc[l] * wc[l];
                }
            }
            *out =
                ((acc[0] + acc[1]) + (acc[2] + acc[3])) + ((acc[4] + acc[5]) + (acc[6] + acc[7]));
        }
    }

    /// At a partition boundary: transform the newest `2P` samples, push the
    /// spectrum, and compute the tail for the next `P` output samples.
    fn advance_tail(&mut self) {
        if self.nparts == 0 {
            return;
        }
        let p = self.p;
        let bins = p + 1;
        // At a boundary `ring_pos` is 0 or P; in chronological order the ring
        // is the part from `ring_pos` to the end, then the part before it.
        let (newer, older) = self.ring.split_at(self.ring_pos);
        self.fft_in[..older.len()].copy_from_slice(older);
        self.fft_in[older.len()..].copy_from_slice(newer);
        self.r2c
            .process_with_scratch(&mut self.fft_in, &mut self.spectrum, &mut self.scratch_fwd)
            .expect("planned for this length");

        self.fdl_pos = (self.fdl_pos + 1) % self.nparts;
        let base = self.fdl_pos * bins;
        for (b, c) in self.spectrum.iter().enumerate() {
            self.fdl_re[base + b] = c.re;
            self.fdl_im[base + b] = c.im;
        }

        // acc = Σ_j H_j · X_j: partition j against the spectrum j boundaries
        // old, newest first. Every slice is cut to exactly `bins`, so the
        // compiler can drop the bounds checks and run the bins as lanes.
        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
        for j in 0..self.nparts {
            let slot = (self.fdl_pos + self.nparts - j) % self.nparts;
            let hr = &self.parts_re[j * bins..(j + 1) * bins];
            let hi = &self.parts_im[j * bins..(j + 1) * bins];
            let xr = &self.fdl_re[slot * bins..(slot + 1) * bins];
            let xi = &self.fdl_im[slot * bins..(slot + 1) * bins];
            let ar = &mut self.acc_re[..bins];
            let ai = &mut self.acc_im[..bins];
            for b in 0..bins {
                ar[b] += hr[b] * xr[b] - hi[b] * xi[b];
                ai[b] += hr[b] * xi[b] + hi[b] * xr[b];
            }
        }
        for (b, c) in self.acc.iter_mut().enumerate() {
            *c = Complex::new(self.acc_re[b], self.acc_im[b]);
        }
        // A real signal's spectrum has real DC and Nyquist bins, and the
        // products keep them exactly real; the inverse transform checks and
        // would refuse a stray sign bit, so say so explicitly.
        self.acc[0].im = 0.0;
        self.acc[p].im = 0.0;
        self.c2r
            .process_with_scratch(&mut self.acc, &mut self.fft_out, &mut self.scratch_inv)
            .expect("planned for this length");
        // Overlap-save: the second half of the result is the valid part.
        self.pending.copy_from_slice(&self.fft_out[p..]);
    }
}

impl fmt::Debug for Convolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Convolver")
            .field("ir_len", &self.ir_len)
            .field("partition", &self.p)
            .field("tail_partitions", &self.nparts)
            .finish_non_exhaustive()
    }
}
