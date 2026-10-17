# 0019. Benchmark window

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

Milestone-0 gates 2 to 4 ([0018](0018-milestone-0-benchmark-method-revised.md))
measure a window that draws tmux panes. That window is benchmark scaffolding,
not product UI, but what it measures should be the path the app will use:
control-mode output decoded by `tmux`, applied by `term` on a terminal worker,
copied into owned display data and drawn by the UI framework
([0005](0005-ui-framework-and-terminal-renderer.md),
[0007](0007-threads-ownership-and-event-ordering.md)).

0005 keeps the UI framework inside the `app` crate; no other crate depends on
it. The benchmark harness, `crates/bench`
([0017](0017-benchmark-harness.md)), starts tmux only through the private
servers of the test fixtures ([0012](0012-testing-strategy-and-tmux-isolation.md)).

Facts observed while building it, with `gpui-pre` 0.3.9 on macOS 27.0.1:

- The packages build with only the Command Line Tools when the
  `runtime_shaders` feature compiles the Metal shaders as the window opens;
  without it, the build needs Xcode's Metal compiler.
- A window stops drawing while it is covered, minimised or on another Space,
  and by default draws at most once every 33.3 ms while inactive.
- The packages bring licences that `deny.toml` did not allow, and three crates
  that the advisory database lists as unmaintained.

## Options considered

| Option | Trade-offs |
| --- | --- |
| A development-only `crates/bench-window` package | Keeps benchmark code out of `app`, but a second crate would depend on GPUI, against 0005, and its terminal code would not be the app's. |
| A hidden mode of the `tmux-workspaces` binary | Measures the shipped binary, but ships benchmark code and its private-server handling. |
| **The terminal path in an `app` library, and the window as an example of the `app` package** | GPUI stays in `app`, the window draws with the code the app will use, and an example is never installed or shipped. `app` gains a library target beside its binary. |

## Decision

**Where it lives.** The `app` package has a library target with two modules,
used by the benchmark window now and by the app later:

- `panes`: a reader thread decodes a control-mode client's output, and a
  terminal worker applies each pane's output in order and copies the panes
  that changed into owned display data. One update at most waits for the user
  interface, which takes it when it draws a frame, so the worker copies at
  most one update per frame and the update holds every change since the last.
- `view`: a GPUI view that draws the panes. Backgrounds are filled rectangles.
  Ligatures and other contextual substitutions are off. With a monospaced
  font, each run of plain ASCII text with one style is shaped as one line at
  its first column, with every character's advance forced to one cell; any
  other character, and every character of a proportional font, is shaped
  alone at its own column. So text layout never moves terminal columns. Typed keys go to one pane as
  literal bytes with `send-keys -H`
  ([0015](0015-input-routing.md)).

The window is the `bench-window` example of `app`. It attaches to a session on
a private server that the harness started, through the test fixtures'
`ServerRef`, which refuses any directory the fixtures did not create. It reads
its font and colours from the configuration file the harness gives Ghostty,
turns off GPUI's inactive-window frame limit, and is not resizable. The harness
starts it as a separate process. The window runs as an accessory app, which
never takes focus, so it opens in front of the current Space without changing
which app is active, and keys reach it only when posted to its process. To
show which code was measured, `app` uses the benchmark's build script, and the
window prints its revision and build when it starts.

**Dependencies of `app`:**

| Dependency | Version | Licence | Reason |
| --- | --- | --- | --- |
| `gpui-pre` | =0.3.9 | Apache-2.0 | The UI framework of 0005. |
| `gpui-pre-platform` | =0.3.9, features `font-kit`, `runtime_shaders`, `wayland`, `x11` | Apache-2.0 | GPUI's platform backends: Metal on macOS, Wayland and X11 on Linux. `runtime_shaders` lets macOS builds use the Command Line Tools alone. |
| `futures` | 0.3.34 | MIT OR Apache-2.0 | The channel that wakes the view, already in GPUI's graph. |

`gpui-component` is not added: one terminal grid needs none of it. 0005's
choice between `gpui-component` and the `gpui-kit` facade waits for the
sidebar.

**Licences.** `deny.toml` also allows these licences, all permissive, which
GPUI's graph uses on macOS or Linux:

| Licence | Crates |
| --- | --- |
| BSD-2-Clause | `arrayref` |
| BSD-3-Clause | `encoding_rs` (with Apache-2.0 OR MIT), `exr`, `lebe`, `sha1_smol`, `subtle`, `tiny-skia`, `tiny-skia-path` |
| ISC | `libloading` |
| Zlib | `foldhash`, `slotmap` |
| CC0-1.0 | `hexf-parse`, `tiny-keccak` |

MPL-2.0 is allowed for three crates only, each named in `deny.toml`:

- `cbindgen` 0.28.0: a build tool of `gpui-pre-apple` on macOS, not linked
  into the program.
- `option-ext` 0.2.0: linked, through `dirs` and `zed-font-kit`.
- `dwrote` 0.11.5: Windows only, through `zed-font-kit`.

MPL-2.0 applies file by file. Using the unmodified crates in an MIT program is
allowed; a distributed binary must say where their source is, which the
release's licence notices will do ([0011](0011-toolchain-and-dependency-policy.md)).

**Advisories.** Three crates in GPUI's graph are unmaintained, with no known
vulnerability, and `deny.toml` ignores exactly these advisories:
RUSTSEC-2024-0436 (`paste`, through `metal` and `image`), RUSTSEC-2026-0206
(`rustybuzz`, through `usvg`) and RUSTSEC-2026-0192 (`ttf-parser`, through
`gpui-pre` and `usvg`).

## Consequences

- Workspace builds compile GPUI and about 300 more crates. Linux builds need
  the system libraries of its Wayland and X11 backends.
- The shared build script reads the Git state, so `app` reruns it, and
  relinks, whenever a commit or the working tree changes.
- `cargo build` warns that `block` 0.1.6, from GPUI's macOS graph, has code a
  future Rust version will reject.
- The window's terminal path is the app's, so its measurements carry over to
  the app; its drawing is minimal: no pane borders, selection or input methods.
- The app must decide whether its own windows keep GPUI's inactive-window
  frame limit, which would slow output in a window without focus.

## Revisit when

GPUI is upgraded, the sidebar adds `gpui-component`, one of the ignored
advisories gains a vulnerability, or GPUI stops depending on those crates.

## Evidence

- `cargo deny check` on this branch: the licences and advisories above.
- [GPUI lineage and a reproducible candidate](../research/ui-framework.md#gpui-lineage-and-a-reproducible-candidate)
  in the UI framework research.
- [MPL-2.0](https://www.mozilla.org/en-US/MPL/2.0/), sections 3.2 and 3.3.
- Advisories [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436),
  [RUSTSEC-2026-0206](https://rustsec.org/advisories/RUSTSEC-2026-0206) and
  [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192).
