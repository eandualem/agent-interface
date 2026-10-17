# Gate 1: headless headroom

**Result: failed.** `libghostty-vt` has the headroom the gate asks for on plain
text, mixed Unicode and scrolling, but not on escape-heavy coloured output,
where it runs at 2.3 to 2.8 times tmux's own rate against the five times the
gate requires. Its unicode and color timings also do not count under 0013,
because the comparison engines' final screens differ from its own. No engine
measured passes on all four payloads.

The method is in [0013](../decisions/0013-milestone-0-benchmark-method.md) and
the [benchmark guide](README.md). The decision this failure needs is
described under [What happens next](#what-happens-next).

## The runs

Three runs of `tmux-workspaces-bench gate1` with its default settings, on
17 October 2026 from 00:38 to 00:42 UTC, from a clean release build of
revision `6c320bb` in a new target directory. No compiler or build tool was
running before or after any run. The machine was not idle: other work kept
the load average between 2.5 and 3.4, after 4.8 at the start of the first
run, just after the build.

| Item | Value |
| --- | --- |
| Machine | Apple M5, 10 cores (4 performance, 6 efficiency), 16 GiB, on AC power, low-power mode off |
| System | macOS 27.0.1 (26A434) |
| tmux | 3.7c |
| Rust | 1.99.1, release profile (opt-level 3, no debug information, no debug assertions) |
| Zig | 0.15.2 |
| `libghostty-vt` | 0.2.2 over Ghostty `a887df4`, `ReleaseFast`, SIMD on, `baseline` CPU target, grapheme clustering on |
| `rio-vt` | 0.5.28, default features off, in its own program |
| `alacritty_terminal` | 0.26.0 |

The commit after `6c320bb` changes no measured behaviour: it corrects
documentation and a test, makes the replay protocol require a build
revision, and makes the history limit's arithmetic saturate, which gives the
same limit for these settings.

Each run's report and every timing are in [gate-1/](gate-1/): `summary.md`
and `raw.tsv` for runs [1](gate-1/run-1/summary.md),
[2](gate-1/run-2/summary.md) and [3](gate-1/run-3/summary.md).

## Rates

Each payload is about 20 MB. Rates are the payload's bytes per second, in MB
(10^6 bytes). tmux's figure is the rate a drained control-mode client
receives the payload at; the engines' figures cover decoding, the engine and
reading the screen out as a 120 Hz display would.

| Payload | Path | Median MB/s, runs 1 / 2 / 3 | p95 MB/s (slow end), runs 1 / 2 / 3 | p95 × tmux median, runs 1 / 2 / 3 |
| --- | --- | --- | --- | --- |
| ascii | tmux, drained client | 46.6 / 46.1 / 46.4 | 45.9 / 42.7 / 45.2 | - |
| ascii | libghostty-vt | 517.3 / 515.0 / 518.6 | 511.6 / 501.6 / 516.3 | 11.0 / 10.9 / 11.1 |
| ascii | rio-vt | 691.9 / 695.4 / 694.0 | 673.3 / 664.8 / 689.1 | 14.4 / 14.4 / 14.8 |
| ascii | alacritty_terminal | 217.6 / 218.3 / 215.1 | 213.5 / 216.6 / 204.3 | 4.6 / 4.7 / 4.4 |
| unicode | tmux, drained client | 10.4 / 10.4 / 10.3 | 10.2 / 10.2 / 10.1 | - |
| unicode | libghostty-vt | 92.7 / 91.4 / 89.0 | 91.6 / 89.8 / 85.5 | 8.8 / 8.7 / 8.3 |
| unicode | rio-vt | 123.8 / 122.8 / 118.1 | 104.9 / 105.2 / 112.4 | 10.1 / 10.2 / 11.0 |
| unicode | alacritty_terminal | 136.2 / 137.8 / 133.5 | 135.6 / 133.2 / 132.3 | 13.1 / 12.8 / 12.9 |
| color | tmux, drained client | 24.7 / 24.9 / 26.8 | 21.8 / 24.0 / 24.2 | - |
| color | libghostty-vt | 71.2 / 72.2 / 69.5 | 62.6 / 69.4 / 60.4 | **2.5 / 2.8 / 2.3** |
| color | rio-vt | 52.8 / 52.6 / 52.8 | 52.3 / 47.6 / 51.7 | 2.1 / 1.9 / 1.9 |
| color | alacritty_terminal | 164.9 / 165.5 / 166.1 | 155.5 / 164.2 / 163.6 | 6.3 / 6.6 / 6.1 |
| scroll | tmux, drained client | 42.2 / 42.2 / 42.3 | 38.0 / 41.9 / 39.3 | - |
| scroll | libghostty-vt | 323.0 / 343.7 / 344.5 | 318.6 / 341.3 / 341.5 | 7.6 / 8.1 / 8.1 |
| scroll | rio-vt | 419.0 / 421.1 / 420.3 | 408.4 / 419.7 / 419.2 | 9.7 / 9.9 / 9.9 |
| scroll | alacritty_terminal | 201.6 / 200.3 / 198.4 | 192.2 / 166.2 / 196.8 | 4.6 / 3.9 / 4.7 |

Against the gate, for `libghostty-vt` at the slow end of its runs:

| Requirement | Runs 1 / 2 / 3 | Result |
| --- | --- | --- |
| ascii at least 200 MB/s | 511.6 / 501.6 / 516.3 MB/s | Met |
| ascii at least 5 × tmux | 11.0 / 10.9 / 11.1 | Met |
| unicode at least 5 × tmux | 8.8 / 8.7 / 8.3 | Rate met, but the timing does not count: see [Final screens](#final-screens) |
| color at least 5 × tmux | 2.5 / 2.8 / 2.3 | **Not met**, and the timing does not count |
| scroll at least 5 × tmux | 7.6 / 8.1 / 8.1 | Met |

`alacritty_terminal` meets the color rate but not the ascii or scroll rate,
and `rio-vt` fails on color.

One replay's read-outs copied 10.7 MB (ascii) to 49.7 MB (unicode) of display
data. Each child process's peak resident memory includes the 21 to 24 MB
recording it replays: `libghostty-vt` 32 to 55 MB, `rio-vt` 43 to 69 MB and
`alacritty_terminal` 78 to 80 MB.

## History

tmux, `rio-vt` and `alacritty_terminal` keep up to 10,000 lines of history.
0013 asks for the same history limit for every engine, which can only be
approximated: `libghostty-vt` limits history by memory, not lines, and the
`term` crate gives it room for about 10,000 lines. Lines kept at the end of
every run:

| Payload | tmux | libghostty-vt | rio-vt | alacritty_terminal |
| --- | ---: | ---: | ---: | ---: |
| ascii | 9,345 | 14,513 | 10,000 | 10,000 |
| unicode | 9,900 | 13,055 | 10,000 | 10,000 |
| color | 9,172 | 10,914 | 10,000 | 10,000 |
| scroll | 9,085 | 3,262 | 10,000 | 10,000 |

tmux trims its history in steps, so it holds a little under its limit. On
the scroll payload, `libghostty-vt` keeps 3,262 lines whatever memory it is
given: the four disagree about which lines scrolled out of a scroll region
enter history. That is a difference in behaviour, which the
[fidelity checks](../decisions/0004-terminal-emulation-core.md#accept-when)
must settle, not a limit; on the other payloads `libghostty-vt` keeps more
history than the others, which costs it memory rather than saving work.

## Final screens

0013 counts an engine's timing only when its final screen and cursor match
the other engines', and the gate applies that strictly. The comparisons were
identical in all three runs.

| Payload | `libghostty-vt` against `rio-vt` | `libghostty-vt` against `alacritty_terminal` | `libghostty-vt` against tmux: text and cursor |
| --- | --- | --- | --- |
| ascii | Same | Same | Same |
| unicode | Same | Differs: no grapheme clustering in `alacritty_terminal` | Same |
| color | Style of erased cells differs | Differs: `alacritty_terminal` stores no blink | Same |
| scroll | Same | Same | Same |

- **Unicode.** `libghostty-vt`, with grapheme clustering on, and `rio-vt`
  produce identical screens, and both match tmux's text and cursor.
  `alacritty_terminal` measures widths character by character, so emoji
  sequences take more columns and 7 of its 50 rows hold different text. In
  one exploratory run, whose recording split the pane's output into
  different pieces, `rio-vt` differed from both `libghostty-vt` and tmux on
  one row.
- **Color.** The text is the same everywhere. Every difference is in style:
  - `alacritty_terminal` does not store blinking, so the cells with that
    attribute differ;
  - on the cells it erases, `rio-vt` keeps the current style, such as bold
    and inverse, where `libghostty-vt` keeps only the background colour; the
    two agree on every background;
  - tmux's capture shows the default background on 400 cells that
    `libghostty-vt` shows erased with a background colour. Whether tmux's own
    state lacks that background or only `capture-pane` omits it is left for
    gate 5, which compares a pane's full state.

These differences come from what each engine stores, not from bytes an
engine skipped or processed twice, but 0013 makes no exception for them, so
the unicode and color timings do not count. Whether such differences should
disqualify a timing is part of the follow-up decision.

**Grapheme clustering.** tmux 3.4 and 3.7c each draw an emoji ZWJ sequence,
a flag, a skin-tone sequence and a VS16 sequence as one cluster two columns
wide, as a probe of nine sequences on private servers showed. `libghostty-vt`
does the same only with mode 2027 on; by default it gives these one or four
columns, so its screen drifts from tmux's. The `term` crate now turns the
mode on, and these results are measured with it. It costs about a fifth of
the unicode rate: about 115 MB/s at the median without it, in exploratory
runs.

## Why color fails

These observations come from exploratory runs outside the gate, on the same
machine and build settings.

- **It is the engine, not the read-out.** Replaying with a single read-out
  at the end gives about the same rate, and so does any write size from 512
  bytes to 1 MiB.
- **A sampling profile of `libghostty-vt` on this payload** spends most of
  its time on its per-page style table (`RefCountedSet` lookups, inserts and
  removals) and on growing pages when that table fills
  (`PageList.increaseCapacity`, `Page.clonePartialRowFrom`).
- **The payload has an almost unlimited set of styles.** Nearly every
  coloured word gets its own colour and attributes: its 139,638 direct
  foreground colours hold 139,079 different values. Mapping every SGR
  sequence onto 16 fixed styles brings `libghostty-vt` level with
  `alacritty_terminal`, about 260 MB/s of that stream. With 256 or 4,096
  styles it falls to about 160 MB/s, while `alacritty_terminal` stays near
  260. Reducing only the direct colours, to 64 levels or to the 256-colour
  palette, changes little, because the 256-colour and attribute choices
  remain random.

So `libghostty-vt`'s rate on coloured output depends on how many different
styles the output uses. Output with a theme, such as a compiler's, `ls` or a
syntax highlighter, probably uses a few dozen styles, though that has not
been measured here; output that gives almost every word a new style is the
case that fails.

## What happens next

Gate 1 stays failed, and the milestone-0 issue stays open, until the
maintainer decides how to proceed. The options:

1. **Keep `libghostty-vt` and limit the requirement for a large style
   vocabulary.** Add a coloured payload with a bounded set of styles, and
   decide whether output with an unlimited set must meet five times tmux or
   a lower bar. This changes 0013 and needs a record that supersedes it.
2. **Look for a fix in the engine.** Measure the newer Git source of
   `libghostty-vt` (wrapper `d2036ba`, Ghostty `3425025`, Zig 0.16), which
   0004 already names, and ask upstream about the style table's growth on
   pages with many styles.
3. **Change engines.** No measured engine passes all four payloads:
   `alacritty_terminal` fails ascii and scroll and lacks grapheme clustering,
   and `rio-vt` fails color.

Gates 2 to 5 measure other parts of the path and can run while this is
decided.
