//! An agent's card, inspected on its profile connection and kept to the
//! agent's durable registration.

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
    /// The inspect route answered 404 (the agent is not there).
    Unsupported,
    /// The agent has no durable registration to inspect.
    Unavailable,
}

/// A registered agent captured from an authenticated inspection.
/// Refreshes never acquire a replacement registration or connection.
#[derive(Clone)]
pub struct Target {
    connection: Connection,
    name: String,
    registration: String,
    runtime: String,
}

/// The initial card and the immutable target acquired in that same response.
pub struct Inspection {
    /// Card metadata from the bound provider.
    pub agent: Agent,
    /// Retain this target for every subsequent inspection.
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
