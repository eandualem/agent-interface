//! The process's peak memory, read from the operating system.
//!
//! `agent-interface-bench-rio` includes this file.

#![allow(unsafe_code)]

use std::mem::MaybeUninit;

/// Returns this process's peak resident memory in bytes, if the system
/// reports it.
pub fn peak_resident_bytes() -> Option<u64> {
    let mut usage = MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` writes one `rusage` through the pointer, which
    // points to space for one; `RUSAGE_SELF` is a valid target.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: `getrusage` succeeded, so it initialised the value; a zeroed
    // `rusage` is valid in any case, being plain integers.
    let usage = unsafe { usage.assume_init() };
    let peak = u64::try_from(usage.ru_maxrss).ok()?;
    // macOS reports bytes; Linux reports kilobytes.
    Some(if cfg!(target_os = "macos") {
        peak
    } else {
        peak * 1024
    })
}
