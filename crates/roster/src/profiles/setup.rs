//! Immutable schema-3 first-agent setup and cancellation, without execution.
//! Every event is checked against the request ID, and the result also
//! against the action: events carry no other identity.

mod outcome;
mod recovery;
pub mod start;

use super::{Error, Profile, valid_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The setup schema the Rust service takes; it refuses 1 and 2.
const SCHEMA_VERSION: u32 = 3;
const REQUEST_LIMIT: usize = 32_768;
/// A request of the most bytes, with its action changed to the longer one.
const CANCEL_LIMIT: usize = 32_775;
/// The most bytes one line of setup's output may have.
pub const LINE_LIMIT: usize = 64 * 1024;
/// The most bytes setup's output may have.
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
/// The highest sequence number an event of one run may have.
pub const EVENT_LIMIT: u64 = 1024;
/// The most characters a written brief may have.
const BRIEF_LIMIT: usize = 16_000;
/// The most characters an agent's folder may have.
const DIRECTORY_LIMIT: usize = 4096;
/// The only template the core starter has.
const TEMPLATE: &str = "orchestrator-v1";
/// What setup counts as white space when it asks whether a brief is blank:
/// Unicode's White_Space and ECMAScript's together.
const WHITE_SPACE: &[char] = &[
    '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{85}', '\u{a0}', '\u{1680}', '\u{2000}',
    '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}',
    '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
];

/// Brief input selected before an immutable setup request is prepared.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Brief {
    /// The starter's brief, `orchestrator-v1`; never a consumer filesystem path.
    Template {
        /// Stable template identifier supplied by the provider.
        template: String,
    },
    /// Explicit instructions supplied by the user.
    Text {
        /// User-authored instructions sent with this request.
        text: String,
    },
}

/// Agent choices; the provider owns validation and effects in its own context.
#[derive(Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Requested agent name.
    pub name: String,
    /// Explicit project directory, not a profile data directory.
    pub directory: PathBuf,
    /// Whether the provider may create that project directory.
    pub create_directory: bool,
    /// Runtime ID from the profile-specific catalog.
    pub runtime: String,
    /// Optional model ID from that catalog.
    pub model: Option<String>,
    /// Explicit brief input.
    pub brief: Brief,
}

/// The two actions share the original request ID and all original choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Prepare and start the first agent.
    FirstAgent,
    /// Cancel only that original first-agent request, if the provider says safe.
    CancelFirstAgent,
}

impl Action {
    /// The action as requests and results name it.
    fn name(self) -> &'static str {
        match self {
            Action::FirstAgent => "first_agent",
            Action::CancelFirstAgent => "cancel_first_agent",
        }
    }
}

/// Prepared request bytes and the profile they are for. Neither submission,
/// cancellation, nor an uncertain outcome can allocate a replacement request
/// or retarget it.
pub struct Prepared {
    request_id: String,
    profile_id: String,
    name: String,
    first: Vec<u8>,
    cancel: Vec<u8>,
}

impl Prepared {
    /// Capture a profile the Rust service serves and explicit setup choices.
    /// Only that service takes setup schema 3, so any other profile is
    /// `Unsupported`.
    pub fn first_agent(
        request_id: &str,
        profile: &Profile,
        agent: Agent,
        start_greeting: bool,
    ) -> Result<Self, Error> {
        if !profile.served() {
            return Err(Error::Unsupported);
        }
        Self::prepare(request_id, &profile.item.profile_id, agent, start_greeting)
    }

    /// Check every field as the contract words it, so the service never has
    /// to refuse a request the app could have refused.
    fn prepare(
        request_id: &str,
        profile_id: &str,
        agent: Agent,
        start_greeting: bool,
    ) -> Result<Self, Error> {
        if !request_id_form(request_id) || !hex(profile_id, 12) || !choices(&agent) {
            return Err(Error::Invalid);
        }
        let name = agent.name.clone();
        let mut body = json!({"schema_version":SCHEMA_VERSION,"request_id":request_id,"action":"first_agent","profile_id":profile_id,"agent":agent,"start_greeting":start_greeting});
        let first = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        body["action"] = json!("cancel_first_agent");
        let cancel = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        if first.len() > REQUEST_LIMIT || cancel.len() > CANCEL_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(Self {
            request_id: request_id.into(),
            profile_id: profile_id.into(),
            name,
            first,
            cancel,
        })
    }
    /// Exact original or cancellation body, including the same request ID.
    pub fn bytes(&self, action: Action) -> &[u8] {
        match action {
            Action::FirstAgent => &self.first,
            Action::CancelFirstAgent => &self.cancel,
        }
    }
    /// The profile the request is for; no connection authority is implied.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
    /// The agent this request adds.
    pub fn agent_name(&self) -> &str {
        &self.name
    }
    /// The runtime this request's agent uses.
    pub fn runtime(&self) -> Option<String> {
        let body: Value = serde_json::from_slice(&self.first).ok()?;
        body["agent"]["runtime"].as_str().map(str::to_owned)
    }
    /// Original durable operation ID.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// A fresh bounded output stream for one submission or exact retry.
    pub fn stream(&self, action: Action) -> Stream {
        Stream {
            request_id: self.request_id.clone(),
            profile_id: self.profile_id.clone(),
            name: self.name.clone(),
            action,
            sequence: 0,
            total: 0,
            pending: Vec::new(),
            result: None,
            failed: false,
        }
    }
}

/// A UUIDv7 in its simple form, as the contract's pattern says.
fn request_id_form(id: &str) -> bool {
    let bytes = id.as_bytes();
    hex(id, 32) && bytes[12] == b'7' && matches!(bytes[16], b'8' | b'9' | b'a' | b'b')
}

/// `length` lowercase hexadecimal digits.
fn hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether every choice is one the contract takes.
fn choices(agent: &Agent) -> bool {
    let directory = agent.directory.to_str().is_some_and(|directory| {
        directory.starts_with('/')
            && directory.chars().count() <= DIRECTORY_LIMIT
            && !directory.split('/').any(|part| part == "..")
            && !directory.chars().any(char::is_control)
    });
    let mut runtime = agent.runtime.chars();
    let runtime = agent.runtime.len() <= 40
        && runtime.next().is_some_and(|c| c.is_ascii_lowercase())
        && runtime.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && agent.runtime != "shell";
    let model = agent.model.as_ref().is_none_or(|model| {
        model.len() <= 500
            && model.starts_with(|c: char| c.is_ascii_alphanumeric())
            && model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
    });
    let brief = match &agent.brief {
        Brief::Template { template } => template == TEMPLATE,
        Brief::Text { text } => {
            !text.chars().all(|c| WHITE_SPACE.contains(&c))
                && text.chars().count() <= BRIEF_LIMIT
                && !text.contains('\0')
        }
    };
    crate::valid_agent(&agent.name)
        && agent.name.len() <= 128
        && directory
        && runtime
        && model
        && brief
}

/// The schema a saved body says it was written for, read before the body.
#[derive(Deserialize)]
struct Version {
    schema_version: u32,
}

/// Why a body saved under schema `version` is refused: `Unsupported` for
/// the earlier schemas 1 and 2, which the service no longer takes; any
/// other is not one this app wrote, and `Invalid`.
fn refused(version: u32) -> Error {
    if matches!(version, 1 | 2) {
        Error::Unsupported
    } else {
        Error::Invalid
    }
}

/// A value setup's events may carry that the app keeps as it came: a step's
/// status, a result's status, code and retry, and a launch, bounded.
pub fn token(text: &str) -> bool {
    valid_id(text) && text.len() <= 128
}

/// Validated progress or terminal outcome; provider prose is never retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The profile the request is for, retained for deferred consumers.
    pub profile_id: String,
    /// Original operation ID, retained for deferred consumers.
    pub request_id: String,
    /// Original setup or cancellation action.
    pub action: Action,
    /// Monotonic sequence within this invocation.
    pub sequence: u64,
    /// Step name, absent on final results.
    pub step: Option<String>,
    /// Status as it came, including one this app doesn't know.
    pub status: String,
    /// Bounded result code, absent on progress.
    pub code: Option<String>,
    /// Retry policy as the result gave it, including one this app doesn't know.
    pub retry: Option<String>,
    /// Provider-confirmed safe cancellation of precisely the original request.
    pub cancelled: bool,
    /// Agent name only when readiness was explicitly confirmed for the requested agent.
    pub ready_agent: Option<String>,
    /// Provider launch identifier, when readiness was confirmed with one.
    pub launch_id: Option<String>,
}

/// What every line of the setup command's output has. The fields its kind
/// defines are read after it, and only those: anything else, a field of the
/// other kind among them, is ignored.
#[derive(Deserialize)]
struct Wire {
    schema_version: u32,
    request_id: String,
    seq: u64,
    #[serde(rename = "type")]
    kind: String,
}

/// What a step event defines. It carries no action.
#[derive(Deserialize)]
struct StepWire {
    step: String,
    status: String,
}

/// What a result event defines.
#[derive(Deserialize)]
struct ResultWire {
    action: String,
    status: String,
    code: String,
    /// Read only to check its type: provider prose is never kept.
    #[serde(rename = "message")]
    _message: String,
    retry: String,
    completed_steps: Vec<String>,
    data: Value,
}

/// A bounded JSON-lines decoder owned by one exact request and action.
/// Any malformed, foreign or contradictory event poisons this invocation.
pub struct Stream {
    request_id: String,
    profile_id: String,
    name: String,
    action: Action,
    sequence: u64,
    total: usize,
    pending: Vec<u8>,
    result: Option<Event>,
    failed: bool,
}

impl Stream {
    /// Decode zero or more complete lines, preserving partial reads.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>, Error> {
        let result = self.push_inner(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, bytes: &[u8]) -> Result<Vec<Event>, Error> {
        if self.failed {
            return Err(Error::Invalid);
        }
        self.total = self.total.checked_add(bytes.len()).ok_or(Error::Invalid)?;
        if self.total > OUTPUT_LIMIT {
            return Err(Error::Invalid);
        }
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            if end > LINE_LIMIT {
                return Err(Error::Invalid);
            }
            let line: Vec<_> = self.pending.drain(..=end).collect();
            events.push(self.line(&line)?);
        }
        if self.pending.len() > LINE_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(events)
    }
    fn line(&mut self, bytes: &[u8]) -> Result<Event, Error> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        let wire = Wire::deserialize(&value).map_err(|_| Error::Invalid)?;
        if wire.schema_version != SCHEMA_VERSION {
            return Err(Error::Unsupported);
        }
        if wire.request_id != self.request_id
            || wire.seq <= self.sequence
            || wire.seq > EVENT_LIMIT
            || self.result.is_some()
        {
            return Err(Error::Invalid);
        }
        let result = wire.kind == "result";
        let mut event = Event {
            profile_id: self.profile_id.clone(),
            request_id: self.request_id.clone(),
            action: self.action,
            sequence: wire.seq,
            step: None,
            status: String::new(),
            code: None,
            retry: None,
            cancelled: false,
            ready_agent: None,
            launch_id: None,
        };
        if wire.kind == "step" {
            // A status this app doesn't know is kept as it came.
            let step = StepWire::deserialize(&value).map_err(|_| Error::Invalid)?;
            if !valid_id(&step.step) || !token(&step.status) {
                return Err(Error::Invalid);
            }
            event.step = Some(step.step);
            event.status = step.status;
        } else if result {
            let wire = ResultWire::deserialize(&value).map_err(|_| Error::Invalid)?;
            let (code, retry) = (wire.code, wire.retry);
            if wire.action != self.action.name()
                || !token(&wire.status)
                || !token(&code)
                || !token(&retry)
                || wire.completed_steps.iter().any(|step| !valid_id(step))
                || !wire.data.is_object()
            {
                return Err(Error::Invalid);
            }
            let data = wire.data;
            event.status = wire.status;
            let completed = event.status == "completed";
            if self.action == Action::CancelFirstAgent && completed && code == "request_cancelled" {
                // Only this answer confirms the cancellation; naming another
                // request contradicts it.
                if data.get("cancelled_request_id").and_then(Value::as_str)
                    != Some(&self.request_id)
                    || data.get("can_start_new_request").and_then(Value::as_bool) != Some(true)
                {
                    return Err(Error::Invalid);
                }
                event.cancelled = true;
            }
            if self.action == Action::FirstAgent && completed && code == "agent_ready" {
                // Ready only for this agent, whose CLI still ran after its
                // launch; a launch is not always named.
                let launch_id = match data.get("launch_id") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(launch)) if token(launch) => Some(launch.clone()),
                    Some(_) => return Err(Error::Invalid),
                };
                if data.get("agent_name").and_then(Value::as_str) != Some(&self.name)
                    || data.get("ready").and_then(Value::as_str) != Some("ready")
                {
                    return Err(Error::Invalid);
                }
                event.ready_agent = Some(self.name.clone());
                event.launch_id = launch_id;
            }
            event.code = Some(code);
            event.retry = Some(retry);
        } else {
            return Err(Error::Invalid);
        }
        self.sequence = wire.seq;
        if result {
            self.result = Some(event.clone());
        }
        Ok(event)
    }
    /// Require a complete, noncontradictory final outcome. Process exit status
    /// remains the executor's responsibility; EOF alone never implies success.
    pub fn finish(mut self) -> Result<Event, Error> {
        if self.failed {
            return Err(Error::Invalid);
        }
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }
        self.result.ok_or(Error::Invalid)
    }
}

#[cfg(test)]
mod tests;
