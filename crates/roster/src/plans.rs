//! Agents' current plans, read through Backbone's authenticated local API.
//!
//! A plan is the agent's declared working intent, not evidence of execution.
//! Reads never publish, approve or change a plan. Transport and parsing run
//! on I/O workers, never the UI thread.

use std::{path::Path, time::Duration};

use serde::Deserialize;

/// The most a list page may hold, as the provider bounds plans.
const LIST_LIMIT: usize = 2 * 1024 * 1024;

/// One page of active agents, each with its current plan or none.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PlansPage {
    /// Active registrations, sorted by stable author identity.
    pub items: Vec<Entry>,
    /// Pass as `after_author_id` to continue with the same filter.
    pub next_after_author_id: Option<String>,
    /// When the provider built this page.
    #[serde(default)]
    pub generated_at: String,
}

/// An active agent and its current plan, if it ever published one.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Entry {
    /// Stable author identity.
    pub author_id: String,
    /// Current registered name.
    pub agent_name: String,
    /// The retained current plan; `None` when none was ever published.
    pub record: Option<Record>,
}

/// The current revision of one agent's plan.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Record {
    /// Stable plan identity; a new plan has a new one.
    pub plan_id: String,
    /// This revision; feedback names the revision the person saw.
    pub revision_id: String,
    /// When this revision was published.
    pub updated_at: String,
    /// Seconds since then, as the provider measured.
    #[serde(default)]
    pub age_seconds: u64,
    /// At least a day old; stale is not complete.
    #[serde(default)]
    pub stale: bool,
    /// The plan itself.
    pub plan: Plan,
}

/// A short ordered checklist.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Plan {
    /// What the plan undertakes.
    pub title: String,
    /// Steps in authored order: now, then next, then later.
    pub items: Vec<Item>,
}

/// One step of a plan.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Item {
    /// Stable across revisions while the step continues.
    pub item_id: String,
    /// What the step is.
    pub text: String,
    /// `now`, `next` or `later`.
    pub section: String,
    /// `pending`, `in_progress`, `blocked` or `done`.
    pub status: String,
    /// A blocker or useful context.
    #[serde(default)]
    pub detail: String,
    /// What the step points to, such as the document behind it.
    #[serde(default)]
    pub references: Vec<Reference>,
}

/// Something a step points to.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Reference {
    /// `issue`, `sheet` or `document`.
    pub kind: String,
    /// Which one, as the agent named it.
    pub target: String,
}

impl Item {
    /// Finished.
    pub fn done(&self) -> bool {
        self.status == "done"
    }

    /// Waiting on someone or something.
    pub fn blocked(&self) -> bool {
        self.status == "blocked"
    }

    /// Being worked on.
    pub fn in_progress(&self) -> bool {
        self.status == "in_progress"
    }
}

impl Plan {
    /// Whether any step waits on something.
    pub fn blocked(&self) -> bool {
        self.items.iter().any(Item::blocked)
    }

    /// The step being worked on: the first in progress, else the first not done.
    pub fn current(&self) -> Option<&Item> {
        self.items
            .iter()
            .find(|item| item.in_progress())
            .or_else(|| self.items.iter().find(|item| !item.done()))
    }
}

/// Why plans could not be read. Fixed text; no response body is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No authenticated private local provider is configured.
    Unavailable,
    /// The provider does not offer current plans yet.
    Unsupported,
    /// The response violated the bounded contract.
    Unreadable,
}

impl Error {
    /// Safe, fixed text for the person.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Current plans need an authenticated private Backbone connection.",
            Self::Unsupported => "This Agent Backbone does not offer current plans yet.",
            Self::Unreadable => "Backbone returned an unreadable plans response.",
        }
    }
}

/// The most pages read for one list, bounding a misbehaving cursor.
const PAGES: usize = 20;

/// Read every active agent's current plan, following the page cursor. Runs on
/// an I/O worker.
pub fn list_all(directory: &Path) -> Result<PlansPage, Error> {
    let mut all = list(directory, None)?;
    for _ in 1..PAGES {
        let Some(after) = all.next_after_author_id.take() else {
            return Ok(all);
        };
        let mut page = list(directory, Some(&after))?;
        all.items.append(&mut page.items);
        all.next_after_author_id = page.next_after_author_id;
    }
    match all.next_after_author_id {
        None => Ok(all),
        Some(_) => Err(Error::Unreadable),
    }
}

/// Read one page of current plans. Runs on an I/O worker.
pub fn list(directory: &Path, after: Option<&str>) -> Result<PlansPage, Error> {
    let (provider, key) =
        crate::configuration::configured(directory, None).ok_or(Error::Unavailable)?;
    let socket = provider.strip_prefix("unix:").ok_or(Error::Unavailable)?;
    if key.is_empty() {
        return Err(Error::Unavailable);
    }
    let mut path = "/api/current-plans?limit=50".to_owned();
    if let Some(after) = after {
        if after.len() != 32 || !after.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Unreadable);
        }
        path.push_str("&after_author_id=");
        path.push_str(after);
    }
    let response = crate::transport::local::response(
        socket,
        &path,
        &key,
        None,
        LIST_LIMIT,
        Duration::from_secs(15),
    )
    .map_err(|_| Error::Unavailable)?;
    match response.status {
        200 => parse(&response.body),
        404 => Err(Error::Unsupported),
        _ => Err(Error::Unavailable),
    }
}

fn parse(body: &[u8]) -> Result<PlansPage, Error> {
    if body.len() > LIST_LIMIT {
        return Err(Error::Unreadable);
    }
    serde_json::from_slice(body).map_err(|_| Error::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_plans_and_agents_without_one() {
        let page = parse(
            br#"{"items":[
              {"author_id":"11111111111111111111111111111111","agent_name":"builder","record":{
                "author_id":"11111111111111111111111111111111","author_name":"builder",
                "agent_name":"builder","plan_id":"33333333333333333333333333333333",
                "revision_id":"55555555555555555555555555555555",
                "updated_at":"2026-10-27T12:00:00+00:00","age_seconds":720,"stale":false,
                "plan":{"plan_id":"33333333333333333333333333333333","title":"Publish",
                  "items":[
                    {"item_id":"a","text":"Scope release","section":"now","status":"done","detail":"","references":[]},
                    {"item_id":"b","text":"Verify candidate","section":"now","status":"in_progress","detail":"","references":[]},
                    {"item_id":"c","text":"Publish","section":"next","status":"blocked","detail":"Needs your decision","references":[]}
                  ]}}},
              {"author_id":"22222222222222222222222222222222","agent_name":"coordinator","record":null}
            ],"next_after_author_id":null,"generated_at":"2026-10-27T12:12:00+00:00","stale_after_seconds":86400}"#,
        )
        .unwrap();
        let plan = &page.items[0].record.as_ref().unwrap().plan;
        assert_eq!(plan.current().unwrap().text, "Verify candidate");
        assert!(plan.blocked());
        assert!(page.items[1].record.is_none());
        assert_eq!(parse(b"{}"), Err(Error::Unreadable));
    }
}
