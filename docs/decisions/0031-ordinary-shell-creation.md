# 0031. Ordinary-shell creation and shared locking

- **Status:** Implementation and compatibility verification; desktop activation pending.
- **Date:** 2026-10-21

## Decision

Create ordinary shells through tmux using the Python library's existing socket,
`terminal-<leaf-id>` identity and append-open `<socket>.viewer-lock`. Hold an
exclusive flock across listing, checking, creation and initial options. Reuse
existing sessions without changing their process, directory, environment or
options, including retained dead panes. Never replace or unlink the lock file.
The socket parent must already be a private, owned, nonsymlink directory. Reject
unsafe socket and lock files; open the lock nonblocking without following symlinks and verify
that its inode still matches after acquisition. Lock waits have a ten-second
limit and command capture retains its five-second bound, all on a worker.

Use the standard library's `File::try_lock`, available since Rust 1.89. On the
current Unix implementation it uses flock, matching Python's `fcntl.flock`.
Verify that interoperability with separate processes in both directions; the
[standard-library documentation](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)
notes that platform implementations may change. The current toolchain is pinned.

Capture the creating window's allowed command environment and SSH/XDG context.
A shell command explicitly unsets TMUX, TMUX_PANE, TMUX_TMPDIR and every context key, then
restores only present values, preserving empty versus absent. Quote each shell
argument literally, including apostrophes, whitespace and semicolons; escape a
terminal semicolon in tmux CLI argument data separately. The tmux subprocess
receives only the reference command allowlist and LC_* variables, with an
explicit socket argument. Test builds also supply their required private
TMUX_TMPDIR; ordinary children do not inherit it. Other launcher variables are
excluded from the snapshot and command environment.

Match the pinned reference's SHELL or `/bin/sh` fallback, login/interactive flags,
and saved cwd or process cwd, falling back to the home directory when invalid.
New sessions use no user tmux configuration, status off, mouse on and
remain-on-exit on. This does not adopt the separate PATH shell-discovery proposal
in [0016](0016-shell-environment-and-launch-context.md), add terminal environment defaults, or enable
implicit missing-shell startup. Desktop activation requires the compatibility
proof below. The explicit Open terminal action first saves an ordinary leaf
through the existing conflict rules. Save failures retain the proposal for retry;
conflicts or discarding an edit cancel startup. Creation runs on a worker only
while the saved destination is still displayed and ordinary, then connects its
native terminal. Switching libraries clears pending startup. Reconnect and normal
navigation never create a missing shell.

## Dependencies and alternatives

The tmux crate directly reuses existing workspace dependencies: `libc` 0.2.191
(MIT OR Apache-2.0) for O_NOFOLLOW and O_NONBLOCK, and `uzers` 0.12.2 (MIT, pinned exactly) for
current UID and home-directory lookup. Versions and licences were checked against
installed crate metadata. No additional locking library or unsafe code is needed.

A process-local mutex cannot coordinate Python and Rust windows. A different
lock name or replaced inode can produce duplicate shells. Reusing the server's
old SSH/XDG environment can send a new terminal to stale sockets. Respawning an
existing dead pane would discard its retained output and change reference behavior.

## Verification boundary

Use only test-support-owned private tmux servers and disposable fake login shells
that record argv, cwd and selected environment fields without reading user startup
files. Verify one session/PID for competing Python and Rust creators, both lock
orders, distinct leaves, changed/absent/empty context, cwd and shell fallbacks,
literal paths, existing-session survival, options and lock release after failure.
Record the reference revision and actual process results. Independent review and
reference-maintainer compatibility review precede desktop activation.

Reference: Python `51de675`, `shells.py`, `tmux.py` and the library socket rules in
[0009](0009-python-library-and-shell-server-compatibility.md).
