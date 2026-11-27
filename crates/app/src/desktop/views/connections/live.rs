//! Owner-scoped data for the native Connections pane. Stream frames only invalidate reads.
use super::card::{self, LOCAL_AGENT};
use super::rules;
use super::*;
use crate::desktop::peer_inbox::{Inbox, Selection, Summary, failure};
use crate::desktop::views::clock::Local;
use agent_interface_roster::profiles::{AddError, Connection, Error, peers as api};
use api::{Direction, EventKind, Policy};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
pub(super) enum Change {
    Invite(String),
    Answer(String, bool),
    Gates(String, Vec<(Direction, Policy, String)>),
    Review(Box<api::Message>, DecisionKind),
    Disconnect(String),
}

#[derive(Clone)]
pub(super) enum DecisionKind {
    Pass,
    Decline,
    Revise(String),
}

impl Change {
    fn belongs_to(&self, selected: Option<&Selection>) -> bool {
        match (self, selected) {
            (Self::Invite(_), _) => true,
            (Self::Answer(request, _), Some(Selection::Request(current))) => request == current,
            (Self::Disconnect(g) | Self::Gates(g, _), Some(Selection::Peer(current))) => {
                g == &current.generation
            }
            (Self::Review(m, _), Some(Selection::Peer(current))) => &m.connection == current,
            _ => false,
        }
    }
    fn target(&self, overview: &api::Overview) -> Option<Selection> {
        match self {
            Self::Invite(_) => None,
            Self::Answer(request, _) => overview
                .invitations
                .iter()
                .any(|r| &r.request_id == request && r.state != "accepted")
                .then(|| Selection::Request(request.clone())),
            Self::Disconnect(g) | Self::Gates(g, _) => overview
                .connections
                .iter()
                .find(|p| &p.connection.generation == g)
                .map(|p| Selection::Peer(p.connection.clone())),
            Self::Review(m, _) => overview
                .connections
                .iter()
                .any(|p| p.connection == m.connection)
                .then(|| Selection::Peer(m.connection.clone())),
        }
    }

    fn still_pending(&self, snapshot: &Snapshot) -> bool {
        let overview = &snapshot.summary.overview;
        match self {
            Self::Invite(_) => true,
            Self::Answer(request, _) => overview.invitations.iter().any(|r| {
                &r.request_id == request
                    && r.direction == Direction::Incoming
                    && r.state == "pending"
            }),
            Self::Disconnect(g) => overview
                .connections
                .iter()
                .any(|p| &p.connection.generation == g && p.state == "connected"),
            Self::Gates(g, changes) => overview
                .connections
                .iter()
                .find(|p| &p.connection.generation == g && p.state == "connected")
                .is_some_and(|p| {
                    changes.iter().any(|(d, policy, _)| match d {
                        Direction::Incoming => p.gates.incoming != *policy,
                        Direction::Outgoing => p.gates.outgoing != *policy,
                    })
                }),
            Self::Review(m, _) => {
                let connected = overview
                    .connections
                    .iter()
                    .any(|p| p.connection == m.connection && p.state == "connected");
                if !connected || snapshot.summary.count(&m.connection.generation) == 0 {
                    return false;
                }
                if snapshot.selection.as_ref() == Some(&Selection::Peer(m.connection.clone()))
                    && let Some(history) = &snapshot.history
                    && let Some(item) = history
                        .items
                        .iter()
                        .find(|item| item.message_id == m.message_id)
                {
                    return item.state == "pending_review";
                }
                true
            }
        }
    }

    /// What this change was, by the connection's name, for a notice.
    fn description(&self, overview: Option<&api::Overview>) -> String {
        let connections = overview.map_or(&[][..], |o| o.connections.as_slice());
        let by_generation = |g: &str| {
            connections
                .iter()
                .find(|p| p.connection.generation == g)
                .map_or_else(
                    || "this connection".to_owned(),
                    |p| known_as(p.display_name.as_deref(), &p.connection.peer_id),
                )
        };
        match self {
            Self::Invite(id) => format!("Connection request to {}", known_as(None, id)),
            Self::Answer(request, _) => {
                let name = overview
                    .and_then(|o| o.invitations.iter().find(|r| &r.request_id == request))
                    .map_or_else(
                        || "this profile".to_owned(),
                        |r| known_as(r.display_name.as_deref(), &r.peer_id),
                    );
                format!("Answer to {name}")
            }
            Self::Disconnect(g) => format!("Disconnect from {}", by_generation(g)),
            Self::Gates(g, _) => format!("Gate change for {}", by_generation(g)),
            Self::Review(m, _) => {
                let name = connections
                    .iter()
                    .find(|p| p.connection == m.connection)
                    .map_or_else(
                        || known_as(None, &m.connection.peer_id),
                        |p| known_as(p.display_name.as_deref(), &p.connection.peer_id),
                    );
                format!(
                    "Decision on the {} message {} {name}",
                    rules::clock(&m.submitted_at),
                    match m.direction {
                        Direction::Incoming => "from",
                        Direction::Outgoing => "to",
                    }
                )
            }
        }
    }

    fn send(&self, connection: &Connection, key: &str) -> Result<(), AddError> {
        match self {
            Self::Invite(id) => connection.invite_peer(id, key).map(drop),
            Self::Answer(id, accept) => connection.answer_peer(id, *accept, key).map(drop),
            Self::Gates(generation, changes) => {
                for (direction, policy, key) in changes {
                    connection.set_peer_gate(generation, *direction, *policy, key)?;
                }
                Ok(())
            }
            Self::Disconnect(generation) => {
                Ok(connection.disconnect_peer(generation, key).map(drop)?)
            }
            Self::Review(message, decision) => {
                let action = match decision {
                    DecisionKind::Pass => api::Decision::Pass,
                    DecisionKind::Decline => api::Decision::Decline,
                    DecisionKind::Revise(note) => api::Decision::Revise(note),
                };
                Ok(connection
                    .review_peer_message(message, action, key)
                    .map(drop)?)
            }
        }
    }
}

pub(super) enum Form {
    Connect,
    Decline(api::Message),
    Revise(api::Message),
    Disconnect(String),
}

struct Snapshot {
    summary: Arc<Summary>,
    selection: Option<Selection>,
    history: Option<api::History>,
}

impl Snapshot {
    fn read(
        c: &Connection,
        summary: Arc<Summary>,
        selected: Option<Selection>,
        oldest: Option<String>,
    ) -> Result<Self, Error> {
        let selection = resolve(&summary.overview, selected);
        let history = match &selection {
            Some(Selection::Peer(id)) => {
                let mut page = c.peer_history(id, None)?;
                if let Some(anchor) = oldest {
                    let mut seen = HashSet::new();
                    while !page.items.iter().any(|m| m.message_id == anchor) {
                        let Some(cursor) = page.older.take() else {
                            break;
                        };
                        if !seen.insert(cursor.clone()) {
                            return Err(Error::Invalid);
                        }
                        let mut prior = c.peer_history(id, Some(&cursor))?;
                        prepend(&mut page, &mut prior);
                        page = prior;
                    }
                }
                Some(page)
            }
            _ => None,
        };
        Ok(Self {
            summary,
            selection,
            history,
        })
    }
}

/// Put `newer`'s messages and events after `older`'s, keeping one of each.
fn prepend(newer: &mut api::History, older: &mut api::History) {
    let messages: HashSet<_> = newer.items.iter().map(|m| m.message_id.as_str()).collect();
    older
        .items
        .retain(|m| !messages.contains(m.message_id.as_str()));
    older.items.append(&mut newer.items);
    let events: HashSet<_> = newer.events.iter().map(|e| e.id).collect();
    older.events.retain(|e| !events.contains(&e.id));
    older.events.append(&mut newer.events);
}

/// A connection's name, or "Unnamed profile" with its short fingerprint.
fn known_as(name: Option<&str>, peer_id: &str) -> String {
    match rules::display_name(name) {
        "Unnamed profile" => format!("Unnamed profile {}", rules::fingerprint(peer_id)),
        name => name.to_owned(),
    }
}

/// What `selected` stands for in `overview` now: an accepted request is its
/// connection, and anything no longer listed is nothing.
fn resolve(overview: &api::Overview, selected: Option<Selection>) -> Option<Selection> {
    match selected {
        Some(Selection::Request(ref id)) => overview
            .invitations
            .iter()
            .find(|r| &r.request_id == id)
            .and_then(|r| {
                if r.state == "accepted" {
                    overview
                        .connections
                        .iter()
                        .find(|p| {
                            Some(&p.connection.generation) == r.generation.as_ref()
                                && p.connection.peer_id == r.peer_id
                        })
                        .map(|p| Selection::Peer(p.connection.clone()))
                } else {
                    Some(Selection::Request(id.clone()))
                }
            }),
        Some(Selection::Peer(ref id))
            if overview.connections.iter().any(|p| &p.connection == id) =>
        {
            selected
        }
        _ => None,
    }
}

/// One gate choice: a radio button whose accessible name carries its
/// direction, also when only an icon labels the gate.
fn gate_segment(
    direction: Direction,
    label: &str,
    choice: Policy,
    on: bool,
) -> gpui::Stateful<Div> {
    ui::segment(
        format!("gate-{direction:?}-{choice:?}"),
        format!("{choice:?}"),
        on,
    )
    .role(gpui::accesskit::Role::RadioButton)
    .aria_label(format!("{label}: {choice:?}"))
    .aria_toggled(if on {
        gpui::accesskit::Toggled::True
    } else {
        gpui::accesskit::Toggled::False
    })
    .focusable()
    .tab_stop(true)
    .focus_visible(|el| {
        el.border(px(tokens().size.focus_ring))
            .border_color(colors().focus)
    })
}

/// A gate's two segments: each choice, whether it is the service's current
/// policy, and the change choosing it sends. The segments show only what
/// the service reports, so a refused or failed change leaves them where they
/// were and its reason shows below the thread.
fn gate_choices(
    generation: &str,
    direction: Direction,
    current: Policy,
) -> [(Policy, bool, Option<Change>); 2] {
    [Policy::Review, Policy::Auto].map(|choice| {
        let change = (current != choice).then(|| {
            Change::Gates(
                generation.to_owned(),
                vec![(direction, choice, request_key())],
            )
        });
        (choice, current == choice, change)
    })
}

/// Whether a lost decision is offered again on its message's card: the
/// message is a waiting one in the thread shown.
fn on_card(action: &Change, shown: &Snapshot, selected: Option<&Selection>) -> bool {
    let Change::Review(m, _) = action else {
        return false;
    };
    action.belongs_to(selected)
        && shown.selection.as_ref() == selected
        && shown.history.as_ref().is_some_and(|history| {
            history
                .items
                .iter()
                .any(|item| item.message_id == m.message_id && item.state == "pending_review")
        })
}

struct PendingRetry {
    action: Change,
    key: String,
    error: Option<String>,
}

pub(super) struct Live {
    connection: Connection,
    pub(super) selected: Option<Selection>,
    snapshot: Option<Snapshot>,
    stream_ok: bool,
    reading: bool,
    dirty: bool,
    sending: bool,
    retry: Option<PendingRetry>,
    status: Option<String>,
    /// A refused action: shown until dismissed, without pausing the pane.
    notice: Option<String>,
    read_at: Option<String>,
    older_loading: bool,
    oldest: Option<String>,
    pub(super) form: Option<Form>,
    inbox: Entity<Inbox>,
    _events: Subscription,
}

impl Live {
    pub(super) fn new(connection: Connection, cx: &mut Context<Pane>) -> Self {
        let inbox = Inbox::shared(&connection, cx);
        let subscription = cx.observe(&inbox, |pane, _, cx| pane.inbox_updated(cx));
        let pane = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = pane.update(cx, |p, cx| p.inbox_updated(cx));
        });
        Self {
            connection,
            selected: None,
            snapshot: None,
            stream_ok: false,
            reading: false,
            dirty: false,
            sending: false,
            retry: None,
            status: None,
            notice: None,
            read_at: None,
            older_loading: false,
            oldest: None,
            form: None,
            inbox,
            _events: subscription,
        }
    }
}

impl Pane {
    fn inbox_updated(&mut self, cx: &mut Context<Self>) {
        let inbox = self.live.inbox.read(cx);
        self.live.stream_ok = inbox.error.is_none() && inbox.summary.is_some();
        if self.live.stream_ok {
            self.refresh_live(cx);
        } else {
            self.live.status = inbox.error.clone();
            cx.notify();
        }
    }

    /// When the shown records were read from Agent Backbone, for the view's
    /// header; nothing in the preview or before the first read.
    pub(super) fn read_at(&self) -> Option<String> {
        let at = self.live.read_at.as_ref()?;
        (self.preview.is_none() && self.live.stream_ok && self.live.snapshot.is_some())
            .then(|| format!("live from Agent Backbone · read {at}"))
    }

    pub(in crate::desktop) fn pick_peer(&mut self, target: Selection, cx: &mut Context<Self>) {
        if self.preview.is_some() {
            self.preview(false, cx);
        }
        self.pick(Some(target), cx);
    }

    pub(in crate::desktop) fn needs_review(&self, cx: &gpui::App) -> bool {
        self.live
            .inbox
            .read(cx)
            .summary
            .as_ref()
            .is_some_and(|s| s.attention())
    }

    fn refresh_live(&mut self, cx: &mut Context<Self>) {
        // The preview's selection is not the service's to resolve.
        if !self.live.stream_ok || self.preview.is_some() {
            return;
        }
        if self.live.reading {
            self.live.dirty = true;
            return;
        }
        let Some(summary) = self.live.inbox.read(cx).summary.clone() else {
            return;
        };
        let connection = self.live.connection.clone();
        let selected = self.live.selected.clone();
        self.live.reading = true;
        let expected = selected.clone();
        let oldest = self.live.oldest.clone();
        let read = cx
            .background_executor()
            .spawn(async move { Snapshot::read(&connection, summary, selected, oldest) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                let live = &mut pane.live;
                live.reading = false;
                if live.selected == expected && live.stream_ok {
                    match result {
                        Ok(snapshot) => {
                            let connected = |id: &str| {
                                snapshot.summary.overview.connections.iter().any(|p| {
                                    p.connection.generation == id && p.state == "connected"
                                })
                            };
                            if matches!(&live.form, Some(Form::Disconnect(id)) if !connected(id))
                                || matches!(&live.form, Some(Form::Decline(m) | Form::Revise(m)) if !connected(&m.connection.generation))
                            {
                                live.form = None;
                            }
                            // The Connect panel belongs to no selection.
                            if live.selected != snapshot.selection
                                && !matches!(live.form, Some(Form::Connect))
                            {
                                live.form = None;
                            }
                            if live
                                .retry
                                .as_ref()
                                .is_some_and(|pending| !pending.action.still_pending(&snapshot))
                            {
                                live.retry = None;
                            }
                            live.selected = snapshot.selection.clone();
                            live.snapshot = Some(snapshot);
                            live.read_at = Some(chrono::Local::now().format("%H:%M").to_string());
                            live.status = None;
                        }
                        Err(e) => live.status = Some(failure(e)),
                    }
                }
                if std::mem::take(&mut live.dirty) {
                    pane.refresh_live(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn pick(&mut self, target: Option<Selection>, cx: &mut Context<Self>) {
        self.live.oldest = None;
        self.live.selected = target;
        self.live.form = None;
        self.copied = None;
        self.menu = Menu::None;
        self.scroll_on_open.set(true);
        self.scroll_waits.set(0);
        self.refresh_live(cx);
        cx.notify();
    }

    pub(super) fn change(&mut self, action: Change, cx: &mut Context<Self>) {
        if self.preview.is_some() {
            self.change_preview(action, cx);
            return;
        }
        if !action.belongs_to(self.live.selected.as_ref())
            || self.live.sending
            || self.live.retry.is_some()
            || self.live.status.is_some()
            || !self.live.stream_ok
        {
            return;
        }
        self.live.notice = None;
        self.live.retry = Some(PendingRetry {
            action,
            key: request_key(),
            error: None,
        });
        self.send_live(cx);
    }

    /// Apply `action` to the preview's records, as the service would.
    fn change_preview(&mut self, action: Change, cx: &mut Context<Self>) {
        let Some(sample) = &mut self.preview else {
            return;
        };
        if !action.belongs_to(self.live.selected.as_ref()) {
            return;
        }
        match action {
            Change::Invite(id) => sample.invite(&id),
            Change::Answer(request, accept) => sample.answer(&request, accept),
            Change::Gates(generation, changes) => {
                for (direction, policy, _) in changes {
                    sample.set_gate(&generation, direction, policy);
                }
            }
            Change::Review(m, decision) => sample.review(
                &m.message_id,
                match decision {
                    DecisionKind::Pass => model::Decision::Pass,
                    DecisionKind::Decline => model::Decision::Decline,
                    DecisionKind::Revise(note) => model::Decision::Revise(note),
                },
            ),
            Change::Disconnect(generation) => sample.disconnect(&generation),
        }
        self.live.form = None;
        self.live.selected = resolve(&sample.overview, self.live.selected.take());
        cx.notify();
    }

    /// Give up on a change whose reply was lost, freeing the pane's actions.
    pub(super) fn dismiss_retry(&mut self, cx: &mut Context<Self>) {
        self.live.retry = None;
        cx.notify();
    }

    /// A decision on `message` whose reply was lost: why, and whether it can
    /// be sent again now.
    pub(super) fn lost_decision(&self, message: &str) -> Option<(String, bool)> {
        let pending = self.live.retry.as_ref()?;
        let Change::Review(m, _) = &pending.action else {
            return None;
        };
        let error = pending.error.clone()?;
        (m.message_id == message).then_some((error, self.live.stream_ok && !self.live.sending))
    }

    pub(super) fn send_live(&mut self, cx: &mut Context<Self>) {
        if self.live.sending || !self.live.stream_ok {
            return;
        }
        let Some((change, key)) = self
            .live
            .retry
            .as_ref()
            .map(|pending| (pending.action.clone(), pending.key.clone()))
        else {
            return;
        };
        if self.preview.is_some() || !change.belongs_to(self.live.selected.as_ref()) {
            return;
        }
        let description = change.description(
            self.live
                .snapshot
                .as_ref()
                .map(|snapshot| &snapshot.summary.overview),
        );
        let invite = matches!(change, Change::Invite(_));
        self.live.sending = true;
        self.live.form = None;
        let service = self.live.connection.clone();
        let operation = cx
            .background_executor()
            .spawn(async move { change.send(&service, &key) });
        cx.spawn(async move |this, cx| {
            let result = operation.await;
            let _ = this.update(cx, |pane, cx| {
                pane.live.sending = false;
                match result {
                    Ok(()) => {
                        pane.live.retry = None;
                        pane.live.status = None;
                        pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx));
                    }
                    // The service said why it refused; nothing was changed.
                    Err(AddError::Refused(reason)) => {
                        pane.live.retry = None;
                        pane.live.notice = Some(format!("{description}: {reason}"));
                        pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx));
                    }
                    Err(AddError::Connection(error)) => {
                        // A lost reply may already have committed. Retry the same bytes and key.
                        if matches!(
                            error,
                            Error::Invalid | Error::Unavailable | Error::Refused(500..=599)
                        ) {
                            if let Some(pending) = &mut pane.live.retry {
                                pending.error = Some(format!("{description}: {}", failure(error)));
                            }
                            pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx));
                        } else {
                            pane.live.retry = None;
                            pane.live.notice = Some(match (invite, error) {
                                // The service has no invitation route yet.
                                (true, Error::Refused(404)) => {
                                    "This Agent Backbone can\u{2019}t send connection requests yet. Nothing was sent.".into()
                                }
                                (_, error) => format!("{description}: {}", failure(error)),
                            });
                            pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Once the connect popover has closed, put focus back on ＋, which
    /// opened it, so the keyboard does not lose its place.
    pub(super) fn connecting(&self) -> bool {
        matches!(self.live.form, Some(Form::Connect))
    }

    pub(super) fn focus_after_connect(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.live.form, Some(Form::Connect)) {
            self.keep_focus(&self.entry, window, cx);
        }
    }

    /// This profile's connection ID, from the records shown.
    fn local_id(&self) -> Option<String> {
        match &self.preview {
            Some(sample) => Some(sample.overview.local_id.clone()),
            None => Some(
                self.live
                    .snapshot
                    .as_ref()?
                    .summary
                    .overview
                    .local_id
                    .clone(),
            ),
        }
    }

    /// Send a decision whose control goes away once it is made, first moving
    /// keyboard focus somewhere that stays: the thread for a message, the
    /// pane's ＋ or chooser for a request or a disconnect.
    pub(super) fn decide(&mut self, action: Change, window: &mut Window, cx: &mut Context<Self>) {
        let stays = match &action {
            Change::Review(..) => self.thread_focus.clone(),
            _ => self.entry.clone(),
        };
        self.keep_focus(&stays, window, cx);
        self.change(action, cx);
    }

    pub(super) fn submit_live_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().trim().to_owned();
        if value.is_empty() {
            return;
        }
        match &self.live.form {
            // Only an ID Agent Backbone would take is sent.
            Some(Form::Connect) => {
                if let Some(local) = self.local_id()
                    && let rules::IdCheck::Valid { id, .. } = rules::check_id(&value, &local)
                {
                    self.change(Change::Invite(id), cx)
                }
            }
            Some(Form::Revise(m)) => {
                let m = m.clone();
                self.decide(
                    Change::Review(Box::new(m), DecisionKind::Revise(value)),
                    window,
                    cx,
                )
            }
            _ => {}
        }
    }
}

impl Pane {
    pub(super) fn live_button(
        &self,
        id: impl Into<String>,
        label: impl Into<String>,
        style: ui::Button,
        cx: &Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        ui::button(id.into(), label.into(), style)
            .focus_visible(|el| {
                el.border(px(tokens().size.focus_ring))
                    .border_color(colors().focus)
            })
            .focusable()
            .tab_stop(true)
            .on_click(cx.listener(move |pane, _, window, cx| action(pane, window, cx)))
    }

    pub(super) fn show_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = Menu::None;
        // A confirmation takes focus on its Cancel; Escape there cancels.
        match &form {
            Form::Decline(m) => self.decline_opener = Some(m.message_id.clone()),
            Form::Revise(m) => self.revise_opener = Some(m.message_id.clone()),
            _ => {}
        }
        if matches!(&form, Form::Disconnect(_) | Form::Decline(_)) {
            window.focus(&self.confirm_focus, cx);
        }
        if matches!(&form, Form::Connect | Form::Revise(_)) {
            self.input.update(cx, |field, cx| {
                field.clear(cx);
                window.focus(&field.focus, cx);
            });
        }
        self.live.form = Some(form);
        cx.notify();
    }

    /// A confirmation, gate choice or note, drawn where it applies.
    fn live_form(&self, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let Some(form) = &self.live.form else {
            return div();
        };
        let cancel = self
            .live_button(
                "live-cancel",
                "Cancel",
                ui::Button::Secondary,
                cx,
                |pane, window, cx| pane.cancel_form(window, cx),
            )
            .track_focus(&self.confirm_focus);
        let strip = |text: String| {
            div()
                .on_key_down(cx.listener(|pane, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        pane.cancel_form(window, cx);
                    }
                }))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(t.space.s8))
                .child(
                    ui::text(
                        div()
                            .flex_grow(1.0)
                            .flex_basis(px(t.size.empty_width / 2.0))
                            .min_w_0()
                            .text_color(c.text_2),
                        &t.text.body,
                    )
                    .child(text),
                )
        };
        let (label, prompt, action) = match form {
            // These are drawn in the connect panel and in their message's card.
            Form::Connect | Form::Revise(_) | Form::Decline(_) => return div(),
            Form::Disconnect(id) => (
                "Disconnect",
                "Disconnect? Delivery stops here at once; the other profile confirms when it next connects.".to_owned(),
                Change::Disconnect(id.clone()),
            ),
        };
        strip(prompt)
            .child(self.live_button(
                "live-confirm",
                label,
                ui::Button::Danger,
                cx,
                move |pane, window, cx| pane.decide(action.clone(), window, cx),
            ))
            .child(cancel)
    }

    /// ＋'s popover: paste another profile's ID, checked as it is typed, or
    /// copy this one's to send them.
    fn connect_panel(
        &self,
        local: &str,
        actions: bool,
        anchored: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let scroll = self.connect_scroll.clone();
        let meta =
            |text: &'static str| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let close = |pane: &mut Self, cx: &mut Context<Self>| {
            pane.live.form = None;
            cx.notify();
        };
        let check = rules::check_id(self.input.read(cx).value(), local);
        let status = match &check {
            rules::IdCheck::Empty => None,
            rules::IdCheck::Valid { ending, .. } => Some(
                ui::text(
                    div().flex().items_center().gap(px(t.space.s4)),
                    &t.text.meta,
                )
                .text_color(c.muted)
                .child(ui::icon("check", t.size.icon - 2.0, c.ok))
                .child("Valid ID, ending")
                .child(
                    ui::text(div().font_family(t.font.mono.clone()), &t.text.key)
                        .text_color(c.text_2)
                        .child(ending.clone()),
                ),
            ),
            rules::IdCheck::Invalid(reason) => {
                Some(ui::text(div().text_color(c.attention_text), &t.text.meta).child(*reason))
            }
        };
        let valid = matches!(check, rules::IdCheck::Valid { .. });
        let send = if valid && actions {
            let send = self.live_button(
                "live-submit",
                "Send request",
                ui::Button::Primary,
                cx,
                |pane, window, cx| {
                    pane.submit_live_form(window, cx);
                    pane.focus_after_connect(window, cx);
                },
            );
            self.in_view(send, None, &scroll, window, cx)
        } else {
            ui::button("live-submit", "Send request", ui::Button::Primary).opacity(0.35)
        };
        // Under ＋ in the list or the strip; elsewhere, centred near the top.
        let (left, top) = match (self.width, self.list_minimised()) {
            _ if !anchored => (0.0, 0.0),
            (peers::Width::Narrow, _) => (t.space.s8, t.space.s8),
            (_, true) => (
                t.size.peer_strip + t.space.s4,
                t.space.s8 * 2.0 + t.size.icon_button * 2.0,
            ),
            (_, false) => (
                t.size.peer_list - t.size.icon_button * 2.0 + t.space.s6,
                t.space.s8 + t.size.row + t.space.s4,
            ),
        };
        let copied = self.copied.as_deref() == Some("copy-local-id-popover");
        let mine = local.to_owned();
        let cancel = self.live_button(
            "live-cancel",
            "Cancel",
            ui::Button::Secondary,
            cx,
            move |pane, window, cx| {
                close(pane, cx);
                pane.focus_after_connect(window, cx);
            },
        );
        let cancel = self.in_view(cancel, None, &scroll, window, cx);
        let copy =
            peers::small_button(
                "copy-local-id-popover",
                if copied { "Copied" } else { "Copy ID" },
            )
            .on_click(cx.listener(move |p, _, _, cx| {
                p.copy("copy-local-id-popover".into(), mine.clone(), cx)
            }));
        let copy = self.in_view(copy, None, &scroll, window, cx);
        let field = self.input.read(cx).focus.clone();
        ui::menu()
            .id("connect-panel")
            .track_focus(&self.connect_focus)
            .track_scroll(&scroll)
            .when(anchored, |el| el.absolute().top(px(top)).left(px(left)))
            .w(px(t.size.connect_popover))
            .max_w(gpui::relative(0.92))
            .max_h(gpui::relative(0.94))
            .overflow_y_scroll()
            .p(px(t.space.s14))
            .gap(px(t.space.s12))
            .on_key_down(
                cx.listener(move |pane, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        close(pane, cx);
                        pane.focus_after_connect(window, cx);
                    }
                }),
            )
            .on_mouse_down_out(cx.listener(move |pane, _, window, cx| {
                // Focus anywhere in the closing popover goes back to ＋.
                if pane.connect_focus.contains_focused(window, cx) {
                    pane.keep_focus(&pane.entry.clone(), window, cx);
                }
                pane.dismissed = Some((Menu::Connect, std::time::Instant::now()));
                close(pane, cx)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        ui::text(div().flex_1().text_color(c.text), &t.text.workspace)
                            .child("Connect a profile"),
                    )
                    .child(ui::keycap("esc")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .child(meta("Their connection ID"))
                    .child(
                        field_box(hinted(&self.input, "v1:…", cx))
                            .font_family(t.font.mono.clone())
                            .border_color(c.accent)
                            .relative()
                            .child(self.reveal.within(&field, &scroll)),
                    )
                    .children(status),
            )
            .child(meta(
                "They have to accept before any message flows. Both directions start on Review.",
            ))
            .when(!actions, |el| {
                el.child(meta("Sending is paused until Agent Backbone answers."))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(send)
                    .child(cancel),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .pt(px(t.space.s12))
                    .border_t(px(t.size.hairline))
                    .border_color(c.line_strong)
                    .whitespace_nowrap()
                    .child(
                        ui::text(
                            div().flex_1().min_w_0().overflow_hidden().text_ellipsis(),
                            &t.text.meta,
                        )
                        .text_color(c.muted)
                        .child("Or send them yours: ")
                        .child(
                            ui::text(div().font_family(t.font.mono.clone()), &t.text.key)
                                .text_color(c.text_2)
                                .child(rules::fingerprint(local)),
                        )
                        .flex()
                        .items_center(),
                    )
                    .child(copy),
            )
    }

    fn copy_button(
        &self,
        id: &'static str,
        label: &'static str,
        local: String,
        entry: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let copied = self.copied.as_deref() == Some(id);
        let button = self.live_button(
            id,
            if copied { "Copied" } else { label },
            ui::Button::Secondary,
            cx,
            move |p, _, cx| p.copy(id.into(), local.clone(), cx),
        );
        if entry {
            button.track_focus(&self.entry)
        } else {
            button
        }
    }

    fn live_older(&mut self, cx: &mut Context<Self>) {
        let Some(Selection::Peer(id)) = self.live.selected.clone() else {
            return;
        };
        let Some(snapshot) = &self.live.snapshot else {
            return;
        };
        if snapshot.selection != self.live.selected || self.live.older_loading {
            return;
        }
        let Some(before) = snapshot.history.as_ref().and_then(|h| h.older.clone()) else {
            return;
        };
        self.live.older_loading = true;
        let c = self.live.connection.clone();
        let req = id.clone();
        let cursor = before.clone();
        let read = cx
            .background_executor()
            .spawn(async move { c.peer_history(&req, Some(&cursor)) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.live.older_loading = false;
                if pane.live.selected == Some(Selection::Peer(id))
                    && let Some(history) =
                        pane.live.snapshot.as_mut().and_then(|s| s.history.as_mut())
                    && history.older.as_ref() == Some(&before)
                {
                    match result {
                        Ok(mut page) if page.older.as_ref() != Some(&before) => {
                            prepend(history, &mut page);
                            pane.live.oldest = page.items.first().map(|m| m.message_id.clone());
                            if pane.live.reading {
                                pane.live.dirty = true;
                            }
                            *history = page;
                        }
                        _ => pane.live.status = Some("Could not read older history".into()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Under the narrow width, a thread with no peer still offers the
    /// chooser.
    fn chooser_bar(&self, s: &Summary, cx: &Context<Self>) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        (self.width == peers::Width::Narrow).then(|| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .h(px(t.size.peer_header))
                .px(px(t.space.s14))
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .child(self.peer_chooser(s, None, "Choose a peer", None, cx))
        })
    }

    /// The thread's 48 px top line: a way back on a narrow tile, the peer's
    /// mark, name and state, its short fingerprint with a copy button, then
    /// any controls at the right.
    fn live_header(
        &self,
        s: &Summary,
        name: Option<&str>,
        peer_id: &str,
        state: Option<Div>,
        controls: Option<Div>,
        cx: &Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let copied = self.copied.as_deref() == Some("copy-peer-id");
        let copy = ui::interaction_icon_button(
            "copy-peer-id",
            if copied { "check" } else { "copy" },
            "Copy its connection ID",
        )
        .tab_stop(true);
        let id = peer_id.to_owned();
        let fit = self.header_fit();
        // What names the connection gives way to the controls, which are
        // never pushed out of the header.
        let named = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(t.space.s10))
            .whitespace_nowrap()
            .map(|el| {
                if self.width == peers::Width::Narrow {
                    el.child(self.peer_chooser(
                        s,
                        name,
                        rules::display_name(name),
                        Some(peer_id),
                        cx,
                    ))
                } else {
                    el.child(peer_mark(name, t.size.view_tile - t.space.s4))
                        .child(
                            ui::text(
                                // The name keeps its width up to a limit; with
                                // less room it ends in an ellipsis before the
                                // state tag beside it is cut.
                                div()
                                    .min_w_0()
                                    .max_w(px(t.size.peer_list - t.size.view_tile * 2.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(c.text),
                                &t.text.workspace,
                            )
                            .child(rules::display_name(name).to_owned()),
                        )
                }
            })
            .children(state.map(|tag| tag.flex_shrink_0()))
            .when(fit.fingerprint, |el| {
                el.child(
                    ui::text(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(t.font.mono.clone()),
                        &t.text.key,
                    )
                    .text_color(c.muted)
                    .child(rules::fingerprint(peer_id)),
                )
                .child(copy.flex_shrink_0().on_click(
                    cx.listener(move |p, _, _, cx| p.copy("copy-peer-id".into(), id.clone(), cx)),
                ))
            });
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.peer_header))
            .pl(px(t.space.s20))
            .pr(px(t.space.s14))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .whitespace_nowrap()
            .child(named)
            .children(controls.map(|controls| controls.flex_shrink_0()))
    }

    /// How the thread header fits its width now.
    pub(super) fn header_fit(&self) -> peers::HeaderFit {
        let list = match (self.width, self.list_minimised()) {
            (peers::Width::Narrow, _) => 0.0,
            (_, true) => tokens().size.peer_strip,
            (_, false) => tokens().size.peer_list,
        };
        peers::header_fit(self.width, self.pane_width - list)
    }

    /// Each gate's label and its Review and Auto segments, in a row or,
    /// in the folded gates' menu, `stacked`. Without `words` a label is its
    /// direction icon, with the word as its tooltip and accessible name.
    fn gate_rows(
        &self,
        peer: &api::Peer,
        actions: bool,
        stacked: bool,
        words: bool,
        cx: &Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let mut controls = div().flex().items_center().gap(px(t.space.s6));
        for (direction, icon, label, current) in [
            (
                Direction::Incoming,
                "arrow-down-left",
                "Incoming",
                peer.gates.incoming,
            ),
            (
                Direction::Outgoing,
                "arrow-up-right",
                "Outgoing",
                peer.gates.outgoing,
            ),
        ] {
            controls = controls.child(
                ui::text(
                    div()
                        .id(format!("gate-label-{direction:?}"))
                        .flex()
                        .items_center()
                        .gap(px(t.space.s6))
                        .when(direction == Direction::Outgoing && !stacked, |el| {
                            el.ml(px(t.space.s6))
                        })
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child(ui::icon(icon, t.size.icon - 2.0, c.muted))
                .map(|el| {
                    if words {
                        el.child(label)
                    } else {
                        el.tooltip(move |_, cx| cx.new(|_| Tip(label.into())).into())
                    }
                }),
            );
            if current == Policy::Unknown {
                controls = controls.child(ui::tag("unknown to this version", ui::Tone::Quiet));
                continue;
            }
            let mut segments = ui::segments();
            for (choice, on, change) in
                gate_choices(&peer.connection.generation, direction, current)
            {
                let mut segment = gate_segment(direction, label, choice, on);
                if choice == Policy::Auto {
                    segment = segment.tooltip(|_, cx| cx.new(|_| Tip(AUTO_NOTE.into())).into());
                }
                if let Some(change) = change.filter(|_| actions) {
                    segment = segment
                        .on_click(cx.listener(move |p, _, _, cx| p.change(change.clone(), cx)));
                }
                segments = segments.child(segment);
            }
            controls = controls.child(
                segments
                    .border(px(t.size.hairline))
                    .border_color(c.line_strong),
            );
        }
        controls
    }

    /// Both gates as Review and Auto, each applied as it is chosen, then the
    /// ⋯ menu with Disconnect.
    fn peer_controls(&self, peer: &api::Peer, actions: bool, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut controls = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6));
        let gates = self.header_fit().gates;
        if gates == peers::Gates::Folded {
            // Without room for both rows, the gates fold into one button
            // that opens them.
            let letter = |policy: Policy| match policy {
                Policy::Review => "Review",
                Policy::Auto => "Auto",
                Policy::Unknown => "?",
            };
            let mut gates = div().relative().child(
                ui::text(
                    div()
                        .id("folded-gates")
                        .focus_visible(|el| {
                            el.border(px(tokens().size.focus_ring))
                                .border_color(colors().focus)
                        })
                        .flex()
                        .items_center()
                        .gap(px(t.space.s6))
                        .h(px(t.size.icon_button + t.space.s4))
                        .px(px(t.space.s8))
                        .rounded(px(t.radius.row))
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                        .bg(c.field)
                        .text_color(c.text_2)
                        .cursor_pointer()
                        .hover(|el| el.bg(colors().hover)),
                    &t.text.meta,
                )
                .child(ui::icon("arrow-down-left", t.size.icon - 2.0, c.muted))
                .child(letter(peer.gates.incoming))
                .child(ui::icon("arrow-up-right", t.size.icon - 2.0, c.muted))
                .child(letter(peer.gates.outgoing))
                .child(ui::icon("chevron-down", t.size.icon - 2.0, c.muted))
                .track_focus(&self.gates_focus)
                .on_click(cx.listener(|p, _, w, cx| p.toggle_menu(Menu::Gates, w, cx))),
            );
            if self.menu == Menu::Gates {
                gates = gates.child(
                    gpui::deferred(
                        self.menu_frame(cx)
                            .absolute()
                            .top(px(t.size.icon_button + t.space.s4))
                            .right_0()
                            .gap(px(t.space.s8))
                            .child(
                                self.gate_rows(peer, actions, true, true, cx)
                                    .flex_col()
                                    .items_start(),
                            ),
                    )
                    .with_priority(1),
                );
            }
            controls = controls.child(gates);
        } else {
            controls = controls.child(self.gate_rows(
                peer,
                actions,
                false,
                gates == peers::Gates::Words,
                cx,
            ));
        }
        let generation = peer.connection.generation.clone();
        let mut more = div().relative().ml(px(t.space.s4)).child(
            ui::interaction_icon_button("peer-more", "ellipsis", "More for this connection")
                .track_focus(&self.more_focus)
                .on_click(cx.listener(|p, _, w, cx| p.toggle_menu(Menu::More, w, cx))),
        );
        // The menu stays drawn while a read or change is in flight, with
        // Disconnect waiting for it, so it never opens later by itself.
        if self.menu == Menu::More {
            more = more.child(
                gpui::deferred(
                    self.menu_frame(cx)
                        .absolute()
                        .top(px(t.size.icon_button + t.space.s4))
                        .right_0()
                        .child(
                            ui::menu_item(
                                "peer-disconnect",
                                Some(ui::icon("link-2-off", t.size.icon, c.attention_text)),
                                "Disconnect",
                                None,
                            )
                            .focus_visible(|el| {
                                el.border(px(tokens().size.focus_ring))
                                    .border_color(colors().focus)
                            })
                            .focusable()
                            .tab_stop(true)
                            .text_color(c.attention_text)
                            .when(!actions, |el| el.opacity(0.35))
                            .on_click(cx.listener(
                                move |p, _, w, cx| {
                                    if !actions {
                                        return;
                                    }
                                    p.menu = Menu::None;
                                    p.show_form(Form::Disconnect(generation.clone()), w, cx)
                                },
                            )),
                        ),
                )
                .with_priority(1),
            );
        }
        controls.child(more)
    }

    fn live_thread(
        &self,
        snapshot: &Snapshot,
        actions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let column = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        let body = div()
            .id("peer-history")
            .track_scroll(&self.thread_scroll)
            .track_focus(&self.thread_focus)
            // A press in the thread lets the copy and select-all keys act
            // on its text.
            .capture_any_mouse_down(cx.listener(|pane, _, window, cx| {
                window.focus(&pane.thread_focus, cx);
            }))
            .on_action(cx.listener(|pane, _: &crate::desktop::Copy, _, cx| {
                pane.selection.borrow().copy(cx);
            }))
            .on_action(cx.listener(|pane, _: &crate::desktop::SelectAll, _, cx| {
                pane.selection.borrow_mut().select_all();
                cx.notify();
            }))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(if self.width == peers::Width::Narrow {
                t.space.s14
            } else {
                t.space.s20
            }))
            .py(px(t.space.s16))
            .flex()
            .flex_col()
            .gap(px(t.space.s12));
        let meta = |text: String| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        // Without a peer header, a narrow tile still needs its way back.
        let empty = |title: &'static str, body: &'static str| {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .children(self.chooser_bar(&snapshot.summary, cx))
                .child(div().flex_1().min_h_0().child(ui::empty_state(
                    ui::empty_tile("link-2", c.selected, c.muted),
                    title,
                    body,
                )))
        };
        if self.live.selected != snapshot.selection {
            return empty("Reading conversation…", "");
        }
        match &snapshot.selection {
            None => empty("No peer selected", "Choose a peer to see its conversation."),
            Some(Selection::Request(id)) => {
                let Some(r) = snapshot
                    .summary
                    .overview
                    .invitations
                    .iter()
                    .find(|r| &r.request_id == id)
                else {
                    return empty("No longer listed", "Choose another peer.");
                };
                let name = rules::display_name(r.display_name.as_deref());
                let incoming = r.direction == Direction::Incoming;
                let waiting = incoming && r.state == "pending";
                let state = match (incoming, r.state.as_str()) {
                    (true, "pending") => ui::tag("Wants to connect", ui::Tone::Tag),
                    (false, "pending") => ui::tag("Pending", ui::Tone::Quiet),
                    (false, "declined") => ui::tag("Declined", ui::Tone::Attention),
                    (_, s) => ui::tag(s.replace('_', " "), ui::Tone::Quiet),
                };
                let profile = self.profile_name.as_deref().unwrap_or("this profile");
                let (title, explanation) = match (incoming, r.state.as_str()) {
                    (true, "pending") => (
                        format!("A profile wants to connect to {profile}"),
                        format!(
                            "Accepting lets its agent send messages to this profile. Each one waits for your review before it reaches {LOCAL_AGENT}. Both directions start on Review, and you can disconnect at any time."
                        ),
                    ),
                    (false, "pending") => (
                        format!("Waiting for {name} to accept"),
                        "No message flows until they accept. Both directions start on Review."
                            .to_owned(),
                    ),
                    (false, "declined") => (
                        "They declined the connection request".to_owned(),
                        "Nothing was kept. Send a new request if they ask for one.".to_owned(),
                    ),
                    (_, state) => (
                        format!(
                            "{} request · {}",
                            if incoming { "Incoming" } else { "Outgoing" },
                            state.replace('_', " ")
                        ),
                        String::new(),
                    ),
                };
                let copied = self.copied.as_deref() == Some("copy-request-id");
                let full = r.peer_id.clone();
                let mut card = div()
                    .w(px(t.size.request_card))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s16))
                    .p(px(t.space.s24))
                    .rounded(px(t.radius.card))
                    .bg(c.panel)
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s6))
                            .child(
                                ui::text(div().text_color(c.text), &t.text.empty_title)
                                    .child(title),
                            )
                            .when(!explanation.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_color(c.muted)
                                        .line_height(gpui::relative(1.5))
                                        .child(explanation),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s6))
                            .child(meta(if incoming {
                                "Its connection ID. Check it matches the one they sent you.".into()
                            } else {
                                "Their connection ID".into()
                            }))
                            .child(
                                field_box(
                                    ui::text(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .font_family(t.font.mono.clone())
                                            .text_color(c.text)
                                            .line_height(gpui::relative(1.5)),
                                        &t.text.key,
                                    )
                                    .child(r.peer_id.clone()),
                                )
                                .gap(px(t.space.s8))
                                .bg(c.field)
                                .child({
                                    let copy = ui::interaction_icon_button(
                                        "copy-request-id",
                                        if copied { "check" } else { "copy" },
                                        "Copy this connection ID",
                                    )
                                    .tab_stop(true)
                                    .on_click(cx.listener(move |p, _, _, cx| {
                                        p.copy("copy-request-id".into(), full.clone(), cx)
                                    }));
                                    self.in_view(copy, None, &self.request_scroll, window, cx)
                                }),
                            ),
                    );
                if waiting && actions {
                    let (accept, decline) = (id.clone(), id.clone());
                    let accept = self.live_button(
                        "answer-true",
                        "Accept",
                        ui::Button::Primary,
                        cx,
                        move |p, w, cx| p.decide(Change::Answer(accept.clone(), true), w, cx),
                    );
                    let accept = self.in_view(accept, None, &self.request_scroll, window, cx);
                    let decline = ui::text_action("answer-false", "Decline")
                        .focus_visible(|el| {
                            el.border(px(tokens().size.focus_ring))
                                .border_color(colors().focus)
                        })
                        .text_color(c.text_2)
                        .h(px(t.size.button))
                        .px(px(t.space.s10))
                        .focusable()
                        .tab_stop(true)
                        .on_click(cx.listener(move |p, _, w, cx| {
                            p.decide(Change::Answer(decline.clone(), false), w, cx)
                        }));
                    let decline = self.in_view(decline, None, &self.request_scroll, window, cx);
                    card = card.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(t.space.s8))
                            .pt(px(t.space.s4))
                            .child(accept)
                            .child(decline)
                            .child(div().flex_1())
                            .child(meta("Declining tells them, and nothing is kept.".into())),
                    );
                }
                column
                    .child(self.live_header(
                        &snapshot.summary,
                        r.display_name.as_deref(),
                        &r.peer_id,
                        Some(state),
                        None,
                        cx,
                    ))
                    .child(
                        div()
                            .id("request")
                            .track_scroll(&self.request_scroll)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p(px(t.space.s24))
                            .child(card),
                    )
            }
            Some(Selection::Peer(id)) => {
                let Some(peer) = snapshot
                    .summary
                    .overview
                    .connections
                    .iter()
                    .find(|p| &p.connection == id)
                else {
                    return empty("No longer listed", "Choose another peer.");
                };
                let name = rules::display_name(peer.display_name.as_deref());
                let connected = peer.state == "connected";
                let state = if connected {
                    ui::tag("Connected", ui::Tone::Done)
                } else if peer.state == "revocation_pending" && peer.revocation_failure.is_some() {
                    // Only a disconnect the relay refused is a refusal.
                    ui::tag(link_status(peer), ui::Tone::Attention)
                } else {
                    ui::tag(link_status(peer), ui::Tone::Quiet)
                };
                let controls = connected.then(|| self.peer_controls(peer, actions, cx));
                let mut column = column.child(self.live_header(
                    &snapshot.summary,
                    peer.display_name.as_deref(),
                    &id.peer_id,
                    Some(state),
                    controls,
                    cx,
                ));
                // An open form stays usable while only a read is in flight: the
                // service checks each change, and the next read reconciles it.
                // The preview's records never wait on Agent Backbone.
                let held = connected
                    && (self.preview.is_some()
                        || self.live.stream_ok
                            && !self.live.sending
                            && self.live.status.is_none()
                            && self.live.retry.is_none());
                if held && matches!(self.live.form, Some(Form::Disconnect(_))) {
                    column = column.child(
                        div()
                            .px(px(t.space.s12))
                            .py(px(t.space.s8))
                            .bg(c.panel)
                            .border_b(px(t.size.hairline))
                            .border_color(c.line)
                            .child(self.live_form(cx)),
                    );
                }
                let note = revocation_note(peer);
                // Counts the thread's rows, to find the first waiting card.
                let mut rows = usize::from(note.is_some());
                let mut waiting_at = None;
                let mut body = body.children(note.map(|note| {
                    ui::text(div().text_color(c.attention_text), &t.text.meta).child(note)
                }));
                let Some(history) = &snapshot.history else {
                    return column.child(body);
                };
                if history.older.is_some() {
                    rows += 1;
                    let older = self.live_button(
                        "older-history",
                        "Older messages",
                        ui::Button::Secondary,
                        cx,
                        // The button goes once the oldest page is in.
                        |pane, window, cx| {
                            pane.keep_focus(&pane.thread_focus.clone(), window, cx);
                            pane.live_older(cx)
                        },
                    );
                    let older = self.in_view(older, None, &self.thread_scroll, window, cx);
                    body = body.child(div().flex().justify_center().child(older));
                }
                let now = Local::now();
                let mut day: Option<Local> = None;
                for item in thread(history) {
                    let at = match item {
                        ThreadItem::Event(e) => &e.at,
                        ThreadItem::Message(m) => &m.submitted_at,
                    };
                    let line = match item {
                        ThreadItem::Event(e) => match event_line(e, history) {
                            Some(line) => line,
                            None => continue,
                        },
                        ThreadItem::Message(m) if m.state == "revision_requested" => {
                            // A record without its event: the note is not known.
                            event(
                                "corner-down-left",
                                format!("You asked {} to revise its message", card::local_agent(m)),
                                String::new(),
                            )
                        }
                        ThreadItem::Message(m) => {
                            self.message_card(name, m, actions && connected, held, window, cx)
                        }
                    };
                    let waits =
                        matches!(item, ThreadItem::Message(m) if m.state == "pending_review");
                    if let Some(at) = epoch(at).and_then(Local::at)
                        && let Some(now) = &now
                        && !day.is_some_and(|d| d.same_day(&at))
                    {
                        rows += 1;
                        body = body.child(ui::thread_divider(at.day_name(now), false));
                        day = Some(at);
                    }
                    if waits && waiting_at.is_none() {
                        waiting_at = Some(rows);
                    }
                    rows += 1;
                    body = body.child(line);
                }
                // A thread opens on its first message waiting for the person,
                // with its choices in view, else on its latest message.
                // A thread drawn for the first time has no size yet to scroll
                // within, so it waits one frame.
                if self.scroll_on_open.get() {
                    if self.thread_scroll.bounds().size.height > px(0.0) {
                        self.scroll_on_open.set(false);
                        match waiting_at {
                            Some(row) => self.thread_scroll.scroll_to_item(row),
                            None => self.thread_scroll.scroll_to_bottom(),
                        }
                    } else if self.scroll_waits.get() < 2 {
                        // Two frames at most: a thread that never gets a
                        // height is left where it is.
                        self.scroll_waits.set(self.scroll_waits.get() + 1);
                        self.redraw_next_frame(window, cx);
                    } else {
                        self.scroll_on_open.set(false);
                    }
                }
                if history.items.is_empty() {
                    body = body.child(event(
                        "message-square",
                        "No messages yet.".into(),
                        String::new(),
                    ));
                }
                if !connected && !ends_disconnected(history, peer) {
                    body = body.child(event("link-2-off", link_status(peer), String::new()));
                }
                column.child(body)
            }
        }
    }

    /// The preview's records as a read of the service would give them.
    fn sample_snapshot(&self, sample: &model::Sample) -> Snapshot {
        let selection = resolve(&sample.overview, self.live.selected.clone());
        let history = match &selection {
            Some(Selection::Peer(generation)) => Some(sample.history(generation)),
            _ => None,
        };
        Snapshot {
            summary: Arc::new(Summary::with_counts(
                sample.overview.clone(),
                sample.counts(),
            )),
            selection,
            history,
        }
    }

    pub(super) fn live_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut view = div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col();
        let refresh = self.live_button(
            "live-refresh",
            "Retry",
            ui::Button::Secondary,
            cx,
            |pane, window, cx| {
                pane.keep_focus(&pane.entry.clone(), window, cx);
                pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx))
            },
        );
        let buttons = || {
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(px(t.space.s8))
        };
        let sample = self
            .preview
            .as_ref()
            .map(|sample| self.sample_snapshot(sample));
        let Some(s) = sample.as_ref().or(self.live.snapshot.as_ref()) else {
            let failed = self.live.status.clone();
            return view.child(ui::empty_state(
                ui::empty_tile(
                    "unplug",
                    c.selected,
                    if failed.is_some() {
                        c.attention
                    } else {
                        c.muted
                    },
                ),
                if failed.is_some() {
                    "Connections unavailable"
                } else {
                    "Reading profile connections…"
                },
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(t.space.s12))
                    .children(failed)
                    .child(
                        buttons().child(refresh).child(
                            self.live_button(
                                "preview-connections",
                                "Preview with sample data",
                                ui::Button::Secondary,
                                cx,
                                |pane, _, cx| pane.preview(true, cx),
                            )
                            .track_focus(&self.entry),
                        ),
                    ),
            ));
        };
        let preview = self.preview.is_some();
        let actions = preview
            || self.live.stream_ok
                && !self.live.reading
                && !self.live.sending
                && self.live.status.is_none()
                && self.live.retry.is_none();
        let local = s.summary.overview.local_id.clone();
        let overview = &s.summary.overview;
        // An invitation waits only for the service, not for a read in flight.
        let can_send = preview
            || self.live.stream_ok
                && !self.live.sending
                && self.live.status.is_none()
                && self.live.retry.is_none();
        let listed = !overview.connections.is_empty()
            || overview.invitations.iter().any(|r| r.state != "accepted");
        let mut main = div().relative().flex_1().min_h_0().min_w_0().flex();
        if !listed {
            let mut choices = buttons();
            if can_send {
                choices = choices.child(self.live_button(
                    "invite-peer",
                    "Connect a profile",
                    ui::Button::Primary,
                    cx,
                    |p, w, cx| p.show_form(Form::Connect, w, cx),
                ));
            }
            main = main.child(ui::empty_state(
                ui::empty_tile("link-2", c.selected, c.muted),
                "No profile connections yet",
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(t.space.s12))
                    .child(
                        "Connect this profile to another Agent Backbone profile so your agents can message each other, with your review in each direction.",
                    )
                    .child(choices.child(self.copy_button(
                        "copy-my-id",
                        "Copy my ID",
                        local.clone(),
                        true,
                        cx,
                    ))),
            ));
        } else {
            if self.width != peers::Width::Narrow {
                main = main.child(if self.list_minimised() {
                    self.peer_strip(&s.summary, window, cx)
                } else {
                    self.peer_list(&s.summary, window, cx)
                });
            }
            {
                main = main.child(loop {
                    // As in a reader: a block that changed after earlier ones
                    // were drawn clears the selection, so draw them again.
                    self.selection.borrow_mut().begin();
                    let thread = self.live_thread(s, actions, window, cx);
                    if !self.selection.borrow_mut().finish() {
                        break thread;
                    }
                });
            }
        }
        if matches!(self.live.form, Some(Form::Connect)) {
            // ＋ is drawn only beside a list or strip; otherwise the popover
            // opens centred near the top of the pane.
            let anchored = listed && self.width != peers::Width::Narrow;
            let panel = self.connect_panel(&local, can_send, anchored, window, cx);
            main = main.child(if anchored {
                panel.into_any_element()
            } else {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .pt(px(t.space.s24))
                    .child(panel)
                    .into_any_element()
            });
        }
        view = view.child(main).child(selection::events(cx));
        // The preview's records never reach Agent Backbone, so its state
        // and refusals are not shown over them.
        if preview {
        } else if let Some(error) = &self.live.status {
            view = view.child(band(error.clone()).child(refresh));
        } else if let Some(notice) = &self.live.notice {
            view = view.child(band(notice.clone()).child(self.live_button(
                "dismiss-notice",
                "Dismiss",
                ui::Button::Secondary,
                cx,
                |p, w, cx| {
                    p.keep_focus(&p.entry.clone(), w, cx);
                    p.live.notice = None;
                    cx.notify();
                },
            )));
        }
        // A lost decision on a message in the shown thread is offered again
        // on that message's card only.
        if !preview
            && let Some(pending) = &self.live.retry
            && let Some(error) = &pending.error
            && !on_card(&pending.action, s, self.live.selected.as_ref())
        {
            let mut banner = band(error.clone());
            if pending.action.belongs_to(self.live.selected.as_ref()) {
                if self.live.stream_ok && !self.live.sending {
                    banner = banner.child(self.live_button(
                        "retry-action",
                        "Retry action",
                        ui::Button::Secondary,
                        cx,
                        |p, w, cx| {
                            p.keep_focus(&p.entry.clone(), w, cx);
                            p.send_live(cx)
                        },
                    ));
                }
            } else if let Some(target) = pending.action.target(&s.summary.overview) {
                banner = banner.child(self.live_button(
                    "retry-target",
                    "Show target",
                    ui::Button::Secondary,
                    cx,
                    move |p, w, cx| {
                        p.keep_focus(&p.entry.clone(), w, cx);
                        p.pick(Some(target.clone()), cx)
                    },
                ));
            }
            banner = banner.child(self.live_button(
                "dismiss-retry",
                "Dismiss",
                ui::Button::Secondary,
                cx,
                |p, w, cx| {
                    p.keep_focus(&p.entry.clone(), w, cx);
                    p.live.retry = None;
                    cx.notify();
                },
            ));
            view = view.child(banner);
        }
        let waiting = s.summary.overview.pending_reviews;
        let requests = s.summary.requests();
        let count = [
            (requests, "request", "requests"),
            (
                waiting,
                "waiting for your review",
                "waiting for your review",
            ),
        ]
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect::<Vec<_>>()
        .join(" · ");
        let mut footer = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s12))
            .h(px(t.size.view_footer))
            .px(px(t.space.s20))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .child(
                ui::text(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .when(self.width != peers::Width::Narrow, |el| {
                    el.child("Agents write here. You decide what crosses a review gate.")
                }),
            );
        // The count opens the next peer waiting on the person.
        let next = peers::next_waiting(&peers::entries(&s.summary), self.live.selected.as_ref());
        if !count.is_empty()
            && let Some(next) = next
        {
            footer = footer.child(
                ui::text(
                    div()
                        .id("next-waiting")
                        .focus_visible(|el| {
                            el.border(px(tokens().size.focus_ring))
                                .border_color(colors().focus)
                        })
                        .flex_shrink_0()
                        .focusable()
                        .tab_stop(true)
                        .cursor_pointer()
                        .text_color(c.accent_text)
                        .hover(|el| el.underline()),
                    &t.text.meta,
                )
                .child(count)
                .on_click(cx.listener(move |p, _, _, cx| p.pick(Some(next.clone()), cx))),
            );
        } else if !count.is_empty() {
            // Nothing listed to open, but the count still says what waits.
            footer = footer.child(
                ui::text(
                    div().flex_shrink_0().text_color(c.accent_text),
                    &t.text.meta,
                )
                .child(count),
            );
        }
        view.child(footer)
    }
}

/// A padded attention band along the bottom: what went wrong, then its buttons.
fn band(message: String) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .flex_wrap()
        .items_center()
        .gap(px(t.space.s8))
        .px(px(t.space.s12))
        .py(px(t.space.s6))
        .bg(c.attention_soft)
        .border_t(px(t.size.hairline))
        .border_color(c.line)
        .child(
            ui::text(
                // A readable width before the buttons may sit beside it.
                div()
                    .flex_grow(1.0)
                    .flex_basis(px(t.size.empty_width / 2.0))
                    .min_w_0()
                    .text_color(c.attention_text),
                &t.text.meta,
            )
            .child(message),
        )
}

const AUTO_NOTE: &str =
    "Future messages on an Auto direction skip review. Already-waiting messages stay in review.";

/// A line of help shown on hover.
pub(super) struct Tip(pub(super) SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::menu()
            .max_w(px(tokens().size.connect_popover))
            .child(self.0.clone())
    }
}

/// A connection's mark: its name's first letter, or a dashed tile with a
/// link when it has no name.
pub(super) fn peer_mark(name: Option<&str>, size: f32) -> Div {
    let (t, c) = (tokens(), colors());
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => ui::profile_mark(name, size),
        None => div()
            .flex_shrink_0()
            .size(px(size))
            .rounded(px(size * ui::PROFILE_MARK_ROUND))
            .border(px(t.size.hairline))
            .border_dashed()
            .border_color(c.faint)
            .flex()
            .items_center()
            .justify_center()
            .child(ui::icon("link-2", size / 2.0, c.muted)),
    }
}

/// One entry in a thread, in the order it is drawn.
#[derive(Clone, Copy, Debug)]
enum ThreadItem<'a> {
    Event(&'a api::Event),
    Message(&'a api::Message),
}

/// A thread's messages and the events this app knows, oldest first. An
/// event goes after a message sent at its own time, and a draft sent back
/// gives way to the event that says so.
fn thread(history: &api::History) -> Vec<ThreadItem<'_>> {
    let sent_back: HashSet<&str> = history
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::RevisionRequested { message_id, .. } => Some(message_id.as_str()),
            _ => None,
        })
        .collect();
    let mut events = history
        .events
        .iter()
        .filter(|e| e.kind != EventKind::Unknown)
        .peekable();
    let mut items = Vec::new();
    for m in &history.items {
        let sent = epoch(&m.submitted_at).unwrap_or(f64::MAX);
        // An event without a readable time never holds back the ones after it.
        while let Some(e) = events.next_if(|e| epoch(&e.at).unwrap_or(f64::MIN) < sent) {
            items.push(ThreadItem::Event(e));
        }
        if !(m.state == "revision_requested" && sent_back.contains(m.message_id.as_str())) {
            items.push(ThreadItem::Message(m));
        }
    }
    items.extend(events.map(ThreadItem::Event));
    items
}

/// Whether the thread's last shown event already says what the connection's
/// closing line would.
fn ends_disconnected(history: &api::History, peer: &api::Peer) -> bool {
    peer.state == "disconnected"
        && history
            .events
            .iter()
            .rfind(|e| e.kind != EventKind::Unknown)
            .is_some_and(|e| e.kind == EventKind::Disconnected)
}

/// The line a connection event leaves in its thread, or none for a kind
/// this app does not know.
fn event_line(e: &api::Event, history: &api::History) -> Option<Div> {
    let (icon, happened, subject) = match &e.kind {
        EventKind::Connected => ("link-2", "Connected".to_owned(), String::new()),
        EventKind::GateChanged { direction, to, .. } => (
            "sliders-horizontal",
            format!(
                "{} gate set to {to:?}",
                match direction {
                    Direction::Incoming => "Incoming",
                    Direction::Outgoing => "Outgoing",
                }
            ),
            String::new(),
        ),
        EventKind::RevisionRequested { message_id, note } => {
            let agent = history
                .items
                .iter()
                .find(|m| &m.message_id == message_id)
                .map_or(LOCAL_AGENT, card::local_agent);
            (
                "corner-down-left",
                format!("You asked {agent} to revise its message"),
                format!(": \u{201c}{}\u{201d}", note.lines().next().unwrap_or("")),
            )
        }
        EventKind::DisconnectRequested => {
            ("link-2-off", "You disconnected".to_owned(), String::new())
        }
        EventKind::Disconnected => ("link-2-off", "Disconnected".to_owned(), String::new()),
        EventKind::Unknown => return None,
    };
    Some(event(
        icon,
        format!("{} · {happened}", rules::clock(&e.at)),
        subject,
    ))
}

/// Something that happened in a thread, centred on one muted line.
fn event(icon: &str, happened: String, subject: String) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .justify_center()
        .min_w_0()
        .child(ui::thread_event(
            ui::icon(icon, t.size.icon - 2.0, c.muted),
            happened,
            subject,
            "",
        ))
}

/// Seconds since the epoch for a service timestamp.
fn epoch(stamp: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|at| at.timestamp_millis() as f64 / 1000.0)
}

/// A change's repeat key: Agent Backbone takes only a UUIDv7 in 32 hex digits
/// and refuses any other request ID before routing it.
fn request_key() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

// Local revocation has stopped delivery; the peer has not yet acknowledged it,
// and may never learn of it once the relay has refused the revocation.
fn link_status(peer: &api::Peer) -> String {
    match (peer.state.as_str(), &peer.revocation_failure) {
        ("revocation_pending", Some(_)) => "Stopped here, relay refused the disconnect".into(),
        ("revocation_pending", None) => "Stopped here, remote confirmation pending".into(),
        (s, _) => s.replace('_', " "),
    }
}

// The full reason, shown below the header where it can wrap.
fn revocation_note(peer: &api::Peer) -> Option<String> {
    let failure = peer.revocation_failure.as_ref()?;
    (peer.state == "revocation_pending").then(|| {
        format!(
            "The relay refused the disconnect ({}), so the other side may not learn of it.",
            failure.code
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_segment_is_a_named_radio_button() {
        use gpui::{Element, accesskit};
        let segment = gate_segment(Direction::Incoming, "Incoming", Policy::Review, true);
        assert_eq!(segment.a11y_role(), Some(accesskit::Role::RadioButton));
        let mut node = accesskit::Node::new(accesskit::Role::RadioButton);
        segment.write_a11y_info(&mut node);
        assert_eq!(node.label(), Some("Incoming: Review"));
        assert_eq!(node.toggled(), Some(accesskit::Toggled::True));
        let mut node = accesskit::Node::new(accesskit::Role::RadioButton);
        gate_segment(Direction::Outgoing, "Outgoing", Policy::Auto, false)
            .write_a11y_info(&mut node);
        assert_eq!(node.label(), Some("Outgoing: Auto"));
        assert_eq!(node.toggled(), Some(accesskit::Toggled::False));
    }

    #[test]
    fn a_gate_segment_shows_the_service_policy_and_sends_one_change() {
        let [review, auto] = gate_choices("gen_1", Direction::Outgoing, Policy::Review);
        assert!(review.1 && review.2.is_none());
        assert!(!auto.1);
        let Some(Change::Gates(generation, changes)) = auto.2 else {
            panic!("choosing Auto sends a gate change");
        };
        assert_eq!(generation, "gen_1");
        assert!(matches!(
            changes.as_slice(),
            [(Direction::Outgoing, Policy::Auto, key)] if key.len() == 32
        ));
        // Until the service reports the new policy, the segments stay put;
        // a refused change is never shown as made.
        let refused = Snapshot {
            summary: Arc::new(Summary::from_overview(api::Overview {
                local_id: "peer_local".into(),
                pending_reviews: 0,
                invitations: vec![],
                connections: vec![api::Peer {
                    connection: api::Generation {
                        peer_id: "peer_a".into(),
                        generation: "gen_1".into(),
                    },
                    display_name: None,
                    state: "connected".into(),
                    gates: api::Gates {
                        incoming: Policy::Review,
                        outgoing: Policy::Review,
                    },
                    revocation_failure: None,
                }],
            })),
            selection: None,
            history: None,
        };
        let change = Change::Gates(generation, changes);
        assert!(change.still_pending(&refused));
    }

    #[test]
    fn a_notice_names_the_connection_never_its_ids() {
        let overview: api::Overview = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/peers/fixtures/peer-overview.json"
        )))
        .unwrap();
        let peer = &overview.connections[0];
        let generation = peer.connection.generation.clone();
        let name = known_as(peer.display_name.as_deref(), &peer.connection.peer_id);
        for change in [
            Change::Disconnect(generation.clone()),
            Change::Gates(generation.clone(), vec![]),
        ] {
            let text = change.description(Some(&overview));
            assert!(text.ends_with(&name), "{text}");
            assert!(!text.contains(&generation) && !text.contains(&peer.connection.peer_id));
        }
        let unnamed = format!("v1:a1b2c3d4{}9e8f", "0".repeat(52));
        assert_eq!(
            Change::Invite(unnamed.clone()).description(None),
            "Connection request to Unnamed profile a1b2c3d4…9e8f"
        );
        assert_eq!(
            Change::Disconnect(generation).description(None),
            "Disconnect from this connection"
        );
    }

    #[test]
    fn request_keys_are_the_uuid_v7_agent_backbone_accepts() {
        let key = request_key();
        assert_eq!(key.len(), 32);
        assert_eq!(uuid::Uuid::parse_str(&key).unwrap().get_version_num(), 7);
    }

    #[test]
    fn a_local_disconnect_reads_as_stopped_until_the_peer_confirms() {
        let actions: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/peers/fixtures/peer-actions.json"
        )))
        .unwrap();
        let peer = |key: &str| serde_json::from_value::<api::Peer>(actions[key].clone()).unwrap();
        assert_eq!(
            link_status(&peer("disconnect_requested")),
            "Stopped here, remote confirmation pending"
        );
        assert_eq!(link_status(&peer("disconnect_confirmed")), "disconnected");
        let refused: api::Peer = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/peers/fixtures/peer-disconnect-refused.json"
        )))
        .unwrap();
        assert_eq!(
            link_status(&refused),
            "Stopped here, relay refused the disconnect"
        );
        assert_eq!(
            revocation_note(&refused).as_deref(),
            Some(
                "The relay refused the disconnect (generation_mismatch), so the other side may not learn of it."
            )
        );
        assert_eq!(revocation_note(&peer("disconnect_requested")), None);
    }

    fn events_fixture() -> api::History {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/peers/fixtures/peer-history-events.json"
        )))
        .unwrap()
    }

    fn order(history: &api::History) -> Vec<String> {
        thread(history)
            .into_iter()
            .map(|item| match item {
                ThreadItem::Event(e) => format!("event {}", e.id),
                ThreadItem::Message(m) => format!("message {}", &m.message_id[30..]),
            })
            .collect()
    }

    #[test]
    fn events_sit_among_messages_by_time_and_a_sent_back_draft_gives_way() {
        let mut history = events_fixture();
        // Event 4 is at message 71's own time, so it follows it; the draft
        // sent back (70) is replaced by its revision event (3).
        let expected = [
            "event 1",
            "event 2",
            "event 3",
            "message 71",
            "event 4",
            "message 72",
            "event 5",
            "event 6",
        ];
        assert_eq!(order(&history), expected);
        // An unreadable time never holds back the events after it.
        history.events[0].at = "not a time".into();
        assert_eq!(order(&history), expected);
        history.events[1].kind = EventKind::Unknown;
        assert!(!order(&history).contains(&"event 2".to_owned()));
        // Without its event, the draft keeps its place.
        history.events.retain(|e| e.id != 3);
        assert_eq!(order(&history)[1], "message 70");
    }

    #[test]
    fn a_thread_ending_in_its_disconnect_has_no_second_line() {
        let mut history = events_fixture();
        let mut peer = api::Peer {
            connection: history.items[0].connection.clone(),
            display_name: None,
            state: "disconnected".into(),
            gates: api::Gates {
                incoming: Policy::Review,
                outgoing: Policy::Review,
            },
            revocation_failure: None,
        };
        assert!(ends_disconnected(&history, &peer));
        let mut later = history.events[0].clone();
        later.kind = EventKind::Unknown;
        history.events.push(later);
        assert!(ends_disconnected(&history, &peer));
        peer.state = "revocation_pending".into();
        assert!(!ends_disconnected(&history, &peer));
    }

    #[test]
    fn an_older_page_goes_first_and_keeps_one_of_each() {
        let whole = events_fixture();
        let mut newer = whole.clone();
        let mut older = whole.clone();
        newer.items.drain(..1);
        newer.events.drain(..2);
        older.items.truncate(2);
        older.events.truncate(3);
        prepend(&mut newer, &mut older);
        let ids = |h: &api::History| {
            (
                h.items
                    .iter()
                    .map(|m| m.message_id.clone())
                    .collect::<Vec<_>>(),
                h.events.iter().map(|e| e.id).collect::<Vec<_>>(),
            )
        };
        assert_eq!(ids(&older), ids(&whole));
    }

    #[test]
    fn a_lost_decision_is_offered_on_its_card_only_when_the_card_is_shown() {
        let history: api::History = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/peers/fixtures/peer-history.json"
        )))
        .unwrap();
        let waiting = history
            .items
            .iter()
            .find(|m| m.state == "pending_review")
            .unwrap()
            .clone();
        let peer = Selection::Peer(waiting.connection.clone());
        let shown = |items: Vec<api::Message>| Snapshot {
            summary: Arc::new(Summary::from_overview(api::Overview {
                local_id: "peer_local".into(),
                pending_reviews: 0,
                invitations: vec![],
                connections: vec![],
            })),
            selection: Some(peer.clone()),
            history: Some(api::History {
                items,
                older: None,
                events: vec![],
            }),
        };
        let lost = Change::Review(Box::new(waiting.clone()), DecisionKind::Pass);
        assert!(on_card(&lost, &shown(history.items.clone()), Some(&peer)));
        // Not on the loaded page: the band keeps it, with Retry and Dismiss.
        let others = history
            .items
            .iter()
            .filter(|m| m.message_id != waiting.message_id)
            .cloned()
            .collect();
        assert!(!on_card(&lost, &shown(others), Some(&peer)));
        assert!(!on_card(&lost, &shown(history.items.clone()), None));
        assert!(!on_card(
            &Change::Disconnect("gen".into()),
            &shown(vec![]),
            Some(&peer)
        ));
    }

    #[test]
    fn retry_reconciliation_clears_committed_answer_and_disconnect() {
        let generation = api::Generation {
            peer_id: "peer_a".into(),
            generation: "gen_1".into(),
        };
        let accepted = Snapshot {
            summary: Arc::new(Summary::from_overview(api::Overview {
                local_id: "peer_local".into(),
                pending_reviews: 0,
                invitations: vec![api::Invitation {
                    request_id: "req_1".into(),
                    direction: Direction::Incoming,
                    peer_id: "peer_a".into(),
                    display_name: None,
                    expires_at: "2026-11-23T00:00:00Z".into(),
                    state: "accepted".into(),
                    generation: Some("gen_1".into()),
                }],
                connections: vec![api::Peer {
                    connection: generation.clone(),
                    display_name: None,
                    state: "disconnected".into(),
                    gates: api::Gates {
                        incoming: Policy::Review,
                        outgoing: Policy::Unknown,
                    },
                    revocation_failure: None,
                }],
            })),
            selection: Some(Selection::Peer(generation)),
            history: None,
        };
        assert!(!Change::Answer("req_1".into(), true).still_pending(&accepted));
        assert!(!Change::Disconnect("gen_1".into()).still_pending(&accepted));
    }
}
