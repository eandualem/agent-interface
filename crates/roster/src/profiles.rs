//! Agent Backbone profiles: explicit discovery and a pinned local connection.

use std::{
    fmt,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub mod operations;

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub mod accounts;
pub mod peers;
pub mod runs;
pub mod runtime_catalog;
pub mod runtime_context;
pub mod secrets;
mod served;
pub mod setup;
mod stream;
pub mod terminal;

pub use stream::Change;

use crate::{RESPONSE_LIMIT, Snapshot, transport::local};

/// A served profile. Display names are not identity.
/// The Rust service has no installation or generation of its own: for its profiles
/// both of those fields hold the machine's ID, which stays the same across restarts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct Identity {
    /// Stable profile.
    pub profile_id: String,
    /// The machine ID used in profile-scoped view keys.
    pub installation_id: String,
    /// The same machine ID in the lifecycle component of those keys.
    pub lifecycle_id: String,
}

/// A bounded failure category; provider prose and credentials are never retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Malformed, oversized or inconsistent discovery/identity data.
    Invalid,
    /// The service returned an unsupported version or profile state.
    Unsupported,
    /// Provisioning is incomplete or failed.
    NotReady,
    /// The local endpoint could not be reached safely.
    Unavailable,
    /// The authenticated provider requires a complete binding.
    BindingRequired,
    /// The expected instance or tmux source does not match.
    BindingMismatch,
    /// The agent's durable registration is no longer the expected one.
    RegistrationChanged,
    /// Another HTTP refusal, retaining only its status.
    Refused(u16),
    /// The service is older than [`MINIMUM_BACKBONE`]; it holds the version it reported.
    Outdated(ReportedVersion),
    /// The service reported no version this app can read.
    VersionUnknown,
}

/// The lowest Agent Backbone version this app works with, compared with the
/// `version` the service reports from `GET /health`. Raised when the app
/// first needs a newer Backbone.
pub const MINIMUM_BACKBONE: semver::Version = semver::Version::new(0, 1, 0);

/// A version a service reported, kept as its text so [`Error`] stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReportedVersion {
    text: [u8; 64],
    len: u8,
}

impl ReportedVersion {
    /// `None` when its text does not fit, so it is reported as unknown.
    fn new(version: &semver::Version) -> Option<Self> {
        let version = version.to_string();
        let mut text = [0; 64];
        text.get_mut(..version.len())?
            .copy_from_slice(version.as_bytes());
        Some(Self {
            text,
            len: version.len() as u8,
        })
    }

    /// The version as the service reported it, such as `0.0.9`.
    pub fn as_str(&self) -> &str {
        // Only whole version text is stored, so it is always valid UTF-8.
        std::str::from_utf8(&self.text[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl fmt::Display for ReportedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ReportedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
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

/// Whether the service serves the profile, independent of its agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Served; still requires an authenticated identity handshake.
    Ready,
    /// The service could not start serving it.
    Failed,
}

/// Discovery metadata. `Ready` describes the listing, not a verified connection.
#[derive(Clone, Debug)]
pub struct Item {
    /// Stable profile identity, never inferred from its name.
    pub profile_id: String,
    /// Human-readable label, not request authority.
    pub display_name: String,
    /// Whether a connection binding is present.
    pub state: State,
    /// Why the service does not serve the profile: its own sentence for a
    /// person, shown as given. Its wording may change at any time.
    pub reason: Option<String>,
    /// The same, as the service's stable code, for logs.
    pub failure: Option<Failure>,
    /// Whose accounts the profile's agents use.
    pub accounts: Accounts,
}

/// Whose CLI and GitHub accounts a profile's agents use, as the service
/// reports it. A value this app does not know, `null` or an absent field
/// is `Unknown`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accounts {
    /// The machine user's, in their home directory.
    MachineUser,
    /// The profile's own, kept in its directory.
    Own,
    /// Not known.
    #[default]
    #[serde(other)]
    Unknown,
}

/// Why the service does not serve a profile, by its stable code. A code this
/// app does not know is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// The profile's settings can't be read.
    SettingsUnreadable,
    /// No database server answered.
    DatabaseUnreachable,
    /// The profile's database doesn't exist and can't be made.
    DatabaseNotCreated,
    /// The database is one this service can't work with.
    DatabaseIncompatible,
    /// Another service already serves the profile.
    AlreadyServed,
    /// Anything else that stopped the start.
    StartFailed,
    /// A code this app doesn't know.
    #[serde(other)]
    Other,
}

/// Decode a successful `profiles list`. A command error must not call this.
pub fn list(bytes: &[u8]) -> Result<Vec<Item>, Error> {
    if bytes.len() > RESPONSE_LIMIT {
        return Err(Error::Invalid);
    }
    served::list(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tmux {
    socket_path: PathBuf,
}

#[derive(Clone, Debug)]
struct Binding {
    installation_id: String,
    lifecycle_id: String,
    /// The owner-only socket, which is the credential: no key is read or sent.
    socket: PathBuf,
    tmux: Tmux,
    /// The service's run, new at each of its starts. It is checked when
    /// connecting and never kept as identity.
    instance: String,
}

/// One entry of the service's profile list; only `connect` can turn it into a verified connection.
#[derive(Clone, Debug)]
pub struct Profile {
    item: Item,
    binding: Option<Binding>,
}

impl Profile {
    /// Decode a bounded `profiles show` reply and validate its binding.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > RESPONSE_LIMIT {
            return Err(Error::Invalid);
        }
        served::show(bytes)
    }

    /// Read metadata without probing or starting the provider.
    pub fn item(&self) -> &Item {
        &self.item
    }

    /// Capture the entry's identity without probing the provider. This
    /// expectation alone does not authorize a connection.
    pub fn expected_identity(&self) -> Result<Identity, Error> {
        let binding = self.binding.as_ref().ok_or(Error::NotReady)?;
        Ok(Identity {
            profile_id: self.item.profile_id.clone(),
            installation_id: binding.installation_id.clone(),
            lifecycle_id: binding.lifecycle_id.clone(),
        })
    }

    /// Authenticate and verify the exact service on the profile's socket.
    /// Runs on an I/O worker; does not discover or start a service.
    pub fn connect(&self) -> Result<Connection, Error> {
        let binding = self.binding.as_ref().ok_or(Error::NotReady)?;
        supported(&binding.socket)?;
        let mut connection = Connection {
            identity: Identity {
                profile_id: self.item.profile_id.clone(),
                installation_id: binding.installation_id.clone(),
                lifecycle_id: binding.lifecycle_id.clone(),
            },
            instance: binding.instance.clone(),
            socket: binding.socket.clone(),
            tmux: binding.tmux.clone(),
            connection_key: ConnectionKey(Arc::new(())),
            local_machine: served::LocalMachine::default(),
        };
        connection.local_machine = connection.handshake(Some(&binding.instance))?;
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

/// Immutable identity and endpoint snapshot for one connection's work.
#[derive(Clone)]
pub struct Connection {
    instance: String,
    identity: Identity,
    socket: PathBuf,
    tmux: Tmux,
    connection_key: ConnectionKey,
    local_machine: served::LocalMachine,
}

impl Connection {
    /// Cache identity shared only by clones of this verified snapshot.
    /// Fresh connections must not reuse workers of an older connection.
    pub fn connection_key(&self) -> ConnectionKey {
        self.connection_key.clone()
    }

    /// Whether `other` verified the same identity and endpoints, so either can
    /// stand in for the other.
    pub fn same_snapshot(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.instance == other.instance
            && self.socket == other.socket
            && self.tmux == other.tmux
    }

    /// The authenticated identity retained by every operation on this connection.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Whether the connected machine reported itself as the profile's home.
    pub fn is_home(&self) -> Option<bool> {
        self.local_machine.home
    }

    /// Display name of the connected machine from `GET /api/profile`, when reported.
    pub fn machine_name(&self) -> Option<&str> {
        self.local_machine.name.as_deref()
    }

    /// The tmux socket the service's entry names for this profile.
    pub fn tmux_socket(&self) -> &Path {
        &self.tmux.socket_path
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
        // Swarms only group agents, so their own failure clears them. A refused
        // binding is about the connection and is never hidden.
        let swarms = match self.response("/api/swarms", None, RESPONSE_LIMIT) {
            Err(error @ (Error::BindingRequired | Error::BindingMismatch)) => return Err(error),
            Err(_) => None,
            Ok(response) => successful(&response).is_ok().then_some(response),
        };
        snapshot.update_swarms(swarms.as_ref().map(|response| response.body.as_slice()));
        // Machine status comes from the profile's services endpoint; an older
        // service without it leaves machines unread without failing the roster.
        let services = match self.response("/api/status/services", None, RESPONSE_LIMIT) {
            Err(error @ (Error::BindingRequired | Error::BindingMismatch)) => return Err(error),
            Err(_) => None,
            Ok(response) => successful(&response).is_ok().then_some(response),
        };
        snapshot.update_machines(services.as_ref().map(|response| response.body.as_slice()));
        Ok(snapshot)
    }

    /// Ask the provider to ensure this pane's shell and return its session name,
    /// which lives on this connection's tmux socket. The provider owns the shell's
    /// environment and tmux server; the name is opaque. Runs on an I/O worker.
    pub fn ensure_shell(&self, shell_id: &str, cwd: Option<&str>) -> Result<String, Error> {
        // The socket is pinned, so the profile alone tells a foreign reply.
        #[derive(Deserialize)]
        struct Reply {
            profile_id: String,
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
        successful(&response)?;
        let reply: Reply = parse(&response.body)?;
        if reply.profile_id != self.identity.profile_id {
            return Err(Error::BindingMismatch);
        }
        if reply.shell_id != shell_id || reply.session.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(reply.session)
    }

    /// End the shell [`Connection::ensure_shell`] opened as `shell_id`. The
    /// service answers alike whether it still ran or had ended already. Runs
    /// on an I/O worker.
    pub fn end_shell(&self, shell_id: &str) -> Result<(), Error> {
        if !is_shell(&format!("{SHELL}{shell_id}")) {
            return Err(Error::Invalid);
        }
        let path = format!("/api/profile/shells/{shell_id}");
        successful(&self.write("DELETE", &path, None, None)?)
    }

    /// Ask the service for a session that signs a CLI account in for `runtime`
    /// inside this profile, and return its name on this connection's tmux socket.
    /// A repeat reuses the session while it is live; see
    /// [`Connection::start_sign_in`]. A refusal keeps its status. Runs on an I/O
    /// worker.
    pub fn sign_in_session(&self, runtime: &str) -> Result<String, Error> {
        self.start_sign_in(runtime)
            .map(|started| started.session)
            .map_err(accounts::Failure::error)
    }

    /// Read whether `runtime` holds a stored sign-in in this profile, as
    /// [`Connection::stored_login`] reads it. A stored sign-in doesn't prove the
    /// account works. Runs on an I/O worker.
    pub fn account_status(&self, runtime: &str) -> Result<runtime_catalog::Authentication, Error> {
        use runtime_catalog::Authentication;
        self.stored_login(runtime)
            .map(|stored| match stored.storage {
                accounts::Storage::Present => Authentication::Authenticated,
                accounts::Storage::Missing => Authentication::NotAuthenticated,
                accounts::Storage::Unavailable => Authentication::Unavailable,
            })
            .map_err(accounts::Failure::error)
    }

    /// Start this stopped agent, continuing its saved conversation or opening a new
    /// one. The provider's start route takes the name only, so a replacement under
    /// the same name would start instead. Runs on an I/O worker.
    pub fn start_agent(&self, name: &str, resume: bool) -> crate::lifecycle::Outcome {
        use crate::lifecycle::{Outcome, start_response};
        if !crate::valid_agent(name) {
            return Outcome::Refused("This session is not a registered agent.".into());
        }
        let body = serde_json::json!({ "resume": resume, "wait": false });
        let Ok(body) = serde_json::to_vec(&body) else {
            return Outcome::Unknown;
        };
        let reply = match self.response(
            &format!("/api/agents/{name}/start"),
            Some(body),
            RESPONSE_LIMIT,
        ) {
            Ok(response) => Ok((response.status, response.body)),
            Err(Error::BindingRequired | Error::BindingMismatch) => {
                return Outcome::Refused(
                    "The original profile binding was refused. Reconnect to that profile.".into(),
                );
            }
            Err(_) => Err(()),
        };
        start_response(name, resume, reply)
    }

    /// Stop this registered agent through the profile service, whether local or
    /// on another machine. When `expected_registration_id` is given, the service
    /// stops only that durable registration. Runs on an I/O worker.
    pub fn stop_agent(
        &self,
        name: &str,
        expected_registration_id: Option<&str>,
    ) -> crate::lifecycle::ActionOutcome {
        use crate::lifecycle::{ActionOutcome, stop_response};
        if !crate::valid_agent(name) {
            return ActionOutcome::Refused("This session is not a registered agent.".into());
        }
        let query = match expected_registration_id {
            Some(registration) if runtime_context::valid_registration(registration) => {
                format!("?expected_registration_id={registration}")
            }
            Some(_) => {
                return ActionOutcome::Refused(
                    "This agent's registration could not be verified.".into(),
                );
            }
            None => String::new(),
        };
        let reply = match self.response_within(
            &format!("/api/agents/{name}/stop{query}"),
            Some(Vec::new()),
            RESPONSE_LIMIT,
            Duration::from_secs(45),
        ) {
            Ok(response) => Ok((response.status, response.body)),
            Err(Error::BindingRequired | Error::BindingMismatch) => {
                return ActionOutcome::Refused(
                    "The original profile binding was refused. Reconnect to that profile.".into(),
                );
            }
            Err(_) => Err(()),
        };
        stop_response(name, reply)
    }

    /// Ask the profile service to stop this registered agent and start its
    /// replacement, whether local or on another machine. Runs on an I/O worker.
    pub fn restart_agent(&self, name: &str, resume: bool) -> crate::lifecycle::ActionOutcome {
        use crate::lifecycle::{ActionOutcome, restart_response};
        if !crate::valid_agent(name) {
            return ActionOutcome::Refused("This session is not a registered agent.".into());
        }
        let body = serde_json::json!({ "resume": resume, "start": true });
        let Ok(body) = serde_json::to_vec(&body) else {
            return ActionOutcome::Unknown;
        };
        let reply = match self.response(
            &format!("/api/agents/{name}/restart"),
            Some(body),
            RESPONSE_LIMIT,
        ) {
            Ok(response) => Ok((response.status, response.body)),
            Err(Error::BindingRequired | Error::BindingMismatch) => {
                return ActionOutcome::Refused(
                    "The original profile binding was refused. Reconnect to that profile.".into(),
                );
            }
            Err(_) => Err(()),
        };
        restart_response(name, reply)
    }

    /// Stop this agent's session and remove it from this profile, only while
    /// `registration` is still its durable registration. The provider checks it
    /// under the agent's lifecycle lock and refuses to remove a running agent, so
    /// the stop comes first. A forgotten agent is refused as `RegistrationChanged`;
    /// a 404 counts as removed. Runs on an I/O worker.
    pub fn remove_agent(&self, name: &str, registration: &str) -> Result<(), Error> {
        if !crate::valid_agent(name) || !runtime_context::valid_registration(registration) {
            return Err(Error::Invalid);
        }
        let query = format!("expected_registration_id={registration}");
        // Stop can wait for an in-flight start and for the workspace to be sealed.
        let response = self.response_within(
            &format!("/api/agents/{name}/stop?{query}"),
            Some(Vec::new()),
            RESPONSE_LIMIT,
            Duration::from_secs(45),
        )?;
        if response.status == 404 {
            return Ok(());
        }
        removal_step(&response)?;
        let response = local::profile_delete(
            self.socket.to_str().ok_or(Error::Invalid)?,
            &format!("/api/agents/{name}?{query}"),
            &self.identity.profile_id,
            RESPONSE_LIMIT,
            Duration::from_secs(15),
        )
        .map_err(|_| Error::Unavailable)?;
        removal_step(&response)
    }

    /// Copy the skill folder at `path` into this profile's store for every agent
    /// in the profile, leaving the folder in place. Runs on an I/O worker.
    pub fn add_skill(&self, path: &Path) -> Result<AddedSkill, AddError> {
        let body = serde_json::to_vec(&served::skill(path)?).map_err(|_| Error::Invalid)?;
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

    /// Verify again on the same binding, never by reading the profile list again.
    pub fn verify(&self) -> Result<(), Error> {
        self.handshake(None).map(|_| ())
    }

    fn handshake(&self, instance: Option<&str>) -> Result<served::LocalMachine, Error> {
        let response = self.response("/api/profile", None, RESPONSE_LIMIT)?;
        successful(&response)?;
        served::verify(&response.body, &self.identity, instance)
    }

    // Every request names the profile, including requests after a successful
    // probe. Content adapters use this connection for every request.
    pub(crate) fn response(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<local::Response, Error> {
        self.response_within(path, body, limit, Duration::from_secs(15))
    }

    /// As [`Self::response`], waiting at most `wait` for each attempt.
    fn response_within(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
        limit: usize,
        wait: Duration,
    ) -> Result<local::Response, Error> {
        // The agent list refuses a read until its next refresh after an agent is
        // removed. That is a moment to read again, not a failure to show anyone.
        // Callers are I/O workers, so waiting here holds no interface thread.
        let mut retries = if path == "/api/agents" { 40 } else { 0 };
        let response = loop {
            let response = local::profile_response(
                self.socket.to_str().ok_or(Error::Invalid)?,
                path,
                &self.identity.profile_id,
                body.clone(),
                limit,
                wait,
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

    /// Change a record on this binding with `method`, under `request_id` when
    /// one is given so a repeat after a lost reply gets the first result.
    /// Refusals of the binding are typed as for reads. Runs on an I/O worker.
    pub(crate) fn write(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Vec<u8>>,
        request_id: Option<&str>,
    ) -> Result<local::Response, Error> {
        self.write_within(method, path, body, request_id, Duration::from_secs(15))
    }

    fn write_within(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Vec<u8>>,
        request_id: Option<&str>,
        timeout: Duration,
    ) -> Result<local::Response, Error> {
        let response = local::profile_write(
            self.socket.to_str().ok_or(Error::Invalid)?,
            method,
            path,
            &self.identity.profile_id,
            body,
            request_id,
            RESPONSE_LIMIT,
            timeout,
        )
        .map_err(|_| Error::Unavailable)?;
        if response.body.len() > RESPONSE_LIMIT {
            return Err(Error::Invalid);
        }
        if let Some(error) = binding_refusal(&response) {
            return Err(error);
        }
        Ok(response)
    }
}

/// What the name of a pane's shell starts with, before its pane's ID.
const SHELL: &str = "shell-";

/// Whether `session` is named as a pane's shell is: `shell-` and 12
/// lowercase hexadecimal digits. Each belongs to the pane it was opened for,
/// so it is never a session to attach on its own.
pub fn is_shell(session: &str) -> bool {
    session.strip_prefix(SHELL).is_some_and(|id| {
        id.len() == 12 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Refuse a service older than [`MINIMUM_BACKBONE`], or one whose `GET /health`
/// gives no version this app can read. `/health` names no profile.
fn supported(socket: &Path) -> Result<(), Error> {
    let response = local::service_response(
        socket.to_str().ok_or(Error::Invalid)?,
        "/health",
        RESPONSE_LIMIT,
        Duration::from_secs(15),
    )
    .map_err(|_| Error::Unavailable)?;
    // A service without `/health` predates the version check.
    if response.status == 404 {
        return Err(Error::VersionUnknown);
    }
    if response.body.len() > RESPONSE_LIMIT {
        return Err(Error::Invalid);
    }
    successful(&response)?;
    #[derive(Deserialize)]
    struct Health {
        version: String,
    }
    let version = serde_json::from_slice::<Health>(&response.body)
        .ok()
        .and_then(|health| semver::Version::parse(&health.version).ok())
        .ok_or(Error::VersionUnknown)?;
    // Full semantic precedence: a pre-release of the minimum is older than it.
    if version < MINIMUM_BACKBONE {
        tracing::debug!(event = "provider_version_outdated", %version, minimum = %MINIMUM_BACKBONE, "Provider older than the supported minimum");
        return Err(ReportedVersion::new(&version).map_or(Error::VersionUnknown, Error::Outdated));
    }
    Ok(())
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
    // The service's reason is the message beside its code.
    let detail = refusal["detail"]["message"].as_str().unwrap_or_default();
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

/// A 404 counts as removed; a replaced or forgotten registration is refused.
fn removal_step(response: &local::Response) -> Result<(), Error> {
    if response.status == 404 {
        return Ok(());
    }
    if registrations_settling(response) {
        return Err(Error::RegistrationChanged);
    }
    successful(response)
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

#[cfg(test)]
mod tests;
