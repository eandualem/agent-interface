# Setup fixtures

Agent Backbone's own protocol fixtures, copied byte for byte, except
`first-agent-result.json`, which is this app's own, and these, which
Backbone's fixtures don't have yet:

| File | Source |
|---|---|
| `machine-runtimes.json` | `GET /api/profile/machines/{machine}/runtimes`, 200: another machine's tools. Backbone's stated contract for this route, not yet released: each item is a `backbone runtimes --json` item plus `sign_in` (`storage`: `present`, `missing` or `unavailable`, and an optional `message`). |
| `machine-runtimes-unreachable.json` | The same route, 503 `machine_unreachable`. |
| `machine-runtimes-not-found.json` | The same route, 404 `not_found` for a machine the profile doesn't have. |
