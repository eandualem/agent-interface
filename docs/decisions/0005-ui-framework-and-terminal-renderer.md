# 0005. UI framework and terminal renderer

- **Status:** Proposed. Waits on a reproducible build of the chosen packages on
  macOS and Linux and on milestone-0 gates 2 to 4
  ([0013](0013-milestone-0-benchmark-method.md)).
- **Date:** 2026-10-17

## Context

The app is one native window: a clickable sidebar with workspaces, tabs and the
agents roster, next to terminal panes drawn natively, never in a web view. The
panes must match Ghostty running tmux for bulk output and plain Ghostty for
typing latency and frame pacing. The window also needs input methods (IME) for
composed text, keyboard navigation and accessibility on macOS and on Linux
under both Wayland and X11.

The [UI framework research](../research/ui-framework.md) compared ten
candidates by source and licence. No build or GPU benchmark was run, so there
is no measured ranking yet.

## Options considered

| Candidate and licence | Assessment |
| --- | --- |
| **GPUI with GPUI Component**, Apache-2.0 (Zed's terminal crates are separately GPL-3.0-or-later and are not used) | Terminal and sidebar share one GPU scene, fonts, focus and presentation. Text shaping is cached. Has input-method and AccessKit support, and macOS, Wayland and X11 backends. Frequent breaking changes; package lineages must not be mixed. |
| Iced 0.14.0, MIT | Clear state and message model, custom shader widget, IME. The released version has no widget accessibility bridge. Second rendering candidate. |
| egui 0.37.0, MIT OR Apache-2.0 | Small integration surface, custom drawing, AccessKit, recent shaping improvements. Credible alternative; terminal cell placement and desktop feel need checking. |
| Slint 1.18.1, GPL-3.0-only OR royalty-free licence OR commercial | Capable, but the non-GPL route has its own terms and attribution rules, and terminal rendering needs integration work. Not preferred over permissive options. |
| Makepad (MIT OR Apache-2.0), Freya (MIT), Floem (MIT), Xilem with Masonry (Apache-2.0) | Each has strengths, but accessibility, release churn or an experimental status makes them later alternatives. |
| Custom renderer on wgpu, glyphon and cosmic-text (MIT OR Apache-2.0; glyphon also Zlib) | Most control over buffers and frame timing. The app would own widgets, focus, input methods, accessibility and windowing. Reserved for a measured framework bottleneck. |
| AppKit on macOS plus GTK on Linux (`objc2` and `gtk4` bindings, MIT; GTK runtime LGPL-2.1-or-later) | Strong platform behaviour, but two user interfaces to build and keep in parity. |

## Decision

1. Use **GPUI** for the whole window, from one package family pinned exactly:
   `gpui-pre =0.3.9`, `gpui-pre-platform =0.3.9` and `gpui-component =0.7.3`,
   or the `gpui-kit =0.7.3` facade that re-exports the same family. The build
   spike picks one of these two routes, and only one GPUI source is ever in the
   dependency graph. `gpui-pre 0.3.9` records Zed commit
   `93d745668805029476e6d7fbb19652116fed51de`.
2. Write the terminal element as the project's own MIT code: one element per
   visible tmux pane, drawing the owned display data the terminal core
   publishes ([0004](0004-terminal-emulation-core.md),
   [0007](0007-threads-ownership-and-event-ordering.md)). Zed's GPL terminal
   view may be read for ideas, never copied.
3. Keep the UI framework inside the `app` crate
   ([0006](0006-workspace-and-crate-layout.md)). No other crate depends on it,
   and there is no general toolkit abstraction over several frameworks.
4. Keep the renderer's stages separate so each can be measured: terminal state
   changes, shaping and layout, drawing, and presentation. Terminal cell
   positions from the core are authoritative; text layout must never move
   terminal columns.
5. If the renderer is the measured bottleneck, replay the same display data
   through Iced's shader widget or a minimal wgpu and glyphon renderer. If
   input methods or accessibility are the blocker, compare egui or native
   front ends first.

## Consequences

- One GPU scene holds both the sidebar and the terminals, with no second
  toolkit or compositor boundary.
- GPUI's main-thread executor drives the window; it is not a Tokio runtime
  ([0008](0008-background-io-runtime.md)).
- The exact pins and the committed lockfile keep component and core versions
  matched; upgrades move the whole family together.
- Input methods, accessible text and keyboard navigation for the terminal
  element are the project's work. Depending on AccessKit does not provide them.
- Benchmark scaffolding for gates 2 to 4 needs a minimal window that draws one
  terminal grid. It is not product UI.

## Accept when

- The chosen GPUI family builds from a clean checkout on macOS arm64 and Linux
  x86-64, with the resolved graph, duplicate GPUI types and licences checked.
- Milestone-0 gates 2 (end-to-end bulk output), 3 (typing latency) and 4 (frame
  pacing) pass, measured as [0013](0013-milestone-0-benchmark-method.md)
  describes.

Before any feature UI is released, input methods, VoiceOver on macOS, and
Orca with input methods on Linux Wayland and X11 are tested. A failure there
reopens this decision.

## Evidence

- [UI framework and terminal renderer research](../research/ui-framework.md):
  the [comparison](../research/ui-framework.md#verdict-and-comparison),
  [platform, text and maintenance evidence](../research/ui-framework.md#platform-text-and-maintenance-evidence),
  [GPUI lineage](../research/ui-framework.md#gpui-lineage-and-a-reproducible-candidate),
  [what the terminal element must own](../research/ui-framework.md#what-the-terminal-element-must-own)
  and [open risks](../research/ui-framework.md#open-risks-and-deciding-measurements).
