//! The refresh rate of the display a window is on.

#![allow(unsafe_code)]

use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGGetDisplaysWithPoint,
};

use super::windows::Bounds;

/// Returns the refresh rate, in hertz, of the display under the centre of
/// `bounds`, or `None` if the system does not report one.
pub fn refresh_rate(bounds: Bounds) -> Option<f64> {
    let centre = CGPoint::new(
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    );
    let mut display: CGDirectDisplayID = 0;
    let mut count = 0u32;
    // SAFETY: `display` has room for the one display the call may write, and
    // `count` is a valid place for the number it found.
    let error = unsafe { CGGetDisplaysWithPoint(centre, 1, &mut display, &mut count) };
    if error != CGError::Success || count == 0 {
        return None;
    }
    let mode = CGDisplayCopyDisplayMode(display)?;
    let rate = CGDisplayMode::refresh_rate(Some(&mode));
    (rate > 0.0).then_some(rate)
}
