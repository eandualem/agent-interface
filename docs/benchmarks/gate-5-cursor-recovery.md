# Gate 5: cursor recovery follow-up

**Result: passed.** All 18 full runs pass on tmux 3.3a, 3.4, 3.5a,
3.6b, 3.7c and 3.8. Across 4,482 comparisons, 3,936 match exactly and 546
differ only as existing capture limitations explain. There are no unexplained differences or failed
fixture runs. All 573 checks and all 360 forced pauses pass.
[Decision 0021](../decisions/0021-pane-state-recovery.md) is accepted for
pane-state recovery. Gate 1 and the outstanding display gates still keep
milestone 0 open; this result does not establish performance or authorize
product UI work.

The [original failed matrix](gate-5.md) remains intact. Its missing byte
trace prevents proving that run's exact cause retrospectively. The new
fixtures reproduce the independently demonstrated cursor-state loss and
check its correction without adding an expected-limit allowance.

## Method and environment

Three full runs per version on 18 October 2026, using
`tmux-workspaces-bench gate5 --out DIR`, from clean committed revision
`771fa92e7eeb9e05b4da930e2bbaa904501a8d87`. Every run uses the same release
binary, covers all nine scenarios and includes 20 stalls that trigger tmux's
own pause. The [benchmark guide](README.md#running-gate-5) gives build and
run commands. The [run metadata](gate-5-cursor-recovery/runs.json) records
the binary's SHA-256, concurrency, timestamps, load averages and exit status.

| Item | Value |
| --- | --- |
| Machine | Apple M5, 10 cores (4 performance, 6 efficiency), 16 GiB |
| System | macOS 27.0.1 (26A434) |
| Power | Battery power, low-power mode off in every run |
| Rust | 1.99.1, release profile, opt-level 3, no debug information or debug assertions |
| Zig | 0.15.2 |
| Terminal engine | `libghostty-vt` 0.2.2, pinned Ghostty source from decision 0004, `ReleaseFast`, SIMD on, portable `baseline` CPU target |
| Reference | Ordinary tmux client in a pseudo-terminal, `TERM=xterm-256color`; no graphical terminal or display involved |
| Load | At most three independent fidelity runs at once, each on a private server; each run reported no compiler or build tool running at its start |

These compare state, not throughput, typing latency or frame pacing. Each
run's report records its environment. Flow-check counts vary with the number
of recoveries completed within that scenario's fixed duration.

## Results

Each row combines three runs, 747 comparisons per version. The linked
reports and their adjacent `raw.tsv` files are byte-for-byte copies of the
harness output.

| tmux | Runs passed | Exact | Documented limits | Unexplained | Checks passed | Own pauses / stalls | Reports |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 3.3a | 3/3 | 648 | 99 | 0 | 93/93 | 60/60 | [1](gate-5-cursor-recovery/3.3a/run-1/summary.md) / [2](gate-5-cursor-recovery/3.3a/run-2/summary.md) / [3](gate-5-cursor-recovery/3.3a/run-3/summary.md) |
| 3.4 | 3/3 | 672 | 75 | 0 | 96/96 | 60/60 | [1](gate-5-cursor-recovery/3.4/run-1/summary.md) / [2](gate-5-cursor-recovery/3.4/run-2/summary.md) / [3](gate-5-cursor-recovery/3.4/run-3/summary.md) |
| 3.5a | 3/3 | 672 | 75 | 0 | 93/93 | 60/60 | [1](gate-5-cursor-recovery/3.5a/run-1/summary.md) / [2](gate-5-cursor-recovery/3.5a/run-2/summary.md) / [3](gate-5-cursor-recovery/3.5a/run-3/summary.md) |
| 3.6b | 3/3 | 648 | 99 | 0 | 95/95 | 60/60 | [1](gate-5-cursor-recovery/3.6b/run-1/summary.md) / [2](gate-5-cursor-recovery/3.6b/run-2/summary.md) / [3](gate-5-cursor-recovery/3.6b/run-3/summary.md) |
| 3.7c | 3/3 | 648 | 99 | 0 | 99/99 | 60/60 | [1](gate-5-cursor-recovery/3.7c/run-1/summary.md) / [2](gate-5-cursor-recovery/3.7c/run-2/summary.md) / [3](gate-5-cursor-recovery/3.7c/run-3/summary.md) |
| 3.8 | 3/3 | 648 | 99 | 0 | 97/97 | 60/60 | [1](gate-5-cursor-recovery/3.8/run-1/summary.md) / [2](gate-5-cursor-recovery/3.8/run-2/summary.md) / [3](gate-5-cursor-recovery/3.8/run-3/summary.md) |

## What changed and what remains limited

A resize can retain a logical cursor beyond the new width. Recovery keeps
that state in an authoritative snapshot and refreshes after output until
the cursor can be streamed through the engine again. A streaming engine's
pending-wrap flag also triggers a refresh because it is ambiguous when
wrapping is disabled. Recovery replies and retries share one deadline.

The harness compares the candidate's logical cursor with tmux's formats,
and separately checks its projected ordinary-client position against the
reference. Cells, modes and continuation checks remain strict. New fixtures
cover off-grid printing, backspace, cursor-left, wrap disabling and output
that reaches the last column with wrapping already disabled. The original
redraw fixture is included in every full run. All new fixture comparisons
match without an allowance.

Every resize comparison now saves its snapshot, exact post-seed replay
bytes, control events and reference bytes in the run's `resize/` directory,
including successful comparisons. Full diagnostic output was retained for
this matrix; the comparison reports and raw tables are included here.

The existing [capture limitations](../decisions/0021-pane-state-recovery.md#decision)
remain: state tmux does not export, old-version modes, hyperlink and tab-cell
capture losses, and the saved primary screen after an alternate-screen
resize. No new limitation was added for the cursor fix. Modes unavailable
from tmux or the engine remain explicitly unmeasured. Soft-wrap metadata,
scrollback equivalence and physical pixels are outside this comparison.

Refreshing a snapshot captures the retained history and may add latency or
lose unexported state such as the current text style. These results validate
the tested fidelity cases, not a latency bound; gates 2–4 still need to
measure that cost.
