# Wry 0.57.1 Linux process sandbox patch

This is the crates.io Wry 0.57.1 source under MIT OR Apache-2.0, with both
licences retained; trailing whitespace in SECURITY.md is normalized. The only implementation change enables WebKitGTK process
sandboxing in `WebContextImpl::create_context`, before a WebView is constructed.
WebKitGTK API 4.1 defaults to disabled sandboxing and requires this call before
any web process exists. The app also verifies the setting before loading HTML.

The workspace patches crates.io to this source because Wry does not expose the
native context before constructing a view. Remove the patch when upstream Wry
provides this guarantee or a public pre-construction configuration hook.
See [WebKitGTK's sandbox setter](https://webkitgtk.org/reference/webkit2gtk/stable/method.WebContext.set_sandbox_enabled.html).
