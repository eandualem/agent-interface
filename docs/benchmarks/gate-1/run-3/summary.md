# Gate 1: headless headroom

**Rates: FAIL**

- ascii: libghostty-vt 525.2 MB/s at its slow end, 10.1 times tmux's median 52.2 MB/s (needs 5): met
- ascii: libghostty-vt 525.2 MB/s at its slow end (needs 200): met
- unicode: libghostty-vt 82.9 MB/s at its slow end, 8.3 times tmux's median 10.0 MB/s (needs 5): met
- color: libghostty-vt 64.9 MB/s at its slow end, 2.5 times tmux's median 26.4 MB/s (needs 5): not met
- scroll: libghostty-vt 297.6 MB/s at its slow end, 6.3 times tmux's median 46.9 MB/s (needs 5): met

## Environment

| Item | Value |
| --- | --- |
| revision | 2cc1d834a68b322d14b51e4bfff5ac52c8bb9ebf |
| os | macOS 27.0.1 (26A434) |
| cpu | Apple M5, 10 cores (4 performance, 6 efficiency) |
| memory | 16 GiB |
| power | Now drawing from 'AC Power'; lowpowermode 0 |
| other builds running before | none |
| tmux | 3.7c |
| rustc | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| zig | 0.15.2 |
| build profile | release (opt-level 3, debug info false, debug assertions false) |
| libghostty-vt | 0.1.0-dev |
| libghostty-vt optimisation | ReleaseFast |
| libghostty-vt SIMD | true |
| libghostty-vt CPU target | baseline |
| rio-vt | 0.5.28, default features off |
| alacritty_terminal | 0.26.0 |
| load average before | { 1.96 2.41 2.55 } |
| load average after | { 2.89 2.70 2.65 } |
| other builds running after | none |

## Settings

- Grid 200×50, 10000 lines of engine history; tmux keeps its default history.
- Payloads from seed 1, at least 20000000 bytes each.
- tmux: 1 warm-up and 10 timed runs per payload, each on a new private server.
- Engines: 2 warm-up and 10 timed replays each, in a child process per engine and payload.
- Replay: 65536 control-wire bytes per decode call; a read-out of the whole screen every N pane bytes, where N is what a 120 Hz display would see at tmux's median rate, and a forced read-out after the last chunk.
- Rates divide the payload's bytes, as generated, by the time.

## ascii

Payload 20000113 bytes; pane output 20178546 bytes; control wire 21401449 bytes. A read-out every 434997 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 52.2 | 45.8 | 55.2 | 45.8 | - | - | - | - |
| libghostty-vt | 528.3 | 525.2 | 531.3 | 525.2 | 10.1 | 47 | 9.6 MB | 26.5 MB |
| rio-vt | 672.3 | 649.7 | 676.7 | 649.7 | 12.4 | 47 | 9.6 MB | 43.5 MB |
| alacritty_terminal | 188.8 | 181.7 | 200.8 | 181.7 | 3.5 | 47 | 9.9 MB | 78.1 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |

## unicode

Payload 20000027 bytes; pane output 20118501 bytes; control wire 20967172 bytes. A read-out every 83418 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 10.0 | 9.9 | 10.2 | 9.9 | - | - | - | - |
| libghostty-vt | 83.6 | 82.9 | 84.3 | 82.9 | 8.3 | 240 | 49.8 MB | 27.3 MB |
| rio-vt | 114.6 | 100.1 | 117.0 | 100.1 | 10.0 | 240 | 49.8 MB | 44.3 MB |
| alacritty_terminal | 130.9 | 128.1 | 131.0 | 128.1 | 12.8 | 240 | 51.0 MB | 80.2 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | 2158 cells in 35 rows from row 0; text differs in 7 rows | 7 rows differ | 2158 cells in 35 rows from row 0; text differs in 7 rows |

First differences:

```text
alacritty_terminal against libghostty-vt:
  0,0 narrow fg=default bg=default ul=- attrs=- N
  0,0 narrow fg=default bg=default ul=- attrs=- e
alacritty_terminal against libghostty-vt:
  0,1 narrow fg=default bg=default ul=- attrs=- a
  0,1 narrow fg=default bg=default ul=- attrs=- 
alacritty_terminal against libghostty-vt:
  0,2 narrow fg=default bg=default ul=- attrs=- z
  0,2 narrow fg=default bg=default ul=- attrs=- N
alacritty_terminal against libghostty-vt:
  0,3 narrow fg=default bg=default ul=- attrs=- p
  0,3 narrow fg=default bg=default ul=- attrs=- a
alacritty_terminal against libghostty-vt:
  0,4 narrow fg=default bg=default ul=- attrs=- 
  0,4 narrow fg=default bg=default ul=- attrs=- z
alacritty_terminal against tmux's capture (tmux first):
  0,0 narrow fg=default bg=default ul=- attrs=- N
  0,0 narrow fg=default bg=default ul=- attrs=- e
alacritty_terminal against tmux's capture (tmux first):
  0,1 narrow fg=default bg=default ul=- attrs=- a
  0,1 narrow fg=default bg=default ul=- attrs=- 
alacritty_terminal against tmux's capture (tmux first):
  0,2 narrow fg=default bg=default ul=- attrs=- z
  0,2 narrow fg=default bg=default ul=- attrs=- N
alacritty_terminal against tmux's capture (tmux first):
  0,3 narrow fg=default bg=default ul=- attrs=- p
  0,3 narrow fg=default bg=default ul=- attrs=- a
alacritty_terminal against tmux's capture (tmux first):
  0,4 narrow fg=default bg=default ul=- attrs=- 
  0,4 narrow fg=default bg=default ul=- attrs=- z
```

## color

Payload 20000655 bytes; pane output 20073377 bytes; control wire 23843523 bytes. A read-out every 219751 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 26.4 | 24.7 | 26.8 | 24.7 | - | - | - | - |
| libghostty-vt | 66.6 | 64.9 | 67.3 | 64.9 | 2.5 | 91 | 18.7 MB | 30.3 MB |
| rio-vt | 52.6 | 49.1 | 52.7 | 49.1 | 1.9 | 91 | 18.7 MB | 69.0 MB |
| alacritty_terminal | 166.7 | 164.9 | 167.4 | 164.9 | 6.3 | 91 | 19.1 MB | 80.2 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | 400 cells in 4 rows from row 1; text differs in 0 rows |
| rio-vt | 1488 cells in 14 rows from row 1; text differs in 0 rows | same | 1644 cells in 15 rows from row 1; text differs in 0 rows |
| alacritty_terminal | 1959 cells in 31 rows from row 0; text differs in 0 rows | same | 2359 cells in 32 rows from row 0; text differs in 0 rows |

First differences:

```text
libghostty-vt against tmux's capture (tmux first):
  1,11 narrow fg=default bg=default ul=- attrs=- 
  1,11 narrow fg=default bg=p6 ul=- attrs=- 
libghostty-vt against tmux's capture (tmux first):
  1,12 narrow fg=default bg=default ul=- attrs=- 
  1,12 narrow fg=default bg=p6 ul=- attrs=- 
libghostty-vt against tmux's capture (tmux first):
  1,13 narrow fg=default bg=default ul=- attrs=- 
  1,13 narrow fg=default bg=p6 ul=- attrs=- 
libghostty-vt against tmux's capture (tmux first):
  1,14 narrow fg=default bg=default ul=- attrs=- 
  1,14 narrow fg=default bg=p6 ul=- attrs=- 
libghostty-vt against tmux's capture (tmux first):
  1,15 narrow fg=default bg=default ul=- attrs=- 
  1,15 narrow fg=default bg=p6 ul=- attrs=- 
rio-vt against libghostty-vt:
  1,11 narrow fg=default bg=p6 ul=- attrs=- 
  1,11 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against libghostty-vt:
  1,12 narrow fg=default bg=p6 ul=- attrs=- 
  1,12 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against libghostty-vt:
  1,13 narrow fg=default bg=p6 ul=- attrs=- 
  1,13 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against libghostty-vt:
  1,14 narrow fg=default bg=p6 ul=- attrs=- 
  1,14 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against libghostty-vt:
  1,15 narrow fg=default bg=p6 ul=- attrs=- 
  1,15 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against tmux's capture (tmux first):
  1,11 narrow fg=default bg=default ul=- attrs=- 
  1,11 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against tmux's capture (tmux first):
  1,12 narrow fg=default bg=default ul=- attrs=- 
  1,12 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against tmux's capture (tmux first):
  1,13 narrow fg=default bg=default ul=- attrs=- 
  1,13 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against tmux's capture (tmux first):
  1,14 narrow fg=default bg=default ul=- attrs=- 
  1,14 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
rio-vt against tmux's capture (tmux first):
  1,15 narrow fg=default bg=default ul=- attrs=- 
  1,15 narrow fg=p15 bg=p6 ul=- attrs=bfkrs 
alacritty_terminal against libghostty-vt:
  0,36 narrow fg=p253 bg=p125 ul=single attrs=bks 3
  0,36 narrow fg=p253 bg=p125 ul=single attrs=bs 3
alacritty_terminal against libghostty-vt:
  0,37 narrow fg=p253 bg=p125 ul=single attrs=bks 2
  0,37 narrow fg=p253 bg=p125 ul=single attrs=bs 2
alacritty_terminal against libghostty-vt:
  0,38 narrow fg=p253 bg=p125 ul=single attrs=bks 2
  0,38 narrow fg=p253 bg=p125 ul=single attrs=bs 2
alacritty_terminal against libghostty-vt:
  0,39 narrow fg=p253 bg=p125 ul=single attrs=bks 7
  0,39 narrow fg=p253 bg=p125 ul=single attrs=bs 7
alacritty_terminal against libghostty-vt:
  0,40 narrow fg=p253 bg=p125 ul=single attrs=bks 1
  0,40 narrow fg=p253 bg=p125 ul=single attrs=bs 1
alacritty_terminal against tmux's capture (tmux first):
  0,36 narrow fg=p253 bg=p125 ul=single attrs=bks 3
  0,36 narrow fg=p253 bg=p125 ul=single attrs=bs 3
alacritty_terminal against tmux's capture (tmux first):
  0,37 narrow fg=p253 bg=p125 ul=single attrs=bks 2
  0,37 narrow fg=p253 bg=p125 ul=single attrs=bs 2
alacritty_terminal against tmux's capture (tmux first):
  0,38 narrow fg=p253 bg=p125 ul=single attrs=bks 2
  0,38 narrow fg=p253 bg=p125 ul=single attrs=bs 2
alacritty_terminal against tmux's capture (tmux first):
  0,39 narrow fg=p253 bg=p125 ul=single attrs=bks 7
  0,39 narrow fg=p253 bg=p125 ul=single attrs=bs 7
alacritty_terminal against tmux's capture (tmux first):
  0,40 narrow fg=p253 bg=p125 ul=single attrs=bks 1
  0,40 narrow fg=p253 bg=p125 ul=single attrs=bs 1
```

## scroll

Payload 20000068 bytes; pane output 20157602 bytes; control wire 22282209 bytes. A read-out every 390867 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 46.9 | 42.4 | 48.6 | 42.4 | - | - | - | - |
| libghostty-vt | 320.5 | 297.6 | 321.2 | 297.6 | 6.3 | 52 | 10.5 MB | 27.5 MB |
| rio-vt | 394.1 | 385.2 | 396.4 | 385.2 | 8.2 | 52 | 10.5 MB | 44.1 MB |
| alacritty_terminal | 186.7 | 153.9 | 187.5 | 153.9 | 3.3 | 52 | 10.9 MB | 78.7 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |
