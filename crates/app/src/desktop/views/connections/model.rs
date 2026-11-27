//! The pane's preview: sample records in Agent Backbone's own shapes, so the
//! preview is drawn by the live pane's code. Its actions change only these
//! records, in this pane, and are gone when the preview closes.
use agent_interface_roster::profiles::peers as api;
use api::{Direction, EventKind, Policy};
use std::collections::HashMap;

/// This profile's ID in the preview.
pub(super) const LOCAL_ID: &str =
    "v1:49d0fa9e42cbf1bcb24f4a7cb29dcac9181d9a2f8f26ce13ddafd2f5e5efd177";

/// A sample service: an overview, every message, and each connection's
/// events by generation.
pub(super) struct Sample {
    pub overview: api::Overview,
    messages: Vec<api::Message>,
    events: Vec<(String, api::Event)>,
    /// The next ID a request, connection or event made in the preview takes, so
    /// none repeats after another is removed.
    next: u32,
}

fn id(hex: &str) -> String {
    format!("v1:{hex}")
}

/// A time `minutes` before now, as the service writes one.
fn ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339()
}

const HARBOR: &str = "f3463bf7a7b9bad26eca3b4ffa91966efba07382d3dbae245b51b19f8ab9b64d";
const OPS: &str = "8e21c4f0a9d35b7e6c1048f2d9a3b5c7e0f1a2b3c4d5e6f708192a3b4c5d6e7f";
const NEW: &str = "0444549921eeca2ff49d54a9b42bddab623b7f02833bc120da5d22f284100520";
const ASKED: &str = "7c3e9a1b5d2f4e6a8c0b1d3f5e7a9c2b4d6f8e0a1c3b5d7f9e2a4c6b8d0f1e3a";
const STAGING: &str = "51d0e2f3a4b5c6d7e8f90a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8091a2b3";
const OLD: &str = "a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1";

const LONG_REPLY: &str = "### Relay check from this side
Three things I checked after your message arrived:
1. The envelope opened with the key from the accepted request.
2. The receipt went back through the relay straight away.
3. Nothing from the message was written to disk in plain text.

One thing to confirm. Your message said:
> the connection is set up

but the relay address here is `http://127.0.0.1:7400`, which is a local relay. Is that intended for this test, or should both profiles point at a shared one?";

const HELD: &str = "Intended, yes. Both profiles run on one machine today, so the relay is local and nothing leaves it.
### What I test next
1. Send a message while your profile is offline and confirm it queues.
2. Switch the outgoing gate to Auto and confirm the next message skips review.
3. Disconnect from this side, then confirm a new message is refused.
4. Reconnect with a fresh request and confirm the old thread stays readable.

For each step I send one line saying what I did, and I would like one line back with what you saw. If a step fails, stop there and send the state your side shows, word for word.";

impl Sample {
    pub(super) fn new() -> Self {
        let generation = |peer: &str, n: u8| api::Generation {
            peer_id: id(peer),
            generation: format!("{n:032x}"),
        };
        let peer = |peer: &str, n, name: Option<&str>, state: &str, incoming, outgoing| api::Peer {
            connection: generation(peer, n),
            display_name: name.map(str::to_owned),
            state: state.into(),
            gates: api::Gates { incoming, outgoing },
            revocation_failure: None,
        };
        let invitation =
            |n: u8, peer: &str, name: Option<&str>, direction, state: &str| api::Invitation {
                request_id: format!("{:032x}", 0x100 + u32::from(n)),
                peer_id: id(peer),
                display_name: name.map(str::to_owned),
                direction,
                expires_at: ago(-7 * 24 * 60),
                state: state.into(),
                generation: None,
            };
        use Direction::{Incoming, Outgoing};
        let mut serial = 0u32;
        let mut message = |peer: &str, n, direction, minutes: i64, text: &str, state: &str| {
            serial += 1;
            let decided = matches!(state, "passed" | "declined" | "revision_requested");
            // An incoming message names its agent once it is handed over.
            let handed = direction == Outgoing || state == "passed";
            api::Message {
                connection: generation(peer, n),
                message_id: format!("{serial:032x}"),
                direction,
                text: text.into(),
                submitted_at: ago(minutes),
                expires_at: ago(minutes - 7 * 24 * 60),
                state: state.into(),
                receipt: api::Receipt::Missing,
                upload_failure: None,
                revision: (direction == Outgoing).then_some(1),
                local_agent: handed.then(|| "main".into()),
                decided_at: decided.then(|| ago(minutes - 1)),
            }
        };
        let mut delivered = message(
            HARBOR,
            1,
            Outgoing,
            40,
            "Hello from this profile. The connection is set up; reply when this reaches you.",
            "delivered",
        );
        delivered.receipt = api::Receipt::Confirmed {
            outcome: "delivered".into(),
            recorded_at: ago(37),
        };
        let mut failed = message(
            OPS,
            2,
            Outgoing,
            25,
            "Please share the new runner image digest when it is ready.",
            "upload_failed",
        );
        failed.upload_failure = Some(api::UploadFailure {
            code: "mailbox_full".into(),
            retry_at: Some(ago(-5)),
        });
        let mut declined = message(
            OPS,
            2,
            Incoming,
            60 * 26,
            "Can you restart the staging runner now?",
            "declined",
        );
        // The person's decline is its receipt, at the same time.
        declined.receipt = api::Receipt::Confirmed {
            outcome: "declined".into(),
            recorded_at: declined.decided_at.clone().unwrap_or_default(),
        };
        let mut messages = vec![
            declined,
            delivered,
            message(
                HARBOR,
                1,
                Incoming,
                36,
                "It reached me. This reply goes back over the same connection.",
                "passed",
            ),
            message(HARBOR, 1, Incoming, 28, LONG_REPLY, "passed"),
            message(
                HARBOR,
                1,
                Outgoing,
                26,
                "Yes, it is local for now.",
                "revision_requested",
            ),
            message(HARBOR, 1, Outgoing, 23, HELD, "pending_review"),
            message(
                OPS,
                2,
                Outgoing,
                31,
                "The benchmark finished: the 95th percentile was 38 ms with 20 workers.",
                "uploaded",
            ),
            failed,
            message(
                OPS,
                2,
                Outgoing,
                20,
                "Once the release is tagged, I send the checksum here.",
                "queued",
            ),
            message(
                OPS,
                2,
                Incoming,
                12,
                "Once the release is tagged, send the checksum here so we can verify the staging canary.",
                "pending_review",
            ),
            // A later message after the waiting one: a thread still opens on
            // what waits.
            message(
                OPS,
                2,
                Outgoing,
                5,
                "The staging canary is ready for the checksum.",
                "uploaded",
            ),
        ];
        // The waiting draft revises the one sent back before it.
        messages[5].revision = Some(2);
        let event = |id, minutes, kind| api::Event {
            id,
            at: ago(minutes),
            kind,
        };
        let events = vec![
            (
                generation(HARBOR, 1).generation,
                event(1, 45, EventKind::Connected),
            ),
            (
                generation(HARBOR, 1).generation,
                event(
                    2,
                    25,
                    EventKind::RevisionRequested {
                        message_id: messages[4].message_id.clone(),
                        note: "Answer the relay question first.".into(),
                    },
                ),
            ),
            (
                generation(OPS, 2).generation,
                event(3, 60 * 27, EventKind::Connected),
            ),
            (
                generation(OPS, 2).generation,
                event(
                    4,
                    60 * 26 + 30,
                    EventKind::GateChanged {
                        direction: Incoming,
                        from: Policy::Review,
                        to: Policy::Auto,
                    },
                ),
            ),
            (
                generation(OLD, 3).generation,
                event(5, 60 * 48, EventKind::Connected),
            ),
            (
                generation(OLD, 3).generation,
                event(6, 60 * 2, EventKind::DisconnectRequested),
            ),
        ];
        let mut sample = Self {
            overview: api::Overview {
                local_id: LOCAL_ID.into(),
                invitations: vec![
                    invitation(1, NEW, None, Incoming, "pending"),
                    invitation(2, ASKED, None, Outgoing, "pending"),
                    invitation(3, STAGING, Some("Staging"), Outgoing, "declined"),
                ],
                connections: vec![
                    peer(
                        HARBOR,
                        1,
                        Some("Harbor"),
                        "connected",
                        Policy::Review,
                        Policy::Review,
                    ),
                    peer(
                        OPS,
                        2,
                        Some("Ops relay"),
                        "connected",
                        Policy::Auto,
                        Policy::Review,
                    ),
                    peer(
                        OLD,
                        3,
                        None,
                        "revocation_pending",
                        Policy::Review,
                        Policy::Review,
                    ),
                ],
                pending_reviews: 0,
            },
            messages,
            events,
            next: 0x400,
        };
        sample.count();
        sample
    }

    fn count(&mut self) {
        self.overview.pending_reviews = self.counts().values().sum();
    }

    /// How many messages wait for review on each connected generation.
    pub(super) fn counts(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for m in &self.messages {
            let connected = self
                .overview
                .connections
                .iter()
                .any(|p| p.connection == m.connection && p.state == "connected");
            if connected && m.state == "pending_review" {
                *counts.entry(m.connection.generation.clone()).or_default() += 1;
            }
        }
        counts
    }

    /// One generation's whole history, oldest first.
    pub(super) fn history(&self, generation: &api::Generation) -> api::History {
        let mut items: Vec<_> = self
            .messages
            .iter()
            .filter(|m| &m.connection == generation)
            .cloned()
            .collect();
        items.sort_by(|a, b| a.submitted_at.cmp(&b.submitted_at));
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|(g, _)| g == &generation.generation)
            .map(|(_, e)| e.clone())
            .collect();
        events.sort_by(|a, b| a.at.cmp(&b.at));
        api::History {
            items,
            older: None,
            events,
        }
    }

    fn record(&mut self, generation: &str, kind: EventKind) {
        self.next += 1;
        let event = api::Event {
            id: self.next.into(),
            at: ago(0),
            kind,
        };
        self.events.push((generation.to_owned(), event));
    }

    /// Pass, decline or send back a waiting message, as the service would.
    pub(super) fn review(&mut self, message: &str, decision: Decision) {
        let Some(m) = self
            .messages
            .iter_mut()
            .find(|m| m.message_id == message && m.state == "pending_review")
        else {
            return;
        };
        m.state = match (&decision, m.direction) {
            (Decision::Pass, Direction::Incoming) => "passed",
            (Decision::Pass, Direction::Outgoing) => "uploaded",
            (Decision::Decline, _) => "declined",
            (Decision::Revise(_), Direction::Outgoing) => "revision_requested",
            (Decision::Revise(_), Direction::Incoming) => return,
        }
        .into();
        let now = ago(0);
        m.decided_at = Some(now.clone());
        // Only a message passed through is handed to an agent.
        if m.direction == Direction::Incoming && m.state == "passed" {
            m.local_agent = Some("main".into());
        }
        if m.state == "declined" && m.direction == Direction::Incoming {
            m.receipt = api::Receipt::Confirmed {
                outcome: "declined".into(),
                recorded_at: now,
            };
        }
        let generation = m.connection.generation.clone();
        if let Decision::Revise(note) = decision {
            self.record(
                &generation,
                EventKind::RevisionRequested {
                    message_id: message.into(),
                    note,
                },
            );
        }
        self.count();
    }

    pub(super) fn set_gate(&mut self, generation: &str, direction: Direction, policy: Policy) {
        if let Some(p) = self
            .overview
            .connections
            .iter_mut()
            .find(|p| p.connection.generation == generation)
        {
            let gate = match direction {
                Direction::Incoming => &mut p.gates.incoming,
                Direction::Outgoing => &mut p.gates.outgoing,
            };
            let from = std::mem::replace(gate, policy);
            if from != policy {
                self.record(
                    generation,
                    EventKind::GateChanged {
                        direction,
                        from,
                        to: policy,
                    },
                );
            }
        }
    }

    pub(super) fn disconnect(&mut self, generation: &str) {
        if let Some(p) = self
            .overview
            .connections
            .iter_mut()
            .find(|p| p.connection.generation == generation)
        {
            p.state = "revocation_pending".into();
            self.record(generation, EventKind::DisconnectRequested);
        }
        self.count();
    }

    /// Accept or decline an incoming request: accepting makes a connection
    /// with both directions on Review, declining keeps nothing.
    pub(super) fn answer(&mut self, request: &str, accept: bool) {
        let Some(at) = self
            .overview
            .invitations
            .iter()
            .position(|r| r.request_id == request && r.state == "pending")
        else {
            return;
        };
        if !accept {
            self.overview.invitations.remove(at);
            return;
        }
        self.next += 1;
        let generation = format!("{:032x}", self.next);
        self.record(&generation, EventKind::Connected);
        let r = &mut self.overview.invitations[at];
        r.state = "accepted".into();
        r.generation = Some(generation.clone());
        let connection = api::Generation {
            peer_id: r.peer_id.clone(),
            generation,
        };
        let display_name = r.display_name.clone();
        self.overview.connections.push(api::Peer {
            connection,
            display_name,
            state: "connected".into(),
            gates: api::Gates {
                incoming: Policy::Review,
                outgoing: Policy::Review,
            },
            revocation_failure: None,
        });
    }

    /// Send a request: it waits as `pending` until the other profile answers.
    pub(super) fn invite(&mut self, peer: &str) {
        self.next += 1;
        self.overview.invitations.push(api::Invitation {
            request_id: format!("{:032x}", self.next),
            peer_id: peer.into(),
            display_name: None,
            direction: Direction::Outgoing,
            expires_at: ago(-7 * 24 * 60),
            state: "pending".into(),
            generation: None,
        });
    }
}

#[derive(Clone, Debug)]
pub(super) enum Decision {
    Pass,
    Decline,
    /// Send an outgoing draft back with this note.
    Revise(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(sample: &Sample, direction: Direction) -> String {
        sample
            .messages
            .iter()
            .find(|m| m.state == "pending_review" && m.direction == direction)
            .map(|m| m.message_id.clone())
            .expect("a waiting message")
    }

    #[test]
    fn the_preview_decides_as_the_service_would() {
        let mut sample = Sample::new();
        assert_eq!(sample.overview.pending_reviews, 2);
        let incoming = waiting(&sample, Direction::Incoming);
        // Only an outgoing message can be sent back.
        sample.review(&incoming, Decision::Revise("note".into()));
        assert_eq!(sample.overview.pending_reviews, 2);
        sample.review(&incoming, Decision::Pass);
        assert_eq!(sample.overview.pending_reviews, 1);
        let outgoing = waiting(&sample, Direction::Outgoing);
        sample.review(&outgoing, Decision::Revise("Name the relay.".into()));
        let m = sample.messages.iter().find(|m| m.message_id == outgoing);
        assert_eq!(m.unwrap().state, "revision_requested");
        assert!(m.unwrap().decided_at.is_some());
        assert_eq!(sample.overview.pending_reviews, 0);
        // Sending a draft back leaves its note in the thread, as the service does.
        let history = sample.history(&m.unwrap().connection);
        assert!(history.events.iter().any(|e| e.kind
            == EventKind::RevisionRequested {
                message_id: outgoing.clone(),
                note: "Name the relay.".into()
            }));
    }

    #[test]
    fn a_sample_thread_has_messages_after_the_one_waiting() {
        let sample = Sample::new();
        let ops = &sample.overview.connections[1].connection;
        let items = sample.history(ops).items;
        let waiting = items.iter().position(|m| m.state == "pending_review");
        assert!(waiting.is_some_and(|at| at + 1 < items.len()));
    }

    #[test]
    fn an_accepted_request_becomes_a_connection_on_review() {
        let mut sample = Sample::new();
        let request = sample.overview.invitations[0].request_id.clone();
        let before = sample.overview.connections.len();
        sample.answer(&request, true);
        let peer = sample.overview.connections.last().unwrap();
        assert_eq!(sample.overview.connections.len(), before + 1);
        assert_eq!(
            (peer.gates.incoming, peer.gates.outgoing),
            (Policy::Review, Policy::Review)
        );
        sample.invite(&format!("v1:{}", "c".repeat(64)));
        let sent = sample.overview.invitations.last().unwrap();
        assert_eq!(
            (sent.direction, sent.state.as_str()),
            (Direction::Outgoing, "pending")
        );
        // Declining keeps nothing, and later IDs never repeat earlier ones.
        let incoming = sample.overview.invitations.len();
        sample.invite(&format!("v1:{}", "d".repeat(64)));
        let first = sample.overview.invitations[incoming].request_id.clone();
        sample.answer(&first, false);
        sample.invite(&format!("v1:{}", "e".repeat(64)));
        let ids: std::collections::HashSet<_> = sample
            .overview
            .invitations
            .iter()
            .map(|r| &r.request_id)
            .collect();
        assert_eq!(ids.len(), sample.overview.invitations.len());
    }
}
