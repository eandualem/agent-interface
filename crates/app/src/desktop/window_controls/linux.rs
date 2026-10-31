//! Own opaque X11 captions even when the WM lacks GTK shadow extensions.
//!
//! GPUI requires both a compositor and `_GTK_FRAME_EXTENTS` before requesting
//! client decorations. WSLg's XWM does not advertise that combination, but it
//! honors Motif decoration hints. Our opaque frame has no external shadow.

use gpui::{App, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::{
    connection::Connection,
    protocol::xproto::{Atom, ConnectionExt, PropMode},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

struct Caption {
    connection: RustConnection,
    window: u32,
    variant: Atom,
    utf8: Atom,
    light: Option<bool>,
}

impl Caption {
    fn new(window: &Window) -> Result<Self, Box<dyn std::error::Error>> {
        let window = match HasWindowHandle::window_handle(window)?.as_raw() {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
            _ => return Err("caption requires an X11 window".into()),
        };
        let (connection, _) = x11rb::connect(None)?;
        let motif = connection
            .intern_atom(false, b"_MOTIF_WM_HINTS")?
            .reply()?
            .atom;
        let variant = connection
            .intern_atom(false, b"_GTK_THEME_VARIANT")?
            .reply()?
            .atom;
        let utf8 = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        // Only the decorations field is set: no override-redirect, no change
        // to window-manager ownership, focus, stacking or allowed functions.
        connection
            .change_property32(PropMode::REPLACE, window, motif, motif, &[2, 0, 0, 0, 0])?
            .check()?;
        connection.flush()?;
        Ok(Self {
            connection,
            window,
            variant,
            utf8,
            light: None,
        })
    }

    fn theme(&mut self, light: bool) {
        if self.light == Some(light) {
            return;
        }
        let variant = if light {
            b"light".as_slice()
        } else {
            b"dark".as_slice()
        };
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            self.connection
                .change_property8(
                    PropMode::REPLACE,
                    self.window,
                    self.variant,
                    self.utf8,
                    variant,
                )?
                .check()?;
            self.connection.flush()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.light = Some(light);
                let _ = self.connection.flush();
            }
            Err(error) => tracing::debug!(event = "caption_theme_failed", %error),
        }
    }
}

pub(super) fn caption(window: &mut Window, cx: &mut App) -> bool {
    let state = window.use_state(cx, |window, _| match Caption::new(window) {
        Ok(caption) => Some(caption),
        Err(error) => {
            tracing::warn!(event = "client_caption_unavailable", %error);
            None
        }
    });
    state.update(cx, |caption, _| {
        if let Some(caption) = caption {
            caption.theme(crate::desktop::design::is_light());
            true
        } else {
            false
        }
    })
}
