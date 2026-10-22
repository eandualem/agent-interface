# Attaching without taking resize control

An observational control client must leave its size undeclared. `ignore-size`
alone is insufficient: when every eligible client has that flag, tmux still
uses their sizes. A grouped helper provides independent session window
selection, but shares the source's windows and their geometry.

The coexistence tests for [decision 0003](../decisions/0003-control-mode-connection-and-attachment-model.md)
exercise this with ordinary clients in pseudo-terminals and control clients on
disposable private servers. They do not open desktop windows or contact existing
sessions. Run them with the tmux version to check first on `PATH`:

```sh
cargo test -p tmux-workspaces-bench coexistence:: -- --nocapture --test-threads=1
```

## Geometry and successful detach runs

On macOS, the original geometry and graceful-detach checks passed on
**21 October 2026** with tmux **3.3a, 3.4, 3.5a, 3.6b, 3.7c and 3.8**.
A subsequent full-suite run exposed a tmux 3.7c server crash during helper
cleanup. The passing runs below therefore do **not** establish safe lifecycle
behavior or satisfy decision 0003's acceptance condition. These checks are
separate from the performance gates.

- An ordinary 120×40 client and unsized control clients coexist without changing
  that geometry under `smallest`, `largest`, `latest` and `manual`. An unsized
  control client reconnecting alone also leaves the retained geometry unchanged.
- Selecting window 1 in a grouped helper leaves the source session on window 0.
  The sessions still refer to the same window identities.
- A marked helper with `destroy-unattached=on` disappears after its last client
  detaches. The source, configured with `destroy-unattached=off`, keeps its session
  identity, window identity and pane PID through helper cleanup, ordinary and
  control client detach, and a subsequent control reconnection.
- Declaring a control size makes it participate in shared geometry. The table
  below holds for both direct control attachments and grouped helpers. The
  ordinary client is 120×40, the control client declares 80×24, and the control
  client explicitly selects the window to become the latest client. The manual
  size is set to 110×35.

| Ordinary client ignores size | Control client ignores size | `smallest` | `largest` | `latest` | `manual` |
| --- | --- | --- | --- | --- | --- |
| No | No | 80×24 | 120×40 | 80×24 | 110×35 |
| No | Yes | 120×40 | 120×40 | 120×40 | 110×35 |
| Yes | No | 80×24 | 80×24 | 80×24 | 110×35 |
| Yes | Yes | 80×24 | 120×40 | 80×24 | 110×35 |

With both clients eligible, declaring `refresh-client -C @id:60x20` caps the
shared window at 60×20 under every policy, including `largest` and `manual`.
Removing that declaration does not change the configured `window-size` policy.
It is not an independent viewport size for the control client.

The fixture controls its own source options and client flags to expose these
cases. Product code must not change another session's options or detach its
clients to obtain a desired size. The tests use default hooks and a source that
survives detach; they do not override a source configured to destroy itself.

## Helper cleanup failure

The graceful-detach test failed once during a full suite on tmux 3.7c because
the private server disappeared. A macOS crash report at the same time recorded
`SIGSEGV` in `server_client_get_pane`, called through `cmd_find_from_client`,
`notify_client` and `server_client_lost`. That initial run did not record the
server PID, so the report alone cannot prove its socket identity.

A bounded interrupted-detach test then reproduced the same stack with the
server PID and private socket recorded together. It stops its own helper
control client, asks its own source control client to detach that helper,
and triggers size recalculation. After verifying that the helper is gone and
the source survives, it kills only the stopped helper client. tmux 3.7c then
crashes and the source becomes unavailable. The corresponding graceful-resume
experiment passed. The interrupted case failed on tmux 3.7c and passed on
tmux 3.8 in both the diagnostic and final fixture runs. These bounded results
do not establish a version-wide fix or change the supported version policy.

The two lifecycle tests are opt-in because the expected source-preservation
assertion currently fails on an affected server. Run them explicitly with a
chosen tmux binary first on `PATH`:

```sh
cargo test -p tmux-workspaces-bench coexistence:: -- --ignored --nocapture --test-threads=1
```

These commands create only disposable private servers. The interrupted case
uses `SIGSTOP` and `SIGKILL` on its own helper child; it never signals an existing
server or client. The geometry test remains in the ordinary test suite.
Passing that suite with the lifecycle tests ignored is not lifecycle acceptance.
A cleanup fix or a supported-version decision still needs separate validation.

## Scope and remaining decisions

The tests live in
[`crates/bench/src/gate5/coexistence.rs`](../../crates/bench/src/gate5/coexistence.rs)
and reuse the existing private-server, control-client and pseudo-terminal
fixtures. They record geometry behavior and a lifecycle failure, not product lifecycle integration,
Linux execution, terminal fidelity, display performance, or visual correctness.

Decision 0003's policy for taking resize control of external sessions remains
open. Keep observational attachments unsized and draw the geometry tmux reports
until that policy is settled. These results do not change any milestone gate or
authorize enabling product attachments.
