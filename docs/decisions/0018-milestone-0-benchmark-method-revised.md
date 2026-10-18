# 0018. Milestone-0 benchmark method, revised

- **Status:** Accepted. Supersedes
  [0013](0013-milestone-0-benchmark-method.md).
- **Date:** 2026-10-17

## Context

[0013](0013-milestone-0-benchmark-method.md) fixed the method of the five
milestone-0 gates. Gate 1 was measured with it and failed on escape-heavy
coloured output ([gate-1 results](../benchmarks/gate-1.md)). Building the
harness for gates 2 to 4 showed three gaps in it:

- **Gate 3's instruments are not usable here.** 0013 names Typometer or a
  240 fps camera. The benchmark machine has no high-speed camera. Typometer
  posts system-wide key events, which reach whichever window has focus, so a
  focus change during a run would type into an unrelated window, possibly a
  terminal running real work.
- **Gates 2 and 4 name no instrument** for "the presented frame that shows the
  final marker" and for the frame rate.
- **The window must be visible.** In GPUI 0.3.9, a window stops drawing when
  it is covered, minimised or on another Space, and an inactive window draws
  at most once every 33.3 ms unless the window turns that limit off
  ([0019](0019-benchmark-window.md)).

This record restates 0013 with those gaps closed. Gates 1 and 5 are unchanged.

## Options considered

How displayed frames are timed for gates 2 to 4:

| Option | Trade-offs |
| --- | --- |
| Typometer | Synthetic keys and screen polling. Its keys go to the focused window, which is unsafe here, and polling times frames more coarsely than the display's own frame times. |
| A 240 fps camera | Measures to the photon, but none is available, and it quantises to 4.2 ms. |
| Each program's own presentation times | Precise, but Ghostty does not expose them, so the comparison would not be paired. |
| **ScreenCaptureKit capture of the measured window** (macOS 12.3 and later) | One method for the app and for Ghostty. Every captured frame carries the time the window server displayed it. Keys can be posted to one process only. It stops at the window server, so it leaves out the display's scan-out and pixel response, which are the same for both programs. Capturing adds work for the window server, also the same for both. |

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

**Display measurement, for gates 2 to 4.** These run on macOS.

- The app is measured through the benchmark window of
  [0019](0019-benchmark-window.md). Ghostty is a separate instance, started
  for the run with its own configuration file and no default configuration
  files, and stopped afterwards. Only windows the harness started are
  measured, captured or sent input.
- The benchmark window and Ghostty read their font, font size, colours and
  sixteen-colour palette from the same file. Ghostty also gets no window
  padding, no cursor blinking and no shell integration, and the window has no
  blinking either, so nothing changes on screen but the output measured.
- The harness captures the measured window alone with ScreenCaptureKit and
  uses only complete frames, each with its display time. Times taken by the
  harness, such as releasing a producer or posting a key, use the same host
  clock.
- Keys are posted with `CGEventPostToPid` to the measured program's process
  only, never as system-wide events.
- Each run measures one window at a time, at the same place on the same
  display. Before each run the harness checks that the window is on screen; a
  window that is covered, minimised or on another Space is a failed run, not a
  slow one.
- The harness's macOS code reaches these system interfaces through the
  `objc2` bindings, at the versions GPUI's macOS backend already uses, so they
  add no second copies. (Other GPUI dependencies keep older copies of `objc2`,
  `objc2-foundation` and `block2`.) They are dependencies of `crates/bench` on
  macOS only:

  | Dependency | Version | Licence | Reason |
  | --- | --- | --- | --- |
  | `objc2-screen-capture-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Window capture and frame display times |
  | `objc2-core-graphics` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Window lists, display refresh rates and key events |
  | `objc2-core-media`, `objc2-core-video` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Reading captured frames |
  | `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | Core Foundation types those interfaces return |
  | `objc2`, `objc2-foundation` | 0.6.5, 0.3.2 | MIT | Calling Objective-C and its collection types |
  | `block2`, `dispatch2` | 0.6.2, 0.3.1 | MIT; Zlib OR Apache-2.0 OR MIT | Completion handlers and the capture's dispatch queue |

- This instrument leaves out the display's scan-out and pixel response, so its
  latencies are lower than a camera's, by the same amount for both programs.
  A latency or end-time comparison (gates 2 and 3) within one refresh
  interval of its limit is confirmed with a camera before the gate is
  decided. Frame counts and intervals (gate 4) do not depend on scan-out.

**Gate 1: headless headroom.** Record control-mode traffic from a private tmux
server while it runs the generated payloads. Replay it through control-mode
decoding, the terminal engine, the render-state update and owned display-data
extraction, with a forced final extraction after the last chunk.

- Every engine compared uses the same grid size, history limit, replay chunk
  size and extraction schedule, and produces the same owned display data.
- Its final screen and cursor must match the other engines' before its timing
  counts. Differences from tmux's own capture of the pane are reported as
  fidelity findings.
- Every rate divides the generator's payload bytes, counted as the generator
  writes them and before the terminal driver's output processing (such as LF
  to CRLF) or tmux's control-mode escaping, by elapsed time. The same
  numerator is used for the engine path and for tmux. Decoded pane bytes and
  control-wire bytes are reported separately, never as the numerator.

Pass: at least 200 MB/s for ASCII on the benchmark machine, and at least five
times tmux's own throughput for every payload. tmux's throughput is the rate at
which a drained control-mode client receives the same payload, measured on the
same machine in the same session.

**Gate 2: end-to-end bulk output.** In the app and in Ghostty running
`tmux attach` on the same kind of fixture:

- **Marker probe.** Each 20 MB payload is printed by a producer that waits on
  a FIFO, as in gate 1, and ends by filling the screen with a marker colour.
  The time runs from the harness releasing the producer to the display time of
  the first frame that the marker colour fills. The rate divides the payload's
  bytes by that time.
- **vtebench**, built from a pinned commit, runs inside the tmux pane and
  reports its own timings.

Pass: at least 0.95 times Ghostty with tmux on every case, with the final frame
no more than one refresh interval later.

**Gate 3: typing latency.** At least 40 keys per condition, one at a time, at
random intervals of 150 to 350 ms so that keys do not fall on the same point
of the refresh cycle. The pane runs `cat`. A key's latency runs from just
before it is posted to the display time of the first frame in which the region
of the typing pane differs from the frame shown before the key. Conditions:
idle, and with a neighbouring pane in the same window streaming output, which
is left out of the compared region. Pass: median within 4 ms and 95th
percentile within 8 ms of **plain Ghostty**, as the product target states.
Ghostty running tmux is measured too, so tmux's own share of the latency is
visible. If the only remaining gap is tmux's own round trip, the gate is still
recorded as failed, and the gap goes to the maintainer as a question about the
target rather than being treated as a renderer problem.

**Gate 4: frame pacing.** A fire animation, from a checked-in generator that
redraws the whole pane as fast as the terminal accepts, and a 30-second scroll
test. The frame rate counts complete frames per second, leaving out the first
second; pacing uses the intervals between consecutive frames. Pass: at least
0.95 times plain Ghostty's frame rate, with at least 99% of frames within 1.5
times the refresh interval.

**Gate 5: control-mode fidelity.** On disposable fixtures, with an ordinary
tmux client attached to the same pane as the reference:

- **Attach and recovery.** Rebuild the pane's state on attach and after a
  forced pause, during coloured output, alternate-screen switches, partial
  escape and UTF-8 sequences, and concurrent output. Compare every cell's text,
  width and style, the cursor position and visibility, and the modes tmux
  exports with the reference. Then send further output and input and compare
  again, so state that a capture hides still shows up.
- **Resize.** Resize while another client is attached, and check both clients
  agree on the pane's geometry and contents.
- **Flow control.** Under a fast-streaming pane, pausing and continuing ends in
  a rebuilt state that passes the comparison above, with no output applied
  twice.

A mismatch passes only if it is caused by state tmux cannot export, such as the
current text style or a general saved cursor, and that limitation is written
into the recovery follow-up decision of
[0003](0003-control-mode-connection-and-attachment-model.md). Any other
mismatch fails the gate.

**Results.** Write the method, environment, raw numbers and a pass or fail per
gate under `docs/benchmarks/`, with the harness in the repository. A failed
gate is recorded as failed, with the follow-up decision linked.

## Consequences

- Gate 3 uses a different instrument from the one 0013 named, and every
  display gate leaves out scan-out, which only a camera sees. Results near a
  threshold need the camera check above.
- Gates 2 to 4 need a quiet machine with one display showing a normal desktop
  where the measured window stays fully visible, and a process allowed to
  record the screen and post events.
- Display gates are measured on macOS only. Linux display measurement needs
  its own instrument and record.
- The refinements 0013 made to the gate issue's draft stand: gate 1's "tmux's
  own throughput" is the control-mode drain rate measured in the same session,
  and gate 3 compares typing latency with plain Ghostty.
- The proposed connection, terminal engine, UI framework, I/O runtime and tmux
  version records ([0003](0003-control-mode-connection-and-attachment-model.md),
  [0004](0004-terminal-emulation-core.md),
  [0005](0005-ui-framework-and-terminal-renderer.md),
  [0008](0008-background-io-runtime.md),
  [0014](0014-supported-tmux-versions.md)) are accepted or superseded using
  these results.
- Speed claims anywhere in the project cite a result produced this way.

## Revisit when

A gate cannot be measured as written on available hardware, a camera becomes
available, or the product's speed targets change.

## Evidence

- Everything listed in [0013](0013-milestone-0-benchmark-method.md#evidence).
- Observed in GPUI 0.3.9: the macOS window stops its display link when
  AppKit reports it no longer visible
  (`window_did_change_occlusion_state` in `gpui-pre-macos` 0.3.9,
  `src/window.rs`), and `WindowOptions::default()` limits inactive windows to
  one frame every 33,333 µs (`gpui-pre` 0.3.9, `src/platform.rs`).
- [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit):
  `SCStreamFrameInfo.displayTime` and the frame status of each sample, and
  [`CGEventPostToPid`](https://developer.apple.com/documentation/coregraphics/cgevent/post(topid:)).
