//! Starting or resuming a registered agent through Backbone, on request only.

use std::{path::Path, time::Duration};

use serde::Deserialize;

use crate::{configuration::configured, transport, valid_agent};

/// One request's result. An ambiguous reply must never cause automatic resubmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Accepted by Backbone; the terminal connects once the session exists.
    Accepted(String),
    /// A running session was left untouched.
    AlreadyRunning,
    /// Refused locally or by the provider.
    Refused(String),
    /// The request may have started the agent. Check before trying again.
    Unknown,
}

impl Outcome {
    /// A short user-facing result, without private response diagnostics.
    pub fn message(&self) -> String {
        match self {
            Self::Accepted(message) | Self::Refused(message) => message.clone(),
            Self::AlreadyRunning => "Already running. Connecting to its terminal.".into(),
            Self::Unknown => {
                "Could not confirm the launch. Check the agent's status before trying again.".into()
            }
        }
    }
}

/// Start an agent with its saved runtime and model, resuming its saved
/// conversation or opening a new one. Runs on an I/O worker.
pub fn start(data_dir: &Path, name: &str, resume: bool) -> Outcome {
    let Some((url, key)) = configured(data_dir, None) else {
        return Outcome::Refused("Backbone is unavailable. No launch was requested.".into());
    };
    start_at(&url, &key, name, resume)
}

fn start_at(url: &str, key: &str, name: &str, resume: bool) -> Outcome {
    let refused = |message: &str| Outcome::Refused(message.into());
    if !valid_agent(name) || name.len() > 200 {
        return refused("This session is not a registered Backbone agent.");
    }
    // Lifecycle operations are authenticated local requests only, never anonymous TCP.
    let Some(socket) = url.strip_prefix("unix:").filter(|_| !key.is_empty()) else {
        return refused("Starting agents requires Backbone's authenticated local connection.");
    };
    let body = serde_json::json!({ "resume": resume, "wait": false });
    let reply = transport::post_local(
        socket,
        &format!("/api/agents/{name}/start"),
        key,
        serde_json::to_vec(&body).expect("launch JSON"),
        Duration::from_secs(30),
    );
    // Backbone reports a missing saved conversation like any other refusal.
    let not_resumed = "Backbone could not resume this agent. If it has no saved conversation, \
        Start new opens a new one; otherwise check its saved folder, runtime and model.";
    let reply = match reply {
        Ok(reply) => reply,
        Err(Some(401 | 403)) => return refused("Backbone refused access to start this agent."),
        Err(Some(400 | 404 | 409 | 422)) => {
            return refused(if resume {
                not_resumed
            } else {
                "Backbone refused the launch. Check the agent's saved folder, runtime and model."
            });
        }
        Err(_) => return Outcome::Unknown,
    };
    #[derive(Deserialize)]
    struct Reply {
        ok: bool,
        session: String,
        already_existed: bool,
        ready: String,
    }
    let Ok(reply) = serde_json::from_slice::<Reply>(&reply) else {
        return Outcome::Unknown;
    };
    if reply.session != name {
        return Outcome::Unknown;
    }
    if reply.already_existed {
        return Outcome::AlreadyRunning;
    }
    if !reply.ok {
        return refused(if resume {
            not_resumed
        } else {
            "The agent could not start. Check its saved configuration in Backbone."
        });
    }
    match reply.ready.as_str() {
        "not_waited" | "ready" | "waiting_for_human" | "timeout" => {
            Outcome::Accepted("Starting. Connecting once its terminal is ready.".into())
        }
        "inbox_only" => refused("This agent has no terminal to open."),
        _ => Outcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        thread,
    };

    fn request(socket: &mut UnixStream) -> (String, serde_json::Value) {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 8192);
        }
        let header = String::from_utf8(header).unwrap();
        let length = header
            .lines()
            .find_map(|line| {
                line.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|n| n.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        (header, serde_json::from_slice(&body).unwrap())
    }

    /// Serve one start request with `status` and `body`, and return what was sent.
    fn launch(resume: bool, status: &str, body: &'static str) -> (Outcome, serde_json::Value) {
        let (_directory, listener, url) = crate::transport::tests::local_listener();
        let status = status.to_owned();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let (header, sent) = request(&mut socket);
            assert!(header.starts_with("POST /api/agents/worker/start "));
            // A refusal lets the client close as soon as it reads the status.
            // Coalesce the small response instead of write!'s separate writes,
            // which can race that close between the headers and body.
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
            drop(socket);
            // Nothing is retried.
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            sent
        });
        let outcome = start_at(&url, "fixture-token", "worker", resume);
        (outcome, server.join().unwrap())
    }

    #[test]
    fn resume_asks_once_for_the_saved_conversation_and_never_starts_fresh_instead() {
        let (outcome, sent) = launch(
            true,
            "400 Bad Request",
            r#"{"detail":"no saved conversation"}"#,
        );
        assert_eq!(sent, serde_json::json!({"resume": true, "wait": false}));
        assert!(matches!(outcome, Outcome::Refused(_)));
    }

    #[test]
    fn start_new_reports_acceptance_and_an_already_running_agent() {
        let (outcome, sent) = launch(
            false,
            "200 OK",
            r#"{"ok":true,"session":"worker","already_existed":false,"ready":"not_waited"}"#,
        );
        assert_eq!(sent, serde_json::json!({"resume": false, "wait": false}));
        assert!(matches!(outcome, Outcome::Accepted(_)));
        let (outcome, _) = launch(
            false,
            "200 OK",
            r#"{"ok":true,"session":"worker","already_existed":true,"ready":"ready"}"#,
        );
        assert_eq!(outcome, Outcome::AlreadyRunning);
    }

    #[test]
    fn an_unreadable_reply_is_unknown_rather_than_retried() {
        let (outcome, _) = launch(false, "200 OK", "not json");
        assert_eq!(outcome, Outcome::Unknown);
    }
}
