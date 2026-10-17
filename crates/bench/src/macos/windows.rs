//! Finding the windows of one process, from the window server's list.

#![allow(unsafe_code)]

use std::io;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowBounds,
    kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// A window of a process, as the window server lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    /// The window server's identifier for the window.
    pub id: u32,
    /// Where the window is, in points, from the top left of the main
    /// display.
    pub bounds: Bounds,
    /// The window's layer: 0 for ordinary application windows.
    pub layer: i32,
    /// Whether the window is on screen now.
    pub on_screen: bool,
    /// The window's title, if the window server reveals it.
    pub title: Option<String>,
}

/// A rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// The left edge.
    pub x: f64,
    /// The top edge.
    pub y: f64,
    /// The width.
    pub width: f64,
    /// The height.
    pub height: f64,
}

/// Returns the windows that process `pid` owns, on screen or not.
///
/// The window server lists every process's windows; all but `pid`'s are
/// dropped here, unread.
pub fn windows_of(pid: i32) -> io::Result<Vec<WindowInfo>> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, kCGNullWindowID)
        .ok_or_else(|| io::Error::other("the window server returned no window list"))?;
    // SAFETY: a CFArray is toll-free bridged with NSArray, and the window
    // list is documented as an array of dictionaries with string keys.
    let list: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*list as *const CFArray).cast() };
    // SAFETY: the keys are constant strings that Core Graphics exports.
    let (owner_key, number_key, bounds_key, layer_key, on_screen_key, name_key) = unsafe {
        (
            kCGWindowOwnerPID,
            kCGWindowNumber,
            kCGWindowBounds,
            kCGWindowLayer,
            kCGWindowIsOnscreen,
            kCGWindowName,
        )
    };
    let mut windows = Vec::new();
    for window in list.iter() {
        if number(&window, owner_key).map(|owner| owner.intValue()) != Some(pid) {
            continue;
        }
        let Some(id) = number(&window, number_key).map(|id| id.unsignedIntValue()) else {
            continue;
        };
        let bounds = entry(&window, bounds_key)
            .and_then(|bounds| bounds.downcast::<NSDictionary>().ok())
            .map(|bounds| {
                // SAFETY: the bounds dictionary has string keys.
                let bounds: &NSDictionary<NSString, AnyObject> =
                    unsafe { &*(&*bounds as *const NSDictionary).cast() };
                let field = |name: &str| {
                    bounds
                        .objectForKey(&NSString::from_str(name))
                        .and_then(|value| value.downcast::<NSNumber>().ok())
                        .map_or(0.0, |value| value.doubleValue())
                };
                Bounds {
                    x: field("X"),
                    y: field("Y"),
                    width: field("Width"),
                    height: field("Height"),
                }
            })
            .ok_or_else(|| io::Error::other(format!("window {id} has no bounds")))?;
        windows.push(WindowInfo {
            id,
            bounds,
            layer: number(&window, layer_key).map_or(0, |layer| layer.intValue()),
            on_screen: number(&window, on_screen_key).is_some_and(|flag| flag.boolValue()),
            title: entry(&window, name_key)
                .and_then(|title| title.downcast::<NSString>().ok())
                .map(|title| title.to_string()),
        });
    }
    Ok(windows)
}

/// Returns the value of `key` in a window's dictionary.
fn entry(
    window: &NSDictionary<NSString, AnyObject>,
    key: &CFString,
) -> Option<Retained<AnyObject>> {
    // SAFETY: a CFString is toll-free bridged with NSString.
    let key: &NSString = unsafe { &*(key as *const CFString).cast() };
    window.objectForKey(key)
}

/// Returns the value of `key` in a window's dictionary, if it is a number.
fn number(
    window: &NSDictionary<NSString, AnyObject>,
    key: &CFString,
) -> Option<Retained<NSNumber>> {
    entry(window, key).and_then(|value| value.downcast::<NSNumber>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_without_windows_has_none() {
        // This test process opens no window.
        let windows = windows_of(i32::try_from(std::process::id()).unwrap()).unwrap();
        assert!(windows.is_empty(), "{windows:?}");
    }
}
