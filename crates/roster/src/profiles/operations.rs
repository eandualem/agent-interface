//! Immutable local lifecycle requests and origin-checked receipts. This module
//! invokes no command; it excludes adoption and first-agent setup contracts.

use super::{Error, Identity, Profile, valid_id};
use serde::Deserialize;
use serde_json::{json, value::RawValue};

/// Existing r2 local lifecycle actions. No action is inferred from a receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Reserve and provision a fresh isolated profile.
    Create,
    /// Start the exact registered instance.
    Start,
    /// Stop the exact registered instance while preserving data.
    Stop,
}
impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }
}

/// Durable progress of one original request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The original operation has not finished.
    Pending,
    /// The provider recorded successful completion.
    Succeeded,
    /// The provider recorded a failure without granting content authority.
    Failed,
}

/// A validated observation of one prepared request; it grants no content access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The original idempotency key.
    pub request_id: String,
    /// The original action.
    pub action: Action,
    /// Full reserved identity; absent only for create before reservation.
    pub identity: Option<Identity>,
    /// Pending or a durable terminal result.
    pub status: Status,
    /// A bounded failure category; raw provider messages are not retained.
    pub error_code: Option<String>,
}

#[derive(Deserialize)]
struct WireReceipt {
    request_id: String,
    action: Action,
    profile_id: Box<RawValue>,
    installation_id: Box<RawValue>,
    lifecycle_id: Box<RawValue>,
    status: Status,
    error: Box<RawValue>,
}
#[derive(Deserialize)]
struct Failure {
    code: String,
    message: String,
}

/// Original request bytes, target and progress. Selection changes cannot rewrite
/// them. Create pins its first reserved identity; terminal results cannot regress.
/// Keep this value across uncertain delivery and look up its original request ID.
pub struct Prepared {
    request_id: String,
    action: Action,
    body: Vec<u8>,
    identity: Option<Identity>,
    terminal: Option<Receipt>,
}

impl Prepared {
    /// Capture a new-profile request without allocating identities or copying
    /// any existing profile's paths, environment, credentials or mutable data.
    pub fn create(
        request_id: &str,
        display_name: &str,
        baseline_version: &str,
    ) -> Result<Self, Error> {
        if !valid_id(request_id)
            || !valid_id(baseline_version)
            || display_name.trim().is_empty()
            || display_name.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        Self::prepare(
            request_id,
            Action::Create,
            None,
            json!({
                "schema_version":1,"request_id":request_id,"action":"create",
                "display_name":display_name,"baseline_version":baseline_version,
            }),
        )
    }

    /// Capture start's expected tuple from an atomic ready registry record.
    /// Registry metadata is an expectation, not authenticated content authority.
    pub fn start(request_id: &str, profile: &Profile) -> Result<Self, Error> {
        Self::for_profile(request_id, Action::Start, profile)
    }

    /// Capture stop's expected tuple from an atomic ready registry record.
    pub fn stop(request_id: &str, profile: &Profile) -> Result<Self, Error> {
        Self::for_profile(request_id, Action::Stop, profile)
    }

    fn for_profile(request_id: &str, action: Action, profile: &Profile) -> Result<Self, Error> {
        if !valid_id(request_id) {
            return Err(Error::Invalid);
        }
        let binding = profile.binding.as_ref().ok_or(Error::NotReady)?;
        let identity = Identity {
            profile_id: profile.item.profile_id.clone(),
            installation_id: binding.installation_id.clone(),
            lifecycle_id: binding.lifecycle_id.clone(),
        };
        let body = json!({"schema_version":1,"request_id":request_id,"action":action.name(),
            "profile_id":identity.profile_id,"expected_installation_id":identity.installation_id,
            "expected_lifecycle_id":identity.lifecycle_id});
        Self::prepare(request_id, action, Some(identity), body)
    }

    fn prepare(
        request_id: &str,
        action: Action,
        identity: Option<Identity>,
        body: serde_json::Value,
    ) -> Result<Self, Error> {
        let body = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        if body.len() > crate::RESPONSE_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(Self {
            request_id: request_id.into(),
            action,
            body,
            identity,
            terminal: None,
        })
    }

    /// The exact request body to retain for submission or an explicit retry.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }
    /// The original durable lookup key; a timeout must not allocate a new one.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// Expected or reserved origin, never a verified connection.
    pub fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// Validate a lookup or submission receipt before changing retained progress.
    /// Partial/foreign tuples, wrong actions/IDs and terminal regressions fail
    /// without changing the original request or previously observed reservation.
    /// Success still requires the normal identity handshake before content use.
    pub fn observe(&mut self, bytes: &[u8]) -> Result<Receipt, Error> {
        let wire: WireReceipt = super::parse(bytes)?;
        if wire.request_id != self.request_id || wire.action != self.action {
            return Err(Error::Invalid);
        }
        let decode = |raw: &RawValue| {
            serde_json::from_str::<Option<String>>(raw.get()).map_err(|_| Error::Invalid)
        };
        let identity = match (
            decode(&wire.profile_id)?,
            decode(&wire.installation_id)?,
            decode(&wire.lifecycle_id)?,
        ) {
            (Some(profile_id), Some(installation_id), Some(lifecycle_id))
                if valid_id(&profile_id)
                    && valid_id(&installation_id)
                    && valid_id(&lifecycle_id) =>
            {
                Some(Identity {
                    profile_id,
                    installation_id,
                    lifecycle_id,
                })
            }
            (None, None, None)
                if self.action == Action::Create && wire.status != Status::Succeeded =>
            {
                None
            }
            _ => return Err(Error::Invalid),
        };
        if self
            .identity
            .as_ref()
            .is_some_and(|expected| Some(expected) != identity.as_ref())
        {
            return Err(Error::BindingMismatch);
        }
        // Derived structs also accept positional arrays; only the canonical
        // object (or null) may establish terminal progress.
        if wire.error.get() != "null" && !wire.error.get().starts_with('{') {
            return Err(Error::Invalid);
        }
        let error: Option<Failure> =
            serde_json::from_str(wire.error.get()).map_err(|_| Error::Invalid)?;
        let error_code = match (wire.status, error) {
            (Status::Failed, Some(failure))
                if valid_id(&failure.code)
                    && failure.code.len() <= 128
                    && !failure.message.trim().is_empty()
                    && failure.message.len() <= 4096 =>
            {
                Some(failure.code)
            }
            (Status::Pending | Status::Succeeded, None) => None,
            _ => return Err(Error::Invalid),
        };
        let receipt = Receipt {
            request_id: wire.request_id,
            action: wire.action,
            identity,
            status: wire.status,
            error_code,
        };
        if self
            .terminal
            .as_ref()
            .is_some_and(|previous| previous != &receipt)
        {
            return Err(Error::Invalid);
        }
        self.identity = receipt.identity.clone();
        if receipt.status != Status::Pending {
            self.terminal = Some(receipt.clone());
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
