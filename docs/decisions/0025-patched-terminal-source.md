# 0025: Reproducible patches to the terminal core

Status: accepted build method; performance and display acceptance remain separate.

## Decision

Keep the published `libghostty-vt` and `libghostty-vt-sys` 0.2.2 bindings, Ghostty
commit `a887df42c56f6de86c0fe6da9c4eeca37931e083` and Zig 0.15.2. Apply the small
MIT-licensed patch in `native/ghostty.patch` before building. The prepared source
retains Ghostty's own licence files. The patch changes five native source files:

- Grow page style storage according to actual occupancy, reserving active rows
  before pruning history when an allocation fails.
- Apply a complete SGR sequence's final style once, with the individual-attribute
  path retained for generic stream handlers and allocation-failure recovery.
- Handle DEL in the native stream without interrupting a partial UTF-8 character.
  This replaces the Rust DEL parser pass; native string handling stays intact.
- On a soft-wrap scroll, use default backgrounds for newly exposed blank cells,
  as tmux does. Printed text and explicit newline scrolling retain their styles.

The native C API and wrapper versions stay the same. The app continues to use
tmux as its only process layer. This build decision changes no benchmark
acceptance criterion and enables no product window, persistence or release.

## Preparing and identifying inputs

`python3 scripts/prepare-native.py` fetches the pinned commit into a temporary
directory, applies the patch and checks its modified files. It then moves the
prepared tree under `.native/` to a name containing the commit and patch SHA-256.
Existing prepared trees are verified and reused, never repaired in place.
`native/ghostty.json` records the commit, patch digest and the five file digests.
The script uses Python's standard library and git; it adds no Python runtime
dependency to the app. CI runs it before Cargo.

Cargo's environment default selects this source directory. The binding crate
watches the source path environment variable, but does not track changes within
that directory. Therefore a changed patch must have a new directory name in
`.cargo/config.toml`. A fixed mutable directory or symlink would risk reusing an
old native build. The `term` build script checks the patch and modified source
files before accepting the build. An explicit `GHOSTTY_SOURCE_DIR` must also be
an absolute, immutable path with the same commit-and-digest directory name;
mutable overrides are rejected even when their current files match. Never patch
an existing override in place: prepare a new directory for each native patch.
These checks protect the required patches; they are not an audit of arbitrary
changes elsewhere in a caller-provided source tree.

The guard uses existing workspace dependencies `sha2` (MIT OR Apache-2.0) and
`serde_json` (MIT OR Apache-2.0) as build dependencies. Benchmark reports include
the patch digest, and their dirty-input check covers the patch, preparation
script and Cargo configuration as well as Rust sources.

When changing the patch, update its digest, the modified-file digests and Cargo's
source directory together. Run the ordinary workspace checks and the native
tests for the changed areas. For this patch, from the prepared source directory:

```sh
zig build test-lib-vt -Demit-lib-vt=true -Demit-xcframework=false \
  -Dapp-runtime=none -Doptimize=Debug -Dcpu=baseline \
  -Dtest-filter=PageList -Dtest-filter=Screen -Dtest-filter=Terminal: \
  -Dtest-filter=stream: -Dtest-filter=sgr -Dtest-filter=simd:
```

## Alternatives and evidence

- Vendoring the whole native tree adds roughly 100 MB of source unrelated to
  these fixes. A pinned patch preserves the existing dependency boundary.
- Editing Cargo's registry cache or using an undocumented local checkout cannot
  reproduce the measured engine in normal contributor and CI builds.
- A fixed mutable source path does not invalidate the binding's build cache.
- Patching only benchmark builds would measure a different terminal from the app.
- Moving to a newer binding/native/Zig combination changes more than the measured
  style-storage costs and tmux fidelity differences require.

The pinned binding's `build.rs` provides `GHOSTTY_SOURCE_DIR` and its
`rerun-if-env-changed` declaration. [Cargo's environment configuration](https://doc.rust-lang.org/cargo/reference/config.html#env)
supports a default repository-relative source path. The native source and its
licence are in [Ghostty's pinned tree](https://github.com/ghostty-org/ghostty/tree/a887df42c56f6de86c0fe6da9c4eeca37931e083).
tmux's `screen_write_linefeed` calls for wrapping use the default background,
unlike explicit newline handling; the regression preserves that distinction.
