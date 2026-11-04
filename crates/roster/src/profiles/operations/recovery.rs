//! Private consumer checkpoints, distinct from provider wire contracts.

use super::{Action, Error, Identity, Prepared, Status};
use serde::Deserialize;
use serde_json::{Value, json};

const CHECKPOINT_LIMIT: usize = 4 * crate::RESPONSE_LIMIT;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    checkpoint_version: u32,
    request: String,
    observation: Value,
}

impl Prepared {
    /// Encode exact request bytes and retained reservation/terminal progress.
    /// The caller must durably save it before submission and after observations.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        let observation = if self.identity.is_some() || self.terminal.is_some() {
            let status = self.terminal.as_ref().map_or(Status::Pending, |r| r.status);
            let identity = self.identity.as_ref();
            let error = self
                .terminal
                .as_ref()
                .and_then(|r| r.error_code.as_ref())
                .map(|code| json!({"code":code,"message":"Operation failed."}));
            json!({"schema_version":1,"request_id":self.request_id,"action":self.action.name(),
                "profile_id":identity.map(|i| &i.profile_id),
                "installation_id":identity.map(|i| &i.installation_id),
                "lifecycle_id":identity.map(|i| &i.lifecycle_id),
                "status":match status {Status::Pending=>"pending",Status::Succeeded=>"succeeded",Status::Failed=>"failed"},
                "error":error})
        } else {
            Value::Null
        };
        let request = std::str::from_utf8(&self.body).map_err(|_| Error::Invalid)?;
        let bytes = serde_json::to_vec(
            &json!({"checkpoint_version":1,"request":request,"observation":observation}),
        )
        .map_err(|_| Error::Invalid)?;
        if bytes.len() > CHECKPOINT_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }

    /// Validate a private checkpoint without discovery, ID allocation or I/O.
    /// Restored expectations are not authenticated content authority. Before
    /// executing, the caller must separately verify the original operator and
    /// registry context; restoring cannot authorize replay in a new context.
    pub fn restore(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > CHECKPOINT_LIMIT
            || !bytes
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'{')
        {
            return Err(Error::Invalid);
        }
        let checkpoint: Checkpoint = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if checkpoint.checkpoint_version != 1 {
            return Err(Error::Unsupported);
        }
        let mut prepared = Self::restore_request(checkpoint.request.as_bytes())?;
        if !checkpoint.observation.is_null() {
            if checkpoint.observation.get("schema_version") != Some(&json!(1)) {
                return Err(Error::Unsupported);
            }
            // Private synthesized observations may be slightly larger than a
            // provider receipt (for example after replacing diagnostic prose).
            // The complete checkpoint is already bounded; share semantic
            // validation without applying the provider response byte limit.
            let wire = serde_json::from_slice(
                &serde_json::to_vec(&checkpoint.observation).map_err(|_| Error::Invalid)?,
            )
            .map_err(|_| Error::Invalid)?;
            prepared.observe_wire(wire)?;
        }
        if prepared.checkpoint()? != bytes {
            return Err(Error::Invalid);
        }
        Ok(prepared)
    }

    fn restore_request(bytes: &[u8]) -> Result<Self, Error> {
        let body: Value = super::super::parse(bytes)?;
        let field = |name: &str| body.get(name).and_then(Value::as_str).ok_or(Error::Invalid);
        let id = field("request_id")?;
        if !super::valid_id(id) {
            return Err(Error::Invalid);
        }
        let prepared = match field("action")? {
            "create" => Self::create(id, field("display_name")?, field("baseline_version")?)?,
            "start" | "stop" => {
                let identity = Identity {
                    profile_id: field("profile_id")?.into(),
                    installation_id: field("expected_installation_id")?.into(),
                    lifecycle_id: field("expected_lifecycle_id")?.into(),
                };
                if [
                    &identity.profile_id,
                    &identity.installation_id,
                    &identity.lifecycle_id,
                ]
                .iter()
                .any(|s| !super::valid_id(s))
                {
                    return Err(Error::Invalid);
                }
                let action = if field("action")? == "start" {
                    Action::Start
                } else {
                    Action::Stop
                };
                let canonical = json!({"schema_version":1,"request_id":id,"action":action.name(),
                    "profile_id":identity.profile_id,"expected_installation_id":identity.installation_id,
                    "expected_lifecycle_id":identity.lifecycle_id});
                Self::prepare(id, action, Some(identity), canonical)?
            }
            _ => return Err(Error::Invalid),
        };
        // Only bytes produced by the original preparation API are accepted.
        // This also rejects duplicate keys, extra fields and altered encoding;
        // recovery never silently normalizes an original request for replay.
        if prepared.bytes() != bytes {
            return Err(Error::Invalid);
        }
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests;
