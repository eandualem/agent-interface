//! Profile-specific runtime observations. These describe compatibility and
//! authentication observations; they never authorize launches or imply readiness.

use super::{Error, Identity, valid_id};
use serde::Deserialize;
use std::collections::HashSet;

/// Authentication is independent of executable availability and compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    /// A provider probe reports an authenticated account.
    Authenticated,
    /// A provider probe reports no authenticated account.
    NotAuthenticated,
    /// The probe could not observe authentication.
    Unavailable,
    /// The runtime has no supported authentication probe.
    Unsupported,
    /// This runtime does not use an account.
    NotApplicable,
    /// Unverified or a future status; never treated as authenticated.
    #[serde(other)]
    Unknown,
}

/// One runtime's metadata, retained only within its originating catalog.
#[derive(Clone, Debug, Deserialize)]
pub struct Runtime {
    /// Stable runtime identifier.
    pub id: String,
    /// Human-readable provider label.
    pub display_name: String,
    /// Whether the provider found its executable in the profile context.
    pub available: bool,
    /// Whether this runtime supports first-agent setup.
    pub setup_supported: bool,
    /// Suggested model identifiers, not permission to launch them.
    pub models: Vec<String>,
    /// Suggested effort identifiers.
    pub efforts: Vec<String>,
    /// Explicit provider authentication observation.
    pub authentication: Authentication,
    /// Result of the requested setup compatibility check.
    pub setup_compatibility: Compatibility,
}

/// Compatibility is separate from authentication and from agent readiness.
/// Executable paths, launch arguments and provider prose are not retained.
#[derive(Clone, Debug, Deserialize)]
pub struct Compatibility {
    /// Whether setup prerequisites passed in the provider's profile context.
    pub compatible: bool,
    /// Machine-readable reason code; display uses consumer-owned text.
    pub reason: String,
    /// Optional observed runtime version, for display only.
    pub version: Option<String>,
}

/// An immutable full-origin catalog for delayed UI consumers.
#[derive(Clone, Debug)]
pub struct Catalog {
    identity: Identity,
    items: Vec<Runtime>,
}

#[derive(Deserialize)]
struct Wire {
    #[serde(flatten)]
    identity: Identity,
    items: Vec<Runtime>,
}

impl Catalog {
    /// Decode only successful schema-2 output for the exact expected tuple.
    /// This does no filesystem, account or provider access.
    pub fn parse(bytes: &[u8], expected: &Identity) -> Result<Self, Error> {
        if bytes.len() > 1024 * 1024
            || !valid_id(&expected.profile_id)
            || !valid_id(&expected.installation_id)
            || !valid_id(&expected.lifecycle_id)
        {
            return Err(Error::Invalid);
        }
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        match value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(2) => (),
            Some(_) => return Err(Error::Unsupported),
            None => return Err(Error::Invalid),
        }
        let wire: Wire = serde_json::from_value(value).map_err(|_| Error::Invalid)?;
        if wire.identity != *expected {
            return Err(Error::BindingMismatch);
        }
        let mut ids = HashSet::new();
        for item in &wire.items {
            if !valid_id(&item.id)
                || !ids.insert(&item.id)
                || item.display_name.trim().is_empty()
                || item.display_name.chars().any(char::is_control)
                || !valid_id(&item.setup_compatibility.reason)
                || item
                    .setup_compatibility
                    .version
                    .as_ref()
                    .is_some_and(|version| {
                        version.is_empty() || version.chars().any(char::is_control)
                    })
                || !valid_choices(&item.models)
                || !valid_choices(&item.efforts)
                || (item.setup_compatibility.compatible
                    && (!item.available || !item.setup_supported))
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            identity: wire.identity,
            items: wire.items,
        })
    }

    /// Full origin retained across delayed rendering and selection.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Runtime observations; no item is itself launch authority.
    pub fn items(&self) -> &[Runtime] {
        &self.items
    }
}

fn valid_choices(values: &[String]) -> bool {
    let mut seen = HashSet::new();
    values
        .iter()
        .all(|value| valid_id(value) && seen.insert(value))
}

#[cfg(test)]
mod tests;
