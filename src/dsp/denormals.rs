//! Flushing denormal results to zero on the thread running the audio.
//!
//! A feedback delay, a reverb tail, a one-pole detector and an IIR filter's
//! ringing all decay towards zero, and a signal passing through them can
//! reach the denormal range, where some processors fall back to microcode
//! and take tens of times longer per operation. It never sounds wrong; it
//! shows up as a callback that intermittently overruns its budget.
//!
//! This is the machine-mode half of the story. The arithmetic half is
//! internal to this crate: every stage ends its own decaying state
//! eighteen decades above the subnormal boundary, on any thread and with
//! no mode set at all. Neither replaces the other. That keeps this crate's
//! state out of the denormal range; this module keeps the signal, and the
//! caller's own arithmetic, out of it too.
//!
//! # Who calls which
//!
//! The mode belongs to the thread, so it has to be set from whatever runs
//! the audio, and the right lifetime depends on whose thread it is.
//!
//! - A crate that **owns** the thread arms it once and leaves it: that is
//!   [`flush_to_zero`], and it is what `valverig-core` does on each of the
//!   worker threads its parallel lanes run on, threads that exist only to
//!   run audio and that end with the rack.
//! - A crate handed **someone else's** thread, a plugin's process block or
//!   an offline loop, arms it for the length of the call and puts it back:
//!   that is [`Restoring`], and it is what `valverig-core` does around each
//!   `Rack::process`.

/// The floating-point mode as it stood before [`flush_to_zero`] changed it.
///
/// Opaque, and only useful for handing to [`restore`].
#[derive(Debug, Clone, Copy)]
pub struct Mode(
    #[cfg(target_arch = "aarch64")] u64,
    #[cfg(any(
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    ))]
    u32,
);

/// Make this thread's arithmetic flush denormal results to zero, and on x86
/// treat denormal operands as zero too. Returns the mode it replaced, for
/// [`restore`]; a caller that owns the thread can drop it and never restore.
///
/// Cheap enough to call on every callback. Silently does nothing, and
/// returns a mode `restore` also ignores, outside aarch64 and x86 with SSE.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn flush_to_zero() -> Mode {
    /// FPCR bit 24, "flushing denormalized numbers to zero". On aarch64 this
    /// covers operands as well as results, so there is no separate
    /// denormals-are-zero bit to set.
    const FZ: u64 = 1 << 24;

    // SAFETY: FPCR is this thread's own floating-point control register and
    // both instructions are unprivileged on aarch64. The value written back
    // differs from the one read only in FZ, so no other setting is disturbed.
    unsafe {
        let fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr | FZ, options(nomem, nostack, preserves_flags));
        Mode(fpcr)
    }
}

/// Put the mode back as [`flush_to_zero`] found it.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn restore(mode: Mode) {
    // SAFETY: as in `flush_to_zero`; this writes back exactly what was read
    // there, on the same thread.
    unsafe {
        std::arch::asm!("msr fpcr, {}", in(reg) mode.0, options(nomem, nostack, preserves_flags));
    }
}

/// Make this thread's arithmetic flush denormal results to zero, and treat
/// denormal operands as zero too. Returns the mode it replaced, for
/// [`restore`]; a caller that owns the thread can drop it and never restore.
///
/// Cheap enough to call on every callback. Silently does nothing, and
/// returns a mode `restore` also ignores, outside aarch64 and x86 with SSE.
#[cfg(any(
    target_arch = "x86_64",
    all(target_arch = "x86", target_feature = "sse")
))]
#[inline]
pub fn flush_to_zero() -> Mode {
    /// MXCSR bit 15, flush-to-zero, and bit 6, denormals-are-zero.
    const FTZ_DAZ: u32 = (1 << 15) | (1 << 6);

    let mut found: u32 = 0;
    // SAFETY: MXCSR is this thread's own floating-point control register and
    // both instructions are unprivileged. `stmxcsr` writes the four bytes of
    // `found` and `ldmxcsr` reads four bytes back; the value written differs
    // from the one read only in FTZ and DAZ, so no other setting is
    // disturbed.
    unsafe {
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut found, options(nostack, preserves_flags));
        let armed = found | FTZ_DAZ;
        std::arch::asm!("ldmxcsr [{}]", in(reg) &armed, options(nostack, preserves_flags, readonly));
    }
    Mode(found)
}

/// Put the mode back as [`flush_to_zero`] found it.
#[cfg(any(
    target_arch = "x86_64",
    all(target_arch = "x86", target_feature = "sse")
))]
#[inline]
pub fn restore(mode: Mode) {
    // SAFETY: as in `flush_to_zero`; this writes back exactly what was read
    // there, on the same thread.
    unsafe {
        std::arch::asm!("ldmxcsr [{}]", in(reg) &mode.0, options(nostack, preserves_flags, readonly));
    }
}

/// Nothing to do on an architecture with no flush-to-zero mode this crate
/// knows how to reach.
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    all(target_arch = "x86", target_feature = "sse")
)))]
#[inline]
pub fn flush_to_zero() -> Mode {
    Mode()
}

/// Nothing to put back.
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    all(target_arch = "x86", target_feature = "sse")
)))]
#[inline]
pub fn restore(_mode: Mode) {}

/// Denormal flushing, armed for as long as this value is alive and put back
/// exactly as it was found when it is dropped, including while a panic
/// unwinds.
///
/// For a caller running on a thread it does not own. A caller that owns the
/// thread wants [`flush_to_zero`] on its own.
#[derive(Debug)]
pub struct Restoring(Mode);

impl Restoring {
    /// Arm it, remembering the mode as it stands.
    #[inline]
    pub fn arm() -> Self {
        Self(flush_to_zero())
    }
}

impl Drop for Restoring {
    #[inline]
    fn drop(&mut self) {
        restore(self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest denormal a multiply can land on, times one, is still
    /// denormal unless the result is flushed.
    fn denormal_survives() -> bool {
        let x = f32::from_bits(0x0000_0100);
        let product = std::hint::black_box(x) * std::hint::black_box(1.0f32);
        std::hint::black_box(product).to_bits() != 0
    }

    #[test]
    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    ))]
    fn arming_flushes_and_restoring_puts_the_thread_back() {
        assert!(denormal_survives(), "the thread starts without flushing");
        let found = flush_to_zero();
        assert!(
            !denormal_survives(),
            "armed, a denormal result becomes zero"
        );
        restore(found);
        assert!(denormal_survives(), "and restoring puts it back");
    }

    #[test]
    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "x86", target_feature = "sse")
    ))]
    fn the_guard_restores_when_it_goes_out_of_scope() {
        assert!(denormal_survives());
        {
            let _armed = Restoring::arm();
            assert!(!denormal_survives());
        }
        assert!(denormal_survives(), "the caller's thread is left as it was");
    }
}
