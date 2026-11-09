//! Private normalized terminal observations, never an interrupted stream.

use super::{Action, Error, Event, LINE_LIMIT, Prepared};
use serde::{Deserialize, Serialize};
use serde_json::json;

const LIMIT: usize = 4 * LINE_LIMIT;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    format: u8,
    result: String,
}

impl Prepared {
    /// Store a validated terminal observation without provider prose or step
    /// details. The executor must first verify the matching process exit; this
    /// model validates semantics only and grants no connection or account access.
    pub fn outcome_checkpoint(&self, event: &Event) -> Result<Vec<u8>, Error> {
        if event.step.is_some() {
            return Err(Error::Invalid);
        }
        let data = if event.cancelled {
            json!({"cancelled_request_id":event.request_id,"can_start_new_request":true})
        } else if let Some(name) = &event.ready_agent {
            json!({"agent_name":name,"ready":"ready","launch_id":event.launch_id})
        } else {
            json!({})
        };
        let result = serde_json::to_string(&json!({
            "schema_version":2,"type":"result",
            "profile_id":event.identity.profile_id,
            "installation_id":event.identity.installation_id,
            "lifecycle_id":event.identity.lifecycle_id,
            "request_id":event.request_id,"action":match event.action {
                Action::FirstAgent=>"first_agent",Action::CancelFirstAgent=>"cancel_first_agent"
            },
            "seq":event.sequence,"status":event.status,"code":event.code,
            "retry":event.retry,"completed_steps":[],"data":data,
        }))
        .map_err(|_| Error::Invalid)?;
        let validated = self.decode_outcome(event.action, &result)?;
        if &validated != event {
            return Err(Error::Invalid);
        }
        let bytes =
            serde_json::to_vec(&Checkpoint { format: 1, result }).map_err(|_| Error::Invalid)?;
        if bytes.len() > LIMIT {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }

    /// Restore only a canonical terminal observation for this exact request and
    /// expected action. It does not restore a stream or infer process-exit proof.
    /// Caller-context and journal validation remain separate requirements.
    pub fn restore_outcome(&self, action: Action, bytes: &[u8]) -> Result<Event, Error> {
        if bytes.len() > LIMIT || bytes.first() != Some(&b'{') {
            return Err(Error::Invalid);
        }
        let checkpoint: Checkpoint = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if checkpoint.format != 1 {
            return Err(Error::Unsupported);
        }
        let event = self.decode_outcome(action, &checkpoint.result)?;
        if self.outcome_checkpoint(&event)? != bytes {
            return Err(Error::Invalid);
        }
        Ok(event)
    }

    fn decode_outcome(&self, action: Action, result: &str) -> Result<Event, Error> {
        if result.len() > LINE_LIMIT {
            return Err(Error::Invalid);
        }
        // A terminal's original sequence need not start at one: preceding
        // progress is intentionally not retained as durable authority.
        let mut stream = self.stream(action);
        stream.push(result.as_bytes())?;
        let event = stream.finish()?;
        if event.step.is_some() {
            return Err(Error::Invalid);
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests;
