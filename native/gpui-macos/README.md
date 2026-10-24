# GPUI macOS focus correction

This directory contains the build inputs from the Apache-2.0-licensed
[`gpui-pre-macos` 0.3.9 package](https://crates.io/crates/gpui-pre-macos/0.3.9).
Its package archive SHA-256 is `c7c988b7e49775cc8542dbe48d5a10e073d8e898a023ef3ff3d24f33363e9863`.
The upstream source revision is `93d745668805029476e6d7fbb19652116fed51de`.
The original licence is retained in `LICENSE-APACHE`.

Only `src/window.rs` differs from the published build inputs:

- Forward the native window's accessibility focus query to the content-view
  AccessKit adapter. Fall back to the window itself when there is no focused
  child or installed adapter, without re-entering the superclass focus search.
- Initialize the adapter with the window's current key status, because the
  window can become key before the adapter is installed.
- Prime accessibility at window creation so GPUI builds its text tree before
  the first external focus query. This enables accessibility rendering from
  startup instead of waiting for an assistive client.

The workspace patches this existing dependency locally; it keeps the rest of
the GPUI package family at 0.3.9. Remove this override when a pinned upstream
version supplies equivalent native focus behavior. Keep these sources separate
from application code so an upstream update can be compared directly.
