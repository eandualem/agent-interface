# 0030. Native empty tabs and splits

- **Status:** Accepted for development; desktop acceptance pending.
- **Date:** 2026-10-21

## Decision

Use the existing core model to create empty tabs and right/below splits from
native controls. TABS has a plus button; Command-T adds a tab, Command-D splits
right and Command-Shift-D splits below. The tab menu also offers both splits.
New tabs use the process launch directory, a random neutral name and fresh model
identities. Splits retain the original leaf as the first child and add an empty
second child at ratio 0.5, selecting it and restoring the full layout. A split
in an empty workspace adds an empty tab instead. No ordinary shell is created.

On the save worker, read the previous current tab's nonempty ordinary-shell
sessions, including parked shells. Successful reads update only saved cwd;
missing or failed reads retain prior values. Never query an attachment's source
for this purpose or refresh an empty leaf from an orphan shell. Reuse bounded,
non-starting queries against the existing library socket. Stop starting further
queries after three seconds; an already running query retains its own three-second
bound. Split creation follows these snapshots and inherits the focused leaf's
ordinary cwd, falling back to the launch directory. New tabs always use the
launch directory. A saved cwd change does not change a terminal connection's
identity or its remembered tmux window.

Build a detached candidate and save before replacing the displayed arrangement.
The existing store validates depth, identities, nesting and record size, and
handles concurrent edits using the accepted version-2 conflict rules. On failure,
retry uses the same generated candidate and IDs. Discard abandons it. After a
successful save, select and reveal the new pane/tab only if the user's visible
navigation still matches the creation request; otherwise retain navigation made
while saving. Other tabs' remembered selections stay local and unchanged.

## Alternatives and verification

Mutating the displayed tree before saving could show an invalid split or a tab
that was never persisted. Rebuilding on retry would allocate a different identity.
Reading directories on the UI thread could stall terminal input. Copying an
external attachment's cwd would overwrite the parked ordinary shell's directory.

Temporary libraries verify save-before-display, retry identity, saved split
structure and navigation during save. Owned private tmux fixtures verify ordinary
and parked directory snapshots and exclusion of empty leaves. Existing validation
and conflict tests cover the unchanged storage boundary. Native desktop event
acceptance, separator dragging, viewport sizing and ordinary-shell creation remain
separate work. This adds no dependency or persistence field.

Reference behavior is pinned to Python `51de675`, `model.py`, `sidebar.py` and
`shells.py`, under the compatibility decision [0009](0009-python-library-and-shell-server-compatibility.md).
