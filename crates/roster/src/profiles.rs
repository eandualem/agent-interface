//! Profile contract r2: explicit discovery and a pinned local connection.
//! Not used by legacy startup or the desktop profile selector.

use std::{
    fs::OpenOptions,
    hash::{Hash, Hasher},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub mod operations;

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub mod runtime_catalog;
pub mod runtime_context;
pub mod setup;

use crate::{RESPONSE_LIMIT, Snapshot, transport::local};

/// A registered provider instance. Display names and baseline versions are not identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct Identity {
    /// Stable profile, including across registered-instance replacement.
    pub profile_id: String,
    /// Existing provider signing audience.
    pub installation_id: String,
    /// Instance generation, changed by replacement or restored activation.
    pub lifecycle_id: String,
}

/// A bounded failure category; provider prose and credentials are never retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Malformed, oversized or inconsistent discovery/identity data.
    Invalid,
    /// The provider or wire schema does not support bound connections.
    Unsupported,
    /// Provisioning is incomplete or failed.
    NotReady,
    /// The exact credential or local endpoint could not be read safely.
    Unavailable,
    /// The authenticated provider requires a complete binding.
    BindingRequired,
    /// The expected instance or tmux source does not match.
    BindingMismatch,
    /// Another HTTP refusal, retaining only its status.
    Refused(u16),
}

/// Why a skill or document was not added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddError {
    /// The provider's reason, to show the person.
    Refused(String),
    /// The connection failed, or the provider gave no reason.
    Connection(Error),
}

impl From<Error> for AddError {
    fn from(error: Error) -> Self {
        Self::Connection(error)
    }
}

/// A skill now in the profile's store.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AddedSkill {
    /// The name it has in the store.
    pub name: String,
    /// What did not happen although the skill was added.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Registry classification; it does not confer legacy connection eligibility.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The original installation, registered in place.
    LegacyDefault,
    /// A separately provisioned profile.
    Isolated,
}

/// Registry provisioning state, independent of whether the service is running.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Not ready for content connections.
    Creating,
    /// Provisioned; still requires an authenticated identity handshake.
    Ready,
    /// Provisioning failed and needs explicit recovery.
    Failed,
}

/// Discovery metadata. `Ready` describes provisioning, not service availability.
#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    /// Stable profile identity, never inferred from its name.
    pub profile_id: String,
    /// Human-readable label, not request authority.
    pub display_name: String,
    /// How the profile was provisioned.
    pub kind: Kind,
    /// Whether a connection binding may be present.
    pub state: State,
}

#[derive(Deserialize)]
struct Catalog {
    items: Vec<Item>,
}

/// Decode a successful registry list. A registry command error must not call this.
pub fn list(bytes: &[u8]) -> Result<Vec<Item>, Error> {
    let catalog: Catalog = parse(bytes)?;
    let mut ids = std::collections::HashSet::new();
    for item in &catalog.items {
        validate_item(item)?;
        if !ids.insert(&item.profile_id) {
            return Err(Error::Invalid);
        }
    }
    Ok(catalog.items)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Endpoint {
    Unix { path: PathBuf },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Credential {
    Dotenv { path: PathBuf, key: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Tmux {
    socket_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct Binding {
    installation_id: String,
    lifecycle_id: String,
    endpoint: Endpoint,
    credential_reference: Credential,
    tmux: Tmux,
}

#[derive(Deserialize)]
struct Record {
    #[serde(flatten)]
    item: Item,
    // Raw JSON distinguishes a required null from a missing binding field.
    binding: Box<serde_json::value::RawValue>,
}

/// An atomic registry record; only `connect` can turn it into a verified connection.
#[derive(Clone, Debug)]
pub struct Profile {
    item: Item,
    binding: Option<Binding>,
}

impl Profile {
    /// Decode a bounded registry show reply and validate its binding consistency.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let record: Record = parse(bytes)?;
        validate_item(&record.item)?;
        let binding: Option<Binding> =
            serde_json::from_str(record.binding.get()).map_err(|_| Error::Invalid)?;
        if (record.item.state == State::Ready) != binding.is_some() {
            return Err(Error::Invalid);
        }
        if let Some(binding) = &binding {
            let Endpoint::Unix { path } = &binding.endpoint;
            let Credential::Dotenv {
                path: credential,
                key,
            } = &binding.credential_reference;
            if !valid_id(&binding.installation_id)
                || !valid_id(&binding.lifecycle_id)
                || !path.is_absolute()
                || !credential.is_absolute()
                || !binding.tmux.socket_path.is_absolute()
                || key != "BACKBONE_API_KEY"
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            item: record.item,
            binding,
        })
    }

    /// Read metadata without probing or starting the provider.
    pub fn item(&self) -> &Item {
        &self.item
    }

    /// Capture registry identity metadata without reading credentials or probing
    /// the provider. This expectation alone does not authorize a connection.
    pub fn expected_identity(&self) -> Result<Identity, Error> {
        let binding = self.binding.as_ref().ok_or(Error::NotReady)?;
        Ok(Identity {
            profile_id: self.item.profile_id.clone(),
            installation_id: binding.installation_id.clone(),
            lifecycle_id: binding.lifecycle_id.clone(),
        })
    }

    /// Read only the named credential, then authenticate and verify the exact provider.
    /// Runs on an I/O worker; does not discover or start a service.
    pub fn connect(&self) -> Result<Connection, Error> {
        let binding = self.binding.as_ref().ok_or(Error::NotReady)?;
        let Credential::Dotenv { path, .. } = &binding.credential_reference;
        let key = credential(path)?;
        let Endpoint::Unix { path: socket } = &binding.endpoint;
        let connection = Connection {
            identity: Identity {
                profile_id: self.item.profile_id.clone(),
                installation_id: binding.installation_id.clone(),
                lifecycle_id: binding.lifecycle_id.clone(),
            },
            kind: self.item.kind.clone(),
            socket: socket.clone(),
            tmux: binding.tmux.clone(),
            credential_reference: path.clone(),
            key,
            connection_key: ConnectionKey(Arc::new(())),
        };
        connection.verify()?;
        Ok(connection)
    }
}

/// Opaque process-local cache identity for a verified connection snapshot.
/// Clones share a key; a fresh handshake gets a distinct key even when the
/// provider identity is unchanged. This is not request authority or a wire ID.
#[derive(Clone)]
pub struct ConnectionKey(Arc<()>);

impl PartialEq for ConnectionKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ConnectionKey {}
impl Hash for ConnectionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

/// Immutable identity, endpoint and credential snapshot for one connection's work.
/// Intentionally has no Debug implementation because it holds a credential.
#[derive(Clone)]
pub struct Connection {
    identity: Identity,
    kind: Kind,
    socket: PathBuf,
    tmux: Tmux,
    credential_reference: PathBuf,
    key: String,
    connection_key: ConnectionKey,
}

impl Connection {
    /// Cache identity shared only by clones of this verified snapshot.
    /// Fresh connections must not reuse workers holding older credentials.
    pub fn connection_key(&self) -> ConnectionKey {
        self.connection_key.clone()
    }

    /// The authenticated identity retained by every operation on this connection.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    /// How this profile was provisioned, from the record that was connected.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }
    /// The tmux socket confirmed by the provider handshake.
    pub fn tmux_socket(&self) -> &Path {
        &self.tmux.socket_path
    }
    /// The original credential locator; this connection never re-reads it.
    pub fn credential_reference(&self) -> &Path {
        &self.credential_reference
    }

    /// Read the roster on this verified binding without re-reading configuration.
    /// Runs on an I/O worker. Refusals retain their typed category; malformed
    /// roster data returns `Invalid` without exposing the response body.
    pub fn roster(&self) -> Result<Snapshot, Error> {
        let response = self.response("/api/agents", None, RESPONSE_LIMIT)?;
        successful(&response)?;
        let mut snapshot = Snapshot::default();
        snapshot.update(&response.body);
        if snapshot.error.is_some() {
            return Err(Error::Invalid);
        }
        Ok(snapshot)
    }

    /// Ask the provider to ensure this pane's shell and return its session name,
    /// which lives on this connection's tmux socket. The provider owns the shell's
    /// environment and tmux server; the name is opaque. Runs on an I/O worker.
    pub fn ensure_shell(&self, shell_id: &str, cwd: Option<&str>) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(flatten)]
            identity: Identity,
            shell_id: String,
            session: String,
        }
        let body = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "shell_id": shell_id,
            "cwd": cwd,
        }))
        .map_err(|_| Error::Invalid)?;
        let response = self.response("/api/profile/shells", Some(body), RESPONSE_LIMIT)?;
        if response.status == 404 {
            return Err(Error::Unsupported);
        }
        successful(&response)?;
        let reply: Reply = parse(&response.body)?;
        if reply.identity != self.identity {
            return Err(Error::BindingMismatch);
        }
        if reply.shell_id != shell_id || reply.session.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(reply.session)
    }

    /// Ask the provider for a session that signs a CLI account in for `runtime`
    /// inside this profile, and return its name on this connection's tmux socket.
    /// A repeat reuses the session while it is live. Runs on an I/O worker.
    pub fn sign_in_session(&self, runtime: &str) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(flatten)]
            identity: Identity,
            runtime: String,
            session: String,
        }
        let body = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "runtime": runtime,
        }))
        .map_err(|_| Error::Invalid)?;
        let response =
            self.response("/api/profile/accounts/sign-in", Some(body), RESPONSE_LIMIT)?;
        if response.status == 404 {
            return Err(Error::Unsupported);
        }
        successful(&response)?;
        let reply: Reply = parse(&response.body)?;
        if reply.identity != self.identity {
            return Err(Error::BindingMismatch);
        }
        if reply.runtime != runtime || reply.session.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(reply.session)
    }

    /// Stop this agent's session and remove it from this profile. The provider
    /// refuses to remove a running agent, so the stop comes first. An agent that
    /// is already gone counts as removed. Runs on an I/O worker.
    pub fn remove_agent(&self, name: &str) -> Result<(), Error> {
        if !crate::valid_agent(name) {
            return Err(Error::Invalid);
        }
        let response = self.response(
            &format!("/api/agents/{name}/stop"),
            Some(Vec::new()),
            RESPONSE_LIMIT,
        )?;
        if response.status == 404 {
            return Ok(());
        }
        successful(&response)?;
        let response = local::profile_delete(
            self.socket.to_str().ok_or(Error::Invalid)?,
            &format!("/api/agents/{name}"),
            (&self.key, &self.identity),
            RESPONSE_LIMIT,
            Duration::from_secs(15),
        )
        .map_err(|_| Error::Unavailable)?;
        if response.status == 404 {
            return Ok(());
        }
        successful(&response)
    }

    /// Copy the skill folder at `path` into this profile's store for every agent
    /// in the profile, leaving the folder in place. Runs on an I/O worker.
    pub fn add_skill(&self, path: &Path) -> Result<AddedSkill, AddError> {
        let body = serde_json::to_vec(&serde_json::json!({
            "path": path.to_str().ok_or(Error::Invalid)?,
            "name": null,
            "tags": ["all"],
            "replace": false,
            "mode": "copy",
            "actor": "app",
        }))
        .map_err(|_| Error::Invalid)?;
        let response = self.response("/api/skills", Some(body), RESPONSE_LIMIT)?;
        added(&response)?;
        Ok(serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?)
    }

    /// Publish a person's own document on this profile and return its revision.
    /// The request ID comes from the content, so a repeat is the same publication.
    /// Runs on an I/O worker.
    pub fn add_document(&self, title: &str, html: &str) -> Result<u64, AddError> {
        #[derive(Deserialize)]
        struct Reply {
            record: Record,
        }
        #[derive(Deserialize)]
        struct Record {
            id: u64,
        }
        let digest = Sha256::new()
            .chain_update(title)
            .chain_update([0])
            .chain_update(html)
            .finalize();
        let body = serde_json::to_vec(&serde_json::json!({
            "author": "operator",
            "request_id": format!("app:{digest:x}"),
            "title": title,
            "html": html,
            "supersedes_id": null,
        }))
        .map_err(|_| Error::Invalid)?;
        let response = self.response("/api/documents", Some(body), RESPONSE_LIMIT)?;
        added(&response)?;
        let reply: Reply = serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?;
        Ok(reply.record.id)
    }

    /// Verify again on the same binding, never by re-resolving registry or environment.
    pub fn verify(&self) -> Result<(), Error> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(flatten)]
            identity: Identity,
            baseline_version: String,
            tmux: Tmux,
        }
        let response = self.response("/api/profile", None, RESPONSE_LIMIT)?;
        if response.status == 404 {
            return Err(Error::Unsupported);
        }
        successful(&response)?;
        let reply: Reply = parse(&response.body)?;
        if reply.baseline_version.is_empty() {
            return Err(Error::Invalid);
        }
        if reply.identity != self.identity || reply.tmux != self.tmux {
            return Err(Error::BindingMismatch);
        }
        Ok(())
    }

    // Every request carries the tuple, including requests after a successful probe.
    // Future content adapters must use this connection rather than legacy configuration.
    pub(crate) fn response(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<local::Response, Error> {
        // The agent list refuses a read until its next refresh after an agent is
        // removed. That is a moment to read again, not a failure to show anyone.
        // Callers are I/O workers, so waiting here holds no interface thread.
        let mut retries = if path == "/api/agents" { 40 } else { 0 };
        let response = loop {
            let response = local::profile_response(
                self.socket.to_str().ok_or(Error::Invalid)?,
                path,
                (&self.key, &self.identity),
                body.clone(),
                limit,
                Duration::from_secs(15),
            )
            .map_err(|_| Error::Unavailable)?;
            if retries == 0 || !registrations_settling(&response) {
                break response;
            }
            retries -= 1;
            std::thread::sleep(Duration::from_millis(250));
        };
        if response.body.len() > limit {
            return Err(Error::Invalid);
        }
        if let Some(error) = binding_refusal(&response) {
            return Err(error);
        }
        Ok(response)
    }
}

fn successful(response: &local::Response) -> Result<(), Error> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    Err(binding_refusal(response).unwrap_or(Error::Refused(response.status)))
}

/// A refused addition keeps the provider's reason, written to be shown to the person.
fn added(response: &local::Response) -> Result<(), AddError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let refusal: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
    let detail = match &refusal["detail"] {
        serde_json::Value::Array(errors) => errors
            .first()
            .map_or("", |error| error["msg"].as_str().unwrap_or_default()),
        detail => detail.as_str().unwrap_or_default(),
    };
    Err(if detail.is_empty() {
        AddError::Connection(Error::Refused(response.status))
    } else {
        AddError::Refused(detail.to_owned())
    })
}

fn registrations_settling(response: &local::Response) -> bool {
    response.status == 409
        && serde_json::from_slice::<serde_json::Value>(&response.body)
            .is_ok_and(|refusal| refusal["detail"]["code"] == "registration_changed")
}

fn binding_refusal(response: &local::Response) -> Option<Error> {
    if response.status == 409 {
        #[derive(Deserialize)]
        struct Refusal {
            detail: Detail,
        }
        #[derive(Deserialize)]
        struct Detail {
            code: String,
        }
        if let Ok(refusal) = serde_json::from_slice::<Refusal>(&response.body) {
            match refusal.detail.code.as_str() {
                "binding_required" => return Some(Error::BindingRequired),
                "binding_mismatch" => return Some(Error::BindingMismatch),
                _ => {}
            }
        }
    }
    None
}

fn schema(version: u32) -> Result<(), Error> {
    if version == 1 {
        Ok(())
    } else {
        Err(Error::Unsupported)
    }
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    if bytes.len() > RESPONSE_LIMIT {
        return Err(Error::Invalid);
    }
    #[derive(Deserialize)]
    struct Version {
        schema_version: u32,
    }
    // Inspect the envelope before decoding fields that a newer schema may change.
    let version: Version = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    schema(version.schema_version)?;
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_graphic())
}

fn validate_item(item: &Item) -> Result<(), Error> {
    if !valid_id(&item.profile_id)
        || item.display_name.trim().is_empty()
        || item.display_name.chars().any(char::is_control)
    {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}

fn credential(path: &Path) -> Result<String, Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Error::Unavailable)?;
    let metadata = file.metadata().map_err(|_| Error::Unavailable)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(Error::Unavailable);
    }
    let mut text = String::new();
    file.take((RESPONSE_LIMIT + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|_| Error::Unavailable)?;
    if text.len() > RESPONSE_LIMIT {
        return Err(Error::Unavailable);
    }
    let key = crate::configuration::dotenv_key(&text)
        .filter(|key| !key.is_empty())
        .ok_or(Error::Unavailable)?;
    http::HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| Error::Unavailable)?;
    Ok(key)
}

#[cfg(test)]
mod tests;
