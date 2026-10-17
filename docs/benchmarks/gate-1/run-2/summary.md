# Gate 1: headless headroom

**Gate 1: FAIL**

- ascii: libghostty-vt's timing counts
- ascii: libghostty-vt 501.6 MB/s at its slow end, 10.9 times tmux's median 46.1 MB/s (needs 5): met
- ascii: libghostty-vt 501.6 MB/s at its slow end (needs 200): met
- unicode: libghostty-vt's timing does not count: its final screen differs from alacritty_terminal's
- unicode: libghostty-vt 89.8 MB/s at its slow end, 8.7 times tmux's median 10.4 MB/s (needs 5): met
- color: libghostty-vt's timing does not count: its final screen differs from rio-vt's; its final screen differs from alacritty_terminal's
- color: libghostty-vt 69.4 MB/s at its slow end, 2.8 times tmux's median 24.9 MB/s (needs 5): not met
- scroll: libghostty-vt's timing counts
- scroll: libghostty-vt 341.3 MB/s at its slow end, 8.1 times tmux's median 42.2 MB/s (needs 5): met

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
| load average before | { 2.77 3.04 2.87 } |
| load average after | { 2.48 2.89 2.82 } |
| other builds running after | none |

## Settings

- Grid 200×50 and a history limit of 10000 lines, for tmux and every engine.
- Payloads from seed 1, at least 20000000 bytes each.
- tmux: 1 warm-up and 10 timed runs per payload, each on a new private server.
- Engines: 2 warm-up and 10 timed replays each, in a child process per engine and payload.
- Replay: 65536 control-wire bytes per decode call; a read-out of the whole screen every N pane bytes, where N is what a 120 Hz display would see at tmux's median rate, and a forced read-out after the last chunk.
- Rates divide the payload's bytes, as generated, by the time.

## ascii

Payload 20000113 bytes; pane output 20178582 bytes; control wire 21402349 bytes. A read-out every 384313 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 46.1 | 42.7 | 47.3 | 42.7 | - | - | - | 9345 | - |
| libghostty-vt | 515.0 | 501.6 | 518.3 | 501.6 | 10.9 | 53 | 10.9 MB | 14513 | 51.1 MB |
| rio-vt | 695.4 | 664.8 | 696.4 | 664.8 | 14.4 | 53 | 10.9 MB | 10000 | 43.5 MB |
| alacritty_terminal | 218.3 | 216.6 | 218.7 | 216.6 | 4.7 | 53 | 11.1 MB | 10000 | 77.9 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |

## unicode

Payload 20000027 bytes; pane output 20118497 bytes; control wire 20966124 bytes. A read-out every 86385 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 10.4 | 10.2 | 10.4 | 10.2 | - | - | - | 9900 | - |
| libghostty-vt | 91.4 | 89.8 | 92.5 | 89.8 | 8.7 | 230 | 47.7 MB | 13055 | 52.1 MB |
| rio-vt | 122.8 | 105.2 | 123.6 | 105.2 | 10.2 | 230 | 47.7 MB | 10000 | 44.3 MB |
| alacritty_terminal | 137.8 | 133.2 | 138.1 | 133.2 | 12.8 | 230 | 48.9 MB | 10000 | 80.1 MB |

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

Payload 20000655 bytes; pane output 20073359 bytes; control wire 23844147 bytes. A read-out every 207704 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 24.9 | 24.0 | 25.2 | 24.0 | - | - | - | 9172 | - |
| libghostty-vt | 72.2 | 69.4 | 72.9 | 69.4 | 2.8 | 97 | 19.9 MB | 10914 | 54.6 MB |
| rio-vt | 52.6 | 47.6 | 52.9 | 47.6 | 1.9 | 97 | 19.9 MB | 10000 | 69.1 MB |
| alacritty_terminal | 165.5 | 164.2 | 166.9 | 164.2 | 6.6 | 97 | 20.4 MB | 10000 | 80.2 MB |

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

Payload 20000068 bytes; pane output 20157598 bytes; control wire 22281785 bytes. A read-out every 351760 pane bytes.

| Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| tmux, drained control-mode client | 42.2 | 41.9 | 43.1 | 41.9 | - | - | - | 9085 | - |
| libghostty-vt | 343.7 | 341.3 | 344.5 | 341.3 | 8.1 | 58 | 11.7 MB | 3262 | 32.3 MB |
| rio-vt | 421.1 | 419.7 | 421.5 | 419.7 | 9.9 | 58 | 11.7 MB | 10000 | 44.2 MB |
| alacritty_terminal | 200.3 | 166.2 | 202.1 | 166.2 | 3.9 | 58 | 12.2 MB | 10000 | 78.6 MB |

| Final screen | Against libghostty-vt | Text and cursor against tmux | Cells against tmux's capture, as libghostty-vt reads it |
| --- | --- | --- | --- |
| libghostty-vt | - | same | same |
| rio-vt | same | same | same |
| alacritty_terminal | same | same | same |
