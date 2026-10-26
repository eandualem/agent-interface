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
- Revisit reports, answers and durable rich documents in **Reports & updates**
  through Agent Backbone. Rich HTML rendering currently requires macOS.
- Detach a session to reuse its pane, or close a tab while tmux manages its sessions.

The app draws terminals natively and needs no Python runtime. It can open
existing schema-v2 libraries from
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces).

The executable, data paths and Linux installation paths remain `tmux-workspaces`
for compatibility. The product, macOS app and repository are **Agent Interface**.

## Get started

Agent Interface runs on macOS and Linux and needs tmux 3.3 or newer. On macOS,
install tmux with `brew install tmux`.

To build and launch the app, install the
[build prerequisites](CONTRIBUTING.md#set-up), then run:

```sh
git clone https://github.com/eandualem/agent-interface.git
cd agent-interface
python3 scripts/prepare-native.py
export LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe
cargo run -p tmux-workspaces-app
```

To install an archive you already have, follow the
[installation guide](docs/INSTALL.md). It covers platform requirements,
installation, updates and removal.

Agent views connect to a running Agent Backbone. Authenticated access requires
Backbone's private Unix listener. The app reads the provider's saved
`backbone.socket_path` setting; `BACKBONE_SOCKET_PATH` in the app's launch
environment can override it. The socket path must be absolute. The socket must be
owned by your user with permissions 0600, in a directory owned by your user with
permissions 0700. TCP-only configurations do not receive credentials.

The first launch creates an empty workspace. Add a tab with **+** or
Command-T, then choose **Open terminal** or **Attach a session**. On Linux,
use Ctrl+Shift+T. Click a terminal before typing.
Close the selected tab with Command-W, or Ctrl+Shift+W on Linux.

To open another saved library, open Settings, choose **Open another**, and
select its folder. The shortcut is Command-O on macOS or Ctrl+Shift+O on Linux.

This release uses tmux sessions on the same computer. It does not connect to
sessions on another host or synchronize live sessions between computers.

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
