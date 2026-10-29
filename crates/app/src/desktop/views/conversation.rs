//! The conversation beside an authored record. Every reader shows this one
//! component: a rail while closed, a thread and a composer while open.
//!
//! When Agent Backbone keeps the record's conversation, the thread is the
//! stored one: the owner's and agents' messages and the record's own events,
//! read every few seconds while the panel is shown. Messages go through
//! `roster::conversations` with a key that makes a retry safe.
//!
//! A provider without conversations gets the earlier behaviour: the thread
//! holds the messages sent from this pane, and a record's conversation sends
//! each once through `roster::feedback` with a reference to the record. A view
//! with its own delivery, such as Sheets, receives [`ConversationEvent`]s and
//! reports each outcome back with [`Conversation::complete`].

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use agent_interface_roster::{
    conversations::{self as stored, Author, Body, Kind},
    feedback::{self, Outcome},
    views,
    views::{ReportContent, ReportItem},
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, ScrollHandle, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};

use super::{clock::Local, reports};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};

/// What became of one message.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Delivery {
    Sending,
    Sent,
    /// Nothing was accepted; sending again is safe.
    NotSent(String),
    /// It may have arrived; sending again could repeat it.
    Unconfirmed(String),
    /// Not sent, and moved back into the composer to edit.
    Moved,
    /// Backbone stored it; the thread shows the stored entry instead.
    Stored,
}

/// How a message went out; a retry goes the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Via {
    /// Backbone's conversation, which stores a request ID at most once.
    Stored,
    /// One-way feedback about the record.
    Feedback,
    /// The owning view's own delivery.
    Owner,
}

struct Message {
    /// Makes sending it again safe where Backbone keeps the conversation.
    request_id: String,
    /// `None` while waiting to learn whether Backbone keeps the conversation.
    via: Option<Via>,
    /// The agent chosen when it first went to Backbone; a retry keeps it.
    recipient: Option<String>,
    text: String,
    /// What it referred to when it was written.
    anchor: Option<String>,
    stored_anchor: Option<stored::Anchor>,
    at: f64,
    delivery: Delivery,
}

/// What a view with its own delivery is asked to do with message `index`.
pub(super) enum ConversationEvent {
    /// Send it for the first time.
    Send(usize),
    /// Send the same request again.
    Retry(usize),
}

impl EventEmitter<ConversationEvent> for Conversation {}

pub(super) struct Conversation {
    /// The record messages are about and where to send them; `None` when the
    /// owning view delivers them itself.
    record: Option<(ReportItem, PathBuf)>,
    agent: String,
    about: &'static str,
    anchor: Option<String>,
    plan_anchor: Option<stored::Anchor>,
    /// Input is held while the owning view cannot accept a message.
    blocked: bool,
    field: Entity<NameField>,
    /// The document surface, which takes focus back when the panel closes.
    surface: Option<Rc<RefCell<super::artifact_web::Surface>>>,
    open: bool,
    /// A key asked to open or close the panel; done when it next renders.
    toggle_requested: bool,
    messages: Vec<Message>,
    /// Retains the reader's position; new replies follow only at the bottom.
    scroll: ScrollHandle,
    return_focus: Option<FocusHandle>,
    /// The conversation Backbone keeps, for a record that has one.
    stored: Option<Stored>,
    /// The view shows this conversation, so it reads Backbone.
    active: bool,
    /// Who the person chose to ask when no agent is responsible, and whom
    /// they may choose from besides the conversation's participants.
    recipient: Option<String>,
    candidates: Vec<String>,
    /// The team's names as last read, for a skill's or policy's chooser,
    /// and whether to read them again.
    team: Vec<String>,
    team_read: bool,
    /// A skill's or policy's conversation found the connection changed; the
    /// view replaces it with a new one rather than retarget its messages.
    stale: bool,
    /// Why the latest read failed, shown while nothing has been read.
    read_error: Option<stored::Error>,
    /// Whether the pane holding it is visible.
    visible: Rc<Cell<bool>>,
    _poll: Option<Task<()>>,
    _input: Subscription,
}

/// A conversation as Backbone keeps it.
struct Stored {
    directory: PathBuf,
    /// The provider the record came from; requests go nowhere else. Empty
    /// until the first read for a skill or policy, which has no record.
    provider: String,
    kind: Kind,
    /// A skill's or policy's source path; its `id` is found from it first.
    source: Option<String>,
    id: String,
    /// `None` until Backbone first answers; `Some(false)` when it keeps none.
    supported: Option<bool>,
    /// The newest page, read again on each poll, plus entries a send returned.
    page: Option<stored::Conversation>,
    /// The entries Backbone itself last returned for the newest page.
    fetched: Vec<u64>,
    /// Older entries loaded on request, oldest first, and where to continue.
    earlier: Vec<stored::Entry>,
    earlier_cursor: Option<String>,
    /// The read marker when the panel opened; the "new" divider follows it.
    divider: Option<u64>,
    reading: bool,
    /// When the newest page was last asked for.
    read_at: Option<std::time::Instant>,
    /// The marker being asked for, so a poll does not ask again meanwhile.
    marking: Option<u64>,
    /// A failed acknowledgement waits for the next polling interval.
    mark_at: Option<std::time::Instant>,
    /// Redeliveries by entry, followed until a fresh read shows them settled.
    redelivery: HashMap<u64, Redelivery>,
    /// Where the next batch of older entries to re-read starts.
    turn: usize,
}

impl Stored {
    fn live(&self) -> bool {
        self.supported == Some(true)
    }

    /// The newest entry Backbone returned; one a send added may be newer
    /// than replies not yet read.
    fn last(&self) -> Option<u64> {
        self.fetched.iter().copied().max()
    }

    /// Take a newly read newest page, keeping the thread continuous: entries
    /// it no longer holds move to the earlier ones. When it shares nothing
    /// with the last page, entries may have been skipped, so earlier entries
    /// are dropped and paging continues from the new page.
    fn receive(&mut self, mut page: stored::Conversation) {
        // A send accepted after this page was read added an entry it lacks.
        let newest = page.entries.iter().map(|entry| entry.id).max().unwrap_or(0);
        let mut sent: Vec<stored::Entry> = self
            .page
            .as_ref()
            .map(|old| {
                old.entries
                    .iter()
                    .filter(|entry| entry.id > newest && !self.fetched.contains(&entry.id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        match self.page.take() {
            Some(old) if !page.entries.is_empty() => {
                // Only what Backbone returned shows continuity, not entries a
                // send added since.
                let overlap = page
                    .entries
                    .iter()
                    .any(|entry| self.fetched.contains(&entry.id));
                if overlap || page.older.is_none() {
                    let first = page.entries.iter().map(|entry| entry.id).min();
                    self.earlier.extend(
                        old.entries
                            .into_iter()
                            .filter(|entry| first.is_some_and(|first| entry.id < first)),
                    );
                    self.earlier.sort_by_key(|entry| entry.id);
                    self.earlier.dedup_by_key(|entry| entry.id);
                } else {
                    self.earlier.clear();
                    self.earlier_cursor = page.older.clone();
                }
            }
            Some(_) => {}
            None => self.earlier_cursor = page.older.clone(),
        }
        self.fetched = page.entries.iter().map(|entry| entry.id).collect();
        page.entries.append(&mut sent);
        self.page = Some(page);
    }

    /// Older owner messages whose delivery may still change, at most 50:
    /// the newest page does not hold them, so they are read by ID.
    /// Those with an unanswered redelivery count too. With more than 50, each
    /// poll takes the next 50 in turn.
    fn unsettled(&mut self) -> Vec<u64> {
        let all: Vec<u64> = self
            .earlier
            .iter()
            .filter(|entry| {
                self.redelivery.contains_key(&entry.id)
                    || matches!(
                        entry.body,
                        Body::Message {
                            delivery: Some(stored::Delivery::Pending | stored::Delivery::Held),
                            ..
                        }
                    )
            })
            .map(|entry| entry.id)
            .collect();
        if all.len() <= 50 {
            return all;
        }
        let start = self.turn % all.len();
        self.turn = start + 50;
        all.iter().cycle().skip(start).take(50).copied().collect()
    }

    /// After a read that began at `started` and returned `returned`: forget
    /// redeliveries queued before it whose entries it returned, since it shows
    /// their state, and notes for entries Backbone no longer reports failed.
    fn settle(&mut self, started: std::time::Instant, returned: &[u64]) {
        self.redelivery.retain(|id, state| {
            !matches!(state, Redelivery::Queued(at) if *at < started && returned.contains(id))
        });
        let noted: Vec<u64> = self
            .redelivery
            .iter()
            .filter(|(_, state)| matches!(state, Redelivery::Note(_)))
            .map(|(id, _)| *id)
            .collect();
        for id in noted {
            let failed = self.entry_mut(id).is_none_or(|entry| {
                matches!(
                    entry.body,
                    Body::Message {
                        delivery: Some(stored::Delivery::Failed),
                        ..
                    }
                )
            });
            if !failed {
                self.redelivery.remove(&id);
            }
        }
    }

    /// The shown copy of entry `id`, wherever it was loaded.
    fn entry_mut(&mut self, id: u64) -> Option<&mut stored::Entry> {
        self.earlier
            .iter_mut()
            .chain(
                self.page
                    .iter_mut()
                    .flat_map(|page| page.entries.iter_mut()),
            )
            .find(|entry| entry.id == id)
    }
}

/// A redelivery the panel follows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Redelivery {
    /// Asked; no answer yet.
    Asking,
    /// Backbone queued it at this time; a read from before then may still
    /// say failed, so only a later one settles it.
    Queued(std::time::Instant),
    /// What Backbone said when it did not queue it.
    Note(String),
}

/// How often the shown conversation is read: often while open, for its
/// replies, and slowly while closed, for the rail's mark.
const OPEN_POLL: Duration = Duration::from_secs(5);
const CLOSED_POLL: Duration = Duration::from_secs(25);

impl Conversation {
    /// The conversation about an authored record, sent to its author.
    pub fn new(
        item: ReportItem,
        directory: PathBuf,
        surface: Rc<RefCell<super::artifact_web::Surface>>,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = Self::create(subject(&item), window, cx);
        conversation.visible = visible;
        conversation.agent = item.author.clone();
        conversation.anchor = Some(format!("#{}", item.id));
        if let Some((kind, id)) = item.conversation() {
            conversation.stored = Some(Stored {
                directory: directory.clone(),
                provider: item.provider_url.clone(),
                kind,
                source: None,
                id,
                supported: None,
                page: None,
                fetched: Vec::new(),
                earlier: Vec::new(),
                earlier_cursor: None,
                divider: None,
                reading: false,
                read_at: None,
                marking: None,
                mark_at: None,
                redelivery: HashMap::new(),
                turn: 0,
            });
            conversation.start_polling(cx);
        }
        conversation.record = Some((item, directory));
        conversation.surface = Some(surface);
        conversation
    }

    /// A plan conversation retains the provider, author and observed step.
    pub fn about_plan(
        target: &super::plans::ConversationTarget,
        directory: PathBuf,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = Self::create("about this plan", window, cx);
        conversation.visible = visible;
        conversation.agent = target.agent.clone();
        conversation.anchor = target.anchor.label.clone();
        conversation.plan_anchor = Some(target.anchor.clone());
        conversation.open = true;
        conversation.stored = Some(Stored {
            directory,
            provider: target.provider.clone(),
            kind: Kind::Plan,
            source: None,
            id: target.author.clone(),
            supported: None,
            page: None,
            fetched: Vec::new(),
            earlier: Vec::new(),
            earlier_cursor: None,
            divider: None,
            reading: false,
            read_at: None,
            marking: None,
            mark_at: None,
            redelivery: HashMap::new(),
            turn: 0,
        });
        conversation.start_polling(cx);
        conversation
    }

    /// The conversation about a skill or policy, by its source path. Who
    /// answers is the maintainer Backbone names, else an agent the person
    /// chooses from `candidates` or the conversation's participants.
    #[allow(clippy::too_many_arguments)]
    pub fn about_source(
        kind: Kind,
        path: String,
        about: &'static str,
        candidates: Vec<String>,
        directory: PathBuf,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = Self::create(about, window, cx);
        conversation.visible = visible;
        conversation.candidates = candidates;
        conversation.stored = Some(Stored {
            directory,
            provider: String::new(),
            kind,
            source: Some(path),
            id: String::new(),
            supported: None,
            page: None,
            fetched: Vec::new(),
            earlier: Vec::new(),
            earlier_cursor: None,
            divider: None,
            reading: false,
            read_at: None,
            marking: None,
            mark_at: None,
            redelivery: HashMap::new(),
            turn: 0,
        });
        conversation.start_polling(cx);
        conversation
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(wait)) = this.update(cx, |conversation, cx| conversation.poll(cx))
                else {
                    break;
                };
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    /// The agents the view offers now, such as those a skill reaches.
    pub fn set_candidates(&mut self, candidates: Vec<String>) {
        self.candidates = candidates;
    }

    /// The connection changed under this skill's or policy's conversation,
    /// or Backbone had none for it; reopening it asks again.
    pub fn stale(&self) -> bool {
        self.stale
            || self
                .stored
                .as_ref()
                .is_some_and(|stored| stored.source.is_some() && stored.supported == Some(false))
    }

    /// Keep the observed plans reachable while a draft or delivery needs work.
    pub fn holds_plan(&self, cx: &App) -> bool {
        self.plan_anchor.is_some() && (!self.draft(cx).is_empty() || !self.disposable())
    }

    /// Nothing would be lost by replacing it: no message still sending or
    /// awaiting a retry. A draft moves to the replacement.
    pub fn disposable(&self) -> bool {
        self.messages.iter().all(|message| {
            !matches!(
                message.delivery,
                Delivery::Sending | Delivery::NotSent(_) | Delivery::Unconfirmed(_)
            )
        })
    }

    /// Nobody answers unless the person names someone: Backbone names no
    /// maintainer, or the one it names has retired.
    fn needs_recipient(&self) -> bool {
        self.stored
            .as_ref()
            .and_then(|stored| stored.page.as_ref())
            .is_some_and(|page| {
                page.responsible.name.is_empty() || page.responsible.status != "active"
            })
    }

    /// Whom the person may ask: the conversation's participants, then the
    /// candidates the view offered, each once.
    fn askable(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .stored
            .as_ref()
            .and_then(|stored| stored.page.as_ref())
            .map(|page| {
                page.participants
                    .iter()
                    .filter(|participant| {
                        participant.status == "active" && !participant.name.is_empty()
                    })
                    .map(|participant| participant.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        for name in self.candidates.iter().chain(&self.team) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// A conversation whose owning view delivers each message.
    pub fn delivered_by_owner(
        about: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(about, window, cx)
    }

    fn create(about: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| NameField::feedback(window, cx));
        let input = cx.subscribe_in(
            &field,
            window,
            |conversation, _, event, window, cx| match event {
                NameEvent::Save => conversation.send(cx),
                NameEvent::Cancel => conversation.close(window, cx),
                NameEvent::Changed(_) => cx.notify(),
            },
        );
        Self {
            record: None,
            agent: String::new(),
            about,
            anchor: None,
            plan_anchor: None,
            blocked: false,
            field,
            surface: None,
            open: false,
            toggle_requested: false,
            messages: Vec::new(),
            scroll: ScrollHandle::new(),
            return_focus: None,
            stored: None,
            active: false,
            recipient: None,
            candidates: Vec::new(),
            team: Vec::new(),
            team_read: false,
            stale: false,
            read_error: None,
            visible: Rc::new(Cell::new(true)),
            _poll: None,
            _input: input,
        }
    }

    /// Who receives messages, what they refer to, and whether input is held.
    pub fn set_context(
        &mut self,
        agent: String,
        anchor: Option<String>,
        blocked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.agent != agent || self.anchor != anchor || self.blocked != blocked {
            self.agent = agent;
            self.anchor = anchor;
            self.hold(blocked, cx);
        }
    }

    /// Hold or release input at once, before the owning view next renders.
    pub fn hold(&mut self, blocked: bool, cx: &mut Context<Self>) {
        self.blocked = blocked;
        self.field
            .update(cx, |field, _| field.set_read_only(blocked));
        cx.notify();
    }

    /// Carry over a draft from the conversation this one replaces.
    pub fn restore_draft(&mut self, text: &str, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.field.update(cx, |field, cx| field.set_value(text, cx));
        }
    }

    /// The unsent draft.
    pub fn draft<'a>(&'a self, cx: &'a App) -> &'a str {
        self.field.read(cx).value()
    }

    /// The text of message `index`.
    pub fn text(&self, index: usize) -> &str {
        &self.messages[index].text
    }

    /// Message `index` is being sent again by the owning view.
    pub fn resending(&mut self, index: usize, cx: &mut Context<Self>) {
        self.messages[index].delivery = Delivery::Sending;
        cx.notify();
    }

    /// What became of message `index`, as the owning view's delivery found.
    pub fn complete(&mut self, index: usize, outcome: Outcome, cx: &mut Context<Self>) {
        self.messages[index].delivery = match outcome {
            Outcome::Accepted(_) => Delivery::Sent,
            Outcome::Refused(reason) => Delivery::NotSent(reason),
            Outcome::Unknown(reason) => Delivery::Unconfirmed(reason),
        };
        cx.notify();
    }

    /// Whether the person can see this conversation; only a seen one reads
    /// Backbone or moves its read marker.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let was = self.active;
        self.active = active;
        if active && !was {
            self.refresh(cx);
        }
    }

    /// Read again if shown, and say when to look next; `None` stops polling
    /// once Backbone turns out to keep no conversations.
    /// It wakes at the open rate, so opening the panel speeds it up at once.
    fn poll(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let stored = self.stored.as_ref()?;
        if stored.supported == Some(false) {
            return None;
        }
        let due = if self.open { OPEN_POLL } else { CLOSED_POLL };
        if self.shown() && stored.read_at.is_none_or(|at| at.elapsed() >= due) {
            self.refresh(cx);
        }
        Some(OPEN_POLL)
    }

    /// The person can see this conversation's rail or panel.
    fn shown(&self) -> bool {
        self.active && self.visible.get()
    }

    fn stored_live(&self) -> bool {
        self.stored.as_ref().is_some_and(Stored::live)
    }

    /// The request's target: data directory, provider, kind and identity.
    fn target(&self) -> Option<(PathBuf, String, Kind, String)> {
        let stored = self.stored.as_ref()?;
        Some((
            stored.directory.clone(),
            stored.provider.clone(),
            stored.kind,
            stored.id.clone(),
        ))
    }

    /// Read the newest page on an I/O worker.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(stored) = self.stored.as_mut().filter(|stored| !stored.reading) else {
            return;
        };
        if stored.supported == Some(false) {
            return;
        }
        stored.reading = true;
        let started = std::time::Instant::now();
        stored.read_at = Some(started);
        let unsettled = stored.unsettled();
        let source = stored.source.clone();
        let need_team = source.is_some() && !self.team_read;
        let (directory, provider, kind, id) = self.target().expect("stored conversation");
        let worker = cx.background_executor().spawn(async move {
            // A skill or policy learns its provider and identity on its first read.
            let provider = if provider.is_empty() {
                stored::provider(&directory).unwrap_or_default()
            } else {
                provider
            };
            let id = match (&source, id.is_empty()) {
                (Some(path), true) => match stored::resolve(&directory, &provider, kind, path) {
                    Ok(id) => id,
                    Err(error) => return (provider, String::new(), None, Err(error), None),
                },
                _ => id,
            };
            // With nobody responsible, the person chooses from the team; asked
            // again until it has answered.
            let team = if need_team {
                match views::read(&directory, None, &views::Request::Team, &|| true) {
                    Ok(views::Data::Team(found)) => Some(
                        found
                            .members
                            .into_iter()
                            .map(|member| member.name)
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                }
            } else {
                None
            };
            let newest = stored::show(&directory, &provider, kind, &id, &stored::Page::Newest);
            let older = (!unsettled.is_empty() && newest.is_ok()).then(|| {
                stored::show(
                    &directory,
                    &provider,
                    kind,
                    &id,
                    &stored::Page::Entries(unsettled),
                )
            });
            (provider, id, team, newest, older)
        });
        cx.spawn(async move |this, cx| {
            let (provider, id, team, result, older) = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                let seen = conversation.open && conversation.shown();
                let Some(stored) = conversation.stored.as_mut() else {
                    return;
                };
                stored.reading = false;
                if let Some(team) = team {
                    conversation.team_read = true;
                    conversation.team = team;
                }
                let Some(stored) = conversation.stored.as_mut() else {
                    return;
                };
                if stored.provider.is_empty() {
                    stored.provider = provider;
                }
                if stored.id.is_empty() {
                    stored.id = id;
                }
                let mut failure = None;
                conversation.read_error = result.as_ref().err().copied();
                let Some(stored) = conversation.stored.as_mut() else {
                    return;
                };
                match result {
                    Ok(page) => {
                        stored.supported = Some(true);
                        if seen && stored.divider.is_none() {
                            stored.divider = Some(page.read_through.unwrap_or_default());
                        }
                        let mut returned: Vec<u64> =
                            page.entries.iter().map(|entry| entry.id).collect();
                        stored.receive(page);
                        for entry in older
                            .and_then(Result::ok)
                            .into_iter()
                            .flat_map(|page| page.entries)
                        {
                            returned.push(entry.id);
                            if let Some(shown) = stored.entry_mut(entry.id) {
                                *shown = entry;
                            }
                        }
                        stored.settle(started, &returned);
                    }
                    // A provider without conversations, or one that does not
                    // know this record, keeps the one-way messages.
                    Err(stored::Error::Unsupported | stored::Error::UnknownArtifact) => {
                        stored.supported = Some(false);
                    }
                    Err(stored::Error::Moved) if stored.source.is_some() => {
                        conversation.stale = true;
                        failure = Some(stored::Error::Moved);
                    }
                    // Keep the last page while Backbone is briefly unavailable.
                    Err(error) => failure = Some(error),
                }
                conversation.route_waiting(failure, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Send the messages written before Backbone first answered, now that it
    /// has; with no answer they are not sent, and Retry asks again.
    fn route_waiting(&mut self, failure: Option<stored::Error>, cx: &mut Context<Self>) {
        let supported = self.stored.as_ref().and_then(|stored| stored.supported);
        let waiting: Vec<usize> = (0..self.messages.len())
            .filter(|&index| {
                let message = &self.messages[index];
                message.via.is_none() && message.delivery == Delivery::Sending
            })
            .collect();
        for index in waiting {
            match supported {
                Some(true) => self.post(index, cx),
                Some(false) => self.route(index, cx),
                None => {
                    self.messages[index].delivery = Delivery::NotSent(match failure {
                        Some(error) => format!("{} Nothing was sent.", error.message()),
                        None => "Backbone did not answer. Nothing was sent.".into(),
                    })
                }
            }
        }
    }

    /// Load the page before the oldest entry shown.
    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(stored) = self.stored.as_mut().filter(|stored| !stored.reading) else {
            return;
        };
        let Some(cursor) = stored.earlier_cursor.clone() else {
            return;
        };
        stored.reading = true;
        let (directory, provider, kind, id) = self.target().expect("stored conversation");
        let worker = cx.background_executor().spawn(async move {
            stored::show(
                &directory,
                &provider,
                kind,
                &id,
                &stored::Page::Before(cursor),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                let Some(stored) = conversation.stored.as_mut() else {
                    return;
                };
                stored.reading = false;
                if let Ok(page) = result {
                    let mut entries = page.entries;
                    entries.append(&mut stored.earlier);
                    // A retry can return an entry already in the older page.
                    entries.sort_by_key(|entry| entry.id);
                    entries.dedup_by_key(|entry| entry.id);
                    stored.earlier = entries;
                    stored.earlier_cursor = page.older;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A painted frame reached this entry; never include a later poll.
    fn mark_read(&mut self, painted_last: u64, cx: &mut Context<Self>) {
        if !(self.open && self.shown()) {
            return;
        }
        let target = self.target();
        let Some(stored) = self.stored.as_mut() else {
            return;
        };
        if !read_marker_due(stored.mark_at, std::time::Instant::now()) {
            return;
        }
        let (Some(last), Some(page)) = (stored.last(), stored.page.as_ref()) else {
            return;
        };
        let last = last.min(painted_last);
        if page.read_through.is_some_and(|read| read >= last) || stored.marking >= Some(last) {
            return;
        }
        stored.marking = Some(last);
        stored.mark_at = Some(std::time::Instant::now());
        let (directory, provider, kind, id) = target.expect("stored conversation");
        let worker = cx
            .background_executor()
            .spawn(async move { stored::read(&directory, &provider, kind, &id, last) });
        cx.spawn(async move |this, cx| {
            let counts = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                let Some(stored) = conversation.stored.as_mut() else {
                    return;
                };
                // A later request owns the marker; a failed one may be asked again.
                if stored.marking == Some(last) {
                    stored.marking = None;
                }
                if let (Ok(counts), Some(page)) = (counts, stored.page.as_mut()) {
                    page.read_through = Some(counts.read_through);
                    page.unread = counts.unread;
                    page.total = counts.total;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Store message `index` in Backbone's conversation, on an I/O worker.
    fn post(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((directory, provider, kind, id)) = self.target() else {
            return;
        };
        let chosen = self.recipient.clone().filter(|_| self.needs_recipient());
        let message = &mut self.messages[index];
        // A retry reuses the request, so it keeps the first recipient too.
        if message.via != Some(Via::Stored) {
            message.recipient = chosen;
        }
        let recipient = message.recipient.clone();
        message.via = Some(Via::Stored);
        // A refused retry says nothing about an earlier attempt that may have
        // been stored.
        let uncertain = matches!(message.delivery, Delivery::Unconfirmed(_));
        message.delivery = Delivery::Sending;
        let request = stored::Message {
            request_id: message.request_id.clone(),
            text: message.text.clone(),
            recipient,
            anchor: message.stored_anchor.clone(),
        };
        let worker = cx
            .background_executor()
            .spawn(async move { stored::send(&directory, &provider, kind, &id, &request) });
        cx.spawn(async move |this, cx| {
            let outcome = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                conversation.messages[index].delivery = match outcome {
                    stored::Outcome::Accepted(entry) => {
                        conversation.add_entry(*entry);
                        Delivery::Stored
                    }
                    stored::Outcome::Refused { message, .. } if uncertain => {
                        Delivery::Unconfirmed(message)
                    }
                    stored::Outcome::Refused { message, .. } => Delivery::NotSent(message),
                    stored::Outcome::AlreadyPending | stored::Outcome::Unknown => {
                        Delivery::Unconfirmed("Backbone did not confirm it.".into())
                    }
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Queue a stored message whose delivery failed again; the entry stays one.
    fn redeliver(&mut self, entry: u64, cx: &mut Context<Self>) {
        let Some((directory, provider, kind, id)) = self.target() else {
            return;
        };
        let Some(stored) = self.stored.as_mut() else {
            return;
        };
        if stored.redelivery.get(&entry) == Some(&Redelivery::Asking) {
            return;
        }
        stored.redelivery.insert(entry, Redelivery::Asking);
        let request = uuid::Uuid::new_v4().simple().to_string();
        let worker = cx.background_executor().spawn(async move {
            stored::redeliver(&directory, &provider, kind, &id, entry, &request)
        });
        cx.spawn(async move |this, cx| {
            let outcome = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                let state = match outcome {
                    stored::Outcome::Accepted(stored) => {
                        conversation.add_entry(*stored);
                        Redelivery::Queued(std::time::Instant::now())
                    }
                    stored::Outcome::AlreadyPending => {
                        Redelivery::Queued(std::time::Instant::now())
                    }
                    stored::Outcome::Refused { message, .. } => Redelivery::Note(message),
                    stored::Outcome::Unknown => {
                        Redelivery::Note("Backbone did not confirm the retry.".into())
                    }
                };
                if let Some(stored) = conversation.stored.as_mut() {
                    stored.redelivery.insert(entry, state);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Put an entry Backbone returned in its place, replacing the shown copy.
    fn add_entry(&mut self, entry: stored::Entry) {
        let Some(stored) = self.stored.as_mut() else {
            return;
        };
        if let Some(shown) = stored.entry_mut(entry.id) {
            *shown = entry;
        } else if let Some(page) = stored.page.as_mut() {
            page.entries.push(entry);
            page.entries.sort_by_key(|entry| entry.id);
        }
    }

    /// Open or close the panel from a key, which has no window to focus with.
    pub fn request_toggle(&mut self, cx: &mut Context<Self>) {
        self.toggle_requested = true;
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.return_focus = window.focused(cx);
            self.open = true;
            // The team may have changed since it was last read.
            self.team_read = false;
            if let Some(stored) = &mut self.stored {
                stored.divider = stored
                    .page
                    .as_ref()
                    .map(|page| page.read_through.unwrap_or_default());
            }
            self.scroll = ScrollHandle::new();
            self.refresh(cx);
            let focus = self.field.read(cx).focus.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        if let Some(stored) = &mut self.stored {
            stored.divider = None;
        }
        match self.return_focus.take() {
            Some(focus) => window.focus(&focus, cx),
            None => window.blur(cx),
        }
        if let Some(surface) = &self.surface {
            surface.borrow().focus();
        }
        cx.notify();
    }

    fn can_send(&self, cx: &gpui::App) -> bool {
        let field = self.field.read(cx);
        // A skill or policy sends only once Backbone has its conversation.
        let ready = self.stored.as_ref().is_none_or(|stored| {
            (stored.source.is_none() && stored.kind != Kind::Plan) || stored.live()
        });
        ready
            && !self.blocked
            && !field.composing()
            && !field.value().trim().is_empty()
            && (self.recipient.is_some() || !self.needs_recipient())
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        let text = self.field.read(cx).value().to_owned();
        self.field.update(cx, |field, cx| field.clear(cx));
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |now| now.as_secs_f64());
        self.messages.push(Message {
            request_id: uuid::Uuid::new_v4().simple().to_string(),
            via: None,
            recipient: None,
            text,
            anchor: self.anchor.clone(),
            stored_anchor: self.plan_anchor.clone().or_else(|| {
                self.record.as_ref().map(|(item, _)| stored::Anchor {
                    revision_id: Some(item.id.to_string()),
                    label: self.anchor.clone(),
                    ..stored::Anchor::default()
                })
            }),
            at,
            delivery: Delivery::Sending,
        });
        let index = self.messages.len() - 1;
        self.route(index, cx);
    }

    /// Send message `index` the way this conversation sends; until Backbone
    /// first answers it waits, so a message is never sent both ways.
    fn route(&mut self, index: usize, cx: &mut Context<Self>) {
        match self.stored.as_ref().map(|stored| stored.supported) {
            Some(Some(true)) => self.post(index, cx),
            Some(None) => {
                self.messages[index].delivery = Delivery::Sending;
                self.refresh(cx);
                cx.notify();
            }
            _ if self.record.is_some() => self.deliver(index, cx),
            _ if self.plan_anchor.is_some() => {
                self.messages[index].delivery = Delivery::NotSent(
                    "Backbone could not open this plan conversation. Nothing was sent.".into(),
                );
                cx.notify();
            }
            _ => {
                self.messages[index].via = Some(Via::Owner);
                cx.emit(ConversationEvent::Send(index));
                cx.notify();
            }
        }
    }

    /// Send message `index` again the way it first went: through Backbone's
    /// conversation with the same key, which it stores at most once; as
    /// feedback when nothing was accepted; or as an owning view's same saved
    /// request when the outcome was unknown. An owning view's refused message
    /// is edited and sent anew, since its context may have changed. One that
    /// never went out is sent as a new message would be.
    fn retry(&mut self, index: usize, cx: &mut Context<Self>) {
        let message = &self.messages[index];
        match (message.via, &message.delivery) {
            (Some(Via::Stored), Delivery::NotSent(_) | Delivery::Unconfirmed(_)) => {
                self.post(index, cx)
            }
            (Some(Via::Feedback), Delivery::NotSent(_)) => self.deliver(index, cx),
            (Some(Via::Owner), Delivery::Unconfirmed(_)) => {
                cx.emit(ConversationEvent::Retry(index))
            }
            (None, Delivery::NotSent(_)) => self.route(index, cx),
            _ => {}
        }
    }

    /// Move a message that was not sent back into an empty composer.
    fn edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked
            || !self.draft(cx).is_empty()
            || !matches!(self.messages[index].delivery, Delivery::NotSent(_))
        {
            return;
        }
        let text = self.messages[index].text.clone();
        self.messages[index].delivery = Delivery::Moved;
        self.field
            .update(cx, |field, cx| field.set_value(&text, cx));
        let focus = self.field.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Send message `index` once, on an I/O worker.
    fn deliver(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((item, directory)) = self.record.clone() else {
            return;
        };
        let message = &mut self.messages[index];
        message.via = Some(Via::Feedback);
        message.delivery = Delivery::Sending;
        let text = message.text.clone();
        let worker = cx.background_executor().spawn(async move {
            let started = std::time::Instant::now();
            let outcome = feedback::send(&directory, &item, &text);
            let result = match &outcome {
                Outcome::Accepted(_) => "accepted",
                Outcome::Refused(_) => "refused",
                Outcome::Unknown(_) => "unknown",
            };
            tracing::debug!(
                event = "feedback_finished",
                result,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Feedback submission finished"
            );
            outcome
        });
        cx.spawn(async move |this, cx| {
            let outcome = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                conversation.complete(index, outcome, cx)
            });
        })
        .detach();
        cx.notify();
    }
}

/// What the conversation is about, as its header and composer name it.
fn subject(item: &ReportItem) -> &'static str {
    match item.content {
        ReportContent::Progress { .. } => "about this report",
        ReportContent::Response { .. } => "about this answer",
        ReportContent::Document(_) => "about this document",
    }
}

impl Conversation {
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        ui::conversation_rail(
            div().id("conversation-open").on_click(
                cx.listener(|conversation, _, window, cx| conversation.toggle(window, cx)),
            ),
            self.mark(),
            self.stored
                .as_ref()
                .and_then(|stored| stored.page.as_ref())
                .is_some_and(|page| page.unread > 0),
        )
        .into_any_element()
    }

    /// With nobody responsible, the agents the person can ask, one chosen.
    fn chooser(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.needs_recipient() {
            return None;
        }
        let t = tokens();
        // Bounded so a large team never pushes the composer out of the pane.
        let mut row = div()
            .id("conversation-chooser")
            .flex_shrink_0()
            .max_h(px(t.size.chooser_height))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .gap(px(t.space.s6))
            .px(px(t.space.s14))
            .py(px(t.space.s8))
            .border_b(px(t.size.hairline))
            .border_color(colors().line);
        for name in self.askable() {
            let chosen = self.recipient.as_deref() == Some(name.as_str());
            row = row.child(
                ui::chip(
                    SharedString::from(format!("conversation-ask-{name}")),
                    SharedString::from(name.clone()),
                    chosen,
                )
                .on_click(cx.listener(move |conversation, _, _, cx| {
                    conversation.recipient = Some(name.clone());
                    cx.notify();
                })),
            );
        }
        Some(row)
    }

    /// The agent the header names: the conversation's responsible agent once
    /// Backbone answers, else the record's author.
    fn agent(&self) -> String {
        if self.needs_recipient() {
            return self
                .recipient
                .clone()
                .unwrap_or_else(|| "Choose who to ask".to_owned());
        }
        self.stored
            .as_ref()
            .and_then(|stored| stored.page.as_ref())
            .map(|page| page.responsible.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.agent.clone())
    }

    /// Backbone's entries in order, with the "new" divider and day dividers.
    fn entries(
        &self,
        now: Option<Local>,
        day: &mut Option<Local>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let c = colors();
        let Some(stored) = self.stored.as_ref().filter(|stored| stored.live()) else {
            return Vec::new();
        };
        let entries: Vec<&stored::Entry> = stored
            .earlier
            .iter()
            .chain(stored.page.iter().flat_map(|page| &page.entries))
            .collect();
        let unseen = |entry: &stored::Entry| {
            stored.divider.is_some_and(|divider| entry.id > divider)
                && !matches!(entry.author, Author::Owner)
        };
        let new = entries.iter().filter(|entry| unseen(entry)).count();
        let mut first_new = entries.iter().position(|entry| unseen(entry));
        let mut out = Vec::new();
        if stored.earlier_cursor.is_some() {
            out.push(
                ui::link(
                    div()
                        .id("conversation-earlier")
                        .text_color(c.accent_text)
                        .child("Earlier messages")
                        .on_click(
                            cx.listener(|conversation, _, _, cx| conversation.load_earlier(cx)),
                        ),
                )
                .into_any_element(),
            );
        }
        for (at, entry) in entries.iter().enumerate() {
            let local = epoch(&entry.created_at).and_then(Local::at);
            if let (Some(local), Some(now)) = (local, now)
                && day.is_none_or(|day| !day.same_day(&local))
            {
                out.push(ui::thread_divider(local.day_name(&now), false).into_any_element());
                *day = Some(local);
            }
            if first_new == Some(at) {
                first_new = None;
                out.push(ui::thread_divider(format!("{new} new"), true).into_any_element());
            }
            let time = local.map(|local| local.time()).unwrap_or_default();
            let who = match &entry.author {
                Author::Owner => "You".to_owned(),
                Author::Agent { name, .. } => name.clone(),
                Author::System => "Backbone".to_owned(),
            };
            let label = entry
                .anchor
                .as_ref()
                .and_then(|anchor| anchor.label.clone());
            out.push(match &entry.body {
                Body::Message { text, delivery, .. } => {
                    let from_you = matches!(entry.author, Author::Owner);
                    let failed = *delivery == Some(stored::Delivery::Failed);
                    let meta = ui::message_meta().children(label.map(ui::anchor));
                    let id = entry.id;
                    let redelivery = stored.redelivery.get(&id);
                    let meta = if failed
                        && matches!(redelivery, Some(Redelivery::Asking | Redelivery::Queued(_)))
                    {
                        meta.child(ui::sending_spinner()).child("Sending…")
                    } else if failed {
                        let why = match redelivery {
                            Some(Redelivery::Note(why)) => Some(why.clone()),
                            _ => None,
                        };
                        meta.text_color(c.attention_text)
                            .child(match why {
                                Some(why) => format!("Not sent: {why} ·"),
                                None => "Not sent ·".to_owned(),
                            })
                            .child(ui::link(
                                div()
                                    .id(SharedString::from(format!("conversation-redeliver-{id}")))
                                    .underline()
                                    .child("Retry")
                                    .on_click(cx.listener(move |conversation, _, _, cx| {
                                        conversation.redeliver(id, cx)
                                    })),
                            ))
                    } else {
                        meta.child(format!("{who} · {time}"))
                    };
                    ui::message(from_you, failed, meta, text.clone()).into_any_element()
                }
                Body::Revision { revision } => {
                    let changed: Vec<&str> = revision
                        .changed
                        .iter()
                        .map(|cell| cell.label.as_str())
                        .collect();
                    let mut happened = format!("published {}", revision.label);
                    if !changed.is_empty() {
                        happened.push_str(&format!(" · changed {}", changed.join(", ")));
                    }
                    ui::thread_event("history", who, format!("{happened} · {time}"), None)
                        .into_any_element()
                }
                Body::Step { step } => {
                    let happened = match step.status.as_str() {
                        "in_progress" => format!("started {}", step.title),
                        "done" => format!("finished {}", step.title),
                        "blocked" => format!("is blocked on {}", step.title),
                        _ => format!("queued {}", step.title),
                    };
                    ui::thread_event(
                        "corner-down-right",
                        who,
                        format!("{happened} · {time}"),
                        None,
                    )
                    .into_any_element()
                }
            });
        }
        out
    }

    fn mark(&self) -> Option<gpui::Div> {
        self.record
            .as_ref()
            .and_then(|(item, _)| reports::mark(item))
    }

    fn thread(&self, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let kind = self.about.trim_start_matches("about this ");
        let now = Local::now();
        let mut day: Option<Local> = None;
        let entries = self.entries(now, &mut day, cx);
        let local = self
            .messages
            .iter()
            .any(|message| !matches!(message.delivery, Delivery::Moved | Delivery::Stored));
        if entries.is_empty() && !local {
            let agent = self.agent();
            let note = if let (Some(error), false) = (self.read_error, self.stored_live()) {
                error.message().to_owned()
            } else if self.stored_live() {
                format!(
                    "Your messages, {agent}'s replies and the changes it publishes stay together beside this {kind}."
                )
            } else {
                format!("Messages you send here go to {agent} with a reference to this {kind}.")
            };
            return div()
                .flex_1()
                .min_h_0()
                .child(ui::empty_state(
                    ui::empty_tile("message-square", c.selected, c.text_2),
                    format!("Talk to {agent} here"),
                    note,
                ))
                .into_any_element();
        }
        // The handle still describes the previous frame here. Preserve an
        // older reading position, or follow the bottom through the new layout.
        if self.scroll.bounds().size.height <= px(0.)
            || at_latest(
                self.scroll.bounds().size.height,
                self.scroll.offset().y,
                self.scroll.max_offset().y,
            )
        {
            self.scroll.scroll_to_bottom();
        }
        let mut thread = div()
            .id("conversation-thread")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.space.s12))
            .p(px(t.space.s14))
            .children(entries);
        for (index, message) in self.messages.iter().enumerate() {
            if matches!(message.delivery, Delivery::Moved | Delivery::Stored) {
                continue;
            }
            let local = Local::at(message.at);
            if let (Some(local), Some(now)) = (local, now)
                && day.is_none_or(|day| !day.same_day(&local))
            {
                thread = thread.child(ui::thread_divider(local.day_name(&now), false));
                day = Some(local);
            }
            let time = local.map(|local| local.time()).unwrap_or_default();
            let meta = ui::message_meta().children(message.anchor.clone().map(ui::anchor));
            let action = |id: &str, label: &'static str| {
                ui::link(
                    div()
                        .id(SharedString::from(format!("conversation-{id}-{index}")))
                        .text_color(c.attention_text)
                        .underline()
                        .child(label),
                )
            };
            let meta = match &message.delivery {
                Delivery::Moved | Delivery::Stored => continue,
                Delivery::Sending => meta.child(ui::sending_spinner()).child("Sending…"),
                Delivery::Sent => meta.child(format!("You · {time}")),
                Delivery::NotSent(reason) => {
                    meta.text_color(c.attention_text)
                        .child(SharedString::from(format!("Not sent: {reason}")))
                        .when(message.via != Some(Via::Owner), |meta| {
                            meta.child(action("retry", "Retry").on_click(cx.listener(
                                move |conversation, _, _, cx| conversation.retry(index, cx),
                            )))
                        })
                        .child(action("edit", "Edit").on_click(cx.listener(
                            move |conversation, _, window, cx| conversation.edit(index, window, cx),
                        )))
                }
                Delivery::Unconfirmed(reason) if message.via != Some(Via::Feedback) => meta
                    .child(SharedString::from(format!("Not confirmed: {reason}")))
                    .child(ui::link(
                        div()
                            .id(SharedString::from(format!("conversation-retry-{index}")))
                            .underline()
                            .child("Retry the same request")
                            .on_click(cx.listener(move |conversation, _, _, cx| {
                                conversation.retry(index, cx)
                            })),
                    )),
                Delivery::Unconfirmed(reason) => meta.child(SharedString::from(format!(
                    "Not confirmed: {reason} Ask {} before sending it again.",
                    self.agent
                ))),
            };
            thread = thread.child(ui::message(
                true,
                matches!(message.delivery, Delivery::NotSent(_)),
                meta,
                message.text.clone(),
            ));
        }
        let scroll = self.scroll.clone();
        let entity = cx.entity().downgrade();
        let last = self.stored.as_ref().and_then(Stored::last);
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(thread)
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, (), _, cx| {
                        // Paint runs after layout has applied scrolling. The
                        // captured ID belongs to this frame, not a future poll.
                        if let Some(last) = last
                            && at_latest(
                                scroll.bounds().size.height,
                                scroll.offset().y,
                                scroll.max_offset().y,
                            )
                        {
                            let entity = entity.clone();
                            cx.defer(move |cx| {
                                let _ = entity.update(cx, |conversation, cx| {
                                    conversation.mark_read(last, cx);
                                });
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }
}

fn read_marker_due(last_attempt: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last_attempt.is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(5))
}

fn at_latest(height: gpui::Pixels, offset: gpui::Pixels, maximum: gpui::Pixels) -> bool {
    height > px(0.) && offset + maximum <= px(1.)
}

/// Seconds since the epoch for an RFC 3339 time.
fn epoch(text: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp_millis() as f64 / 1000.0)
}

impl Render for Conversation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel(None, window, cx)
    }
}

impl Conversation {
    /// Draw the shared thread and composer with the reader's selected context.
    pub fn panel(
        &mut self,
        context: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A skill or policy Backbone keeps no conversation for shows none.
        if self.record.is_none()
            && self
                .stored
                .as_ref()
                .is_some_and(|stored| stored.source.is_some() && stored.supported == Some(false))
            && self.disposable()
        {
            return div().into_any_element();
        }
        if std::mem::take(&mut self.toggle_requested) {
            self.toggle(window, cx);
        }
        if !self.open {
            let rail = self.rail(cx);
            return match context {
                Some(context) => ui::conversation_panel()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(context),
                    )
                    .child(rail)
                    .into_any_element(),
                None => rail,
            };
        }
        let t = tokens();
        let can_send = self.can_send(cx);
        ui::conversation_panel()
            .child(ui::conversation_header(
                self.mark(),
                self.agent(),
                self.about,
                ui::icon_button("conversation-close", "panel-right-close").on_click(
                    cx.listener(|conversation, _, window, cx| conversation.close(window, cx)),
                ),
            ))
            .children(context)
            .children(self.chooser(cx))
            .child(self.thread(cx))
            .child(ui::composer(
                div()
                    .relative()
                    .text_size(t.text.row.size())
                    .child(self.field.clone())
                    // The board's placeholder, until something is typed.
                    .when(self.draft(cx).is_empty(), |el| {
                        el.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .text_color(colors().faint)
                                .child(format!("Message {} {}…", self.agent(), self.about)),
                        )
                    }),
                self.anchor.clone().map(|label| {
                    ui::anchor(label)
                        .max_w(gpui::relative(0.6))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                }),
                ui::send_button("conversation-send", can_send)
                    .on_click(cx.listener(|conversation, _, _, cx| conversation.send(cx))),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_read_receipts_wait_for_the_poll_interval_despite_repaints() {
        let attempt = std::time::Instant::now();
        assert!(read_marker_due(None, attempt));
        for frame in 0..300 {
            assert!(!read_marker_due(
                Some(attempt),
                attempt + Duration::from_millis(frame * 16),
            ));
        }
        assert!(read_marker_due(
            Some(attempt),
            attempt + Duration::from_secs(5)
        ));
    }

    #[test]
    fn arrivals_are_unread_until_the_scrolled_view_reaches_them() {
        // A 300px panel is following a 900px thread. Another 100px reply
        // arrives: the old offset must not qualify for a read receipt.
        assert!(at_latest(px(300.), px(-600.), px(600.)));
        assert!(!at_latest(px(300.), px(-600.), px(700.)));
        assert!(at_latest(px(300.), px(-700.), px(700.)));
        // A reader back in history, and a panel not yet laid out, cannot
        // acknowledge the newest entry. Short threads need no scrolling.
        assert!(!at_latest(px(300.), px(-200.), px(700.)));
        assert!(!at_latest(px(0.), px(0.), px(0.)));
        assert!(at_latest(px(300.), px(0.), px(0.)));
    }

    #[test]
    fn stored_times_read_as_seconds_since_the_epoch() {
        assert_eq!(epoch("1970-01-01T00:01:00.5Z"), Some(60.5));
        assert_eq!(epoch("2026-10-27T14:41:03+02:00"), Some(1_793_104_863.0));
        assert_eq!(epoch("yesterday"), None);
    }

    fn page(ids: &[u64], older: Option<&str>) -> stored::Conversation {
        let entries: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({"id": id, "kind": "message", "created_at": "2026-10-27T14:00:00Z",
                    "author": {"type": "owner"}, "text": "hi"})
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "kind": "report", "artifact_id": "1",
            "responsible": {"agent_id": "a", "name": "builder", "status": "active"},
            "total": 0, "unread": 0, "read_through": null,
            "entries": entries, "older": older, "newer": null
        }))
        .unwrap()
    }

    #[test]
    fn polling_keeps_the_thread_continuous() {
        let mut stored = Stored {
            directory: PathBuf::new(),
            provider: String::new(),
            kind: Kind::Report,
            source: None,
            id: "1".into(),
            supported: Some(true),
            page: None,
            fetched: Vec::new(),
            earlier: Vec::new(),
            earlier_cursor: None,
            divider: None,
            reading: false,
            read_at: None,
            marking: None,
            mark_at: None,
            redelivery: HashMap::new(),
            turn: 0,
        };
        let ids = |entries: &[stored::Entry]| entries.iter().map(|e| e.id).collect::<Vec<_>>();
        stored.receive(page(&[2, 3, 4], Some("before-2")));
        // The newest page moved on by one: entry 2 joins the earlier ones and
        // paging still continues from before it.
        stored.receive(page(&[3, 4, 5], Some("before-3")));
        assert_eq!(ids(&stored.earlier), [2]);
        assert_eq!(stored.earlier_cursor.as_deref(), Some("before-2"));
        // Nothing shared: entries in between may be missing, so start again.
        stored.receive(page(&[9, 10], Some("before-9")));
        assert!(stored.earlier.is_empty());
        assert_eq!(stored.earlier_cursor.as_deref(), Some("before-9"));
        assert_eq!(stored.entry_mut(10).map(|e| e.id), Some(10));
        // An entry a send added survives a poll read before it, and does not
        // stand in for continuity.
        stored
            .page
            .as_mut()
            .unwrap()
            .entries
            .push(page(&[11], None).entries.remove(0));
        stored.receive(page(&[9, 10], Some("before-9")));
        assert_eq!(ids(&stored.page.as_ref().unwrap().entries), [9, 10, 11]);
        // The read marker stops at what Backbone returned, not the sent entry.
        assert_eq!(stored.last(), Some(10));
        // More than 50 older messages awaiting delivery are checked in turns.
        stored.earlier = (1..=60)
            .map(|id| {
                serde_json::from_value(serde_json::json!({"id": id, "kind": "message",
                    "created_at": "2026-10-27T14:00:00Z", "author": {"type": "owner"},
                    "text": "hi", "delivery": "pending"}))
                .unwrap()
            })
            .collect();
        let first = stored.unsettled();
        let second = stored.unsettled();
        assert_eq!((first.len(), first[0], second[0]), (50, 1, 51));
        assert!(second.contains(&60));
        stored
            .page
            .as_mut()
            .unwrap()
            .entries
            .push(page(&[30], None).entries.remove(0));
        stored.receive(page(&[29, 30], Some("before-29")));
        assert_eq!(stored.earlier_cursor.as_deref(), Some("before-29"));
    }
}
