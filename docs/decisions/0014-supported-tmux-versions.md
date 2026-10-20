# 0014. Supported tmux versions

- **Status:** Proposed. Waits on version fixtures and the control-mode fidelity
  gate, described under **Accept when**.
- **Date:** 2026-10-17

## Context

The Python tool requires tmux 3.3 or newer and checks it at startup with
`tmux -V` ([SU-2 and SU-3](../PARITY.md#startup-preflight-and-recovery)).
Whether the same floor works for a control-mode client is an open question in
the parity checklist.

The control-mode research found that the features this app uses arrived in
different releases:

| Mechanism | First release |
| --- | --- |
| Control mode with `%output` and reply guards; `capture-pane` with `-p`, `-e`, `-a`, `-P` and `-q` | 1.8 |
| `capture-pane -S -` for all retained history | 2.0 |
| Literal `send-keys -H` (use 3.3 or later for a fix to literal control bytes) | 3.0 |
| Flow control: `pause-after`, `%extended-output`, `%pause` and `%continue`; client flags including `ignore-size` | 3.2 |
| Per-window `refresh-client -C @id:WxH` | 3.3 |
| `send-keys -K` through key tables (input ordering fixed in 3.8) | 3.4 |
| Per-pane `pane_key_mode` format; `refresh-client -r` colour reports | 3.5 |
| `capture-pane -M` for mode screens | 3.6 |
| `bracket_paste_flag` format; floating panes | 3.7 |
| Notifications kept outside reply guards; bounded buffered replies; `new-layouts` layout format | 3.8 |

In a local test on tmux 3.7c, a `%pause` notification arrived inside a
command's reply guards, although the manual says notifications never do. tmux
3.8 fixes this. On the other hand, the `active-pane` client flag was removed in
3.8, so it is no foundation for independent pane focus.

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Keep the Python floor, 3.3** | Matches the Python tool, so anyone who runs it can run this app. The reply parser must handle notifications inside replies, and newer features need detection and fallbacks. |
| Raise the floor to 3.8 | Simplest reply parsing, and every feature above is available. Excludes tmux 3.3 to 3.7 installations, including distributions that package an older release. |
| An intermediate floor, such as 3.4 or 3.5 | Gains `send-keys -K` or `pane_key_mode`, still needs the reply-interleaving work, and still excludes some Python users. |

## Decision

Target **tmux 3.3** as the floor, matching the Python tool, under these rules:

- Detect capabilities from the reported version and by probing. A format or
  command that is missing is **unknown**, never a false value; for example, a
  missing `bracket_paste_flag` does not mean bracketed paste is off.
- The reply parser handles notifications inside reply guards as observed on
  3.7c, backed by fixtures recorded from each supported version. It never
  treats every `%`-prefixed line inside a reply as a notification, because
  captured terminal text can start with `%`.
- Features that need a newer tmux are offered only where available, and their
  absence is shown to the user rather than hidden.
- Keep the Python tool's version check and messages (SU-3, SU-4), and apply the
  floor to every server the app connects to, not only to the `tmux` binary it
  runs.

## Read-only executable preflight

`tmux::preflight::check_tmux` is an explicit library call. It finds an executable
using literal `PATH` entries, including relative entries, and runs only `tmux -V`.
The existing bounded process capture drains stdout and stderr, limits their
combined size to 1 MiB, and kills and reaps a stalled child after five seconds.
It never contacts a server. Missing, failed, malformed and older versions have
separate remedies; malformed output and stderr are not echoed in diagnostics.
Normal product startup and checks of connected server versions remain pending.

The parser follows Python's patch and numbered-development suffix rules and its
Unicode digit/word classification. It compares major/minor values without machine
integer overflow. Unicode decimal digits form ascending groups of ten
([Unicode numeric values](https://www.unicode.org/versions/Unicode16.0.0/core-spec/chapter-4/));
the existing `unicode-properties` dependency supplies their category.

Executable permission checks use exact-pinned `rustix` 1.1.5 (MIT OR Apache-2.0 OR
Apache-2.0 WITH LLVM-exception), already in the workspace lockfile. This exposes
Unix access checks through a safe API, including filesystem permissions. The
`which` 8.0.6 helper was considered but expands tildes in `PATH`, unlike Python's
lookup. A literal path search avoids selecting a different executable. Existing
`tempfile` 3.27.0 (MIT OR Apache-2.0) supplies isolated executable fixtures. No new
package versions are introduced.

## Consequences

- The protocol crate carries version fixtures and capability checks from the
  start.
- CI records the tmux version each runner provides; coverage for versions not
  packaged on a runner comes from recorded fixtures or a tmux built from
  source in CI ([0012](0012-testing-strategy-and-tmux-isolation.md)).

## Accept when

Recorded fixtures from 3.3 and 3.8 (and from each version in between that
behaves differently) pass the reply-parsing tests, and milestone-0 gate 5
([0013](0013-milestone-0-benchmark-method.md)) passes on 3.3 and 3.8.

If supporting 3.3 needs workarounds that make the protocol layer hard to
understand or test, raise the floor to the lowest version that avoids them, in
a superseding record that shows the evidence, and update SU-2.

## Evidence

- [Version and implementation baseline](../research/tmux-control-mode.md#version-and-implementation-baseline)
  and the [observed version discrepancy](../research/tmux-control-mode.md#process-lifetime-transport-and-protocol)
  in the control-mode research.
- [Parity checklist](../PARITY.md#startup-preflight-and-recovery), rows SU-2
  to SU-4, and [LP-16](../PARITY.md#launchers-tpm-and-packaging).
