//! Presentation state for one peer thread. The design preview owns its sample data.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Receipt {
    Waiting,
    Delivered,
    Notified,
    Passed,
    Declined,
    Revision,
    Queued,
}

pub(super) struct Message {
    pub direction: Direction,
    pub time: String,
    pub body: String,
    pub receipt: Receipt,
    pub revision_note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Link {
    Connected,
    IncomingRequest,
    Pending,
    Offline,
    StoppedHere,
    Refused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Gates {
    pub incoming_auto: bool,
    pub outgoing_auto: bool,
}

impl Gates {
    pub fn labels(self) -> (&'static str, &'static str) {
        let label = |auto| if auto { "Auto" } else { "Review" };
        (label(self.incoming_auto), label(self.outgoing_auto))
    }
}

pub(super) struct Peer {
    pub name: String,
    pub id: String,
    pub link: Link,
    pub gates: Gates,
    pub messages: Vec<Message>,
}

impl Peer {
    pub fn can_review(&self) -> bool {
        matches!(self.link, Link::Connected | Link::Offline)
    }

    pub fn waiting(&self) -> usize {
        if self.can_review() {
            self.messages
                .iter()
                .filter(|m| m.receipt == Receipt::Waiting)
                .count()
        } else {
            0
        }
    }

    pub fn needs_you(&self) -> bool {
        self.waiting() > 0 || matches!(self.link, Link::IncomingRequest | Link::Refused)
    }

    pub fn review(&mut self, index: usize, decision: Decision) {
        if !self.can_review() {
            return;
        }
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        if message.receipt != Receipt::Waiting {
            return;
        }
        message.receipt = match decision {
            Decision::Pass => Receipt::Passed,
            Decision::Decline => Receipt::Declined,
            Decision::Revise(note)
                if message.direction == Direction::Outgoing && !note.trim().is_empty() =>
            {
                message.revision_note = Some(note);
                Receipt::Revision
            }
            Decision::Revise(_) => return,
        };
    }
}

pub(super) enum Decision {
    Pass,
    Decline,
    Revise(String),
}

pub(super) fn sample() -> Vec<Peer> {
    use Direction::{Incoming, Outgoing};
    use Receipt::{Delivered, Notified, Queued, Waiting};
    let message = |direction, time: &str, body: &str, receipt| Message {
        direction,
        time: time.to_owned(),
        body: body.to_owned(),
        receipt,
        revision_note: None,
    };
    let peer = |name: &str, id: &str, link, messages| Peer {
        name: name.into(),
        id: id.into(),
        link,
        gates: Gates::default(),
        messages,
    };
    let mut ops = peer(
        "maya-ops",
        "sample-ops",
        Link::Connected,
        vec![
            message(
                Outgoing,
                "13:42",
                "Did the staging migration finish? I can run the read-only benchmark once the replica catches up.",
                Delivered,
            ),
            message(
                Incoming,
                "13:46",
                "The migration finished. The replica has caught up and the connection pool is back at its usual size.",
                Notified,
            ),
            message(
                Outgoing,
                "14:02",
                "The benchmark finished. The 95th percentile was 38 ms with 20 concurrent workers.",
                Delivered,
            ),
            message(
                Incoming,
                "14:05",
                "Once the release is tagged, send the checksum here so we can verify the staging canary.",
                Waiting,
            ),
        ],
    );
    ops.gates.outgoing_auto = true;
    vec![
        ops,
        peer(
            "alex-core",
            "sample-core",
            Link::Connected,
            vec![message(
                Outgoing,
                "14:08",
                "The migration report is ready. Can your agent check the rollback instructions before we share it?",
                Waiting,
            )],
        ),
        peer(
            "priya-platform",
            "sample-platform",
            Link::IncomingRequest,
            vec![],
        ),
        peer(
            "sam-infra",
            "sample-infra",
            Link::Offline,
            vec![message(
                Outgoing,
                "14:01",
                "Please share the new runner image digest when it is ready.",
                Queued,
            )],
        ),
        peer("old-staging", "sample-staging", Link::Refused, vec![]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incoming_review_cannot_revise_and_passing_does_not_claim_delivery() {
        let mut peers = sample();
        peers[0].review(3, Decision::Revise("more detail".into()));
        assert_eq!(peers[0].waiting(), 1);
        peers[0].review(3, Decision::Pass);
        assert_eq!(peers[0].messages[3].receipt, Receipt::Passed);
        peers[0].review(3, Decision::Decline);
        assert_eq!(peers[0].messages[3].receipt, Receipt::Passed);
        assert!(!peers[0].needs_you());
    }

    #[test]
    fn a_gate_change_preserves_waiting_text_and_a_revision_closes_the_original() {
        let mut peers = sample();
        let peer = &mut peers[1];
        let original = peer.messages[0].body.clone();
        peer.gates.outgoing_auto = true;
        assert_eq!(peer.waiting(), 1);
        peer.review(0, Decision::Revise(" ".into()));
        assert_eq!(peer.waiting(), 1);
        peer.review(0, Decision::Revise("Include the version".into()));
        assert_eq!(peer.messages[0].body, original);
        assert_eq!(
            peer.messages[0].revision_note.as_deref(),
            Some("Include the version")
        );
        assert_eq!(peer.messages[0].receipt, Receipt::Revision);
    }
}
