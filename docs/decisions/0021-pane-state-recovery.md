# 0021. Pane state recovery and the control-mode fidelity harness

- **Status:** Proposed. Settles follow-up 1 of
  [0003](0003-control-mode-connection-and-attachment-model.md) once
  milestone-0 gate 5 passes, as described under **Accept when**.
- **Date:** 2026-10-17, updated 2026-10-18

## Context

The app shows a pane by keeping its own terminal state, fed from control-mode
`%output` ([0003](0003-control-mode-connection-and-attachment-model.md)). It
must build that state without the pane's earlier output in three cases: when
it attaches to a running pane, after a pause (tmux discards a paused pane's
queued output) and when a pane changes size. tmux has no snapshot command.
What it offers is a capture of the grid, re-serialised as text and escape
sequences, plus formats for the cursor, modes and other state
([seeding and pause recovery](../research/tmux-control-mode.md#seeding-and-pause-recovery)).
0003 leaves the exact command sequence and its limits to this follow-up.

Gate 5 ([0018](0018-milestone-0-benchmark-method-revised.md)) measures the
result against an ordinary tmux client attached to the same pane, and
[0014](0014-supported-tmux-versions.md) requires it to pass on tmux 3.3 and
3.8. This record states the candidate the gate will measure and how the gate
compares, from experiments on disposable fixtures on private tmux servers,
with tmux 3.3a, 3.4, 3.5a, 3.6b, 3.7c and 3.8 built from their release
sources. The experiments confirm the research's candidate (captures and
formats in one batch of commands that do not wait, while the pane is paused)
and add what it left open: which reply marks the boundary, and how a capture
becomes terminal state.

Two questions decide the design:

- **The boundary.** Which live output belongs after the rebuilt state, so
  that nothing is lost or applied twice?
- **The installation.** How does a capture become terminal state that
  matches what an ordinary client shows?

## Options considered

**Boundary.** A pane printed consecutive numbers while a neighbouring pane
flooded output and the client stopped reading for 50 or 200 ms around each
recovery, so output backed up inside tmux. A recovery was exact when the
captured screen followed by the live output read on without a gap or repeat.

| Option | Result | Trade-offs |
| --- | --- | --- |
| Capture, then apply the output that follows the capture's reply | Lost output in 15 of 30 recoveries under backlog on 3.7c, and also on 3.3a | Output queued before the capture but sent after it is applied twice or skipped. Rejected. |
| **Pause the pane, wait for the pause command's reply, then send one line that captures, reads formats and continues** | Exact in 100 of 100 recoveries under backlog on 3.3a, 3.7c and 3.8 | Touches only the recovered pane. Works on one connection. |
| A fresh connection attached with `-f no-output,ignore-size`, capturing in one line, then turning output on | Exact in 100 of 100 recoveries under backlog on 3.3a, 3.7c and 3.8 | A process per recovery. Suits the first attach, which has nothing queued. |
| Continue first, then capture | Not tried: output between the two is applied twice | Rejected. |

**Installation.**

| Option | Trade-offs |
| --- | --- |
| Feed the capture's bytes to the engine as they are | Wrong in several ways: tmux writes overline as `5:3`, which reads as blink; the text style carries from one row into the next, so rows scrolled or erased later take the wrong colour; pending control characters run twice; a cursor waiting to wrap is lost. Rejected. |
| **Parse tmux's capture format into cells, then write synthesized escape sequences into a fresh terminal** | Needs only the engine's ordinary write path, so it survives an engine change ([0004](0004-terminal-emulation-core.md)). The parser follows tmux's capture format, which can change between releases, so it is covered by recorded fixtures. |
| Import state through an engine API | `libghostty-vt` 0.2.2 has none. Would tie recovery to one engine. |

**Resize.** Another client resized the window from 40x8 to 30x8, 50x10, 40x6
and 36x9. A capture runs inside tmux's event loop, so it costs every pane on
the server time (see **Consequences**).

| Option | Result | Trade-offs |
| --- | --- | --- |
| Resize the local terminal and let the engine reflow | Differed from tmux on plain output on every version: rows moved between the screen and history differently when the height changed. Matched for a full-screen program that redraws itself. | Cheap, but wrong for ordinary shells. |
| **Rebuild the pane with the recovery sequence once its size settles** | Matched on every version for plain output, a redrawing full-screen program and streaming output, also after more output | A capture of the whole retained history per resized pane. |
| Rebuild with the screen and only the most recent history, keeping the app's older scrollback | Not measured | Bounds the cost, but the engine reflows the kept scrollback differently from tmux, so lines can repeat or go missing where the kept and captured parts meet. |
| Rebuild after every layout change, without waiting for the size to settle | Not measured | Dragging a window edge sends many layout changes, each costing a full capture. |

**Reference for the gate.**

| Option | Trade-offs |
| --- | --- |
| **An ordinary tmux client in a pseudo-terminal, its bytes fed to the same engine as the app** | Differences come from recovery, not from two engines disagreeing about character widths, which gate 1 found ([gate 1](../benchmarks/gate-1.md)). |
| The same client fed to a second emulator | Mixes engine differences into the result. Rejected. |
| tmux's own capture as the expected state | Circular: the candidate is built from it. |

**A cursor at or beyond the right margin.** A resize can leave tmux's
logical cursor beyond the new width. On all six supported versions, an
alternate-screen pane at 40×6 with `R` at row 6, column 40 retains logical
cursor `(40, 5)` after shrinking to 36×9. The ordinary client hides its
cursor and parks it at `(0, 0)`. Restoring every column at or beyond the
width as pending wrap loses information: cursor-left-five followed by `X`
puts X at column 35 in tmux and column 30 in the rebuilt engine. Merely
moving the engine's cursor home also breaks the next printable character.
Controls with only a height change expose related pending-wrap differences:
backspace, cursor-left and disabling wrap do not have the same transitions
in tmux and the engine. These are retained diagnostic observations, not
expected-limit allowances.

| Option | Trade-offs |
| --- | --- |
| Clamp the cursor or treat every off-grid column as pending wrap | Discards the logical column and fails subsequent output. Rejected. |
| Add a stateful compatibility interpreter for the off-grid cursor | Must implement every operation that can affect that state, including split sequences, modes and edits. A partial implementation would silently diverge. Not selected. |
| Import the off-grid cursor into the engine | The pinned engine has no public setter and requires its cursor to be inside the grid. Not available. |
| **Retain a snapshot while the cursor is at or beyond the right margin; capture again after output** | Uses tmux's state transitions. Adds recovery work and the existing capture limitations until a snapshot's cursor is inside the grid. |

**Fixtures in a window with several panes.** On tmux 3.7c, an ordinary
client viewing a pane narrower than the window is not redrawn when a
character there grows, for example when a combining accent or an emoji
joiner follows: it kept a bare `e` and a bare emoji. In a full-width pane,
and on the other five versions, tmux rewrites the cell. The app does not use
that drawing path; it reads the pane's own output.

| Option | Trade-offs |
| --- | --- |
| **Run fixtures whose characters grow only in windows with one pane** | The other fixtures still test cropping and per-pane recovery in a shared window. Growing characters are tested at full width, where the reference is right on every version. |
| Check cells where the two differ against tmux's capture as a third witness | Keeps the fixture everywhere, but the candidate is built from that capture, so the check proves little, and it needs a list of excused cells. |
| Make the reference redraw (`refresh-client`) before comparing | Fixes most cells, but a character widened by a variation selector still differs, and the redraw brings back the underline-colour merge described below. |

## Decision

**Recovery sequence.** For each pane that needs rebuilding, on the pane's
control connection:

1. Send `refresh-client -A '%<pane>:pause'` and wait for that command's
   `%end`. The pane ID is quoted, because `%` starts a format in a control
   command. From the pause on, tmux queues nothing for the pane, so all of the
   pane's output that arrives after this `%end` belongs after the rebuilt
   state. The `%end` is the marker, not the `%pause` notification, which tmux
   before 3.8 sends inside the command's reply and 3.8 sends after it.
2. Send one line of `;`-separated commands, none of which waits: capture the
   current grid with all retained history
   (`capture-pane -p -e -N -C -S - -E -`), capture the saved grid
   (`capture-pane -p -e -N -C -a -q -S - -E -`), read each format with its own
   `display-message -p`, read pending parser bytes
   (`capture-pane -p -P -C`), then `refresh-client -A '%<pane>:continue'`.
   tmux runs these with no pane output read in between, so the capture is
   exactly the state at which output resumes. Each format gets its own
   command because some, such as `cursor_character`, can contain a newline.
3. Build a fresh terminal at the pane's size from the replies, then apply the
   pane's output from step 1 on, in order.

The first attach uses a fresh connection attached with
`-f no-output,ignore-size`, the same captures and formats in one line, and
`refresh-client -f '!no-output'` in place of the continue; all of that
connection's output follows the rebuilt state. A pause that tmux makes itself
(`pause-after`) is recovered with step 2 onwards, taking the pane's output from
its `%pause` notification on.

If a command in the line fails (`%error`), for example because the pane
exited after the pause, tmux skips the rest of the line, including the
continue. The replies are then not a snapshot, and the app discards them.
On a first attach, the connection is left without output for every pane, so
the app sends the line again on the same connection for the panes that
remain, still ending with `refresh-client -f '!no-output'`. It does not
close that connection first: it may be the session's only client, and a
session with `destroy-unattached` on would be destroyed. Otherwise
the pane is left paused, and the app starts its recovery again from step 1,
or drops the pane if tmux reports it gone.

When several panes need rebuilding, they are rebuilt one at a time: the next
pause is sent after the previous line's replies, so tmux serves the other
panes' output between captures.

tmux runs a command's after-hooks before the next command in the line. A
hook that waits lets pane output in between, and any hook can change the
pane, so the guarantee holds only without such hooks. The app reads
`show-hooks` and reports a session with a hook on `capture-pane`,
`display-message` or `refresh-client` as one it cannot recover exactly.

**Installation.** The `tmux` crate owns the protocol, so it parses the capture
format into rows of cells with their style, hyperlink and line-drawing
character set. The `term` crate writes them into a fresh terminal:

- Each row starts from the default style, and rows are separated by a style
  reset and a new line, so rows scrolled later fill with the default colours.
- Rows captured with `-N` hold only the cells tmux allocated; the rest of the
  row is left blank. Without `-N`, tmux trims blank cells that carry a
  background colour.
- Overline, written by tmux as `5:3`, is written as `53`. A cell tmux marks
  with shift-out is written with G1 set to DEC line drawing, restored after.
  Hyperlinks are written with OSC 8 when they change.
- From tmux 3.6, a tab over blank cells is kept as one cell and captured as
  a tab character, without its width. It is written as blank cells in its
  style from its column up to, not including, the next of the pane's tab
  stops, or the last column if no stop follows, as a tab moves the cursor.
- With the alternate screen on, the primary rows go in first, then the cursor
  tmux saved on entry (`alternate_saved_x/y`), then the switch and the
  alternate rows. The history rows come from the current capture. The
  switch saves the style, character sets and origin mode with the cursor
  and restores them when the program leaves, so the primary rows end in
  the default style and character sets, and origin mode is set before
  them, as tmux keeps it.
- Then tab stops (cleared, then set from `pane_tabs`), the cursor at its
  position on the screen, and the scrolling region, set without moving the
  cursor: tmux moves the cursor to the top left of the screen when a region
  is set, even in origin mode, so the cursor can be outside the region.
  Then the other modes (cursor and keypad application modes, insert, wrap,
  mouse modes, bracketed paste, cursor visibility, the extended-keys mode
  from `pane_key_mode` and focus reporting from `pane_private_modes` where
  tmux has them). They come after the cursor because with wrapping off, the
  engine can join a combining character to the wrong cell when the
  cursor's row is written again.
- A cursor waiting to wrap (`cursor_x` equal to the width) is restored by
  writing the cursor's whole row again from its first column, which leaves
  the engine waiting to wrap whatever the last cell's width. If the row's
  last cells were never written, as after the row is erased, blank cells
  fill it.
- Pending bytes from `-P -C` are decoded once. Inside an escape or control
  sequence, tmux has already acted on the control characters in them, so
  those are dropped; inside a string sequence such as OSC they are kept.

**Right-margin recovery.** The installation above can represent ordinary
pending wrap, but cannot express an off-grid column or all of tmux's
pending-wrap transitions. The caller therefore retains the full snapshot
when `cursor_x >= pane_width`. It installs only a display copy in the
engine, with a hidden cursor at home and without pending parser bytes. That
copy never receives live output; the retained snapshot supplies the logical
cursor and cursor mode.

When more pane output arrives, the caller coalesces the available output
and repeats the normal pause/capture/continue recovery. A snapshot whose
cursor is still at or beyond the margin replaces the displayed snapshot.
Once a snapshot has `cursor_x < pane_width`, normal engine streaming resumes
at that capture's boundary, including its pending bytes and subsequent
output. Already captured output is not replayed. No output means no extra
capture. Failure to recover is reported; it is not an exact match. The
harness bounds repeated attempts with its recovery timeout.

Streaming output can also leave the engine's pending-wrap flag set. That
flag is ambiguous when wrapping is disabled: tmux may still hold the last
in-grid column. Before exposing such state, the caller captures again and
uses tmux's actual cursor instead of inferring an extra column from the
engine flag. The recovery deadline is shared by all replies and retries;
receiving another event does not extend it.

This is a recovery rule, demonstrated by the harness's candidate,
not an extra terminal interpreter. `Terminal::restored` rejects a cursor
beyond the width instead of silently converting it to pending wrap. No new
dependency or licence is introduced.

**Resize.** When `%layout-change` reports a new size for a pane, whichever
client caused it, the app rebuilds that pane with the recovery sequence once
no further change to its size has arrived for a short settle interval, so
dragging a window edge costs one rebuild per pane rather than one per step.
Until then it shows the engine's own reflow. The rebuild captures the whole
retained history, the only option measured exact.

**What a rebuild cannot restore.** tmux does not export these, so after a
rebuild they can differ until the program sets them again:

- The current text style (output after the rebuild that relies on it is
  drawn in the default style), the general saved cursor (DECSC) and its
  style, a partly received UTF-8 character, which character sets are
  designated and which is in use (cells already drawn in line drawing are
  restored, but a program still in line-drawing mode has its next characters
  drawn as plain text), and colour-palette changes (OSC 4).
- Some modes have no format on older versions: bracketed paste before tmux
  3.7, the extended-keys mode before 3.5 and focus reporting before 3.8. No
  release up to 3.8 tracks the kitty keyboard protocol's flags. The app's key
  encoder and paste read these modes ([0015](0015-input-routing.md)), so
  until the program sets them again, keys and pastes go to it as to a
  program that has not asked for them.
- If the pane changed size while the alternate screen was on, tmux keeps the
  primary screen saved on entry at its old size and reflows it when the
  program leaves, but a capture returns it at the current width. Cells
  beyond that width are lost, so after the program leaves the alternate
  screen, the primary screen can differ.
- On tmux 3.3, a capture stops at the last cell written in each row, so the
  tail of a line erased with a background colour comes back with the default
  background. tmux 3.4 captures the full width.
- Before tmux 3.4, tmux keeps no hyperlinks, so a rebuild drops them. From
  3.4, a capture closes a hyperlink at the end of each row and does not open
  it again where it continues, so a link that wraps onto the next row keeps
  its link only on its first row.
- tmux keeps a tab cell's width, but a capture leaves it out, so a rebuild
  measures the cell against the pane's current tab stops. A tab cell that
  has moved since it was written, by inserted or deleted characters or a
  reflow, or that was written before the program changed the tab stops, can
  end in a different column.

The sequence also leaves out which rows were soft-wrapped. That does not
change what is shown, and a resize rebuilds the pane anyway, but selection
and copy then treat a wrapped line as several. tmux 3.8 reports it with
`capture-pane -F`; the change that needs it adds it.

**Harness for gate 5.** The harness runs each scenario on a disposable
session on a private server
([0012](0012-testing-strategy-and-tmux-isolation.md)):

- The reference is an ordinary tmux client in a pseudo-terminal with
  `TERM=xterm-256color`, `status off`, one window, and `terminal-features`
  for true colour, underline styles and colours, overline, strikethrough,
  hyperlinks and synchronised output. It attaches before the fixture draws
  anything and is not asked to redraw. tmux still redraws it when the pane
  is resized and when the program enters or leaves the alternate screen.
  Its bytes go into the same engine and version as the app's.
- The candidate is the app's recovery code on a control connection.
- After each scenario, and again after the fixture has written more output
  and received input sent through the app, the harness compares every cell
  of the pane: text, width, colours, underline style and colour, attributes
  and hyperlink. Blank cells (empty or a space) are equal when they look the
  same: the same background, attributes, underline style and colour and
  hyperlink, and the same foreground when reverse video, an underline, an
  overline or a strikethrough makes it visible. In a window with several
  panes, the reference's screen is cropped to the pane's position and size.
  The candidate's logical cursor, including pending wrap and an off-grid
  column, is compared with fresh tmux cursor formats. Its position projected
  into the ordinary client's viewport is independently compared with the
  reference: pane offsets are added, a column at the window's width is
  clamped to its last column, and a cursor beyond the viewport is parked at
  `(0, 0)`, as tmux's client does. Both position checks must pass. Cursor
  visibility mode and other modes come from tmux's formats, because the reference terminal's modes are the
  ones tmux sets for itself, not the pane's. A mode a version does not
  export is reported as not compared, not as a match.
- Scenarios: attach; a pause forced under backlog; tmux's own pause, reporting
  how often a stall ended in a pause; a resize by another client, also
  checking that the app's pane size equals tmux's `pane_width` and
  `pane_height`; a window with several panes; the pane exiting between the
  pause and the capture, and during a first attach while another pane
  remains; a session with a hook on a recovery command, which
  the app must report.
- Fixtures: text styles and colours, background-coloured erases, wide and
  combining characters and emoji sequences, line drawing, history, the
  alternate screen with a scrolling region, tab stops and modes, pending
  escape, control and string sequences, a cursor waiting to wrap after a
  narrow and a wide character, origin mode with wrapping off, hyperlinks
  including one that wraps, tabs over coloured blank cells and a tab cell
  moved by inserted characters, underline colours, and the unexportable
  state above (current style, partial UTF-8, DECSC, a program still in
  line-drawing mode, a resize while the alternate screen is on followed by
  leaving it). Fixtures whose characters grow after they are drawn run only
  in windows with one pane.
  Neither those nor neighbouring cells with different underline colours are
  used where tmux redraws the reference: in the resize scenario, or in
  fixtures that enter or leave the alternate screen.
- Right-margin fixtures also exercise backspace, cursor-left and disabling
  wrap after recovery. Resize fixtures retain their exact seed snapshots,
  post-seed replay bytes, control events and ordinary-client bytes under the
  run's `resize/` directory, including successful comparisons. The reference
  bytes begin immediately before the latest resize, as the comparison does.

Three behaviours of tmux affect the reference, not the app, and the harness
allows for them: tmux 3.7 and 3.8 merge underline colours across a run of
cells when they redraw a pane or a whole screen (the reference attaches
first, so apart from the redraws above it draws through tmux's ordinary
path); tmux 3.3 sends a palette underline colour as RGB (compared by value); and tmux hides the client's
cursor while the pane's cursor waits to wrap (visibility is taken from
`cursor_flag`).

## Consequences

- Every rebuild, including a right-margin refresh, captures the pane's whole retained history inside tmux's
  event loop, so every pane on that server waits about as long. On tmux 3.7c,
  for a 120x40 pane of styled 100-character lines, with a neighbouring pane
  printing a line every millisecond (median of 5):

  | Lines of history | Reply | Capture | Longest gap in the neighbour's output | Installing in the engine |
  | --- | --- | --- | --- | --- |
  | 1,961 | 0.31 MB | 21 ms | 17 ms | about 4 ms |
  | 9,961 | 1.57 MB | 81 ms | 61 ms | about 4 ms |
  | 49,961 | 7.85 MB | 245 ms | 248 ms | about 15 ms |

  tmux's default history limit is 2,000 lines. With much longer histories, a
  settled resize visibly pauses the server once per resized pane. If gate 4
  or use shows that matters, the bounded rebuild above needs its own
  measurement and record.
- A pane that remains at the right margin while producing output needs
  repeated snapshots. This can add latency and repeats the existing losses
  of unexported state, such as the current text style. Gate 5 checks the
  resulting cells and continuation without an additional allowance; it does
  not establish a latency bound. Gates 2–4 must still measure that cost.
- The capture parser follows a format tmux can change. It carries recorded
  captures from each supported version, as the reply parser does
  ([0014](0014-supported-tmux-versions.md)).
- tmux keeps the lines scrolled out of a scrolling region that does not start
  at the top in its history; the engine does not. A rebuild therefore brings
  such lines into the app's scrollback, which live output would not.
- The 3.3 floor stays: its limits above are losses of fidelity, not
  workarounds.

## Accept when

The harness is in the repository and gate 5 passes on tmux 3.3 and 3.8, and
on each version in between that behaves differently, with every mismatch
explained by a limit listed above. Then follow-up 1 of 0003 is settled.

## Evidence

Observed on disposable fixtures on private servers, with a prototype of the
installation feeding `libghostty-vt` 0.2.2 (MIT OR Apache-2.0, over
Ghostty's core, MIT; see [0004](0004-terminal-emulation-core.md)). A dash
means not run.

| Check | 3.3a | 3.4 | 3.5a | 3.6b | 3.7c | 3.8 |
| --- | --- | --- | --- | --- | --- | --- |
| Pause, then capture and continue, 200 ms stalls under backlog | 100/100 exact | – | – | – | 100/100 | 100/100 |
| Fresh connection without output, same load | 100/100 | – | – | – | 100/100 | 100/100 |
| tmux's own pause: exact recoveries / stalls that ended in a pause, of 40 | 14 / 14 | – | – | – | 17 / 17 | 19 / 19 |
| Pane output inside a command's reply, of 30 | none | – | – | – | none | none |
| 17 fixtures, one pane: differ after attach | background erase | none | none | none | none | none |
| Same, in a split window | background erase | none | none | none | growing characters (reference) | none |
| Resize by another client, rebuild / local reflow | match / differs | match / differs | match / differs | match / differs | match / differs | match / differs |

After more output, on every version, the fixtures for the current style, a
partial UTF-8 character and DECSC differed as expected, and the origin-mode
fixture differed only in the history line described under **Consequences**.
Hyperlinks were compared on 3.4 and later, and underline colours with the
3.3 normalisation above.

- tmux source, ISC licence, read for behaviour: pausing and continuing
  (`control.c`), reply guards (`cmd-queue.c`), capture (`cmd-capture-pane.c`,
  `grid.c`), region scrolling into history (`grid.c`) and key modes
  (`input.c`), logical cursor transitions (`screen.c`, `screen-write.c`)
  and the ordinary-client cursor projection (`server-client.c`, `tty.c`).
