//! Private setup request storage; this is not a provider result or authority.

use super::{Action, Agent, CANCEL_LIMIT, Error, Prepared, REQUEST_LIMIT};
use serde::Deserialize;
use serde_json::json;

const CHECKPOINT_LIMIT: usize = 4 * REQUEST_LIMIT + 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    checkpoint_version: u32,
    first: String,
    cancel: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(rename = "schema_version")]
    _version: u32,
    request_id: String,
    action: Action,
    profile_id: String,
    agent: Agent,
    start_greeting: bool,
}

impl Prepared {
    /// Encode both immutable request bodies for private durable storage. Briefs
    /// and choices may be sensitive. This records no execution or readiness.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        let first = std::str::from_utf8(&self.first).map_err(|_| Error::Invalid)?;
        let cancel = std::str::from_utf8(&self.cancel).map_err(|_| Error::Invalid)?;
        let bytes =
            serde_json::to_vec(&json!({"checkpoint_version":1,"first":first,"cancel":cancel}))
                .map_err(|_| Error::Invalid)?;
        if bytes.len() > CHECKPOINT_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }

    /// Restore exact original setup/cancellation choices without discovery,
    /// new IDs or I/O. The caller must separately validate the original operator context before execution.
    /// This does not restore an interrupted stream, establish readiness or
    /// claim that cancellation succeeded.
    pub fn restore(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > CHECKPOINT_LIMIT || bytes.first() != Some(&b'{') {
            return Err(Error::Invalid);
        }
        let checkpoint: Checkpoint = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if checkpoint.checkpoint_version != 1 {
            return Err(Error::Invalid);
        }
        if checkpoint.first.len() > REQUEST_LIMIT || checkpoint.cancel.len() > CANCEL_LIMIT {
            return Err(Error::Invalid);
        }
        let request: Request =
            serde_json::from_str(&checkpoint.first).map_err(|_| Error::Invalid)?;
        if request.action != Action::FirstAgent {
            return Err(Error::Invalid);
        }
        let prepared = Self::prepare(
            &request.request_id,
            &request.profile_id,
            request.agent,
            request.start_greeting,
        )?;
        // Reconstruct only to validate: any normalization, unknown field,
        // duplicate key, or change between first/cancel bodies is rejected.
        if prepared.first != checkpoint.first.as_bytes()
            || prepared.cancel != checkpoint.cancel.as_bytes()
            || prepared.checkpoint()? != bytes
        {
            return Err(Error::Invalid);
        }
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests;
