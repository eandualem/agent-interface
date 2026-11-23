//! Work an agent runs in the background on the Rust service, such as a
//! review, a build or a test. `GET /api/runs` lists only the active runs,
//! queued or running; a run leaves the list when it ends. `GET
//! /api/runs/{run_id}` reads one run, ended or not. An older service has
//! neither route. Nothing here starts, cancels or retries a run.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Connection, Error, successful};
use crate::{RESPONSE_LIMIT, Snapshot, is_uuid, object, views::epoch_seconds, within_depth};

/// Runs that left the list read by their ID after one roster read; any
/// more are forgotten.
const LOOKUPS: usize = 8;
/// Ended runs kept, newest first.
const FINISHED: usize = 32;
/// How long one runs read may take; they are optional, so they are not
/// waited for as long as the roster.
const WAIT: Duration = Duration::from_secs(3);
/// How long one runs round may take, counted from when its list read
/// starts: no lookup starts after it, and each waits at most what is left.
const LOOKING: Duration = Duration::from_secs(5);
/// How long after its last good read a list is still shown, as the last
/// good roster is.
const STALE: Duration = Duration::from_secs(30);
/// How often ended runs that still name a view are read again, so a view
/// the service has ended stops being offered.
const VIEWS: Duration = Duration::from_secs(30);

/// One run, with the fields this app shows, as the service wrote them. An
/// optional field that is mistyped reads as absent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Run {
    /// Its ID, 32 lowercase hex digits.
    pub run_id: String,
    /// The registration of the agent that started it.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub agent_id: Option<String>,
    /// The name of the agent that started it.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub agent: Option<String>,
    /// `review`, `build`, `test`, `other` or a kind this app doesn't know.
    pub kind: String,
    /// One line saying what it does.
    pub label: String,
    /// `queued`, `running`, `finished`, `failed`, `cancelled` or a state
    /// this app doesn't know.
    pub state: String,
    /// What a queued run waits on.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub waiting_on: Option<String>,
    /// When it was queued, in RFC 3339.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub queued_at: Option<String>,
    /// When it started, in RFC 3339.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub started_at: Option<String>,
    /// When it ended, in RFC 3339.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub finished_at: Option<String>,
    /// Its exit code, once it ended with one.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub exit_code: Option<i64>,
    /// Its last progress line.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub progress: Option<String>,
    /// The session on the profile's tmux server of the agent's machine that
    /// shows a copy of what it prints, while that session lasts.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub session: Option<String>,
    /// Who reviews, for a review.
    #[serde(default, deserialize_with = "crate::lenient_object")]
    pub review: Option<Review>,
}

impl Run {
    /// The session showing its output, only when the service named this
    /// run's own view.
    pub fn view(&self) -> Option<&str> {
        self.session.as_deref().filter(|session| {
            is_view(session) && session.strip_prefix(VIEW) == Some(self.run_id.as_str())
        })
    }
}

/// What the name of a run's view starts with, before the run's ID.
const VIEW: &str = "run-";

/// Whether `session` is named as a run's view is: `run-` and a run's ID. The
/// service keeps these on a profile's tmux server; they are not shells.
pub fn is_view(session: &str) -> bool {
    session.strip_prefix(VIEW).is_some_and(|id| {
        id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Who a review run asked, as the service wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Review {
    /// The CLI it runs in.
    #[serde(deserialize_with = "crate::lenient")]
    pub runtime: Option<String>,
    /// Its model, when one was named.
    #[serde(deserialize_with = "crate::lenient")]
    pub model: Option<String>,
    /// Its reasoning effort, when one was named.
    #[serde(deserialize_with = "crate::lenient")]
    pub effort: Option<String>,
}

/// The runs a roster worker has seen on its connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Runs {
    /// Queued and running runs, as the last read listed them.
    pub active: Vec<Run>,
    /// Runs that ended while the worker watched them, newest first. Empty
    /// after the app starts, since the service lists only active runs.
    pub finished: Vec<Run>,
    /// The list has failed to read for a while, so `active` may no longer be
    /// what runs and isn't shown.
    pub stale: bool,
}

impl Runs {
    /// The runs of the agent with this registration; no active ones while
    /// the list is stale.
    pub fn of(&self, registration: &str) -> Self {
        let mine = |runs: &[Run]| -> Vec<Run> {
            runs.iter()
                .filter(|run| run.agent_id.as_deref() == Some(registration))
                .cloned()
                .collect()
        };
        Self {
            active: if self.stale {
                Vec::new()
            } else {
                mine(&self.active)
            },
            finished: mine(&self.finished),
            stale: self.stale,
        }
    }
}

/// The runs one roster worker follows, read again after its roster reads.
#[derive(Default)]
pub struct Book {
    runs: Runs,
    /// The service answered that it has no runs, so it isn't asked again.
    unserved: bool,
    /// When the roster read that the last good list read followed was
    /// observed: the moment both 30 seconds are counted from.
    listed_at: Option<Instant>,
    /// When ended runs with a view were last due to be read again.
    viewed_at: Option<Instant>,
}

/// Runs that left the list at one read, to read by their IDs until `deadline`.
#[derive(Debug, Default)]
pub struct Gone {
    runs: Vec<Run>,
    deadline: Option<Instant>,
}

impl Book {
    /// The runs as last read.
    pub fn runs(&self) -> &Runs {
        &self.runs
    }

    /// Read the active runs after `roster` was read on `connection`, only
    /// when one of its agents reports runs or some were active at the last
    /// read, so a service without runs is never asked. The runs that left
    /// the list come back for [`Self::settle`], so the new list can be shown
    /// first. A failed read keeps the book as it was, and marks the list
    /// stale as [`Self::expire`] does. Runs on an I/O worker.
    pub fn list(&mut self, connection: &Connection, roster: &Snapshot) -> Gone {
        self.list_at(connection, roster, Instant::now())
    }

    fn list_at(&mut self, connection: &Connection, roster: &Snapshot, now: Instant) -> Gone {
        self.review_views(connection, now);
        let reported = roster
            .details
            .values()
            .filter_map(|details| details.background)
            .any(|counts| counts.running.saturating_add(counts.queued) > 0);
        if self.unserved || (!reported && self.runs.active.is_empty()) {
            return Gone::default();
        }
        let deadline = now + LOOKING;
        let listed = match connection.runs() {
            Ok(listed) => listed,
            Err(Error::Unsupported) => {
                self.unserved = true;
                self.runs.active.clear();
                self.runs.stale = false;
                return Gone::default();
            }
            Err(_) => {
                // The list stays inside, so a later read still finds the
                // runs that ended meanwhile.
                self.expire_at(now);
                return Gone::default();
            }
        };
        self.listed_at = Some(roster.observed_at.unwrap_or(now));
        self.runs.stale = false;
        let runs = std::mem::replace(&mut self.runs.active, listed)
            .into_iter()
            .filter(|run| self.runs.active.iter().all(|now| now.run_id != run.run_id))
            .take(LOOKUPS)
            .collect();
        Gone {
            runs,
            deadline: Some(deadline),
        }
    }

    /// Read again, every 30 seconds, each ended run that still names a view,
    /// and forget the view of one the service no longer knows. A failed read
    /// keeps the run as it was.
    fn review_views(&mut self, connection: &Connection, now: Instant) {
        if self
            .viewed_at
            .is_some_and(|at| now.saturating_duration_since(at) < VIEWS)
        {
            return;
        }
        self.viewed_at = Some(now);
        for kept in self
            .runs
            .finished
            .iter_mut()
            .filter(|run| run.view().is_some())
        {
            match connection.run_within(&kept.run_id, WAIT) {
                Ok(run) => *kept = run,
                Err(Error::Refused(404)) => kept.session = None,
                Err(_) => {}
            }
        }
    }

    /// Mark the list stale when it is too old to show. The worker calls it at
    /// the start of every pass, before the pass publishes anything.
    pub fn expire(&mut self) {
        self.expire_at(Instant::now());
    }

    /// The one rule: a book with active runs whose last good list read
    /// followed a roster read 30 seconds or more old is stale; an empty book
    /// never is. A good list read clears it. Evaluated at the start of every
    /// pass and when a list read fails, so every update the worker publishes
    /// has `stale` true exactly when the book has active runs and its last
    /// good list read followed a roster read 30 seconds or more old at the
    /// start of that pass (or, after a failed list read, when that read
    /// started).
    fn expire_at(&mut self, now: Instant) {
        self.runs.stale = !self.runs.active.is_empty()
            && self
                .listed_at
                .is_some_and(|at| now.saturating_duration_since(at) >= STALE);
    }

    /// Read each run that left the list once by its ID and keep those that
    /// ended, newest first. A run that can't be read, or isn't reached
    /// before the deadline or while `live`, is forgotten. Runs on an I/O
    /// worker.
    pub fn settle(&mut self, connection: &Connection, gone: Gone, live: &dyn Fn() -> bool) {
        let Some(deadline) = gone.deadline else {
            return;
        };
        let mut ended = Vec::new();
        for run in gone.runs {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !live() {
                break;
            }
            let Ok(run) = connection.run_within(&run.run_id, left.min(WAIT)) else {
                continue;
            };
            if !matches!(run.state.as_str(), "queued" | "running") {
                ended.push(run);
            }
        }
        self.runs
            .finished
            .retain(|kept| ended.iter().all(|run| run.run_id != kept.run_id));
        ended.append(&mut self.runs.finished);
        // Newest first; an end time that doesn't parse counts as oldest.
        let at = |run: &Run| {
            run.finished_at
                .as_deref()
                .and_then(epoch_seconds)
                .unwrap_or(f64::NEG_INFINITY)
        };
        ended.sort_by(|a, b| {
            at(b)
                .total_cmp(&at(a))
                .then_with(|| b.run_id.cmp(&a.run_id))
        });
        ended.truncate(FINISHED);
        self.runs.finished = ended;
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
        let response = self.response_within("/api/runs", None, RESPONSE_LIMIT, WAIT)?;
        if response.status == 404 {
            return Err(Error::Unsupported);
        }
        successful(&response)?;
        if !within_depth(&response.body) {
            return Err(Error::Invalid);
        }
        let listed: Listed = std::str::from_utf8(&response.body)
            .ok()
            .and_then(object)
            .ok_or(Error::Invalid)?;
        Ok(listed
            .items
            .into_iter()
            .filter_map(|item| object(item.get()))
            .collect())
    }

    /// One run by its ID, ended or not. Only an ID of 32 lowercase hex
    /// digits is put in the path. Runs on an I/O worker.
    pub fn run(&self, run_id: &str) -> Result<Run, Error> {
        self.run_within(run_id, WAIT)
    }

    fn run_within(&self, run_id: &str, wait: Duration) -> Result<Run, Error> {
        if run_id.len() != 32 || !is_uuid(run_id) {
            return Err(Error::Invalid);
        }
        let response =
            self.response_within(&format!("/api/runs/{run_id}"), None, RESPONSE_LIMIT, wait)?;
        successful(&response)?;
        if !within_depth(&response.body) {
            return Err(Error::Invalid);
        }
        let run: Run = std::str::from_utf8(&response.body)
            .ok()
            .and_then(object)
            .ok_or(Error::Invalid)?;
        if run.run_id != run_id {
            return Err(Error::Invalid);
        }
        Ok(run)
    }
}

#[cfg(test)]
mod tests;
