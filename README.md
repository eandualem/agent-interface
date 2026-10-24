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

Install a native package using the [installation guide](docs/INSTALL.md), then
open the app. Packages need no Rust, Zig or Python. Terminal sessions require
tmux 3.3 or newer; Agent Backbone is optional.

The first launch creates an empty workspace. Add a tab with **+** or
Command-T, then choose **Open terminal** or **Attach a session**. On Linux,
use Ctrl+Shift+T. Click a terminal before typing.

To open another saved library, choose **Open Library…** from the application
menu and select its folder.

This release uses tmux sessions on the same computer. It does not connect to
sessions on another host or synchronize live sessions between computers.

To build from source, follow [Contributing](CONTRIBUTING.md#set-up).

## Guides

- [Use the app](docs/GUIDE.md): workspaces, sessions, settings, shortcuts and saved libraries.
- [Build and contribute](CONTRIBUTING.md): prerequisites, native builds, tests and pull requests.
- [Architecture](docs/ARCHITECTURE.md): processes, crate boundaries and data flow.
- [Design](docs/DESIGN.md): appearance, components and design tokens.
- [Run benchmarks](docs/BENCHMARKING.md): repeatable measurements on isolated sessions.

## Platform support

The app runs natively on macOS and Linux, with Wayland and X11 backends on Linux.
See the [installation guide](docs/INSTALL.md) for package architectures and
runtime requirements. Native Windows is not supported. WSL has not been verified.

Terminal input methods, accessible terminal text, key-release events and
selection across history pages are not yet implemented. See the
[user guide](docs/GUIDE.md) for supported interactions and limits.

## Licence

MIT. See [LICENSE](LICENSE).
