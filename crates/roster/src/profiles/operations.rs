//! An immutable request to create a profile, and what became of it. This
//! module invokes no command. First-agent setup uses a separate versioned
//! contract.

use super::{Error, Identity, Profile, valid_id};

mod recovery;

use serde::Deserialize;
use serde_json::json;

/// Maximum bytes in a consumer-generated request ID, which `profiles create`
/// takes as one command argument.
pub const REQUEST_ID_LIMIT: usize = 1024;

/// What a request asks of the service. No action is inferred from an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Make a new profile, which the service serves at once.
    Create,
}
impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
        }
    }
}

/// The result of one original request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The service made the profile.
    Succeeded,
    /// The service made nothing; this grants no content authority.
    Failed,
}

/// A validated observation of one prepared request; it grants no content access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The original idempotency key.
    pub request_id: String,
    /// The original action.
    pub action: Action,
    /// The new profile's identity; absent when nothing was made.
    pub identity: Option<Identity>,
    /// The terminal result.
    pub status: Status,
    /// A bounded failure category; raw provider messages are not retained.
    pub error_code: Option<String>,
}

/// Original request bytes and progress. Selection changes cannot rewrite them.
/// The first result is pinned and cannot change. Keep this value across
/// uncertain delivery and ask again under its original request ID.
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
    pub fn create(request_id: &str, display_name: &str) -> Result<Self, Error> {
        Self::create_request(request_id, display_name, false)
    }

    /// As [`Self::create`], and records that the new profile is to get this
    /// computer's Git user.name and user.email, which the app sets in it once
    /// it is made; nothing else of theirs.
    pub fn create_copying_git_identity(
        request_id: &str,
        display_name: &str,
    ) -> Result<Self, Error> {
        Self::create_request(request_id, display_name, true)
    }

    fn create_request(
        request_id: &str,
        display_name: &str,
        copy_git_identity: bool,
    ) -> Result<Self, Error> {
        if !valid_id(request_id)
            || display_name.trim().is_empty()
            || display_name.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        let mut body = json!({
            "request_id":request_id,"action":"create","display_name":display_name,
        });
        // Omitted means none, so a request without the choice keeps its bytes.
        if copy_git_identity {
            body["git_identity"] = json!("copy");
        }
        Self::prepare(request_id, Action::Create, body)
    }

    fn prepare(request_id: &str, action: Action, body: serde_json::Value) -> Result<Self, Error> {
        if request_id.len() > REQUEST_ID_LIMIT {
            return Err(Error::Invalid);
        }
        let body = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        if body.len() > crate::RESPONSE_LIMIT {
            return Err(Error::Invalid);
        }
        Ok(Self {
            request_id: request_id.into(),
            action,
            body,
            identity: None,
            terminal: None,
        })
    }

    /// The exact request body to retain for submission or an explicit retry.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }
    /// Original action, retained independently of later selection or receipts.
    pub fn action(&self) -> Action {
        self.action
    }

    /// The original idempotency key; a timeout must not allocate a new one.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    /// The validated terminal observation, if any; restored checkpoints retain it.
    pub fn terminal(&self) -> Option<&Receipt> {
        self.terminal.as_ref()
    }

    /// The new profile's identity once made, never a verified connection.
    pub fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// The new profile's name, which `profiles create` takes as an argument.
    pub fn display_name(&self) -> Option<String> {
        let body: serde_json::Value = serde_json::from_slice(&self.body).ok()?;
        body.get("display_name")?.as_str().map(str::to_owned)
    }

    /// Whether this create request asks for this computer's Git name and
    /// email, which the Rust service leaves to the app to set.
    pub fn copies_git_identity(&self) -> bool {
        serde_json::from_slice::<serde_json::Value>(&self.body)
            .is_ok_and(|body| body.get("git_identity") == Some(&json!("copy")))
    }

    /// Record the Rust service's answer to this create request: the entry of
    /// the profile it made, which it serves at once.
    pub fn observe_created(&mut self, profile: &Profile) -> Result<Receipt, Error> {
        if self.action != Action::Create {
            return Err(Error::Invalid);
        }
        self.record(Receipt {
            request_id: self.request_id.clone(),
            action: self.action,
            identity: Some(profile.expected_identity()?),
            status: Status::Succeeded,
            error_code: None,
        })
    }

    /// Record that the Rust service refused this create request, or could not
    /// be reached, so it made nothing; `code` names which.
    pub fn observe_refused(&mut self, code: &str) -> Result<Receipt, Error> {
        if self.action != Action::Create || !valid_id(code) || code.len() > 128 {
            return Err(Error::Invalid);
        }
        self.record(Receipt {
            request_id: self.request_id.clone(),
            action: self.action,
            identity: None,
            status: Status::Failed,
            error_code: Some(code.to_owned()),
        })
    }

    /// Keep a validated observation: the first result cannot change.
    fn record(&mut self, receipt: Receipt) -> Result<Receipt, Error> {
        if self
            .identity
            .as_ref()
            .is_some_and(|expected| Some(expected) != receipt.identity.as_ref())
        {
            return Err(Error::BindingMismatch);
        }
        if self
            .terminal
            .as_ref()
            .is_some_and(|previous| previous != &receipt)
        {
            return Err(Error::Invalid);
        }
        self.identity = receipt.identity.clone();
        self.terminal = Some(receipt.clone());
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
