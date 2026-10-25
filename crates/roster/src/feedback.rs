//! Explicit user feedback on an authored record, through Backbone's message API.

use std::path::Path;

use serde::Deserialize;

use crate::{
    configuration::configured,
    transport, valid_agent,
    views::{ReportContent, ReportItem},
};

/// Local composer limit, independent of Backbone's larger wire limit.
pub const TEXT_LIMIT: usize = 4000;

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

/// Revalidate the source and author, then send once. Runs on an I/O worker.
pub fn send(data_dir: &Path, item: &ReportItem, text: &str) -> Outcome {
    let Some((url, key)) = configured(data_dir, None) else {
        return Outcome::Refused(
            "Backbone is unavailable. Your feedback has not been sent.".into(),
        );
    };
    send_to(&url, &key, item, text)
}

fn send_to(url: &str, key: &str, item: &ReportItem, text: &str) -> Outcome {
    let refused = |text: &str| Outcome::Refused(text.into());
    if text.trim().is_empty() || text.chars().count() > TEXT_LIMIT {
        return refused("Enter feedback of at most 4,000 characters.");
    }
    let Some(author_id) = item.author_id.as_deref().filter(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) else {
        return refused("This record has no stable author identity. Refresh it before sending.");
    };
    if item.id == 0 || item.id > i64::MAX as u64 {
        return refused("This record has an invalid reference.");
    }
    if url != item.provider_url {
        return refused("The Backbone connection changed. Reopen this record before sending.");
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
    let body = match transport::fetch(url, &path, key, limit) {
        Ok(body) if body.len() <= limit => body,
        Err(Some(404)) => {
            return refused("This record is no longer available. Your feedback has not been sent.");
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
    let body = match transport::post_message(url, key, body) {
        Ok(body) => body,
        Err(Some(401 | 403)) => {
            return refused("Backbone refused this app's access. Your feedback has not been sent.");
        }
        Err(Some(404 | 413 | 422)) => {
            return refused("Backbone refused this feedback. Your text is kept here.");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    fn request(socket: &mut TcpStream) -> (String, Vec<u8>) {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        let headers = String::from_utf8(headers).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|n| n.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        (headers, body)
    }

    fn reply(socket: &mut TcpStream, body: &str) {
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    fn item(url: &str) -> ReportItem {
        ReportItem {
            id: 7,
            author: "previous-name".into(),
            author_id: Some("a".repeat(32)),
            provider_url: url.into(),
            created_at: String::new(),
            content: ReportContent::Response {
                question: "Question".into(),
                answer: "Private answer body".into(),
                action: String::new(),
                superseded_by: None,
            },
        }
    }

    #[test]
    fn feedback_uses_the_current_author_and_never_retries_a_lost_post_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let (headers, _) = request(&mut socket);
            assert!(headers.starts_with("GET /api/responses/7 "));
            reply(
                &mut socket,
                &format!(
                    r#"{{"id":7,"author_id":"{}","agent_name":"current-name"}}"#,
                    "a".repeat(32)
                ),
            );
            drop(socket);
            let (mut socket, _) = listener.accept().unwrap();
            let (headers, body) = request(&mut socket);
            assert!(headers.starts_with("POST /api/messages "));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["target_session"], "current-name");
            assert_eq!(body["from_entity"], "agent-interface-app");
            assert_eq!(body["priority"], false);
            assert_eq!(
                body["message"],
                "Feedback on response #7 (source=/api/responses/7).\n\nPlease explain café.\nMore detail."
            );
            assert!(body.get("owner_confirmation").is_none());
            // Losing the response after accepting bytes must not produce another POST.
            drop(socket);
            listener.set_nonblocking(true).unwrap();
            listener
        });
        assert!(matches!(
            send_to(
                &url,
                "fixture-token",
                &item(&url),
                "Please explain café.\nMore detail."
            ),
            Outcome::Unknown(_)
        ));
        let listener = server.join().unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn a_replaced_author_receives_no_feedback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            reply(
                &mut socket,
                &format!(
                    r#"{{"id":7,"author_id":"{}","agent_name":"previous-name"}}"#,
                    "b".repeat(32)
                ),
            );
            drop(socket);
            listener.set_nonblocking(true).unwrap();
            listener
        });
        assert!(matches!(
            send_to(&url, "fixture-token", &item(&url), "Feedback"),
            Outcome::Refused(_)
        ));
        let listener = server.join().unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
