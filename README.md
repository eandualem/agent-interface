# tmux-workspaces (Rust)

A native desktop app for working in tmux: workspaces, tabs and split
layouts in a clickable sidebar, around natively drawn terminal panes that
aim to match a dedicated GPU terminal's speed.

It is a Rust reimplementation of
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces). The aim is
feature parity first, then a better interface. tmux is meant to stay the
process layer, so shells keep running when the app closes and existing
sessions can be attached.

**Status:** in development. There is no product window or installable release
yet; the source build includes read-only keymap diagnostics.

Targets macOS first and Linux from the same code. Windows is optional.

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
