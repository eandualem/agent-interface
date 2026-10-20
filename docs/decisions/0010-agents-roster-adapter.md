# 0010. Agents roster adapter

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The sidebar can show an agents roster: one row per active agent with its state,
read from a local Backbone service. The roster is optional. The workspaces must
work fully without it, and a roster failure must never affect terminals
([AG-1, AG-17 and AG-25](../PARITY.md#agents-roster)).

The Python tool's adapter reads Backbone's API address from Backbone's settings
database (read-only) and its API key from the environment or Backbone's `.env`
file, then polls
`/api/agents` every five seconds. It accepts only loopback HTTP endpoints,
refuses redirects, ignores proxy settings, and sends the key only after the
endpoint passes those checks.

## Options considered

| Option | Trade-offs |
| --- | --- |
| **A read-only adapter in its own crate, with the Python tool's endpoint and credential rules** | Keeps the core free of any provider, and keeps the credential confined to one validated local endpoint. |
| Build the roster into the model or the user interface | Fewer crates, but a provider outage or format change would reach code that terminals depend on. |
| Derive agent states from terminal text or running processes | No service needed, but guesses states and contradicts AG-11, which takes states only from Backbone. Rejected. |

## Decision

The roster is a read-only adapter in the `roster` crate, which depends on no
other project crate ([0006](0006-workspace-and-crate-layout.md)). `app` maps
roster entries onto tmux sessions and tabs.

- **When it runs.** Follow the Python tool's rules (AG-3, AG-4, AG-7): the
  adapter starts when Backbone's database exists in its data directory, or when
  the user asks for it with `--backbone`, `--backbone-data-dir` or `--url`.
  Otherwise the app reads no Backbone configuration and makes no request.
  `--no-backbone` keeps it off.
- **Address.** Use `--url` when given. Otherwise read the host and port from
  Backbone's settings database, opened read-only, with the `BACKBONE_PORT`
  environment variable taking precedence over the stored port. A missing host
  means `127.0.0.1`, a stored `0.0.0.0` is contacted as `127.0.0.1`, and a
  missing port means 7120, as in the Python tool.
- **Endpoint checks, before any network access.** The scheme is `http`, the
  host is `localhost`, `127.0.0.1` or `::1`, and there is no user information,
  query string or fragment.
- **Requests.** `GET /api/agents` every five seconds. Never follow a redirect,
  even to another local address, and ignore proxy environment variables.
- **Credential.** Take the API key from the `BACKBONE_API_KEY` environment
  variable, or, when that is empty, from the same variable in Backbone's
  `.env`, reading no other value from that file. Send it as a bearer token only
  to the validated endpoint, and send no token when there is no key. Never pass
  it to tmux, a shell or a log message.
- **Read-only.** Never write Backbone's files or call anything other than the
  agents endpoint.
- **Failures.** Keep the last states but mark them stale, with a fixed message
  that contains no credentials, response bodies or error text (AG-16). Polling
  continues, and terminals and tmux discovery are unaffected (AG-17, AG-25). A
  roster state can never make a missing tmux session attachable (AG-18).

The HTTP client is exact-pinned `reqwest` 0.13.5 (MIT OR Apache-2.0), with
only its `blocking` feature and no default features. It provides explicit proxy,
redirect and connection-logging controls. Each request has a three-second total
deadline, including its body, and the reader stops after 1 MiB plus one overflow
byte. The decoder accepts exactly 1 MiB and rejects larger bodies.

`ureq` 3.4.2 was considered for its smaller blocking implementation, but its
protocol dependency traces serialized authorization bytes even when the header
is marked sensitive. Disabling all application trace logging or maintaining a
patched HTTP stack would impose unrelated costs. A handwritten client would
duplicate framing and deadline handling. `reqwest` uses an internal Tokio runtime
behind its blocking API; no runtime type escapes the roster crate. This choice
does not settle the separate child-process runtime comparison in [0008](0008-background-io-runtime.md).
TLS, compression and system-proxy support are disabled for this HTTP-only adapter.

The existing `http` 1.5.0 (MIT OR Apache-2.0) is exact-pinned for strict URI
validation before the client's URL normalization; malformed explicit ports are
rejected. Test-only `log` 0.4.34 (MIT OR Apache-2.0) captures enabled trace output
to verify credentials remain absent. Newly locked HTTP/runtime dependencies are:

| Purpose | Dependencies and versions | Licence |
| --- | --- | --- |
| HTTP bodies, framing and connection support | http-body-util 0.1.5, hyper 1.12.0, hyper-util 0.1.21, want 0.3.2, try-lock 0.2.5 | MIT |
| Request middleware | tower 0.5.3, tower-http 0.6.11, tower-layer 0.3.3, tower-service 0.3.3 | MIT |
| Socket readiness | mio 1.2.4 | MIT |
| Header parsing and address/socket support | httparse 1.10.1, ipnet 2.12.2, socket2 0.6.5 | MIT OR Apache-2.0 |
| Thread-safe request storage | sync_wrapper 1.0.2 | Apache-2.0 |

See the [client controls](https://docs.rs/reqwest/0.13.5/reqwest/blocking/struct.ClientBuilder.html)
and [total request timeout](https://docs.rs/reqwest/0.13.5/reqwest/blocking/struct.RequestBuilder.html#method.timeout).

## Consequences

- The app is fully usable with no Backbone installed, and makes no network
  request in that case.
- Tests use fixture responses and a local fake endpoint, including a check
  that a synthetic key never reaches a redirect target, a proxy or a
  diagnostic message ([0012](0012-testing-strategy-and-tmux-isolation.md)).

## Revisit when

A second roster provider is needed, or Backbone's API changes in a way that
requires more than the agents endpoint.

## Evidence

- [Configuration, errors and observability](../research/rust-architecture.md#configuration-errors-and-observability)
  in the Rust architecture research, which links the Python adapter's endpoint
  validation, redirect refusal and request construction.
- [Parity checklist](../PARITY.md#agents-roster), rows AG-1 to AG-25.

## Response model

The `roster::Snapshot` response model retains only validated agent names and
reported states. Failed decoding or a caller-reported transport failure retains
the last states and observation time with a fixed stale diagnostic. A successful
response replaces the entire list, including an empty list, and clears the error.
Decoding does not load configuration, read credentials or issue requests.

The decoder uses exact-pinned `serde_json` 1.0.154 (MIT OR Apache-2.0), already
used by the workspace, to parse the endpoint's `items` array. It limits responses
to 1 MiB and 128 nested containers; oversized, deeply nested or invalid
JSON becomes unavailable rather than an unbounded allocation. Names follow the
Python adapter's ASCII agent-name rule; disabled records are skipped, non-string
states become `unknown`, and the last valid duplicate name wins. Unknown fields,
including API attachment-availability claims, are discarded.

The app's `sessions::with_roster` merges these values with discovery without
changing either observation. Only successful discovery supplies `online`; a
metadata failure cannot remove live ordinary sessions. Metadata names absent
from discovery stay as offline references. The combined timestamp is the oldest
observation only when all participating providers have one. The separate roster
snapshot remains the input for future roster display. `Snapshot::fetch` reads one
response using an explicit address and key. It validates the address before
constructing a request, disables proxies and redirects, limits the complete call
to three seconds and the body to 1 MiB, and exposes only the fixed stale error.
Credentials are never retained in the snapshot. Configuration loading, polling
and product controls remain separate work.

Typed object/array reads and borrowed raw field values preserve literal JSON
types even when the workspace enables serde_json's raw-value and arbitrary-
precision features. Internal marker-looking keys remain ordinary object keys;
they cannot turn an object into an empty list or a false configured value. An
explicit depth check bounds skipped raw values, including unknown fields.
