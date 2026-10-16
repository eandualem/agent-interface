# UI framework and terminal renderer

Evidence checked **17 October 2026**. This is research for hypothesis 5, not an
architecture decision or a benchmark result. **O** means observed in the linked
source, package metadata or API; **R** means reported by its authors; **I** means
an inference or recommendation. Dates below use upstream publication dates.

## Verdict and comparison

**Hypothesis 5: revised.** **I:** GPUI remains the strongest first candidate for
one native GPU window containing the sidebar and terminal grid. Test the
compatible `gpui-pre 0.3.9` / `gpui-component 0.7.3` family, rather than mixing
crates.io `gpui`, current Zed Git dependencies and component releases. Keep the
terminal core independent of the UI. A custom GPUI element is feasible from the
available drawing APIs; Ghostty-equivalent throughput, latency and presentation
are unproven. **O:** GPUI already caches shaped line layouts, so the claim that
Zed necessarily reshapes every run on every paint is too strong. **I:** Iced is a
useful second rendering candidate, but is not an unconditional fallback because
released Iced 0.14.0 lacks a widget accessibility bridge. [G1][G2][G3][I1][I2]

The table weighs contributor comprehension, maintenance and platform facilities
alongside rendering control. Version numbers identify publications available on
the evidence date; linked source snapshots are identified separately below.
There is **no measured speed ranking** in this table.

| Candidate | Licence observed (O) | Assessment for this app (I) |
| --- | --- | --- |
| GPUI + GPUI Component | Apache-2.0 for both; Zed's terminal crates are separately GPL-3.0-or-later | First candidate: terminal and sidebar can share primitives, fonts, focus and presentation. Exact source pairing and an app-owned terminal element are required. [G1][G2][Z] |
| Iced 0.14.0 | MIT | Clear state/message architecture and custom GPU rendering path. Benchmark alternative; accessibility gap makes it a conditional fallback. [I1][I2] |
| egui/eframe 0.37.0 | MIT OR Apache-2.0 | Good small integration surface, custom drawing and AccessKit. Keep as a credible alternative; assess terminal cell placement and desktop interaction quality. Do not reject it using pre-0.35 shaping limitations. [E] |
| Slint 1.18.1 | GPL-3.0-only OR Slint Royalty-free 2.0 OR commercial Slint Software 3.0 | Capable declarative toolkit, but the non-GPL route adds its own terms and attribution, and custom terminal rendering needs integration work. Not preferred over permissively licensed candidates. [S] |
| Makepad | MIT OR Apache-2.0 for widgets | Custom GPU/text stack and live UI language offer control, with additional framework-specific knowledge to maintain. Accessibility readiness is unresolved. [M] |
| Freya | MIT | Skia, text input and AccessKit make it credible. Current release-candidate churn and renderer integration need a focused spike before choosing it. [F] |
| Floem | MIT | Reactive model and multiple renderers are attractive; current Git differs substantially from its old publication, and accessibility remains open. [L] |
| Xilem + Masonry | Apache-2.0 | Strong text/accessibility building blocks; upstream still calls the architecture experimental. A longer-term alternative, not the first integration target. [X] |
| Custom wgpu + glyphon + cosmic-text | wgpu/cosmic-text: MIT OR Apache-2.0; glyphon: MIT OR Apache-2.0 OR Zlib | Most direct control of buffers, shaders and rendering cadence. Also makes the app responsible for widgets, focus, IME, accessibility and much platform integration. Reserve for a measured framework bottleneck. [W] |
| AppKit on macOS + GTK on Linux | Apple system frameworks under SDK terms; `objc2` bindings MIT. GTK LGPL-2.1-or-later; `gtk4` Rust bindings MIT | Strong platform facilities and plausible performance. Two UI implementations increase maintenance and parity work. Rust bindings exist, so this does **not** inherently require two implementation languages. [N] |

## Platform, text and maintenance evidence

**O unless marked R:** the following are facilities present in source, not an
end-to-end usability certification. An IME is an input method editor, such as a
Japanese composition and candidate interface. An accessibility bridge exposes
semantic controls and text to assistive technology; drawing glyphs alone does
not provide that information. **I:** every custom terminal surface still needs
composition positioning, keyboard navigation and accessible text behaviour.

| Framework/source snapshot | Text and GPU path observed | IME and accessibility observed | macOS and Linux windowing observed |
| --- | --- | --- | --- |
| GPUI, Zed `93d7456` | CoreText shaping on Apple; CosmicTextSystem on Linux; cached line layouts and glyph painting; Metal on macOS, wgpu on Linux | Input-handler APIs for marked text, replacement and range bounds; AccessKit, including `accesskit_unix` in both Linux backends | macOS; Wayland and X11 feature paths. `gpui_platform` selects backends. [G1][G3][G4] |
| Iced 0.14.0 | wgpu plus tiny-skia fallback; cosmic-text 0.15 and cryoglyph; custom shader widget | IME support merged and included; no AccessKit integration in this release's workspace. Accessibility tracking issue remains open | winit; macOS plus Wayland/X11 features enabled by default. [I1][I2] |
| egui, `86099ac` / 0.37 release | epaint text/mesh rendering and GPU integrations; HarfRust shaping added in 0.35; 0.37 adds colour emoji | Composition rendering and IME handling; AccessKit. Custom painted content needs supplied semantics | Native eframe/winit macOS, Wayland and X11 paths. [E] |
| Slint 1.18.1 | Selectable FemtoVG, Skia and software paths; declarative components or custom rendered content | Accessibility feature enabled by default; winit input-method handling and backend accessibility adapter | winit backend explicitly supports macOS, Wayland and X11. [S] |
| Makepad `71c9ddc` | Own GPU stack, glyph atlases, Rustybuzz shaping and LRU-shaped-text cache | macOS IME implementation; X11 XIM preedit; Wayland text-input-v3 preedit/commit. Wayland surrounding-text deletion is unimplemented in the inspected handler. Accessibility issue #196 remains open; full bridge coverage not established | macOS and separate native Wayland/X11 implementations. [M] |
| Freya `6c6eb67` | Skia textlayout and custom canvas callbacks; own reactive core since 0.4 | AccessKit/winit integration enabled by default; IME role handling includes terminal and text inputs | winit desktop integration, macOS and Linux GPU paths; Linux X11/Wayland configuration. [F] |
| Floem `1351ffb` | Current Git uses Parley, with vger/Vello/Skia renderer choices and tiny-skia fallback | Text input handles preedit, commit, surrounding deletion and IME cursor bounds; AccessKit issue #8 still open | macOS and Linux; Wayland/X11 integration. [L] |
| Xilem `271a27a` | Masonry, Vello/wgpu and Parley/Fontique, with configurable imaging backends | AccessKit; text-area IME preedit/commit and candidate bounds. Source still has an IME cursor-movement TODO | winit desktop backend; documented Linux Wayland/X11 dependencies. [X] |
| Custom wgpu stack | glyphon prepares/renders text in a wgpu render pass; cosmic-text supplies shaping, fallback and layout | These three crates are not a widget or input-method framework. Add platform input and semantic accessibility separately | wgpu supplies graphics, not desktop windows; add a windowing layer such as winit. [W] |
| AppKit + GTK | AppKit/CoreText/Metal on macOS; GTK/Pango and a custom GPU surface on Linux | `NSTextInputClient`/NSAccessibility; GTK IMContext/AccessibleText. Custom terminal must implement relevant protocols | Platform-specific front ends; GTK has Wayland and X11 backends. [N] |

**O:** publication and source activity show different maintenance risks. GPUI
[0.2.2](https://crates.io/api/v1/crates/gpui/0.2.2) was published on 22 October 2025,
whereas [gpui-pre 0.3.9](https://crates.io/api/v1/crates/gpui-pre/0.3.9) and
[GPUI Component 0.7.3](https://crates.io/api/v1/crates/gpui-component/0.7.3)
were published on 12 October 2026. Iced's latest publication is
[0.14.0](https://crates.io/api/v1/crates/iced/0.14.0) from 7 December 2025,
with [source commits continuing in October 2026](https://github.com/iced-rs/iced/commit/3cbdacc3b0841c5145c766c3a758657d36b41ffe).
[egui 0.37.0](https://crates.io/api/v1/crates/egui/0.37.0) was published on
15 October 2026. [Slint 1.18.1](https://crates.io/api/v1/crates/slint/1.18.1)
was published on 21 September 2026. Makepad's published widgets are
[1.0.0](https://crates.io/api/v1/crates/makepad-widgets/1.0.0) from May 2025 while
its inspected development branch has October 2026 changes. Freya published
[0.5.0-rc.10](https://crates.io/api/v1/crates/freya/0.5.0-rc.10) on 11 October 2026.
Floem's registry [0.2.0](https://crates.io/api/v1/crates/floem/0.2.0) dates to
November 2024, despite September 2026 Git changes; Xilem's registry
[0.4.0](https://crates.io/api/v1/crates/xilem/0.4.0) dates to
October 2025, with September 2026 Git changes. **I:** publication age is a
packaging/upgrade signal, not proof of abandonment or quality.

**R:** Xilem describes itself as experimental; GPUI warns of frequent breaking
changes and expects current stable Rust. **I:** using one narrow app boundary
around the chosen framework limits replacement cost. Building a generic toolkit
abstraction over all these frameworks would add work without resolving the
terminal's actual integration risks. [X][G1]

### Licence implications

**O:** GPUI's Apache licence does not extend to Zed's `terminal` and
`terminal_view`, whose manifests explicitly specify GPL-3.0-or-later. They were
read for architecture only; no code is copied here. [Z]

**O:** Slint offers a royalty-free desktop/mobile/web route alongside GPL and
commercial terms; its licence requires the specified attribution and excludes
embedded use from that grant. **I:** it is inaccurate to reject Slint as
"GPL-only", but this project should prefer the simpler permissive alternatives
unless Slint wins a material integration comparison. No licence route or purchase
is selected by this research. GTK's LGPL runtime likewise needs a distribution
plan if adopted; MIT bindings do not change the runtime's licence. [S][N]

**I:** before adoption, audit the resolved dependency graph and bundled fonts,
icons and native libraries. The table records the named dependencies' licences;
it is not a claim that all possible feature combinations have identical terms.

## GPUI lineage and a reproducible candidate

**O:** these are distinct choices, not interchangeable ways to spell GPUI:

1. **Zed Git.** Pin the full commit and its platform crates together. The
   inspected current-source baseline is
   `93d745668805029476e6d7fbb19652116fed51de`, the revision recorded in
   `gpui-pre 0.3.9`. It has separate `gpui_platform`, `gpui_apple`,
   `gpui_macos`, `gpui_linux` and `gpui_wgpu` crates. Its toolchain file selects
   Rust `1.98.1`; that is an observed build baseline, not a proven minimum. [G1]
2. **crates.io `gpui 0.2.2`.** The October 2025 publication is older than the
   February 2026 replacement of GPUI's Linux Blade renderer with wgpu. Do not
   infer current Git capabilities from its unchanged package version. [G5]
3. **Longbridge snapshots.** The published `gpui-pre 0.3.9` manifest records
   the Zed revision above. GPUI Component/Kit `0.7.3` package provenance points
   to `67265a36e260ffe03c5aaebdcd8d654dbf892eab`. Their manifests require
   `gpui-pre =0.3.9`; the source explains that a caret requirement previously
   let a snapshot update break component compilation. [G2]
4. **Other forks.** GPUI-CE is Apache-2.0 and explicitly says it is only mostly
   API-compatible, with divergence increasing. Kit also offers an optional
   [Apache-2.0 gpui-fast 0.1.7](https://crates.io/api/v1/crates/gpui-fast/0.1.7)
   backend. Neither is evidence that a component release will build
   against an arbitrary fork; evaluate them as separate candidates. [G2][G6]

**I:** use exact `gpui-pre =0.3.9`, matching `gpui-pre-platform =0.3.9`, and
`gpui-component =0.7.3` for the first spike, or the exact `gpui-kit =0.7.3`
facade that re-exports the matching family. The registry manifests still allow
compatible-range updates of some `0.7.x` internal component crates, so commit
the application lockfile and inspect resolved duplicates. Choose one route;
do not add a second GPUI source to satisfy an example copied from older docs.
This pairing is verified from manifests, **not from a successful build here**.

**O:** the platform README calls out `font-kit` for macOS glyph rasterization
and Wayland and/or X11 features for Linux. Its macOS setup documents Xcode and
Metal tooling. **I:** the build spike must record actual toolchain/system
requirements for the selected package features; do not promise that command-line
tools alone suffice from an unrelated configuration. [G1]

**O:** GPUI provides a foreground executor for main-thread futures and a
background executor accepting `Send` futures. Tokio is an optional integration
crate, not the UI's mandatory runtime; Component also uses smol. **I:** keep
blocking reads and parsing away from the UI loop, and keep runtime-specific
handles out of the pure model and terminal contracts. [G7]

**O/R:** the upstream custom-fragment-shader proposal was closed unmerged in
December 2025; a maintainer explained that contributions without a Zed use case
were difficult to validate while prioritising the product. **I:** this is a
support-policy risk, not evidence GPUI stopped development. The inspected
public drawing surface offers glyphs, emoji, quads, paths and images; do not
base the initial renderer on an assumed portable custom shader callback. [G3][G5]

## What the terminal element must own

**I:** the minimum useful design is a terminal element per visible tmux pane,
inside the app's session/layout view. A workspace leaf referring to a session
does not imply that session contains just one pane. Pane geometry and identity
come from the tmux adapter. Terminal bytes and VT state stay in the headless
terminal layer; the element consumes display data. See the
[control-mode research](tmux-control-mode.md) and
[terminal-core research](terminal-core.md).

The initial dirty-row hypothesis needs four distinct stages:

1. **State changes.** **O:** the terminal-core render-state interface supplies
   cells, graphemes, styles, cursor state and dirty information, not shaped
   glyphs or a GPU renderer. **I:** preserve revisions when transporting owned
   display data across threads; dropped intermediate frames must not lose a
   row change. A queue of "latest dirty rows" alone is insufficient if later
   entries overwrite earlier updates. [terminal-core research](terminal-core.md)
2. **Shaping and layout.** **O:** GPUI checks current and previous frame caches
   before invoking platform line layout. **I:** an app row cache may save text
   assembly, run creation and cache lookup, but its extra value must be
   profiled. Cache keys/invalidation must cover content, fonts/fallbacks, size,
   style, cell metrics and scale. Terminal grapheme widths and cell positions
   remain authoritative; a general paragraph layout must not move terminal
   columns or reorder the grid. [G3]
3. **Drawing.** **O:** `paint_glyph`, `paint_emoji` and `paint_quad` submit
   primitives to GPUI's scene. **I:** reuse unchanged row geometry where useful,
   batch backgrounds, and clip to pane bounds. A cursor, selection, search
   highlight, hover or IME change can require repaint without new terminal
   bytes. Dirty rows do not by themselves guarantee dirty-only draw submission,
   GPU work, or a persistently retained framebuffer. [G8]
4. **Presentation.** **O:** GPUI separates scene drawing from presentation.
   **I:** one shared UI pipeline avoids a second toolkit/compositor boundary,
   but does not establish latency. Measure scheduling, CPU preparation, GPU
   submission and visible presentation separately. Neither fast parsing nor a
   low `paint` duration proves a prompt screen update. [G8]

**I:** bounded work per UI update is essential when output arrives continuously.
Coalesce redraw notifications while preserving all required state updates; do
not notify once per byte or block input handling behind a large output batch.
The renderer should not silently substitute its own history or geometry after
control-mode recovery. Pause/resume and seeding limitations belong to the tmux
and terminal contracts, not a rendering optimization.

**I:** input needs three paths: application commands, terminal key/mouse encoding,
and committed IME text. Preedit is a local overlay; do not send tentative text
as shell input. Test candidate positioning at the terminal cursor, UTF-16 API
offset conversion where required, cancellation on focus changes, selection,
clipboard and multiple panes. Provide the sidebar's semantic tree and the
terminal's accessible text/selection model; an AccessKit dependency alone does
not implement those behaviours. [G4][G9]

## Lessons from existing GPU terminals

These are **O** source observations at the pinned commits below; the design
lessons are **I**. None is a local performance measurement.

| Terminal | Observed structure | Lesson for this app |
| --- | --- | --- |
| Zed, `93d7456` | `terminal` wraps Alacritty state; `terminal_view` implements a GPUI terminal element, shaping text runs and drawing with the rest of the editor | An integrated element is technically credible. Calling the shaping API can still hit GPUI's cache. Read GPL terminal code only for ideas; write the MIT element independently. [Z][G3] |
| Rio (MIT), `59203cc` | Sugarloaf separates text-run caching, glyph/image atlases and rendering from the terminal frontend. Its current manifest prefers native Metal/Vulkan on macOS/Linux and makes wgpu optional there | Separate shaping data from glyph rasterization/atlas lifetime. "Rio is a wgpu renderer" is no longer a complete description of this snapshot. Adopting Sugarloaf would still require UI integration. [R] |
| WezTerm (MIT), `e2a98e1` | Line-to-element shaping cache plus cluster/glyph caches; rendering has Glium and WebGPU paths and layered vertex buffers | Cache validity includes composition, hover and cursor effects, not only bytes. Separate line layout from atlas resources and final GPU submission. [T] |
| Alacritty (Apache-2.0), `5421470` | Renderer selects GLSL3 or GLES2 text paths and draws cells through a glyph cache; crossfont provides font metrics/rasterization facilities | A compact cell renderer is a useful baseline for draw overhead. Do not equate per-cell glyph caching with full grapheme/run shaping. [A] |
| Ghostty (MIT), `e3ebb1d` | Generic renderer over Metal/OpenGL APIs, font shaping cache, separate text/background cell buffers and a renderer thread. Normal rendering is driven by wakeup messages; animations have timers | Parsing, shaping, reusable GPU data and wakeup policy are separate costs. Matching its terminal core does not reproduce the rest of Ghostty's renderer or latency. [H] |

## Open risks and deciding measurements

**I:** keep GPUI provisional until these focused checks resolve the material
unknowns. The issue calls for research only; no application build, GPU benchmark
or interactive IME/accessibility test was run for this document.

| Check | Evidence to collect and decision it enables |
| --- | --- |
| Reproducible dependency build | Build the exact GPUI family on macOS and Linux; record compiler, Zig/core pin, platform packages, features and lockfile. Inspect duplicate GPUI types, source changes and licence audit results. Establishes a viable dependency baseline, not performance. |
| Renderer-only replay | Proposed experiment; its sequencing belongs to the milestone-0 plan. Use fixed 200×50 grids plus multi-pane cases. Include ASCII, CJK, combining marks, emoji ZWJ, ligatures, colour changes and box drawing. Count actual shape calls/cache hits, allocations, prepared bytes, draw submissions and GPU time. Compare warm and cold caches and scale/font changes. |
| End-to-end presentation | Compare identical private-tmux workloads with Ghostty running tmux; record final visible completion, not just producer exit or pipe drain. Compare typing latency and frame pacing with **plain Ghostty**, the product target, under idle and neighbouring output load. Control fonts, dimensions, refresh rate, machine state and version pins; report distributions and uncertainty. |
| Desktop input and accessibility | Test macOS IMEs and VoiceOver; Linux Wayland and X11 with input methods and Orca. Verify preedit/candidate geometry, focus, keyboard-only navigation, text reading/selection, scaling and clipboard. A source API or unit test cannot certify these. |
| Sustained-output behaviour | Measure UI responsiveness, memory and power with hidden panes, scrollback, resizing, selections and reconnects. Ensure notification coalescing preserves state and bounded queues have an explicit recovery policy. |
| Fallback decision | If GPUI preparation/submission is the measured bottleneck, replay the same display data through Iced's shader widget or a minimal wgpu/glyphon renderer. If platform or accessibility integration is the blocker, compare egui or native front ends first. A new renderer cannot fix a tmux transport or terminal-state problem. |

**I:** these measurements should select the smallest architecture meeting the
contract. They do not justify starting feature UI before the project's benchmark
gate, nor selecting a more complex renderer solely because it offers more GPU
control.

## Primary sources

All links were inspected on the evidence date. Commit links are immutable
snapshots; issue status and registry metadata are dated observations. Published
crate archives were also read in memory to check manifests and
`.cargo_vcs_info.json`; no source from other projects was added to this repository.

[G1]: https://github.com/zed-industries/zed/tree/93d745668805029476e6d7fbb19652116fed51de/crates/gpui
[G2]: https://docs.rs/crate/gpui-component/0.7.3/source/Cargo.toml
[G3]: https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/text_system/line_layout.rs
[G4]: https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/input.rs
[G5]: https://github.com/zed-industries/zed/pull/46758
[G6]: https://github.com/gpui-ce/gpui-ce/blob/094fe979fc4bd9a7dd4544d75b147a9278afcecf/README.md
[G7]: https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/executor.rs
[G8]: https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/window.rs
[G9]: https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/_accessibility.rs
[I1]: https://github.com/iced-rs/iced/blob/3997291f318a8bc06fa522f5579836fb3feb94df/Cargo.toml
[I2]: https://github.com/iced-rs/iced/issues/552
[E]: https://github.com/emilk/egui/tree/86099aca1276fe786921992c8d5a68914ee79bec
[S]: https://github.com/slint-ui/slint/tree/372cf0ee5577c3dfec309a45e7b778ba4e81b734
[M]: https://github.com/makepad/makepad/tree/71c9ddcbb9c01dd208f99621fdd71024a5d35c65
[F]: https://github.com/marc2332/freya/tree/6c6eb67b579e5ca3fcc9636e93cdde79c7ffa72b
[L]: https://github.com/lapce/floem/tree/1351ffb162faeb8be983f1301d718a0d5fb59c27
[X]: https://github.com/linebender/xilem/tree/271a27a6d4a930f7878d404f9014e3c50a3a9b88
[W]: https://github.com/grovesNL/glyphon/blob/7f43114dd17bfe15916127d4132ae3b981f3d5b3/Cargo.toml
[N]: https://docs.gtk.org/gtk4/
[Z]: https://github.com/zed-industries/zed/tree/93d745668805029476e6d7fbb19652116fed51de/crates/terminal_view
[R]: https://github.com/raphamorim/rio/tree/59203ccd5fa1e6b760a738c1ad653bb29f2f3465/sugarloaf
[T]: https://github.com/wezterm/wezterm/tree/e2a98e1b8af156988ad0ac778658ac27b60a55fb/wezterm-gui/src/termwindow/render
[A]: https://github.com/alacritty/alacritty/blob/542147067dd016fc5f49120ef0ba605a98a4e3a7/alacritty/src/renderer/mod.rs
[H]: https://github.com/ghostty-org/ghostty/tree/e3ebb1d1bf772cf5d1bc70b5ae19e6943e33a110/src/renderer

- **GPUI details:** [package provenance](https://docs.rs/crate/gpui-pre/0.3.9/source/Cargo.toml.orig),
  [Kit/component source manifest](https://github.com/longbridge/gpui-kit/blob/67265a36e260ffe03c5aaebdcd8d654dbf892eab/Cargo.toml),
  [toolchain](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/rust-toolchain.toml),
  [drawing and presentation](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/window.rs),
  [Apple text](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui_apple/src/text_system.rs),
  [Linux features](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui_linux/Cargo.toml),
  [accessibility guide](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/_accessibility.rs),
  [optional Tokio bridge](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui_tokio/src/gpui_tokio.rs),
  [shader proposal and maintainer explanation](https://github.com/zed-industries/zed/pull/42905#issuecomment-3647662949).
- **Alternative input/rendering evidence:**
  [Iced shader widget](https://github.com/iced-rs/iced/tree/3997291f318a8bc06fa522f5579836fb3feb94df/widget/src/shader),
  [Iced IME merge](https://github.com/iced-rs/iced/pull/2777),
  [egui change history](https://github.com/emilk/egui/blob/86099aca1276fe786921992c8d5a68914ee79bec/CHANGELOG.md),
  [egui shaping](https://github.com/emilk/egui/blob/86099aca1276fe786921992c8d5a68914ee79bec/crates/epaint/src/text/text_layout.rs),
  [Slint backend and renderer guide](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backend_winit/),
  [Slint licence terms](https://slint.dev/agreements/slint-royalty-free-license.pdf),
  [Makepad shaper](https://github.com/makepad/makepad/blob/71c9ddcbb9c01dd208f99621fdd71024a5d35c65/draw/src/text/shaper.rs),
  [Makepad Wayland IME](https://github.com/makepad/makepad/blob/71c9ddcbb9c01dd208f99621fdd71024a5d35c65/platform/src/os/linux/wayland/wayland_state.rs),
  [Makepad accessibility issue](https://github.com/makepad/makepad/issues/196),
  [Freya desktop configuration](https://github.com/marc2332/freya/blob/6c6eb67b579e5ca3fcc9636e93cdde79c7ffa72b/crates/freya-winit/Cargo.toml),
  [Floem IME implementation](https://github.com/lapce/floem/blob/1351ffb162faeb8be983f1301d718a0d5fb59c27/src/views/text_input.rs),
  [Floem accessibility issue](https://github.com/lapce/floem/issues/8),
  [Masonry text/input/accessibility](https://github.com/linebender/xilem/blob/271a27a6d4a930f7878d404f9014e3c50a3a9b88/masonry/src/widgets/text_area.rs),
  [cosmic-text 0.19 source](https://github.com/pop-os/cosmic-text/tree/c24886c2471e5606587c46090cd25dbbf209186b),
  [wgpu 30.0.1 licence metadata](https://crates.io/api/v1/crates/wgpu/30.0.1),
  [Apple text input](https://developer.apple.com/documentation/appkit/nstextinputclient),
  [Apple accessibility](https://developer.apple.com/documentation/appkit/nsaccessibility),
  [objc2 binding licence](https://crates.io/api/v1/crates/objc2/0.6.5),
  [gtk4 binding licence](https://crates.io/api/v1/crates/gtk4/0.11.5),
  [GTK GPU integration](https://docs.gtk.org/gtk4/class.GLArea.html).
- **Terminal rendering detail:**
  [Zed terminal licence](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/terminal/Cargo.toml),
  [Zed element and licence](https://github.com/zed-industries/zed/tree/93d745668805029476e6d7fbb19652116fed51de/crates/terminal_view),
  [Rio cached text runs](https://github.com/raphamorim/rio/blob/59203ccd5fa1e6b760a738c1ad653bb29f2f3465/sugarloaf/src/font/text_run_cache.rs),
  [Rio licence manifest](https://github.com/raphamorim/rio/blob/59203ccd5fa1e6b760a738c1ad653bb29f2f3465/Cargo.toml),
  [WezTerm licence](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/LICENSE.md),
  [WezTerm render backends](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/wezterm-gui/src/renderstate.rs),
  [Alacritty licence manifest](https://github.com/alacritty/alacritty/blob/542147067dd016fc5f49120ef0ba605a98a4e3a7/alacritty/Cargo.toml),
  [Ghostty licence](https://github.com/ghostty-org/ghostty/blob/e3ebb1d1bf772cf5d1bc70b5ae19e6943e33a110/LICENSE),
  [Ghostty renderer thread](https://github.com/ghostty-org/ghostty/blob/e3ebb1d1bf772cf5d1bc70b5ae19e6943e33a110/src/renderer/Thread.zig).
