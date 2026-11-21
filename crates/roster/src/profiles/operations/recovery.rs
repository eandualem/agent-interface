//! Private consumer checkpoints, distinct from provider wire contracts.

use super::{Action, Error, Identity, Prepared, Receipt, Status};
use serde::Deserialize;
use serde_json::{Value, json};

const CHECKPOINT_LIMIT: usize = 4 * crate::RESPONSE_LIMIT;

/// The checkpoint's own version. A record saved under another one is
/// `Invalid`, and is forgotten rather than replayed.
const VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    checkpoint_version: u32,
    request: String,
    observation: Option<Observation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    identity: Option<CreatedProfile>,
    status: Status,
    error_code: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatedProfile {
    profile_id: String,
    machine_id: String,
}

impl Prepared {
    /// Encode exact request bytes and the retained result, if any. The caller
    /// must durably save it before submission and after an observation.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        let observation = self.terminal.as_ref().map(|receipt| {
            json!({
                "identity": receipt.identity.as_ref().map(|identity| json!({
                    "profile_id": identity.profile_id,
                    "machine_id": identity.installation_id,
                })),
                "status": match receipt.status {
                    Status::Succeeded => "succeeded",
                    Status::Failed => "failed",
                },
                "error_code": receipt.error_code,
            })
        });
        let request = std::str::from_utf8(&self.body).map_err(|_| Error::Invalid)?;
        let bytes = serde_json::to_vec(
            &json!({"checkpoint_version":VERSION,"request":request,"observation":observation}),
        )
        .map_err(|_| Error::Invalid)?;
        if bytes.len() > CHECKPOINT_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }

    /// Validate a private checkpoint without discovery, ID allocation or I/O.
    /// Restored expectations are not authenticated content authority. Before
    /// executing, the caller must separately verify the original operator
    /// context; restoring cannot authorize replay in a new context.
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
        if checkpoint.checkpoint_version != VERSION {
            return Err(Error::Invalid);
        }
        let mut prepared = Self::restore_request(checkpoint.request.as_bytes())?;
        if let Some(observation) = checkpoint.observation {
            let valid = match (&observation.identity, &observation.error_code) {
                (Some(identity), None) => {
                    observation.status == Status::Succeeded
                        && super::valid_id(&identity.profile_id)
                        && super::valid_id(&identity.machine_id)
                }
                (None, Some(code)) => {
                    observation.status == Status::Failed
                        && super::valid_id(code)
                        && code.len() <= 128
                }
                _ => false,
            };
            if !valid {
                return Err(Error::Invalid);
            }
            prepared.record(Receipt {
                request_id: prepared.request_id.clone(),
                action: prepared.action,
                identity: observation.identity.map(|profile| Identity {
                    profile_id: profile.profile_id,
                    installation_id: profile.machine_id.clone(),
                    lifecycle_id: profile.machine_id,
                }),
                status: observation.status,
                error_code: observation.error_code,
            })?;
        }
        if prepared.checkpoint()? != bytes {
            return Err(Error::Invalid);
        }
        Ok(prepared)
    }

    fn restore_request(bytes: &[u8]) -> Result<Self, Error> {
        let body: Value = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        let field = |name: &str| body.get(name).and_then(Value::as_str).ok_or(Error::Invalid);
        let id = field("request_id")?;
        if field("action")? != Action::Create.name() {
            return Err(Error::Invalid);
        }
        let name = field("display_name")?;
        let prepared = match body.get("git_identity").map(Value::as_str) {
            None => Self::create(id, name)?,
            Some(Some("copy")) => Self::create_copying_git_identity(id, name)?,
            Some(_) => return Err(Error::Invalid),
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
