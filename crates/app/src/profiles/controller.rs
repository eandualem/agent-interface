//! Durable lifecycle coordination. Recovery never implicitly resubmits intent.

use super::{Client, journal, lifecycle::Operation};
use agent_interface_roster::profiles::{
    Identity,
    operations::{Prepared, Receipt},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LIMIT: usize = journal::LIMIT;

/// Submission intent, independent of provider progress or content readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Exact intent is durable, but this controller has not submitted it.
    Prepared,
    /// Submission may have happened; recovery must look up the original request.
    Submitted,
}

/// Fixed diagnostics; uncertain outcomes always retain the original journal key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid or unsupported durable envelope.
    Invalid,
    /// No record exists under this key.
    Missing,
    /// First submission or explicit retry is inappropriate for this phase.
    WrongPhase,
    /// A terminal receipt is already durable; no further command is needed.
    Completed,
    /// Storage failed or a concurrent observation won. Reread before retrying.
    Storage(journal::Error),
    /// Original caller context or provider invocation could not be validated.
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

/// Durable state only; even a successful terminal result grants no connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Stable journal key to retain across errors and restarts.
    pub key: String,
    /// Current journal revision, useful for identifying stale UI observations.
    pub revision: u64,
    /// Whether submission might already have occurred.
    pub phase: Phase,
    /// The original provider idempotency key.
    pub request_id: String,
    /// Original or reserved tuple; still requires the normal content handshake.
    pub identity: Option<Identity>,
    /// Validated terminal receipt, returned only after durable storage.
    pub terminal: Option<Receipt>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: u8,
    kind: String,
    phase: Phase,
    capsule: String,
}
struct Stored {
    record: journal::Record,
    phase: Phase,
    operation: Operation,
}
impl Stored {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            key: self.record.id().into(),
            revision: self.record.revision(),
            phase: self.phase,
            request_id: self.operation.request_id().into(),
            identity: self.operation.identity().cloned(),
            terminal: self.operation.terminal().cloned(),
        }
    }
}

/// A private journal plus a captured caller. Methods perform blocking I/O and
/// must run off the UI thread. Constructing/restoring never invokes a provider.
pub struct Controller {
    journal: journal::Journal,
    client: Client,
}
impl Controller {
    /// Capture the original command context alongside an already opened journal.
    pub fn new(journal: journal::Journal, client: &Client) -> Self {
        Self {
            journal,
            client: client.clone(),
        }
    }

    /// Stable key for this original request, including after an ambiguous write.
    /// The same request ID cannot be enqueued twice under alternate record names.
    pub fn key(request_id: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"lifecycle\0");
        digest.update(request_id.as_bytes());
        digest.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Save fresh immutable intent before any submission. Existing request IDs
    /// return a conflict: restore their original record instead of replacing it.
    pub fn enqueue(&self, prepared: Prepared) -> Result<Snapshot, Error> {
        if prepared.terminal().is_some() {
            return Err(Error::Completed);
        }
        let operation = Operation::new(&self.client, prepared);
        let key = Self::key(operation.request_id());
        let bytes = encode(Phase::Prepared, &operation)?;
        let record = self.journal.save(&key, None, &bytes)?;
        Ok(Stored {
            record,
            phase: Phase::Prepared,
            operation,
        }
        .snapshot())
    }

    /// Restore validated state without issuing an action or allocating a new ID.
    pub fn read(&self, key: &str) -> Result<Snapshot, Error> {
        Ok(self.load(key)?.snapshot())
    }

    /// First submission only. The Submitted phase is synced before invocation.
    pub fn submit(&self, key: &str, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.invoke(key, Invocation::Submit, live)
    }

    /// Read the original receipt after an uncertain/pending submission. This is
    /// the default recovery action and never reissues the original mutation.
    pub fn refresh(&self, key: &str, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.invoke(key, Invocation::Refresh, live)
    }

    /// Explicitly resend identical bytes under the original ID and caller only.
    /// Provider idempotency is required, including for concurrent explicit retries.
    pub fn retry(&self, key: &str, live: &dyn Fn() -> bool) -> Result<Receipt, Error> {
        self.invoke(key, Invocation::Retry, live)
    }

    fn load(&self, key: &str) -> Result<Stored, Error> {
        let record = self.journal.read_durable(key)?.ok_or(Error::Missing)?;
        let envelope: Envelope =
            serde_json::from_slice(record.payload()).map_err(|_| Error::Invalid)?;
        if envelope.format != 1
            || envelope.kind != "lifecycle"
            || serde_json::to_vec(&envelope).map_err(|_| Error::Invalid)? != record.payload()
        {
            return Err(Error::Invalid);
        }
        let operation = Operation::restore(&self.client, envelope.capsule.as_bytes())?;
        if Self::key(operation.request_id()) != key
            || (envelope.phase == Phase::Prepared && operation.terminal().is_some())
        {
            return Err(Error::Invalid);
        }
        Ok(Stored {
            record,
            phase: envelope.phase,
            operation,
        })
    }

    fn invoke(
        &self,
        key: &str,
        invocation: Invocation,
        live: &dyn Fn() -> bool,
    ) -> Result<Receipt, Error> {
        let mut stored = self.load(key)?;
        if stored.operation.terminal().is_some() {
            return Err(Error::Completed);
        }
        let expected_phase = match invocation {
            Invocation::Submit => Phase::Prepared,
            Invocation::Refresh | Invocation::Retry => Phase::Submitted,
        };
        if stored.phase != expected_phase {
            return Err(Error::WrongPhase);
        }
        if !live() {
            return Err(Error::Client(super::Error::Cancelled));
        }
        // Claim a new revision for every invocation. A concurrent result can
        // never overwrite another claim, reservation or terminal observation.
        let bytes = encode(Phase::Submitted, &stored.operation)?;
        let claimed = self
            .journal
            .save(key, Some(stored.record.revision()), &bytes)?;
        let receipt = match invocation {
            Invocation::Submit | Invocation::Retry => stored.operation.apply(live),
            Invocation::Refresh => stored.operation.refresh(live),
        }?;
        let bytes = encode(Phase::Submitted, &stored.operation)?;
        self.journal.save(key, Some(claimed.revision()), &bytes)?;
        // No provider receipt escapes before its retained progress is durable.
        Ok(receipt)
    }
}

enum Invocation {
    Submit,
    Refresh,
    Retry,
}

fn encode(phase: Phase, operation: &Operation) -> Result<Vec<u8>, Error> {
    let capsule = String::from_utf8(operation.checkpoint()?).map_err(|_| Error::Invalid)?;
    let envelope = Envelope {
        format: 1,
        kind: "lifecycle".into(),
        phase,
        capsule,
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|_| Error::Invalid)?;
    if bytes.len() > LIMIT {
        return Err(Error::Storage(journal::Error::TooLarge));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
