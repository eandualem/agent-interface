# 0011. Toolchain and dependency policy

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The chosen UI and terminal dependencies move fast: GPUI warns of frequent
breaking changes, and the terminal engine's C API is explicitly unstable
([0004](0004-terminal-emulation-core.md),
[0005](0005-ui-framework-and-terminal-renderer.md)). The terminal engine also
builds native code with Zig, outside Cargo's view. The project is MIT-licensed,
and GPL code may be read for ideas but never copied. Builds and benchmark
numbers need to be reproducible.

Observed facts from the research:

- Rust 1.99.1 was released on 16 October 2026. The GPUI source snapshot builds
  with Rust 1.98.1, which is a build baseline, not a published minimum.
- Cargo's Rust-version-aware resolver prefers compatible packages but can still
  pick an incompatible one, so it does not prove a minimum supported version.
- `libghostty-vt-sys` builds Ghostty's core in Zig Debug mode whenever Cargo
  passes `DEBUG=true`, which Cargo does whenever debug information is on, even
  in an optimised profile, unless `LIBGHOSTTY_VT_SYS_OPTIMIZE` is set.
- `cargo-deny` checks licences, advisories, banned crates and sources in the
  Cargo graph, but cannot see Zig or C sources, fonts or files downloaded by
  build scripts.

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Pin one tested toolchain and exact versions of fast-moving dependencies** | Reproducible builds and benchmarks; upgrades are deliberate and carry their own evidence. Someone has to do the upgrades. |
| Claim an older minimum Rust version from individual dependency manifests | Wider reach in theory, but unproven for the whole graph. |
| Float on caret requirements and the latest stable Rust | Least upkeep, but a component release or snapshot update can break the build without any change here. |

## Decision

**Rust.**

- Pin Rust **1.99.1** in `rust-toolchain.toml`, with `rustfmt` and `clippy`.
- Use edition 2024 and workspace resolver 3 ([0006](0006-workspace-and-crate-layout.md)).
- Support the pinned toolchain only. Set `rust-version` once the complete
  dependency graph builds on macOS and Linux; if that floor ever differs from
  the pinned toolchain, add a CI job for the floor.

**Rust dependencies.**

- Commit `Cargo.lock`.
- Use exact `=` versions for the UI and terminal bindings, and a full commit
  `rev` for any Git dependency. Keep the GPUI packages on one lineage.
- Every dependency added to a crate has its reason and licence recorded in the
  decision record for that part of the system, or in a new record.

**Native build inputs.**

- Pin Zig **0.15.2**, the version the published terminal engine crate
  requires. A move to the engine's Git source moves to its matching Zig 0.16
  release in the same change.
- The engine's `-sys` crate pins the Ghostty source commit; record it with
  every benchmark.
- Set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast` for release and benchmark builds,
  and record the native build mode with results.
- Distributed builds use the engine's portable `baseline` CPU target, and no
  `target-cpu=native`. A benchmark that uses native CPU targeting says so
  ([0013](0013-milestone-0-benchmark-method.md)).
- Document an offline build route (`GHOSTTY_SOURCE_DIR` with a pre-fetched
  source) separately from the normal networked build.

**Licences and audits.**

- Run `cargo deny check` (cargo-deny 0.20.2, MIT OR Apache-2.0) from the first
  dependency graph, in CI, covering every target, feature, build and
  development dependency. Allow only reviewed licences, deny unknown Git
  sources, and keep each exception narrow with its reason. Do not skip the
  audit because packages are not published.
- Copyleft licences, such as the GPL, are not allowed in the shipped graph
  without a new decision record. GPL code is never copied.
- Keep licence notices and source pins for native inputs that `cargo-deny`
  cannot see (Zig and C sources, fonts, shaders, build-script downloads) with
  the dependency's decision record, and collect them before any release.
- Check the licence of the exact crate used: a permissive crate does not make
  a copyleft file from the same repository usable.

## Consequences

- A fresh clone uses the same compiler as CI without extra steps.
- Contributors need Zig 0.15.2 and a C toolchain installed; the contributor
  guide says how.
- Upgrading Rust, GPUI or the terminal engine is a deliberate pull request
  with build evidence for both operating systems, and, for the engine, the
  matching Zig and native pins.
- Publishing crates, release binaries or packages is not part of this policy
  and needs the maintainer's approval.

## Revisit when

The project starts shipping releases and needs a supported minimum Rust version
for packagers, or a pinned dependency stops receiving fixes.

## Evidence

- [Toolchain and dependency policy](../research/rust-architecture.md#toolchain-and-dependency-policy)
  and [licence audit and contributor documentation](../research/rust-architecture.md#licence-audit-and-contributor-documentation)
  in the Rust architecture research.
- [Build, portability and licence surface](../research/terminal-core.md#build-portability-and-licence-surface)
  in the terminal-core research.
- [GPUI lineage and a reproducible candidate](../research/ui-framework.md#gpui-lineage-and-a-reproducible-candidate)
  in the UI framework research.
