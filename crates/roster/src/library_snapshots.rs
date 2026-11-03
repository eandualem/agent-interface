//! Shared Skill and Policy snapshots retained through one verified connection.
//! Provider paths are display metadata, never local filesystem authority.

use crate::profiles::{Connection, Identity};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

/// The two provider-observed shared stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A shared Skill's SKILL.md snapshot.
    Skill,
    /// A shared Policy's instruction snapshot.
    Policy,
}
impl Kind {
    fn route(self) -> &'static str {
        match self {
            Self::Skill => "/api/skills",
            Self::Policy => "/api/policies",
        }
    }
}

/// Fixed failures; provider prose and credentials are never returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The original connection failed or refused its binding.
    Connection(crate::profiles::Error),
    /// The provider does not offer this artifact or endpoint.
    Missing,
    /// The response violated the bounded snapshot contract.
    Invalid,
    /// The request is no longer wanted.
    Cancelled,
}

/// A listed source. Its path is only a label; an unreadable source may have no ID.
#[derive(Clone)]
pub struct Entry {
    /// The provider's display name.
    pub name: String,
    /// Display-only source path.
    pub path: String,
    /// Captured stable identity, when the provider has observed readable content.
    pub artifact: Option<Artifact>,
}

/// A stable artifact and its original connection, captured during catalog read.
/// No path lookup or current selection is consulted when it is opened later.
#[derive(Clone)]
pub struct Artifact {
    connection: Connection,
    kind: Kind,
    id: String,
}
impl Artifact {
    /// Stable artifact ID for discussion targets on this same connection.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The store containing this artifact.
    pub fn kind(&self) -> Kind {
        self.kind
    }
    /// Full authenticated origin retained by this artifact.
    pub fn identity(&self) -> &Identity {
        self.connection.identity()
    }
    /// Original connection for related discussions; it never reloads credentials.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Read the latest observed version and then that exact immutable snapshot.
    /// Call on an I/O worker. Later edits cannot change the selected revision.
    pub fn latest(&self, live: &dyn Fn() -> bool) -> Result<Snapshot, Error> {
        #[derive(Deserialize)]
        struct Versions {
            items: Vec<Box<RawValue>>,
        }
        #[derive(Deserialize)]
        struct Version {
            revision_id: String,
        }
        let route = format!("{}/{}/versions", self.kind.route(), self.id);
        let versions: Versions = read(&self.connection, &(route.clone() + "?limit=1"), live)?;
        if versions.items.len() != 1 {
            return Err(Error::Invalid);
        }
        let version: Version = object(versions.items[0].get().as_bytes())?;
        let revision = &version.revision_id;
        if !revision_id(revision) {
            return Err(Error::Invalid);
        }
        #[derive(Deserialize)]
        struct Body {
            revision_id: String,
            label: String,
            created_at: String,
            sha256: Box<RawValue>,
            content: Box<RawValue>,
        }
        let body: Body = read(&self.connection, &format!("{route}/{revision}"), live)?;
        if body.revision_id != *revision {
            return Err(Error::Invalid);
        }
        let content: Option<String> =
            serde_json::from_str(body.content.get()).map_err(|_| Error::Invalid)?;
        let digest: Option<String> =
            serde_json::from_str(body.sha256.get()).map_err(|_| Error::Invalid)?;
        match (&content, &digest) {
            (Some(content), Some(digest))
                if *digest == format!("{:x}", Sha256::digest(content.as_bytes())) => {}
            (None, None) => {}
            _ => return Err(Error::Invalid),
        }
        Ok(Snapshot {
            artifact: self.clone(),
            revision_id: body.revision_id,
            label: body.label,
            created_at: body.created_at,
            content,
        })
    }
}

/// The exact provider snapshot, with its immutable artifact origin.
#[derive(Clone)]
pub struct Snapshot {
    /// Original artifact for discussion and subsequent reads.
    pub artifact: Artifact,
    /// The exact revision that supplied this content.
    pub revision_id: String,
    /// Provider display label for the revision.
    pub label: String,
    /// Provider observation timestamp.
    pub created_at: String,
    /// Snapshot text; absent for an observed missing source.
    pub content: Option<String>,
}

/// List shared sources on the supplied connection. Each returned artifact owns
/// that connection, so a retained row cannot be redirected by selection changes.
/// This deliberately excludes repository and runtime-local skill files.
pub fn list(
    connection: &Connection,
    kind: Kind,
    live: &dyn Fn() -> bool,
) -> Result<Vec<Entry>, Error> {
    #[derive(Deserialize)]
    struct Listing {
        items: Vec<Box<RawValue>>,
    }
    #[derive(Deserialize)]
    struct Row {
        name: String,
        path: String,
        artifact_id: Option<String>,
    }
    let listing: Listing = read(connection, kind.route(), live)?;
    if listing.items.len() > 4096 {
        return Err(Error::Invalid);
    }
    let mut ids = std::collections::BTreeSet::new();
    listing
        .items
        .into_iter()
        .map(|raw| {
            let row: Row = object(raw.get().as_bytes())?;
            if row.name.is_empty() || row.path.is_empty() {
                return Err(Error::Invalid);
            }
            let artifact = match row.artifact_id {
                Some(id) => {
                    if !token(&id) || !ids.insert(id.clone()) {
                        return Err(Error::Invalid);
                    }
                    Some(Artifact {
                        connection: connection.clone(),
                        kind,
                        id,
                    })
                }
                None => None,
            };
            Ok(Entry {
                name: row.name,
                path: row.path,
                artifact,
            })
        })
        .collect()
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn revision_id(value: &str) -> bool {
    !value.starts_with('0')
        && value.len() <= 19
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|n| n > 0)
}
fn read<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    path: &str,
    live: &dyn Fn() -> bool,
) -> Result<T, Error> {
    if !live() {
        return Err(Error::Cancelled);
    }
    let response = connection
        .response(path, None, crate::RESPONSE_LIMIT)
        .map_err(Error::Connection)?;
    if !live() {
        return Err(Error::Cancelled);
    }
    match response.status {
        200 => {}
        404 => return Err(Error::Missing),
        status => return Err(Error::Connection(crate::profiles::Error::Refused(status))),
    }
    object(&response.body)
}

fn object<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    // Object envelopes only: derived structs otherwise accept positional arrays.
    if body.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(Error::Invalid);
    }
    serde_json::from_slice(body).map_err(|_| Error::Invalid)
}
