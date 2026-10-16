# 0016. Shell environment and launch context

- **Status:** Proposed. Waits on the launch and environment tests described
  under **Accept when**.
- **Date:** 2026-10-17

## Context

The Python tool always starts from a terminal, so it inherits the user's shell
environment. It passes tmux only an allowlist of variables (`PATH`, `HOME`,
`USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `TMPDIR`, `COLORTERM`, `TERMINFO`,
`TERMINFO_DIRS` and `LC_*`). Each new shell is started with
`env -u TMUX -u TMUX_PANE`, with every supported SSH and XDG context variable
(`SSH_AUTH_SOCK` and seven `XDG_*` names) cleared and then set again only from
the window that creates it, followed by the user's shell with `-l -i`. Missing
values stay unset, so the persistent server cannot hand a new shell a stale
agent socket.

A desktop app started from the Dock or a desktop launcher gets a minimal
environment instead. Its `PATH` usually lacks the directory where tmux is
installed, and it has no terminal of its own. Parity rows: SH-2 and SU-5 are
open questions about exactly this, and CH-3 asks which directory a new shell
starts in ([SH-1 to SH-10](../PARITY.md#shell-environment-and-process-lifetime),
[SU-5](../PARITY.md#startup-preflight-and-recovery),
[CH-3](../PARITY.md#choosers-and-attaching-existing-sessions)).

## Options considered

| Option | Trade-offs |
| --- | --- |
| Use the launcher's environment as it is | Correct when started from a terminal. From the Dock, tmux is often not found and shells get a different `PATH` from the user's terminals. |
| **Ask the user's login shell for its `PATH` once at startup** | Matches what the user's terminals see without guessing install locations. Runs the user's startup files, so it needs a timeout, an output limit and careful parsing. |
| Search a list of known install locations for tmux | Simple, but guesses, and does not give shells the user's `PATH`. |
| A setting for the tmux path or `PATH` | Explicit, but every user has to configure it before the app works. |

## Decision

1. **Snapshot the launcher once.** At startup, keep only the allowlisted
   variables above and the SSH and XDG context variables. A variable that is
   absent stays absent.
2. **Resolve `PATH` from the login shell.** Use the user's shell from `SHELL`
   as a validated absolute path. Run it once as a login, interactive shell,
   with the filtered snapshot as its environment and without `TMUX` or
   `TMUX_PANE`, a fixed timeout and an output limit, and read `PATH` from a
   framed result. Keep only `PATH` from that run, not the rest of the shell's
   environment.
3. **Find tmux in that `PATH`.** If the shell is missing, the run times out,
   the output is malformed or tmux is not found, show the reason (SU-4) and
   leave shell creation disabled.
4. **Create shells as the Python tool does**, with the context from the
   snapshot in step 1. Never recover SSH or XDG values from the tmux server's
   environment ([0009](0009-python-library-and-shell-server-compatibility.md)).
5. **Give tmux control and discovery commands the narrower environment** without
   the SSH and XDG context, as the Python tool does (SH-10).

Not decided here: what `TERM` and `COLORTERM` mean when the app has no outer
terminal (the second half of SH-2), and which directory a new shell starts in
when the app was not launched from a shell (CH-3). Until those are decided,
absent values stay absent and the app does not invent them.

## Consequences

- Launching from the Dock and from a terminal give shells the same `PATH`. A
  terminal launch also uses the login shell's `PATH` rather than the launching
  terminal's, which differs from the Python tool when that terminal changed
  `PATH` after login.
- A slow or noisy shell startup file delays startup only up to the timeout,
  and then the app reports it instead of hanging.
- The resolver runs the user's startup files once per app launch; it must
  never run them with secrets or variables outside the allowlist.

## Accept when

- Launch from the Dock or a desktop launcher and from a terminal, on macOS and
  Linux, and get the same `PATH` and tmux.
- Shell startup files that print output, hang or fail are handled within the
  timeout, with a clear message.
- After reconnecting with changed or absent SSH and XDG context, new shells get
  the current values and existing shells keep theirs (SH-7, SH-8).
- Two app windows, and an app window beside a Python window, create shells
  without disturbing each other, using the shared creation lock
  ([0009](0009-python-library-and-shell-server-compatibility.md)).

## Evidence

- [Existing shell-server compatibility](../research/tmux-control-mode.md#existing-shell-server-compatibility)
  in the control-mode research, which sets out the Python creation policy and
  this acquisition candidate, and notes that the candidate has not been tested.
- [Parity checklist](../PARITY.md#shell-environment-and-process-lifetime),
  rows SH-1 to SH-10, SU-5 and CH-3.
