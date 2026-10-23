# tmux-workspaces (Rust)

A native desktop app for organizing tmux sessions into workspaces, tabs and
split panes. Open a shell, attach a session you already use, and return to the
same arrangement later. tmux owns the processes, so ordinary shells keep
running when you close the app.

- Arrange terminal panes in saved tabs and workspaces.
- Attach existing tmux sessions and switch between their windows.
- Search tabs, panes and commands with Jump.
- Choose a light or dark theme and customize keyboard shortcuts.
- Show agent status when Agent Backbone is installed.

The app draws terminals natively and needs neither Python nor Agent Backbone
at runtime. It can open existing schema-v2 libraries from
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces).

## Install

See the [installation guide](docs/INSTALL.md) for native macOS and Linux packages,
single-command installation, runtime prerequisites and unsigned macOS launch.

## Get started

Build from source using the [setup instructions](CONTRIBUTING.md#set-up).
You need Rust, Zig 0.15.2, a C toolchain, and tmux 3.3 or newer for terminals.
Python is used only to prepare the native source during the build.

```sh
python3 scripts/prepare-native.py
export LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe
cargo run --release -p tmux-workspaces-app --bin tmux-workspaces
```

The first launch creates an empty workspace. Add a tab with **+** or
Command-T, then choose **Open terminal** or **Attach a session**. On Linux,
use Ctrl+Shift+T. Click a terminal before typing.

To open a particular library:

```sh
cargo run --release -p tmux-workspaces-app --bin tmux-workspaces -- --data-dir /path/to/library
```

## Guides

- [Use the app](docs/GUIDE.md): workspaces, sessions, settings, shortcuts and saved libraries.
- [Build and contribute](CONTRIBUTING.md): prerequisites, native builds, tests and pull requests.
- [Architecture](docs/ARCHITECTURE.md): processes, crate boundaries and data flow.
- [Design](docs/DESIGN.md): appearance, components and design tokens.
- [Run benchmarks](docs/BENCHMARKING.md): repeatable measurements on isolated sessions.

## Platform support

The app is in early development. macOS is the primary platform. Native builds
and automated tests have been verified on Ubuntu 24.04 (ARM64) with tmux 3.4.
Linux has Wayland and X11 backends, but desktop interaction has not yet been
fully verified there. Windows is not currently supported by the app.

Terminal input methods, accessible terminal text, key-release events and
selection across history pages are not yet implemented. See the
[user guide](docs/GUIDE.md) for supported interactions and limits.

## Licence

MIT. See [LICENSE](LICENSE).
