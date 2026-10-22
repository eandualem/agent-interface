# tmux-workspaces (Rust)

A native desktop app for working in tmux: workspaces, tabs and split
layouts in a clickable sidebar, around natively drawn terminal panes that
aim to match a dedicated GPU terminal's speed.

It is a Rust reimplementation of
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces). The aim is
feature parity first, then a better interface. tmux is meant to stay the
process layer, so shells keep running when the app closes and existing
sessions can be attached.

**Status:** early development. The native app opens the Python library's saved
workspaces, tabs and split layouts and connects to their existing tmux sessions.
It displays live terminal output and sends typed keys to the selected ready pane.
Editing, full input parity and performance acceptance remain unfinished; it is not yet a replacement for the Python app.

Targets macOS first and Linux from the same code. Windows is optional.

## Run the app

Follow the [build prerequisites](CONTRIBUTING.md#set-up), then run:

```sh
cargo run -p tmux-workspaces-app --bin tmux-workspaces -- --data-dir /path/to/library
```

Use the workspace heading to choose a workspace, then choose a tab. The saved
split layout contains actual sessions; click a terminal before typing. Command-1
through Command-9 select tabs, and Command-Option-1 through Command-Option-9 select
workspaces. Command-[ and Command-] cycle panes; add Shift to cycle tabs.
Command-Option-Left/Right cycle workspaces, Command-Shift-Enter expands or
restores the selected pane, and Command-B focuses the sidebar. Navigation focuses
an already connected destination; if it is still connecting, click it when ready.
Command-V pastes clipboard text (Control-Shift-V on Linux), using
bracketed paste when the pane requests it. Keys follow the destination pane's
application cursor and extended-key modes. Paste is limited to 4 MiB per operation.
**Focus one pane** in tab options toggles the selected pane's view. Scroll over a
terminal to read its retained history. Shift-Page Up/Down moves a page;
Shift-Home/End goes to the oldest output or the live screen. Typing or pasting
returns to live output. Drag to select visible text; double-click selects a word
and triple-click selects a row. Command-C copies it (Control-Shift-C on Linux).
Selection keeps its displayed text while new output arrives. Escape, typing or
a new click outside the selection resumes the updated view. Copies from recovered
screens currently retain their displayed row breaks, including wrapped lines.
For programs with mouse reporting enabled, hold Shift to select text locally.
Ordinary clicks, drags, requested pointer motion and wheel gestures reach the
program. Each drag stays with its original pane; only one held-button gesture is
forwarded at a time. On macOS, either left/right release ends an active left/right gesture;
simultaneous left/right button chords are unsupported.
Hold Shift to keep scrolling local to the native viewer.
Command-N or **New Window** opens another native window on the current library,
with independent navigation and terminal connections. Closing one leaves the
other windows open. **Open Library…** in the application menu, or Command-O,
chooses another folder containing `layouts.db`.

The navigation panel on the left is native: the system font, the current
workspace's icon and name at the top, its tabs below, and menus that open where
you click. The terminals sit beside it on a slightly lighter surface, with a
status row along the bottom naming the workspace, tab and pane, the keys for the
current mode and a light while the saved arrangement is current. The bottom row
of the panel switches workspaces with one click, showing each workspace's icon,
or its number when it has none. Right-click one for its options, including
**Choose icon…**; when the row is full, **⋯** opens the full list.
**Settings** above it, or Command-comma, opens the Settings window: a dark or
light theme and the terminal text size apply to every window at once and are
saved in `~/.config/tmux-workspaces/settings.toml`; it also lists the shortcuts
and shows the Agent Backbone connection. **Close Window** (Command-W) closes a
window and leaves every session running. With Agent Backbone installed, an **AGENTS** section
under the tabs lists active agents with their state: red needs you, amber is
working. Click one to show the tab holding its session; **agents shown** in the
status row hides the section.

Open the workspace menu by clicking or right-clicking the workspace heading.
Choose **New workspace**, or press Command-Shift-N, and enter a name. Enter
saves it; Escape cancels. New workspaces have no tabs or shells.
Choose **Delete workspace** to remove an empty workspace; the last workspace and
workspaces with tabs cannot be deleted. Deleting selects the first remaining
workspace and leaves its tabs and sessions intact.

Click **+** beside TABS or press Command-T to add an empty tab. Command-D splits
right; Command-Shift-D splits below. Both split actions are also in tab options.
The existing pane remains intact and the new pane asks what it should run: Open
terminal, or Attach a session. New tabs
use the launch directory; splits inherit the ordinary shell’s remembered directory.
The new arrangement appears after saving. Navigation made while saving stays local;
otherwise the new tab or pane is selected.

Drag the divider between native panes to adjust their relative sizes. Release
saves the ratio; Escape or switching to another window during a drag cancels it.
This changes the native viewport without resizing other tmux clients. A changed
tab or split tree cancels the gesture. Save errors offer the usual Retry or Discard.

Choose **Open terminal** in an empty pane or tab options to start its
login shell, or reuse it if already running. The pane is saved before its shell
starts; a save error offers Retry or Discard. The new shell uses its saved directory
and the creating window's allowed environment. Existing shells retain their process
and environment. Navigating away before saving finishes cancels pending startup;
choose Open terminal again when returning. Exited shells retain their output and
are not restarted by this action.

Existing ordinary shells use the Python library's shell server; attachments use
their saved source socket, or the source resolved from the launch environment.
Use **Attach a session** in an empty pane, **Attach session** in tab options, or
Command-Shift-A, to
choose an existing source session. Type to filter names, use Up/Down and Enter,
or click a row. Escape or Cancel closes the chooser. It retains its destination
when focus moves within the tab; a changed or hidden empty destination must be
chosen again. Confirmation saves before replacing the terminal. Empty panes keep
their identity and saved directory when filled. Replacing an ordinary terminal
leaves its shell on the library server. Saves refresh the current tab’s nonempty
ordinary-shell directories, including parked shells, without copying external
session directories. Failed reads retain saved paths. Return to shell in tab options restores the existing ordinary shell
after saving; choose Open terminal if that shell is absent. The chooser lists tmux sessions
without optional agent metadata.

Missing sessions show an error with **Reconnect** instead of starting a shell.
Changing tabs or closing the app detaches its clients; the source’s existing
session-lifetime policy still applies. Connections preserve
the session environment. Like the Python viewer's own tmux clients, a displayed
terminal sizes its session to the pane under tmux's `window-size` option; text
scales down only while another client keeps the window larger.

Changes saved by the Python app refresh automatically. Double-click the workspace
name or the active tab's name to rename it. An inactive tab must first be selected.
Command-R renames the active tab; Command-Shift-R renames the workspace. The tab's
**⋯**, or right-clicking any tab, opens options to rename it, move it up/down or
move its complete layout to another workspace. Workspace options also offer
**Rename workspace**. A tab changed by another window must have its options
opened again before editing it.

The original name starts selected; Enter saves it, Escape or
clicking elsewhere cancels, and Ctrl-U clears the field. Names are limited to 80
characters. A failed save retains the edit and offers **Retry saving changes** or
**Discard unsaved changes**. Discard restores the last shared arrangement and
resumes polling with surviving local navigation. Panes that remain displayed keep
their connections.
A conflicting peer edit refreshes the saved arrangement and asks you to repeat
the edit.
Renaming requires an existing version-2 library and does not change tmux sessions.
Navigation and pane expansion stay local. Attached
sessions start on the source’s current window. An attached session has one row
above its terminal naming it on the right. When it has several windows, their
names share that row: click one to view it without switching other clients.
The chosen window is remembered locally when switching workspace tabs; reopening
the library starts from the source's current window again.
The list follows tmux, checking every two seconds. Custom tmux
prefix bindings, selection across history pages, terminal input methods
and key-release events are not yet wired into the product UI.

Omit `--data-dir` to use the Python-compatible environment and default library
location. An absent or unreadable library shows an error and can be replaced
through **Open Library…**; the browser never creates or repairs a library.
`--help` and `--version` work without a display or tmux.

## Inspect a keymap

After the [source build setup](CONTRIBUTING.md#build-and-test), print the effective
shortcuts or a complete compatible TOML configuration:

```sh
cargo run -p tmux-workspaces-app -- --print-keymap help
cargo run -p tmux-workspaces-app -- --keymap ./keymap.toml --print-keymap toml
cargo run -p tmux-workspaces-app -- --no-keymap --print-keymap toml
```

Selection follows `--keymap`, then `TMUX_WORKSPACES_KEYMAP`, then
`$XDG_CONFIG_HOME/tmux-workspaces/keymap.toml` (or
`~/.config/tmux-workspaces/keymap.toml`). `--no-keymap` uses shipped defaults.
Only a missing implicit default is ignored; invalid or explicitly missing files
report an error. These commands create no library, start no terminal and write
no configuration. Native shortcut dispatch is not available yet.

## Licence

MIT. See [LICENSE](LICENSE).
