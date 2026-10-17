//! Finding the windows of one process, from the window server's list.

#![allow(unsafe_code)]

use std::io;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGCursorWindowLevel, kCGNullWindowID,
    kCGWindowAlpha, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerPID,
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
        let bounds = bounds_of(&window, bounds_key)
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

/// Returns whether any other process's visible window lies over part of
/// `window`, which belongs to process `pid`.
///
/// Only the place, layer and opacity of the windows in front of `window` are
/// read; nothing else about them. The mouse pointer, which the window server
/// lists as a window of its own, does not count.
pub fn is_covered(pid: i32, window: &WindowInfo) -> io::Result<bool> {
    let target = window.bounds;
    Ok(in_front(pid, window)?
        .iter()
        .any(|other| other.covers(target)))
}

/// Returns whether `window`, which belongs to process `pid`, is frontmost:
/// no ordinary window of another process is in front of it, wherever that
/// window is.
///
/// As for [`is_covered`], only the place, layer and opacity of the windows in
/// front are read.
pub fn is_frontmost(pid: i32, window: &WindowInfo) -> io::Result<bool> {
    Ok(!in_front(pid, window)?
        .iter()
        .any(InFront::is_ordinary_window))
}

/// What the window list says about another process's window in front of a
/// measured one: its layer, opacity and place, and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
struct InFront {
    layer: Option<i32>,
    alpha: Option<f64>,
    bounds: Option<Bounds>,
}

impl InFront {
    /// Whether it can be seen at all: it is not fully transparent, and it is
    /// not the mouse pointer.
    fn is_visible(&self) -> bool {
        self.alpha != Some(0.0) && self.layer != Some(kCGCursorWindowLevel)
    }

    /// Whether it is visible and lies over part of `target`.
    fn covers(&self, target: Bounds) -> bool {
        self.is_visible() && self.bounds.is_some_and(|bounds| bounds.overlaps(target))
    }

    /// Whether it is a visible, non-empty ordinary application window.
    fn is_ordinary_window(&self) -> bool {
        self.is_visible()
            && self.layer == Some(0)
            && self
                .bounds
                .is_some_and(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
    }
}

/// Lists the on-screen windows of other processes in front of `window`,
/// which belongs to process `pid`.
fn in_front(pid: i32, window: &WindowInfo) -> io::Result<Vec<InFront>> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenAboveWindow, window.id)
        .ok_or_else(|| io::Error::other("the window server returned no window list"))?;
    // SAFETY: as in `windows_of`.
    let list: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*list as *const CFArray).cast() };
    // SAFETY: the keys are constant strings that Core Graphics exports.
    let (owner_key, bounds_key, alpha_key, layer_key) = unsafe {
        (
            kCGWindowOwnerPID,
            kCGWindowBounds,
            kCGWindowAlpha,
            kCGWindowLayer,
        )
    };
    Ok(list
        .iter()
        .filter(|other| number(other, owner_key).map(|owner| owner.intValue()) != Some(pid))
        .map(|other| InFront {
            layer: number(&other, layer_key).map(|layer| layer.intValue()),
            alpha: number(&other, alpha_key).map(|alpha| alpha.doubleValue()),
            bounds: bounds_of(&other, bounds_key),
        })
        .collect())
}

impl Bounds {
    /// Returns whether this rectangle and `other` share any area.
    pub fn overlaps(self, other: Bounds) -> bool {
        let overlap = |start: f64, length: f64, other_start: f64, other_length: f64| {
            start.max(other_start) < (start + length).min(other_start + other_length)
        };
        overlap(self.x, self.width, other.x, other.width)
            && overlap(self.y, self.height, other.y, other.height)
    }

    /// Returns whether `inner` lies entirely within this rectangle.
    pub fn contains(self, inner: Bounds) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.x + inner.width <= self.x + self.width
            && inner.y + inner.height <= self.y + self.height
    }
}

/// Returns a window's bounds from its dictionary.
fn bounds_of(window: &NSDictionary<NSString, AnyObject>, key: &CFString) -> Option<Bounds> {
    let bounds = entry(window, key)?.downcast::<NSDictionary>().ok()?;
    // SAFETY: the bounds dictionary has string keys.
    let bounds: &NSDictionary<NSString, AnyObject> =
        unsafe { &*(&*bounds as *const NSDictionary).cast() };
    let field = |name: &str| {
        bounds
            .objectForKey(&NSString::from_str(name))
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .map_or(0.0, |value| value.doubleValue())
    };
    Some(Bounds {
        x: field("X"),
        y: field("Y"),
        width: field("Width"),
        height: field("Height"),
    })
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

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> Bounds {
        Bounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn rectangles_overlap_only_when_they_share_area() {
        let window = bounds(40.0, 60.0, 100.0, 50.0);
        assert!(window.overlaps(bounds(130.0, 100.0, 20.0, 20.0)));
        // Touching edges share no area.
        assert!(!window.overlaps(bounds(140.0, 60.0, 20.0, 20.0)));
        assert!(!window.overlaps(bounds(0.0, 0.0, 40.0, 60.0)));
    }

    #[test]
    fn a_display_contains_only_windows_wholly_inside_it() {
        let display = bounds(0.0, 0.0, 1512.0, 982.0);
        assert!(display.contains(bounds(40.0, 60.0, 1400.0, 900.0)));
        assert!(display.contains(display));
        assert!(!display.contains(bounds(40.0, 60.0, 1500.0, 900.0)));
        assert!(!display.contains(bounds(-10.0, 60.0, 100.0, 100.0)));
        assert!(!display.contains(bounds(1600.0, 60.0, 100.0, 100.0)));
    }

    #[test]
    fn only_visible_ordinary_windows_in_front_take_the_front() {
        let window = |layer, alpha, bounds| InFront {
            layer: Some(layer),
            alpha: Some(alpha),
            bounds: Some(bounds),
        };
        let anywhere = bounds(2000.0, 0.0, 300.0, 200.0);
        // Ordinary windows count wherever they are, even on another display.
        assert!(window(0, 1.0, anywhere).is_ordinary_window());
        // Transparent, empty, menu bar and pointer windows do not.
        assert!(!window(0, 0.0, anywhere).is_ordinary_window());
        assert!(!window(0, 1.0, bounds(0.0, 0.0, 0.0, 0.0)).is_ordinary_window());
        assert!(!window(24, 1.0, anywhere).is_ordinary_window());
        assert!(!window(kCGCursorWindowLevel, 1.0, anywhere).is_ordinary_window());
    }

    #[test]
    fn visible_windows_of_any_layer_cover_but_the_pointer_does_not() {
        let target = bounds(40.0, 60.0, 100.0, 50.0);
        let over = bounds(50.0, 70.0, 10.0, 10.0);
        let window = |layer, alpha, bounds| InFront {
            layer: Some(layer),
            alpha: Some(alpha),
            bounds: Some(bounds),
        };
        assert!(window(24, 1.0, over).covers(target));
        assert!(!window(0, 0.0, over).covers(target));
        assert!(!window(kCGCursorWindowLevel, 1.0, over).covers(target));
        assert!(!window(0, 1.0, bounds(500.0, 500.0, 10.0, 10.0)).covers(target));
    }

    #[test]
    fn a_process_without_windows_has_none() {
        // This test process opens no window.
        let windows = windows_of(i32::try_from(std::process::id()).unwrap()).unwrap();
        assert!(windows.is_empty(), "{windows:?}");
    }
}
