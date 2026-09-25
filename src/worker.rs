//! The handshake that lets a lane run on a thread of its own.
//!
//! A lane is owned by exactly one thread at a time, and `state` is what
//! hands it across:
//!
//! ```text
//!   IDLE ──(rack publishes src, n; stores GO)──▶ GO
//!     ▲                                           │
//!     │                                     worker runs the lane
//!     └──(rack takes the result, stores IDLE)── DONE
//! ```
//!
//! The rack writes the input pointers and the frame count *before* storing
//! `GO` with release order, and the worker reads them *after* seeing `GO`
//! with acquire order, so both are published. The worker stores `DONE` with
//! release order once the lane's result is in place, and the rack acquires
//! it before touching the lane again. Between `GO` and `DONE` the lane
//! belongs to the worker and to nobody else; at every other moment it
//! belongs to whichever thread holds the rack, which is `&mut` and so
//! unique.
//!
//! `QUIT` ends a worker. The rack stores it, unparks the thread, and joins.
//!
//! Nothing here allocates, and nothing here blocks on a lock: a worker
//! spins for a short while waiting for the next block, then parks, and an
//! `unpark` that arrives before the park is remembered by the token, so no
//! wake-up is lost.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::thread;

use crate::dsp::denormals;
use crate::rack::{Lane, ThreadHook};

/// Nobody is running the lane.
pub(crate) const IDLE: u8 = 0;
/// The worker may take the lane and run the published block.
pub(crate) const GO: u8 = 1;
/// The worker has finished; the result is where `in_a` says.
pub(crate) const DONE: u8 = 2;
/// The worker should return.
pub(crate) const QUIT: u8 = 3;

/// How many times a worker spins for the next block before parking. Long
/// enough to cover the gap between two blocks of a busy rack, short enough
/// not to burn a core while the stream is idle.
const SPINS: u32 = 2000;

/// The host's hook, if any; a newtype so the rack can print itself.
#[derive(Clone, Default)]
pub(crate) struct Hook(pub(crate) Option<ThreadHook>);

impl std::fmt::Debug for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Hook(some)"
        } else {
            "Hook(none)"
        })
    }
}

/// A lane, and the state that says who may touch it.
#[derive(Debug)]
pub(crate) struct Shared {
    lane: UnsafeCell<Lane>,
    pub(crate) state: AtomicU8,
    pub(crate) src: [AtomicPtr<f32>; 2],
    pub(crate) n: AtomicUsize,
    pub(crate) in_a: AtomicBool,
    /// The lane's own delay in frames, published at `prepare`, so that the
    /// rack can report its latency without reaching into the lane.
    pub(crate) latency: AtomicUsize,
}

// SAFETY: `lane` is reached only through `Shared::lane`, whose contract is
// the handshake described above; every other field is atomic.
unsafe impl Sync for Shared {}

impl Shared {
    pub(crate) fn new(lane: Lane) -> Arc<Self> {
        Arc::new(Self {
            lane: UnsafeCell::new(lane),
            state: AtomicU8::new(IDLE),
            src: [
                AtomicPtr::new(std::ptr::null_mut()),
                AtomicPtr::new(std::ptr::null_mut()),
            ],
            n: AtomicUsize::new(0),
            in_a: AtomicBool::new(true),
            latency: AtomicUsize::new(0),
        })
    }

    /// The lane, for the worker that has taken it.
    ///
    /// # Safety
    ///
    /// The caller must be the worker thread, between seeing `GO` and
    /// storing `DONE`. That window is the only time a worker owns the lane.
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn lane_running(&self) -> &mut Lane {
        // SAFETY: the caller states it is the worker inside its window.
        unsafe { &mut *self.lane.get() }
    }

    /// The lane, for the thread holding the rack.
    ///
    /// # Safety
    ///
    /// The caller must hold `&mut Rack`, or own the builder the cells came
    /// from. That is what rules out a worker being on the lane, and it is
    /// what makes the reference unique: handing out a second `&mut Lane` is
    /// undefined behaviour even if neither side writes. In particular this
    /// must never be reached from a `&self` method of `Rack`, which is
    /// `Sync` and so callable from two threads at once.
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn lane_held(&self) -> &mut Lane {
        debug_assert_ne!(
            self.state.load(Ordering::Acquire),
            GO,
            "the lane's worker is running it"
        );
        // SAFETY: the caller states it holds the rack exclusively.
        unsafe { &mut *self.lane.get() }
    }
}

/// A lane worker: waits for `GO`, runs the lane, says `DONE`; returns on
/// `QUIT`.
pub(crate) fn worker(cell: Arc<Shared>, hook: Hook) {
    if let Some(raise_priority) = &hook.0 {
        raise_priority();
    }
    // The same floating-point mode the rack runs the first lane under, so
    // that a lane gives the same samples whichever thread runs it. This
    // thread is the rack's own and does nothing else, so it is armed once
    // here and never put back.
    let _ = denormals::flush_to_zero();
    loop {
        let mut spins = 0u32;
        loop {
            match cell.state.load(Ordering::Acquire) {
                GO => break,
                QUIT => return,
                _ => {}
            }
            if spins < SPINS {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
        let n = cell.n.load(Ordering::Relaxed);
        let (left, right) = (
            cell.src[0].load(Ordering::Relaxed),
            cell.src[1].load(Ordering::Relaxed),
        );
        // SAFETY: both pointers and `n` were published before `GO`, which
        // this thread acquired; the rack keeps those buffers alive and does
        // not write them until it has seen `DONE`.
        let src: [&[f32]; 2] = unsafe {
            [
                std::slice::from_raw_parts(left, n),
                std::slice::from_raw_parts(right, n),
            ]
        };
        // SAFETY: this thread saw `GO` and has not yet stored `DONE`.
        let in_a = unsafe { cell.lane_running() }.run(src, n);
        cell.in_a.store(in_a, Ordering::Relaxed);
        cell.state.store(DONE, Ordering::Release);
    }
}
