/// CSP does not govern WebRTC sockets. Disable peer connections in the native
/// preferences before any authored script can run. Fail closed if WebKit no
/// longer exposes this SPI; a JavaScript override would not be a security boundary.
pub fn artifact_configuration()
-> Result<objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>, String> {
    use objc2::{MainThreadMarker, msg_send, runtime::NSObjectProtocol, sel};
    use objc2_web_kit::{WKWebViewConfiguration, WKWebsiteDataStore};
    let main = MainThreadMarker::new().ok_or("HTML configuration requires the main thread")?;
    // SAFETY: WebKit objects are owned and used on the main thread. The checked
    // selectors have BOOL getter/setter signatures in WKPreferencesPrivate.h.
    unsafe {
        let configuration = WKWebViewConfiguration::new(main);
        let preferences = configuration.preferences();
        if !preferences.respondsToSelector(sel!(_setPeerConnectionEnabled:))
            || !preferences.respondsToSelector(sel!(_peerConnectionEnabled))
        {
            return Err("This WebKit version cannot isolate document networking".into());
        }
        let _: () = msg_send![&*preferences, _setPeerConnectionEnabled: false];
        let enabled: bool = msg_send![&*preferences, _peerConnectionEnabled];
        if enabled {
            return Err("WebKit could not disable document peer connections".into());
        }
        // Wry preserves the data store of a supplied configuration, so explicitly
        // retain the ephemeral session that with_incognito normally selects.
        configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(main));
        Ok(configuration)
    }
}

/// Round the bottom corners `(left, right)` of a document's web view by
/// `radius` points, clipping its page to them, so the view stays inside the
/// rounded pane it is laid over. `view` is the web view's `NSView`. With
/// neither corner, it undoes an earlier rounding and sets the layer's
/// clipping back to `clipped`, what it was before. Returns whether the layer
/// clipped before this call, or `None` when the layer could not be reached.
pub fn round_bottom_corners(
    view: &objc2::runtime::AnyObject,
    (left, right): (bool, bool),
    radius: f64,
    clipped: bool,
) -> Option<bool> {
    use objc2::{MainThreadMarker, msg_send, runtime::AnyObject, sel};
    MainThreadMarker::new()?;
    // SAFETY: on the main thread that owns the view. `setWantsLayer:` and
    // `layer` are NSView methods, checked before the call; the rest are
    // CALayer properties with these types.
    unsafe {
        let responds: bool = msg_send![view, respondsToSelector: sel!(setWantsLayer:)];
        if !responds {
            return None;
        }
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![view, layer];
        let layer = layer.as_ref()?;
        let before: bool = msg_send![layer, masksToBounds];
        if !left && !right {
            // Back to a square layer that clips as it did before rounding.
            let _: () = msg_send![layer, setCornerRadius: 0.0f64];
            let _: () = msg_send![layer, setMaskedCorners: 15usize];
            let _: () = msg_send![layer, setMasksToBounds: clipped];
            return Some(before);
        }
        // CACornerMask: MinX-MinY 1, MaxX-MinY 2, MinX-MaxY 4, MaxX-MaxY 8,
        // in the layer's own coordinates. Unflipped, the window's y grows
        // upward and MinY is the bottom. Whether the layer is drawn flipped
        // depends on it and every layer above it, which `contentsAreFlipped`
        // accounts for; the layer's own `geometryFlipped` alone does not.
        let flipped: bool = msg_send![layer, contentsAreFlipped];
        let (min_x, max_x): (usize, usize) = if flipped { (4, 8) } else { (1, 2) };
        let mask = (if left { min_x } else { 0 }) | (if right { max_x } else { 0 });
        let _: () = msg_send![layer, setMaskedCorners: mask];
        let _: () = msg_send![layer, setCornerRadius: radius];
        let _: () = msg_send![layer, setMasksToBounds: true];
        Some(before)
    }
}
