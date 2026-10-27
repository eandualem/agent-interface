//! One conversation per artifact, read and written through Backbone's
//! authenticated local API (provider contract r3).
//!
//! A conversation lasts across all of an artifact's versions. Its entries are
//! owner and agent messages, and the events the artifact produces when a new
//! version or plan step appears. Transport and parsing run on I/O workers,
//! never the UI thread.

use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};

/// What a conversation is about.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A document, across its revisions.
    Document,
    /// A workbook, across its revisions.
    Sheet,
    /// An answer and its corrections.
    Response,
    /// A progress report and its corrections.
    Report,
    /// An agent's current plan.
    Plan,
    /// A shared skill.
    Skill,
    /// A policy.
    Policy,
}

/// One entry, in the order Backbone stored it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Entry {
    /// Stable and increasing across the installation.
    pub id: u64,
    #[serde(flatten)]
    /// What it says.
    pub body: Body,
    /// When Backbone stored it, RFC 3339 UTC.
    pub created_at: String,
    /// Who wrote it; for events, who published.
    pub author: Author,
    /// The version or place the author was looking at.
    #[serde(default)]
    pub anchor: Option<Anchor>,
}

/// What an entry says.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// Something the owner or an agent wrote.
    Message {
        /// At most 4,000 characters.
        text: String,
        /// Owner messages: the agent it was addressed to.
        #[serde(default)]
        recipient: Option<Agent>,
        /// Agent messages: the entry this answers.
        #[serde(default)]
        in_reply_to: Option<u64>,
        /// Agent messages: the version this reply produced.
        #[serde(default)]
        resulting_revision_id: Option<String>,
        /// Owner messages: how far delivery got.
        #[serde(default)]
        delivery: Option<Delivery>,
    },
    /// A new version of the artifact appeared.
    Revision {
        /// The version.
        revision: Revision,
    },
    /// A newly published plan changed a step's status.
    Step {
        /// The step and its new state.
        step: Step,
    },
}

/// Who wrote an entry.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Author {
    /// The person using the app.
    Owner,
    /// An agent, by registration.
    Agent {
        /// Stable registration identity.
        agent_id: String,
        /// Current registered name.
        name: String,
    },
    /// Backbone itself, such as for a change it observed, or an author type
    /// this client does not know.
    #[serde(other)]
    System,
}

/// An agent registration: stable identity plus its current name.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Agent {
    /// Stable registration identity.
    pub agent_id: String,
    /// Current registered name.
    pub name: String,
}

/// How far an owner message's delivery got. A reply is a later entry, not a
/// delivery state.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Stored, queued or retrying.
    Pending,
    /// Pasted, or acknowledged from the inbox.
    Delivered,
    /// Waiting in the recipient's inbox.
    Held,
    /// Refused or expired; it can be redelivered.
    Failed,
}

/// Where an entry points: a version, and a range or plan item within it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Anchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// The version the author was viewing.
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Sheets: the selected cells.
    pub sheet: Option<SheetAnchor>,
    /// A plan item or dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// The text shown when it was written, such as `Runs!B3:D5`; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Cells by stable tab, row and column identity, so a sort or move keeps them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SheetAnchor {
    /// Stable tab identity.
    pub tab_id: String,
    /// Rows, in selection order.
    pub row_ids: Vec<String>,
    /// Columns, in selection order.
    pub column_ids: Vec<String>,
}

/// A version that appeared.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Revision {
    /// The new version's identity.
    pub revision_id: String,
    /// Such as `rev 12`.
    pub label: String,
    /// Sheets: the cells that changed, when the provider could compute them.
    #[serde(default)]
    pub changed: Vec<Changed>,
}

/// Changed cells, by label and stable identity.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Changed {
    /// Display text.
    pub label: String,
    /// The cells, by stable identity.
    pub sheet: SheetAnchor,
}

/// A plan step's new state.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Step {
    /// Stable across plan revisions.
    pub item_id: String,
    /// What the step is.
    pub title: String,
    /// `pending`, `in_progress`, `blocked` or `done`.
    pub status: String,
    /// `now`, `next` or `later`.
    #[serde(default)]
    pub section: String,
}

/// The most a response may hold; the provider caps a page at 1 MiB.
const LIMIT: usize = 1024 * 1024 + 64 * 1024;

impl Kind {
    /// The path segment the API uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Sheet => "sheet",
            Self::Response => "response",
            Self::Report => "report",
            Self::Plan => "plan",
            Self::Skill => "skill",
            Self::Policy => "policy",
        }
    }
}

/// An agent as a conversation lists it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Participant {
    /// Stable registration identity; empty while unassigned.
    #[serde(default, deserialize_with = "text")]
    pub agent_id: String,
    /// Current registered name; empty while unassigned.
    #[serde(default, deserialize_with = "text")]
    pub name: String,
    /// `active`, `retired` or `unassigned`.
    #[serde(default)]
    pub status: String,
    /// Whether it receives messages that name no recipient.
    #[serde(default)]
    pub responsible: bool,
}

/// A string the provider may send as null.
fn text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// One page of a conversation and its header.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Conversation {
    /// What it is about.
    pub kind: Kind,
    /// The artifact's stable identity.
    pub artifact_id: String,
    /// The default recipient.
    pub responsible: Participant,
    /// The responsible agent and every agent addressed or writing here.
    #[serde(default)]
    pub participants: Vec<Participant>,
    /// Messages, events excluded.
    pub total: u64,
    /// Agent messages and events after `read_through`.
    pub unread: u64,
    /// The owner's read marker; the "new" divider sits after it.
    #[serde(default)]
    pub read_through: Option<u64>,
    /// Oldest to newest within the page.
    pub entries: Vec<Entry>,
    /// Pass as [`Page::Before`]; `None` at the start.
    pub older: Option<String>,
    /// Pass as [`Page::After`]; `None` on the newest page.
    pub newer: Option<String>,
}

/// One conversation's badge.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Summary {
    /// What it is about.
    pub kind: Kind,
    /// The artifact's stable identity.
    pub artifact_id: String,
    /// The default recipient.
    pub responsible: Participant,
    /// Agent messages and events not yet read.
    pub unread: u64,
    /// Messages, events excluded.
    pub total: u64,
    /// When the latest entry was stored.
    #[serde(default)]
    pub last_entry_at: Option<String>,
}

#[derive(Deserialize)]
struct Summaries {
    items: Vec<Summary>,
}

/// Header counts after the read marker moved.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Counts {
    /// The marker now.
    pub read_through: u64,
    /// Agent messages and events after it.
    pub unread: u64,
    /// Messages, events excluded.
    pub total: u64,
}

/// Which page of a conversation to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    /// The newest entries.
    Newest,
    /// Entries older than a page's `older` cursor.
    Before(String),
    /// Entries newer than a page's `newer` cursor, for polling.
    After(String),
    /// Exactly these entries, up to 50, to follow their delivery.
    Entries(Vec<u64>),
}

/// An owner message to append.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Message {
    /// Client idempotency key; a retry reuses it.
    pub request_id: String,
    /// At most 4,000 characters.
    pub text: String,
    /// Registration ID or name; `None` for the responsible agent.
    pub recipient: Option<String>,
    /// What the owner was looking at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
}

/// What became of a send or a redelivery.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Stored; the entry carries its delivery state.
    Accepted(Box<Entry>),
    /// A redelivery for this entry is already queued.
    AlreadyPending,
    /// Refused with a typed code and readable text.
    Refused {
        /// Such as `too_long` or `rate_limited`.
        error: String,
        /// Safe to show the person.
        message: String,
    },
    /// No answer that says either way; retry with the same request ID.
    Unknown,
}

/// Why a conversation could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No authenticated private local provider is configured.
    Unavailable,
    /// The configured provider is not the one the record came from.
    Moved,
    /// Backbone answered but did not let this app read or write it.
    Forbidden,
    /// The provider does not offer conversations yet.
    Unsupported,
    /// Backbone knows no such artifact.
    UnknownArtifact,
    /// The response violated the bounded contract.
    Unreadable,
}

impl Error {
    /// Safe, fixed text for the person.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Conversations need an authenticated private Backbone connection.",
            Self::Moved => "The Backbone connection changed. Reopen this item to continue.",
            Self::Forbidden => "Backbone did not let this app open this conversation.",
            Self::Unsupported => "This Agent Backbone does not keep conversations yet.",
            Self::UnknownArtifact => "Backbone has no record of this item.",
            Self::Unreadable => "Backbone returned an unreadable conversation.",
        }
    }
}

/// Read one page of a conversation. Runs on an I/O worker.
pub fn show(
    directory: &Path,
    provider: &str,
    kind: Kind,
    id: &str,
    page: &Page,
) -> Result<Conversation, Error> {
    let path = show_path(kind, id, page).ok_or(Error::Unreadable)?;
    let response = exchange(directory, provider, &path, None)?;
    match response.status {
        200 => parse(&response.body),
        404 => Err(missing(&response.body)),
        401 | 403 => Err(Error::Forbidden),
        _ => Err(Error::Unavailable),
    }
}

/// The provider conversations go to now, for an artifact read without one,
/// such as a skill. Runs on an I/O worker; the key never leaves this crate.
pub fn provider(directory: &Path) -> Option<String> {
    crate::configuration::configured(directory, None).map(|(provider, _)| provider)
}

/// The conversation identity of a skill or policy, by the source path the
/// app shows it under. A provider without the identity is `Unsupported`.
/// Runs on an I/O worker.
pub fn resolve(directory: &Path, provider: &str, kind: Kind, path: &str) -> Result<String, Error> {
    #[derive(Deserialize)]
    struct Item {
        #[serde(default)]
        path: String,
        #[serde(default)]
        artifact_id: Option<String>,
    }
    #[derive(Deserialize)]
    struct Listing {
        items: Vec<Item>,
    }
    let route = match kind {
        Kind::Skill => "/api/skills",
        Kind::Policy => "/api/policies",
        _ => return Err(Error::Unsupported),
    };
    let response = exchange(directory, provider, route, None)?;
    match response.status {
        200 => {
            let listing: Listing = parse(&response.body)?;
            listing
                .items
                .into_iter()
                .find(|item| item.path == path)
                .and_then(|item| item.artifact_id)
                .filter(|id| token(id))
                .ok_or(Error::Unsupported)
        }
        404 => Err(Error::Unsupported),
        401 | 403 => Err(Error::Forbidden),
        _ => Err(Error::Unavailable),
    }
}

/// Badges for up to 50 conversations. Runs on an I/O worker.
pub fn summaries(
    directory: &Path,
    provider: &str,
    keys: &[(Kind, &str)],
) -> Result<Vec<Summary>, Error> {
    if keys.is_empty() || keys.len() > 50 || keys.iter().any(|(_, id)| !token(id)) {
        return Err(Error::Unreadable);
    }
    let ids: Vec<String> = keys
        .iter()
        .map(|(kind, id)| format!("{}:{id}", kind.name()))
        .collect();
    let path = format!("/api/conversations?limit=50&ids={}", ids.join(","));
    let response = exchange(directory, provider, &path, None)?;
    match response.status {
        200 => parse::<Summaries>(&response.body).map(|page| page.items),
        404 => Err(Error::Unsupported),
        401 | 403 => Err(Error::Forbidden),
        _ => Err(Error::Unavailable),
    }
}

/// Append an owner message. Runs on an I/O worker.
pub fn send(directory: &Path, provider: &str, kind: Kind, id: &str, message: &Message) -> Outcome {
    let Some(path) = base(kind, id).map(|base| base + "/messages") else {
        return Outcome::Unknown;
    };
    let Ok(body) = serde_json::to_vec(message) else {
        return Outcome::Unknown;
    };
    outcome(exchange(directory, provider, &path, Some(body)))
}

/// Queue a new delivery for a failed owner message. Runs on an I/O worker.
pub fn redeliver(
    directory: &Path,
    provider: &str,
    kind: Kind,
    id: &str,
    entry: u64,
    request_id: &str,
) -> Outcome {
    let Some(path) = base(kind, id).map(|base| format!("{base}/messages/{entry}/redeliver")) else {
        return Outcome::Unknown;
    };
    let body = serde_json::json!({ "request_id": request_id })
        .to_string()
        .into_bytes();
    outcome(exchange(directory, provider, &path, Some(body)))
}

/// Move the owner's read marker forward. Runs on an I/O worker.
pub fn read(
    directory: &Path,
    provider: &str,
    kind: Kind,
    id: &str,
    through: u64,
) -> Result<Counts, Error> {
    let path = base(kind, id).ok_or(Error::Unreadable)? + "/read";
    let body = serde_json::json!({ "read_through": through })
        .to_string()
        .into_bytes();
    let response = exchange(directory, provider, &path, Some(body))?;
    match response.status {
        200 => parse(&response.body),
        404 => Err(missing(&response.body)),
        401 | 403 => Err(Error::Forbidden),
        _ => Err(Error::Unavailable),
    }
}

fn show_path(kind: Kind, id: &str, page: &Page) -> Option<String> {
    let mut path = base(kind, id)? + "?limit=50";
    match page {
        Page::Newest => {}
        Page::Before(cursor) | Page::After(cursor) if !token(cursor) => return None,
        Page::Before(cursor) => path.push_str(&format!("&before={cursor}")),
        Page::After(cursor) => path.push_str(&format!("&after={cursor}")),
        Page::Entries(ids) if ids.is_empty() || ids.len() > 50 => return None,
        Page::Entries(ids) => {
            let ids: Vec<String> = ids.iter().map(u64::to_string).collect();
            path.push_str(&format!("&entries={}", ids.join(",")));
        }
    }
    Some(path)
}

fn base(kind: Kind, id: &str) -> Option<String> {
    token(id).then(|| format!("/api/conversations/{}/{id}", kind.name()))
}

/// Identifiers and cursors are opaque but never need escaping.
fn token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

/// `provider` is the address the record was read from; a changed connection
/// could hold an unrelated artifact under the same ID.
fn exchange(
    directory: &Path,
    expected: &str,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<crate::transport::local::Response, Error> {
    let (provider, key) =
        crate::configuration::configured(directory, None).ok_or(Error::Unavailable)?;
    if provider != expected {
        return Err(Error::Moved);
    }
    // Conversations need the authenticated private socket; another connection
    // keeps the one-way feedback it already offers.
    let socket = provider.strip_prefix("unix:").ok_or(Error::Unsupported)?;
    if key.is_empty() {
        return Err(Error::Unsupported);
    }
    let route = path.split('?').next().unwrap_or_default();
    let response =
        crate::transport::local::response(socket, path, &key, body, LIMIT, Duration::from_secs(15))
            .map_err(|_| {
                tracing::debug!(
                    event = "conversation_unreachable",
                    route,
                    "Conversation request failed"
                );
                Error::Unavailable
            })?;
    // The status only: bodies hold the person's messages.
    tracing::debug!(
        event = "conversation_response",
        route,
        status = response.status,
        "Conversation request answered"
    );
    Ok(response)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    if body.len() > LIMIT {
        return Err(Error::Unreadable);
    }
    serde_json::from_slice(body).map_err(|_| Error::Unreadable)
}

/// A plain 404 is a provider without conversations; a typed one is a missing artifact.
fn missing(body: &[u8]) -> Error {
    #[derive(Deserialize)]
    struct Missing {
        error: String,
    }
    match serde_json::from_slice::<Missing>(body) {
        Ok(missing) if missing.error == "unknown_artifact" => Error::UnknownArtifact,
        _ => Error::Unsupported,
    }
}

fn outcome(response: Result<crate::transport::local::Response, Error>) -> Outcome {
    #[derive(Deserialize)]
    struct Reply {
        result: String,
        #[serde(default)]
        entry: Option<Entry>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    let response = match response {
        Ok(response) => response,
        // Nothing was sent to a provider the record did not come from.
        Err(error @ Error::Moved) => {
            return Outcome::Refused {
                error: "moved".into(),
                message: error.message().into(),
            };
        }
        Err(_) => return Outcome::Unknown,
    };
    let Ok(reply) = parse::<Reply>(&response.body) else {
        return Outcome::Unknown;
    };
    match (reply.result.as_str(), reply.entry) {
        ("accepted" | "queued", Some(entry)) if (200..300).contains(&response.status) => {
            Outcome::Accepted(Box::new(entry))
        }
        ("already_pending", _) => Outcome::AlreadyPending,
        ("refused", _) if (400..500).contains(&response.status) || response.status == 200 => {
            Outcome::Refused {
                error: reply.error.unwrap_or_default(),
                message: reply
                    .message
                    .unwrap_or_else(|| "Backbone refused the message.".to_owned()),
            }
        }
        _ => Outcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_messages_revisions_and_steps() {
        let entries: Vec<Entry> = serde_json::from_str(
            r#"[
              {"id":4182,"kind":"message","created_at":"2026-10-27T14:30:00Z",
               "author":{"type":"owner"},"text":"Could History come first?",
               "recipient":{"agent_id":"a1","name":"builder"},"delivery":"held",
               "anchor":{"revision_id":"r11","label":"Runs!B3:D5",
                 "sheet":{"tab_id":"t","row_ids":["r3","r4"],"column_ids":["c2"]}}},
              {"id":4183,"kind":"message","created_at":"2026-10-27T14:41:03Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "text":"Yes.","in_reply_to":4182,"resulting_revision_id":"r12"},
              {"id":4184,"kind":"revision","created_at":"2026-10-27T14:42:00Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "revision":{"revision_id":"r12","label":"rev 12","changed":[
                 {"label":"Runs!D4","sheet":{"tab_id":"t","row_ids":["r4"],"column_ids":["c4"]}}]}},
              {"id":4185,"kind":"step","created_at":"2026-10-27T14:43:00Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "step":{"item_id":"b","title":"Verify candidate","status":"in_progress","section":"now"}}
            ]"#,
        )
        .unwrap();
        assert!(matches!(
            entries[0].body,
            Body::Message {
                delivery: Some(Delivery::Held),
                ..
            }
        ));
        assert_eq!(
            entries[0]
                .anchor
                .as_ref()
                .unwrap()
                .sheet
                .as_ref()
                .unwrap()
                .row_ids
                .len(),
            2
        );
        assert!(matches!(&entries[1].author, Author::Agent { name, .. } if name == "builder"));
        assert!(
            matches!(&entries[2].body, Body::Revision { revision } if revision.changed[0].label == "Runs!D4")
        );
        assert!(matches!(&entries[3].body, Body::Step { step } if step.status == "in_progress"));
    }

    #[test]
    fn a_page_reads_its_header_and_cursors() {
        let page: Conversation = parse(
            br#"{"kind":"report","artifact_id":"3144",
              "responsible":{"agent_id":"6bff","name":"builder","status":"active"},
              "participants":[{"agent_id":"6bff","name":"builder","status":"active","responsible":true}],
              "total":4,"unread":2,"read_through":4180,"entries":[],
              "older":"e4178","newer":null}"#,
        )
        .unwrap();
        assert_eq!((page.unread, page.read_through), (2, Some(4180)));
        assert_eq!(page.older.as_deref(), Some("e4178"));
        assert!(page.participants[0].responsible);
        assert!(token("c2:report:3144") && !token("a/b") && !token("x&y"));
        assert_eq!(
            show_path(Kind::Report, "3144", &Page::Before("e4178".into())).as_deref(),
            Some("/api/conversations/report/3144?limit=50&before=e4178")
        );
        assert!(show_path(Kind::Sheet, "../x", &Page::Newest).is_none());
        assert_eq!(
            show_path(Kind::Document, "d", &Page::Entries(vec![4182, 4190])).as_deref(),
            Some("/api/conversations/document/d?limit=50&entries=4182,4190")
        );
        assert!(show_path(Kind::Document, "d", &Page::Entries(Vec::new())).is_none());
        assert!(show_path(Kind::Plan, "a", &Page::After("x y".into())).is_none());
    }

    #[test]
    fn replies_and_missing_artifacts_are_told_apart() {
        let reply = |status, body: &str| {
            outcome(Ok(crate::transport::local::Response {
                status,
                body: body.as_bytes().to_vec(),
                sheet_warnings: None,
            }))
        };
        assert_eq!(
            reply(
                422,
                r#"{"result":"refused","error":"too_long","message":"Too long."}"#
            ),
            Outcome::Refused {
                error: "too_long".into(),
                message: "Too long.".into()
            }
        );
        assert_eq!(
            reply(200, r#"{"result":"already_pending","error":null}"#),
            Outcome::AlreadyPending
        );
        assert_eq!(reply(502, "gateway"), Outcome::Unknown);
        assert_eq!(
            missing(br#"{"error":"unknown_artifact","message":"No report 9999."}"#),
            Error::UnknownArtifact
        );
        assert_eq!(missing(b"Not Found"), Error::Unsupported);
    }

    /// Bodies captured from the provider's routes.
    #[test]
    fn provider_bodies_parse() {
        let page: Conversation = parse(br#"{"kind": "document", "artifact_id": "59dc30d330934c35ba501c6622e1bcfd", "title": "Benchmark runs", "latest_revision_id": "2", "responsible": {"agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder", "status": "active"}, "participants": [{"agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder", "status": "active", "responsible": true}], "total": 2, "unread": 2, "read_through": 0, "entries": [{"id": 1, "kind": "message", "created_at": "2026-10-27T18:16:51.345939Z", "author": {"type": "owner", "agent_id": null, "name": "owner"}, "text": "Could History come before column paging?", "anchor": {"revision_id": "1", "label": "Runs!B3:D5"}, "recipient": {"agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder"}, "delivery": "pending"}, {"id": 2, "kind": "revision", "created_at": "2026-10-27T18:16:51.352948Z", "author": {"type": "agent", "agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder"}, "revision": {"revision_id": "2", "label": "rev 2"}}, {"id": 3, "kind": "message", "created_at": "2026-10-27T18:16:51.356040Z", "author": {"type": "agent", "agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder"}, "text": "Done, see rev 2.", "anchor": null, "in_reply_to": 1, "resulting_revision_id": "2"}], "older": null, "newer": null}"#).unwrap();
        assert!(matches!(page.entries[0].author, Author::Owner));
        let unassigned: Participant =
            serde_json::from_str(r#"{"agent_id":null,"name":null,"status":"unassigned"}"#).unwrap();
        assert!(unassigned.name.is_empty());
        let observed: Author =
            serde_json::from_str(r#"{"type":"backbone","agent_id":null,"name":"backbone"}"#)
                .unwrap();
        assert_eq!(observed, Author::System);
        assert!(
            matches!(&page.entries[1].body, Body::Revision { revision } if revision.changed.is_empty())
        );
        assert!(matches!(&page.entries[2].body,
            Body::Message { resulting_revision_id: Some(id), in_reply_to: Some(1), .. } if id == "2"));
        let reply = |status, body: &str| {
            outcome(Ok(crate::transport::local::Response {
                status,
                body: body.as_bytes().to_vec(),
                sheet_warnings: None,
            }))
        };
        assert_eq!(
            reply(
                200,
                r#"{"result": "already_pending", "entry": {"id": 1, "kind": "message", "created_at": "2026-10-27T18:16:51.345939Z", "author": {"type": "owner", "agent_id": null, "name": "owner"}, "text": "Could History come before column paging?", "anchor": {"revision_id": "1", "label": "Runs!B3:D5"}, "recipient": {"agent_id": "b0c85a4ec22d4754a1183fd1abf8ef1f", "name": "builder"}, "delivery": "pending"}, "error": null}"#
            ),
            Outcome::AlreadyPending
        );
    }
}
