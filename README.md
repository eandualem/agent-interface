# tmux-workspaces (Rust)

A native desktop app for working in tmux: workspaces, tabs and split
layouts in a clickable sidebar, around natively drawn terminal panes that
aim to match a dedicated GPU terminal's speed.

It is a Rust reimplementation of
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces). The aim is
feature parity first, then a better interface. tmux is meant to stay the
process layer, so shells keep running when the app closes and existing
sessions can be attached.

**Status:** early development. The native window can browse an existing library's
workspaces, tabs and saved split layouts. Navigation is local to the window;
saved layouts are not edited. Terminal connections, editing controls and
performance acceptance remain unfinished. This is not yet a replacement for
the Python app.

Targets macOS first and Linux from the same code. Windows is optional.

## Try the library browser

Follow the [build prerequisites](CONTRIBUTING.md#set-up), then run:

```sh
cargo run -p tmux-workspaces-app --bin tmux-workspaces -- --data-dir /path/to/library
```

Select workspaces and tabs in the sidebar to inspect their saved panes. The
panes are previews; they are not connected terminals. **Open library…** chooses
another folder containing `layouts.db`. Changes saved by the Python app refresh
automatically, while your local navigation stays selected where possible.

Omit `--data-dir` to use the Python-compatible environment and default library
location. An absent or unreadable library shows an error and can be replaced
through **Open library…**; the browser never creates or repairs a library.
`--help` and `--version` work without a display or tmux.

## Licence

MIT. See [LICENSE](LICENSE).
