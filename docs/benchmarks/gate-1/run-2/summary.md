# Gate 1: headless headroom

**Rates: FAIL**

- ascii: libghostty-vt 528.3 MB/s at its slow end, 11.3 times tmux's median 46.6 MB/s (needs 5): met
- ascii: libghostty-vt 528.3 MB/s at its slow end (needs 200): met
- unicode: libghostty-vt 77.8 MB/s at its slow end, 7.6 times tmux's median 10.3 MB/s (needs 5): met
- color: libghostty-vt 56.6 MB/s at its slow end, 2.1 times tmux's median 26.9 MB/s (needs 5): not met
- scroll: libghostty-vt 340.9 MB/s at its slow end, 8.0 times tmux's median 42.6 MB/s (needs 5): met

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
| load average before | { 2.56 2.59 2.62 } |
| load average after | { 1.96 2.41 2.55 } |
| other builds running after | none |

## Settings

- Grid 200×50, 10000 lines of engine history; tmux keeps its default history.
- Payloads from seed 1, at least 20000000 bytes each.
- tmux: 1 warm-up and 10 timed runs per payload, each on a new private server.
- Engines: 2 warm-up and 10 timed replays each, in a child process per engine and payload.
- Replay: 65536 control-wire bytes per decode call; a read-out of the whole screen every N pane bytes, where N is what a 120 Hz display would see at tmux's median rate, and a forced read-out after the last chunk.
- Rates divide the payload's bytes, as generated, by the time.

## ascii

Payload 20000113 bytes; pane output 20178569 bytes; control wire 21401973 bytes. A read-out every 388582 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 46.6 | 45.7 | 48.7 | 45.7 | - | - | - | - |
| libghostty-vt | 536.5 | 528.3 | 538.7 | 528.3 | 11.3 | 52 | 10.7 MB | 26.5 MB |
| rio-vt | 693.9 | 689.0 | 696.8 | 689.0 | 14.8 | 52 | 10.7 MB | 43.4 MB |
| alacritty_terminal | 210.6 | 204.3 | 214.2 | 204.3 | 4.4 | 52 | 10.9 MB | 77.9 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |

## unicode

Payload 20000027 bytes; pane output 20118482 bytes; control wire 20966316 bytes. A read-out every 85762 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 10.3 | 10.2 | 10.4 | 10.2 | - | - | - | - |
| libghostty-vt | 88.6 | 77.8 | 89.9 | 77.8 | 7.6 | 234 | 48.6 MB | 27.3 MB |
| rio-vt | 120.0 | 113.3 | 121.7 | 113.3 | 11.0 | 234 | 48.6 MB | 44.4 MB |
| alacritty_terminal | 131.9 | 125.2 | 133.9 | 125.2 | 12.2 | 234 | 49.7 MB | 80.2 MB |

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

Payload 20000655 bytes; pane output 20073373 bytes; control wire 23843507 bytes. A read-out every 224377 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 26.9 | 26.1 | 27.3 | 26.1 | - | - | - | - |
| libghostty-vt | 71.0 | 56.6 | 71.7 | 56.6 | 2.1 | 90 | 18.5 MB | 30.3 MB |
| rio-vt | 52.6 | 51.9 | 52.8 | 51.9 | 1.9 | 90 | 18.5 MB | 69.0 MB |
| alacritty_terminal | 163.1 | 152.5 | 163.6 | 152.5 | 5.7 | 90 | 18.9 MB | 80.2 MB |

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

Payload 20000068 bytes; pane output 20157620 bytes; control wire 22282101 bytes. A read-out every 355125 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 42.6 | 40.5 | 43.5 | 40.5 | - | - | - | - |
| libghostty-vt | 343.6 | 340.9 | 344.2 | 340.9 | 8.0 | 57 | 11.5 MB | 27.5 MB |
| rio-vt | 419.4 | 416.4 | 420.8 | 416.4 | 9.8 | 57 | 11.5 MB | 44.1 MB |
| alacritty_terminal | 189.6 | 189.0 | 191.8 | 189.0 | 4.4 | 57 | 12.0 MB | 78.7 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |
