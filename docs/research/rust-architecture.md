# Rust architecture and tooling

Research date: **17 October 2026**. This document evaluates hypothesis 6 and
proposes a foundation for the desktop application. Recommendations are inputs to
decision records, not adopted decisions or implemented capabilities.

## Verdict and alternatives

**Hypothesis 6: revised.** Keep the independent model, tmux protocol, terminal
engine, roster adapter and application boundaries, but extract SQLite storage
from `core` into a sixth crate, `store`. Headless testing remains practical for
every library. Headless does not mean dependency-free: `term` still needs its
native engine and build tools, and tmux integration tests need tmux.

**Inferred:** This extra boundary protects the deterministic model and merge
rules from filesystem, database and runtime concerns. It also makes the initial
read-only compatibility phase explicit. A single `app` crate can assemble the
libraries without a general plugin system, service framework or daemon.

Throughout this document, **Observed** means inspected primary-source behavior
or metadata, **Reported** means an upstream assertion not verified here, and
**Inferred** means a recommendation or deduction. Source observations were made
on the research date. Versioned links identify inspected releases; commit links
identify repository snapshots. No application build, runtime benchmark or
cross-language compatibility test was performed for this document.

The following assessments are **Inferred**; dependency licences and capabilities
are **Observed** in the linked sources.

| Choice | Alternatives and licences | Recommendation and reason |
|---|---|---|
| Crate layout | One application crate; original five crates; six crates. Project-owned code is MIT in each case. | Six crates. One crate is initially simpler but cannot enforce dependency direction; five still couples pure model tests to SQLite. Avoid further crates until a real boundary requires them. |
| Background I/O | [Standard threads/channels](https://github.com/rust-lang/rust/blob/1.99.1/library/std/Cargo.toml), MIT OR Apache-2.0; [smol 2.0.2](https://docs.rs/crate/smol/2.0.2), MIT OR Apache-2.0; [Tokio 1.53.3](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/Cargo.toml), MIT | Provisionally Tokio for pipe I/O, timers and child supervision, isolated from UI and model APIs. Compare against blocking threads in the foundation spike; smol is a credible alternative, especially with GPUI components. No throughput advantage is established. |
| SQLite | [rusqlite 0.40.2](https://docs.rs/crate/rusqlite/0.40.2), MIT; [SQLx 0.9.0](https://docs.rs/crate/sqlx/0.9.0), MIT OR Apache-2.0; direct SQLite C API, public domain | rusqlite on one storage worker. The existing store is a small SQL interface around JSON; an async pool or handwritten unsafe bindings add little value. |
| Configuration | [toml 0.9.8](https://docs.rs/crate/toml/0.9.8), MIT OR Apache-2.0; [serde_json 1.0.154](https://docs.rs/crate/serde_json/1.0.154), MIT OR Apache-2.0; environment-only settings need no parser | Retain TOML for theme/keymap files and JSON inside SQLite. Environment-only configuration cannot represent the existing files; replacing TOML with JSON creates migration work without a parity benefit. |
| Errors | Handwritten `std::error::Error`; [thiserror 2.0.21](https://docs.rs/crate/thiserror/2.0.21), MIT OR Apache-2.0; [anyhow 1.0.104](https://docs.rs/crate/anyhow/1.0.104), MIT OR Apache-2.0 | Typed library errors, deriving with thiserror when useful; anyhow at application startup/reporting boundaries. Keep recoverable outcomes inspectable without string matching. |
| Diagnostics | Plain stderr; [tracing 0.1.44](https://docs.rs/crate/tracing/0.1.44) and [tracing-subscriber 0.3.23](https://docs.rs/crate/tracing-subscriber/0.3.23), both MIT | tracing with the subscriber configured only by `app`; structured timing and lifecycle events can relate transport, parser and frame work. |

## Crates and dependency direction

**Inferred:** Use a virtual Cargo workspace, a flat `crates/` directory, and
package names prefixed `tmux-workspaces-`. The application package may be
`tmux-workspaces-app`, but its binary must be `tmux-workspaces`. Paths below are a
proposed organization, not files that currently exist.

| Directory | Responsibility | Internal dependencies |
|---|---|---|
| `crates/core` | Workspace/tab/layout model, stable identities, validation, shared-layout merge and local navigation | None |
| `crates/store` | SQLite access, schema-v2 decoding/encoding and persistence transactions | `core` |
| `crates/tmux` | Control-protocol decoder, commands, session/pane identity, connection lifecycle | None |
| `crates/term` | Safe terminal-engine boundary, input encoding, terminal state and owned display data | None |
| `crates/roster` | Read-only roster transport and provider-specific response types | None |
| `crates/app` | UI, configuration loading, service ownership, mapping adapter data into model/actions | All five libraries |

**Inferred:** `app` maps tmux and roster identifiers into the domain model.
Neither adapter needs a dependency on `core` just to share a string identifier.
`tmux` emits pane bytes; `term` consumes them. The connection between them belongs
in application orchestration, not a dependency from either library to the other.
Do not put GPUI entities, Tokio handles, SQLite connections or Ghostty pointers in
`core`. Provider outages must remain separate from the ability to open an ordinary
tmux workspace. Keep protocol parsing separate from transport modules within
`tmux`, and roster decoding separate from polling within `roster`.

**Observed:** Cargo workspaces share a lockfile and target directory, support
inherited package metadata/dependencies/lints, and require an explicit resolver
in a virtual root. [Cargo workspace reference at `d419f52`](https://github.com/rust-lang/cargo/blob/d419f522507a7f981d19bd80a7ae8566e744a487/doc/book/src/reference/workspaces.md)

**Inferred:** Inherit edition, licence, supported Rust version and common
dependency declarations; mark workspace packages `publish = false` while they
are application internals. Use explicit member lists while the boundary set is
small. Keep the virtual root's normal test/check selection covering every member;
document `cargo run -p tmux-workspaces-app` for launching. Put library integration
tests and fixtures beside their owning crate. A future benchmark harness can
start as examples/tests rather than another production crate.

**Inferred:** Enforce dependency direction through manifests and review, not a
new architectural framework. Prefer concrete types and small modules; introduce
a trait when there is an actual second implementation or a needed test seam.
Restrict project-written unsafe code to the terminal/platform boundary with
documented invariants. Avoid a workspace-wide `forbid(unsafe_code)` that would
make the intended FFI wrapper impossible; pure crates can forbid it individually.

## Concurrency and terminal ownership

**Observed:** The GPUI snapshot underlying `gpui-pre 0.3.9` has a foreground
executor for the main thread and a background executor accepting `Send` futures.
It supplies its own scheduler; that does not make a Tokio reactor available to
arbitrary futures. The UI research covers compatible package selection.
[GPUI executor at `93d7456`](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/executor.rs),
[UI framework research](ui-framework.md)

**Observed:** At binding commit `d2036ba`, the terminal wrapper contains raw
pointers and opaque `NonNull` handles, and does not implement `Send`/`Sync` for
them. A render snapshot borrows the render state. Moving a borrowed snapshot to
the UI thread or asserting thread safety with a new unsafe implementation is not
a supported shortcut. [Allocation wrapper](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/alloc.rs),
[terminal wrapper](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/terminal.rs),
[render wrapper](https://github.com/Uzaaft/libghostty-rs/blob/d2036baeb605d31d09a9e2ba6d0c923e04c34d54/crates/libghostty-vt/src/render.rs)

**Inferred:** Keep the UI event loop on its required thread. Run a single
application-owned Tokio service runtime in the background for local/SSH child
I/O and timeouts. Construct and destroy each terminal and its render state on a
dedicated terminal worker; that worker can own multiple terminals if fairness
measurements permit. Use a separate storage worker for the retained SQLite
connection. These are threads inside the application, not additional process
layers. Core, terminal and storage APIs remain synchronous; the application
owns the scheduling and shutdown policy.

**Inferred:** Tokio is a provisional maintainability choice: its process,
timer, bounded-channel and shutdown APIs can be exercised in headless tests
without GPUI. It also adds a runtime beside GPUI's executor and potentially
smol-based component internals. A blocking-thread reference is easier to debug
for a few sessions but needs separate drains and cancellation for each child;
thread count grows with attached leaves. smol provides suitable process/I/O
facilities and could reduce ecosystem duplication. The deciding spike should
compare those costs, not assume that an async runtime makes parsing faster.

**Observed:** Tokio explicitly supports a GUI main thread with a background
runtime. Its `spawn_blocking` tasks cannot be aborted after starting, and the
documentation recommends dedicated threads for persistent blocking work. A
current-thread runtime must be driven continuously, not entered only when the UI
has work. [Tokio GUI integration guidance](https://tokio.rs/tokio/topics/bridging),
[blocking-task source, Tokio 1.53.3](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/task/blocking.rs)

**Inferred:** Define the following boundaries before measuring throughput:

- Preserve ordered bytes and persistent parser state per tmux pane. Bound queues
  by bytes, not only message count. Drain child stdout and stderr independently.
- Publish owned display rows and cursor/mode metadata from the terminal owner.
  Coalesce repaint requests and superseded complete display snapshots; never
  discard undecoded protocol bytes or ordered terminal events as an optimization.
- Give input, resize and disconnect handling progress during sustained output.
  Bound parse batches and snapshot work; queue limits and scheduling budgets
  must be measured, not invented as performance guarantees.
- Treat a tmux pause or transport gap as a recovery state. Area A finds that
  pause/continue is not a lossless replay mechanism; bounded memory alone cannot
  promise retained history. [Control-mode research](tmux-control-mode.md)
- Tag asynchronous results with a connection generation so a result from an old
  connection cannot mutate a newly connected pane. Keep application shutdown
  explicit: stop new requests, terminate/reap only child clients the app owns,
  then join workers. Persistent sessions belong to tmux.

**Observed:** Tokio's `Child` continues running when dropped by default; even
`kill_on_drop` is not a substitute for explicitly waiting when deterministic
cleanup matters. Its string `read_line` requires valid UTF-8 and is not
cancellation-safe; byte framing needs a persistent buffer and explicit bounds.
[Process source, Tokio 1.53.3](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/process/mod.rs),
[buffered-read source, Tokio 1.53.3](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/io/util/async_buf_read_ext.rs)

## SQLite and Python compatibility

**Observed:** The compatibility baseline inspected here is Python
`tmux-workspaces` commit `51de6757cd0aeda3c19fe68fa4ec5b93ab206804`
(28 September 2026). Its SQLite layout table is a container for a JSON document;
the document's `version: 2` is not evidence that `PRAGMA user_version` is 2.
[Persistence source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/persistence.py)

The contract details below are **Observed** at that commit. They identify what
the Rust adapter must test; they are not a claim that compatibility is complete.

| Surface | Existing behavior | Consequence for the Rust design (Inferred) |
|---|---|---|
| Storage | `layouts.db`; current `terminal_layout(id INTEGER PRIMARY KEY, value TEXT)`, row `id = 1`; older `shared_layout` and `layout` are retained | Open the current table read-only first. Missing/unknown schema is an actionable error, not permission to initialize or migrate. |
| Initialization | Python `Store.load` enters `BEGIN IMMEDIATE`, including when the current table exists | A read-only Rust opener cannot simply reproduce Python initialization. Separate reading from migration and writing. |
| Record decoding | Text capped at 16 MiB measured as UTF-8; duplicate JSON fields rejected | Bound input before recursive parsing and reject duplicate keys before constructing an ordinary JSON value/map. |
| Save/refresh | Save reads latest state under `BEGIN IMMEDIATE`, merges base/local/latest, validates, restores local navigation, then replaces row 1. Refresh samples `data_version` before reading the row | Preserve transaction order and refresh invalidation; a generic upsert or database pool does not implement the conflict contract. |

**Observed:** Validation also requires globally unique 12-character lowercase
hexadecimal identities, a split-tree depth at most 64, finite ratios between
0.01 and 0.99, valid navigation references, and absolute NUL-free paths. Generic
JSON containers and values have separate bounds, including unknown fields.
[Validation source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/layout_validation.py)

**Observed:** The merge removes per-viewer navigation from shared comparisons.
It merges dictionaries recursively and lists by stable IDs, with explicit order
conflicts; concurrent changes to the same split tree conflict as a unit. It is
not a last-writer-wins policy. Unknown fields survive the model's copying and
merging rather than being discarded by a fixed schema projection.
[Model and merge source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/model.py#L179)

**Observed:** `serde_json::Value` normally inserts object fields into a map,
replacing an existing value for a repeated key. Using it directly would weaken
the Python decoder's duplicate-key rejection. [serde_json 1.0.154 decoder](https://github.com/serde-rs/json/blob/v1.0.154/src/value/de.rs#L121)
**Inferred:** Keep schema-v2 wire data and validation distinct from ergonomic
in-memory types. Preserve extension fields and distinguish absent fields from
explicit values wherever merge rules do. Differential fixtures must cover JSON
number types, Unicode/path handling, depth limits and duplicates; matching a
few ordinary layouts is insufficient.

**Observed:** rusqlite's connection is `Send` but not `Sync`, supports explicit
read-only open flags and immediate transactions, and defaults to deferred
transactions. SQLx also uses a background thread for SQLite's blocking API.
[rusqlite 0.40.2 connection](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html),
[SQLx 0.9.0 connection](https://docs.rs/sqlx/0.9.0/sqlx/sqlite/struct.SqliteConnection.html)
**Inferred:** One retained connection on a storage worker is sufficient for this
JSON-record store. Prefer bundled SQLite for consistent local behavior, with the
native source included in dependency audits; compare system SQLite only if
packaging requirements warrant another build mode.

**Observed:** The inspected rusqlite release uses `libsqlite3-sys 0.38.2` (MIT),
whose bundled header names SQLite 3.53.2. SQLite's deliverable library is public
domain. Bundling uses a C compiler through the `cc` build dependency.
[Native manifest](https://github.com/rusqlite/rusqlite/blob/v0.40.2/libsqlite3-sys/Cargo.toml),
[bundled header](https://github.com/rusqlite/rusqlite/blob/v0.40.2/libsqlite3-sys/sqlite3/sqlite3.h),
[SQLite copyright statement](https://sqlite.org/copyright.html)

**Observed:** SQLite's `data_version` can only be compared across time on the
same connection. A read transaction retains its snapshot until it ends, and
`BEGIN IMMEDIATE` can fail with `SQLITE_BUSY`. Read-only WAL databases require
accessible sidecar files or conditions allowing their creation. `immutable=1`
disables locking/change detection and is invalid for a library another process
may modify. [SQLite data-version documentation](https://sqlite.org/pragma.html#pragma_data_version),
[transactions](https://sqlite.org/lang_transaction.html),
[WAL read-only behavior](https://sqlite.org/wal.html#read_only_databases),
[open flags and immutable semantics](https://sqlite.org/c3ref/open.html)

**Inferred:** Use `SQLITE_OPEN_READONLY` without creation or migrations, keep read
transactions short, and retain one polling connection. Do not change journal
mode, checkpoint, repair, or use `immutable=1` on the shared library. Report a
busy/unreadable/corrupt store while preserving the user's current arrangement.
Logical read-only access can still involve SQLite sidecars; do not promise zero
filesystem activity.

**Inferred:** Writes remain disabled until Rust passes the Python conflict
fixtures and mixed-process tests for rename/delete/move/reorder, concurrent split
edits, unknown fields, local navigation and failures. The write gate includes
legacy migration behavior; supporting v2 reads does not authorize rewriting
older records. A new private store is an alternative only after an explicit
product decision, not an automatic fallback when compatibility is difficult.

## Configuration, errors and observability

**Observed:** The Python tool already reads TOML theme and keymap files, with
CLI/environment/XDG path resolution. Its data directory precedence is explicit
`--data-dir`, then `TMUX_WORKSPACES_DATA_DIR`, then XDG data home/default. The
supervisor resolves the library path. [Theme source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/theme.py#L880),
[keymap source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/keymap.py#L464),
[CLI source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/cli.py#L12),
[supervisor source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/supervisor.py#L45)

**Inferred:** Preserve these inputs and their precedence during parity work;
do not move the macOS library to an OS-native application directory just because
a path helper defaults there. Resolve paths once at the application boundary and
pass them into libraries. Keep shell-server socket derivation and session names
compatible with the Python contract described in area A. Read-only database
compatibility does not by itself prove attachment compatibility.

**Observed:** Existing configuration writes use a sibling lock, compare the
previously read bytes, write a temporary file and replace atomically; they refuse
unsafe write targets. Uncooperative external editors leave a documented race
between the final comparison and replacement. Serialization can lose comments
and ordering. [Configuration-file source](https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/tmux_workspaces/configfile.py)
**Inferred:** Port the observable conflict behavior before enabling configuration
editing; TOML syntax compatibility alone is not save compatibility. Avoid a new
unified configuration format or general configuration framework at this stage.

**Inferred:** Expose typed errors only where callers need different responses:
malformed protocol, disconnected transport, unsupported schema, invalid layout,
database busy, and merge conflict. Keep source errors for diagnostics, but avoid
wrapping every internal function in a new error type. An unavailable roster is a
recoverable adapter failure. Terminal output and input bytes are sensitive and
high-volume; log counts, durations, queue depth and lifecycle transitions by
default, not pane contents, paths or credentials. Configure diagnostic output in
`app`, keeping libraries free of global subscriber setup.

**Inferred:** Instrument decode, terminal ingestion, display extraction, UI
submission and frame presentation as separate stages. Producer completion is
not final-frame completion. Compare bulk throughput with Ghostty running tmux;
compare typing latency and frame pacing with plain Ghostty. Include logging-off
and normal-diagnostics runs when measuring overhead. This document establishes
no benchmark threshold or speed claim.

## Toolchain and dependency policy

**Observed:** Rust 1.99.1 was released on 16 October 2026. The selected GPUI source
snapshot pins Rust 1.98.1; that is build context, not a published minimum-version
guarantee. libghostty-vt 0.2.2 declares Rust 1.90/edition 2024 and requires a Zig
build; registry and Git binding revisions expose different APIs.
[Rust 1.99.1 release](https://github.com/rust-lang/rust/releases/tag/1.99.1),
[GPUI snapshot toolchain](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/rust-toolchain.toml),
[binding manifest](https://docs.rs/crate/libghostty-vt/0.2.2/source/Cargo.toml),
[terminal-core research](terminal-core.md)

**Inferred:** Use edition 2024 and an explicit workspace resolver 3. Propose
1.99.1 as the first stable toolchain to validate, then pin the passing release in
`rust-toolchain.toml` with rustfmt/clippy. Initially support that tested toolchain
rather than claiming an older MSRV from an individual dependency's manifest.
Set `rust-version` to the documented supported floor once the complete graph
builds on both operating systems. If it later differs from the development pin,
add a separate minimum-version CI job. Dependency/toolchain upgrades should carry
their updated compatibility evidence.

**Observed:** Cargo's Rust-version-aware resolver prefers compatible packages
but can still choose an incompatible dependency when no compatible version meets
the requirements; it is not an MSRV proof. rustup supports a checked-in exact
toolchain file. [Cargo resolver at `d419f52`](https://github.com/rust-lang/cargo/blob/d419f522507a7f981d19bd80a7ae8566e744a487/doc/book/src/reference/resolver.md),
[Rust-version support expectations](https://github.com/rust-lang/cargo/blob/d419f522507a7f981d19bd80a7ae8566e744a487/doc/book/src/reference/rust-version.md),
[rustup overrides at `5d71027`](https://github.com/rust-lang/rustup/blob/5d710274c9422fc184fa27c2ad26d5275c9dd6a9/doc/user-guide/src/overrides.md)

**Inferred:** Commit `Cargo.lock`; use exact versions for the fast-moving UI and
terminal bindings and a full `rev` for Git sources. Keep compatible GPUI packages
on the same lineage. The lockfile records ordinary transitive Rust dependencies;
also pin Zig, Ghostty's source commit and native build inputs. `--locked` does not
make arbitrary build-script downloads reproducible. Record target architecture,
SDK/compiler, native library versions, features and native optimization settings
with benchmark results. Do not use `target-cpu=native` in distributed binaries
without an explicit portability decision.

## Testing and continuous integration

**Inferred:** Build the smallest test layer that exercises each boundary. All
items below are proposed validation, not completed tests.

| Layer | Evidence to produce |
|---|---|
| Pure model | Examples for layout operations; property tests for valid identities/structure and unchanged edits; Python differential tests for merge results and conflicts. Do not assume merge commutativity when ordering rules are asymmetric. |
| Protocol and terminal | Split fixtures at every relevant byte boundary, including escaped bytes/UTF-8/VT sequences; disconnect and reconnect; malformed records; version-specific notification interleaving inside command replies; snapshot lifetime checks; owned display-data snapshots with a fixed viewport. |
| Store | Synthetic schema-v2 databases, duplicate/oversized/deep records, missing tables/rows, read-only enforcement, retained-connection refresh, busy errors and two-writer conflicts. |
| Real tmux | Disposable sessions on private servers; process exit/reaping, resize, multiple windows/panes, output pressure and recovery. Version coverage follows area A's capability findings. |
| Roster | Fixture responses and a local fake endpoint; absent provider, timeouts, invalid responses and recovery. Ordinary tests require neither a live roster service nor credentials. |
| UI/native boundary | First build and headless library tests; after the benchmark gate, native smoke tests for input, IME and rendering on macOS plus Linux Wayland and X11. Hosted CI build success is not display-latency evidence. |

**Inferred:** Use Rust's test harness first. [proptest 1.12.0](https://docs.rs/crate/proptest/1.12.0)
(MIT OR Apache-2.0) is appropriate for trees, edit sequences and protocol
chunking; retain regression seeds. [insta 1.49.0](https://docs.rs/crate/insta/1.49.0)
(Apache-2.0) can keep selected decoded/display snapshots reviewable; ordinary
assertions suffice where a snapshot adds no clarity. [tempfile 3.27.0](https://docs.rs/crate/tempfile/3.27.0)
(MIT OR Apache-2.0) supplies isolated fixture directories. Defer
[cargo-nextest 0.9.148](https://docs.rs/crate/cargo-nextest/0.9.148)
(MIT OR Apache-2.0) until suite size or process isolation justifies another tool;
it does not replace the required `cargo test` baseline.

**Observed:** proptest supports persisted failure seeds; Insta can run with
snapshot updates disabled in CI. [proptest persistence API](https://docs.rs/proptest/1.12.0/proptest/test_runner/enum.FileFailurePersistence.html),
[Insta snapshot modes](https://docs.rs/insta/1.49.0/insta/index.html#updating-snapshots)

**Inferred:** Centralize test tmux spawning in a fixture helper that creates its
private `TMUX_TMPDIR` before launch, unsets `TMUX` on every command, and always
supplies `-L` or `-S`. Cleanup must address only that created server, including on
failure. No test may attach to existing sessions. Closing a worker must not
accidentally execute a default-server command. These checks are especially
important when running tests concurrently.

**Observed:** The runner-images inventory at `ee1ff45` lists `macos-15` as arm64,
`macos-15-intel` as x64, and `ubuntu-24.04` as x64. Named OS images still receive
software updates, whereas `*-latest` can also change OS versions.
[Runner inventory](https://github.com/actions/runner-images/blob/ee1ff4569e381a9649b400338bdc8ceb7e18a80e/README.md)

**Inferred:** Start CI with explicit macOS arm64 and Linux x64 images, using the
same locked graph and Rust/Zig versions. Compile both Linux display backends as
required by the chosen UI family; add actual display tests after a UI exists.
Add Intel macOS/Linux arm64 coverage when those targets become supported, not
by claiming that cross-compilation proves runtime behavior. Install native
dependencies from the selected UI/terminal recipes and print their versions.

**Inferred:** The workspace setup should run `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, and `cargo doc --workspace --no-deps --locked`
with rustdoc warnings denied. Keep doctests in the test job. Cache keys include
OS, architecture, toolchain, lockfile and native-source/build settings. Pin
Actions by full commit and grant read-only repository access. CI should verify
build/test/audit work; publishing or signing belongs to a separately authorized
release workflow.

## Licence audit and contributor documentation

**Observed:** [cargo-deny 0.20.2](https://docs.rs/crate/cargo-deny/0.20.2)
(MIT OR Apache-2.0) provides advisory, licence, banned-dependency and source
checks. Its template supports explicit licence allowances, source allowances and
per-package clarifications, and can include unpublished workspace crates.
[Configuration at `bca0dde`](https://github.com/EmbarkStudios/cargo-deny/blob/bca0dde53651ee946720e4540b5ce2610bec8f06/deny.template.toml)

**Inferred:** Introduce `cargo deny check` with the first dependency graph, not
after the renderer is complete. Review all supported target/feature paths and
dev/build dependencies. Permit only reviewed licences, disallow unknown Git
sources, and keep exceptions narrow with their reason and evidence. Do not turn
off audits because internal crates have `publish = false`. Check the actual
per-crate licence: a permissive UI crate does not grant permission to copy a
copyleft terminal view from the same repository.

**Inferred:** A Cargo graph check is necessary but insufficient for Zig/C
sources, fonts, shader assets and downloads performed by build scripts. Maintain
their licence notices and source pins alongside the relevant dependency record.
The direct licences in this document are an inventory, not a completed audit of
an unresolved future lockfile. [Serde 1.0.229](https://docs.rs/crate/serde/1.0.229)
(MIT OR Apache-2.0) is the remaining proposed serialization dependency; each
future dependency still needs its own reason and licence recorded.

**Inferred:** Contributor documentation should have three entry points:
`README.md` for status and build entry; `CONTRIBUTING.md` for prerequisites,
checks and fixture isolation; `docs/ARCHITECTURE.md` for crate direction, ownership,
startup/shutdown, byte-to-frame flow and persistence/conflict behavior. Document
library contracts in rustdoc next to the API. Avoid duplicating full upstream
installation instructions when a pinned recipe plus project-specific steps is
clearer.

**Observed:** The ADR template in `adr-tools` uses number/title, date, status,
context, decision and consequences. [Template at `b3279ba`](https://github.com/npryce/adr-tools/blob/b3279baf9be2207d1a4f4bbd608fd0b591c72aee/src/template.md)
**Inferred:** Under `docs/decisions/`, use one numbered Markdown record per actual
decision, adding alternatives, dependency licences/pins, evidence and acceptance
measurements. Keep status explicit: proposed, accepted or superseded. Link this
research rather than presenting its recommendations as decisions already made.

## What would settle the remaining risks

The following are **Inferred** follow-up experiments, scoped to the foundation:

1. **Build reproducibility:** build the chosen UI/binding/native-source set on
   macOS arm64 and Linux x64 from a clean checkout. Record Rust, Zig, C compiler,
   SDK, package features and all fetched revisions. Establish the supported Rust
   version from those results.
2. **Worker architecture:** replay identical recorded traffic through a
   blocking-thread reference and the proposed runtime/terminal-worker boundary.
   Measure allocations, byte-queue high-water marks, idle thread count/RSS,
   input responsiveness during output, display-copy cost and clean shutdown at
   increasing leaf counts. Prefer the simpler model if both satisfy the gate.
3. **Persistence parity:** generate synthetic databases/configuration files
   using the pinned Python behavior. Check Rust reads, invalid-input rejection,
   refresh races and merge outcomes. Then run concurrent Python/Rust writers on
   disposable copies before enabling writes to the shared library.
4. **Ownership proof:** compile-check the binding's actual thread traits and
   exercise terminal creation, resize, snapshot extraction and destruction on
   its owner thread. Bound the lifetime and size of data sent to the UI.
5. **Dependency audit:** resolve the concrete workspace and review cargo-deny
   output plus native-source/asset licences. Source-level suitability is not a
   substitute for auditing the resulting graph.

None of these experiments requires a production daemon, live user sessions or
UI feature development before the milestone-0 gate.
