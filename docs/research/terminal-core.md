# Terminal emulation core

Research date: **17 October 2026**. Scope: hypothesis 4, the terminal engine
behind native rendering. This is evidence for a later decision record, not a
dependency adoption or a completed benchmark gate.

## Verdict and alternatives

**Hypothesis 4: revised.** **[Inferred]** Keep libghostty-vt as the leading
candidate for the milestone-0 comparison, behind a small Rust adapter. Its
source exposes the required terminal state, Unicode handling and incremental
render data. However, the published Rust crate and current Git revision have
different APIs; Rust handles cannot simply cross threads; and neither a local
build nor parser throughput was established by this research. Compare it with
the newly separated `rio-vt` core before making performance or maintenance
claims. [G1][G2][G3][R1]

The table distinguishes observed source capabilities from the inferred choice.
All versions and repository snapshots below were inspected on the research date.

| Candidate and licence | Observed strengths | Observed limitation or integration cost | Inferred disposition |
| --- | --- | --- | --- |
| `libghostty-vt` / `libghostty-vt-sys`: MIT OR Apache-2.0; Ghostty: MIT | Scrollback, resize reflow, grapheme-aware cells, key/mouse encoders, selection, dirty render snapshots; newer Git wrapper adds search | Zig and native FFI build; explicitly unstable C API; registry/Git API mismatch; thread ownership needs care | Leading candidate, conditional on build, fidelity and throughput spikes |
| `alacritty_terminal` 0.26.0: Apache-2.0 | Rust grid, scrollback/reflow, selection, regex search, damage tracking, OSC 8/52 and kitty keyboard mode state | Character-width plus zero-width-cell model does not provide general grapheme clustering; input encoder is not supplied by this crate's mode state alone | Mature fallback if grapheme requirement can be satisfied without a large fork |
| `wezterm-term` at `e2a98e1`: MIT; `termwiz` 0.23.3: MIT | Grapheme processing, reflow, scrollback, rich escape semantics and keyboard encoding | Core depends on numerous sibling workspace crates; `termwiz` alone is a toolkit, not the complete emulator; no `wezterm-term` registry package found | Credible feature alternative, larger integration surface |
| `vt100` 0.16.2: MIT | Small parser/screen interface, scrollback, formatted screen output, OSC 52 callbacks | Resize truncates/resizes rows rather than reflowing; no OSC 8 handler; selection/search/modern input handling would need substantial work | Useful fixture oracle for its supported subset, not product engine |
| Rio `rio-vt` / `rio-backend` 0.5.28: MIT manifests; core files also retain Apache-2.0 provenance | Separate core with reflow, selection, regex search, grapheme processing, SIMD UTF-8 path and damage tracking | Default core feature enables PTY driver; disable it for this app. Some encoders and UI policy still need integration. Core extraction is recent enough to require API/build verification | Strongest additional candidate for a focused comparison |
| Build our own on `vte` 0.15.0: MIT OR Apache-2.0 | Reusable parser and dispatch interface; full control over data model | Parser does not implement screen/history/reflow/selection/search/input semantics | Reject for parity-first foundation: unnecessary emulator maintenance |

Sources for the comparison: [G1]–[G8], [A1], [W1], [V1], [R1], [P1].
These are source inspections, not comparative runtime measurements.

## What is actually pinned

**[Observed]** The registry's latest non-yanked `libghostty-vt` and
`libghostty-vt-sys` versions were **0.2.2**, published **28 September 2026**.
Their package metadata identifies wrapper commit
`c9876b2902d5bb3e1f3d61b6395059f1e5e7b721`; the packaged build script pins Ghostty
`a887df42c56f6de86c0fe6da9c4eeca37931e083`. Both declare Rust **1.90**, edition
**2024**, and `MIT OR Apache-2.0`. [G1]

**[Observed]** The newer wrapper repository snapshot inspected here is
`d2036baeb605d31d09a9e2ba6d0c923e04c34d54`, committed **9 October 2026**. It pins
Ghostty `3425025e585a3403340d4dc6d65132f42e05e605`, committed **4 October 2026**.
Its workspace manifest still says version `0.2.1`; a version string therefore
does not identify this source. In this document, **Git candidate** means that
exact wrapper/core pair, not either repository's moving default branch. [G2]

| API distinction | Published 0.2.2 | Git candidate |
| --- | --- | --- |
| Terminal construction | `Terminal::new(TerminalOptions { ... })` | `Terminal::new(cols, rows)` and terminal options through setters |
| Selection and grapheme access | Present | Present, with additional tracked references |
| Search module | Absent from public modules | `search::Search` and match snapshots present |
| Snapshot serialization module | Absent from public modules | Present; distinct from a render snapshot |
| Clearing render damage | Manage frame and row flags separately | Also provides `Snapshot::clean()` to clear both |

**[Observed]** These differences come from the downloaded registry packages and
the pinned Git source, not from assuming that current online documentation
describes the release. [G1][G2][G3][G4]

**[Inferred]** A spike needing search should pin the Git candidate's full commit,
keep its matching `-sys` crate and checked-in bindings, and record both Ghostty
and Zig revisions. A spike using 0.2.2 should explicitly leave search unproven.
Do not substitute an arbitrary installed `libghostty-vt` through `pkg-config`:
the build documentation does not promise ABI compatibility across revisions.
A future upgrade must move wrapper, headers, bindings and native library
together. [G2]

## Feature coverage and boundaries

### State, Unicode and user interaction

**[Observed]** The Git candidate exposes a byte-fed terminal, resize, viewport
scrolling, modes, grid references, formatting and scrollback controls. Ghostty's
public header documents reflow on resize. Terminal source implements grapheme
clustering and wide-cell handling; render access exposes a cell's constituent
Unicode codepoints. This supports a native terminal grid, but glyph shaping,
font fallback and ligature rendering belong to the renderer. A correct cell
width is not proof that a selected font draws it correctly. [G3][G5][G6]

**[Observed]** Selection APIs cover character ranges, rectangular selection,
word/line/all/output selection, gesture interpretation and text formatting.
The Git search API covers active and scrollback content in primary and alternate
screens, supports bounded `tick` work and terminal-dependent `feed` work, and
returns selection-shaped matches. Its matching is byte-exact except for ASCII
case folding; it is **not a regex or full Unicode case-folding search API**.
Match references are borrowed and must be re-read after terminal mutation.
The find bar, navigation shortcuts, highlights and clipboard integration remain
application work. [G4][G7]

**[Observed]** Key and mouse encoders can derive options from the terminal's
mode state, including kitty keyboard and terminal mouse formats. OSC 8 URI
access exists through grid references. OSC 52 write handling exists in 0.2.2;
the Git candidate has richer clipboard read/write callbacks and replies.
These are protocol facilities, not access to the operating-system clipboard.
The app must supply its intended interaction policy. [G3][G8]

**[Inferred]** Verify these capabilities through the tmux path, not only by
writing bytes directly to the core. tmux and the core can disagree about Unicode
widths, mode state or supported input protocols. A native key encoder does not
by itself implement tmux prefixes, key tables or copy mode. The connection
research owns that routing contract. [T1]

### Render state is not a renderer

**[Observed]** `RenderState::update` copies terminal-dependent state and consumes
terminal dirty markers. `begin_update`/`end` separate terminal access from the
remaining snapshot work. A snapshot borrows the render-state object, not the
terminal. It exposes rows, cells, styles, palette, cursor and selection data;
the Git API also exposes row identity, overscan and reusable access buffers.
Frame-level and row-level damage are independent. `clean()` clears both only
after a consumer has successfully handled the complete frame. [G3]

**[Inferred]** Give each terminal one authoritative render-state updater;
independent consumers should use extracted data rather than competing to consume
the terminal's dirty markers. Cache shaped rows in the UI, invalidate on font,
scale, palette, cursor and selection changes as well as terminal text changes,
and acknowledge damage only after the renderer has retained or drawn the update.
If UI notifications coalesce, retain all unconsumed row changes or publish a
complete owned snapshot with a revision; replacing a pending delta with only
the newest dirty rows can lose earlier changes. Dropping a presentation is safe
only when its state changes remain represented in the next consumed update.
Prefer reusable buffers over allocating a new `Vec<char>` per cell. Whether
this design is fast enough requires measurement. [G3]

**[Observed]** The VT library supplies no GPU pipeline, font shaper, glyph atlas,
native window, IME candidate positioning or accessibility tree. Ghostty's full
application renderer and its embedding surface are separate from this VT API.
The framework research must validate those concerns independently. [G5][G6]

### Ownership and tmux effects

**[Observed]** At the Git candidate, the Rust `Object` wrapper contains
`NonNull`, and `Terminal` also owns a raw callback pointer. Source inspection
found no `Send` or `Sync` implementation for these handles. Render snapshots
retain a borrow of a handle. The C API's documentation about synchronizing
terminal access is not a Rust thread-transfer guarantee. [G9]

**[Inferred]** Construct and use terminal, selection/search and render handles
on one owner thread, and send owned display data to the UI if it is on another
thread. Do not add an unsafe `Send` implementation merely to fit the planned
architecture. A compile-time trait probe and a callback/lifetime review should
settle this boundary before implementation. A thread per pane is not implied:
one worker can own multiple terminal instances. [G9]

**[Observed]** `on_pty_write` receives terminal-generated responses. tmux is
already the pane's emulator and answers queries such as DSR and DA. Forwarding
every response from a second emulator can duplicate replies. The tmux research
also identifies state recovery after pause rather than a lossless byte-stream
assumption. [G8][T1]

**[Inferred]** Separate user input and explicitly supported integration effects
from tmux-owned terminal-query replies. Define reply suppression before
connecting callbacks to `send-keys`. Maintain terminal state per pane, not merely
per session: a leaf session may contain multiple windows and panes. Screen
capture on attach is not a complete emulator snapshot, and Ghostty's own
snapshot serialization cannot recover state tmux did not export. Selection,
search and history guarantees must state what happens after reconnect, pruning
or dropped output. [T1]

## Build, portability and licence surface

**[Observed]** The Git wrapper requires Zig **0.16.x** on `PATH`; the paired
Ghostty package declares minimum Zig **0.16.0**. The build script invokes
`zig build -Demit-lib-vt=true -Dapp-runtime=none`, disables XCFramework output
for ordinary desktop builds, and links the static archive by default. It uses
baseline CPU targeting; `LIBGHOSTTY_VT_SYS_CPU` can override it. Cargo profile
settings select Debug, ReleaseSmall or ReleaseFast unless explicitly overridden.
Checked-in bindings avoid requiring bindgen/libclang for normal builds. [G2][G10]

**[Observed]** Without overrides, Cargo's build script clones the pinned Ghostty
source, and Zig may fetch native build dependencies. `GHOSTTY_SOURCE_DIR` and
`GHOSTTY_ZIG_SYSTEM_DIR` provide the documented pre-fetched source/package route.
Static linking does not promise a completely self-contained executable; native
runtime dependencies still depend on build configuration. The target map covers
Apple Silicon/Intel macOS and ARM64/x86-64 Linux GNU and musl. Upstream CI declares
macOS ARM64 and Linux ARM64/x86-64 jobs; this is evidence of configured coverage,
not a locally reproduced successful build. [G2][G10]

**[Inferred]** Pin an exact Zig 0.16 patch release for the first build, use the
macOS SDK/toolchain for native macOS linking and the intended Linux toolchain
for Linux linking, and record cold build time, cached build time, final binary
dependencies and archive size. Cross-target mappings alone do not establish a
working release pipeline. Test offline/pre-fetched builds separately from
ordinary network-enabled developer builds. [G2][G10]

| Layer | Observed licence evidence and purpose | Remaining check |
| --- | --- | --- |
| Rust wrapper and `-sys` | Manifest: MIT OR Apache-2.0; repository `LICENSE`: MIT. Safe API and raw C bindings. [G1][G2] | Preserve MIT notice; audit the exact resolved Cargo graph and enabled features |
| Ghostty core | MIT. Terminal parser, state, Unicode integration and encoders. [G10] | Include core notice alongside distributed native archive |
| SIMD native code | Build links simdutf and Highway when SIMD is enabled. Highway's pinned source uses Apache-2.0; simdutf upstream supplies MIT and Apache licences. [L1] | Verify notices in the exact vendored simdutf source, not only current upstream |
| Unicode support | Pinned uucode: MIT plus embedded MIT-style decoder and Unicode License V3 notices; generated Unicode tables are also build inputs. [L2] | Carry data/code notices and inventory generated-table provenance |
| Optional graphics | Ghostty wires Wuffs into the VT module when kitty graphics is enabled; wrapper enables its graphics feature by default. Wuffs upstream supplies MIT and Apache licences. [L3][G2] | Inspect the exact archived Wuffs dependency before shipping; do not assume Cargo feature changes remove native code without inspecting the build |

**[Observed]** Ghostty's VT build explicitly disables its Oniguruma integration
and imports terminal-specific dependencies rather than the full application
renderer. Therefore the whole Ghostty GUI dependency list is not the VT library's
licence graph. Conversely, `cargo-deny` alone cannot inventory the Zig/C/C++
dependencies. The dependency archive for Wuffs could not be inspected in this
research because retrieval returned HTTP 403; its exact bundled notices remain
an open check. [G10][L3]

**[Inferred]** There is no identified GPL dependency in the proposed direct
wrapper/core choice, but this is not a complete release licence clearance.
Audit the locked Cargo graph and native build inputs together. None of the
alternatives requires copying Zed's GPL terminal integration.

## Why the alternatives remain or fall away

**[Observed: Alacritty]** Version 0.26.0 supplies the grid, scrolling, resize
reflow, selections and regex search used by a real terminal. Its input handler
calculates width per `char` and appends zero-width characters to a previous cell.
That is narrower than full grapheme-cluster segmentation; the upstream emoji
modifier issue remains open. Its kitty mode flags must not be confused with a
complete keyboard encoder. **[Inferred]** It is attractive for Rust-only
integration, but adopting it means accepting or resolving the Unicode gap,
not dismissing that gap as font rendering. [A1]

**[Observed: WezTerm]** The pinned core handles graphemes in its performer,
rewraps primary-screen history on column changes, supports OSC 8/52, and uses
termwiz key encoding including kitty modes. Its manifest depends on sibling
cell, surface, escape-parser, bidi and dynamic-value crates as well as termwiz.
The registry lookup for `wezterm-term` returned 404; the Git core manifest is
version 0.1.0. **[Inferred]** Evaluate the Git workspace as a unit. Selection
and search UI from the full application should not be assumed to be reusable
core APIs. `termwiz` 0.23.3 alone supplies useful parsing/display/input tools,
not the stateful emulator required here. [W1]

**[Observed: vt100]** Version 0.16.2 has configurable history, wide/combining
character handling and OSC 52 callbacks, so it is more than a text stripper.
Its resize implementation clears wrapping state and resizes rows; its OSC
dispatcher has no hyperlink handler. **[Inferred]** Adding reflow, hyperlink
state, modern keyboard encoding, selection and search would defeat the reason
to choose this small library. Keep it only for narrow tests where its supported
semantics are the intended oracle. [V1]

**[Observed: Rio]** `rio-backend` 0.5.28 delegates terminal logic to `rio-vt`
0.5.28 and retains configuration/renderer integration. The latter supplies
scrollback/reflow, selection, regex search, mode-2027 grapheme handling,
OSC 8/52 handlers and damage tracking. Its native dependencies include simdutf.
Although the package declares MIT, grid, selection and search files explicitly
retain Apache-2.0 attribution to Alacritty; preserve and audit those notices too.
Its manifest, despite a lean-default README description, enables `pty` by
default. **[Inferred]** Test `rio-vt` with `default-features = false`, without
the renderer/window/clipboard/graphics features unless a requirement needs
them. This fits tmux process ownership and avoids bringing Sugarloaf into a
GPUI comparison. It deserves the same correctness corpus as libghostty-vt;
the source does not establish which is faster. [R1]

**[Observed: vte]** Version 0.15.0 parses escape-sequence actions through a
`Perform` implementation and deliberately leaves their meaning to its consumer.
**[Inferred]** Building a fresh terminal on it would make this project own
history, reflow, Unicode layout, selection, modes and protocol edge cases.
That is a poor foundation for parity-first delivery. [P1]

## Evidence still needed before adoption

No local terminal-core build, fidelity test or throughput benchmark ran for this
document. Source capability is **observed**; upstream feature descriptions are
**reported** where not supported by inspected implementation; all preferences
and proposed gates are **inferred**. No parser MB/s or renderer FPS ranking is
claimed.

1. **Build and pin spike:** build the exact Git pair on macOS ARM64 and Linux
   x86-64, then test the other supported deployment targets. Verify static
   linkage, release optimisation, enabled SIMD, offline inputs, dependency
   notices and the wrapper's thread traits. Failure selects a repair, narrower
   supported pin, or the Rio comparison; it does not silently float dependencies.
2. **Fidelity corpus:** replay complete and arbitrarily fragmented streams with
   scrollback, alternate screens, reflow, CJK, combining marks, emoji ZWJ and
   modifiers, OSC 8/52, kitty keys, mouse reports and bracketed paste. Check
   cells, cursor, modes and selection/search results. Repeat through a private
   tmux server to expose width negotiation, reply duplication and seed gaps.
3. **Parse and extraction benchmark:** compare libghostty-vt and `rio-vt` at the
   same grid, history cap, input bytes, release settings and CPU policy. Measure
   parser-only and parser-plus-render-extraction separately, then include tmux
   decoding. Use ASCII, multilingual/grapheme, escape-heavy and scrolling
   workloads; disclose chunk sizes, warmups, repetitions, median/tail times,
   peak memory and copied bytes. Verify equivalent final state before accepting
   timings. Keep unrelated builds off the timing machine.
4. **UI boundary spike:** measure owned display-data extraction, dirty-row
   shaping and presentation during adjacent-pane output. Include resize,
   palette changes, selection/search overlays and long history. Core headroom
   alone cannot establish input latency or frame pacing.

Every future tmux probe must use disposable fixtures on its own named/socket
server, an already-created private `TMUX_TMPDIR`, and an unset `TMUX`. The
milestone-0 gate owns final pass/fail thresholds; this research does not invent
performance results or approve UI feature work.

## Primary sources

Accessed **17 October 2026**. Git links use the inspected full commit. Registry
links use exact versions. Upstream issue state is an observation on that date.

- **[G1]** Published [wrapper 0.2.2 source](https://docs.rs/crate/libghostty-vt/0.2.2/source/),
  [sys 0.2.2 build script](https://docs.rs/crate/libghostty-vt-sys/0.2.2/source/build.rs),
  and [registry version metadata](https://crates.io/api/v1/crates/libghostty-vt/0.2.2).
- **[G2]** Git wrapper [manifest](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/Cargo.toml),
  [build script](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt-sys/build.rs),
  [README](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/README.md),
  [crate features](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/Cargo.toml)
  and [licence](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/LICENSE).
- **[G3]** Git wrapper [render API](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/render.rs)
  and [terminal API](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/terminal.rs).
- **[G4]** Git wrapper [search](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/search.rs)
  and [public modules](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/lib.rs).
- **[G5]** Paired Ghostty [public VT header](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/include/ghostty/vt.h)
  and [render header](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/include/ghostty/vt/render.h).
- **[G6]** Paired Ghostty [terminal implementation](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/src/terminal/Terminal.zig)
  and [stream fast paths](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/src/terminal/stream.zig).
- **[G7]** Git wrapper [selection](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/selection.rs).
- **[G8]** Git wrapper [key](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/key.rs),
  [mouse](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/mouse.rs),
  [grid references](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/screen.rs)
  and terminal callbacks in [G3].
- **[G9]** Git wrapper [opaque handle storage](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/alloc.rs#L54),
  terminal and render structs in [G3].
- **[G10]** Paired Ghostty [licence](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/LICENSE),
  [Zig package](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/build.zig.zon),
  [VT module dependencies](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/src/build/GhosttyZig.zig)
  and wrapper [CI](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/.github/workflows/ci.yml).
- **[A1]** Alacritty [0.26.0 packaged source](https://docs.rs/crate/alacritty_terminal/0.26.0/source/),
  [pinned character handling](https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/alacritty_terminal/src/term/mod.rs#L1062),
  and [emoji modifier issue](https://github.com/alacritty/alacritty/issues/3975).
- **[W1]** WezTerm [core manifest](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/term/Cargo.toml),
  [performer](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/term/src/terminalstate/performer.rs),
  [resize](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/term/src/screen.rs),
  [keyboard](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/term/src/terminalstate/keyboard.rs)
  and [termwiz 0.23.3](https://docs.rs/crate/termwiz/0.23.3/source/).
- **[V1]** vt100 0.16.2 [manifest](https://docs.rs/crate/vt100/0.16.2/source/Cargo.toml),
  [resize implementation](https://docs.rs/crate/vt100/0.16.2/source/src/grid.rs)
  and [escape handling](https://docs.rs/crate/vt100/0.16.2/source/src/perform.rs).
- **[R1]** Rio [0.5.28 core source](https://docs.rs/crate/rio-vt/0.5.28/source/),
  [core manifest](https://github.com/raphamorim/rio/blob/37624389e8dde79ba25a8147a5e809c01d16d1c9/rio-vt/Cargo.toml),
  [state and graphemes](https://github.com/raphamorim/rio/blob/37624389e8dde79ba25a8147a5e809c01d16d1c9/rio-vt/src/crosswords/mod.rs),
  [search](https://github.com/raphamorim/rio/blob/37624389e8dde79ba25a8147a5e809c01d16d1c9/rio-vt/src/crosswords/search.rs)
  and [backend manifest](https://docs.rs/crate/rio-backend/0.5.28/source/Cargo.toml).
- **[P1]** vte [0.15.0 source and README](https://docs.rs/crate/vte/0.15.0/source/).
- **[L1]** Ghostty [SIMD linkage](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/src/build/SharedDeps.zig#L886),
  pinned [Highway licence](https://github.com/google/highway/blob/66486a10623fa0d72fe91260f96c892e41aceb06/LICENSE),
  and inspected simdutf upstream [MIT](https://github.com/simdutf/simdutf/blob/11e48b24b13396fc1301b37c9b9f005d8ad2c418/LICENSE-MIT)
  and [Apache](https://github.com/simdutf/simdutf/blob/11e48b24b13396fc1301b37c9b9f005d8ad2c418/LICENSE-APACHE)
  notices. The simdutf upstream revision is not asserted to equal Ghostty's vendored copy.
- **[L2]** Pinned uucode [licence](https://github.com/jacobsandlund/uucode/blob/9d55524551411b493cca41ca06363625d90aff1e/LICENSE.md)
  and [additional notices](https://github.com/jacobsandlund/uucode/tree/9d55524551411b493cca41ca06363625d90aff1e/licenses).
- **[L3]** Ghostty [Wuffs dependency pin](https://github.com/ghostty-org/ghostty/blob/3425025e585a3403340d4dc6d65132f42e05e605/pkg/wuffs/build.zig.zon)
  and inspected upstream [licence](https://github.com/google/wuffs/blob/1f513d802343ec2f8a5c6e987cd071e655d36936/LICENSE).
  The upstream licence revision is not a verification of the inaccessible archive.
- **[T1]** Companion [tmux control-mode research](tmux-control-mode.md), which
  supplies the pinned tmux evidence and private-server probes for process,
  reply, seed, input and recovery behaviour.
