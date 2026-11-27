//! Peer invitations, review and delivery history on the owner's profile socket.
use super::{AddError, Connection, Error, added, successful, valid_id};
use crate::{RESPONSE_LIMIT, views::encode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const ROOT: &str = "/api/peer-connections";

#[derive(Clone, Debug, Deserialize)]
/// The local identity, invitations, generations and number of messages awaiting review.
pub struct Overview {
    /// The full shareable identity of the local profile.
    pub local_id: String,
    /// Outstanding and retained invitations.
    pub invitations: Vec<Invitation>,
    /// Accepted generations, including locally revoked ones.
    pub connections: Vec<Peer>,
    /// Total number of messages waiting for local review.
    pub pending_reviews: usize,
}

#[derive(Clone, Debug, Deserialize)]
/// One invitation, retained by its stable request ID after the answer.
pub struct Invitation {
    /// Stable service ID of this invitation.
    pub request_id: String,
    /// Full shareable identity of the external profile.
    pub peer_id: String,
    /// Optional self-reported name from the remote profile.
    pub display_name: Option<String>,
    /// Direction relative to the local profile.
    pub direction: Direction,
    /// Service timestamp when the invitation or message expires.
    pub expires_at: String,
    /// Service state, retained verbatim for forward compatibility.
    pub state: String,
    /// The generation issued on acceptance.
    pub generation: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
/// The invitation after an answer and its newly established generation, if accepted.
pub struct Answer {
    /// The answered invitation.
    pub invitation: Invitation,
    /// The generation to which this record belongs.
    pub connection: Option<Peer>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
/// An accepted relationship between two profiles, distinct from any earlier connection.
pub struct Generation {
    /// Full shareable identity of the external profile.
    pub peer_id: String,
    /// The generation issued on acceptance.
    pub generation: String,
}

#[derive(Clone, Debug, Deserialize)]
/// Current service state and per-direction delivery gates for a generation.
pub struct Peer {
    /// The generation to which this record belongs.
    pub connection: Generation,
    /// Optional self-reported name from the remote profile.
    pub display_name: Option<String>,
    /// Service state, retained verbatim for forward compatibility.
    pub state: String,
    /// Current delivery policy in both directions.
    pub gates: Gates,
    /// The relay's lasting refusal of a local disconnect, only while the
    /// state is `revocation_pending`.
    pub revocation_failure: Option<RevocationFailure>,
}

#[derive(Clone, Debug, Deserialize)]
/// Why the relay refused to carry a disconnect to the other profile.
pub struct RevocationFailure {
    /// Verbatim relay refusal code.
    pub code: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Direction relative to the profile that owns this socket.
pub enum Direction {
    /// From the remote agent.
    Incoming,
    /// From the local agent.
    Outgoing,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// The rule applied to future messages in one direction.
pub enum Policy {
    /// Pass future messages without owner review.
    Auto,
    /// Hold future messages for owner review.
    Review,
    /// An unfamiliar service policy, readable but never written by this client.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
/// Independent incoming and outgoing delivery rules.
pub struct Gates {
    /// Policy for messages addressed to the local agent.
    pub incoming: Policy,
    /// Policy for messages sent by the local agent.
    pub outgoing: Policy,
}

#[derive(Clone, Debug, Deserialize)]
/// One oldest-first page, with a cursor for the preceding page.
pub struct Page<T> {
    /// Records ordered oldest first within this page.
    pub items: Vec<T>,
    /// Opaque cursor to the preceding page.
    pub older: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
/// Immutable message bytes, their transport state and any terminal receipt.
pub struct Message {
    /// The generation to which this record belongs.
    pub connection: Generation,
    /// Stable message ID within its generation.
    pub message_id: String,
    /// Direction relative to the local profile.
    pub direction: Direction,
    /// The complete immutable message text.
    pub text: String,
    /// Original submission timestamp from the service.
    pub submitted_at: String,
    /// Service timestamp when the invitation or message expires.
    pub expires_at: String,
    /// Service state, retained verbatim for forward compatibility.
    pub state: String,
    /// Evidence of final delivery or remote decline.
    pub receipt: Receipt,
    /// Upload refusal and retry time, when provided.
    pub upload_failure: Option<UploadFailure>,
    /// Which draft this is, 1 for a first draft; absent on older records.
    pub revision: Option<u32>,
    /// The local orchestrator that sent an outgoing message, or the agent an
    /// incoming one was handed to; absent until it was, and on older records.
    pub local_agent: Option<String>,
    /// When the local owner passed, declined or asked for a revision; absent
    /// for an automatic pass, while the message waits, and on older records.
    pub decided_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
/// One oldest-first history page with the connection's events in its span.
pub struct History {
    /// Messages ordered oldest first within this page.
    pub items: Vec<Message>,
    /// Opaque cursor to the preceding page.
    pub older: Option<String>,
    /// Events ordered oldest first; absent when the page has none.
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Deserialize)]
/// Something that happened to a connection, at a time on this profile's clock.
pub struct Event {
    /// Stable event ID within its generation.
    pub id: u64,
    /// When it happened.
    pub at: String,
    /// What happened.
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// The kinds of connection event.
pub enum EventKind {
    /// The connection was established.
    Connected,
    /// The owner changed one direction's gate.
    GateChanged {
        /// The direction changed.
        direction: Direction,
        /// The policy before.
        from: Policy,
        /// The policy after.
        to: Policy,
    },
    /// The owner sent an outgoing draft back with a note.
    RevisionRequested {
        /// The draft sent back.
        message_id: String,
        /// The owner's note.
        note: String,
    },
    /// The local owner asked to disconnect.
    DisconnectRequested,
    /// The connection ended.
    Disconnected,
    /// A kind this client does not know.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
/// The service’s reason for declining an upload attempt.
pub struct UploadFailure {
    /// Verbatim service reason.
    pub code: String,
    /// Service retry timestamp when known.
    pub retry_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// What the service knows about the final delivery decision.
pub enum Receipt {
    /// No terminal outcome has been established.
    Missing,
    /// Dispatch may already have happened.
    Uncertain,
    /// The service recorded a terminal outcome.
    Confirmed {
        /// Verbatim terminal outcome.
        outcome: String,
        /// When the terminal receipt was recorded.
        recorded_at: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
/// An item held for owner review, ordered by local arrival.
pub struct Review {
    /// The complete message awaiting review.
    pub message: Message,
    /// When this message entered the local review queue.
    pub queued_at: String,
}

/// The owner’s explicit decision on one immutable message.
pub enum Decision<'a> {
    /// Release this message unchanged.
    Pass,
    /// Decline this message.
    Decline,
    /// Return an outgoing draft to the local agent with a note.
    Revise(&'a str),
}

fn segment(value: &str) -> Result<String, Error> {
    if !valid_id(value) || matches!(value, "." | "..") {
        return Err(Error::Invalid);
    }
    Ok(encode(value))
}

fn page_path(path: String, before: Option<&str>) -> String {
    match before {
        Some(cursor) => format!("{path}?before={}", encode(cursor)),
        None => path,
    }
}

impl Connection {
    fn read_peer<T: DeserializeOwned>(&self, path: String) -> Result<T, Error> {
        let response = self.response(&path, None, RESPONSE_LIMIT)?;
        successful(&response)?;
        serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)
    }

    // The caller retains the same key and payload across an uncertain reply.
    /// A change the service refuses (4xx) keeps its reason, written for the
    /// person; a server error keeps its status, so the change can be retried.
    fn change_peer<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: String,
        body: Value,
        key: &str,
    ) -> Result<T, AddError> {
        let response = self.send_peer(method, &path, &body, key)?;
        if (400..500).contains(&response.status) {
            added(&response)?;
        }
        successful(&response)?;
        Ok(serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?)
    }

    fn send_peer(
        &self,
        method: &'static str,
        path: &str,
        body: &Value,
        key: &str,
    ) -> Result<crate::transport::local::Response, Error> {
        if !valid_id(key) || key.len() > 1024 {
            return Err(Error::Invalid);
        }
        let body = serde_json::to_vec(body).map_err(|_| Error::Invalid)?;
        self.write(method, path, Some(body), Some(key))
    }

    /// A connection request keeps the service's reason only for a conflict or
    /// an invalid request; any other refusal, such as a 404 from a service
    /// without the route, keeps its status.
    fn request_peer<T: DeserializeOwned>(
        &self,
        path: String,
        body: Value,
        key: &str,
    ) -> Result<T, AddError> {
        let response = self.send_peer("POST", &path, &body, key)?;
        if matches!(response.status, 409 | 422) {
            added(&response)?;
        }
        successful(&response)?;
        Ok(serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?)
    }

    /// Read the owner’s identity, invitations, generations and review badge.
    pub fn peer_overview(&self) -> Result<Overview, Error> {
        self.read_peer(ROOT.into())
    }

    /// Look up an invitation by its original request ID.
    pub fn peer_invitation(&self, id: &str) -> Result<Invitation, Error> {
        self.read_peer(format!("{ROOT}/requests/{}", segment(id)?))
    }

    /// Read the current state and gates of a generation.
    pub fn peer_connection(&self, generation: &str) -> Result<Peer, Error> {
        self.read_peer(format!("{ROOT}/{}", segment(generation)?))
    }

    /// Read one history page and verify that every message belongs to the selected generation.
    pub fn peer_history(
        &self,
        generation: &Generation,
        before: Option<&str>,
    ) -> Result<History, Error> {
        let path = format!("{ROOT}/{}/history", segment(&generation.generation)?);
        let page: History = self.read_peer(page_path(path, before))?;
        if page.items.iter().any(|m| &m.connection != generation) {
            return Err(Error::BindingMismatch);
        }
        Ok(page)
    }

    /// Read the local owner’s pending reviews in queue order.
    pub fn peer_reviews(&self, before: Option<&str>) -> Result<Page<Review>, Error> {
        self.read_peer(page_path(format!("{ROOT}/reviews"), before))
    }

    /// Send an invitation under an idempotency key retained by the caller.
    pub fn invite_peer(&self, id: &str, key: &str) -> Result<Invitation, AddError> {
        self.request_peer(format!("{ROOT}/requests"), json!({"peer_id":id}), key)
    }

    /// Accept or decline an invitation under its request ID.
    pub fn answer_peer(&self, id: &str, accept: bool, key: &str) -> Result<Answer, AddError> {
        let path = format!("{ROOT}/requests/{}/answer", segment(id)?);
        self.request_peer(
            path,
            json!({"action":if accept {"accept"} else {"decline"}}),
            key,
        )
    }

    /// Change one direction; messages already in review retain their original decision requirement.
    pub fn set_peer_gate(
        &self,
        generation: &str,
        direction: Direction,
        policy: Policy,
        key: &str,
    ) -> Result<Peer, AddError> {
        if policy == Policy::Unknown {
            return Err(Error::Invalid.into());
        }
        let path = format!("{ROOT}/{}/gates", segment(generation)?);
        self.change_peer(
            "PATCH",
            path,
            json!({"direction":direction,"policy":policy}),
            key,
        )
    }

    /// Apply an explicit review action without changing or resending the original text.
    pub fn review_peer_message(
        &self,
        message: &Message,
        decision: Decision<'_>,
        key: &str,
    ) -> Result<Message, AddError> {
        if message.state != "pending_review" {
            return Err(Error::Invalid.into());
        }
        let body = match decision {
            Decision::Pass => json!({"action":"pass"}),
            Decision::Decline => json!({"action":"decline"}),
            Decision::Revise(note)
                if message.direction == Direction::Outgoing && !note.trim().is_empty() =>
            {
                json!({"action":"ask_to_revise","note":note})
            }
            Decision::Revise(_) => return Err(Error::Invalid.into()),
        };
        let path = format!(
            "{ROOT}/{}/messages/{}/review",
            segment(&message.connection.generation)?,
            segment(&message.message_id)?
        );
        self.change_peer("POST", path, body, key)
    }

    /// Request local revocation of this generation.
    pub fn disconnect_peer(&self, generation: &str, key: &str) -> Result<Peer, AddError> {
        self.change_peer(
            "POST",
            format!("{ROOT}/{}/disconnect", segment(generation)?),
            json!({}),
            key,
        )
    }
}

/// Follow invalidations. The cursor belongs to this one profile connection.
/// Read the snapshot on hello, and read it again for changes during the read.
impl Connection {
    /// Follow invalidations and resume from the last cursor on reconnect.
    pub fn watch_peers(
        &self,
        cursor: &mut Option<String>,
        live: &dyn Fn() -> bool,
        changed: &mut dyn FnMut(),
    ) -> Result<(), Error> {
        let mut frame = PeerFrame::default();
        let resume = cursor.clone();
        crate::transport::local::stream(
            self.socket.to_str().ok_or(Error::Invalid)?,
            "/api/stream?kinds=peer.connections.changed,resync",
            &self.identity.profile_id,
            resume.as_deref(),
            std::time::Duration::from_secs(45),
            live,
            &mut |chunk| frame.push(chunk, cursor, changed),
        )
        .map_err(|status| status.map(Error::Refused).unwrap_or(Error::Unavailable))
    }
}

#[derive(Default)]
struct PeerFrame {
    line: Vec<u8>,
    kind: Vec<u8>,
    id: Option<String>,
    data: Vec<u8>,
}

impl PeerFrame {
    fn push(
        &mut self,
        chunk: &[u8],
        cursor: &mut Option<String>,
        changed: &mut dyn FnMut(),
    ) -> bool {
        for &byte in chunk {
            if byte != b'\n' {
                if self.line.len() >= 64 * 1024 {
                    return false;
                }
                self.line.push(byte);
                continue;
            }
            let line = self.line.trim_ascii_end();
            if line.is_empty() {
                match self.kind.as_slice() {
                    b"hello" => {
                        #[derive(Deserialize)]
                        struct Hello {
                            cursor: String,
                        }
                        let Ok(hello) = serde_json::from_slice::<Hello>(&self.data) else {
                            return false;
                        };
                        if cursor.is_none() {
                            *cursor = Some(hello.cursor);
                        }
                        changed();
                    }
                    b"resync" | b"peer.connections.changed" => {
                        *cursor = self.id.take();
                        changed();
                    }
                    _ => {}
                }
                self.kind.clear();
                self.data.clear();
                self.id = None;
            } else if let Some(value) = line.strip_prefix(b"event:") {
                self.kind = value.trim_ascii().to_vec();
            } else if let Some(value) = line.strip_prefix(b"id:") {
                let Ok(value) = std::str::from_utf8(value.trim_ascii()) else {
                    return false;
                };
                self.id = Some(value.into());
            } else if let Some(value) = line.strip_prefix(b"data:") {
                self.data = value.trim_ascii_start().to_vec();
            }
            self.line.clear();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_hello_and_changes_preserve_the_resume_cursor() {
        let mut stream = PeerFrame::default();
        let mut cursor = None;
        let mut changes = 0;
        for bytes in b"event: hello\r\ndata: {\"cursor\":\"start:1\"}\r\n\r\n".chunks(3) {
            assert!(stream.push(bytes, &mut cursor, &mut || changes += 1));
        }
        assert_eq!(cursor.as_deref(), Some("start:1"));
        assert!(stream.push(
            b"id: start:2\nevent: peer.connections.changed\ndata: {}\n\n",
            &mut cursor,
            &mut || changes += 1
        ));
        assert!(stream.push(
            b"event: hello\ndata: {\"cursor\":\"start:3\"}\n\n",
            &mut cursor,
            &mut || changes += 1
        ));
        assert_eq!(cursor.as_deref(), Some("start:2"));
        assert_eq!(changes, 3);
    }

    #[test]
    fn the_service_change_frame_advances_the_cursor() {
        let event: Value = serde_json::from_str(include_str!(
            "peers/fixtures/stream-peer-connections-changed.json"
        ))
        .unwrap();
        let id = event["id"].as_str().unwrap();
        let kind = event["event"].as_str().unwrap();
        let frame = format!("id: {id}\nevent: {kind}\ndata: {}\n\n", event["data"]);
        let mut cursor = Some("earlier".to_owned());
        let mut changes = 0;
        let mut stream = PeerFrame::default();
        assert!(stream.push(frame.as_bytes(), &mut cursor, &mut || changes += 1));
        assert_eq!(cursor.as_deref(), Some(id));
        assert_eq!(changes, 1);
    }
}
