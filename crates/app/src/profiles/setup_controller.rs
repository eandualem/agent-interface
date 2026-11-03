//! Durable first-agent setup and exact cancellation recovery.

use super::{Client, journal, setup::Setup};
use agent_interface_roster::profiles::{
    Identity,
    setup::{Action, Event, Prepared},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Last durably claimed action; an interrupted cancellation cannot become setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Intent is saved, without a submitted setup action.
    Prepared,
    /// First-agent setup may have been submitted.
    Submitted,
    /// Cancellation may have been submitted; recover that exact action first.
    CancelPending,
}

/// Fixed diagnostics never include request bodies, provider prose or paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid or unsupported stored envelope.
    Invalid,
    /// No record exists under this key.
    Missing,
    /// This action does not match the durable submission state.
    WrongPhase,
    /// Confirmed readiness or safe cancellation already closed the request.
    Completed,
    /// The validated provider result does not permit a same-request retry.
    RetryRefused,
    /// Storage failed or another revision won; reread the original key.
    Storage(journal::Error),
    /// Caller context or provider invocation could not be validated.
    Client(super::Error),
}
impl From<journal::Error> for Error {
    fn from(error: journal::Error) -> Self {
        Self::Storage(error)
    }
}
impl From<super::Error> for Error {
    fn from(error: super::Error) -> Self {
        Self::Client(error)
    }
}

/// Durable observations only; neither readiness nor cancellation proves account access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Stable private journal key.
    pub key: String,
    /// Revision used to reject stale deferred UI observations.
    pub revision: u64,
    /// Last action that may have been submitted.
    pub phase: Phase,
    /// Original full expected origin.
    pub identity: Identity,
    /// Original request ID shared with cancellation.
    pub request_id: String,
    /// Latest durable first-agent result, absent during an unknown attempt.
    pub first_result: Option<Event>,
    /// Latest durable cancellation result, absent during an unknown cancel attempt.
    pub cancellation_result: Option<Event>,
}

/// Nonterminal progress stamped with its claimed durable revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Journal record that owns this update.
    pub key: String,
    /// Consumers must reject progress from superseded revisions.
    pub revision: u64,
    /// Validated step; terminal results are returned only after durable storage.
    pub event: Event,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: u8,
    kind: String,
    phase: Phase,
    capsule: String,
    first: Option<String>,
    cancel: Option<String>,
}
struct Stored {
    record: journal::Record,
    phase: Phase,
    setup: Setup,
    first: Option<Event>,
    cancel: Option<Event>,
}
impl Stored {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            key: self.record.id().into(),
            revision: self.record.revision(),
            phase: self.phase,
            identity: self.setup.identity().clone(),
            request_id: self.setup.request_id().into(),
            first_result: self.first.clone(),
            cancellation_result: self.cancel.clone(),
        }
    }
    fn closed(&self) -> bool {
        self.first
            .as_ref()
            .is_some_and(|event| event.ready_agent.is_some())
            || self.cancel.as_ref().is_some_and(|event| event.cancelled)
    }
}

/// Blocking coordinator for an original setup request and captured caller.
/// Invoke off the UI thread. Construction and restoration execute no command.
pub struct Controller {
    journal: journal::Journal,
    client: Client,
}
impl Controller {
    /// Capture a caller alongside a previously opened private journal.
    pub fn new(journal: journal::Journal, client: &Client) -> Self {
        Self {
            journal,
            client: client.clone(),
        }
    }

    /// Stable key to retain even when a storage result is ambiguous.
    pub fn key(request_id: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"setup\0");
        digest.update(request_id.as_bytes());
        digest.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Persist exact setup and cancellation intent before any invocation.
    pub fn enqueue(&self, prepared: Prepared) -> Result<Snapshot, Error> {
        let setup = Setup::new(&self.client, prepared);
        let key = Self::key(setup.request_id());
        let bytes = encode(Phase::Prepared, &setup, None, None)?;
        let record = self.journal.save(&key, None, &bytes)?;
        Ok(Stored {
            record,
            phase: Phase::Prepared,
            setup,
            first: None,
            cancel: None,
        }
        .snapshot())
    }

    /// Restore after a fresh durability barrier, without retrying either action.
    pub fn read(&self, key: &str) -> Result<Snapshot, Error> {
        Ok(self.load(key)?.snapshot())
    }

    /// First submission only; a saved submission claim precedes execution.
    pub fn submit(
        &self,
        key: &str,
        live: &dyn Fn() -> bool,
        emit: impl FnMut(Progress),
    ) -> Result<Event, Error> {
        self.invoke(key, Invocation::Submit, live, emit)
    }
    /// Explicit same-request setup retry, governed by the retained retry policy.
    /// An unknown cancellation must be recovered before resuming setup.
    pub fn retry(
        &self,
        key: &str,
        live: &dyn Fn() -> bool,
        emit: impl FnMut(Progress),
    ) -> Result<Event, Error> {
        self.invoke(key, Invocation::Retry, live, emit)
    }
    /// First explicit cancellation of the original submitted setup request.
    pub fn cancel(
        &self,
        key: &str,
        live: &dyn Fn() -> bool,
        emit: impl FnMut(Progress),
    ) -> Result<Event, Error> {
        self.invoke(key, Invocation::Cancel, live, emit)
    }
    /// Recover a lost cancellation reply using only its original exact body.
    pub fn retry_cancel(
        &self,
        key: &str,
        live: &dyn Fn() -> bool,
        emit: impl FnMut(Progress),
    ) -> Result<Event, Error> {
        self.invoke(key, Invocation::RetryCancel, live, emit)
    }

    fn load(&self, key: &str) -> Result<Stored, Error> {
        let record = self.journal.read_durable(key)?.ok_or(Error::Missing)?;
        let envelope: Envelope =
            serde_json::from_slice(record.payload()).map_err(|_| Error::Invalid)?;
        if envelope.format != 1
            || envelope.kind != "setup"
            || serde_json::to_vec(&envelope).map_err(|_| Error::Invalid)? != record.payload()
        {
            return Err(Error::Invalid);
        }
        let setup = Setup::restore(&self.client, envelope.capsule.as_bytes())?;
        if Self::key(setup.request_id()) != key
            || (envelope.phase == Phase::Prepared
                && (envelope.first.is_some() || envelope.cancel.is_some()))
        {
            return Err(Error::Invalid);
        }
        let first = envelope
            .first
            .as_ref()
            .map(|bytes| setup.restore_outcome(Action::FirstAgent, bytes.as_bytes()))
            .transpose()?;
        let cancel = envelope
            .cancel
            .as_ref()
            .map(|bytes| setup.restore_outcome(Action::CancelFirstAgent, bytes.as_bytes()))
            .transpose()?;
        Ok(Stored {
            record,
            phase: envelope.phase,
            setup,
            first,
            cancel,
        })
    }

    fn invoke(
        &self,
        key: &str,
        invocation: Invocation,
        live: &dyn Fn() -> bool,
        mut emit: impl FnMut(Progress),
    ) -> Result<Event, Error> {
        let mut stored = self.load(key)?;
        if stored.closed() {
            return Err(Error::Completed);
        }
        let action = match invocation {
            Invocation::Submit => {
                if stored.phase != Phase::Prepared {
                    return Err(Error::WrongPhase);
                }
                Action::FirstAgent
            }
            Invocation::Retry => {
                if stored.phase == Phase::Prepared {
                    return Err(Error::WrongPhase);
                }
                if stored.phase == Phase::CancelPending
                    && !stored.cancel.as_ref().is_some_and(|event| {
                        event.status == "action_required"
                            && event.code.as_deref() == Some("cancellation_unsafe")
                    })
                {
                    return Err(Error::WrongPhase);
                }
                retry_allowed(stored.first.as_ref())?;
                Action::FirstAgent
            }
            Invocation::Cancel => {
                if stored.phase != Phase::Submitted || stored.cancel.is_some() {
                    return Err(Error::WrongPhase);
                }
                Action::CancelFirstAgent
            }
            Invocation::RetryCancel => {
                if stored.phase == Phase::Prepared
                    || (stored.phase != Phase::CancelPending && stored.cancel.is_none())
                {
                    return Err(Error::WrongPhase);
                }
                retry_allowed(stored.cancel.as_ref())?;
                Action::CancelFirstAgent
            }
        };
        if !live() {
            return Err(Error::Client(super::Error::Cancelled));
        }
        match action {
            Action::FirstAgent => {
                stored.phase = Phase::Submitted;
                stored.first = None;
            }
            Action::CancelFirstAgent => {
                stored.phase = Phase::CancelPending;
                stored.cancel = None;
            }
        }
        let bytes = encode(
            stored.phase,
            &stored.setup,
            stored.first.as_ref(),
            stored.cancel.as_ref(),
        )?;
        let claimed = self
            .journal
            .save(key, Some(stored.record.revision()), &bytes)?;
        let event = stored.setup.run(action, live, |event| {
            emit(Progress {
                key: key.into(),
                revision: claimed.revision(),
                event,
            })
        })?;
        match action {
            Action::FirstAgent => stored.first = Some(event.clone()),
            Action::CancelFirstAgent => stored.cancel = Some(event.clone()),
        };
        let bytes = encode(
            stored.phase,
            &stored.setup,
            stored.first.as_ref(),
            stored.cancel.as_ref(),
        )?;
        self.journal.save(key, Some(claimed.revision()), &bytes)?;
        Ok(event)
    }
}

enum Invocation {
    Submit,
    Retry,
    Cancel,
    RetryCancel,
}
fn retry_allowed(event: Option<&Event>) -> Result<(), Error> {
    if event.is_some_and(|event| event.retry.as_deref() != Some("same_request")) {
        Err(Error::RetryRefused)
    } else {
        Ok(())
    }
}
fn encode(
    phase: Phase,
    setup: &Setup,
    first: Option<&Event>,
    cancel: Option<&Event>,
) -> Result<Vec<u8>, Error> {
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).map_err(|_| Error::Invalid);
    let outcome =
        |event: &Event| -> Result<String, Error> { text(setup.outcome_checkpoint(event)?) };
    let envelope = Envelope {
        format: 1,
        kind: "setup".into(),
        phase,
        capsule: text(setup.checkpoint()?)?,
        first: first.map(outcome).transpose()?,
        cancel: cancel.map(outcome).transpose()?,
    };
    serde_json::to_vec(&envelope).map_err(|_| Error::Invalid)
}

#[cfg(test)]
mod tests;
