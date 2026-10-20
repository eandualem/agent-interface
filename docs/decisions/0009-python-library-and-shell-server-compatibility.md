# 0009. Compatibility with the Python tool's library and shell server

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The product contract says the app reads the Python tool's `layouts.db` (schema
version 2) and uses its shell server, and writes that store only after it
implements the same conflict rules. Users should be able to open one library
in both tools.

The contract details below were read from the Python tool at commit
[`51de675`](https://github.com/eandualem/tmux-workspaces/tree/51de6757cd0aeda3c19fe68fa4ec5b93ab206804)
and are summarised in the
[Rust architecture research](../research/rust-architecture.md#sqlite-and-python-compatibility)
and the [control-mode research](../research/tmux-control-mode.md#existing-shell-server-compatibility):

- `layouts.db` is a SQLite database whose table `terminal_layout` holds the
  whole arrangement as one JSON document in row `id = 1`. The document's
  `version` field is 2; that is not SQLite's `user_version`. Older tables
  `shared_layout` and `layout` are kept.
- The Python tool saves under `BEGIN IMMEDIATE` with a three-way merge by stable
  IDs, and detects other writers through `PRAGMA data_version`. It uses SQLite's
  default rollback journal with a five-second busy timeout, so a reader that
  holds a read transaction can make its saves fail.
- Ordinary shells run on a per-library tmux server whose socket path is derived
  from the library path, as sessions named `terminal-<leaf id>`, created under a
  file lock with a fixed environment.

Parity rows: [PE-1 to PE-25](../PARITY.md#persistence-concurrent-windows-and-conflict-handling)
and [SH-1 to SH-17](../PARITY.md#shell-environment-and-process-lifetime).

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Read the shared library now; write only after passing the Python tool's conflict tests** | One library works in both tools from the start, and nothing the Python tool saved can be damaged. Editing stays limited until the merge rules are ported and proven. |
| Read and write immediately with a simple "last writer wins" save | Faster to build, but loses concurrent edits and breaks the Python tool's merge contract. Rejected. |
| A new private store for the Rust app | Free of the Python schema, but users would have two libraries. Only by an explicit product decision, never as a fallback when compatibility is hard. |

For SQLite access:

| Library | Version examined | Licence | Trade-offs |
| --- | --- | --- | --- |
| **`rusqlite`** with the `bundled` feature | 0.40.2, using `libsqlite3-sys` 0.38.2 (MIT) with SQLite 3.53.2 (public domain) | MIT | Synchronous, small, supports read-only open flags and immediate transactions. Bundling compiles SQLite with a C compiler. |
| SQLx | 0.9.0 | MIT OR Apache-2.0 | Async pool, but it also runs SQLite on a background thread; a pool adds nothing for one JSON row. |
| SQLite's C API directly | — | Public domain | Hand-written `unsafe` bindings for no gain. |

## Decision

**Finding the library.** Choose the library directory in the Python tool's
order: `--data-dir`, then `TMUX_WORKSPACES_DATA_DIR`, then
`$XDG_DATA_HOME/tmux-workspaces`, then `~/.local/share/tmux-workspaces`, on
macOS too (PE-1). `store::library_directory` implements this selection on Unix,
ignores empty environment overrides, expands `~` and `~user`, and resolves
relative paths and symlinks. Like Python's non-strict `Path.resolve`, a missing
intermediate component may be removed by a later `..`; symlinks are expanded
before processing their following parent components. The read-only phase
requires the final resolved directory
to exist; it never creates a missing library. `app::library` performs this lookup
on its storage thread before opening SQLite. Product command-line parsing and
window integration remain pending.

Named-user home expansion uses [`uzers` 0.12.2](https://docs.rs/uzers/0.12.2/uzers/),
MIT, pinned exactly with default caching, mock and logging features disabled.
It adds only the already used MIT/Apache-2.0 `libc` dependency. Its safe account
lookup accepts native OS strings, so paths need not be UTF-8. Standard-library
home lookup alone cannot expand `~user`; direct libc calls would add local
unsafe code. A broad shell-expansion library is unnecessary: Python does not
expand environment variables inside these paths.

**Reading `layouts.db`.**

- Open with `SQLITE_OPEN_READONLY` (`mode=ro`), never creating or migrating
  anything. Keep one connection open on the storage worker
  ([0007](0007-threads-ownership-and-event-ordering.md)) and poll
  `PRAGMA data_version` on it.
- Finish or reset every statement before returning to the poll loop; never hold
  a read transaction between polls. Use a bounded busy timeout for the brief
  locks of a commit.
- If SQLite reports `SQLITE_READONLY_ROLLBACK` (a journal left by a crashed
  writer), retry for a bounded time, then show that recovery is needed and keep
  the current view. Never reopen read-write, change the journal mode,
  checkpoint, `VACUUM`, repair, or open with `immutable=1`.
- A missing `terminal_layout` table means the library has not been initialised
  for this reader; the app says so and suggests opening it once with the Python
  tool. It never imports the older tables itself (this settles
  [PE-4](../PARITY.md#persistence-concurrent-windows-and-conflict-handling) for
  the read-only phase). A missing row, an invalid schema or an invalid document
  is an error that names `layouts.db` and the problem (SU-13).
- Check the 16 MiB size limit before parsing, and reject duplicate JSON keys
  before building any map, because an ordinary JSON map keeps the last
  duplicate silently.
- Apply the Python validation rules: identities, split ratios and nesting
  limits, absolute NUL-free paths, the presence of the `agent` key marking a
  leaf, and the other rules listed under PE-8 to PE-12.
- Keep unknown fields, and keep the difference between an absent field and an
  explicit `null` wherever the merge rules depend on it. The stored schema-v2
  types are separate from the in-memory model types in `core`.
- Treat the stored `selected` and `focus` values as hints that seed a new
  window, not as shared state.

**Writing `layouts.db`.** Disabled until all of these exist:

1. The merge and conflict rules are ported to `core`, and Rust passes
   differential tests generated from the pinned Python behaviour: rename,
   delete, move, reorder, concurrent split edits, unknown fields, local
   navigation and failures.
2. Mixed-process tests with Python and Rust writers on disposable copies pass.
3. The legacy-table migration behaviour and the write contract are agreed with
   the Python tool's maintainers.

What the app does with edits before then is still open
([PE-21](../PARITY.md#persistence-concurrent-windows-and-conflict-handling)).

**Using the shell server.**

- Derive the socket path exactly as the Python tool does:
  `/tmp/tmux-workspaces-<uid>/<digest>-terminals.sock`, where `<digest>` is the
  first 16 hexadecimal characters of the SHA-256 of the UTF-8 text of the
  resolved library path. Refuse a socket directory that is a symlink, is not
  owned by the user, or grants any group or other permission.
- Attach to existing `terminal-<leaf id>` sessions. A missing session is
  normal and is not an error.
- Never kill a `terminal-*` session except when the user removes its pane or
  tab, which needs write support. Never stop the shell server.
- Creating missing shell sessions is a separate gate from database writes. It
  stays disabled until the app reproduces the Python creation policy and lock:
  an exclusive `flock` on `<socket>.viewer-lock` around listing and creating,
  `-f /dev/null`, `status off`, `mouse on`, `remain-on-exit on`, and the
  environment described in [0016](0016-shell-environment-and-launch-context.md).
  Lock interoperability with a running Python window is tested first.

**Theme and keymap files.** Read `theme.toml` and `keymap.toml` from the same
XDG paths for parity, with `toml` (0.9.8, MIT OR Apache-2.0). Do not write them
until a write contract matching the Python tool's conflict checks is agreed.

**Leave the Python tool's runtime alone.** Never create, change or delete the
Python tool's `windows/<instance>/` runtime directories, viewer sockets or
action sockets (PE-24).

JSON is handled with `serde` (1.0.229) and `serde_json` (1.0.154), both MIT OR
Apache-2.0, with duplicate-key detection added as described above. `core` also uses
`serde_json` for shared-document merging: merging only the typed display model
would discard unknown extension fields and absent/null distinctions.

## Consequences

- The Python tool's saves keep working while the app is open, because the app
  never holds a lock between polls.
- Opening `layouts.db` read-only does not make a tmux attachment read-only:
  resizing and typing still affect tmux. Those effects are governed by
  [0003](0003-control-mode-connection-and-attachment-model.md) and
  [0015](0015-input-routing.md).
- Tests need synthetic schema-v2 databases and fixtures generated from the
  pinned Python commit ([0012](0012-testing-strategy-and-tmux-isolation.md)).

## Revisit when

The Python tool changes a documented compatibility boundary (data-directory
order, socket identity, session names, the version-2 record, saved attachment
references or configuration paths), or the maintainer decides on a separate
store.

## Evidence

- [SQLite and Python compatibility](../research/rust-architecture.md#sqlite-and-python-compatibility)
  and [configuration, errors and observability](../research/rust-architecture.md#configuration-errors-and-observability)
  in the Rust architecture research.
- [Existing shell-server compatibility](../research/tmux-control-mode.md#existing-shell-server-compatibility)
  in the control-mode research.
- [Parity checklist](../PARITY.md#persistence-concurrent-windows-and-conflict-handling),
  rows PE-1 to PE-25.

## Read-only implementation

`Store::open` takes an already resolved library directory. `load` validates the
current row and returns a `Document` with the typed `core::Layout` and the exact
original JSON. The original record retains unknown fields and absent/null
values; the model is not a replacement serialization format. `data_version`
is read before loading, on the same connection, so a concurrent commit still
invalidates the next poll. Calls finish their statements before returning.
The app's `library` storage worker resolves the selected library path and owns
the retained connection. Controller integration and all writes remain separate
work.

The decoder bounds all containers, including extensions, and rejects duplicate
keys before constructing maps. Borrowed `serde_json::RawValue` slices let it
check depth itself rather than relying on serde_json's lower default recursion
limit; `arbitrary_precision` retains large integer extension values. Split
nodes use the schema's 64-level limit. Invalid input is never normalized into
a different saved arrangement.

Printable names and session references use `unicode-properties` 0.1.4
(MIT OR Apache-2.0), already used by the terminal dependency graph, with only
its general-category feature requested here. Its Unicode 17 categories reject
control, format, unassigned and separator characters, except ordinary space.
Python runtimes with older Unicode data can differ for newly assigned
characters. Rust JSON strings also require Unicode scalar values; unpaired
surrogate escapes are rejected even in unknown fields. These are explicit
compatibility limits, not evidence of complete persistence parity.

SQLite uses a 250 ms busy timeout. A hot journal requiring recovery is retried
read-only for at most 250 ms, then reported for recovery by a writer; the reader
never reopens writable. `app::library` polls every 250 ms on its storage thread
and coalesces complete documents and errors into one pending update. A read
failure preserves any valid document still awaiting consumption; callers also
retain their displayed document. Unchanged errors are reported once, and a
successful subsequent read clears the error. An initial open failure is reported
and ends that worker; it never creates or repairs the library. Dropping the
handle stops polling and joins the thread. Worker tests use the same pinned
`rusqlite` only as a development dependency to simulate another writer on a
disposable library.

`Layout::retain_navigation` applies this window's selections to the incoming
model and reports whether the active tab's tree changed. This includes attachment
changes, splits and deletion; a rename alone does not trigger it. The controller
must use that result to return input focus to navigation and require another pane
selection before sending input. This follows the Python viewer's refresh check
in `sidebar.py` at the pinned reference version. The storage worker itself never
dispatches input or changes focus.

## In-memory conflict handling

`core::merge::merge_shared_layout` accepts three already validated schema-v2
JSON documents and removes window-local navigation before combining shared
edits. Independent fields and stable-identity lists merge; conflicting trees,
incompatible reorders, duplicate tab moves and deleting all workspaces fail.
Unknown fields retain Python's numeric equality and identity-list semantics.
Seventeen reference cases generated from the pinned Python model cover these
rules, including deletion versus editing and numeric precision boundaries.

The result has no navigation fields. A future caller must restore local
navigation, validate the complete merged schema and size, and perform the
required transactional checks before saving. The mixed-writer tests, migration
agreement and write gate above remain outstanding; this function performs no
persistence.

## Saved split geometry

`core::Tab::pane_rectangles` calculates content rectangles without drawing or
changing tmux. It follows the pinned Python `model.minimum_size` and
`display.Display._tree`: a leaf needs 34 columns by 6 rows, sibling panes have
one separator cell, and each ratio is rounded to the nearest cell with ties to
even, then clamped to fit both subtrees. These are terminal content dimensions;
the caller excludes the native sidebar and other window decorations.

If the available content area is too small, or the local focus flag is enabled,
only the focused leaf receives a rectangle. Restoring space or leaving focus
mode reveals the same saved tree and ratios. A zero-sized content area has no
rectangles. This preserves the Python behavior in a pure model operation;
renderer integration and native-window resizing remain separate work. No new
dependency is needed.

`Layout::choose_pane` accepts only a leaf in the current tab. After a resize,
`Node::remember_split_sizes` records the ratio of the measured child content
sizes, excluding the separator, with the Python viewer's 0.15 to 0.85 clamp.
Missing identities and zero-sized children do nothing. These operations only
change the model; terminal readiness, input handoff and resizing remain caller
responsibilities.
