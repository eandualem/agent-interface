# 0023. Visible window layouts

- **Status:** Accepted for benchmark terminal integration; floating panes remain unsupported.
- **Date:** 2026-10-20

## Context

The terminal view needs the visible rectangles after a split, resize or zoom.
A saved layout still describes every tiled pane while a window is zoomed.
The visible layout describes the rectangles to draw.

The tmux 3.8 startup query returns a version 2 JSON layout
([format callbacks](https://github.com/tmux/tmux/blob/3.8/format.c),
[layout encoding](https://github.com/tmux/tmux/blob/3.8/layout-custom.c)). A control client
without `new-layouts` receives the legacy layout, which omits floating panes.
Older tmux versions ignore unknown client flags. These behaviors were checked
in the corresponding tmux source; no tmux implementation code is copied.

## Decision

Read `window_visible_layout` and retain the window and pane identities.
Accept the legacy checksum-prefixed format and version 2 JSON. Request the
`new-layouts` client flag, which selects JSON notifications on tmux 3.8 and
is ignored by older versions. Preserve nested tiled rectangles and zoom.
Reject malformed or unsupported layouts rather than guessing geometry.
Floating panes in JSON produce an explicit unsupported-layout error; the
legacy protocol cannot represent them, so floating-pane parity remains open.

Use [`serde_json` **1.0.154**](https://docs.rs/crate/serde_json/1.0.154),
**MIT OR Apache-2.0**, already present in the
workspace lockfile, for JSON decoding. Keep its default nesting limit, with
overly deep JSON rejected. A handwritten JSON decoder would add protocol
code without improving this integration. Legacy layout parsing uses an
explicit stack so nesting does not consume the Rust call stack.

Publish changed rectangles with screens recovered at their corresponding
sizes. Keep the previous geometry while recovery runs. Wait for 100 ms of
layout quiet before starting captures, matching the existing resize fidelity
harness. Native window resize requests and attachment window switching remain
separate integrations.

## Alternatives

- Reading only `list-panes` gives pane rectangles but does not express the
  visible layout or nesting; zoom requires additional filtering.
- Reading only the legacy layout fails the tmux 3.8 startup query and cannot
  represent floating panes.
- Drawing the new rectangles immediately pairs them with stale screen sizes.
  Reflowing the local engine already failed the resize fidelity comparison;
  recover the pane instead, as specified in [0021](0021-pane-state-recovery.md).
