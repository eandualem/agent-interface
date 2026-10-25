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
