# Contributing to Agent Interface

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
- **Python 3.9 or newer** for preparing the native source. It uses only the
  standard library; the app itself does not need Python.
- **git and network access** for the first build. The preparation script
  fetches Ghostty at a pinned commit, and Zig then fetches Ghostty's own
  dependencies. To build without a network, see
  [Building offline](#building-offline).
- **tmux 3.3 or newer** in `PATH` to use the app's terminal sessions and run
  tests that start tmux. An empty workspace can open without starting tmux.
- **On Linux, the libraries the UI framework builds against**, for its
  Wayland and X11 backends. On Debian and Ubuntu:
  `sudo apt install pkg-config libfontconfig-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev libwebkit2gtk-4.1-dev`.
  On macOS the Command Line Tools are enough; Xcode is not needed.

Check the versions with `zig version` and `tmux -V`.

## Build and test

```sh
python3 scripts/prepare-native.py
export LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe
cargo build --workspace
cargo test --workspace
cargo run -p agent-interface-app
```

The first build takes a few minutes because it compiles Ghostty's core; later
builds reuse it.

Before submitting a change, run the local checks:

```sh
export LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  cargo deny --locked --target "$target" check
done
```

When running the tests in Docker, start the container with `--init` so exited
tmux processes are reaped.

`cargo deny` audits licences, security advisories, crate sources and the
dependency direction between the project's crates. Audit each supported target
separately: combining platforms can include impossible paths through dependencies
enabled only on macOS to their own Linux- or Android-only dependencies. Keep
all features enabled within each graph, as configured in `deny.toml`.
Install the version CI
uses with `cargo install cargo-deny --version 0.20.2 --locked`.

The CI workflow defines the same checks with `--locked` for macOS
(`macos-15`, arm64) and Linux (`ubuntu-24.04`, x86-64 and
`ubuntu-24.04-arm`, arm64). Linux graphical checks include Documents under
software Mesa and Xvfb, with interface zoom, display DPI, keyboard input,
window teardown and process-isolation checks. Record local verification in the
pull request alongside hosted CI.

## The terminal engine's native build

The `term` crate uses `libghostty-vt`, whose build script compiles Ghostty's
core with Zig. Three settings matter:

- **Build mode.** Whenever Cargo builds with debug information, as `cargo
  build` and `cargo test` do by default, Ghostty's core is built in Zig's
  Debug mode. Its additional full-page integrity scans can overwhelm the
  deadlines of bulk-output tests. Development checks and CI use
  `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe`: optimized native code with runtime
  safety checks, without those extra scans. Rust remains in its normal debug
  profile. For release builds and every measurement, set
  `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`.
- **CPU target.** The default, `baseline`, runs on any CPU of the
  architecture and is what distributed builds use.
  `LIBGHOSTTY_VT_SYS_CPU=native` is for local experiments only, and any result
  measured with it says so.
- **Source.** The build fetches Ghostty commit
  `a887df42c56f6de86c0fe6da9c4eeca37931e083`, the commit that
  `libghostty-vt-sys` 0.2.2 pins, and applies `native/ghostty.patch`. Run
  `python3 scripts/prepare-native.py` after cloning or changing native inputs.
  It verifies and reuses a prepared directory under `.native/`, named by the
  source commit and patch digest. It never edits a caller's source checkout.
  Cargo selects that directory through `.cargo/config.toml`; the `term` build
  checks the patched files against `native/ghostty.json`. An explicit
  `GHOSTTY_SOURCE_DIR` must be an absolute UTF-8 path ending in the same
  `ghostty-<commit>-<patch-sha256>` directory name and contain the same patched
  files. Treat it as immutable: prepare a new directory for each patch, never
  patch an existing override in place. The binding does not watch source files,
  so reusing a mutable path can link an old native library. An unpatched checkout
  is rejected because DEL handling now belongs to the native stream.

### Building offline

Prepare once, while online. In the prepared source directory, fetch
every Zig package that Ghostty's build can ask for. That is about 600 MB,
because it includes packages that only the full Ghostty app uses:

```sh
native_source="$(python3 scripts/prepare-native.py)"
(cd "$native_source" && zig build --fetch=all -Demit-lib-vt=true -Demit-xcframework=false -Dapp-runtime=none)
cargo fetch
```

Zig keeps the packages in its global cache, where later builds find them, as
long as `ZIG_GLOBAL_CACHE_DIR`, if you set it, has the same value. Build
without a network with:

```sh
cargo build --workspace --offline
```

## Tests that use tmux

People and tools on the same machine work in tmux sessions on its default
server, and any attached client, control mode included, takes part in window
sizing. Tests, benchmarks and scripts must never touch those sessions:

- Start tmux only through `TmuxServer` in the `agent-interface-test-support`
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
- **Review.** Every pull request is reviewed independently of its author. Include
  the checks you ran and any platform you could not verify. Once hosted CI is
  enabled, its required checks must pass before merging.
- **Architecture.** Keep [the architecture guide](docs/ARCHITECTURE.md) accurate
  when behavior or crate boundaries change. Explain the technical reason for a
  change in its pull request.
- **Dependencies.** Record each new dependency's purpose and licence in its pull
  request and keep the architecture guide's dependency summary current. Pin
  fast-moving dependencies, such as the terminal engine and UI framework, to an
  exact version. The project is MIT-licensed; do not copy GPL code into it.
- **Measurements.** Follow the [benchmark guide](docs/BENCHMARKING.md). Include
  the revision, environment, method and raw results with any performance claim.

## Build installable packages

Install the build prerequisites above and the notice generator:

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
rustup component add rust-docs
python3 scripts/package.py
```

The script builds for the host architecture, explicitly using Cargo's release
profile and Ghostty `ReleaseFast` with the portable `baseline` CPU target. It
produces an archive and adjacent SHA-256 file under `dist/`. macOS archives
contain an ad-hoc-signed `.app`; Linux archives contain the binary and notices.
No compiler or Python runtime is included or required by the installed app.
The script needs network access for hash-pinned native notices and upstream
Rust licence texts. `cargo-about` is a build-only MIT/Apache-2.0 tool.

The manual **Native packages** workflow runs this script for Apple silicon,
Intel macOS, and ARM64/x86-64 Ubuntu 24.04. It builds without uploading by default.
Enable **Upload the built archives and checksums** only when the packages are
cleared for distribution: workflow artifacts in a public repository can be
downloaded by signed-in readers, even before a release is published. A build is
not a desktop compatibility test: check launch, keyboard input, terminal output, Settings and
the library chooser on the intended desktop before publishing that target.
The packaged `BUILD.json` must identify clean, reviewed source. Audit the
unpacked binary and notices, and retain the archive checksum with the release.

Installer regression checks use inert payloads and temporary home directories:

```sh
python3 -B -m unittest discover -s scripts/tests -v
```

Test the actual archive with `sh install.sh --archive dist/<archive>.tar.gz`.
Use a separate test account for installation and upgrade checks. On macOS,
verify the real download's first-launch behavior as well as its ad-hoc signature;
a local package does not establish Gatekeeper acceptance on another Mac.
