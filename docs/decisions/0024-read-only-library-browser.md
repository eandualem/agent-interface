# 0024. Native read-only library browser

- **Status:** Accepted for development. Visual, accessibility and terminal
  performance acceptance remain pending.
- **Date:** 2026-10-21

The placeholder pane presentation is superseded by
[0025](0025-existing-session-terminals.md). The library remains read-only.

## Context

Workspace models and read-only library polling can support browsing saved
arrangements independently of terminal connection and rendering acceptance.
The initial native interface needs to make that distinction clear to users.

## Decision

Use the existing GPUI windowing path for a workspace/tab sidebar and a preview
of the selected tab's split tree. Pane cards show saved terminal or attachment
names and directories, with an explicit not-connected label. Selection changes
only the window's in-memory model. There are no editing or terminal controls.

Reuse `Library` for asynchronous reads and `WindowLayout` for refreshes that
retain local selection. A read failure keeps the last valid arrangement visible
with an error. Opening another library replaces the window's reader and model;
it neither repairs the library nor starts a tmux connection. The CLI accepts
the existing Python `--data-dir` convention, and the native folder picker
provides a desktop route to the same operation.

Reuse the pinned `gpui-pre =0.3.9` and `gpui-pre-platform =0.3.9` packages
(Apache-2.0), the existing SQLite reader and the existing model. No dependency
or terminal performance criterion changes. The UI framework's remaining
acceptance requirements in [0005](0005-ui-framework-and-terminal-renderer.md)
still apply.

## Alternatives

Connecting terminals in this first interface would couple library navigation
to unfinished connection lifecycle and input routing. Keep those integrations
separate so a saved pane cannot be mistaken for a ready input destination.

A separate preview renderer would duplicate the chosen native UI path. Use
the existing framework, keeping the browser inside the app crate and retaining
the benchmark window as a separate measurement program.
