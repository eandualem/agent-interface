//! Provider-parsed permissions retained by connection and agent registration.
//! Display paths are metadata only; this module never reads runtime files.

use serde::Deserialize;

use super::{Connection, Identity};
use crate::{RESPONSE_LIMIT, views::Agent};

/// Bounded failures without provider diagnostics or replacement-agent data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The retained connection refused or could not complete the request.
    Connection(super::Error),
    /// The selected registration was replaced or removed.
    RegistrationChanged,
    /// Malformed or inconsistent response metadata.
    Invalid,
    /// The provider does not support this contract version or endpoint.
    Unsupported,
    /// The agent has no durable registration to inspect.
    Unavailable,
}

/// A registered agent captured from an authenticated inspection.
/// Refreshes never acquire a replacement registration or credential snapshot.
#[derive(Clone)]
pub struct Target {
    connection: Connection,
    name: String,
    registration: String,
    runtime: String,
}

/// The initial card and the immutable target acquired in that same response.
pub struct Inspection {
    /// Card metadata from the bound provider, without local file enrichment.
    pub agent: Agent,
    /// Retain this target for every subsequent inspection and permissions read.
    pub target: Target,
}

/// Inspect an explicitly selected name, optionally pinned by a list observation.
/// Omit the registration only for fresh navigation, never a retained card refresh.
/// Runs on an I/O worker and does not start or discover a provider.
pub fn inspect(
    connection: Connection,
    name: &str,
    expected_registration: Option<&str>,
) -> Result<Inspection, Error> {
    if !crate::valid_agent(name) || expected_registration.is_some_and(|id| !valid_registration(id))
    {
        return Err(Error::Invalid);
    }
    let suffix = expected_registration
        .map(|id| format!("?expected_registration_id={id}"))
        .unwrap_or_default();
    let body = get(&connection, &format!("/api/agents/{name}/inspect{suffix}"))?;
    #[derive(Deserialize)]
    struct Registered {
        // Raw JSON requires the field, distinguishing missing from explicit null.
        registration_id: Box<serde_json::value::RawValue>,
        name: String,
        runtime: String,
    }
    let metadata: Registered = serde_json::from_slice(&body).map_err(|_| Error::Invalid)?;
    let registration: Option<String> =
        serde_json::from_str(metadata.registration_id.get()).map_err(|_| Error::Invalid)?;
    if metadata.name != name || !label(&metadata.runtime) {
        return Err(Error::Invalid);
    }
    if expected_registration.is_some_and(|id| registration.as_deref() != Some(id)) {
        return Err(Error::RegistrationChanged);
    }
    let registration = registration.ok_or(Error::Unavailable)?;
    if !valid_registration(&registration) {
        return Err(Error::Invalid);
    }
    let agent = Agent::decode(&body).map_err(|_| Error::Invalid)?;
    Ok(Inspection {
        agent,
        target: Target {
            connection,
            name: name.into(),
            registration,
            runtime: metadata.runtime,
        },
    })
}

impl Target {
    /// Full authenticated origin of this target.
    pub fn identity(&self) -> &Identity {
        self.connection.identity()
    }

    /// The exact durable registration acquired with the agent card.
    pub fn registration_id(&self) -> &str {
        &self.registration
    }

    /// Refresh only the captured registration; reject runtime replacement too.
    pub fn refresh(&self) -> Result<Agent, Error> {
        let result = inspect(
            self.connection.clone(),
            &self.name,
            Some(&self.registration),
        )?;
        if result.target.runtime != self.runtime {
            return Err(Error::Invalid);
        }
        Ok(result.agent)
    }

    /// Read sanitized permission metadata for this exact retained target.
    /// No local configuration, home directory or name-only fallback is consulted.
    pub fn permissions(&self) -> Result<Permissions, Error> {
        let body = get(
            &self.connection,
            &format!(
                "/api/agents/{}/runtime-context?expected_registration_id={}",
                self.name, self.registration
            ),
        )?;
        self.decode(&body)
    }

    fn decode(&self, body: &[u8]) -> Result<Permissions, Error> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(flatten)]
            identity: Identity,
            agent_name: String,
            registration_id: String,
            runtime: String,
            permissions: Permissions,
        }
        let reply: Reply = super::parse(body).map_err(|error| match error {
            super::Error::Unsupported => Error::Unsupported,
            _ => Error::Invalid,
        })?;
        if reply.identity != *self.identity()
            || reply.agent_name != self.name
            || reply.runtime != self.runtime
        {
            return Err(Error::Invalid);
        }
        if reply.registration_id != self.registration {
            return Err(Error::RegistrationChanged);
        }
        reply.permissions.validate()?;
        Ok(reply.permissions)
    }
}

fn get(connection: &Connection, path: &str) -> Result<Vec<u8>, Error> {
    let response = connection
        .response(path, None, RESPONSE_LIMIT)
        .map_err(Error::Connection)?;
    if response.status == 409 {
        #[derive(Deserialize)]
        struct Refusal {
            detail: Code,
        }
        #[derive(Deserialize)]
        struct Code {
            code: String,
        }
        if serde_json::from_slice::<Refusal>(&response.body)
            .is_ok_and(|error| error.detail.code == "registration_changed")
        {
            return Err(Error::RegistrationChanged);
        }
    }
    if response.status == 404 {
        return Err(Error::Unsupported);
    }
    if response.status != 200 {
        return Err(Error::Connection(super::Error::Refused(response.status)));
    }
    Ok(response.body)
}

pub(super) fn valid_registration(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn label(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

/// Whether the provider could inspect permissions in its own runtime context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The provider completed permission inspection.
    Available,
    /// Runtime context or required configuration could not be inspected.
    Unavailable,
    /// This runtime does not support permission inspection.
    Unsupported,
}

/// Provider-owned configuration scope, not filesystem authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Runtime-wide settings.
    Runtime,
    /// Project settings.
    Project,
    /// Local project overrides.
    ProjectLocal,
}

/// Outcome of the provider's bounded configuration read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Parsed successfully.
    Read,
    /// Not present in the provider's context.
    Missing,
    /// Present but malformed.
    Invalid,
    /// Could not be read completely.
    Unreadable,
}

/// A relative display label. Consumers must never open this as a local path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct File {
    /// Configuration layer inspected by the provider.
    pub scope: Scope,
    /// Relative, display-only path within that layer.
    pub display_path: String,
    /// Provider inspection result.
    pub state: FileState,
}

/// One sanitized runtime approval mode.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Mode {
    /// Runtime setting name.
    pub name: String,
    /// Provider-parsed setting value.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Diagnostic {
    code: String,
    message: String,
}

/// Sanitized permission data from the exact authenticated registration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Permissions {
    /// Whether inspection completed, failed or is unsupported.
    pub status: Status,
    /// Display-only file observations.
    pub files: Vec<File>,
    /// Rules allowed without prompting.
    pub allow: Vec<String>,
    /// Rules always refused.
    pub deny: Vec<String>,
    /// Rules requiring confirmation.
    pub ask: Vec<String>,
    /// Other approval settings.
    pub modes: Vec<Mode>,
    error: Option<Diagnostic>,
}

impl Permissions {
    /// What a provider without permission inspection supplies: nothing.
    pub fn unsupported() -> Self {
        Self {
            status: Status::Unsupported,
            files: Vec::new(),
            allow: Vec::new(),
            deny: Vec::new(),
            ask: Vec::new(),
            modes: Vec::new(),
            error: None,
        }
    }

    /// Fixed status explanation; raw provider diagnostic text is never displayed.
    pub fn message(&self) -> Option<&'static str> {
        match self.status {
            Status::Available => None,
            Status::Unavailable => Some("Runtime permission metadata is unavailable."),
            Status::Unsupported => Some("Runtime permission inspection is unsupported."),
        }
    }

    fn validate(&self) -> Result<(), Error> {
        let valid_files = self.files.iter().all(|file| {
            label(&file.display_path)
                && !file.display_path.contains('\\')
                && file.display_path.split('/').all(|part| {
                    !part.is_empty() && part != "." && part != ".." && !part.contains(':')
                })
        });
        let valid_values = self
            .allow
            .iter()
            .chain(&self.deny)
            .chain(&self.ask)
            .all(|s| label(s))
            && self
                .modes
                .iter()
                .all(|mode| label(&mode.name) && label(&mode.value));
        let valid_status = match self.status {
            Status::Available => {
                self.error.is_none()
                    && self.files.iter().all(|file| {
                        !matches!(file.state, FileState::Invalid | FileState::Unreadable)
                    })
            }
            Status::Unavailable | Status::Unsupported => {
                let code = if self.status == Status::Unavailable {
                    "unavailable"
                } else {
                    "unsupported"
                };
                self.allow.is_empty()
                    && self.deny.is_empty()
                    && self.ask.is_empty()
                    && self.modes.is_empty()
                    && self.error.as_ref().is_some_and(|error| {
                        error.code == code && Some(error.message.as_str()) == self.message()
                    })
            }
        };
        if valid_files && valid_values && valid_status {
            Ok(())
        } else {
            Err(Error::Invalid)
        }
    }
}
