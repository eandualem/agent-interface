# Connections between independent profiles

The local profile home owns these records. Its owner reads them on the
profile socket. The identity in `local_id` is shareable. Compare peer
fingerprints out of band before accepting. A peer's display name is
self-reported. All directions are relative to the profile receiving the
API call. Each accepted pair starts with both gates in review.

The following routes specify the client contract for the serving
implementation. The portable protocol fixtures can be consumed independently.

| Method and path | Request | Successful result |
| --- | --- | --- |
| GET /api/peer-connections | — | Overview with identity, outstanding and recently closed invitations, accepted generations, pending review count and the local orchestrator, or null while none is set |
| POST /api/peer-connections/requests | Invite | Invitation with its stable request ID |
| GET /api/peer-connections/requests/{request_id} | — | Invitation and its latest answer, including declined or expired outgoing requests |
| POST /api/peer-connections/requests/{request_id}/answer | Answer | Answered invitation and any established generation |
| GET /api/peer-connections/{generation} | — | Connection with both gate settings |
| PATCH /api/peer-connections/{generation}/gates | SetGate | Connection after applying one direction |
| GET /api/peer-connections/{generation}/history | — | History page |
| GET /api/peer-connections/reviews | — | ReviewPage with local queue arrival times |
| POST /api/peer-connections/{generation}/messages | Submit | Message waiting for review, or passed; only the orchestrator named in `peer_connections.orchestrator` may send |
| POST /api/peer-connections/{generation}/messages/{message_id}/review | ReviewDecision | Updated immutable Message |
| POST /api/peer-connections/{generation}/disconnect | Empty | Connection after local revocation |

The overview includes retained declined and expired requests so a second client can discover their outcome by ID after a reload.

Static route segments take precedence over generation IDs. For the two
paged reads, omit `before` for the newest page. Supply the returned opaque
`older` cursor as `before` for the next page in the same view. Items appear
oldest first within a page. A null cursor marks the oldest page. Sending the
same mutation again uses its original `x-backbone-request-id`. A stale
generation or already decided review cannot authorize another delivery.

| Operation | JSON body |
| --- | --- |
| Invite | `{"peer_id":"<shareable ID>"}` |
| Answer invitation | `{"action":"accept"}` or `{"action":"decline"}` |
| Set one gate | `{"direction":"incoming","policy":"auto"}` |
| Submit a message, as the orchestrator | `{"text":"Is the staging build ready?"}` |
| Submit a revised draft, as the orchestrator | `{"text":"Is version 2 ready?","revises":"<message ID>"}` |
| Review either direction | `{"action":"pass"}` or `{"action":"decline"}` |
| Request revision of outgoing draft | `{"action":"ask_to_revise","note":"Please include the version."}` |

A refused submission stores nothing, and its error's `detail.reason` says
why:

| Reason | Error | When |
| --- | --- | --- |
| `sending_off` | 403 `outside_authority` | No orchestrator is set |
| `not_orchestrator` | 403 `outside_authority` | The caller is not the orchestrator set |
| `empty_message` | 422 `invalid_request` | The text is empty or only whitespace |
| `message_too_large` | 422 `invalid_request` | The text is over 16 KiB of UTF-8 |
| `control_characters` | 422 `invalid_request` | The text holds a control character other than a line break or tab, a bidirectional control or mark, a line or paragraph separator, or a tag character |
| `not_connected` | 409 `conflict` | No connected generation has this ID |
| `rate_limited` | 429 `rate_limited` | The connection took 30 messages in the last minute |
| `mailbox_full` | 409 `conflict` | 256 outgoing messages, or 4 MiB of them, wait for review or for a confirmed upload; messages the relay holds for the peer are not counted |
| `revised_unknown` | 422 `invalid_request` | No message has the ID in `revises` |
| `revised_other_connection` | 422 `invalid_request` | The message in `revises` belongs to another generation |
| `revised_incoming` | 422 `invalid_request` | The message in `revises` is incoming |
| `revision_not_requested` | 409 `conflict` | The draft in `revises` is not `revision_requested` |
| `already_revised` | 409 `conflict` | Another draft already revises the draft in `revises` |

A `rate_limited` refusal carries `retry_after` in `detail`, and the same
value in the `Retry-After` header: the whole seconds, at least 1, until
the oldest of those messages leaves the minute and a slot opens.

A refused review decision changes nothing:

| Reason | Error | When |
| --- | --- | --- |
| — | 404 `not_found` | No message of this generation has this ID |
| — | 409 `conflict` | The message does not wait for this decision: it was decided, its connection is not connected, or its direction does not take it |
| — | 409 `conflict` | A revision is asked for while no registered orchestrator is set in `peer_connections.orchestrator` to take the note, or the one set changed meanwhile |
| `note_too_large` | 422 `invalid_request` | The revision's note is over 16 KiB of UTF-8 |
| `control_characters` | 422 `invalid_request` | The revision's note holds a character a message may not hold, as for a submission |

Passing freezes the reviewed bytes. Asking for revision closes the original
draft. Its replacement gets a new message ID and passes the outgoing gate
again. Messages already awaiting a decision stay in review when the owner
switches to automatic mode.

The orchestrator names the draft it replaces in `revises`: the message ID
the revision's note names. The replacement's `revises` keeps that ID and
its `revision` counts the drafts, 1 for a first draft. Both stay on this
profile: the peer receives only the text of the draft that passed.

A message's `local_agent` is the local orchestrator that submitted an
outgoing message, or the agent an incoming message was handed to, absent
until it was. `decided_at` is when the local owner passed, declined or
asked for a revision; it is absent for an automatic pass and while the
message waits. Every one of these fields is absent on records written
before a home had them.

A history page also carries `events`: the connection's `connected`,
`gate_changed` (with `direction`, `from` and `to`), `revision_requested`
(with `message_id` and `note`), `disconnect_requested` (by the local
owner) and `disconnected` events, each with an `id` and its `at` time, in
the page's span, oldest first. The newest page holds the events at or after
its oldest message; an older page those before the newer page's oldest
message and at or after its own; the oldest page also every earlier one.
A generation without messages has all its events on its one page. Each
event is on exactly one page. An event a client does not know reads as
`unknown`. `events` is absent when a page has none.

Events within a page are oldest first, as items are. A client places an
event among the messages by comparing its `at` with their `submitted_at`:
an event whose `at` equals a message's `submitted_at` goes after that
message, as the span rule does. An event's `at` and an outgoing message's
`submitted_at` are this home's clock; an incoming message's `submitted_at`
is the sender's, within the clock-skew bound, so an incoming message and
an event close together may order either way by up to that skew.

When a peer has not supplied a name, use its fingerprint as the label. A client can render the pending badge from the overview without opening a
review. Open the stream and receive `hello` before fetching the initial snapshot. Keep the cursor from the hello frame until a later event supplies a newer cursor. On reconnect, resume from the most recent cursor. A reconnect without a cursor requires a fresh snapshot after hello. If a change arrives during that fetch, refresh again after it returns. On `peer.connections.changed` or stream resync, refresh the open
views and the badge. An incoming message's expiry recorded while the
orchestrator's queue drains or its inbox is read is announced by the next
dispatch pass, up to 15 seconds after it is recorded. The shared stream carries an empty object. Text, IDs
and decisions are read through the owner-authorized endpoints.

- [Overview](peer-overview.json) includes a peer, an incoming request and the badge.
- [Accepted invitation](peer-invitation-accepted.json) includes the new generation.
- [History with events](peer-history-events.json) shows a whole generation on one page: connected, gate changes (one at the same millisecond as a message, so after it), a revised draft and its revision, an incoming message handed to the orchestrator, and the owner's disconnect.
- [History](peer-history.json) illustrates missing, uncertain and confirmed receipts. A relay upload or remote storage acknowledgment alone leaves the terminal receipt missing. Uncertain delivery must be reconciled before another paste. Confirmed delivery and confirmed owner decline have distinct outcomes.
- [Local disconnect](peer-disconnect.json) shows local revocation while remote confirmation is pending. The state changes to `disconnected` after the peer reconciles outstanding dispatch and acknowledges revocation. The disconnect body may be empty or `{}`; any other body is 422 `invalid_request`. An unknown generation is 404 `not_found`. A repeat on a `revocation_pending` or `disconnected` generation returns the connection as it stands.
- [Refused revocation](peer-disconnect-refused.json) shows a local revocation the relay refused for good, other than by expiry, so the peer may not learn of it. `revocation_failure` carries the relay's code. It is present only while the state is `revocation_pending` and absent otherwise, so it disappears once the connection leaves that state.
- [Change event](stream-peer-connections-changed.json) uses the profile stream's cursor.


[Older history](peer-history-older.json) ends the same generation's history.
A possible dispatch still has an unknown outcome after its expiry or local
revocation. Only reconciliation can resolve it. Expired and revoked message
states require evidence that dispatch was prevented. A [later history
record](peer-revoked-message.json) shows the outgoing draft from the newest
page after local disconnect prevented its dispatch. [Pending reviews](peer-reviews.json)
use local queue arrival time even when transmission was delayed. The
[action exchanges](peer-actions.json) pair requests with their results and
include the requester view after decline and the peer-confirmed disconnect.

Both homes retain the accepted generation in the invitation. When an upload attempt has no response, the sender reports `upload_unconfirmed`. The relay may have stored the envelope. The sender reconciles or retries the identical envelope using its original ID and deadline. A later explicit refusal of a retry alone cannot rule out acceptance of an earlier attempt. An explicit upload refusal records its service code and retry time. A retry keeps the original message ID and expiry. Missing terminal evidence stays missing even if that deadline passes after a possible dispatch.

An offline peer home can leave a message at `uploaded`. Remote storage
advances it to `received`, and waiting for an available terminal is
`queued`. Each still has a missing terminal receipt. The outgoing pass and
decline examples in the action exchanges show the local owner's decision
on the same draft. Neither supplies a peer receipt.
