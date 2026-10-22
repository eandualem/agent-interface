# 0027. Native name editing

- **Status:** Accepted for development; desktop and input-method acceptance pending.
- **Date:** 2026-10-21

## Decision

Connect workspace and tab renaming to the existing-version-2 writer accepted in
[0009](0009-python-library-and-shell-server-compatibility.md). Double-click only
the name to open an inline native field with selected text. An inactive tab's
first double-click selects it; another double-click opens its editor. Enter and
Save name submit, Escape and clicking outside cancel, and Ctrl-U clears. Empty
input keeps the editor open. The captured identity and original name must still
match when submitted. Renaming changes no tmux identity, process or attachment.

Use GPUI's platform text-input handler for composition, UTF-16 selection and
candidate-window geometry, with native shaped text, selection and horizontal
scrolling. Reuse pinned GPUI 0.3.9 (Apache-2.0). Add a direct exact dependency on
unicode-segmentation 1.13.3 (MIT OR Apache-2.0), already used transitively, so cursor
movement and deletion treat a combining sequence as one grapheme. The name limit
continues to count Unicode scalar values, matching the existing core operation.
No terminal renderer or terminal-input behavior changes.

One save runs at a time, off the UI thread. Its model/base snapshot stays fixed;
local navigation may continue. Stop applying reads before dispatch, discard the
old reader, apply the transaction's updated base and result while retaining the
current navigation, then start a fresh reader. This avoids applying a stale
queued snapshot after committing. Structural changes return input to navigation
before typing. A conflict refreshes the shared arrangement and displays a retry
notice. Other errors preserve the unsaved edit and expose an explicit save retry.
Further editing and switching libraries wait for that retry. Closing a window
releases its UI; a dispatched save can finish while the app remains running.

## Alternatives and verification

Saving on the UI thread would expose its SQLite lock timeout in terminal input.
A separate writer with an uninterrupted reader would require ordered generations
and acknowledgments to reject old snapshots. Restarting this lightweight reader
at the save boundary is sufficient for one in-flight edit. A platform dialog
would lose inline name interaction; a clipboard-only input would omit normal
text editing and composition.

Tests use a temporary database to check save completion after navigation,
conflict and retry, and active-tab removal during a save. Text-state tests cover
Unicode boundaries and the 80-character replacement limit. Existing mixed-process
storage fixtures remain the Python compatibility evidence. These tests do not
establish desktop focus, clipboard or input-method acceptance.

## Tab options

The selected tab's ellipsis and right-clicking any tab open native options.
Rename tab is first; move up/down and move to another workspace operate on
existing saved tabs. A captured tab and workspace identity guard these actions:
refresh before applying, and cancel if the selected target or tab content changed.
Moves save through the same asynchronous path and preserve the tab's pane
identities and session references. Navigation may continue during the save;
additional edits and library switching wait for completion or explicit retry.
The workspace chooser also offers renaming. Command-R and Command-Shift-R open
the tab and workspace name fields directly. Other tab-menu actions and keyboard
menu navigation remain separate parity work.
