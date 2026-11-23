//! A profile's first agent under the Rust service: `POST /api/agents/start`
//! on the profile's socket, the request `backbone agent start` sends.

use crate::profiles::{AddError, Connection, Error, added, valid_id};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

/// How long a start may take to answer: as long as the service's own CLI waits.
const STARTING: Duration = Duration::from_secs(135);

/// What to start: name, folder, CLI, model and an optional watched repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Start {
    name: String,
    dir: String,
    runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    watch: Vec<String>,
    /// The agent's own pane shows it starting, so the request doesn't wait.
    wait: bool,
}

impl Start {
    /// Check the choices. `dir` is absolute; `watch` is `owner/name`.
    pub fn new(
        name: &str,
        dir: &Path,
        runtime: &str,
        model: Option<&str>,
        watch: Option<&str>,
    ) -> Result<Self, Error> {
        let dir = dir
            .is_absolute()
            .then(|| dir.to_str())
            .flatten()
            .filter(|dir| !dir.chars().any(char::is_control))
            .ok_or(Error::Invalid)?;
        if !crate::valid_agent(name)
            || !valid_id(runtime)
            || runtime == "shell"
            || model.is_some_and(|model| !valid_id(model))
            || watch.is_some_and(|repo| !repository(repo))
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            name: name.into(),
            dir: dir.into(),
            runtime: runtime.into(),
            model: model.map(str::to_owned),
            watch: watch.map(str::to_owned).into_iter().collect(),
            wait: false,
        })
    }

    /// The agent this asks for.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A repository as GitHub names it: `owner/name`.
pub fn repository(text: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    text.split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// The service's answer to a change it sent but whose result it doesn't know.
fn outcome_unknown(response: &crate::transport::local::Response) -> bool {
    serde_json::from_slice::<serde_json::Value>(&response.body)
        .is_ok_and(|refusal| refusal["detail"]["code"] == "outcome_unknown")
}

/// Whether the agent reached its prompt; a start that doesn't wait says
/// `NotWaited`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    /// The start did not wait, or nothing started.
    NotWaited,
    /// The agent is at its prompt.
    Ready,
    /// The CLI asks the person something first.
    WaitingForHuman,
    /// The session ended before the CLI reached its prompt.
    Exited,
    /// The CLI did not reach its prompt in time; it may still.
    Timeout,
    /// Registered and never launched.
    InboxOnly,
    /// A state this app does not know.
    #[serde(other)]
    Unknown,
}

/// What the service answered.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Started {
    /// Whether the agent has a session that did not end while it started.
    pub ok: bool,
    /// The agent's name, which its pane is opened by.
    pub name: String,
    /// Whether it reached its prompt.
    pub ready: Readiness,
    /// The service's optional explanation for a refused launch.
    pub reason: Option<String>,
    /// What its machine saw, for a person to read.
    pub evidence: Vec<String>,
}

impl Connection {
    /// Start `start` under `request_id`, 32 lowercase hexadecimal digits: a
    /// repeat with the same ID after a lost reply gets the first result. A
    /// refusal keeps the service's message. Runs on an I/O worker.
    pub fn start_first_agent(&self, start: &Start, request_id: &str) -> Result<Started, AddError> {
        let spelled = request_id.len() == 32
            && request_id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !spelled {
            return Err(Error::Invalid.into());
        }
        let body = serde_json::to_vec(start).map_err(|_| Error::Invalid)?;
        let response = self.write_within(
            "POST",
            "/api/agents/start",
            Some(body),
            Some(request_id),
            STARTING,
        )?;
        // The service sent the start on but doesn't know what came of it. As
        // after a lost reply, only the same request ID may ask again.
        if outcome_unknown(&response) {
            return Err(Error::Unavailable.into());
        }
        added(&response)?;
        let started: Started =
            serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?;
        if !crate::valid_agent(&started.name) {
            return Err(Error::Invalid.into());
        }
        Ok(started)
    }
}
