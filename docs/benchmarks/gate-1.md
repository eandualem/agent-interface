# Gate 1: headless headroom

**Result: failed.** `libghostty-vt` has the headroom the gate asks for on plain
text, mixed Unicode and scrolling, but not on escape-heavy coloured output,
where it runs at 2.1 to 2.8 times tmux's own rate against the five times the
gate requires. No engine measured passes on all four payloads.

The method is in [0013](../decisions/0013-milestone-0-benchmark-method.md) and
the [benchmark guide](README.md). The decision this failure needs is
described under [What happens next](#what-happens-next).

## The runs

Three runs of `tmux-workspaces-bench gate1` with its default settings, on
17 October 2026 from 00:15 to 00:19 UTC, from a clean release build of
revision `2cc1d83` in a new target directory. No compiler or build tool was
running before or after any run. The machine was not idle: other work kept
the load average between 1.9 and 3.4, about the same in every run.

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
| ascii | tmux, drained client | 46.6 / 46.6 / 52.2 | 44.3 / 45.7 / 45.8 | - |
| ascii | libghostty-vt | 537.2 / 536.5 / 528.3 | 445.4 / 528.3 / 525.2 | 9.6 / 11.3 / 10.1 |
| ascii | rio-vt | 693.0 / 693.9 / 672.3 | 665.1 / 689.0 / 649.7 | 14.3 / 14.8 / 12.4 |
| ascii | alacritty_terminal | 216.3 / 210.6 / 188.8 | 210.4 / 204.3 / 181.7 | 4.5 / 4.4 / 3.5 |
| unicode | tmux, drained client | 10.4 / 10.3 / 10.0 | 10.1 / 10.2 / 9.9 | - |
| unicode | libghostty-vt | 91.8 / 88.6 / 83.6 | 90.8 / 77.8 / 82.9 | 8.7 / 7.6 / 8.3 |
| unicode | rio-vt | 124.1 / 120.0 / 114.6 | 120.0 / 113.3 / 100.1 | 11.5 / 11.0 / 10.0 |
| unicode | alacritty_terminal | 135.5 / 131.9 / 130.9 | 115.4 / 125.2 / 128.1 | 11.1 / 12.2 / 12.8 |
| color | tmux, drained client | 25.0 / 26.9 / 26.4 | 22.8 / 26.1 / 24.7 | - |
| color | libghostty-vt | 71.7 / 71.0 / 66.6 | 70.9 / 56.6 / 64.9 | **2.8 / 2.1 / 2.5** |
| color | rio-vt | 52.5 / 52.6 / 52.6 | 47.9 / 51.9 / 49.1 | 1.9 / 1.9 / 1.9 |
| color | alacritty_terminal | 161.0 / 163.1 / 166.7 | 158.8 / 152.5 / 164.9 | 6.3 / 5.7 / 6.3 |
| scroll | tmux, drained client | 42.8 / 42.6 / 46.9 | 41.5 / 40.5 / 42.4 | - |
| scroll | libghostty-vt | 344.1 / 343.6 / 320.5 | 340.6 / 340.9 / 297.6 | 8.0 / 8.0 / 6.3 |
| scroll | rio-vt | 413.8 / 419.4 / 394.1 | 291.6 / 416.4 / 385.2 | 6.8 / 9.8 / 8.2 |
| scroll | alacritty_terminal | 196.2 / 189.6 / 186.7 | 188.4 / 189.0 / 153.9 | 4.4 / 4.4 / 3.3 |

Against the gate, for `libghostty-vt` at the slow end of its runs:

| Requirement | Runs 1 / 2 / 3 | Result |
| --- | --- | --- |
| ascii at least 200 MB/s | 445.4 / 528.3 / 525.2 MB/s | Met |
| ascii at least 5 × tmux | 9.6 / 11.3 / 10.1 | Met |
| unicode at least 5 × tmux | 8.7 / 7.6 / 8.3 | Met |
| color at least 5 × tmux | 2.8 / 2.1 / 2.5 | **Not met** |
| scroll at least 5 × tmux | 8.0 / 8.0 / 6.3 | Met |

`alacritty_terminal` passes on color but not on ascii or scroll, and `rio-vt`
fails on color.

Each `libghostty-vt` child process peaked at 26 to 31 MB of resident memory,
including the 21 to 24 MB recording it replays; `rio-vt` at 43 to 69 MB and
`alacritty_terminal` at 78 to 80 MB. One replay's read-outs copied 11 MB
(ascii, scroll) to 48 MB (unicode) of display data.

## Final screens

0013 counts an engine's timing only when its final screen and cursor match
the other engines'. The comparisons were identical in all three runs.

| Payload | `libghostty-vt` against `rio-vt` | `libghostty-vt` against `alacritty_terminal` | `libghostty-vt` against tmux: text and cursor |
| --- | --- | --- | --- |
| ascii | Same | Same | Same |
| unicode | Same | Differs: no grapheme clustering in `alacritty_terminal` | Same |
| color | Style of erased cells differs | Differs: `alacritty_terminal` stores no blink | Same |
| scroll | Same | Same | Same |

- **Unicode.** `libghostty-vt`, with grapheme clustering on, and `rio-vt`
  produce identical screens, and both match tmux's text and cursor.
  `alacritty_terminal` measures widths character by character, so emoji
  sequences take more columns and 7 of its 50 rows hold different text.
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

None of these differences is in the work an engine does, so `libghostty-vt`'s
timings count on every payload.

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
