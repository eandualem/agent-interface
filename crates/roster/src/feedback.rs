//! Explicit user feedback on an authored record, through Backbone's message API.

use serde::Deserialize;

use crate::{
    profiles::{Connection, Error, Identity},
    valid_agent,
    views::{ReportContent, ReportItem},
};

/// Result of one user-initiated submission. No outcome causes an automatic retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The server accepted the message; its detail distinguishes delivery and holds.
    Accepted(String),
    /// No message was accepted; the user may correct the problem and send again.
    Refused(String),
    /// Delivery may have happened. Check with the author before sending again.
    Unknown(String),
}

/// Reference included with the user's text. Never includes the authored HTML or prose.
pub fn reference(item: &ReportItem) -> String {
    let (label, path) = match &item.content {
        ReportContent::Progress { .. } => ("report", "reports"),
        ReportContent::Response { .. } => ("response", "responses"),
        ReportContent::Document(_) => ("document revision", "documents"),
    };
    format!(
        "Feedback on {label} #{} (source=/api/{path}/{}).",
        item.id, item.id
    )
}

#[derive(Deserialize)]
struct Author {
    id: u64,
    author_id: String,
    agent_name: Option<String>,
}

#[derive(Deserialize)]
struct Delivery {
    ok: bool,
    queued: bool,
    outcome: String,
    detail: String,
}

/// Feedback on one retained record, pinned to its originating profile.
/// Clones retain the same connection for delayed submissions.
#[derive(Clone)]
pub struct Client {
    connection: Connection,
    item: ReportItem,
}

impl Client {
    /// Reject a record from another profile or lifecycle before any I/O.
    pub fn new(connection: Connection, origin: &Identity, item: ReportItem) -> Result<Self, Error> {
        if connection.identity() != origin {
            return Err(Error::BindingMismatch);
        }
        Ok(Self { connection, item })
    }

    /// Full origin for deferred UI results, never a current-selection lookup.
    pub fn identity(&self) -> &Identity {
        self.connection.identity()
    }

    /// Validate author and submit on the same captured binding. An unknown
    /// delivery must be checked before another send.
    pub fn send(&self, text: &str) -> Outcome {
        send_using(&self.connection, &self.item, text)
    }
}

fn fetch(
    connection: &Connection,
    path: &str,
    body: Option<Vec<u8>>,
    limit: usize,
) -> Result<Vec<u8>, Error> {
    let response = connection.response(path, body, limit)?;
    if (200..300).contains(&response.status) {
        Ok(response.body)
    } else {
        Err(Error::Refused(response.status))
    }
}

/// Revalidate the author, then send once. Runs on an I/O worker.
fn send_using(connection: &Connection, item: &ReportItem, text: &str) -> Outcome {
    let refused = |text: &str| Outcome::Refused(text.into());
    if text.trim().is_empty() {
        return refused("Enter feedback before sending.");
    }
    let Some(author_id) = item.author_id.as_deref().filter(|id| crate::is_uuid(id)) else {
        return refused("This record has no stable author identity. Refresh it before sending.");
    };
    if item.id == 0 || item.id > i64::MAX as u64 {
        return refused("This record has an invalid reference.");
    }
    let path = match &item.content {
        ReportContent::Progress { .. } => format!("/api/reports/{}", item.id),
        ReportContent::Response { .. } => format!("/api/responses/{}", item.id),
        ReportContent::Document(_) => format!("/api/documents/{}", item.id),
    };
    // Documents include their HTML in this endpoint; the existing document wire
    // limit accounts for JSON escaping. None of that content is included in feedback.
    let limit = if matches!(item.content, ReportContent::Document(_)) {
        6 * crate::views::DOCUMENT_HTML_LIMIT + 8192
    } else {
        4 * 1024 * 1024
    };
    let body = match fetch(connection, &path, None, limit) {
        Ok(body) => body,
        Err(Error::Refused(404)) => {
            return refused("This record is no longer available. Your feedback has not been sent.");
        }
        Err(Error::BindingRequired | Error::BindingMismatch) => {
            return refused(
                "The Backbone profile binding was refused. Your feedback has not been sent.",
            );
        }
        _ => return refused("Could not verify the author. Your feedback has not been sent."),
    };
    let Ok(author) = serde_json::from_slice::<Author>(&body) else {
        return refused("Could not verify the author. Your feedback has not been sent.");
    };
    if author.id != item.id || author.author_id != author_id {
        return refused("The record's author changed. Reopen it before sending.");
    }
    let Some(target) = author.agent_name.filter(|name| valid_agent(name)) else {
        return refused("This author is no longer registered. Your feedback has not been sent.");
    };
    let message = format!("{}\n\n{text}", reference(item));
    let body = serde_json::to_vec(&serde_json::json!({
        "target_session": target,
        "from_entity": "agent-interface-app",
        "message": message,
        "priority": false,
    }))
    .expect("feedback JSON");
    let unknown = || {
        Outcome::Unknown("Delivery could not be confirmed. Your text is kept here; check with the author before sending it again.".into())
    };
    let body = match fetch(connection, "/api/messages", Some(body), 65536) {
        Ok(body) => body,
        Err(Error::Refused(401 | 403)) => {
            return refused("Backbone refused this app's access. Your feedback has not been sent.");
        }
        Err(Error::Refused(404 | 413 | 422)) => {
            return refused("Backbone refused this feedback. Your text is kept here.");
        }
        Err(Error::BindingRequired | Error::BindingMismatch) => {
            return refused(
                "The Backbone profile binding was refused. Your feedback has not been sent.",
            );
        }
        Err(_) => return unknown(),
    };
    let Ok(delivery) = serde_json::from_slice::<Delivery>(&body) else {
        return unknown();
    };
    let detail: String = delivery
        .detail
        .chars()
        .filter(|ch| !ch.is_control())
        .take(2000)
        .collect();
    if delivery.outcome == "delivery_failed" {
        Outcome::Unknown(format!("Delivery needs checking. {detail}"))
    } else if (delivery.ok && delivery.outcome == "delivered") || delivery.queued {
        Outcome::Accepted(detail)
    } else {
        Outcome::Unknown(format!("Delivery could not be confirmed. {detail}"))
    }
}
