# Gate 5: control-mode fidelity

**Result: failed.** Across 3,600 comparisons in 18 full runs, 1
comparison differed in a way the recovery decision does not explain.
17 runs passed and 1 failed. The unexplained difference is tracked in
[issue #24](https://github.com/eandualem/tmux-workspaces-rs/issues/24).
[Decision 0021](../decisions/0021-pane-state-recovery.md) remains proposed.
This result does not pass milestone 0 or authorize product UI work.

## Method and environment

Three full runs on each of tmux 3.3a, 3.4, 3.5a, 3.6b, 3.7c and 3.8,
on 18 October 2026, using the defaults of
`tmux-workspaces-bench gate5 --out DIR`. Each run covers all nine scenarios
and all fixtures, including 20 stalls intended to trigger tmux's own pause.
The [benchmark guide](README.md#running-gate-5) gives the build and run commands;
[0021](../decisions/0021-pane-state-recovery.md#decision) defines the comparison
and its explicit allowances for state tmux cannot export.

The binary was built from clean committed revision `1e475fd11247faae4672948f16c0742214864a94`.
All runs used that same binary. A later Linux lint fix passes the window size
to `openpty` through a raw pointer; it passes the same pointer on macOS and
does not change the measured behaviour.

| Item | Value |
| --- | --- |
| Machine | Apple M5, 10 cores (4 performance, 6 efficiency), 16 GiB, AC power, low-power mode off |
| System | macOS 27.0.1 (26A434) |
| Rust | 1.99.1, release profile, opt-level 3, no debug information or debug assertions |
| Zig | 0.15.2 |
| Terminal engine | `libghostty-vt` 0.2.2, pinned Ghostty source from decision 0004, `ReleaseFast`, SIMD on, portable `baseline` CPU target |
| Reference | Ordinary tmux client in a pseudo-terminal, `TERM=xterm-256color`; no graphical terminal or display involved |
| Load | Up to three independent fidelity runs at once, each on its own private server; each run reported no compiler or build tool running at its start |

The fixture sizes vary by scenario, as 0021 specifies: single panes start at
40×8, split windows at 81×19, and resize steps end at 36×9. These are state
comparisons, not measurements of throughput, latency or frame pacing. Numeric
system load averages were not retained; concurrency is recorded above. No
acceptance claim is made from the failed matrix.

## Results

Counts below combine the three runs for each version, 600 comparisons per
row. Each linked report contains every comparison and check; `raw.tsv` beside
it contains every comparison in tabular form. Check counts vary because the
flow scenario performs as many recoveries as fit in its fixed duration.

| tmux | Runs passed | Exact | Documented limits | Unexplained | Checks passed | Own pauses / stalls | Reports |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 3.3a | 3/3 | 501 | 99 | 0 | 93/93 | 60/60 | [1](gate-5/3.3a/run-1/summary.md) / [2](gate-5/3.3a/run-2/summary.md) / [3](gate-5/3.3a/run-3/summary.md) |
| 3.4 | 2/3 | 524 | 75 | 1 | 97/97 | 60/60 | [1](gate-5/3.4/run-1/summary.md) / [2](gate-5/3.4/run-2/summary.md) / [3](gate-5/3.4/run-3/summary.md) |
| 3.5a | 3/3 | 525 | 75 | 0 | 93/93 | 60/60 | [1](gate-5/3.5a/run-1/summary.md) / [2](gate-5/3.5a/run-2/summary.md) / [3](gate-5/3.5a/run-3/summary.md) |
| 3.6b | 3/3 | 501 | 99 | 0 | 95/95 | 60/60 | [1](gate-5/3.6b/run-1/summary.md) / [2](gate-5/3.6b/run-2/summary.md) / [3](gate-5/3.6b/run-3/summary.md) |
| 3.7c | 3/3 | 501 | 99 | 0 | 99/99 | 60/60 | [1](gate-5/3.7c/run-1/summary.md) / [2](gate-5/3.7c/run-2/summary.md) / [3](gate-5/3.7c/run-3/summary.md) |
| 3.8 | 3/3 | 501 | 99 | 0 | 96/96 | 60/60 | [1](gate-5/3.8/run-1/summary.md) / [2](gate-5/3.8/run-2/summary.md) / [3](gate-5/3.8/run-3/summary.md) |

## Unexplained resize cursor difference

In tmux 3.4 run 1, `resize / redraw` after the rebuild at 36×9 had identical
cells but different cursor positions: rebuilt `(35, 5)`, ordinary-client
reference `(0, 0)`. The comparison after more output and input matched.
The failed run is retained in full; repeated success does not discard it.

Ten focused reruns on the same binary, using
`gate5 --scenarios resize --fixtures redraw`, matched both comparisons
([rerun counts](gate-5/redraw-reruns.tsv)). The harness correctly marks those
subset runs incomplete, not a whole-gate pass.

A separate private-server probe on all six versions entered the alternate
screen at 40×6, wrote `R` at row 6, column 40, then resized to 36×9 without
further pane output. Every version kept `cursor_x=40` and `cursor_y=5` after
the shrink ([probe results](gate-5/resize-cursor-probe.json)). Tmux can therefore
retain a cursor beyond the new width. Restoration currently treats a column
at or beyond the width as pending wrap; tmux's ordinary-client path can move
its cursor to the origin when the pane cursor is outside the viewport.

This is a plausible explanation, not proof of the failed run's cause: that
run did not retain its recovery snapshot or reference byte stream. Issue #24
tracks a deterministic reproduction with those artifacts and a resolution.
The comparison has not been relaxed and no new expected limitation has been
added.

## Limits of the comparison

The documented differences are narrowly checked against the fixture, version,
cell properties and region that 0021 permits. They cover current text style,
a partial UTF-8 character, the general saved cursor, the active character set,
a wrapped hyperlink, a moved tab cell, the saved primary screen after an
alternate-screen resize, and the tmux 3.3 background-erased tail. The reports
show the affected cells and cursor, including any fixture that matched despite
an allowed difference.

Bracketed-paste mode is not compared before tmux 3.7, and focus reporting is
not compared before 3.8, because those versions do not export the modes.
Extended-key state is not compared in any version because the engine has no
getter; its restoration is covered separately by the terminal crate's encoder
test. The reference does not measure physical pixels. Soft-wrap metadata and
scrollback history equivalence are outside this visible-pane comparison.
