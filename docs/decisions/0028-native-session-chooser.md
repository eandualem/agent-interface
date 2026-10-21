# 0028. Native existing-session chooser

- **Status:** Accepted for development; desktop acceptance pending.
- **Date:** 2026-10-21

## Decision

Open Attach session from a pane header, tab options or Command-Shift-A. Show the
existing source-session poller's results in a native pane chooser, with
case-insensitive filtering, arrow selection and Enter or a row click to confirm.
The source socket is captured by the window at launch. New references store its
resolved path explicitly; existing legacy references retain their fallback rules.
A failed discovery replaces previous availability with an error. Saving a reference
cannot guarantee its source remains online after discovery.

Capture the destination when opening. Ordinary attachment changes require the
same selected tab and unchanged pane attachment after refresh, even if focus moved
within that tab. Empty fills also require the same pane to remain empty and
displayed. The core guards implement these distinct checks. Save a proposed model
before installing it in the window, so neither a failed save nor a conflict can
connect an uncommitted replacement terminal. Keep failed proposed edits available
for explicit retry. Discarding a failed proposal abandons only the unsaved edit,
restores the known shared base with local navigation, and resumes polling without
tearing down unchanged displayed panes. Successful replacement returns keyboard focus
to navigation.

Reuse the accepted existing-version-2 store and its whole-tab-tree conflict rules.
A filled leaf retains its identity and saved directory, removes the empty flag,
and stores the attachment name/socket. No extra library fields or migration are
needed. The existing ordinary shell stays on its server. This chooser selects
existing sessions; ordinary-shell creation is a separate workflow. Fresh ordinary
shell directory snapshots remain separate from preserving the stored directory.

Reuse GPUI and the native text field from [0027](0027-native-name-editing.md).
Use the existing [session choice model](0029-attachment-chooser-model.md) for full
Unicode case folding, 80 printable scalar filters and stale-selection guards.
The native field uses the existing MIT/Apache-2.0 unicode-properties dependency
for the same printable-character rules. Polling, storage and worker disposal
stay off the UI thread. Discovery preserves manual scrolling; keyboard selection
and filter edits explicitly reveal the selected row.

## Alternatives and verification

Applying the model before saving would allow the UI to connect a replacement that
was never committed. A synchronous save would block terminal interaction during
a database lock. Keep the displayed model while the worker saves a detached edit,
then apply the result with the window's surviving local navigation.

Verify destination guards and saved JSON with temporary libraries, and discovery
and connections with owned private tmux servers. Check that no replacement becomes
displayable before saving, including conflict and failure paths. Source contracts
are pinned to the Python reference recorded in [0009](0009-python-library-and-shell-server-compatibility.md).
