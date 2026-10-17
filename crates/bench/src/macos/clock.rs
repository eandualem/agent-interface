//! The host clock that ScreenCaptureKit stamps frames with.
//!
//! ScreenCaptureKit reports when the window server displayed a frame as
//! `mach_absolute_time` ticks. [`now`] reads the same clock, so the time a key
//! was posted and the time its frame was displayed can be subtracted.

#![allow(unsafe_code)]

use std::sync::OnceLock;
use std::time::Duration;

/// A moment on the host clock, in nanoseconds since the machine started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostTime(u64);

impl HostTime {
    /// Converts `mach_absolute_time` ticks, the unit ScreenCaptureKit uses.
    pub fn from_ticks(ticks: u64) -> Self {
        let (numer, denom) = timebase();
        let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom);
        Self(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// A moment `nanos` nanoseconds after the machine started, for tests.
    #[cfg(test)]
    pub fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Returns the nanoseconds since the machine started.
    pub fn nanos(self) -> u64 {
        self.0
    }

    /// Returns the moment `duration` after this one.
    pub fn after(self, duration: Duration) -> Self {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(nanos))
    }

    /// Returns the time from `earlier` to this moment, or zero if `earlier`
    /// is later.
    pub fn since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.nanos().saturating_sub(earlier.nanos()))
    }
}

/// Returns the current time on the host clock.
pub fn now() -> HostTime {
    HostTime::from_ticks(mach_absolute_time())
}

#[repr(C)]
struct TimebaseInfo {
    numer: u32,
    denom: u32,
}

// The `libc` declarations of these functions are deprecated in favour of a
// separate crate; two declarations are simpler than a further dependency.
unsafe extern "C" {
    safe fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
}

/// Returns the factor that turns ticks into nanoseconds, as a fraction.
fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = TimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid, writable `mach_timebase_info_data_t`.
        let status = unsafe { mach_timebase_info(&mut info) };
        if status != 0 || info.numer == 0 || info.denom == 0 {
            // The call does not fail on any supported system; nanoseconds
            // are the safest guess if it ever does.
            (1, 1)
        } else {
            (info.numer, info.denom)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_clock_moves_with_the_standard_clock() {
        let host = now();
        let standard = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let host_elapsed = now().since(host);
        let standard_elapsed = standard.elapsed();
        let difference = host_elapsed.abs_diff(standard_elapsed);
        assert!(difference < Duration::from_millis(5), "{difference:?}");
        assert_eq!(host.since(now()), Duration::ZERO);
    }
}
