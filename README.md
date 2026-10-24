# Agent Interface

**A native workspace for your agents and terminals.**

Run agents and shells in full native terminals, arranged in saved workspaces,
tabs and split panes. Open live views of your agents' work right beside them,
and attach the tmux sessions you already use without disconnecting. tmux keeps
every process running; Agent Interface is the desktop around them.

[Get started](#get-started) · [User guide](docs/GUIDE.md) · [Contribute](CONTRIBUTING.md) · [Architecture](docs/ARCHITECTURE.md)

## What you can do

- Arrange terminal panes in saved tabs and workspaces.
- Attach existing tmux sessions and switch between their windows.
- Search tabs, panes and commands with Jump.
- Choose a light or dark theme and customize keyboard shortcuts.
- Follow live agent states, with animation while agents work.
- Open Team, Activity, Usage, Skills, Policies and Tags views beside your
  terminals, or inspect a single agent, through the built-in
  [Agent Backbone](https://github.com/eandualem/agent-backbone) integration.
- Detach a session to reuse its pane, or close a tab while tmux manages its sessions.

The app draws terminals natively and needs no Python runtime. It can open
existing schema-v2 libraries from
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces).

The executable and existing installation and data paths remain `tmux-workspaces`
for compatibility. The product and repository are **Agent Interface**.

## Get started

Native release downloads are being prepared. To run the app now, follow the
[source setup](CONTRIBUTING.md#set-up) for Rust, Zig 0.15.2 and the platform
libraries, then:

```sh
git clone https://github.com/eandualem/agent-interface.git
cd agent-interface
python3 scripts/prepare-native.py
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe cargo run -p tmux-workspaces-app
```

Terminal sessions require tmux 3.3 or newer. Agent views connect to a running
Agent Backbone.
The [installation guide](docs/INSTALL.md) covers native packages, prerequisites,
updates and macOS first launch once release downloads are available.

The first launch creates an empty workspace. Add a tab with **+** or
Command-T, then choose **Open terminal** or **Attach a session**. On Linux,
use Ctrl+Shift+T. Click a terminal before typing.
Close the selected tab with Command-W, or Ctrl+Shift+W on Linux.

To open another saved library, open Settings, choose **Open another**, and
select its folder. The shortcut is Command-O on macOS or Ctrl+Shift+O on Linux.

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
