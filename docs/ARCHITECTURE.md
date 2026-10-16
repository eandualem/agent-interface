# Architecture

tmux-workspaces is a native desktop window for working in tmux. A sidebar holds
workspaces, tabs and an optional agents roster; the rest of the window shows
terminal panes that the app draws itself. tmux runs every shell, and the app is
a tmux client.

The code is not written yet. This page describes the design decided so far and
links the decision record behind each part. It will describe the code as the
code lands.

## The design at a glance

**Processes.** tmux servers own every shell, terminal and session, so shells
keep running when the app closes. The app runs no daemon. It starts tmux
control-mode clients (`tmux -C`) and one-off tmux commands, and may start the
library's shell server, which then belongs to tmux
([0002](decisions/0002-tmux-is-the-only-process-layer.md)).

**Connections.** Each leaf of a workspace layout refers to a tmux session: an
ordinary shell on the library's shell server, or an attached session. The app
keeps one control-mode connection per attachment and shows the session's
selected window with that window's own panes
([0003](decisions/0003-control-mode-connection-and-attachment-model.md)).

**Crates.** Six crates with a one-way dependency direction
([0006](decisions/0006-workspace-and-crate-layout.md)):

| Crate | Responsibility |
| --- | --- |
| `core` | Workspace, tab and layout model, validation and merge rules |
| `store` | Reading `layouts.db`, the library shared with the Python tool |
| `tmux` | Control-mode protocol and connections |
| `term` | Terminal state for each pane, around the terminal engine |
| `roster` | Optional, read-only agents roster |
| `app` | The window, workers and the wiring between the other crates |

**From pane output to a frame.** tmux reports pane output on a control
connection. The `tmux` crate decodes it, and a terminal worker feeds the bytes
to that pane's terminal state in `term`. The worker publishes owned display
data, which the user-interface thread draws
([0007](decisions/0007-threads-ownership-and-event-ordering.md)).

**From a key to a pane.** The app reads every key. Its own shortcuts run
natively; other input is encoded for the pane and sent to tmux as literal bytes
for an exact pane. After a navigation, input waits until the destination is
ready ([0007](decisions/0007-threads-ownership-and-event-ordering.md),
[0015](decisions/0015-input-routing.md)).

**The shared library.** A storage worker reads `layouts.db` read-only and
watches it for changes made by the Python tool. Writing waits until the Python
tool's merge and conflict rules are ported and tested
([0009](decisions/0009-python-library-and-shell-server-compatibility.md)).

## Decision records

Each record states what was chosen, the options considered and the evidence.
A proposed record names the measurement that settles it. How records work is
described in [0001](decisions/0001-record-architecture-decisions.md).

| Record | Status |
| --- | --- |
| [0001. Record architecture decisions](decisions/0001-record-architecture-decisions.md) | Accepted |
| [0002. tmux is the only process layer](decisions/0002-tmux-is-the-only-process-layer.md) | Accepted |
| [0003. Control-mode connection and attachment model](decisions/0003-control-mode-connection-and-attachment-model.md) | Accepted |
| [0004. Terminal emulation core](decisions/0004-terminal-emulation-core.md) | Proposed |
| [0005. UI framework and terminal renderer](decisions/0005-ui-framework-and-terminal-renderer.md) | Proposed |
| [0006. Workspace and crate layout](decisions/0006-workspace-and-crate-layout.md) | Accepted |
| [0007. Threads, ownership and event ordering](decisions/0007-threads-ownership-and-event-ordering.md) | Accepted |
| [0008. Background I/O runtime](decisions/0008-background-io-runtime.md) | Proposed |
| [0009. Compatibility with the Python tool's library and shell server](decisions/0009-python-library-and-shell-server-compatibility.md) | Accepted |
| [0010. Agents roster adapter](decisions/0010-agents-roster-adapter.md) | Accepted |
| [0011. Toolchain and dependency policy](decisions/0011-toolchain-and-dependency-policy.md) | Accepted |
| [0012. Testing strategy and tmux isolation](decisions/0012-testing-strategy-and-tmux-isolation.md) | Accepted |
| [0013. Milestone-0 benchmark method](decisions/0013-milestone-0-benchmark-method.md) | Accepted |
| [0014. Supported tmux versions](decisions/0014-supported-tmux-versions.md) | Proposed |
| [0015. Input routing](decisions/0015-input-routing.md) | Proposed |
| [0016. Shell environment and launch context](decisions/0016-shell-environment-and-launch-context.md) | Proposed |

## Further reading

- [Architecture research](research/README.md): the evidence behind the
  records, with the alternatives and licences examined.
- [Parity checklist](PARITY.md): every behaviour of the Python tool and what
  this app does with it.
