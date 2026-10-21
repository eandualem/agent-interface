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
It displays live terminal output and sends typed keys to the clicked pane.
Editing, shell creation, full input parity and performance acceptance remain
unfinished; it is not yet a replacement for the Python app.

Targets macOS first and Linux from the same code. Windows is optional.

## Run the app

Follow the [build prerequisites](CONTRIBUTING.md#set-up), then run:

```sh
cargo run -p tmux-workspaces-app --bin tmux-workspaces -- --data-dir /path/to/library
```

Use the workspace heading to choose a workspace, then choose a tab. The saved
split layout contains actual sessions; click a terminal before typing. Command-1
through Command-9 select tabs, and Command-Option-1 through Command-Option-9 select
workspaces. Command-V pastes clipboard text (Control-Shift-V on Linux), using
bracketed paste when the pane requests it. Keys follow the destination pane's
application cursor and extended-key modes. Paste is limited to 4 MiB per operation.
The pane header's expand control toggles the selected pane's view. Scroll over a
terminal to read its retained history. Shift-Page Up/Down moves a page;
Shift-Home/End goes to the oldest output or the live screen. Typing or pasting
returns to live output. Drag to select visible text; double-click selects a word
and triple-click selects a row. Command-C copies it (Control-Shift-C on Linux).
Selection keeps its displayed text while new output arrives. Escape, typing or
a new click outside the selection returns to the live view. Copies from recovered
screens currently retain their displayed row breaks, including wrapped lines.
**Open library…** chooses another folder containing `layouts.db`.

Existing ordinary shells use the Python library's shell server; attachments use
their saved source socket, or the source resolved from the launch environment.
Missing sessions show an error with **Reconnect** instead of starting a shell.
Sessions keep running when you change tabs or close the app. Connections preserve
the session environment and do not participate in resizing other clients; text
scales down when the existing terminal is larger than its native pane.

Changes saved by the Python app refresh automatically. Native navigation and
pane expansion are local, and the app does not write saved layouts. Attached
sessions currently show the window selected at connection time. Custom tmux
prefix bindings, terminal mouse reporting, selection across history pages, input methods
and key-release events are not yet wired into the product UI.

Omit `--data-dir` to use the Python-compatible environment and default library
location. An absent or unreadable library shows an error and can be replaced
through **Open library…**; the browser never creates or repairs a library.
`--help` and `--version` work without a display or tmux.

## Licence

MIT. See [LICENSE](LICENSE).
