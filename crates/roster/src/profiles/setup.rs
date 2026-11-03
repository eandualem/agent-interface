//! Immutable schema-2 first-agent setup and cancellation, without execution.
//! Every event is checked against the original tuple, action and request ID.

mod outcome;
mod recovery;

use super::{Error, Identity, Profile, valid_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

const REQUEST_LIMIT: usize = 32 * 1024;
const LINE_LIMIT: usize = 64 * 1024;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const EVENT_LIMIT: u64 = 1024;

/// Brief input selected before an immutable setup request is prepared.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Brief {
    /// Provider-known template ID; never a consumer filesystem path.
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

/// Prepared request bytes and expected origin. Neither submission, cancellation,
/// nor an uncertain outcome can allocate a replacement request or retarget it.
pub struct Prepared {
    request_id: String,
    identity: Identity,
    name: String,
    first: Vec<u8>,
    cancel: Vec<u8>,
}

impl Prepared {
    /// Capture one atomic ready registry record and explicit setup choices.
    /// Registry metadata supplies expectations; it does not authorize content.
    pub fn first_agent(
        request_id: &str,
        profile: &Profile,
        hooks: Vec<String>,
        agent: Agent,
        start_greeting: bool,
    ) -> Result<Self, Error> {
        let binding = profile.binding.as_ref().ok_or(Error::NotReady)?;
        let identity = Identity {
            profile_id: profile.item.profile_id.clone(),
            installation_id: binding.installation_id.clone(),
            lifecycle_id: binding.lifecycle_id.clone(),
        };
        Self::prepare(request_id, identity, hooks, agent, start_greeting)
    }

    fn prepare(
        request_id: &str,
        identity: Identity,
        hooks: Vec<String>,
        agent: Agent,
        start_greeting: bool,
    ) -> Result<Self, Error> {
        let brief_ok = match &agent.brief {
            Brief::Template { template } => valid_id(template),
            Brief::Text { text } => !text.trim().is_empty() && !text.contains('\0'),
        };
        if !valid_id(request_id)
            || [
                &identity.profile_id,
                &identity.installation_id,
                &identity.lifecycle_id,
            ]
            .iter()
            .any(|id| !valid_id(id))
            || !crate::valid_agent(&agent.name)
            || !agent.directory.is_absolute()
            || !agent
                .directory
                .to_str()
                .is_some_and(|s| !s.chars().any(char::is_control))
            || !valid_id(&agent.runtime)
            || agent.runtime == "shell"
            || agent.model.as_ref().is_some_and(|model| !valid_id(model))
            || !brief_ok
            || hooks.iter().any(|hook| !valid_id(hook))
            || hooks.iter().collect::<std::collections::HashSet<_>>().len() != hooks.len()
        {
            return Err(Error::Invalid);
        }
        let name = agent.name.clone();
        let mut body = json!({"schema_version":2,"request_id":request_id,"action":"first_agent","profile_id":identity.profile_id,"expected_installation_id":identity.installation_id,"expected_lifecycle_id":identity.lifecycle_id,"hooks":hooks,"agent":agent,"start_greeting":start_greeting});
        let first = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        body["action"] = json!("cancel_first_agent");
        let cancel = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        if first.len() > REQUEST_LIMIT || cancel.len() > REQUEST_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(Self {
            request_id: request_id.into(),
            identity,
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
    /// Retained full expected origin; no connection authority is implied.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    /// Original durable operation ID.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// A fresh bounded output stream for one submission or exact retry.
    pub fn stream(&self, action: Action) -> Stream {
        Stream {
            request_id: self.request_id.clone(),
            identity: self.identity.clone(),
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

/// Validated progress or terminal outcome; provider prose is never retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Full captured origin, retained for deferred consumers.
    pub identity: Identity,
    /// Original operation ID, retained for deferred consumers.
    pub request_id: String,
    /// Original setup or cancellation action.
    pub action: Action,
    /// Monotonic sequence within this invocation.
    pub sequence: u64,
    /// Step name, absent on final results.
    pub step: Option<String>,
    /// Bounded protocol status.
    pub status: String,
    /// Bounded result code, absent on progress.
    pub code: Option<String>,
    /// Retry policy supplied by a validated terminal result.
    pub retry: Option<String>,
    /// Provider-confirmed safe cancellation of precisely the original request.
    pub cancelled: bool,
    /// Agent name only when readiness was explicitly confirmed for the requested agent.
    pub ready_agent: Option<String>,
    /// Original provider launch identifier, retained only with confirmed readiness.
    pub launch_id: Option<String>,
}

#[derive(Deserialize)]
struct Wire {
    schema_version: u32,
    request_id: String,
    #[serde(flatten)]
    identity: Identity,
    action: Action,
    seq: u64,
    #[serde(rename = "type")]
    kind: String,
    status: String,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    retry: Option<String>,
    #[serde(default)]
    completed_steps: Option<Vec<String>>,
    #[serde(default)]
    data: Option<Value>,
}

/// A bounded JSON-lines decoder owned by one exact request and action.
/// Any malformed, foreign or contradictory event poisons this invocation.
pub struct Stream {
    request_id: String,
    identity: Identity,
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
        let wire: Wire = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if wire.schema_version != 2 {
            return Err(Error::Unsupported);
        }
        if wire.identity != self.identity {
            return Err(Error::BindingMismatch);
        }
        if wire.request_id != self.request_id
            || wire.action != self.action
            || wire.seq <= self.sequence
            || wire.seq > EVENT_LIMIT
            || self.result.is_some()
        {
            return Err(Error::Invalid);
        }
        let result = wire.kind == "result";
        let mut event = Event {
            identity: self.identity.clone(),
            request_id: self.request_id.clone(),
            action: wire.action,
            sequence: wire.seq,
            step: None,
            status: wire.status.clone(),
            code: None,
            retry: None,
            cancelled: false,
            ready_agent: None,
            launch_id: None,
        };
        if wire.kind == "step"
            && matches!(
                wire.status.as_str(),
                "running" | "completed" | "skipped" | "failed"
            )
        {
            let step = wire
                .step
                .filter(|step| valid_id(step))
                .ok_or(Error::Invalid)?;
            event.step = Some(step);
        } else if result
            && matches!(
                wire.status.as_str(),
                "completed" | "action_required" | "failed"
            )
        {
            let code = wire
                .code
                .filter(|code| valid_id(code) && code.len() <= 128)
                .ok_or(Error::Invalid)?;
            let retry = wire
                .retry
                .filter(|retry| matches!(retry.as_str(), "same_request" | "new_request" | "none"))
                .ok_or(Error::Invalid)?;
            let completed = wire.completed_steps.ok_or(Error::Invalid)?;
            if completed.iter().any(|step| !valid_id(step)) {
                return Err(Error::Invalid);
            }
            let data = wire.data.filter(Value::is_object).ok_or(Error::Invalid)?;
            event.cancelled = self.action == Action::CancelFirstAgent
                && wire.status == "completed"
                && code == "request_cancelled"
                && retry == "new_request"
                && completed.is_empty()
                && data.get("cancelled_request_id").and_then(Value::as_str)
                    == Some(&self.request_id)
                && data.get("can_start_new_request").and_then(Value::as_bool) == Some(true);
            if self.action == Action::CancelFirstAgent
                && wire.status == "completed"
                && !event.cancelled
            {
                return Err(Error::Invalid);
            }
            if self.action == Action::FirstAgent && wire.status == "completed" {
                if code != "agent_ready"
                    || retry != "none"
                    || data.get("agent_name").and_then(Value::as_str) != Some(&self.name)
                    || data.get("ready").and_then(Value::as_str) != Some("ready")
                    || !data
                        .get("launch_id")
                        .and_then(Value::as_str)
                        .is_some_and(valid_id)
                {
                    return Err(Error::Invalid);
                }
                event.ready_agent = Some(self.name.clone());
                event.launch_id = data
                    .get("launch_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
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
