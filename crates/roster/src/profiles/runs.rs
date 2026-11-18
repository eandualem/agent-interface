//! Work an agent runs in the background on the Rust service, such as a
//! review, a build or a test. `GET /api/runs` lists only the active runs,
//! queued or running; a run leaves the list when it ends. `GET
//! /api/runs/{run_id}` reads one run, ended or not. An older service has
//! neither route. Nothing here starts, cancels or retries a run.

use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Connection, Error, successful};
use crate::{RESPONSE_LIMIT, Snapshot, is_uuid, within_depth};

/// Runs that left the list read by their ID after one roster read; any
/// more are forgotten.
const LOOKUPS: usize = 8;
/// Ended runs kept, newest first.
const FINISHED: usize = 32;

/// One run, with the fields this app shows, as the service wrote them. An
/// optional field that is mistyped reads as absent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Run {
    /// Its ID, 32 lowercase hex digits.
    pub run_id: String,
    /// The registration of the agent that started it.
    #[serde(default, deserialize_with = "lenient")]
    pub agent_id: Option<String>,
    /// The name of the agent that started it.
    #[serde(default, deserialize_with = "lenient")]
    pub agent: Option<String>,
    /// `review`, `build`, `test`, `other` or a kind this app doesn't know.
    pub kind: String,
    /// One line saying what it does.
    pub label: String,
    /// `queued`, `running`, `finished`, `failed`, `cancelled` or a state
    /// this app doesn't know.
    pub state: String,
    /// What a queued run waits on.
    #[serde(default, deserialize_with = "lenient")]
    pub waiting_on: Option<String>,
    /// When it was queued, in RFC 3339.
    #[serde(default, deserialize_with = "lenient")]
    pub queued_at: Option<String>,
    /// When it started, in RFC 3339.
    #[serde(default, deserialize_with = "lenient")]
    pub started_at: Option<String>,
    /// When it ended, in RFC 3339.
    #[serde(default, deserialize_with = "lenient")]
    pub finished_at: Option<String>,
    /// Its exit code, once it ended with one.
    #[serde(default, deserialize_with = "lenient")]
    pub exit_code: Option<i64>,
    /// Its last progress line.
    #[serde(default, deserialize_with = "lenient")]
    pub progress: Option<String>,
    /// Who reviews, for a review.
    #[serde(default, deserialize_with = "lenient")]
    pub review: Option<Review>,
}

/// Who a review run asked, as the service wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Review {
    /// The CLI it runs in.
    #[serde(deserialize_with = "lenient")]
    pub runtime: Option<String>,
    /// Its model, when one was named.
    #[serde(deserialize_with = "lenient")]
    pub model: Option<String>,
    /// Its reasoning effort, when one was named.
    #[serde(deserialize_with = "lenient")]
    pub effort: Option<String>,
}

/// A value that doesn't decode as `T`, `null` included, reads as absent.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = Box::<RawValue>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).ok())
}

/// The runs a roster worker has seen on its connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Runs {
    /// Queued and running runs, as the last read listed them.
    pub active: Vec<Run>,
    /// Runs that ended while the worker watched them, newest first. Empty
    /// after the app starts, since the service lists only active runs.
    pub finished: Vec<Run>,
}

impl Runs {
    /// The runs of the agent with this registration.
    pub fn of(&self, registration: &str) -> Self {
        let mine = |runs: &[Run]| -> Vec<Run> {
            runs.iter()
                .filter(|run| run.agent_id.as_deref() == Some(registration))
                .cloned()
                .collect()
        };
        Self {
            active: mine(&self.active),
            finished: mine(&self.finished),
        }
    }
}

/// The runs one roster worker follows, read again after its roster reads.
#[derive(Default)]
pub struct Book {
    runs: Runs,
    /// The service answered that it has no runs, so it isn't asked again.
    unserved: bool,
}

impl Book {
    /// The runs as last read.
    pub fn runs(&self) -> &Runs {
        &self.runs
    }

    /// Read the active runs after `roster` was read on `connection`, only
    /// when one of its agents reports runs or some were active at the last
    /// read, so a service without runs is never asked. A run that left the
    /// list is read once by its ID and kept when it ended; one that can't
    /// be read is forgotten. A failed list keeps the book as it was. Runs
    /// on an I/O worker.
    pub fn follow(&mut self, connection: &Connection, roster: &Snapshot) {
        let reported = roster
            .details
            .values()
            .filter_map(|details| details.background)
            .any(|counts| counts.running.saturating_add(counts.queued) > 0);
        if self.unserved || !connection.served() || (!reported && self.runs.active.is_empty()) {
            return;
        }
        let listed = match connection.runs() {
            Ok(listed) => listed,
            Err(Error::Unsupported) => {
                self.unserved = true;
                self.runs.active.clear();
                return;
            }
            Err(_) => return,
        };
        let gone: Vec<Run> = std::mem::take(&mut self.runs.active)
            .into_iter()
            .filter(|run| listed.iter().all(|now| now.run_id != run.run_id))
            .take(LOOKUPS)
            .collect();
        for run in gone {
            let Ok(ended) = connection.run(&run.run_id) else {
                continue;
            };
            if matches!(ended.state.as_str(), "queued" | "running") {
                continue;
            }
            self.runs
                .finished
                .retain(|kept| kept.run_id != ended.run_id);
            self.runs.finished.insert(0, ended);
        }
        self.runs.finished.truncate(FINISHED);
        self.runs.active = listed;
    }
}

impl Connection {
    /// The active runs on this profile's service, queued or running. A
    /// service without runs is `Unsupported`; a run that doesn't decode is
    /// skipped. Runs on an I/O worker.
    pub fn runs(&self) -> Result<Vec<Run>, Error> {
        #[derive(Deserialize)]
        struct Listed {
            items: Vec<Box<RawValue>>,
        }
        if !self.served {
            return Err(Error::Unsupported);
        }
        let response = self.response("/api/runs", None, RESPONSE_LIMIT)?;
        if response.status == 404 {
            return Err(Error::Unsupported);
        }
        successful(&response)?;
        if !within_depth(&response.body) {
            return Err(Error::Invalid);
        }
        let listed: Listed = serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?;
        Ok(listed
            .items
            .into_iter()
            .filter_map(|item| serde_json::from_str(item.get()).ok())
            .collect())
    }

    /// One run by its ID, ended or not. Only an ID of 32 lowercase hex
    /// digits is put in the path. Runs on an I/O worker.
    pub fn run(&self, run_id: &str) -> Result<Run, Error> {
        if !self.served {
            return Err(Error::Unsupported);
        }
        if run_id.len() != 32 || !is_uuid(run_id) {
            return Err(Error::Invalid);
        }
        let response = self.response(&format!("/api/runs/{run_id}"), None, RESPONSE_LIMIT)?;
        successful(&response)?;
        let run: Run = serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?;
        if run.run_id != run_id {
            return Err(Error::Invalid);
        }
        Ok(run)
    }
}

#[cfg(test)]
mod tests;
