# Contributing

This guide covers setting up a machine, building and testing, the rules for
tests that use tmux, and how changes are proposed. Read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first for how the system fits
together.

## Set up

You need:

- **Rust**, installed with [rustup](https://rustup.rs). The repository's
  `rust-toolchain.toml` pins Rust 1.99.1 with rustfmt and clippy. Recent
  rustup versions install it the first time you run `cargo` here; otherwise
  run `rustup toolchain install` in the repository.
- **Zig 0.15.2**, exactly. The terminal engine builds Ghostty's terminal core
  from source with it; Zig 0.16 does not build this version.
  - macOS: `brew install zig@0.15`. The formula is not linked into your
    `PATH`, so add `$(brew --prefix zig@0.15)/bin` to it.
  - Linux: download the 0.15.2 archive for your architecture from
    [ziglang.org/download](https://ziglang.org/download/), unpack it and add
    its directory to your `PATH`.
- **A C toolchain**: the Xcode Command Line Tools on macOS
  (`xcode-select --install`), or your distribution's compiler package, such as
  `build-essential` on Debian and Ubuntu.
- **git and network access** for the first build. The engine's build script
  clones Ghostty at a pinned commit, and Zig then fetches Ghostty's own
  dependencies. To build without a network, see
  [Building offline](#building-offline).
- **tmux 3.3 or newer** for the tests that start tmux.
- **On Linux, the libraries the UI framework builds against**, for its
  Wayland and X11 backends. On Debian and Ubuntu:
  `sudo apt install pkg-config libfontconfig-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev`.
  On macOS the Command Line Tools are enough; Xcode is not needed.

Check the versions with `zig version` and `tmux -V`.

## Build and test

```sh
cargo build --workspace
cargo test --workspace
cargo run -p tmux-workspaces-app
```

The first build takes a few minutes because it compiles Ghostty's core; later
builds reuse it.

Before each commit, run the checks that CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
```

`cargo deny check` audits licences, security advisories, crate sources and the
dependency direction between the project's crates. Install the version CI
uses with `cargo install cargo-deny --version 0.20.2 --locked`.

CI runs the same commands with `--locked` on macOS (`macos-15`, arm64) and
Linux (`ubuntu-24.04`, x86-64), as set out in `.github/workflows/ci.yml`.

## The terminal engine's native build

The `term` crate uses `libghostty-vt`, whose build script compiles Ghostty's
core with Zig. Three settings matter:

- **Build mode.** Whenever Cargo builds with debug information, as `cargo
  build` and `cargo test` do by default, Ghostty's core is built in Zig's
  Debug mode. That suits tests, but it runs many times slower. For release
  builds and every measurement, set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`.
- **CPU target.** The default, `baseline`, runs on any CPU of the
  architecture and is what distributed builds use.
  `LIBGHOSTTY_VT_SYS_CPU=native` is for local experiments only, and any result
  measured with it says so.
- **Source.** The build fetches Ghostty commit
  `a887df42c56f6de86c0fe6da9c4eeca37931e083`, the commit that
  `libghostty-vt-sys` 0.2.2 pins.

### Building offline

Prepare once, while online. In a Ghostty checkout at the pinned commit, fetch
every Zig package that Ghostty's build can ask for. That is about 600 MB,
because it includes packages that only the full Ghostty app uses:

```sh
git clone https://github.com/ghostty-org/ghostty.git
cd ghostty
git checkout a887df42c56f6de86c0fe6da9c4eeca37931e083
zig build --fetch=all -Demit-lib-vt=true -Demit-xcframework=false -Dapp-runtime=none
```

Then, in this repository, download the Rust crates with `cargo fetch`.

Zig keeps the packages in its global cache, where later builds find them, as
long as `ZIG_GLOBAL_CACHE_DIR`, if you set it, has the same value. Build
without a network with:

```sh
GHOSTTY_SOURCE_DIR=/path/to/ghostty cargo build --workspace --offline
```

## Tests that use tmux

People and tools on the same machine work in tmux sessions on its default
server, and any attached client, control mode included, takes part in window
sizing. Tests, benchmarks and scripts must never touch those sessions:

- Start tmux only through `TmuxServer` in the `tmux-workspaces-test-support`
  crate. It gives every server a private `TMUX_TMPDIR` and its own `-L`
  socket name, removes `TMUX` and `TMUX_PANE` from every command (an unset
  `TMUX` alone still falls back to the default server), starts tmux with no
  user configuration, and stops only its own server when dropped.
- Never run `kill-server` on, or change options of, a server you did not
  create, and never attach to a session you did not create.
- A check against a live session is read-only (`-f read-only,ignore-size`)
  and is agreed with the maintainer before it runs.

## Making a change

- **Branches.** `develop` is the default and integration branch. Create a
  topic branch from `develop` and open the pull request into `develop`.
  `main` is the released state and receives `develop` as a merge commit when
  a release is made.
- **Commits.** Use [Conventional Commits](https://www.conventionalcommits.org)
  (`feat:`, `fix:`, `docs:`, `chore:`), with a body that says why the change
  is needed.
- **Review.** Every pull request is reviewed by someone other than its author
  and needs green CI on both platforms before it is merged.
- **Architecture.** Decisions are recorded in [docs/decisions/](docs/decisions/),
  as described in [0001](docs/decisions/0001-record-architecture-decisions.md).
  A change that goes against a record, such as a dependency that crosses the
  crate boundaries in
  [0006](docs/decisions/0006-workspace-and-crate-layout.md), needs a new
  record that supersedes it.
- **Dependencies.** Record each new dependency's licence and the reason for
  it in the decision record for that part of the system. Pin fast-moving
  dependencies, such as the terminal engine and the UI framework, to an exact
  version. The project is MIT-licensed: GPL code, such as Zed's terminal
  crates, may be read for ideas but never copied.
- **Speed claims** cite a result measured with the method in
  [0018](docs/decisions/0018-milestone-0-benchmark-method-revised.md). The
  [benchmark guide](docs/benchmarks/README.md) explains how to run the
  harness.
