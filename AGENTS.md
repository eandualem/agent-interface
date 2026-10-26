# Agent Interface — project instructions

Applies to every agent and all work in this repository.

Agent Interface reimplements
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces) in Rust as a
native desktop app. The binary and packages are named `agent-interface`.
Legacy names remain only for upgrade compatibility and upstream attribution.

## Architecture and documentation

Keep `docs/ARCHITECTURE.md` accurate for contributors: explain the current
crate boundaries, process and data ownership, and relevant technical tradeoffs.
Public documentation helps readers install, use, understand and contribute to
the app. Development research, acceptance worklists and historical run receipts
belong in the local memory directory described below.

## Product contract

- This app replaces the Python tool entirely; nothing may depend on it.
  Its layout and proportions were the reference for this app's window. The
  navigation panel, menus and settings are native UI with real icons;
  the terminals keep their terminal rendering.
- Terminal panes are drawn natively, never in a web view. Targets: bulk
  output equal to Ghostty running tmux; typing latency and frame pacing
  equal to plain Ghostty. Speed claims are measured, with the method and
  numbers recorded.
- tmux is the only process layer. The app is a client and runs no daemon of
  its own.
- macOS first, Linux from the same code. Windows is optional.
- Existing libraries carry over: read and write `layouts.db` (schema v2)
  and its shell server with the established conflict rules. New fields need
  not stay readable by the Python tool.
- The app works with Agent Backbone directly for the agents roster, and the
  core stays useful without it.

## tmux isolation

Agents and the maintainer's own sessions run on this machine's default tmux
server.

- Every tmux command in tests, benchmarks and spikes targets a private
  server: `-L` or `-S`, plus a private `TMUX_TMPDIR` that already exists,
  with `TMUX` unset. Unsetting `TMUX` alone falls back to the default server.
- Never run `kill-server` against, or change options on, a server you did
  not create.
- Tests never attach to sessions they did not create: an attached client,
  control mode included, takes part in window sizing. Use disposable fixture
  sessions on private servers. A check against a live session is read-only
  (`-f read-only,ignore-size`) and is reported to the maintainer before it
  runs.
- The Python repository belongs to its own agent. Ask that agent for library
  and schema facts; never edit that repository.

## Models and swarms

- Implementation code is written by you, or by swarm members running Claude
  Code with `claude-opus-5-5`. No other model writes implementation code.
- Use Codex `gpt-6-astra` as much as the work needs, in swarms too, for
  research, analysis, benchmarking and verification. Codex `gpt-5.6-sol` may
  serve in a swarm in a non-implementation role. Fable models are not used.
- Work at the depth the problem needs: progress matters more than token
  economy.

## Dependencies and licences

- The project is MIT. GPL code, such as Zed's terminal crates, may be read
  for ideas but never copied.
- Record each dependency's licence and purpose in its pull request and the
  architecture guide. Pin fast-moving dependencies to an exact version or commit.

## Boundaries

- No API keys, spending or publishing (crates.io, Homebrew, release
  binaries) without the maintainer's go-ahead. Swarms run on the existing
  subscriptions.
- Repository visibility and publication are the maintainer's decisions.
- The repository is private for now but written for publication: code,
  docs, issues, PRs, commits and comments read as if public. Never quote
  local notes or absolute local paths in tracked content.

## Branches

`develop` is the default and integration branch. Create topic branches from
`develop` and open pull requests into `develop`. `main` is released state:
it receives a promotion from `develop` only when the maintainer asks, as a
merge commit. Use conventional commits (`feat:`, `fix:`, `docs:`, `chore:`)
with a body saying why. Once the workspace exists, keep `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings` and `cargo test` passing before
each commit.

## Local continuity

At startup read `.backbone/memory/SCRATCHPAD.md` and the files it links. In
a linked worktree, resolve `.backbone/memory/` from the parent of the common
Git directory. Memory is never tracked; the shared project-context policy
governs it.
