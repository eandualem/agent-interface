//! Profile-specific runtime observations: what each runtime is and whether it
//! is signed in. They never authorize launches or imply readiness.

use super::{Error, Identity, valid_id};
use serde::Deserialize;
use std::collections::HashSet;

/// Authentication is independent of executable availability and setup support.
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
}

/// An immutable full-origin catalog for delayed UI consumers.
#[derive(Clone, Debug)]
pub struct Catalog {
    identity: Identity,
    items: Vec<Runtime>,
}

impl Catalog {
    /// Decode the Rust service's `backbone runtimes --json` (schema 1) for the
    /// profile `expected`. It names no profile and runs no setup check: what
    /// it lists is this machine's.
    pub fn served(bytes: &[u8], expected: &Identity) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct Served {
            items: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            id: String,
            display_name: String,
            available: bool,
            setup_supported: bool,
            models: Vec<String>,
            efforts: Vec<String>,
            authentication: Authentication,
        }
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        // Only the object the service writes; a derived struct also accepts a
        // positional array.
        if bytes.len() > 1024 * 1024 || bytes.trim_ascii_start().first() != Some(&b'{') {
            return Err(Error::Invalid);
        }
        // The version comes first, so a newer shape is told apart from a
        // malformed one.
        let version: Version = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if version.schema_version != 1 {
            return Err(Error::Unsupported);
        }
        let served: Served = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        let items = served
            .items
            .into_iter()
            .map(|item| Runtime {
                id: item.id,
                display_name: item.display_name,
                available: item.available,
                setup_supported: item.setup_supported,
                models: item.models,
                efforts: item.efforts,
                authentication: item.authentication,
            })
            .collect();
        Self::checked(expected.clone(), items)
    }

    fn checked(identity: Identity, items: Vec<Runtime>) -> Result<Self, Error> {
        if !valid_id(&identity.profile_id)
            || !valid_id(&identity.installation_id)
            || !valid_id(&identity.lifecycle_id)
        {
            return Err(Error::Invalid);
        }
        let mut ids = HashSet::new();
        for item in &items {
            if !valid_id(&item.id)
                || !ids.insert(&item.id)
                || item.display_name.trim().is_empty()
                || item.display_name.chars().any(char::is_control)
                || !valid_choices(&item.models)
                || !valid_choices(&item.efforts)
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Self { identity, items })
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
