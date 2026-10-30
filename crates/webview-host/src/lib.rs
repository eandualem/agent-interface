//! Adapt GPUI's XCB window identity to the Xlib identity WebKitGTK expects.
//! This crate owns no windows, connections, document content or application state.

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};

/// A borrowed X11 window, retaining the original handle's lifetime.
pub struct XlibWindow<'a>(WindowHandle<'a>);

impl<'a> XlibWindow<'a> {
    /// Borrow an XCB or Xlib window. Other window systems are rejected.
    pub fn new(window: &'a impl HasWindowHandle) -> Result<Self, HandleError> {
        let handle = window.window_handle()?;
        match handle.as_raw() {
            RawWindowHandle::Xcb(_) | RawWindowHandle::Xlib(_) => Ok(Self(handle)),
            _ => Err(HandleError::NotSupported),
        }
    }
}

impl HasWindowHandle for XlibWindow<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = as_xlib(self.0.as_raw());
        // SAFETY: the source handle keeps the window alive for this borrow. Only
        // the representation of its XID changes; no pointer or connection is cast.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

fn as_xlib(raw: RawWindowHandle) -> RawWindowHandle {
    match raw {
        RawWindowHandle::Xcb(xcb) => {
            // Both APIs identify the same server-side X11 window with its XID.
            let mut xlib = XlibWindowHandle::new(xcb.window.get().into());
            xlib.visual_id = xcb.visual_id.map_or(0, |id| id.get().into());
            RawWindowHandle::Xlib(xlib)
        }
        raw => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::XcbWindowHandle;
    use std::num::NonZeroU32;

    #[test]
    fn xcb_window_and_visual_are_preserved_as_xlib_ids() {
        let mut xcb = XcbWindowHandle::new(NonZeroU32::new(u32::MAX).unwrap());
        xcb.visual_id = NonZeroU32::new(47);
        let RawWindowHandle::Xlib(xlib) = as_xlib(xcb.into()) else {
            panic!("expected Xlib")
        };
        assert_eq!(xlib.window, std::os::raw::c_ulong::from(u32::MAX));
        assert_eq!(xlib.visual_id, 47);
        xcb.visual_id = None;
        let RawWindowHandle::Xlib(xlib) = as_xlib(xcb.into()) else {
            panic!("expected Xlib")
        };
        assert_eq!(xlib.visual_id, 0);
    }

    #[test]
    fn already_xlib_handle_passes_through_without_a_new_identity() {
        let mut xlib = XlibWindowHandle::new(123);
        xlib.visual_id = 321;
        assert_eq!(as_xlib(xlib.into()), xlib.into());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod glib_regression {
    use glib::prelude::*;

    #[test]
    fn variant_string_iterator() {
        let variant = ["first", "second", "third", "fourth", "last"].to_variant();
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.next(), Some("first"));
        assert_eq!(iter.next_back(), Some("last"));
        assert_eq!(iter.nth(1), Some("third"));
        assert_eq!(iter.last(), Some("fourth"));
        assert_eq!(variant.array_iter_str().unwrap().nth_back(2), Some("third"));
        assert_eq!(
            variant.array_iter_str().unwrap().collect::<Vec<_>>(),
            ["first", "second", "third", "fourth", "last"]
        );
    }
}
