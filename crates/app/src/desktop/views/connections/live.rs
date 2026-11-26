//! Owner-scoped data for the native Connections pane. Stream frames only invalidate reads.
use super::card::LOCAL_AGENT;
use super::rules;
use super::*;
use crate::desktop::peer_inbox::{Inbox, Selection, Summary, failure};
use crate::desktop::views::clock::Local;
use agent_interface_roster::profiles::{AddError, Connection, Error, peers as api};
use api::{Direction, Policy};
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

    fn description(&self) -> String {
        match self {
            Self::Invite(id) => format!("Invitation to {id}"),
            Self::Answer(id, _) => format!("Answer to request {id}"),
            Self::Disconnect(g) => format!("Disconnect generation {g}"),
            Self::Gates(g, _) => format!("Gate change for generation {g}"),
            Self::Review(m, _) => {
                format!("Review of {} for {}", m.message_id, m.connection.peer_id)
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
    history: Option<api::Page<api::Message>>,
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
                        let existing: HashSet<_> =
                            page.items.iter().map(|m| m.message_id.as_str()).collect();
                        prior
                            .items
                            .retain(|m| !existing.contains(m.message_id.as_str()));
                        prior.items.append(&mut page.items);
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

/// The line a gate change leaves in its thread.
fn gate_event(change: &Change) -> Option<(String, String)> {
    let Change::Gates(generation, changes) = change else {
        return None;
    };
    let text = changes
        .iter()
        .map(|(direction, policy, _)| {
            format!(
                "{} gate set to {policy:?}",
                match direction {
                    Direction::Incoming => "Incoming",
                    Direction::Outgoing => "Outgoing",
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let at = chrono::Local::now().format("%H:%M");
    Some((generation.clone(), format!("{at} · {text}")))
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
    /// Gate changes made in this pane, by generation, for their thread.
    events: Vec<(String, String)>,
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
            events: Vec::new(),
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
        if let Some(event) = gate_event(&action) {
            self.live.events.push(event);
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
                    DecisionKind::Revise(_) => model::Decision::Revise,
                },
            ),
            Change::Disconnect(generation) => sample.disconnect(&generation),
        }
        self.live.form = None;
        self.live.selected = resolve(&sample.overview, self.live.selected.take());
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
        let description = change.description();
        let event = gate_event(&change);
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
                        pane.live.events.extend(event);
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

    pub(super) fn submit_live_form(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().trim().to_owned();
        if value.is_empty() {
            return;
        }
        match &self.live.form {
            // An ID never contains spaces; a paste can carry a line break.
            Some(Form::Connect) => {
                self.change(Change::Invite(value.split_whitespace().collect()), cx)
            }
            Some(Form::Revise(m)) => {
                let at = chrono::Local::now().format("%H:%M").to_string();
                self.notes.insert(m.message_id.clone(), (value.clone(), at));
                let m = m.clone();
                self.change(Change::Review(Box::new(m), DecisionKind::Revise(value)), cx)
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
            .focusable()
            .tab_stop(true)
            .on_click(cx.listener(move |pane, _, window, cx| action(pane, window, cx)))
    }

    pub(super) fn show_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
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
        let cancel = self.live_button(
            "live-cancel",
            "Cancel",
            ui::Button::Secondary,
            cx,
            |pane, _, cx| {
                pane.live.form = None;
                cx.notify();
            },
        );
        let strip = |text: String| {
            div()
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
                move |pane, _, cx| pane.change(action.clone(), cx),
            ))
            .child(cancel)
    }

    /// ＋'s panel: paste another profile's ID, or copy this one's.
    fn connect_panel(&self, local: &str, actions: bool, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let label = |text: &'static str| {
            ui::text(div().text_color(c.text_2), &t.text.meta)
                .font_weight(t.text.row_selected.weight())
                .child(text)
        };
        let note =
            |text: &'static str| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let close = |pane: &mut Self, cx: &mut Context<Self>| {
            pane.live.form = None;
            cx.notify();
        };
        let mut send = div().flex().flex_wrap().gap(px(t.space.s8));
        if actions {
            send = send.child(self.live_button(
                "live-submit",
                "Send request",
                ui::Button::Primary,
                cx,
                |pane, _, cx| pane.submit_live_form(cx),
            ));
        }
        let local = local.to_owned();
        ui::menu()
            .id("connect-panel")
            .absolute()
            .top(px(t.space.s8))
            .left(px(t.space.s8))
            .max_h(gpui::relative(0.94))
            .overflow_y_scroll()
            .w(px(t.size.connect_popover))
            .max_w(gpui::relative(0.92))
            .gap(px(t.space.s8))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(t.text.row_selected.weight())
                            .child("Connect a profile"),
                    )
                    .child(
                        ui::interaction_icon_button("close-connect", "x", "Close")
                            .tab_stop(true)
                            .on_click(cx.listener(move |pane, _, _, cx| close(pane, cx))),
                    ),
            )
            .child(label("Send a connection request"))
            .child(field_box(hinted(&self.input, "Paste the other profile’s ID", cx)))
            .child(send.child(self.live_button(
                "live-cancel",
                "Cancel",
                ui::Button::Secondary,
                cx,
                move |pane, _, cx| close(pane, cx),
            )))
            .child(note(if actions && cfg!(target_os = "macos") {
                "⌘↵ sends. Both directions start on Review once they accept."
            } else if actions {
                "Ctrl+↵ sends. Both directions start on Review once they accept."
            } else {
                "Sending is paused until Agent Backbone answers."
            }))
            .child(div().h(px(t.size.hairline)).bg(c.line))
            .child(label("This profile’s ID"))
            .child(field_box(ui::mono(local.clone()).text_color(c.text_2)))
            .child(div().flex().child(self.copy_button("copy-local-id", "Copy ID", local, false, cx)))
            .child(note(
                "Share it outside the app so the other profile can send you a request. Agent Backbone handles keys and relay delivery.",
            ))
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
                            let existing: HashSet<_> = history
                                .items
                                .iter()
                                .map(|m| m.message_id.as_str())
                                .collect();
                            page.items
                                .retain(|m| !existing.contains(m.message_id.as_str()));
                            page.items.append(&mut history.items);
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
                .child(self.peer_chooser(s, None, "Choose a peer", cx))
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
        let copy = ui::interaction_icon_button("copy-peer-id", "copy", "Copy its connection ID")
            .tab_stop(true)
            .when(self.copied.as_deref() == Some("copy-peer-id"), |el| {
                el.child(ui::icon("check", t.size.icon - 1.0, c.ok))
            });
        let id = peer_id.to_owned();
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
            .map(|el| {
                if self.width == peers::Width::Narrow {
                    el.child(self.peer_chooser(s, name, rules::display_name(name), cx))
                } else {
                    el.child(peer_mark(name, t.size.view_tile - t.space.s4))
                        .child(
                            ui::text(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(c.text),
                                &t.text.workspace,
                            )
                            .font_weight(t.text.row_selected.weight())
                            .child(rules::display_name(name).to_owned()),
                        )
                }
            })
            .children(state)
            .child(
                ui::text(
                    div().flex_shrink_0().font_family(t.font.mono.clone()),
                    &t.text.key,
                )
                .text_color(c.muted)
                .child(rules::fingerprint(peer_id)),
            )
            .child(copy.on_click(
                cx.listener(move |p, _, _, cx| p.copy("copy-peer-id".into(), id.clone(), cx)),
            ))
            .child(div().flex_1())
            .children(controls)
    }

    /// Each gate's label and its Review and Auto segments.
    fn gate_rows(&self, peer: &api::Peer, actions: bool, cx: &Context<Self>) -> Div {
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
                        .flex()
                        .items_center()
                        .gap(px(t.space.s6))
                        .when(direction == Direction::Outgoing, |el| el.ml(px(t.space.s6)))
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child(ui::icon(icon, t.size.icon - 2.0, c.muted))
                .child(label),
            );
            if current == Policy::Unknown {
                controls = controls.child(ui::tag("unknown to this version", ui::Tone::Quiet));
                continue;
            }
            let mut segments = ui::segments();
            for (choice, on, change) in
                gate_choices(&peer.connection.generation, direction, current)
            {
                let mut segment = ui::segment(
                    format!("gate-{direction:?}-{choice:?}"),
                    format!("{choice:?}"),
                    on,
                )
                .focusable()
                .tab_stop(true);
                if choice == Policy::Auto {
                    segment = segment.tooltip(|_, cx| cx.new(|_| Tip(AUTO_NOTE)).into());
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
        if self.width == peers::Width::Narrow {
            // Narrow: both gates fold into one button that opens them.
            let letter = |policy: Policy| match policy {
                Policy::Review => "Review",
                Policy::Auto => "Auto",
                Policy::Unknown => "?",
            };
            let mut gates = div().relative().child(
                ui::button(
                    "folded-gates",
                    format!(
                        "↙ {} · ↗ {}",
                        letter(peer.gates.incoming),
                        letter(peer.gates.outgoing)
                    ),
                    ui::Button::Secondary,
                )
                .h(px(t.size.icon_button))
                .px(px(t.space.s8))
                .child(ui::icon("chevron-down", t.size.icon - 2.0, c.muted))
                .focusable()
                .tab_stop(true)
                .on_click(cx.listener(|p, _, _, cx| {
                    p.menu = if p.menu == Menu::Gates {
                        Menu::None
                    } else {
                        Menu::Gates
                    };
                    cx.notify();
                })),
            );
            if self.menu == Menu::Gates {
                gates = gates.child(
                    gpui::deferred(
                        ui::menu()
                            .absolute()
                            .top(px(t.size.icon_button + t.space.s4))
                            .right_0()
                            .gap(px(t.space.s8))
                            .on_mouse_down_out(cx.listener(|p, _, _, cx| {
                                p.menu = Menu::None;
                                cx.notify();
                            }))
                            .child(self.gate_rows(peer, actions, cx).flex_col().items_start()),
                    )
                    .with_priority(1),
                );
            }
            controls = controls.child(gates);
        } else {
            controls = controls.child(self.gate_rows(peer, actions, cx));
        }
        let generation = peer.connection.generation.clone();
        let mut more = div().relative().ml(px(t.space.s4)).child(
            ui::interaction_icon_button("peer-more", "ellipsis", "More for this connection")
                .tab_stop(true)
                .on_click(cx.listener(|p, _, _, cx| {
                    p.menu = if p.menu == Menu::More {
                        Menu::None
                    } else {
                        Menu::More
                    };
                    cx.notify();
                })),
        );
        if self.menu == Menu::More && actions {
            more = more.child(
                gpui::deferred(
                    ui::menu()
                        .absolute()
                        .top(px(t.size.icon_button + t.space.s4))
                        .right_0()
                        .on_mouse_down_out(cx.listener(|p, _, _, cx| {
                            p.menu = Menu::None;
                            cx.notify();
                        }))
                        .child(
                            ui::menu_item(
                                "peer-disconnect",
                                Some(ui::icon("link-2-off", t.size.icon, c.attention_text)),
                                "Disconnect",
                                None,
                            )
                            .text_color(c.attention_text)
                            .on_click(cx.listener(
                                move |p, _, w, cx| {
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

    fn live_thread(&self, snapshot: &Snapshot, actions: bool, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let column = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        let body = div()
            .id("peer-history")
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
            .px(px(t.space.s20))
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
                let name = r.display_name.clone().unwrap_or_else(|| r.peer_id.clone());
                let incoming = r.direction == Direction::Incoming;
                let waiting = incoming && r.state == "pending";
                let state = match (incoming, r.state.as_str()) {
                    (true, "pending") => ui::tag("Connection request", ui::Tone::Attention),
                    (false, "pending") => ui::tag("Pending", ui::Tone::Tag),
                    (false, "declined") => ui::tag("Declined", ui::Tone::Attention),
                    (_, s) => ui::tag(s.replace('_', " "), ui::Tone::Quiet),
                };
                let mut card = body.child(
                    div()
                        .max_w(px(t.size.empty_width * 1.6))
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s10))
                        .p(px(t.space.s14))
                        .rounded(px(t.radius.popover))
                        .bg(c.panel)
                        .border(px(t.size.hairline))
                        .border_color(if waiting { c.attention } else { c.line })
                        .child(
                            div().font_weight(t.text.row_selected.weight()).child(
                                match (incoming, r.state.as_str()) {
                                    (true, "pending") => format!("{name} wants to connect"),
                                    (false, "pending") => {
                                        format!("Waiting for {name} to accept")
                                    }
                                    (false, "declined") => {
                                        "Connection request declined".to_owned()
                                    }
                                    _ => format!(
                                        "{} request · {}",
                                        if incoming { "Incoming" } else { "Outgoing" },
                                        r.state.replace('_', " ")
                                    ),
                                },
                            ),
                        )
                        .children(
                            r.display_name
                                .is_some()
                                .then(|| meta("Name provided by requester".into())),
                        )
                        .child(field_box(ui::mono(r.peer_id.clone()).text_color(c.text_2)))
                        .when(waiting, |el| {
                            el.child(
                                ui::text(div().text_color(c.text_2), &t.text.body).child(format!(
                                    "Accepting lets {name}’s agent send messages to your profile. Both directions start on Review, so nothing passes without you."
                                )),
                            )
                        })
                        .when(waiting && actions, |el| {
                            let mut buttons = div().flex().flex_wrap().gap(px(t.space.s8));
                            for (accept, label) in [(true, "Accept"), (false, "Decline")] {
                                let id = id.clone();
                                buttons = buttons.child(self.live_button(
                                    format!("answer-{accept}"),
                                    label,
                                    if accept {
                                        ui::Button::Primary
                                    } else {
                                        ui::Button::Danger
                                    },
                                    cx,
                                    move |p, _, cx| p.change(Change::Answer(id.clone(), accept), cx),
                                ));
                            }
                            el.child(buttons)
                        }),
                );
                card = card.items_center();
                column
                    .child(self.live_header(
                        &snapshot.summary,
                        r.display_name.as_deref(),
                        &r.peer_id,
                        Some(state),
                        None,
                        cx,
                    ))
                    .child(card)
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
                } else if peer.state == "revocation_pending" {
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
                let held = connected
                    && (self.preview.is_some() || self.live.stream_ok)
                    && !self.live.sending
                    && self.live.status.is_none()
                    && self.live.retry.is_none();
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
                let mut body = body.children(revocation_note(peer).map(|note| {
                    ui::text(div().text_color(c.attention_text), &t.text.meta).child(note)
                }));
                let Some(history) = &snapshot.history else {
                    return column.child(body);
                };
                if history.older.is_some() {
                    body = body.child(div().flex().justify_center().child(self.live_button(
                        "older-history",
                        "Older messages",
                        ui::Button::Secondary,
                        cx,
                        |pane, _, cx| pane.live_older(cx),
                    )));
                }
                if history.items.is_empty() {
                    body = body.child(event(
                        "message-square",
                        "No messages yet.".into(),
                        String::new(),
                    ));
                }
                let now = Local::now();
                let mut day: Option<Local> = None;
                for m in &history.items {
                    if let Some(at) = epoch(&m.submitted_at).and_then(Local::at)
                        && let Some(now) = &now
                        && !day.is_some_and(|d| d.same_day(&at))
                    {
                        body = body.child(ui::thread_divider(at.day_name(now), false));
                        day = Some(at);
                    }
                    body = body.child(if m.state == "revision_requested" {
                        let (note, at) = self.notes.get(&m.message_id).cloned().unzip();
                        event(
                            "corner-down-left",
                            format!(
                                "{}You asked {LOCAL_AGENT} to revise its message",
                                at.map(|at| format!("{at} · ")).unwrap_or_default()
                            ),
                            note.map(|note| {
                                format!(": \u{201c}{}\u{201d}", note.lines().next().unwrap_or(""))
                            })
                            .unwrap_or_default(),
                        )
                    } else {
                        let revision = self
                            .preview
                            .as_ref()
                            .and_then(|sample| sample.revisions.get(&m.message_id).copied());
                        self.message_card(name, m, revision, actions && connected, cx)
                    });
                }
                for (_, text) in self.live.events.iter().filter(|(g, _)| g == &id.generation) {
                    body = body.child(event("sliders-horizontal", text.clone(), String::new()));
                }
                if !connected {
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

    pub(super) fn live_view(&self, cx: &mut Context<Self>) -> Div {
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
            |pane, _, cx| pane.live.inbox.update(cx, |inbox, cx| inbox.refresh(cx)),
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
                    self.peer_strip(&s.summary, cx)
                } else {
                    self.peer_list(&s.summary, cx)
                });
            }
            {
                main = main.child(loop {
                    // As in a reader: a block that changed after earlier ones
                    // were drawn clears the selection, so draw them again.
                    self.selection.borrow_mut().begin();
                    let thread = self.live_thread(s, actions, cx);
                    if !self.selection.borrow_mut().finish() {
                        break thread;
                    }
                });
            }
        }
        if matches!(self.live.form, Some(Form::Connect)) {
            main = main.child(self.connect_panel(&local, can_send, cx));
        }
        view = view.child(main).child(selection::events(cx));
        if let Some(error) = &self.live.status {
            view = view.child(band(error.clone()).child(refresh));
        } else if let Some(notice) = &self.live.notice {
            view = view.child(band(notice.clone()).child(self.live_button(
                "dismiss-notice",
                "Dismiss",
                ui::Button::Secondary,
                cx,
                |p, _, cx| {
                    p.live.notice = None;
                    cx.notify();
                },
            )));
        }
        if let Some(pending) = &self.live.retry
            && let Some(error) = &pending.error
        {
            let mut banner = band(error.clone());
            if pending.action.belongs_to(self.live.selected.as_ref()) {
                if self.live.stream_ok && !self.live.sending {
                    banner = banner.child(self.live_button(
                        "retry-action",
                        "Retry action",
                        ui::Button::Secondary,
                        cx,
                        |p, _, cx| p.send_live(cx),
                    ));
                }
            } else if let Some(target) = pending.action.target(&s.summary.overview) {
                banner = banner.child(self.live_button(
                    "retry-target",
                    "Show target",
                    ui::Button::Secondary,
                    cx,
                    move |p, _, cx| p.pick(Some(target.clone()), cx),
                ));
            }
            banner = banner.child(self.live_button(
                "dismiss-retry",
                "Dismiss",
                ui::Button::Secondary,
                cx,
                |p, _, cx| {
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
                .child("Agents write here. You decide what crosses a review gate."),
            );
        // The count opens the first peer waiting on the person.
        let rows = s.summary.attention_rows();
        if let Some((next, ..)) = rows
            .iter()
            .find(|(id, ..)| Some(id) != self.live.selected.as_ref())
            .or(rows.first())
            .cloned()
            && !count.is_empty()
        {
            footer = footer.child(
                ui::text(
                    div()
                        .id("next-waiting")
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
struct Tip(&'static str);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::menu()
            .max_w(px(tokens().size.connect_popover))
            .child(self.0)
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
        assert!(gate_event(&change).is_some_and(|(g, text)| {
            g == "gen_1" && text.ends_with("Outgoing gate set to Auto")
        }));
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
