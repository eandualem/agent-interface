# Wry 0.57.1 Linux embedding patches

This is the crates.io Wry 0.57.1 source under MIT OR Apache-2.0, with both
licences retained; trailing whitespace in SECURITY.md is normalized. The patches:

- Enable WebKitGTK process sandboxing in `WebContextImpl::create_context`, before a WebView is constructed.
- Retain the embedding X11 parent XID and focus it when returning to native controls,
  instead of focusing GTK's intermediary window.
- Destroy the GTK owner synchronously rather than requesting a window-manager close.
- Hold a weak WebView reference in its content manager's IPC handler, avoiding a
  reference cycle after a document is closed or its revision changes.

WebKitGTK API 4.1 defaults to disabled sandboxing and requires this call before
any web process exists. The app also verifies the setting before loading HTML.

The workspace patches crates.io to this source because Wry does not expose the
native context before constructing a view. Remove the corresponding patches when
upstream Wry provides these sandbox, focus and teardown guarantees.
See [WebKitGTK's sandbox setter](https://webkitgtk.org/reference/webkit2gtk/stable/method.WebContext.set_sandbox_enabled.html).
