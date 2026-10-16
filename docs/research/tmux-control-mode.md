# tmux control mode and process model

Evidence checked **17 October 2026**. **Observed** means read in the linked
primary source or reproduced in the private-server probes below. **Reported**
means an upstream description rather than a measurement here. **Inferred**
marks this project's recommendation. These are research verdicts, not adopted
decision records or a passed performance gate.

| Hypothesis | Verdict | Consequence |
| --- | --- | --- |
| 1. tmux is the only process layer | **Confirmed**, with lifetime qualifications | The app owns client connections; tmux owns shells, PTYs and sessions. Disconnect normally preserves processes, but existing server/session options can deliberately destroy them. |
| 2. One control client per leaf; capture, send keys and resize | **Revised** | Keep control mode, but model every window and pane in the attached session. Decide independent navigation, prefix handling, screen recovery and sizing explicitly. A capture is not a terminal checkpoint, and pause/continue is not lossless replay. |
| 3. Fixed tmux throughput ceiling and Ghostty targets | **Revised** | tmux processing is on the critical path, but the proposed fixed rates are workload- and machine-specific. The new probe does not reproduce those rates. Keep Ghostty running tmux as the bulk-output baseline and plain Ghostty as the typing/frame-pacing baseline; neither target has been demonstrated here. |

## Decisions that materially change the architecture

**Inferred:** a leaf should remain a reference to a tmux session, with a native
view of that session's selected window and its pane layout. The project's
workspace split tree and tmux's window split tree are separate models. One
session can have many windows, each containing multiple panes; replacing the
session with its active pane silently loses existing behavior. A control
connection receives output from panes in *all* windows of its attached
session, not just its selected pane. [Observed: control output routing][control]

| Attachment/view approach | Benefit | Cost and recommendation |
| --- | --- | --- |
| Mirror the selected tmux window's panes inside each leaf | Preserves split-pane visibility, zoom and pane targeting; follows iTerm2 and Ghostty's viewer model | **Preferred research direction.** Parse window/layout notifications, retain a terminal state per pane and distinguish workspace splits from tmux splits. Hidden windows still need state or an explicit reseed policy. |
| Show only the active pane | Smallest renderer integration | **Reject as the parity architecture.** Hides other running panes and their geometry; selection elsewhere can unexpectedly replace the visible terminal. Could only be a separately defined zoom presentation. |
| Attach directly and keep window/pane selection local to the app | No helper sessions; explicit pane targets avoid many navigation side effects | Feasible if all navigation is native. tmux key bindings that select windows still act on the shared session, so this does not preserve independent tmux navigation automatically. |
| Create a grouped helper session, then control-attach to it | Independent current/previous window and session options, while sharing the source's windows | **Preferred candidate when the source lifetime policy permits grouping.** Grouping does not isolate pane processes, window sizes, split layouts or the window's active pane. It also creates server state needing ownership and cleanup rules. |
| Ordinary tmux client inside a PTY | Retains tmux's own key tables, status line, copy-mode display and custom interactive UI | Valid comparison/fallback, but renders tmux's composed terminal stream and retains much of the behavior this native client is intended to improve. No extra application daemon is needed. |
| One server-wide synthetic session containing all windows | Could reduce connection count | Reject as the initial design: changes server session/window membership and complicates isolation and ownership merely to save client connections. |

The comparison above is **inferred** from tmux's [session grouping contract][man],
its [sizing implementation][resize], and the existing clients discussed below.
The pinned existing-tool attachment behavior below supplies the compatibility
reference; local probes independently verify the tmux mechanisms.

**Observed locally, tmux 3.7c:** two direct clients attached to one fixture
session both observed its current window change to window 1. A grouped helper
then selected window 0 while the original session remained on window 1. No
processes were duplicated. The group still referred to the same windows.
[Observed source: group creation and synchronization][group]

**Inferred:** connection identity needs host, socket/server, source session and
attachment session. Pane IDs are unique only within a running server; a
restarted server can reuse them. Reconnect must rediscover identities instead
of feeding new traffic into an old pane emulator. One connection per
independently navigable attachment is a reasonable starting point, not a
requirement for one connection per pane. [Observed identity/lifetime contract][man]

## Existing shell-server compatibility

**Observed in Python tmux-workspaces commit `51de6757`**, checked 17 October
2026: the launch context expands and resolves the library directory. The shell
socket is `/tmp/tmux-workspaces-<uid>/<digest>-terminals.sock`, where `digest`
is the first 16 hexadecimal characters of SHA-256 of the UTF-8 encoding of
`str(data_dir.resolve())`. The runtime directory must be owned by the current
user, private and not a symlink. Viewer sockets use a different suffix and
source/viewer/shell sockets must be distinct. These are product protocol paths,
not a licence to discover or alter unrelated servers. [Launch context][python-context],
[socket derivation and launch][python-launch]

**Observed:** an ordinary shell leaf uses `terminal-<leaf id>` with a validated
12-character lowercase hexadecimal ID. `Shells.ensure_many` opens the shell
socket name plus `.viewer-lock`, takes an exclusive `flock`, lists sessions and
creates missing ones under that lock. It releases the lock after that operation;
it is **not an exclusive lock held for the lifetime of a viewer attachment**.
Creation uses no user tmux config and sets status off, mouse on and
remain-on-exit on for the created session/window. [Shell ownership][python-shells]

**Observed:** external attachment first resolves the source's session ID,
window ID, server PID and `destroy-unattached` policy. Only policy `off` uses a
grouped helper. Other or unknown policies use direct `attach-session -E`,
because grouping can destroy a detached `keep-last` source or change its
lifetime. Helpers use a generated `tw-` name, the
`@tmux_workspaces_attachment=1` marker and a source-session marker. They copy
source session options except status and destruction policy, then configure
their own presentation and `destroy-unattached=on`. The client checks source
session existence every two seconds and kills only its helper when the source
is gone, avoiding an orphan helper keeping the source's windows alive. Stored
window hints are validated against the original server PID/session ID.
[Attachment implementation][python-attachments]

**Inferred:** the Rust client should recognize this identity contract without
calling the Python creation or cleanup routines merely to view existing shells.
Opening `layouts.db` read-only does not make a control attachment read-only:
resizing, sending keys and adding grouped sessions still affect tmux. Database
write conflict rules and shell creation/attachment ownership are separate
contracts. Before enabling concurrent creation, confirm lock interoperability,
missing-session behavior, helper cleanup and source-incarnation handling with
the existing implementation's compatibility tests. These are pinned source observations; interoperability with simultaneous
Python and Rust viewers remains unverified. See [Rust architecture](rust-architecture.md)
for the database boundary.

## Process lifetime, transport and protocol

**Observed:** tmux's server owns sessions and pane PTYs; local control clients
can use stdin/stdout pipes without an outer terminal. `-C` selects control mode;
`-CC` additionally disables terminal echo and is intended for terminal-hosted
integration. Client exit does not ordinarily kill its session. However,
`destroy-unattached`, `exit-unattached`, explicit kill commands, shell exit and
host shutdown can end that lifetime. A client must respect an external server's
policy rather than promise unconditional persistence or rewrite its options.
The local fixture survived the last control client's detach. [tmux manual][man]

**Inferred:** use an ordinary SSH executable with **no remote PTY** (`ssh -T`)
as the remote transport. Run the selected remote tmux control command, keep
protocol stdout separate from diagnostic stderr, and preserve normal host-key
verification and SSH configuration. The command supplied to SSH is interpreted
on the remote side, so local argument arrays alone do not solve remote quoting.
Session names, sockets and host settings require distinct validation/quoting.
Connection loss means reconnect and reseed; tmux does not supply a durable
control-output replay log. One SSH channel per attachment is sufficient;
connection sharing is an optimization, not a new daemon requirement.
[Observed SSH behavior][ssh], [observed tmux persistence][man]. Remote transport,
authentication prompts and disconnect recovery were **not executed** here.

**Observed:** commands are newline-terminated tmux commands. Responses have
matching `%begin` and `%end`/`%error` guards with timestamp, command number and
flags. Notifications include session/window/layout changes and pane output.
Treat command bodies as command data, not as arbitrary notifications just
because a line starts with `%`. Retain command-response ordering and handle
asynchronous notifications without assuming that every incoming line completes
an outstanding request. [Manual][man], [command queue][queue]

**Observed:** `%output %pane payload` octal-escapes bytes below space and
backslash; the source stream need not be UTF-8. `%extended-output` adds an age
and extensible fields ending at the standalone `:` before its payload. The age
is time buffered by tmux, not network latency or input-to-photon latency. Decode
control escaping first, then feed a **persistent parser per pane**. A terminal
escape or UTF-8 sequence may cross notification boundaries. A local fixture
produced `ESC[32` and `mX` in separate notifications. Ghostty fixed this same
parser-lifetime mistake in [PR #14676, merged 13 October 2026][ghostty-parser].
[Observed encoding and output implementation][control]

**Observed version discrepancy:** the manual promises notifications never
occur inside a command response. In the installed **3.7c** probe, manual
pause and continue produced the following *structure*:

```text
%begin <time> <command> 1
%pause %0
%end <time> <command> 1
```

The corresponding continue response contained `%continue %0` inside its guards.
The [notification deferral fix][guard-fix] is included in **3.8**. **Inferred:**
prefer 3.8 for the first supported runtime baseline. Supporting earlier versions
requires explicit compatibility fixtures for this behavior; a blanket rule to
classify all percent-prefixed response lines as notifications would corrupt
legitimate captured terminal text. [Observed 3.8 implementation][control38]

## Input, prefix keys, mouse and history ownership

**Observed:** `send-keys -H` accepts individual literal byte values, including
UTF-8 encoded as multiple bytes; it is not an arbitrary Unicode-codepoint API.
It bypasses normal prefix processing when delivering to an ordinary pane.
The local test sent `02 71` and the application received Ctrl-B and `q`; a
custom prefix binding did not execute. **But control clients are not inherently
excluded from tmux key tables:** `send-keys -K C-b q` routed through the client's
key handling and executed that same binding. `-K` is available from 3.4;
3.8 also fixes its input-queue ordering. [Observed send/input paths][send],
[release changes][changes]

**Inferred:** there are two explicit input policies to settle before UI work:

- Native workspace actions and application input can use native bindings plus
  literal bytes targeted to the intended pane. Encode terminal keys, paste and
  mouse reports according to the pane's negotiated modes.
- A tmux-compatibility path can send semantic keys through `-K`, preserving
  prefix/custom bindings that invoke normal commands. This also changes tmux's
  selected window/pane and can invoke tmux modes, prompts or menus. Implementing
  a full native presentation for those effects is separate work. Do not equate
  the successful binding probe with full key-table parity.

**Observed:** tmux's own copy/choose-mode screen is not sent as application
`%output`. Entering copy mode in the fixture generated `%pane-mode-changed`,
not a screen redraw in the control stream. `capture-pane -M` can capture the
mode screen in newer versions; `send-keys -X` operates copy-mode commands.
Neither supplies a complete live native UI by itself. iTerm2 instead maps
native actions to tmux commands and its current controller filters imported
prefix bindings, excluding commands whose interactive presentation it cannot
use directly. [tmux control documentation][wiki], [capture implementation][capture],
[iTerm2 controller][iterm-controller]

**Inferred:** native scrollback, selection, search and local clipboard copying
should operate on the app's per-pane terminal history. That history is distinct
from tmux's retained grid/history: tmux's history is the reconnect seed and is
bounded by its configured history limit; local history is accumulated while
receiving live bytes and disappears on app exit unless separately persisted.
Reseeding cannot recover output already evicted from tmux or reproduce every
historical VT action. Never silently concatenate a full capture onto existing
local scrollback; define replacement and deduplication at the recovery boundary.
[Observed capture and history behavior][capture]

**Observed/inferred:** application mouse reporting can be encoded and sent as
literal pane input; the `-H` addition explicitly supported that use. This does
not traverse the ordinary outer tmux mouse handling path, so tmux's `mouse on`
alone does not supply native scrolling, pane focus, resizing or selection.
The app must distinguish local selection/scroll gestures from reports for a
mouse-aware TUI. `send-keys -M` forwards an existing tmux mouse event from a
binding, not an arbitrary GUI pointer event. [Input paths][send],
[original literal-input change][hex-change]

**Observed/inferred:** tmux itself answers terminal queries such as DA and DSR
on the pane PTY. The embedded terminal core will see those query bytes too;
blindly sending all its generated response callbacks through `send-keys` risks
duplicate or contradictory responses. Suppress tmux-owned replies and define
explicit ownership for keyboard/paste/mouse input versus clipboard, title,
colour and other terminal events. For example, `refresh-client -r` exists for
control clients to supply foreground/background colour reports. Full OSC 52
and keyboard-negotiation behavior needs a dedicated integration fixture.
[tmux input parser][input], [refresh implementation][refresh]

## Seeding and pause recovery

**Observed:** `capture-pane -p -e -q -S -` reconstructs text and rendition from
tmux's grid. It is not the original byte stream or a serialized terminal state.
A useful seed also needs geometry, cursor, modes, scrolling region and tab stops
from formats, plus pending parser bytes from `capture-pane -p -P -C`.
`-S -` means all **retained** history, not unlimited session history.
[Capture implementation][capture], [formats][formats]

**Observed:** when alternate screen is active, normal capture reads its current
grid and `-a` reads the saved primary grid. The fixture returned `ALT` normally
and `PRIMARY` with `-a`, with `alternate_on=1`. The code uses `saved_grid` for
`-a`; map the captures using `alternate_on`, rather than treating `-a` as an
unconditional alternate-screen selector. `alternate_saved_x/y` expose the
primary cursor saved on alternate-screen entry. They are not a general DECSC
saved-state export. [Capture][capture], [alternate-screen implementation][screen]

**Observed:** available formats include current cursor position/shape/visibility,
insert/wrap/origin modes, application keypad/cursor modes, mouse modes,
`pane_key_mode`, `bracket_paste_flag`, scroll-region bounds and tab stops.
Absent formats need capability handling; an empty value is not evidence that a
mode is off. **Inferred limitation:** these sources do not expose a complete
current SGR pen, general saved cursor state, every parser mode or every graphics
resource. A captured styled cell does not reveal a style change made after the
last printed character. Arbitrary live-application attach therefore cannot yet
be promised as an exact clone. [Observed format surface][formats]

**Observed:** pause is a **lossy recovery mechanism**. `control_pause_pane` and
automatic age pausing discard queued pane blocks. `control_continue_pane` resets
the client's offsets to the pane's current position; it does not retransmit the
gap. In the fixture, `PAUSED_PAYLOAD` appeared in a capture while paused but was
absent from the control stream after continue. New `AFTER_CONTINUE` bytes did
arrive. `off` is different again: if all relevant clients turn a pane off, tmux
may stop reading its PTY, backpressuring the application. [Control source][control],
[refresh source][refresh]

**Inferred recovery candidate, still requiring validation:**

1. Mark the pane's current state stale, keep draining the connection and bound
   queued work independently of UI paints. Never drop an arbitrary middle
   portion of the control stream as a memory policy.
2. While the pane is paused, issue one non-waiting command sequence for current
   and saved-grid captures, formats, pending bytes, then continue. Rebuild a
   fresh terminal state and discard its superseded local parser state.
3. Install the seed at a defined response boundary, apply pending bytes once,
   then process subsequent live notifications in order. Reconcile intervening
   pane/window destruction and geometry changes.

Batching reduces the capture/resume gap: the observed command queue runs
non-waiting entries sequentially. It is **not a documented atomic snapshot
protocol**; hooks, commands that wait, preexisting queued output and TCP reads
split across command boundaries need adversarial tests. Capturing first and
then independently resuming can miss output between those operations; resuming
first and appending a later capture can double-apply it. iTerm2's opener requests
history, saved screen, state, pending output and continuation together, making
it a useful behavioral reference, but not a proof of exact recovery for this
client. [Command queue][queue], [iTerm2 opener][iterm-opener]

## Sizing and coexistence with other clients

**Observed:** a control client does not participate in sizing until it declares
a size with `refresh-client -C`. Thereafter, `window-size` policies and all
eligible clients affect the actual shared window. `smallest`, `largest`,
`latest` and `manual` are different policies, not equivalent methods of giving
each viewer an independent PTY size. A pane has one actual PTY geometry.
[tmux sizing source][resize]

**Observed nuance:** `ignore-size` excludes flagged clients when an unflagged
eligible attached client exists; if all are flagged, flagged clients can still
participate. This check is server-wide in the inspected implementation.
Changing flags alone did not immediately recalculate the fixture's size;
the following resize triggered the calculation. Do not treat `ignore-size`
as an unconditional guarantee that a sized client never changes geometry.
[Source][resize]

The private 3.7c fixture observed these dimensions after explicit recalculation:

| State | Shared window size |
| --- | --- |
| Detached window, then two control attachments without sizes | 200×50 |
| First client declares 120×40 | 120×40 |
| Second declares 80×24 under `smallest` | 80×24 |
| Second gets `ignore-size`; first remains unflagged | 120×40 |
| Both get `ignore-size` | 80×24 |
| Eligible client declares per-window `@0:60x20` under `largest` | 60×20 |

**Observed:** per-window `refresh-client -C @id:WxH` both supplies a preferred
size and caps the shared window in the final sizing pass, including with
`largest` or `manual`. It is not a private viewport-size operation. Grouped
helpers share these windows, so grouping does not solve this conflict.
**Inferred:** size app-owned sessions deliberately; for external sessions,
start with an observational attachment and a visible policy for assuming resize
control. Render at the server-reported geometry, with padding or a viewport
when necessary. Never detach other clients or rewrite their window options to
make a benchmark or UI fit. [Refresh][refresh], [sizing][resize]

## Version and implementation baseline

**Observed:** installed and locally tested tmux was **3.7c** at source tag
[`e476c123`][tmux37]. GitHub's latest release resolved to **3.8**; its tag points
to [`7f2a35ad`][tmux38]. Current master was `6e157ce9` when checked. The table
uses release history and tagged source, not an assumption that latest master
and the installed binary behave identically. The proposed initial support
baseline is **3.8**; older-version compatibility remains an explicit decision.

| Mechanism used here | First relevant release / qualification | Primary evidence |
| --- | --- | --- |
| Documented `-C`, `%output`, reply guards; capture `-p/-e/-a/-P/-q` | 1.8; earlier control skeleton was not the documented integration contract | [Release history][changes] |
| `refresh-client -C` | 1.8 already accepts `width,height`; modern versions also accept `widthxheight` | [1.8 implementation][refresh18] |
| `capture-pane -S -` for start of retained history | 2.0 | [Release history][changes] |
| Literal `send-keys -H`, mouse encoding formats | 3.0 feature line; use 3.3+ for the later literal Ctrl-byte fix | [Addition][hex-change], [fix][hex-fix] |
| `pause-after`, `%extended-output`, `%pause/%continue`, `refresh-client -A`; general `-f` flags and `ignore-size` | 3.2 | [Release history][changes], [flow-control change][pause-change] |
| Per-window `refresh-client -C @id:WxH` | 3.3, present in tagged implementation | [3.3 implementation][refresh33] |
| `send-keys -K` | 3.4; input ordering corrected in 3.8 | [Addition][keys-change], [3.8 changes][changes] |
| Per-pane extended-key state `pane_key_mode`; control colour response `refresh-client -r` | 3.5 | [3.5 formats][formats35], [release history][changes] |
| `capture-pane -M` mode-screen snapshot | 3.6 | [Release history][changes] |
| `bracket_paste_flag` format | 3.7 | [Release history][changes] |
| Floating panes | Introduced in 3.7; 3.8 adds `new-layouts` JSON-subset layout representation | [Release history][changes] |
| Deferred notifications outside reply guards; bounded buffered command replies | 3.8 | [Fix][guard-fix], [3.8 control source][control38] |

**Inferred:** feature detection and fixtures should supplement version checks.
The native layout parser needs an explicit choice between legacy layouts and
`new-layouts`; silently flattening an unfamiliar floating-pane layout is not
parity. The `active-pane` client flag was removed in 3.8, so it is not a stable
foundation for independent pane focus. [Observed changes][changes]

## Existing implementations and licences

All implementation snapshots below were inspected on **17 October 2026**.
Licences describe the inspected projects; no code was copied into this project.

| Implementation / alternative | Licence | Observed evidence and assessment |
| --- | --- | --- |
| tmux control mode | [ISC-style permissive licence][tmux-license] | Native protocol and process owner. Preferred backend; capture and sizing limitations remain application responsibilities. |
| iTerm2 | [GPL v2-or-later; project says effectively GPL v3 due to dependencies][iterm-license] | Mature native-window integration documented upstream; source handles layouts, captures, pending bytes, pause/resume and per-window sizes. Strong behavioral reference, **not code to copy** into this MIT project. Snapshot `cceeed4c`. |
| WezTerm | [MIT][wez-license] | Snapshot `e2a98e1b` contains [control-mode capture, resize and literal-input code][wez-commands]. Its [tracking issue #336][wez-issue] remains open and its checklist lags implemented source, so neither “complete client” nor “no implementation” is established by that issue alone. Useful Rust protocol reference, not an off-the-shelf dependency established by this research. |
| Ghostty tmux viewer | [MIT][ghostty-license] | Snapshot `e3ebb1d1` has a viewer state machine, window/pane layout model, screen captures and persistent output parser. Startup-order TODOs and seed assumptions remain. Native GUI [PR #14277][ghostty-gui] closed unmerged on 20 September 2026. Prefer studying its MIT core; do not assume it is already a supported embeddable tmux-client API. |
| Ordinary tmux attach over PTY | Same tmux licence; use OS PTY interface | Strong behavioral baseline for prefixes, custom binds and tmux modes; requires no second application process daemon. Retains composed tmux display and its UI semantics. |
| App-owned PTYs or a new persistence daemon | Licence depends on implementation; no dependency selected | Reject within this product contract: app-owned PTYs lose the required lifetime/attachment model, while another daemon duplicates the selected process layer. Performance alone does not justify that change here. |

**Observed:** Ghostty's viewer uses `capture-pane`, `list-panes` and separate
terminal state per pane, but its own comments flag startup ordering assumptions.
The current capture/mode mappings must be tested against tmux itself; for
example the inspected viewer requests `bracketed_paste`, whereas tmux exports
`bracket_paste_flag`. Existing code is evidence of an approach, not proof that
its seed is complete. [Ghostty viewer][ghostty-viewer], [tmux formats][formats]

## Local throughput evidence

**Observed, 17 October 2026:** macOS 27.0.1, Darwin arm64, Homebrew tmux 3.7c,
Python 3.14.7, `C.UTF-8` locale. Three runs per case, fresh private server per
tmux run, no user configuration, 200×50 initial pane, status disabled, default
history configuration. The host was not otherwise quiescent; CPU model/power
state were not established. These are exploratory measurements, not a gate.

The generator built an 80-byte line: ASCII was 79 `x` bytes plus LF; UTF-8 was
39 U+03BB characters, `x`, then LF. It wrote 8,192 lines per block, 32 blocks:
**20,971,520 bytes (20 MiB)** per run. Python handled short writes until each
block was accepted. A file barrier started production after setup. The generator
used a monotonic clock immediately before the first write and after the final
marker write. The table divides payload bytes by this **producer-completion**
interval; MB/s means decimal megabytes per second. The UTF-8 case has fewer
screen columns per byte, so it is not an equal-cell workload.

| Path | ASCII runs, MB/s | ASCII median | UTF-8 runs, MB/s | UTF-8 median |
| --- | --- | --- | --- | --- |
| tmux, no attached client | 41.93, 37.18, 31.72 | **37.18** | 4.42, 3.89, 3.94 | **3.94** |
| tmux `-C`, stdout continuously drained | 32.72, 35.68, 25.72 | **32.72** | 4.65, 4.36, 3.88 | **4.36** |
| Raw PTY, continuously drained | 95.50, 99.19, 77.40 | **95.50** | 97.44, 89.77, 90.90 | **90.90** |

For drained cases, the harness also timestamped receiving the marker. Median
**drain** rates were 32.72/4.36 MB/s for control ASCII/UTF-8 and 95.50/90.90 MB/s
for raw PTY. Default PTY output processing remained enabled, including LF→CRLF.
Control wire traffic was approximately 23.0 MB per run because of escaping and
framing; input payload bytes, not wire bytes, form the rate numerator. The
reader did not perform octal decoding, VT parsing, shaping or painting.
Detached completion can precede processing of the final buffered tail; drain
completion still does not mean a displayed frame. A faster control UTF-8 run
than detached is noise at this sample size, not evidence of a speedup.

**Inferred:** these results support treating tmux's parsing/history work and
control delivery as performance costs, but do not establish a universal ceiling
of 40–60 MB/s ASCII, under 10 MB/s UTF-8 or 200 MB/s raw PTY. They also do not
compare against Ghostty, an ordinary tmux client, Linux or SSH. Retain the
product's performance goals as goals, then run paired end-to-end measurements.

For reproduction, create a private temporary directory *before* launching tmux,
remove `TMUX` from the child's environment, and give **every** tmux invocation
both that `TMUX_TMPDIR` and an explicit private `-L` or `-S` target. Use a short
socket path on macOS. Create only disposable fixture sessions, start the
producer behind a barrier, drain stdout independently of control commands,
record actual geometry and history settings, then terminate only the server
created by that run. Repeat the three modes and payloads above. Neither the
maintainer's default server nor any existing session was contacted by these
probes; all created servers were terminated.

## Spikes needed before selecting the final contract

These are **inferred validation tasks**, not new performance thresholds:

1. **Session and input parity:** multiple windows, split/zoom/floating panes,
   externally selected windows, grouped helpers, explicit pane targeting,
   prefix repeat tables, custom bindings, copy mode, command prompts and menus.
   Decide what is native, supported through tmux or explicitly unavailable.
2. **Recovery fidelity:** attach and force pause during coloured output,
   alternate-screen transitions, partial CSI/OSC/UTF-8, saved-cursor use,
   reflow and concurrent output. Compare cells, cursor, modes and subsequent
   input behavior with an ordinary tmux client. Verify the single-sequence
   capture/continue boundary under hooks and resize; include errors and pane
   respawn. Test 3.8 directly, plus every older version proposed for support.
3. **Terminal ownership:** ensure DA/DSR replies are not duplicated; verify
   extended keys, bracketed paste, application mouse, OSC 8 and OSC 52 across
   initial attach and reseed. The terminal core's standalone capabilities do
   not guarantee the tmux path exposes the same capabilities.
4. **Coexistence:** ordinary and control clients on the same disposable session,
   grouped helpers, all size policies and ignore-size combinations. Test detach,
   helper cleanup and reconnect without killing source sessions.
5. **Performance:** separate producer completion, control-wire drain, decode plus
   parse, render-state update, final presentation and input-to-photon latency.
   Compare bulk output with Ghostty running the same tmux/workload; compare
   typing and frame pacing with plain Ghostty. Use repeated paired runs, known
   host/load/font/refresh rate, slow consumers, mixed panes and remote latency.
   Hidden-pane pause should measure both resource savings and recovery quality.

[man]: https://github.com/tmux/tmux/blob/7f2a35ad3321f9ba57a1062ca73b1f3ff26aca53/tmux.1
[control]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/control.c
[control38]: https://github.com/tmux/tmux/blob/7f2a35ad3321f9ba57a1062ca73b1f3ff26aca53/control.c
[resize]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/resize.c
[group]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-new-session.c
[send]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-send-keys.c
[input]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/input.c
[capture]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-capture-pane.c
[formats]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/format.c
[screen]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/screen.c
[queue]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-queue.c
[refresh]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-refresh-client.c
[refresh18]: https://github.com/tmux/tmux/blob/1.8/cmd-refresh-client.c
[refresh33]: https://github.com/tmux/tmux/blob/3.3/cmd-refresh-client.c
[formats35]: https://github.com/tmux/tmux/blob/3.5/format.c
[changes]: https://github.com/tmux/tmux/blob/7f2a35ad3321f9ba57a1062ca73b1f3ff26aca53/CHANGES
[tmux37]: https://github.com/tmux/tmux/tree/e476c1230b958df0cb12977517d24b3dc931375b
[tmux38]: https://github.com/tmux/tmux/tree/7f2a35ad3321f9ba57a1062ca73b1f3ff26aca53
[tmux-license]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/COPYING
[hex-change]: https://github.com/tmux/tmux/commit/fc2016db
[hex-fix]: https://github.com/tmux/tmux/commit/9f3874e5
[pause-change]: https://github.com/tmux/tmux/commit/c5862089
[keys-change]: https://github.com/tmux/tmux/commit/3b3f4205
[guard-fix]: https://github.com/tmux/tmux/commit/6db5175e
[wiki]: https://github.com/tmux/tmux/wiki/Control-Mode
[ssh]: https://man.openbsd.org/ssh.1
[iterm-controller]: https://github.com/gnachman/iTerm2/blob/cceeed4cbe4e2dc7624dcedc8157dca36bea285e/sources/tmux/TmuxController.m
[iterm-opener]: https://github.com/gnachman/iTerm2/blob/cceeed4cbe4e2dc7624dcedc8157dca36bea285e/sources/tmux/TmuxWindowOpener.m
[iterm-license]: https://github.com/gnachman/iTerm2/blob/cceeed4cbe4e2dc7624dcedc8157dca36bea285e/COPYING
[wez-license]: https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/LICENSE.md
[wez-issue]: https://github.com/wezterm/wezterm/issues/336
[ghostty-license]: https://github.com/ghostty-org/ghostty/blob/e3ebb1d1bf772cf5d1bc70b5ae19e6943e33a110/LICENSE
[ghostty-viewer]: https://github.com/ghostty-org/ghostty/blob/e3ebb1d1bf772cf5d1bc70b5ae19e6943e33a110/src/terminal/tmux/viewer.zig
[ghostty-parser]: https://github.com/ghostty-org/ghostty/pull/14676
[ghostty-gui]: https://github.com/ghostty-org/ghostty/pull/14277
[python-context]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/supervisor.py
[python-launch]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/application.py
[python-shells]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/shells.py
[python-attachments]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/attachments.py
[wez-commands]: https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/mux/src/tmux_commands.rs
