//! Posting key presses to one process.
//!
//! Events go to the given process only, through `CGEventPostToPid`, never to
//! whichever window has focus: other people's sessions may be open on the
//! same machine. Posting needs the host's permission to post events, which
//! this module checks but never requests.
//!
//! Before every key, the process's window must also be frontmost, with no
//! other ordinary window in front of it, whichever process owns that window;
//! otherwise nothing is posted.

use std::io;

use objc2_core_graphics::{CGEvent, CGEventFlags, CGKeyCode, CGPreflightPostEventAccess};

use super::clock::{self, HostTime};
use super::windows::{self, WindowInfo};

/// Virtual key codes of the ANSI keyboard layout, for [`post_key`].
pub mod keycode {
    /// D, which with Command and Shift splits a Ghostty window downwards.
    pub const D: u16 = 2;
    /// Return.
    pub const RETURN: u16 = 36;
    /// The up arrow.
    pub const UP: u16 = 126;
}

/// The modifier keys held during a key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Command.
    pub command: bool,
    /// Shift.
    pub shift: bool,
    /// Control.
    pub control: bool,
    /// Option.
    pub option: bool,
}

impl Modifiers {
    fn flags(self) -> CGEventFlags {
        let mut flags = CGEventFlags::empty();
        for (held, flag) in [
            (self.command, CGEventFlags::MaskCommand),
            (self.shift, CGEventFlags::MaskShift),
            (self.control, CGEventFlags::MaskControl),
            (self.option, CGEventFlags::MaskAlternate),
        ] {
            if held {
                flags |= flag;
            }
        }
        flags
    }
}

/// Types `text` into process `pid`, whose window `window` must be frontmost:
/// a key press and release for each character, carrying the character
/// itself. Returns the host time read just before the first event was posted.
///
/// Letters, digits, space and common punctuation also carry their key code
/// on the ANSI layout, with Shift for capitals and shifted symbols, because
/// some programs read key codes rather than characters. Any other character
/// is sent with key code 0 and its text only.
pub fn post_text(pid: i32, window: &WindowInfo, text: &str) -> io::Result<HostTime> {
    check_access()?;
    check_frontmost(pid, window)?;
    let mut started = None;
    for character in text.chars() {
        let (code, shift) = ansi_key(character).unwrap_or((0, false));
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        let mut units = [0u16; 2];
        let units = character.encode_utf16(&mut units);
        let down = key_event(code, true, modifiers, units)?;
        let up = key_event(code, false, modifiers, units)?;
        started.get_or_insert_with(clock::now);
        CGEvent::post_to_pid(pid, Some(&down));
        CGEvent::post_to_pid(pid, Some(&up));
    }
    Ok(started.unwrap_or_else(clock::now))
}

/// Presses and releases one key, `code` from [`keycode`], with `modifiers`
/// held, in process `pid`, whose window `window` must be frontmost. Returns
/// the host time read just before posting.
pub fn post_key(
    pid: i32,
    window: &WindowInfo,
    code: u16,
    modifiers: Modifiers,
) -> io::Result<HostTime> {
    check_access()?;
    check_frontmost(pid, window)?;
    let down = key_event(code, true, modifiers, &[])?;
    let up = key_event(code, false, modifiers, &[])?;
    let started = clock::now();
    CGEvent::post_to_pid(pid, Some(&down));
    CGEvent::post_to_pid(pid, Some(&up));
    Ok(started)
}

fn check_access() -> io::Result<()> {
    if CGPreflightPostEventAccess() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "this process may not post key events: grant the program that runs it \
             Accessibility access in System Settings",
        ))
    }
}

/// Refuses to post unless `window`, of process `pid`, is frontmost.
fn check_frontmost(pid: i32, window: &WindowInfo) -> io::Result<()> {
    if windows::is_frontmost(pid, window)? {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "window not frontmost: another window is in front of process {pid}'s window; \
             no keys sent"
        )))
    }
}

/// Builds one key event. `text`, when not empty, is the UTF-16 text the
/// event carries.
#[allow(unsafe_code)]
fn key_event(
    code: CGKeyCode,
    down: bool,
    modifiers: Modifiers,
    text: &[u16],
) -> io::Result<objc2_core_foundation::CFRetained<CGEvent>> {
    let event = CGEvent::new_keyboard_event(None, code, down)
        .ok_or_else(|| io::Error::other("could not create a key event"))?;
    // Set the flags explicitly, so the event never inherits modifiers that
    // happen to be held on the keyboard.
    CGEvent::set_flags(Some(&event), modifiers.flags());
    if !text.is_empty() {
        // SAFETY: `text` is a live slice of `text.len()` UTF-16 units, which
        // the call copies into the event.
        unsafe {
            CGEvent::keyboard_set_unicode_string(Some(&event), text.len() as _, text.as_ptr());
        }
    }
    Ok(event)
}

/// Returns the ANSI key code of `character`, and whether it needs Shift.
fn ansi_key(character: char) -> Option<(CGKeyCode, bool)> {
    const UNSHIFTED: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0 `";
    const SHIFTED: &str = "ASDFHGZXCV\0BQWERYT!@#$^%+(&_*)}OU{IP\0LJ\"K:|<?NM>\0\0~";
    for (table, shift) in [(UNSHIFTED, false), (SHIFTED, true)] {
        if let Some(code) = table
            .chars()
            .position(|key| key == character && key != '\0')
        {
            return Some((code as CGKeyCode, shift));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_characters_to_ansi_key_codes() {
        assert_eq!(ansi_key('a'), Some((0, false)));
        assert_eq!(ansi_key('A'), Some((0, true)));
        assert_eq!(ansi_key('d'), Some((2, false)));
        assert_eq!(ansi_key('n'), Some((45, false)));
        assert_eq!(ansi_key('w'), Some((13, false)));
        assert_eq!(ansi_key('1'), Some((18, false)));
        assert_eq!(ansi_key('!'), Some((18, true)));
        assert_eq!(ansi_key('0'), Some((29, false)));
        assert_eq!(ansi_key(' '), Some((49, false)));
        assert_eq!(ansi_key('`'), Some((50, false)));
        assert_eq!(ansi_key('~'), Some((50, true)));
        assert_eq!(ansi_key('/'), Some((44, false)));
        assert_eq!(ansi_key('é'), None);
        assert_eq!(ansi_key('\0'), None);
    }

    #[test]
    fn modifiers_become_event_flags() {
        let flags = Modifiers {
            command: true,
            shift: true,
            ..Modifiers::default()
        }
        .flags();
        assert_eq!(flags, CGEventFlags::MaskCommand | CGEventFlags::MaskShift);
        assert_eq!(Modifiers::default().flags(), CGEventFlags::empty());
    }
}
