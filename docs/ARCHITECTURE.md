# Architecture

tmux-workspaces is a native desktop window for working in tmux. A sidebar holds
workspaces, tabs and an optional agents roster; the rest of the window shows
terminal panes that the app draws itself. tmux runs every shell, and the app is
a tmux client.

The code is not written yet. This page describes the process model, crate boundaries and data flow. It will describe the code as the
code lands.

## The design at a glance

**Processes.** tmux servers own every shell, terminal and session, so shells
keep running when the app closes. The app runs no daemon. It starts tmux
control-mode clients (`tmux -C`) and one-off tmux commands, and may start the
library's shell server, which then belongs to tmux.

**Connections.** Each leaf of a workspace layout refers to a tmux session: an
ordinary shell on the library's shell server, or an attached session. The app
keeps one control-mode connection per attachment and shows the session's
selected window with that window's own panes.

**Crates.** Six crates with a one-way dependency direction:

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
data, which the user-interface thread draws.

**From a key to a pane.** The app reads every key. Its own shortcuts run
natively; other input is encoded for the pane and sent to tmux as literal bytes
for an exact pane. After a navigation, input waits until the destination is
ready.

**The shared library.** A storage worker reads `layouts.db` read-only and
watches it for changes made by the Python tool. Writing waits until the Python
tool's merge and conflict rules are ported and tested.

## Further reading

