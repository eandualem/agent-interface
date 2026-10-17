//! The displays measured windows are on: the main display's bounds, the
//! refresh rate of the display under a window and whether it is asleep, and
//! whether the screen is locked.

#![allow(unsafe_code)]

use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFDictionary, CGPoint};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsAsleep, CGDisplayMode,
    CGError, CGGetDisplaysWithPoint, CGMainDisplayID, CGSessionCopyCurrentDictionary,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

use super::windows::Bounds;

/// Returns the refresh rate, in hertz, of the display under the centre of
/// `bounds`, or `None` if the system does not report one.
pub fn refresh_rate(bounds: Bounds) -> Option<f64> {
    let mode = CGDisplayCopyDisplayMode(display_at(bounds)?)?;
    let rate = CGDisplayMode::refresh_rate(Some(&mode));
    (rate > 0.0).then_some(rate)
}

/// Returns the main display's bounds, in the points window bounds use: from
/// the main display's top left corner.
pub fn main_bounds() -> Bounds {
    let rect = CGDisplayBounds(CGMainDisplayID());
    Bounds {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// Whether the screen can show a run: the display under the window awake and
/// the screen unlocked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayState {
    /// Whether the display under the window is asleep, or `None` if no
    /// display is under it.
    pub asleep: Option<bool>,
    /// Whether the screen is locked, or `None` if the login session does not
    /// say.
    pub locked: Option<bool>,
}

impl DisplayState {
    /// Reads the state of the display under the centre of `bounds`, and
    /// whether the screen is locked.
    pub fn read(bounds: Bounds) -> Self {
        Self {
            asleep: display_at(bounds).map(|display| CGDisplayIsAsleep(display)),
            locked: screen_locked(),
        }
    }

    /// Returns an error, for a failed run, unless the display is known to be
    /// awake and the screen known to be unlocked: an asleep display or a
    /// locked screen shows nothing, so a run then would only look slow.
    pub fn check(self) -> Result<(), String> {
        match (self.asleep, self.locked) {
            (Some(true), _) => Err("display asleep: the window's display is asleep".to_owned()),
            (None, _) => Err("display unknown: no display is under the window".to_owned()),
            (_, Some(true)) => Err("screen locked: unlock the screen for the run".to_owned()),
            (_, None) => Err(
                "screen state unknown: the login session does not say whether the screen \
                     is locked"
                    .to_owned(),
            ),
            (Some(false), Some(false)) => Ok(()),
        }
    }
}

/// Returns the display under the centre of `bounds`.
fn display_at(bounds: Bounds) -> Option<CGDirectDisplayID> {
    let centre = CGPoint::new(
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    );
    let mut display: CGDirectDisplayID = 0;
    let mut count = 0u32;
    // SAFETY: `display` has room for the one display the call may write, and
    // `count` is a valid place for the number it found.
    let error = unsafe { CGGetDisplaysWithPoint(centre, 1, &mut display, &mut count) };
    (error == CGError::Success && count > 0).then_some(display)
}

/// Returns whether the screen is locked, or `None` outside a login session.
fn screen_locked() -> Option<bool> {
    let session = CGSessionCopyCurrentDictionary()?;
    // SAFETY: a CFDictionary is toll-free bridged with NSDictionary, and the
    // session dictionary has string keys.
    let session: &NSDictionary<NSString, AnyObject> =
        unsafe { &*(&*session as *const CFDictionary).cast() };
    // The session lists this key only while the screen is locked.
    Some(
        session
            .objectForKey(&NSString::from_str("CGSSessionScreenIsLocked"))
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .is_some_and(|locked| locked.boolValue()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_awake_display_and_an_unlocked_screen_allow_a_run() {
        let state = |asleep, locked| DisplayState { asleep, locked };
        assert_eq!(state(Some(false), Some(false)).check(), Ok(()));
        for refused in [
            state(Some(true), Some(false)),
            state(Some(false), Some(true)),
            state(None, Some(false)),
            state(Some(false), None),
        ] {
            assert!(refused.check().is_err(), "{refused:?}");
        }
    }
}
