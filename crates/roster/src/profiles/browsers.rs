//! Agent Backbone's named browsers: Chrome profiles of their own, one machine
//! each, that the owner signs in to once and gives to agents.
//!
//! The browsers are read with `GET /api/browsers` and `GET /api/browsers/{id}`
//! and changed with `POST /api/browsers`, `POST /api/browsers/{id}/window`,
//! `.../save` and `.../check`, `DELETE /api/browsers/{id}`,
//! `DELETE /api/browsers/{id}/hold` (taking it back) and
//! `PUT /api/browsers/{id}/policies`; older use is read with
//! `GET /api/browsers/{id}/sessions`. Who may use one is the agents' grants of
//! kind `browser` naming it, and who may give it out their ceilings, both
//! changed on the access routes. Each change is pushed on the stream as
//! `browser.changed`.
//!
//! Backbone judges every change and replies with the browser as it is now, or
//! a refusal whose message is written for the person.
use super::{
    Connection, Error,
    access::{Failure, Role},
    stream::Frames,
    successful,
};
use crate::{RESPONSE_LIMIT, transport::local, views::encode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::time::Duration;

/// Where a browser stands, the first of these that holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Created and not saved yet: its sign-in window is open.
    SigningIn,
    /// The latest check found no sign-in.
    SignedOut,
    /// Someone holds it.
    InUse,
    /// At least one agent asks for it.
    Asked,
    /// At least one agent may use it.
    Given,
    /// None of the above.
    Nobody,
    /// A state this version does not know.
    #[default]
    #[serde(other)]
    Unknown,
}

/// Who holds a browser now.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Holder {
    /// The agent; none for the owner.
    #[serde(default)]
    pub agent: Option<String>,
    /// Since when, in RFC 3339.
    pub since: String,
    /// How many tabs are open.
    #[serde(default)]
    pub tabs: u32,
    /// The session; only a browser's detail gives it.
    #[serde(default)]
    pub session: Option<i64>,
    /// The agent-browser session the holder drives it with, when it named
    /// one; only a browser's detail gives it.
    #[serde(default)]
    pub agent_browser_session: Option<String>,
}

/// A browser as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Browser {
    /// Its ID, which grants, ceilings and requests name.
    pub id: String,
    /// Its name for people.
    pub name: String,
    /// The machine its profile is on, by ID.
    pub machine: String,
    /// That machine's name, while the machine is in the profile.
    #[serde(default)]
    pub machine_name: Option<String>,
    /// The address it opens at.
    #[serde(default)]
    pub opens_at: String,
    /// The domains the latest check found it signed in to.
    #[serde(default)]
    pub signed_in_to: Vec<String>,
    /// Where it stands.
    #[serde(default)]
    pub state: State,
    /// How many agents may use it.
    #[serde(default)]
    pub agents: u32,
    /// How many requests for it wait.
    #[serde(default)]
    pub asks: u32,
    /// Who holds it; none when nobody does.
    #[serde(default)]
    pub holder: Option<Holder>,
    /// Since when it has been signed in, in RFC 3339; none while it is not.
    #[serde(default)]
    pub signed_in_since: Option<String>,
    /// When its sign-in was last checked, in RFC 3339; none before the
    /// first check.
    #[serde(default)]
    pub checked_at: Option<String>,
}

/// The browsers, and those agents ask for that are not there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Browsers {
    /// The browsers that could be read, in the order they were created.
    pub items: Vec<Browser>,
    /// The browsers agents ask for that are not there.
    pub wanted: Vec<Wanted>,
}

/// A browser agents ask for that is not there: creating one with its ID shows
/// the requests on it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Wanted {
    /// The ID asked for.
    pub browser: String,
    /// The machine it is asked for on, by ID.
    pub machine: String,
    /// That machine's name, while the machine is in the profile.
    #[serde(default)]
    pub machine_name: Option<String>,
    /// The requests that wait for it.
    #[serde(default)]
    pub requests: Vec<Ask>,
}

/// A request for a browser that waits.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Ask {
    /// The request's ID, which the access routes approve or refuse.
    pub id: i64,
    /// The agent that asks.
    pub agent: String,
    /// Why, as it said.
    #[serde(default)]
    pub reason: String,
    /// When it asked, in RFC 3339.
    #[serde(default)]
    pub requested_at: String,
    /// The agent it waits on; none for the owner.
    #[serde(default)]
    pub waits_on: Option<String>,
}

/// A browser with who may use it, give it out and hold it, its sign-ins,
/// policies and recent use: the reply to `GET /api/browsers/{id}` and to the
/// routes that change a browser.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Detail {
    /// The browser, as the list shows it.
    pub browser: Browser,
    /// Who holds it, with the session; none when nobody does.
    #[serde(default)]
    pub holder: Option<Holder>,
    /// The DevTools endpoint of the Chrome that runs it, for the owner only
    /// and only while it runs.
    #[serde(default)]
    pub devtools: Option<String>,
    /// The agents that may use it.
    #[serde(default)]
    pub grants: Vec<Given>,
    /// The agents whose ceiling holds it, which may give it to the agents
    /// under them.
    #[serde(default)]
    pub givers: Vec<Giver>,
    /// The requests for it that wait.
    #[serde(default)]
    pub requests: Vec<Ask>,
    /// What it is signed in to, and to which account.
    #[serde(default)]
    pub signed_in_to: Vec<SignedIn>,
    /// What its holder may do there.
    #[serde(default)]
    pub policies: Policies,
    /// Its latest sessions, the newest first.
    #[serde(default)]
    pub recent: Vec<Session>,
    /// Whether older sessions follow.
    #[serde(default)]
    pub has_more_recent: bool,
}

/// An agent that may use a browser.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Given {
    /// The agent.
    pub agent: String,
    /// The agent that gave it, by its name now; none for the owner, and for
    /// an agent forgotten since.
    #[serde(default)]
    pub given_by: Option<String>,
    /// That agent's registration ID; none for the owner.
    #[serde(default)]
    pub given_by_id: Option<String>,
    /// When it was given, in RFC 3339.
    #[serde(default)]
    pub given_at: String,
    /// How many sessions the agent has had of the browser.
    #[serde(default)]
    pub uses: u64,
}

/// An agent that may give a browser to the agents under it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Giver {
    /// The agent.
    pub agent: String,
    /// Its role.
    pub role: Role,
    /// How many agents are under it.
    #[serde(default)]
    pub under: u32,
}

/// A domain a browser is signed in to.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SignedIn {
    /// The domain.
    pub domain: String,
    /// The domain of the account the sign-in belongs to.
    #[serde(default)]
    pub account: String,
    /// Said once for each account whose one sign-in also opens other
    /// services, such as a Google account's.
    #[serde(default)]
    pub note: Option<String>,
}

/// What a browser's holder may do there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policies {
    /// The holder may open no top-level address outside `sites`.
    pub keep_on_sites: bool,
    /// The sites it keeps to; the signed-in domains when `keep_on_sites` is
    /// set without them.
    pub sites: Vec<String>,
    /// Posting, sending and paying wait for the owner.
    pub stop_before_last_button: bool,
}

/// Why a session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The holder let it go.
    Released,
    /// The owner took it back.
    TakenBack,
    /// The holder's grant was revoked.
    Revoked,
    /// Its window was closed, or Chrome ended.
    Closed,
    /// The browser was deleted.
    Deleted,
    /// A reason this version does not know.
    #[serde(other)]
    Unknown,
}

/// One hold of a browser.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Session {
    /// The session's ID.
    pub id: i64,
    /// The agent that held it; none for the owner.
    #[serde(default)]
    pub agent: Option<String>,
    /// When it began, in RFC 3339.
    #[serde(default)]
    pub started_at: String,
    /// When it ended, in RFC 3339; none while it is open.
    #[serde(default)]
    pub ended_at: Option<String>,
    /// Why it ended; none while it is open.
    #[serde(default)]
    pub ended_by: Option<End>,
    /// The most tabs open at once.
    #[serde(default)]
    pub tabs: u32,
    /// How many times it stopped for the owner.
    #[serde(default)]
    pub waited: u32,
    /// What the holder said it did; may be empty.
    #[serde(default)]
    pub summary: String,
}

/// A page of older sessions, the newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPage {
    /// The sessions on this page that could be read.
    pub items: Vec<Session>,
    /// Whether older sessions follow.
    pub has_more: bool,
    /// The `before` that reads the next page, when older sessions follow.
    pub next_before: Option<i64>,
}

/// A new browser, as `POST /api/browsers` takes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Create {
    /// Its name; Backbone makes its ID from it.
    pub name: String,
    /// The address it opens at: a host or an https URL.
    pub opens_at: String,
    /// The machine its profile is to be on, by name or ID; this one when
    /// none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// Keeping a browser that is signing in, as `POST /api/browsers/{id}/save`
/// takes it. Every field may be left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Save {
    /// A new name; the ID stays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new address to open at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opens_at: Option<String>,
    /// The agents given the browser; saving approves a waiting request of
    /// one of them for it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<String>,
    /// Its policies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<Policies>,
}

/// What changed of a browser, from `browser.changed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// It was created.
    Created,
    /// What it is signed in to changed.
    SignedIn,
    /// It was saved.
    Saved,
    /// Its state changed.
    State,
    /// A grant, a giver or a request for it changed.
    Grants,
    /// Who holds it changed.
    Holder,
    /// Its policies changed.
    Policies,
    /// It was deleted.
    Deleted,
    /// A change this version does not know.
    #[serde(other)]
    Unknown,
}

/// What one frame of the browsers' stream changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Read the browsers again: the stream opened, events were missed, or a
    /// change could not be read.
    Read,
    /// A browser changed.
    Changed {
        /// Its ID.
        id: String,
        /// Its machine, by ID.
        machine: String,
        /// What changed.
        kind: Kind,
        /// The browser as the list shows it now; none once it is deleted.
        browser: Option<Box<Browser>>,
    },
}

/// The stream writes a comment every 15 seconds while nothing else happens,
/// so three missed ones mean a broken stream.
const QUIET: Duration = Duration::from_secs(45);

impl Connection {
    /// Read the browsers, those on the machine `machine` (by ID) when given.
    /// Runs on an I/O worker.
    pub fn browsers(&self, machine: Option<&str>) -> Result<Browsers, Error> {
        let path = match machine {
            Some(machine) => format!("/api/browsers?machine={}", encode(machine)),
            None => "/api/browsers".to_owned(),
        };
        let response = self.response(&path, None, RESPONSE_LIMIT)?;
        successful(&response)?;
        decode_browsers(&response.body).ok_or(Error::Invalid)
    }

    /// Read the browser `id`. Runs on an I/O worker.
    pub fn browser(&self, id: &str) -> Result<Detail, Failure> {
        let response = self.response(&browser_path(id, "")?, None, RESPONSE_LIMIT)?;
        detail(&response)
    }

    /// Read the sessions of the browser `id` older than `before`, at most
    /// `limit` of them when given. Runs on an I/O worker.
    pub fn browser_sessions(
        &self,
        id: &str,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> Result<SessionPage, Error> {
        let mut path = browser_path(id, "/sessions")?;
        let query: Vec<String> = [
            before.map(|before| format!("before={before}")),
            limit.map(|limit| format!("limit={limit}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !query.is_empty() {
            path = format!("{path}?{}", query.join("&"));
        }
        let response = self.response(&path, None, RESPONSE_LIMIT)?;
        successful(&response)?;
        decode_sessions(&response.body).ok_or(Error::Invalid)
    }

    /// Create a browser and open its sign-in window on its machine. Runs on
    /// an I/O worker.
    pub fn create_browser(&self, create: &Create) -> Result<Detail, Failure> {
        self.browser_change("POST", "/api/browsers".to_owned(), Some(json(create)?))
    }

    /// Show the browser `id`'s window to the owner. Runs on an I/O worker.
    pub fn show_browser(&self, id: &str) -> Result<Detail, Failure> {
        self.browser_change("POST", browser_path(id, "/window")?, None)
    }

    /// Keep the browser `id` that is signing in. Runs on an I/O worker.
    pub fn save_browser(&self, id: &str, save: &Save) -> Result<Detail, Failure> {
        self.browser_change("POST", browser_path(id, "/save")?, Some(json(save)?))
    }

    /// Check the browser `id`'s sign-in now. Runs on an I/O worker.
    pub fn check_browser(&self, id: &str) -> Result<Detail, Failure> {
        self.browser_change("POST", browser_path(id, "/check")?, None)
    }

    /// Set what the browser `id`'s holder may do there. Runs on an I/O worker.
    pub fn set_browser_policies(&self, id: &str, policies: &Policies) -> Result<Detail, Failure> {
        let path = browser_path(id, "/policies")?;
        self.browser_change("PUT", path, Some(json(policies)?))
    }

    /// Take the browser `id` back from whoever holds it, closing their tabs.
    /// Runs on an I/O worker.
    pub fn take_back_browser(&self, id: &str) -> Result<(), Failure> {
        self.send_change("DELETE", &browser_path(id, "/hold")?, None)
            .map(drop)
    }

    /// Sign the browser `id` out and delete its Chrome profile. Runs on an
    /// I/O worker.
    pub fn delete_browser(&self, id: &str) -> Result<(), Failure> {
        self.send_change("DELETE", &browser_path(id, "")?, None)
            .map(drop)
    }

    /// Follow the browsers' changes on the push stream until `live` is
    /// false, calling `changed` with each. No cursor is kept: a stream that
    /// is opened again starts with [`Change::Read`].
    ///
    /// A service without the stream is `Unsupported`. A stream that ends
    /// while `live` is an error. Blocks its caller, which is an I/O worker
    /// of its own.
    pub fn watch_browsers(
        &self,
        live: &dyn Fn() -> bool,
        changed: &mut dyn FnMut(Change),
    ) -> Result<(), Error> {
        let mut frames = Frames::default();
        local::stream(
            self.socket.to_str().ok_or(Error::Invalid)?,
            "/api/stream?kinds=browser.changed,resync",
            &self.identity.profile_id,
            None,
            QUIET,
            live,
            &mut |bytes| {
                frames.push(bytes, &mut |kind, data| {
                    if let Some(change) = change(kind, data) {
                        changed(change);
                    }
                })
            },
        )
        .map_err(|status| match status {
            Some(404) => Error::Unsupported,
            Some(status) => Error::Refused(status),
            None => Error::Unavailable,
        })
    }

    fn browser_change(
        &self,
        method: &'static str,
        path: String,
        body: Option<serde_json::Value>,
    ) -> Result<Detail, Failure> {
        detail(&self.send_change(method, &path, body)?)
    }
}

/// The route of the browser `id`, followed by `rest`.
fn browser_path(id: &str, rest: &str) -> Result<String, Error> {
    if id.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(format!("/api/browsers/{}{rest}", encode(id)))
}

fn json(body: &impl Serialize) -> Result<serde_json::Value, Error> {
    serde_json::to_value(body).map_err(|_| Error::Invalid)
}

/// A browser's detail from a reply, or Backbone's refusal.
fn detail(response: &local::Response) -> Result<Detail, Failure> {
    super::access::refusal(response)?;
    Ok(read(&response.body).ok_or(Error::Invalid)?)
}

fn read<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// Items of a list, each read on its own: one that cannot be read is left
/// out.
fn items<T: DeserializeOwned>(raw: &[&RawValue]) -> Vec<T> {
    raw.iter()
        .filter_map(|item| serde_json::from_str(item.get()).ok())
        .collect()
}

fn decode_browsers(body: &[u8]) -> Option<Browsers> {
    #[derive(Deserialize)]
    struct Document<'a> {
        #[serde(borrow)]
        items: Vec<&'a RawValue>,
        #[serde(default, borrow)]
        wanted: Vec<&'a RawValue>,
    }
    let document: Document = serde_json::from_slice(body).ok()?;
    Some(Browsers {
        items: items(&document.items),
        wanted: items(&document.wanted),
    })
}

fn decode_sessions(body: &[u8]) -> Option<SessionPage> {
    #[derive(Deserialize)]
    struct Document<'a> {
        #[serde(borrow)]
        items: Vec<&'a RawValue>,
        #[serde(default)]
        has_more: bool,
        #[serde(default)]
        next_before: Option<i64>,
    }
    let document: Document = serde_json::from_slice(body).ok()?;
    Some(SessionPage {
        items: items(&document.items),
        has_more: document.has_more,
        next_before: document.next_before,
    })
}

/// What a frame of `kind` with `data` changes; none for a kind that changes
/// nothing shown. A change that can't be read asks for a read instead.
fn change(kind: &[u8], data: &[u8]) -> Option<Change> {
    #[derive(Deserialize)]
    struct Changed {
        id: String,
        machine: String,
        change: Kind,
        #[serde(default)]
        browser: Option<Browser>,
    }
    Some(match kind {
        b"hello" | b"resync" => Change::Read,
        b"browser.changed" => {
            read::<Changed>(data).map_or(Change::Read, |changed| Change::Changed {
                id: changed.id,
                machine: changed.machine,
                kind: changed.change,
                browser: changed.browser.map(Box::new),
            })
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_carries_the_browser_as_listed_or_none_once_deleted() {
        let mut frames = Frames::default();
        let mut changes = Vec::new();
        for data in [
            include_str!("browsers/fixtures/browser-changed.json"),
            include_str!("browsers/fixtures/browser-changed-deleted.json"),
            "{\"id\":3}",
        ] {
            let data: serde_json::Value = serde_json::from_str(data).unwrap();
            let frame = format!("event: browser.changed\ndata: {data}\n\n");
            assert!(frames.push(frame.as_bytes(), &mut |kind, data| {
                changes.extend(change(kind, data));
            }));
        }
        frames.push(
            b"event: resync\ndata: {}\n\nevent: agent.state\ndata: {}\n\n",
            &mut |kind, data| {
                changes.extend(change(kind, data));
            },
        );
        let [held, deleted, unread, resync] = &changes[..] else {
            panic!("{changes:?}");
        };
        let Change::Changed {
            id,
            kind: Kind::Holder,
            browser: Some(browser),
            ..
        } = held
        else {
            panic!("{held:?}");
        };
        assert_eq!((id.as_str(), browser.state), ("instagram", State::InUse));
        assert_eq!(
            browser.holder.as_ref().unwrap().agent.as_deref(),
            Some("poster")
        );
        assert_eq!(
            deleted,
            &Change::Changed {
                id: "facebook".into(),
                machine: "5f2c9e0d1b7a4e38".into(),
                kind: Kind::Deleted,
                browser: None,
            }
        );
        // A change that cannot be read asks for a read.
        assert_eq!((unread, resync), (&Change::Read, &Change::Read));
    }
}
