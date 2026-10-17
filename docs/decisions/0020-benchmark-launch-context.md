# 0020. Benchmark launch context

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

Gates 2 to 4 ([0018](0018-milestone-0-benchmark-method-revised.md)) compare
the benchmark window ([0019](0019-benchmark-window.md)) with Ghostty. The
harness started Ghostty through LaunchServices (`open`), as the system starts
any application, but started the benchmark window and the private tmux
servers itself.

On macOS a process keeps the scheduling of the process that started it, and
the programs a tmux server's panes run keep the server's. So the app's side
of each comparison ran with whatever scheduling the harness had, while
Ghostty ran with what macOS gives an application. Started from a process that
a background launchd job started, the harness ran at priority 20 with heavy
timer coalescing, and so did everything it started (see **Evidence**). From
an ordinary terminal it would run at the default priority of 31, but Ghostty
still gets an application's 46.

## Options considered

| Option | Trade-offs |
| --- | --- |
| The harness starts them (before) | Simple. The app's scheduling depends on where the harness was started, and never matches Ghostty's. |
| Change the scheduling from the harness with `taskpolicy` | Does not work: a program started this way still ran at priority 20 with the same timer coalescing. |
| Run the whole harness as an application that LaunchServices starts | Every process, the harness included, gets ordinary scheduling. But the capture and the keys then need that application's own Screen Recording and Accessibility permissions, granted by hand, and its output and exit status pass through `open`. |
| **LaunchServices starts the benchmark window and each tmux server** | Each measured program is started as Ghostty is, wherever the harness runs, with no new permissions: capture and keys stay in the harness. The window runs from a copy in a temporary application bundle, and its output passes through files. |

## Decision

For gates 2 to 4, the harness starts every measured program and every private
tmux server through LaunchServices.

- **The benchmark window** runs from a copy of the `bench-window` example in a
  temporary application bundle marked `LSUIElement`, an accessory application
  like the window itself. `open -g -n` starts it without bringing it to the
  front. Its standard output and error go to files, where the harness reads
  its `ready` line. It is found and stopped by its exact process, as Ghostty
  is.
- **Each private tmux server** is started by a temporary background-only
  application (`LSBackgroundOnly`), which `open -W` runs until it has run the
  server's start command and exited. The server, and the programs its panes
  run, then have the scheduling of an ordinary process, as the programs that
  Ghostty starts do. The isolation helper still builds that command
  (`TmuxServer::start_with`), so tmux isolation stays reviewable in one file
  ([0012](0012-testing-strategy-and-tmux-isolation.md)).
- **Their environment** is the user session's, as Ghostty's is, plus the
  harness's `PATH` and locale (`LANG`, `LC_ALL` and `LC_CTYPE`). The harness
  does not pass its whole environment, because `open` would show every
  variable on its command line.
- **The harness itself**, which captures the window and posts the keys, keeps
  its own scheduling. It is the same for every target.

## Consequences

- The app's window and Ghostty run with the same application priority, and
  their tmux servers and pane programs at the default priority, wherever the
  harness is started.
- A signal to the harness or its process group no longer reaches the window
  or Ghostty, so an interrupt first stops every program the harness started
  through LaunchServices, and any that is starting, and then the tmux servers.
- Each run's directory holds a copy of the window program, about 10 MB, and a
  small bundle that starts tmux. LaunchServices registers both, as it does any
  application that runs.
- Gate 1 is unchanged: the tmux server and the replays it compares both run in
  the harness's context.

## Revisit when

- The gates run on Linux, which has no LaunchServices.
- The harness's own scheduling shows up in a result, for example as frames
  the capture drops.

## Evidence

Observed on the benchmark machine (Apple M5, macOS 27.0.1, on battery) on
17 October 2026, with `ps -o pri`. The late wake-up is the median, over 60
tries, of how much later than asked a 14.7 ms sleep returned, measured in a
pane.

| Started by | Process | Priority | Late wake-up |
| --- | --- | --- | --- |
| The harness, started from a background job | tmux server and its pane | 20 | 53 ms |
| LaunchServices | tmux server and its pane | 31 | 2.4 ms |
| The harness, started from a background job | Benchmark window, all 14 threads | 20 | |
| LaunchServices | Benchmark window | 46 on its main thread, 31 to 54 on others | |
| The user, through LaunchServices | Ghostty | 46 | |

During a gate-3 run with this change, sampled once a second, the benchmark
window ran at 46 to 54. Its tmux server, the pane programs and its control
client ran at 31. The harness stayed at 20.
