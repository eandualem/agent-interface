//! A served profile's logins to its agents' CLIs on this machine, on the Rust
//! service's own routes. `GET /api/profile/accounts/status` reads whether a
//! login is stored and runs nothing. `POST /api/profile/accounts/sign-in`
//! starts the CLI's own sign-in in a session on the profile's tmux server.
//! `POST /api/profile/accounts/check` asks the CLI itself. A CLI is named by
//! its runtime's ID, such as `codex`; one the service doesn't know is its
//! refusal. Nothing here is stored.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{AddError, Connection, Error, parse};
use crate::{RESPONSE_LIMIT, transport::local};

const STATUS: &str = "/api/profile/accounts/status";
const SIGN_IN: &str = "/api/profile/accounts/sign-in";
const CHECK: &str = "/api/profile/accounts/check";
/// How long a check is waited for. The service ends one after 30 seconds.
const CHECK_WAIT: Duration = Duration::from_secs(35);

/// Whether a CLI's login is stored, as its files' metadata tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    /// A login is stored, which doesn't prove the account works.
    Present,
    /// No login is stored.
    Missing,
    /// What is stored can't be told, such as a login in the macOS keychain.
    Unavailable,
}

/// The service's answer about a CLI's stored login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Whether a login is stored.
    pub storage: Storage,
    /// A line for a person, which any answer may carry.
    pub message: Option<String>,
}

/// A sign-in the service started, or found running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    /// Its session on this connection's tmux socket, `sign-in-<runtime>`.
    pub session: String,
    /// False when it was already running.
    pub created: bool,
}

/// What a CLI said when it was asked whether it is signed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// It reports a login it can use, which doesn't prove the provider
    /// still takes it.
    SignedIn,
    /// It says it isn't signed in.
    SignedOut,
    /// No answer that can be taken, or one this app doesn't know.
    #[serde(other)]
    Unknown,
}

/// A check's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// What the CLI said.
    pub answer: Answer,
    /// Why, for a person; the service gives it with `Unknown`.
    pub message: Option<String>,
    /// When it was checked, as the service wrote it, in RFC 3339.
    pub checked_at: String,
}

/// Why a login couldn't be read, started or checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A check of this CLI is already running.
    Running,
    /// The service refused it with this status, and its reason to show the person.
    Refused(u16, String),
    /// The connection failed, the answer couldn't be read, or the service
    /// refused without a reason.
    Connection(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Connection(error)
    }
}

impl Failure {
    /// Its bounded category, keeping a refusal's status, for a caller that
    /// words refusals itself.
    pub(super) fn error(self) -> Error {
        match self {
            Self::Running => Error::Refused(409),
            Self::Refused(status, _) => Error::Refused(status),
            Self::Connection(error) => error,
        }
    }
}

/// The body of a sign-in or a check: the CLI and nothing else, since the
/// service refuses any other field.
#[derive(Serialize)]
struct Asked<'a> {
    schema_version: u32,
    runtime: &'a str,
}

#[derive(Deserialize)]
struct Status {
    profile_id: String,
    runtime: String,
    storage: Storage,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct SignIn {
    profile_id: String,
    runtime: String,
    session: String,
    created: bool,
}

#[derive(Deserialize)]
struct Check {
    profile_id: String,
    runtime: String,
    result: Answer,
    #[serde(default)]
    message: Option<String>,
    checked_at: String,
}

impl Connection {
    /// Whether `runtime` has a login stored in this profile, which the
    /// service reads from metadata without running anything. Runs on an I/O
    /// worker.
    pub fn stored_login(&self, runtime: &str) -> Result<Stored, Failure> {
        let path = format!("{STATUS}?runtime={}", crate::views::encode(runtime));
        let response = self.response(&path, None, RESPONSE_LIMIT)?;
        let reply: Status = answer(&response)?;
        about(self, &reply.profile_id, &reply.runtime, runtime)?;
        Ok(Stored {
            storage: reply.storage,
            message: reply.message,
        })
    }

    /// Start `runtime`'s own sign-in in its session on this profile's tmux
    /// server, or find it running. Started is not signed in. Runs on an I/O
    /// worker.
    pub fn start_sign_in(&self, runtime: &str) -> Result<Started, Failure> {
        let response = self.write("POST", SIGN_IN, Some(asked(runtime)?), None)?;
        let reply: SignIn = answer(&response)?;
        about(self, &reply.profile_id, &reply.runtime, runtime)?;
        // A pane knows a sign-in, and which CLI it was for, by this name.
        if reply.session != format!("sign-in-{runtime}") {
            return Err(Error::Invalid.into());
        }
        Ok(Started {
            session: reply.session,
            created: reply.created,
        })
    }

    /// Ask `runtime` itself whether it is signed in, which may make the
    /// system ask for keychain access. Waits up to 35 seconds. The route
    /// takes no request ID: while a check of the CLI runs, another is
    /// [`Failure::Running`]. Runs on an I/O worker.
    pub fn check_sign_in(&self, runtime: &str) -> Result<Checked, Failure> {
        let body = asked(runtime)?;
        let response = self.write_within("POST", CHECK, Some(body), None, CHECK_WAIT)?;
        if running(&response) {
            return Err(Failure::Running);
        }
        let reply: Check = answer(&response)?;
        about(self, &reply.profile_id, &reply.runtime, runtime)?;
        Ok(Checked {
            answer: reply.result,
            message: reply.message,
            checked_at: reply.checked_at,
        })
    }
}

fn asked(runtime: &str) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(&Asked {
        schema_version: 1,
        runtime,
    })
    .map_err(|_| Error::Invalid)
}

/// A reply must be this profile's, about the CLI that was asked about.
fn about(
    connection: &Connection,
    profile_id: &str,
    runtime: &str,
    asked: &str,
) -> Result<(), Error> {
    if profile_id != connection.identity.profile_id {
        return Err(Error::BindingMismatch);
    }
    if runtime != asked {
        return Err(Error::Invalid);
    }
    Ok(())
}

/// A success's body, or the service's refusal with its status and reason.
/// A body that isn't schema 1, as this app reads it, is unreadable.
fn answer<T: DeserializeOwned>(response: &local::Response) -> Result<T, Failure> {
    match super::added(response) {
        Ok(()) => parse(&response.body).map_err(|_| Error::Invalid.into()),
        Err(AddError::Refused(message)) => Err(Failure::Refused(response.status, message)),
        Err(AddError::Connection(error)) => Err(error.into()),
    }
}

/// The refusal while a check of the same CLI runs: 409 `conflict`.
fn running(response: &local::Response) -> bool {
    response.status == 409
        && serde_json::from_slice::<serde_json::Value>(&response.body)
            .is_ok_and(|refusal| refusal["detail"]["code"] == "conflict")
}

#[cfg(test)]
mod tests;
