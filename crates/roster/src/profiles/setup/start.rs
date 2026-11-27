//! A profile's first agent under the Rust service: `POST /api/agents/start`
//! on the profile's socket, the request `backbone agent start` sends.

use crate::profiles::{
    AddError, Connection, Error, added, runtime_catalog::Catalog, successful, valid_id,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

/// How long a start may take to answer: as long as the service's own CLI waits.
const STARTING: Duration = Duration::from_secs(135);

/// What to start: name, folder, CLI, model and an optional watched repository,
/// on this machine or another of the profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Start {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    /// Another machine of the profile, by its full ID; left out on this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    machine: Option<String>,
    /// The repository, as `owner/name`, that machine clones or already has.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// Left out where the machine's tools aren't known: the profile's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<String>,
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
            dir: Some(dir.into()),
            machine: None,
            repo: None,
            runtime: Some(runtime.into()),
            model: model.map(str::to_owned),
            watch: watch.map(str::to_owned).into_iter().collect(),
            wait: false,
        })
    }

    /// Start on another machine of the profile, `machine` by its full ID.
    /// `place` is as typed: `owner/name` to clone or use there, or a folder
    /// there (`/path` or `~/path`), never checked against this machine's
    /// disk. Without `runtime` the profile's default tool starts.
    pub fn on_machine(
        name: &str,
        machine: &str,
        place: &str,
        runtime: Option<&str>,
        model: Option<&str>,
        watch: Option<&str>,
    ) -> Result<Self, Error> {
        let (dir, repo) = match self::place(place).ok_or(Error::Invalid)? {
            Place::Repository(repo) => (None, Some(repo)),
            Place::Folder(dir) => (Some(dir), None),
        };
        if !crate::valid_agent(name)
            || machine.is_empty()
            || machine.len() > 256
            || !machine.bytes().all(|b| b.is_ascii_alphanumeric())
            || runtime.is_some_and(|runtime| !valid_id(runtime) || runtime == "shell")
            || model.is_some_and(|model| runtime.is_none() || !valid_id(model))
            || watch.is_some_and(|repo| !repository(repo))
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            name: name.into(),
            dir,
            machine: Some(machine.into()),
            repo,
            runtime: runtime.map(str::to_owned),
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

/// Where an agent works on another machine, as typed there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// `owner/name`: the machine clones it or uses the clean checkout there.
    Repository(String),
    /// A folder on that machine: `/path`, or `~/path` in its home.
    Folder(String),
}

/// What `text` names on another machine, if anything: checked as text only.
pub fn place(text: &str) -> Option<Place> {
    let text = text.trim();
    if repository(text) {
        return Some(Place::Repository(text.into()));
    }
    let folder =
        (text.starts_with('/') || text.starts_with("~/")) && !text.chars().any(char::is_control);
    folder.then(|| Place::Folder(text.into()))
}

/// A repository as GitHub names it: `owner/name`, neither of them `.` or `..`.
pub fn repository(text: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
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

/// A start the machine's checks refused, as one line: Backbone's message,
/// then each failed check's.
fn refused_start(response: &crate::transport::local::Response) -> Option<String> {
    if (200..300).contains(&response.status) {
        return None;
    }
    let refusal: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    let detail = &refusal["detail"];
    if detail["code"] != "start_refused" {
        return None;
    }
    let message = super::one_line(detail["message"].as_str()?);
    let checks: Vec<String> = detail["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|check| check["message"].as_str())
        .map(super::one_line)
        .filter(|line| !line.is_empty())
        .collect();
    Some(if checks.is_empty() {
        message
    } else {
        format!("{message} ({})", checks.join("; "))
    })
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
        if let Some(reason) = refused_start(&response) {
            return Err(AddError::Refused(reason));
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

/// Whether the service has no such route: one older than the route for
/// another machine's tools. Agent Backbone answers every unknown route with
/// this refusal (`Failure::no_such_route` in its daemon), while that route's
/// own 404 names the machine it doesn't know.
fn no_such_route(response: &crate::transport::local::Response) -> bool {
    response.status == 404
        && serde_json::from_slice::<serde_json::Value>(&response.body).is_ok_and(|refusal| {
            refusal["detail"]["code"] == "not_found"
                && refusal["detail"]["message"] == "the API has no such route"
        })
}

/// Backbone's reason when `machine` is reachable but runs a Backbone too old
/// to list its tools: a 503 `unavailable` that names that machine. Backbone
/// answers other 503s `unavailable` too, without naming the machine asked
/// about, so those stay refusals.
fn older_backbone(response: &crate::transport::local::Response, machine: &str) -> Option<String> {
    if response.status != 503 {
        return None;
    }
    let refusal: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    let detail = &refusal["detail"];
    (detail["code"] == "unavailable" && detail["machine"] == machine)
        .then(|| detail["message"].as_str().map(super::one_line))
        .flatten()
        .filter(|reason| !reason.is_empty())
}

/// What another machine's tools are, as its home lists them.
#[derive(Clone, Debug)]
pub enum Tools {
    /// The machine's tools.
    Listed {
        /// What can run there.
        catalog: Catalog,
        /// When the machine was checked, RFC 3339.
        checked_at: Option<String>,
    },
    /// Their list isn't served: by this service, or, with Backbone's reason,
    /// by that machine's older Backbone.
    NotListed(Option<String>),
}

impl Connection {
    /// The profile's machines, `GET /api/profile/machines`, in the service's
    /// order. Runs on an I/O worker.
    pub fn machines(&self) -> Result<Vec<crate::ProfileMachine>, Error> {
        let response = self.response("/api/profile/machines", None, crate::RESPONSE_LIMIT)?;
        successful(&response)?;
        crate::decode_machines(&response.body).ok_or(Error::Invalid)
    }

    /// The names of the profile's agents, `GET /api/agents`. Runs on an I/O
    /// worker.
    pub fn agent_names(&self) -> Result<Vec<String>, Error> {
        let response = self.response("/api/agents", None, crate::RESPONSE_LIMIT)?;
        successful(&response)?;
        let mut snapshot = crate::Snapshot::default();
        snapshot.update(&response.body);
        if snapshot.error.is_some() {
            return Err(Error::Invalid);
        }
        Ok(snapshot.states.into_keys().collect())
    }

    /// What can run on the profile's machine `machine`, by its full ID:
    /// `GET /api/profile/machines/{id}/runtimes`. A service without that
    /// route lists nothing; a refusal keeps its reason. Runs on an I/O worker.
    pub fn machine_tools(&self, machine: &str) -> Result<Tools, AddError> {
        if machine.is_empty()
            || machine.len() > 256
            || !machine.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(Error::Invalid.into());
        }
        let path = format!("/api/profile/machines/{machine}/runtimes");
        let response = self.response(&path, None, crate::RESPONSE_LIMIT)?;
        if no_such_route(&response) {
            return Ok(Tools::NotListed(None));
        }
        if let Some(reason) = older_backbone(&response, machine) {
            return Ok(Tools::NotListed(Some(reason)));
        }
        added(&response)?;
        // The catalog's own read tells a newer shape from a malformed one.
        let catalog = Catalog::served(&response.body, self.identity()).map_err(AddError::from)?;
        #[derive(Deserialize)]
        struct Checked {
            machine: String,
            #[serde(default)]
            checked_at: Option<String>,
        }
        let checked: Checked =
            serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?;
        // Another machine's tools are never shown as this one's.
        if checked.machine != machine {
            return Err(Error::Invalid.into());
        }
        Ok(Tools::Listed {
            catalog,
            checked_at: checked.checked_at.filter(|at| at.len() <= 64),
        })
    }
}
