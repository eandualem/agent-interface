# GPUI macOS integration

This directory contains the build inputs from the Apache-2.0-licensed
[`gpui-pre-macos` 0.3.9 package](https://crates.io/crates/gpui-pre-macos/0.3.9).
Its package archive SHA-256 is `c7c988b7e49775cc8542dbe48d5a10e073d8e898a023ef3ff3d24f33363e9863`.
The upstream source revision is `93d745668805029476e6d7fbb19652116fed51de`.
The original licence is retained in `LICENSE-APACHE`.

The local changes in `src/window.rs`, `src/pasteboard.rs` and `src/artifact.rs` are:

- Forward the native window's accessibility focus query to the content-view
  AccessKit adapter. Fall back to the window itself when there is no focused
  child or installed adapter, without re-entering the superclass focus search.
- Initialize the adapter with the window's current key status, because the
  window can become key before the adapter is installed.
- Prime accessibility at window creation so GPUI builds its text tree before
  the first external focus query. This enables accessibility rendering from
  startup instead of waiting for an assistive client.

- Read current file-URL pasteboard entries as well as legacy filename lists.
- Accept image drags, including floating screenshot thumbnails, by staging a
  PNG in a private temporary directory. Detected input formats are limited to
  PNG, TIFF and JPEG before decoder construction. Cancelled drags remove the staged image;
  accepted drops retain it for the receiving terminal program. `Cargo.toml` adds
  the existing MIT OR Apache-2.0 `tempfile` dependency, pinned to 3.27.0.

- Provide a safe artifact configuration function that disables WebRTC peer
  connections before document loading and selects an ephemeral WebKit data store.
  It checks the native preference selectors and fails closed when unavailable.
  This uses the existing MIT-licensed `objc2-web-kit` bindings, pinned to 0.3.2.

The workspace patches this existing dependency locally; it keeps the rest of
the GPUI package family at 0.3.9. Remove this override when a pinned upstream
version supplies equivalent native focus behavior. Keep these sources separate
from application code so an upstream update can be compared directly.
