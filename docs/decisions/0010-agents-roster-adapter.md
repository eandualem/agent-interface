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

This record does not choose an HTTP client library. Whichever is used must let
the adapter turn off redirects and proxy settings, and its licence is recorded
when it is added ([0011](0011-toolchain-and-dependency-policy.md)).

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
