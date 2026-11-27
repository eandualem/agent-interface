//! The Access pane: what each agent may reach, who is above whom, and the
//! requests for more. It reads Agent Backbone live and draws boards A (by
//! agent) and E (requests); every change goes to Backbone, and the pane shows
//! what Backbone returns, a refusal and its reason included.
use super::{NameEvent, NameField, colors, tokens, ui};
use agent_interface_roster::profiles::{
    Connection, Error,
    access::{
        Access, AgentAccess, Failure, Grant, Mode, Overview, Placed, Request, RequestPage,
        RequestState, Requests, Role,
    },
    browsers::{self as served, Browsers, Create, Detail, Policies, Save, Session},
};
use futures::StreamExt as _;
use gpui::{Context, Div, Entity, Render, Subscription, Task, Window, div, prelude::*, px};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod agents;
mod browsers;
mod model;
mod requests;

/// How often the pane reads Backbone again, as the other live views do.
const EVERY: Duration = Duration::from_secs(3);

/// What one read of the pane returns: the agents, the waiting requests, the
/// browsers and the shown browser's detail.
type Read = (Overview, RequestPage, Option<Browsers>, Option<Detail>);

/// Which of the pane's two views is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    Agents,
    Requests,
    Browsers,
}

/// Which small menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Role,
    Superior,
    AgentFilter,
    /// A browser's More menu.
    BrowserMore,
    /// The agents a browser can be given to.
    BrowserUser,
    /// The orchestrators and coordinators that can be let give it out.
    BrowserGiver,
    /// The agents a new browser's sheet can offer besides its chips.
    BrowserAnother,
}

/// A thing being added to the chosen agent, with the access chosen for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adding {
    Repository(Access),
    Folder(Access),
}

/// A change sent to Backbone.
#[derive(Clone, Debug)]
enum Change {
    Grant(String, Grant),
    Revoke(String, Grant),
    Mode(String, Mode),
    Place(String, Role, Option<String>),
    /// Approve or refuse these requests, with the decider's note.
    Decide(Vec<i64>, bool, String),
    Browser(BrowserChange),
}

/// A change to a named browser, by its ID.
#[derive(Clone, Debug)]
enum BrowserChange {
    Create(Create),
    Show(String),
    Save(String, Save),
    Check(String),
    Policies(String, Policies),
    TakeBack(String),
    Delete(String),
    /// Set an agent's whole ceiling, with or without a browser.
    Ceiling(String, Vec<Grant>),
}

/// What Backbone returned for a change.
enum Applied {
    Access(AgentAccess),
    Placed(Placed),
    /// The requests decided, and why one after them was not.
    Decided(Vec<Request>, Option<Failure>),
    /// A browser as Backbone returned it.
    Browser(Box<Detail>),
    /// A browser taken back or deleted, by its ID and whether it was
    /// deleted.
    Released(String, bool),
}

/// A line at the top of the pane about the last change: a warning Backbone
/// served with it, or why it refused it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    text: String,
    refused: bool,
}

pub(super) struct Pane {
    connection: Connection,
    visible: Rc<Cell<bool>>,
    /// The records last read; none before the first read.
    model: Option<model::Model>,
    /// When they were read, as HH:MM.
    read_at: Option<String>,
    /// Why the last read failed, shown over the records still drawn.
    error: Option<String>,
    reading: bool,
    /// A read asked for while one was under way, made when it ends.
    again: bool,
    /// How many change replies have been applied, so a read that started
    /// before one is not shown over it.
    writes: u64,
    /// Whether a page of decided requests is being read.
    reading_decided: bool,
    shown: Shown,
    filter: model::Filter,
    query: Entity<NameField>,
    /// The chosen agent, by name.
    selected: Option<String>,
    /// Swarms shown with their members.
    open_swarms: HashSet<String>,
    /// Whether every main agent is listed, not only the first few.
    all_main: bool,
    /// This profile's name, for the header.
    profile_name: Option<String>,
    menu: Option<Menu>,
    adding: Option<Adding>,
    add_field: Entity<NameField>,
    /// The control whose change is under way; one change at a time.
    sending: Option<String>,
    notice: Option<Notice>,
    tab: requests::Tab,
    /// The agent whose requests are listed; none for all agents.
    agent_filter: Option<String>,
    /// The `before` that reads older decided requests, when there are more.
    decided_before: Option<i64>,
    /// The request card the keys act on, by its first request's ID.
    card: Option<i64>,
    /// The request card whose note is open.
    noting: Option<i64>,
    note_field: Entity<NameField>,
    /// What had focus before the add or note field took it, to give it back.
    return_focus: Option<gpui::FocusHandle>,
    /// The menu a press outside just closed, so the press on its own button
    /// does not open it again.
    dismissed: Option<(Menu, std::time::Instant)>,
    /// The browsers view: the chosen browser and the new browser sheet.
    browsing: browsers::Browsing,
    /// Whether the next read is the poll's, which may check a new browser's
    /// sign-in.
    check_due: bool,
    /// Whether the browsers' newest sessions are to be read again, and
    /// whether a read of them is under way.
    newest_due: bool,
    reading_newest: bool,
    /// The new browser's name and address.
    name_field: Entity<NameField>,
    opens_field: Entity<NameField>,
    _poll: Task<()>,
    _stream: BrowserStream,
    _fields: [Subscription; 5],
}

/// The worker that follows `browser.changed`, stopped when the pane goes.
struct BrowserStream {
    live: Arc<AtomicBool>,
    _changes: Task<()>,
}

impl Drop for BrowserStream {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Relaxed);
    }
}

impl Pane {
    pub(super) fn new(
        connection: Connection,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut field = NameField::bare_filter(window, cx);
                field.focus = field.focus.clone().tab_stop(true);
                field
            })
        };
        let query = field(window, cx);
        let add_field = cx.new(|cx| NameField::bare("", window, cx));
        let note_field = field(window, cx);
        let name_field = cx.new(|cx| NameField::bare("", window, cx));
        let opens_field = cx.new(|cx| NameField::bare("", window, cx));
        let changed = cx.subscribe(&query, |_, _, event: &NameEvent, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                cx.notify();
            }
        });
        // Leaving a field gives the keys back to the pane.
        let added = cx.subscribe_in(
            &add_field,
            window,
            |pane, _, event: &NameEvent, window, cx| match event {
                NameEvent::Save => {
                    if pane.submit_add(cx) {
                        pane.leave_field(window, cx);
                    }
                }
                NameEvent::Cancel => {
                    pane.adding = None;
                    pane.leave_field(window, cx);
                }
                NameEvent::Changed(_) => cx.notify(),
            },
        );
        // Enter keeps the note for Approve or Refuse; Escape drops it.
        let noted = cx.subscribe_in(
            &note_field,
            window,
            |pane, _, event: &NameEvent, window, cx| match event {
                NameEvent::Cancel => {
                    pane.noting = None;
                    pane.leave_field(window, cx);
                }
                NameEvent::Save => pane.leave_field(window, cx),
                NameEvent::Changed(_) => cx.notify(),
            },
        );
        // Enter in the new browser's fields opens it, or keeps a new name;
        // Escape cancels the sheet.
        let new_field = |field: &Entity<NameField>, window: &mut Window, cx: &mut Context<Self>| {
            cx.subscribe_in(
                field,
                window,
                |pane, _, event: &NameEvent, window, cx| match event {
                    NameEvent::Save => {
                        if pane
                            .browsing
                            .new
                            .as_ref()
                            .is_some_and(|new| new.id.is_none())
                        {
                            pane.create_browser(cx);
                        } else if let Some(new) = &mut pane.browsing.new {
                            new.renaming = false;
                            new.before_rename = None;
                        }
                        pane.leave_field(window, cx);
                    }
                    NameEvent::Cancel => pane.escape_new(window, cx),
                    NameEvent::Changed(_) => cx.notify(),
                },
            )
        };
        let named = new_field(&name_field, window, cx);
        let opened = new_field(&opens_field, window, cx);
        let stream = BrowserStream::start(connection.clone(), cx);
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let polled = this.update(cx, |pane, cx| {
                    pane.check_due = true;
                    pane.read(cx);
                });
                if polled.is_err() {
                    break;
                }
                cx.background_executor().timer(EVERY).await;
            }
        });
        Self {
            connection,
            visible,
            model: None,
            read_at: None,
            error: None,
            reading: false,
            again: false,
            writes: 0,
            reading_decided: false,
            shown: Shown::Agents,
            filter: model::Filter::All,
            query,
            selected: None,
            open_swarms: HashSet::new(),
            all_main: false,
            profile_name: None,
            menu: None,
            adding: None,
            add_field,
            sending: None,
            notice: None,
            tab: requests::Tab::Waiting,
            agent_filter: None,
            decided_before: None,
            card: None,
            noting: None,
            note_field,
            return_focus: None,
            dismissed: None,
            browsing: browsers::Browsing::default(),
            check_due: false,
            newest_due: true,
            reading_newest: false,
            name_field,
            opens_field,
            _poll: poll,
            _stream: stream,
            _fields: [changed, added, noted, named, opened],
        }
    }

    /// Tell the pane its profile's name, which can arrive after it was made.
    /// Says whether it changed.
    pub(super) fn place(&mut self, name: Option<&str>) -> bool {
        let name = name.map(str::to_owned);
        let changed = self.profile_name != name;
        self.profile_name = name;
        changed
    }

    /// Read the agents and waiting requests, unless the pane is hidden. A
    /// read asked for while one is under way follows it, so a change's
    /// result is never left behind an older read.
    fn read(&mut self, cx: &mut Context<Self>) {
        if !self.visible.get() {
            return;
        }
        if self.reading {
            self.again = true;
            return;
        }
        self.reading = true;
        let writes = self.writes;
        let connection = self.connection.clone();
        let shown = self.browsing.shown_id(self.shown == Shown::Browsers);
        // Backbone reads a new browser's sign-in only when it is checked, so
        // the poll checks it while the sheet waits for one and its Chrome
        // runs; a read the check itself sets off does not check again.
        let check = self.browsing.waits_for_sign_in() && std::mem::take(&mut self.check_due);
        let read = cx.background_executor().spawn(async move {
            let overview = connection.access_overview()?;
            let pending = connection.access_requests(Requests::Pending)?;
            // Browsers only add to the pane, so their own failure keeps the
            // last ones read.
            let browsers = connection.browsers(None).ok();
            let detail = shown.and_then(|id| match check {
                true => connection.check_browser(&id).ok(),
                false => connection.browser(&id).ok(),
            });
            Ok::<Read, Error>((overview, pending, browsers, detail))
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.reading = false;
                match result {
                    // Read before a change's reply was applied: the read that
                    // follows the change shows the newer state.
                    Ok(_) if pane.writes != writes => pane.again = true,
                    Ok((overview, pending, browsers, detail)) => {
                        let model = pane.model.get_or_insert_with(Default::default);
                        model.overview = overview;
                        model.pending = pending.items;
                        if let Some(browsers) = browsers {
                            model.browsers = browsers;
                        }
                        pane.browsing.read(detail);
                        pane.keep_new();
                        pane.keep_browser();
                        pane.read_at = Some(chrono::Local::now().format("%H:%M").to_string());
                        pane.error = None;
                        pane.keep_selection();
                    }
                    Err(error) => pane.error = Some(failure(error)),
                }
                if std::mem::take(&mut pane.again) {
                    pane.read(cx);
                }
                pane.read_newest(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read each browser's newest session, for the shown agent's browser
    /// rows, once it is due: when the shown agent changes or a browser
    /// changes. It runs beside the pane's own read, so a slow one holds
    /// neither up.
    fn read_newest(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        if !self.newest_due
            || self.reading_newest
            || self.shown != Shown::Agents
            || self.selected.is_none()
        {
            return;
        }
        self.newest_due = false;
        self.reading_newest = true;
        let ids: Vec<String> = model.browsers.items.iter().map(|b| b.id.clone()).collect();
        let connection = self.connection.clone();
        let read = cx.background_executor().spawn(async move {
            ids.into_iter()
                .filter_map(|id| {
                    let newest = connection.browser_sessions(&id, None, Some(1)).ok()?;
                    Some((id, newest.items.into_iter().next()?))
                })
                .collect::<HashMap<String, Session>>()
        });
        cx.spawn(async move |this, cx| {
            let newest = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.reading_newest = false;
                if let Some(model) = pane.model.as_mut() {
                    model.newest = newest;
                }
                // Due again while this read was under way.
                pane.read_newest(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read decided requests: the newest page, or the page older than
    /// `before`, added below those already read.
    fn read_decided(&mut self, before: Option<i64>, cx: &mut Context<Self>) {
        if self.reading_decided {
            return;
        }
        self.reading_decided = true;
        let connection = self.connection.clone();
        let read = cx
            .background_executor()
            .spawn(async move { connection.access_requests(Requests::All { before }) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.reading_decided = false;
                match result {
                    Ok(page) => {
                        let model = pane.model.get_or_insert_with(Default::default);
                        let decided = page
                            .items
                            .into_iter()
                            .filter(|r| r.state != RequestState::Pending);
                        match (&mut model.decided, before) {
                            // A request decided here may already lead the list.
                            (Some(read), Some(_)) => {
                                let older: Vec<Request> = decided
                                    .filter(|r| read.iter().all(|shown| shown.id != r.id))
                                    .collect();
                                read.extend(older);
                            }
                            (read, _) => *read = Some(decided.collect()),
                        }
                        pane.decided_before = page.next_before_id.filter(|_| page.has_more);
                    }
                    Err(error) => pane.error = Some(failure(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the chosen agent while it is listed; otherwise choose the first
    /// one the list shows.
    fn keep_selection(&mut self) {
        let Some(model) = &self.model else { return };
        if self
            .selected
            .as_ref()
            .is_some_and(|name| model.agent(name).is_some())
        {
            return;
        }
        let groups = model::groups(&model.overview, model::Filter::All);
        self.newest_due = true;
        self.selected = groups
            .orchestrators
            .iter()
            .chain(&groups.coordinators)
            .chain(&groups.main)
            .chain(groups.swarms.iter().flat_map(|swarm| &swarm.members))
            .next()
            .map(|agent| agent.name.clone());
    }

    /// Send `change` to Backbone from the control `key`, unless another
    /// change is under way, then show what Backbone returned and read again.
    fn send(&mut self, key: String, change: Change, cx: &mut Context<Self>) {
        if self.sending.is_some() {
            return;
        }
        self.sending = Some(key);
        self.notice = None;
        self.menu = None;
        let connection = self.connection.clone();
        let sent = cx.background_executor().spawn(async move {
            match change {
                Change::Grant(name, grant) => connection.grant(&name, &grant).map(Applied::Access),
                Change::Revoke(name, grant) => {
                    connection.revoke(&name, &grant).map(Applied::Access)
                }
                Change::Mode(name, mode) => connection.set_mode(&name, mode).map(Applied::Access),
                Change::Place(name, role, superior) => connection
                    .place(&name, role, superior.as_deref())
                    .map(Applied::Placed),
                Change::Decide(ids, approve, note) => {
                    let mut decided = Vec::new();
                    for id in ids {
                        let reply = if approve {
                            connection.approve(id, &note)
                        } else {
                            connection.refuse(id, &note)
                        };
                        // What was decided before a refusal stays decided;
                        // the read that follows shows the rest still waiting.
                        match reply {
                            Ok(request) => decided.push(request),
                            Err(failure) if decided.is_empty() => return Err(failure),
                            Err(failure) => return Ok(Applied::Decided(decided, Some(failure))),
                        }
                    }
                    Ok(Applied::Decided(decided, None))
                }
                Change::Browser(change) => match change {
                    BrowserChange::Create(create) => connection
                        .create_browser(&create)
                        .map(|detail| Applied::Browser(Box::new(detail))),
                    BrowserChange::Show(id) => connection
                        .show_browser(&id)
                        .map(|detail| Applied::Browser(Box::new(detail))),
                    BrowserChange::Save(id, save) => connection
                        .save_browser(&id, &save)
                        .map(|detail| Applied::Browser(Box::new(detail))),
                    BrowserChange::Check(id) => connection
                        .check_browser(&id)
                        .map(|detail| Applied::Browser(Box::new(detail))),
                    BrowserChange::Policies(id, policies) => connection
                        .set_browser_policies(&id, &policies)
                        .map(|detail| Applied::Browser(Box::new(detail))),
                    BrowserChange::TakeBack(id) => connection
                        .take_back_browser(&id)
                        .map(|()| Applied::Released(id, false)),
                    BrowserChange::Delete(id) => connection
                        .delete_browser(&id)
                        .map(|()| Applied::Released(id, true)),
                    BrowserChange::Ceiling(name, ceiling) => {
                        connection.set_ceiling(&name, &ceiling).map(Applied::Access)
                    }
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let result = sent.await;
            let _ = this.update(cx, |pane, cx| {
                let key = pane.sending.take().unwrap_or_default();
                if matches!(result, Ok(Applied::Decided(..))) {
                    pane.noting = None;
                }
                if result.is_ok() {
                    pane.writes += 1;
                }
                // A browser cancelled that is gone already is as good as
                // deleted.
                let result = match result {
                    Err(Failure::Refused { code, .. })
                        if key == "access-browser-cancel" && code == "not_found" =>
                    {
                        let gone = pane.browsing.new.as_ref().and_then(|new| new.id.clone());
                        Ok(Applied::Released(gone.unwrap_or_default(), true))
                    }
                    other => other,
                };
                // A browser's reply is shown in the browsers view itself.
                let unsettled = match result {
                    Ok(Applied::Browser(detail)) => {
                        pane.browsed(&key, *detail);
                        None
                    }
                    Ok(Applied::Released(id, deleted)) => {
                        pane.released(&id, deleted);
                        None
                    }
                    other => Some(other),
                };
                pane.notice = match (unsettled, pane.model.as_mut()) {
                    (Some(result), Some(model)) => settle(model, result),
                    _ => None,
                };
                pane.read(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Add what the add field names to the chosen agent.
    /// Says whether it was sent: an empty field, or one while another change
    /// is under way, stays open with its text.
    fn submit_add(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(adding), Some(name)) = (self.adding, self.selected.clone()) else {
            return false;
        };
        let value = self.add_field.read(cx).value().trim().to_owned();
        if value.is_empty() || self.sending.is_some() {
            return false;
        }
        let grant = match adding {
            Adding::Repository(access) => Grant::Repository {
                repo: value,
                access,
            },
            Adding::Folder(access) => Grant::Folder {
                machine: None,
                machine_name: None,
                path: value,
                access,
            },
        };
        self.adding = None;
        self.add_field.update(cx, |field, cx| field.clear(cx));
        self.send("access-add".into(), Change::Grant(name, grant), cx);
        true
    }

    /// Open the add field for `adding`, or change the access it will add.
    fn start_add(&mut self, adding: Adding, window: &mut Window, cx: &mut Context<Self>) {
        let same_kind = matches!(
            (self.adding, adding),
            (Some(Adding::Repository(_)), Adding::Repository(_))
                | (Some(Adding::Folder(_)), Adding::Folder(_))
        );
        if !same_kind {
            self.add_field.update(cx, |field, cx| field.clear(cx));
        }
        self.adding = Some(adding);
        let field = self.add_field.read(cx).focus.clone();
        self.enter_field(field, window, cx);
    }

    /// Give keyboard focus to one of the pane's fields, remembering what had
    /// it so leaving the field gives it back.
    fn enter_field(
        &mut self,
        field: gpui::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.in_field(window, cx) {
            self.return_focus = window.focused(cx);
        }
        window.focus(&field, cx);
        cx.notify();
    }

    /// Whether one of the pane's own fields has keyboard focus.
    fn in_field(&self, window: &Window, cx: &gpui::App) -> bool {
        [
            &self.add_field,
            &self.note_field,
            &self.name_field,
            &self.opens_field,
        ]
        .iter()
        .any(|field| field.read(cx).focus.is_focused(window))
    }

    /// Give focus back to what had it before a field took it. It is kept,
    /// as a kept note can be clicked into again without passing through
    /// [`Self::enter_field`].
    fn leave_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_field(window, cx)
            && let Some(focus) = self.return_focus.clone()
        {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Show the requests, or the agents again.
    fn toggle_requests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = match self.shown {
            Shown::Agents | Shown::Browsers => Shown::Requests,
            Shown::Requests => Shown::Agents,
        };
        self.browsing.deleting = None;
        self.menu = None;
        self.noting = None;
        self.adding = None;
        self.leave_field(window, cx);
    }

    /// Keys while the pane is selected: Escape closes what is open or returns
    /// from the requests to the agents; in the requests, ↑↓ move between
    /// cards and a, r and n approve, refuse and open a note. Returns whether
    /// the key was one of them.
    pub(super) fn key(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.modifiers.modified() {
            return false;
        }
        // A deletion waiting to be confirmed is kept by Escape and Return.
        if matches!(key.key.as_str(), "escape" | "enter")
            && self.shown == Shown::Browsers
            && self.browsing.confirming()
        {
            self.browsing.deleting = None;
            cx.notify();
            return true;
        }
        if key.key == "escape" {
            let closed = self.menu.take().is_some()
                || self.adding.take().is_some()
                || self.noting.take().is_some();
            if !closed {
                match self.shown {
                    Shown::Agents => return false,
                    Shown::Browsers if self.browsing.new.is_some() => self.escape_new(window, cx),
                    Shown::Requests | Shown::Browsers => self.shown = Shown::Agents,
                }
            }
            cx.notify();
            return true;
        }
        let typing = [&self.query, &self.add_field, &self.note_field]
            .iter()
            .any(|field| field.read(cx).focus.is_focused(window));
        if typing || self.shown != Shown::Requests || self.tab != requests::Tab::Waiting {
            return false;
        }
        let cards = self.card_order();
        let at = self
            .card
            .and_then(|card| cards.iter().position(|ids| ids.last() == Some(&card)));
        match key.key.as_str() {
            "down" | "up" => {
                let next = match (at, key.key.as_str()) {
                    (None, _) => 0,
                    (Some(at), "down") => (at + 1).min(cards.len().saturating_sub(1)),
                    (Some(at), _) => at.saturating_sub(1),
                };
                self.card = cards.get(next).and_then(|ids| ids.last().copied());
            }
            "a" | "r" => {
                let Some(ids) = at.and_then(|at| cards.get(at)).cloned() else {
                    return false;
                };
                self.decide(ids, key.key == "a", window, cx);
            }
            "n" => {
                let Some(first) = at.and_then(|at| cards.get(at)).and_then(|ids| ids.last()) else {
                    return false;
                };
                self.open_note(*first, window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The waiting requests the requests view lists, for its agent filter.
    fn listed_pending(&self) -> Vec<Request> {
        self.model
            .iter()
            .flat_map(|model| &model.pending)
            .filter(|r| self.agent_filter.as_ref().is_none_or(|a| &r.agent == a))
            .cloned()
            .collect()
    }

    /// The waiting cards in the order they are drawn, each by its requests'
    /// IDs.
    fn card_order(&self) -> Vec<Vec<i64>> {
        let pending = self.model.as_ref().map(|m| &m.pending[..]).unwrap_or(&[]);
        model::card_order(pending, self.agent_filter.as_deref())
    }

    /// Approve or refuse a card's requests, with its note when one is open.
    fn decide(
        &mut self,
        ids: Vec<i64>,
        approve: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_field(window, cx);
        let first = model::card_key(&ids);
        let note = if self.noting == Some(first) {
            self.note_field.read(cx).value().trim().to_owned()
        } else {
            String::new()
        };
        self.decide_with(ids, approve, note, cx);
    }

    fn decide_with(&mut self, ids: Vec<i64>, approve: bool, note: String, cx: &mut Context<Self>) {
        let first = model::card_key(&ids);
        self.card = Some(first);
        self.send(
            format!("access-decide-{first}"),
            Change::Decide(ids, approve, note),
            cx,
        );
    }

    /// Open the note under the card whose first request is `first`.
    fn open_note(&mut self, first: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.noting != Some(first) {
            self.note_field.update(cx, |field, cx| field.clear(cx));
        }
        self.noting = Some(first);
        self.card = Some(first);
        let field = self.note_field.read(cx).focus.clone();
        self.enter_field(field, window, cx);
    }

    /// "live from Agent Backbone · read 10:31", once something was read.
    fn live_note(&self) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        let at = self.read_at.as_ref()?;
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s6))
                .text_color(c.muted)
                .child(ui::dot(c.ok))
                .child(format!("live from Agent Backbone · read {at}")),
        )
    }

    /// The last change's notice, with the button that dismisses it: a served
    /// warning in the accent tint, a refusal in the attention tint.
    fn notice_band(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        let notice = self.notice.as_ref()?;
        let (fill, tint) = if notice.refused {
            (c.attention_soft, c.attention_text)
        } else {
            (c.accent_soft, c.accent_text)
        };
        Some(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_start()
                    .gap(px(t.space.s8))
                    .pl(px(t.space.s12))
                    .pr(px(t.space.s6))
                    .py(px(t.space.s6))
                    .bg(fill)
                    .border_b(px(t.size.hairline))
                    .border_color(c.line)
                    .text_color(c.text),
                &t.text.status,
            )
            .child(ui::icon("triangle-alert", t.size.icon - 1.0, tint).mt(px(t.space.s4)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(t.space.s2))
                    .child(notice.text.clone()),
            )
            .child(
                ui::icon_button("access-notice-dismiss", "x").on_click(cx.listener(
                    |pane, _, _, cx| {
                        pane.notice = None;
                        cx.notify();
                    },
                )),
            ),
        )
    }
}

/// Apply a change's result to `model` as Backbone returned it, and say what
/// Backbone warned of or why it refused. A refusal changes nothing, so the
/// controls keep what Backbone holds.
fn settle(model: &mut model::Model, result: Result<Applied, Failure>) -> Option<Notice> {
    let refused = |failure| Notice {
        text: refusal(failure),
        refused: true,
    };
    let warning = match result {
        Err(failure) => return Some(refused(failure)),
        Ok(Applied::Access(reply)) => {
            let warning = reply.warning.clone();
            model.apply_access(reply);
            warning
        }
        Ok(Applied::Placed(reply)) => {
            model.apply_placed(reply);
            None
        }
        Ok(Applied::Browser(_) | Applied::Released(..)) => None,
        Ok(Applied::Decided(replies, failure)) => {
            let warning = replies.iter().find_map(|r| r.warning.clone());
            for reply in replies {
                model.apply_decided(reply);
            }
            if let Some(failure) = failure {
                return Some(refused(failure));
            }
            warning
        }
    };
    warning.map(|text| Notice {
        text,
        refused: false,
    })
}

/// What the pane says when a read fails, over the records still drawn.
fn failure(error: Error) -> String {
    match error {
        Error::Unavailable => "Backbone is not answering. Showing the last data read.".into(),
        _ => format!(
            "{} Showing the last data read.",
            agent_interface_roster::views::UNAVAILABLE
        ),
    }
}

/// What the pane says for a change Backbone did not make: its own message,
/// or what is known when it gave none.
fn refusal(failure: Failure) -> String {
    match failure {
        Failure::Refused { message, .. } => message,
        Failure::Connection(Error::Refused(status)) => {
            format!("Agent Backbone did not make the change (HTTP {status}).")
        }
        Failure::Connection(Error::Unavailable) => {
            "Agent Backbone did not answer, so the change may not have been made.".into()
        }
        Failure::Connection(Error::Invalid) => {
            "This change cannot be sent to Agent Backbone.".into()
        }
        Failure::Connection(error) => {
            format!("Agent Backbone did not make the change ({error:?}).")
        }
    }
}

/// A band at the bottom of the pane, for a read that failed.
fn band(message: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s12))
            .py(px(t.space.s6))
            .bg(c.attention_soft)
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.attention_text),
        &t.text.meta,
    )
    .child(message)
}

impl gpui::EventEmitter<super::WatchBrowser> for Pane {}

impl BrowserStream {
    /// Follow `browser.changed` on an I/O worker of its own while the pane
    /// lives: a change replaces its browser in the list, or drops a deleted
    /// one, and the pane reads again, which reads the shown browser anew.
    fn start(connection: Connection, cx: &mut Context<Pane>) -> Self {
        let live = Arc::new(AtomicBool::new(true));
        let (sender, mut changes) = futures::channel::mpsc::unbounded();
        let following = Arc::clone(&live);
        let started = std::thread::Builder::new()
            .name("access browsers stream".into())
            .spawn(move || {
                let live = || following.load(Ordering::Relaxed);
                while live() {
                    let ended = connection.watch_browsers(&live, &mut |change| {
                        let _ = sender.unbounded_send(change);
                    });
                    if ended == Err(Error::Unsupported) {
                        break;
                    }
                    if live() {
                        std::thread::park_timeout(Duration::from_secs(2));
                    }
                }
            });
        if let Err(error) = started {
            tracing::debug!(event = "access_browsers_stream", %error, "Browser stream did not start");
        }
        let task = cx.spawn(async move |this, cx| {
            while let Some(change) = changes.next().await {
                let applied = this.update(cx, |pane, cx| {
                    if let (served::Change::Changed { id, browser, .. }, Some(model)) =
                        (change, pane.model.as_mut())
                    {
                        match browser {
                            Some(browser) => model.apply_browser(*browser),
                            None => model.remove_browser(&id),
                        }
                    }
                    pane.newest_due = true;
                    pane.keep_new();
                    pane.read(cx);
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        Self {
            live,
            _changes: task,
        }
    }
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let body = match (&self.model, self.shown) {
            (None, _) => match &self.error {
                Some(error) => ui::empty_state(
                    ui::empty_tile("shield", c.selected, c.muted),
                    "Access unavailable",
                    error.clone(),
                ),
                None => div().flex_1(),
            },
            (Some(_), Shown::Agents) => self.agents_view(cx),
            (Some(_), Shown::Requests) => self.requests_view(cx),
            (Some(_), Shown::Browsers) => self.browsers_view(cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(body)
            .children(
                self.error
                    .clone()
                    .filter(|_| self.model.is_some())
                    .map(band),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_roster::profiles::access::Agent;

    fn model() -> model::Model {
        let tester = Agent {
            name: "tester".into(),
            role: Some(Role::Main),
            ..Agent::default()
        };
        model::Model {
            overview: Overview {
                agents: vec![tester],
                swarms: Vec::new(),
            },
            ..model::Model::default()
        }
    }

    #[test]
    fn a_reply_is_applied_with_its_warning_and_a_refusal_changes_nothing() {
        let mut model = model();
        let reply = AgentAccess {
            name: "tester".into(),
            mode: Mode::Unsandboxed,
            warning: Some("Unsandboxed, 'tester' runs with no agent sandbox.".into()),
            ..AgentAccess::default()
        };
        assert_eq!(
            settle(&mut model, Ok(Applied::Access(reply))),
            Some(Notice {
                text: "Unsandboxed, 'tester' runs with no agent sandbox.".into(),
                refused: false,
            })
        );
        assert_eq!(model.agent("tester").unwrap().mode, Mode::Unsandboxed);
        let held = model.clone();
        let refused = Failure::Refused {
            code: "outside_authority".into(),
            message: "Only the owner may set how an agent is confined.".into(),
        };
        assert_eq!(
            settle(&mut model, Err(refused)),
            Some(Notice {
                text: "Only the owner may set how an agent is confined.".into(),
                refused: true,
            })
        );
        assert_eq!(model, held);
        // A reply without a warning leaves no notice.
        let placed = Placed {
            name: "tester".into(),
            role: Role::Coordinator,
            superior: None,
        };
        assert_eq!(settle(&mut model, Ok(Applied::Placed(placed))), None);
        assert_eq!(model.agent("tester").unwrap().role, Some(Role::Coordinator));
    }

    #[test]
    fn a_refused_change_shows_backbones_reason() {
        let refused = Failure::Refused {
            code: "outside_authority".into(),
            message: "Only the owner may set how an agent is confined.".into(),
        };
        assert_eq!(
            refusal(refused),
            "Only the owner may set how an agent is confined."
        );
        assert_eq!(
            refusal(Failure::Connection(Error::Refused(500))),
            "Agent Backbone did not make the change (HTTP 500)."
        );
        assert!(
            refusal(Failure::Connection(Error::Unavailable)).contains("may not have been made")
        );
    }
}
