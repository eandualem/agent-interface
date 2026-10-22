# 0035. Standalone first-run library initialization

Status: Accepted

## Context

The existing-library boundary in [0009](0009-python-library-and-shell-server-compatibility.md)
preserves shared data, but makes a fresh installation unusable without another
application creating `layouts.db`. Standalone operation needs an initial library.
The schema-v2 reader accepts one workspace with no tabs, so first launch needs
neither tmux sessions nor optional roster metadata.

## Decision

On the storage worker, resolve the normal library location and create an absent
database with one empty **Workspace 1**, a fresh 12-character identity, no selected
tab and `focus: false`. Use the existing `terminal_layout` table and row 1; the
legacy `layout` and `shared_layout` tables are unnecessary for current readers.
No schema or existing save-conflict rule changes.

Prepare and commit the database in a private temporary file in the destination
directory. Close SQLite and synchronize the file before installing it with a
no-replace operation. Concurrent first launches load the winning database.
New directories are private and the database has mode 0600. Existing paths,
including empty or malformed databases and dangling symlinks, never enter the
initializer. A missing database with journal/WAL sidecars is a recovery case,
not a fresh library. Existing read-only and writable-open APIs retain their
no-creation contract. No tmux server or shell is started by initialization.

This supersedes only the absent-library restriction in 0009. Legacy import,
repair, destructive reset and journal-mode changes remain outside this path.

## Alternatives and evidence

- Requiring another application to initialize the database prevents standalone
  adoption. Rejected.
- Creating the final file before writing its schema exposes a partial database
  to another launcher or after a crash. Rejected.
- Automatically repairing an existing or unrecognized database risks losing
  saved arrangements. Rejected.
- A temporary database and no-replace installation keep initialization separate
  from existing data. Selected. Tests cover simultaneous creators, normal saving
  afterwards, byte-preservation of existing files, dangling links and orphaned
  recovery data. Storage-worker tests cover first-run creation and subsequent
  polling without changing the window's UI.

The schema facts match the MIT-licensed source at
[`51de675`](https://github.com/eandualem/tmux-workspaces/tree/51de6757cd0aeda3c19fe68fa4ec5b93ab206804):
current-v2 loading uses `terminal_layout` alone. The Rust initializer deliberately
starts with an empty workspace, using the existing new-tab/attachment actions.

## Dependencies and licences

`tempfile` 3.27.0 (MIT OR Apache-2.0), already used by tests, becomes a runtime
dependency of `store` for private temporary files and `persist_noclobber`.
`rusqlite` 0.40.2 (MIT) and bundled SQLite (public domain), JSON serialization,
and the existing core identity generator are reused. No UI dependency or asset
changes are needed.
