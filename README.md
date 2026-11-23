<div align="center">

<img src="packaging/icon/agent-interface.svg" alt="" width="96">

# Agent Interface

**The native desktop for your agent team.**

Run Claude Code, Codex, OpenCode and Antigravity agents in real terminals, see at a glance
which one needs you, and read their reports and documents beside the work.

[![Release](https://img.shields.io/github/v/release/eandualem/agent-interface)](https://github.com/eandualem/agent-interface/releases)
[![CI](https://github.com/eandualem/agent-interface/actions/workflows/ci.yml/badge.svg?branch=develop)](https://github.com/eandualem/agent-interface/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[Website](https://agentinterface.dev) · [Install](#install) · [User guide](docs/GUIDE.md) · [Architecture](docs/ARCHITECTURE.md) · [Security](SECURITY.md) · [Contribute](CONTRIBUTING.md)

</div>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/workspace-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/media/workspace-light.png">
    <img alt="The Agent Interface window: a sidebar of tabs and agents with three needing attention, a builder agent's terminal, a reviewer agent asking a question, and the Skills view." src="docs/media/workspace-light.png">
  </picture>
</p>
<p align="center"><em>Illustration with example data.</em></p>

## One system for agent work

Agent Interface and Agent Backbone
work together. Backbone turns agents across CLIs and projects into one team, with
names, safe messages, live state and continuity. Agent Interface is where you work
with that team.

- **Know who needs you.** Active agents show their state, such as working, idle
  or waiting on you. Click one to go to the tab that shows its session.
- **Real terminals.** Panes are drawn natively with GPUI and Ghostty's terminal
  core. tmux keeps every process running, and you can attach the sessions you
  already use.
- **The team beside the work.** Open Team, Activity, Usage, Skills, Policies and
  Tags views in a pane, next to the agents they describe.
- **Reports, answers and documents.** **Reports & updates** collects progress
  reports, saved answers and Documents. Reply to the author from the record you
  are reading.
- **Workspaces that come back.** Tabs and split panes are saved in workspaces.
  Jump (⌘K) finds tabs, panes, agents and commands.

Without Backbone, Agent Interface is still a native workspace for tmux.

```mermaid
flowchart LR
  you([You]) --> app[Agent Interface]
  app -- terminals --> tmux[tmux]
  app -- "team, reports, documents" --> backbone[Agent Backbone]
  backbone -- "messages and state" --> agents["Agents: Claude Code, Codex, OpenCode, Antigravity"]
  tmux --- agents
```

## Install

You need macOS or Linux and tmux 3.4 or newer (`brew install tmux` on macOS).
On Linux, install the [runtime libraries](docs/INSTALL.md#runtime-prerequisites) first.

**1. Install Agent Interface.** The installer downloads the compiled app from the
[latest release](https://github.com/eandualem/agent-interface/releases) and checks
its checksum. No Rust installation is required. It does not install Agent
Backbone yet; see [connecting Agent Backbone](docs/INSTALL.md#connect-agent-backbone).

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh | sh
```

**2. Open Agent Interface.** With Agent Backbone installed, native onboarding
creates your first profile, signs you in to an agent tool and starts your first
agent, its guide. Without it, first run says so and **Continue with terminals**
opens the workspace.

The [installation guide](docs/INSTALL.md) covers package architectures, manual
downloads, Windows 11 / WSL setup, updates and removal.

## Get started

1. Press **⌘T** (Ctrl+Shift+T on Linux), then choose **Open terminal** or pick a
   running tmux session to attach it.
2. Split the pane with **⌘D**, or **⌘⇧D** to split below (Ctrl+Shift+D and
   Ctrl+Shift+E on Linux).
3. In a workspace with an Agent Backbone profile (see
   [Connect Agent Backbone](docs/INSTALL.md#connect-agent-backbone)), choose a
   view such as **Reports & updates**, or **Team** under **Skills, usage, team…**,
   in an empty pane.
4. In a workspace with a profile, once an agent's session is in a tab, click the
   agent in the sidebar to go there.

The [user guide](docs/GUIDE.md) covers every view, shortcut and setting.

## Documentation

- [User guide](docs/GUIDE.md): workspaces, sessions, agent views, reports, shortcuts and libraries.
- [Installation](docs/INSTALL.md): packages, prerequisites, Backbone connection, updates and removal.
- [Architecture](docs/ARCHITECTURE.md): processes, crate boundaries, data flow and dependencies.
- [Security](SECURITY.md): the security model, how the app is tested, open security work and how to report a vulnerability.
- [Design](docs/DESIGN.md): appearance, components and design tokens.
- [Benchmarks](docs/BENCHMARKING.md): repeatable measurements on isolated tmux servers.
- [Contributing](CONTRIBUTING.md): building from source, tests and pull requests.

## Status

Agent Interface runs on macOS and Linux (X11, or XWayland on Wayland desktops). Windows 11 runs the
Linux package through WSL 2 with WSLg. Terminals and tmux sessions run on the
same computer as the app; agents on a profile's other machines open in native
panes too. Rich HTML Documents render on macOS and Linux, including Windows through WSLg.

Not yet implemented: terminal input methods, key-release events and selection
across history pages.

Agent Interface began as a Rust reimplementation of
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces).

## Licence

MIT. See [LICENSE](LICENSE).
