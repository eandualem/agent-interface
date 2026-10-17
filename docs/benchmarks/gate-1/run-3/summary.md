# Gate 1: headless headroom

**Gate 1: FAIL**

- ascii: libghostty-vt's timing counts
- ascii: libghostty-vt 516.3 MB/s at its slow end, 11.1 times tmux's median 46.4 MB/s (needs 5): met
- ascii: libghostty-vt 516.3 MB/s at its slow end (needs 200): met
- unicode: libghostty-vt's timing does not count: its final screen differs from alacritty_terminal's
- unicode: libghostty-vt 85.5 MB/s at its slow end, 8.3 times tmux's median 10.3 MB/s (needs 5): met
- color: libghostty-vt's timing does not count: its final screen differs from rio-vt's; its final screen differs from alacritty_terminal's
- color: libghostty-vt 60.4 MB/s at its slow end, 2.3 times tmux's median 26.8 MB/s (needs 5): not met
- scroll: libghostty-vt's timing counts
- scroll: libghostty-vt 341.5 MB/s at its slow end, 8.1 times tmux's median 42.3 MB/s (needs 5): met

## Environment

| Item | Value |
| --- | --- |
| revision | 6c320bbd7414e886e6bf063d8429d7c1d242c307 |
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
| load average before | { 2.48 2.89 2.82 } |
| load average after | { 2.54 2.89 2.82 } |
| other builds running after | none |

## Settings

- Grid 200×50 and a history limit of 10000 lines, for tmux and every engine.
- Payloads from seed 1, at least 20000000 bytes each.
- tmux: 1 warm-up and 10 timed runs per payload, each on a new private server.
- Engines: 2 warm-up and 10 timed replays each, in a child process per engine and payload.
- Replay: 65536 control-wire bytes per decode call; a read-out of the whole screen every N pane bytes, where N is what a 120 Hz display would see at tmux's median rate, and a forced read-out after the last chunk.
- Rates divide the payload's bytes, as generated, by the time.

## ascii

Payload 20000113 bytes; pane output 20178594 bytes; control wire 21402145 bytes. A read-out every 387067 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 46.4 | 45.2 | 47.9 | 45.2 | - | - | - | 9345 | - |
| libghostty-vt | 518.6 | 516.3 | 521.1 | 516.3 | 11.1 | 53 | 10.9 MB | 14513 | 51.1 MB |
| rio-vt | 694.0 | 689.1 | 696.2 | 689.1 | 14.8 | 53 | 10.9 MB | 10000 | 43.5 MB |
| alacritty_terminal | 215.1 | 204.3 | 218.3 | 204.3 | 4.4 | 53 | 11.1 MB | 10000 | 77.9 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |

## unicode

Payload 20000027 bytes; pane output 20118482 bytes; control wire 20966496 bytes. A read-out every 85497 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 10.3 | 10.1 | 10.4 | 10.1 | - | - | - | 9900 | - |
| libghostty-vt | 89.0 | 85.5 | 90.8 | 85.5 | 8.3 | 234 | 48.6 MB | 13055 | 52.1 MB |
| rio-vt | 118.1 | 112.4 | 119.0 | 112.4 | 11.0 | 234 | 48.6 MB | 10000 | 44.4 MB |
| alacritty_terminal | 133.5 | 132.3 | 135.4 | 132.3 | 12.9 | 234 | 49.7 MB | 10000 | 80.1 MB |

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

Payload 20000655 bytes; pane output 20073368 bytes; control wire 23842707 bytes. A read-out every 223357 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 26.8 | 24.2 | 27.4 | 24.2 | - | - | - | 9172 | - |
| libghostty-vt | 69.5 | 60.4 | 71.4 | 60.4 | 2.3 | 90 | 18.5 MB | 10914 | 54.6 MB |
| rio-vt | 52.8 | 51.7 | 53.0 | 51.7 | 1.9 | 90 | 18.5 MB | 10000 | 69.1 MB |
| alacritty_terminal | 166.1 | 163.6 | 166.8 | 163.6 | 6.1 | 90 | 18.9 MB | 10000 | 80.2 MB |

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

Payload 20000068 bytes; pane output 20157614 bytes; control wire 22281573 bytes. A read-out every 352103 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 42.3 | 39.3 | 42.9 | 39.3 | - | - | - | 9085 | - |
| libghostty-vt | 344.5 | 341.5 | 344.8 | 341.5 | 8.1 | 58 | 11.7 MB | 3262 | 32.3 MB |
| rio-vt | 420.3 | 419.2 | 422.9 | 419.2 | 9.9 | 58 | 11.7 MB | 10000 | 44.2 MB |
| alacritty_terminal | 198.4 | 196.8 | 201.4 | 196.8 | 4.7 | 58 | 12.2 MB | 10000 | 78.6 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |
