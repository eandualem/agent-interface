# 0013. Milestone-0 benchmark method

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

No user interface is built until measurements show that the chosen terminal
path can meet the product's speed targets:

- bulk output equal to Ghostty running tmux, because tmux sits on the output
  path of any tmux-based design ([0002](0002-tmux-is-the-only-process-layer.md));
- typing latency and frame pacing equal to plain Ghostty.

The milestone-0 gate issue drafts five gates. This record fixes the method
those gates are measured with, so the results can be trusted and repeated.

The research ran two exploratory probes that are **not** gate evidence:

- a headless engine probe that did no control-mode decoding, kept only the best
  of three runs, did different read-out work per engine, never forced a final
  read-out or compared final states, and used native CPU targeting for one
  engine only ([terminal-core research](../research/terminal-core.md#exploratory-headless-probe));
- a tmux throughput probe that timed producer completion and pipe drain, not a
  displayed frame, on a host that was not quiet
  ([control-mode research](../research/tmux-control-mode.md#local-throughput-evidence)).

Their numbers also show why a fixed tmux throughput figure from elsewhere is
not a usable baseline: tmux's rate depends on the workload and the machine.

## Options considered

| Option | Trade-offs |
| --- | --- |
| Reuse the exploratory probes as the gate | Already exist, but have the gaps listed above. Rejected. |
| Absolute targets only | Simple to state, but meaningless on another machine or workload. |
| **Paired comparisons on one machine, plus a headless headroom check** | Each result is compared with its baseline measured the same way, on the same machine, in the same session. Needs more setup. |

## Decision

**Common rules for every gate.**

- Run on one machine. Record the hardware, OS version, power settings, display
  refresh rate, background load, font and size, and the exact versions of
  tmux, Ghostty, Rust, Zig, the terminal engine and its native source, the
  build profile, `LIBGHOSTTY_VT_SYS_OPTIMIZE` and the CPU targeting mode.
- Use a 200×50 terminal and the same font for the app and the baselines.
- Every tmux process runs on a private server through the isolation helper
  ([0012](0012-testing-strategy-and-tmux-isolation.md)), on disposable
  fixture sessions.
- Generate payloads with a checked-in generator from a fixed seed. Cover at
  least ASCII, mixed multilingual text with wide characters, combining marks
  and emoji sequences, escape-heavy coloured output, and scrolling.
- Do warm-up runs, then enough timed runs to report the median and the
  nearest-rank 95th percentile, not just the best run. Report minimum, maximum,
  peak memory and bytes copied to the user interface where relevant.
- An acceptance result needs at least three independent runs under comparable
  load and power, from a clean build of a committed revision, with no tracing
  and no unrelated builds on the machine. Runs that do not meet this are
  labelled exploratory (PF-9, PF-10).
- The pass or fail decision uses the distributable build configuration
  (portable `baseline` CPU target). Native-CPU results may be reported beside
  it, labelled.
- Producer completion and pipe drain are not frame completion. End-to-end gates
  stop the clock at the presented frame that shows the final marker.

**Gate 1: headless headroom.** Record control-mode traffic from a private tmux
server while it runs the generated payloads. Replay it through control-mode
decoding, the terminal engine, the render-state update and owned display-data
extraction, with a forced final extraction after the last chunk. Every engine
compared produces the same owned display data, and its final screen and cursor
must match the other engines' before its timing counts. Differences from
tmux's own capture of the pane are reported as fidelity findings. Pass: at least 200 MB/s for ASCII on the benchmark machine, and at
least five times tmux's own control-mode throughput for every payload, where
tmux's throughput is the drain rate of the same payload measured on the same
machine in the same session.

**Gate 2: end-to-end bulk output.** vtebench inside a tmux pane, plus a 20 MB
marker probe, in the app and in Ghostty running `tmux attach` on the same kind
of fixture. Pass: at least 0.95 times Ghostty with tmux on every case, with the
final frame no more than one refresh interval later.

**Gate 3: typing latency.** Typometer or a 240 fps camera, at least 40 keys
per condition, idle and with a neighbouring pane streaming output. Pass:
median within 4 ms and 95th percentile within 8 ms of **plain Ghostty**, as
the product target states. Ghostty running tmux is measured too, so tmux's own
share of the latency is visible. If the only remaining gap is tmux's own round
trip, the gate is still recorded as failed, and the gap goes to the maintainer
as a question about the target rather than being treated as a renderer
problem.

**Gate 4: frame pacing.** A fire animation and a 30-second scroll test. Pass:
at least 0.95 times plain Ghostty's frame rate, with at least 99% of frames
within 1.5 times the refresh interval.

**Gate 5: control-mode fidelity.** On disposable fixtures: the rebuilt pane
state after attach matches the pane; resizing works while another client is
attached; and flow control under a fast-streaming pane recovers without
corrupting state.

Gates 2 to 4 need a minimal window that draws one terminal grid. That window
is benchmark scaffolding, not product UI.

**Results.** Write the method, environment, raw numbers and a pass or fail per
gate under `docs/benchmarks/`, with the harness in the repository. A failed
gate is recorded as failed, with the follow-up decision linked.

## Consequences

- The gate issue's typing-latency comparison changes from Ghostty with tmux to
  plain Ghostty, matching the product target stated in the same issue and in
  the project's contract.
- The proposed terminal engine, UI framework and I/O runtime records
  ([0004](0004-terminal-emulation-core.md),
  [0005](0005-ui-framework-and-terminal-renderer.md),
  [0008](0008-background-io-runtime.md)) are accepted or superseded from these
  results.
- Speed claims anywhere in the project cite a result produced this way.

## Revisit when

A gate cannot be measured as written on available hardware, or the product's
speed targets change.

## Evidence

- [Exploratory headless probe](../research/terminal-core.md#exploratory-headless-probe)
  and [evidence still needed before adoption](../research/terminal-core.md#evidence-still-needed-before-adoption)
  in the terminal-core research.
- [Local throughput evidence](../research/tmux-control-mode.md#local-throughput-evidence)
  and the performance item of the
  [spike list](../research/tmux-control-mode.md#spikes-needed-before-selecting-the-final-contract)
  in the control-mode research.
- [Open risks and deciding measurements](../research/ui-framework.md#open-risks-and-deciding-measurements)
  in the UI framework research.
- [Parity checklist](../PARITY.md#performance-budgets), rows PF-1, PF-9,
  PF-10 and PF-14.
