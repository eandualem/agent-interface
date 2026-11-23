//! Owner-scoped data for the native Connections pane. Stream frames only invalidate reads.
use super::*;
use crate::desktop::peer_inbox::{Inbox, Selection, Summary, failure};
use agent_interface_roster::profiles::{Connection, Error, peers as api};
use api::{Direction, Policy};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
enum Change {
    Invite(String),
    Answer(String, bool),
    Gates(String, Vec<(Direction, Policy, String)>),
    Review(Box<api::Message>, DecisionKind),
    Disconnect(String),
}

#[derive(Clone)]
enum DecisionKind {
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

    fn send(&self, connection: &Connection, key: &str) -> Result<(), Error> {
        match self {
            Self::Invite(id) => connection.invite_peer(id, key).map(drop),
            Self::Answer(id, accept) => connection.answer_peer(id, *accept, key).map(drop),
            Self::Gates(generation, changes) => {
                for (direction, policy, key) in changes {
                    connection.set_peer_gate(generation, *direction, *policy, key)?;
                }
                Ok(())
            }
            Self::Disconnect(generation) => connection.disconnect_peer(generation, key).map(drop),
            Self::Review(message, decision) => {
                let action = match decision {
                    DecisionKind::Pass => api::Decision::Pass,
                    DecisionKind::Decline => api::Decision::Decline,
                    DecisionKind::Revise(note) => api::Decision::Revise(note),
                };
                connection
                    .review_peer_message(message, action, key)
                    .map(drop)
            }
        }
    }
}

pub(super) enum Form {
    Connect,
    Decline(api::Message),
    Revise(api::Message),
    Disconnect(String),
    Gate(String, api::Gates, api::Gates),
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
        let overview = &summary.overview;
        let selection = match selected {
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
        };
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

struct PendingRetry {
    action: Change,
    key: String,
    error: Option<String>,
}

pub(super) struct Live {
    connection: Connection,
    selected: Option<Selection>,
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

    pub(in crate::desktop) fn pick_peer(&mut self, target: Selection, cx: &mut Context<Self>) {
        if self.preview {
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
        if !self.live.stream_ok {
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
                            if matches!(&live.form,Some(Form::Gate(id,original,_)) if !snapshot.summary.overview.connections.iter().any(|p| &p.connection.generation==id && &p.gates==original))
                                || matches!(&live.form, Some(Form::Gate(id, ..) | Form::Disconnect(id)) if !connected(id))
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

    fn pick(&mut self, target: Option<Selection>, cx: &mut Context<Self>) {
        self.live.oldest = None;
        self.live.selected = target;
        self.live.form = None;
        self.show_thread = self.live.selected.is_some();
        self.copied = false;
        self.refresh_live(cx);
        cx.notify();
    }

    fn change(&mut self, action: Change, cx: &mut Context<Self>) {
        if self.preview
            || !action.belongs_to(self.live.selected.as_ref())
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

    fn send_live(&mut self, cx: &mut Context<Self>) {
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
        if self.preview || !change.belongs_to(self.live.selected.as_ref()) {
            return;
        }
        let description = change.description();
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
                    Err(error) => {
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
            Some(Form::Revise(m)) => self.change(
                Change::Review(Box::new(m.clone()), DecisionKind::Revise(value)),
                cx,
            ),
            _ => {}
        }
    }
}

impl Pane {
    fn live_button(
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

    fn show_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
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
            Form::Connect => return div(),
            Form::Revise(_) => {
                return div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .child(
                        ui::text(div().text_color(c.text_2), &t.text.meta)
                            .child("Revision note for main"),
                    )
                    .child(field_box(hinted(&self.input, "What should main change?", cx)))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(t.space.s8))
                            .child(self.live_button(
                                "live-submit",
                                "Send note to main",
                                ui::Button::Primary,
                                cx,
                                |pane, _, cx| pane.submit_live_form(cx),
                            ))
                            .child(cancel),
                    );
            }
            Form::Decline(message) => (
                "Confirm decline",
                "Decline this message? It will not be delivered.".to_owned(),
                Change::Review(Box::new(message.clone()), DecisionKind::Decline),
            ),
            Form::Disconnect(id) => (
                "Disconnect",
                "Disconnect? Delivery stops here at once; the other profile confirms when it next connects.".to_owned(),
                Change::Disconnect(id.clone()),
            ),
            Form::Gate(id, original, draft) => {
                let mut menu = ui::menu()
                    .min_w_0()
                    .max_w_full()
                    .gap(px(t.space.s8))
                    .child(ui::menu_heading("Delivery gate"));
                for d in [Direction::Incoming, Direction::Outgoing] {
                    let original_policy = match d {
                        Direction::Incoming => original.incoming,
                        Direction::Outgoing => original.outgoing,
                    };
                    let name = match d {
                        Direction::Incoming => "↙ Incoming",
                        Direction::Outgoing => "↗ Outgoing",
                    };
                    if original_policy == Policy::Unknown {
                        menu = menu.child(format!("{name} · unknown to this version"));
                        continue;
                    }
                    let mut row = ui::segments();
                    for choice in [Policy::Review, Policy::Auto] {
                        let id = id.clone();
                        let original = *original;
                        let mut next = *draft;
                        let current = match d {
                            Direction::Incoming => &mut next.incoming,
                            Direction::Outgoing => &mut next.outgoing,
                        };
                        let selected = *current == choice;
                        *current = choice;
                        row = row.child(
                            ui::segment(format!("gate-{d:?}-{choice:?}"), format!("{choice:?}"), selected)
                                .focusable()
                                .tab_stop(true)
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.live.form = Some(Form::Gate(id.clone(), original, next));
                                    cx.notify();
                                })),
                        );
                    }
                    menu = menu.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(t.space.s8))
                            .child(name)
                            .child(row),
                    );
                }
                let buttons = div().flex().flex_wrap().gap(px(t.space.s8));
                if original == draft {
                    return menu.child(buttons.child(cancel));
                }
                let changes = [
                    (Direction::Incoming, original.incoming, draft.incoming),
                    (Direction::Outgoing, original.outgoing, draft.outgoing),
                ]
                .into_iter()
                .filter(|(_, a, b)| a != b)
                .map(|(d, _, p)| (d, p, request_key()))
                .collect();
                let action = Change::Gates(id.clone(), changes);
                let automatic = draft.incoming == Policy::Auto || draft.outgoing == Policy::Auto;
                let apply = self.live_button(
                    "apply-live-gate",
                    "Apply gate",
                    ui::Button::Primary,
                    cx,
                    move |pane, _, cx| pane.change(action.clone(), cx),
                );
                return menu
                    .when(automatic, |el| {
                        el.child(
                            ui::text(div().text_color(c.muted), &t.text.meta).child(
                                "Future messages on an Auto direction skip review. Already-waiting messages stay in review.",
                            ),
                        )
                    })
                    .child(buttons.child(apply).child(cancel));
            }
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
            .w(px(t.size.connections_rail * 2.0))
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
        let button = self.live_button(
            id,
            if self.copied { "Copied" } else { label },
            ui::Button::Secondary,
            cx,
            move |p, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(local.clone()));
                p.copied = true;
                cx.notify();
            },
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

    /// On a narrow tile, a thread without its header still offers the way back.
    fn back_bar(&self, cx: &Context<Self>) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        self.compact.then(|| {
            div()
                .flex()
                .flex_shrink_0()
                .px(px(t.space.s12))
                .py(px(t.space.s8))
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .child(self.back_button(cx))
        })
    }

    fn back_button(&self, cx: &Context<Self>) -> gpui::Stateful<Div> {
        ui::back_button("peer-back", "All")
            .focusable()
            .tab_stop(true)
            .track_focus(&self.entry)
            .on_click(cx.listener(|p, _, _, cx| p.pick(None, cx)))
    }

    /// The thread's top line: a way back on a narrow tile, the peer's mark
    /// and name, its state, and any controls.
    fn live_header(&self, name: &str, state: Option<Div>, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s8))
            .px(px(t.space.s12))
            .py(px(t.space.s8))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .when(self.compact, |el| el.child(self.back_button(cx)))
            .child(ui::profile_mark(name, t.size.view_tile))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(t.text.row_selected.weight())
                    .child(name.to_owned()),
            )
            .children(state)
    }

    fn live_thread(&self, snapshot: &Snapshot, actions: bool, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let column = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        let body = div()
            .id("peer-history")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s10));
        let meta = |text: String| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        // Without a peer header, a narrow tile still needs its way back.
        let empty = |title: &'static str, body: &'static str| {
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .children(self.back_bar(cx))
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
                    .child(self.live_header(&name, Some(state), cx))
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
                let name = peer.display_name.as_deref().unwrap_or(&id.peer_id);
                let gates = peer.gates;
                let gate_label = format!("↙ {:?} · ↗ {:?}", gates.incoming, gates.outgoing);
                let connected = peer.state == "connected";
                let state = if connected {
                    ui::tag("Connected", ui::Tone::Done)
                } else if peer.state == "revocation_pending" {
                    ui::tag(link_status(&peer.state), ui::Tone::Attention)
                } else {
                    ui::tag(link_status(&peer.state), ui::Tone::Quiet)
                };
                let mut controls = div()
                    .ml_auto()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(t.space.s6));
                let editable =
                    gates.incoming != Policy::Unknown || gates.outgoing != Policy::Unknown;
                if actions && connected && editable {
                    let generation = id.generation.clone();
                    controls = controls.child(self.live_button(
                        "peer-gates",
                        format!("{gate_label} ⌄"),
                        ui::Button::Secondary,
                        cx,
                        move |p, w, cx| {
                            p.show_form(Form::Gate(generation.clone(), gates, gates), w, cx)
                        },
                    ));
                } else {
                    controls = controls.child(ui::tag(gate_label, ui::Tone::Quiet));
                }
                if actions && connected {
                    let disconnect = id.generation.clone();
                    controls = controls.child(self.live_button(
                        "peer-disconnect",
                        "Disconnect",
                        ui::Button::Danger,
                        cx,
                        move |p, w, cx| p.show_form(Form::Disconnect(disconnect.clone()), w, cx),
                    ));
                }
                let mut column =
                    column.child(self.live_header(name, Some(state), cx).child(controls));
                // An open form stays usable while only a read is in flight: the
                // service checks each change, and the next read reconciles it.
                let held = connected
                    && self.live.stream_ok
                    && !self.live.sending
                    && self.live.status.is_none()
                    && self.live.retry.is_none();
                if held && matches!(self.live.form, Some(Form::Gate(..) | Form::Disconnect(_))) {
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
                // A self-reported name never hides the identity it stands for.
                let mut body = body
                    .children(
                        peer.display_name
                            .is_some()
                            .then(|| ui::mono(id.peer_id.clone()).text_color(c.muted)),
                    )
                    .child(meta(format!(
                        "Left: {name} (external · available to main) · Right: main"
                    )));
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
                    body = body.child(meta("No messages yet.".into()));
                }
                let tint = [c.info, c.violet, c.green, c.accent][id
                    .peer_id
                    .bytes()
                    .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
                    % 4];
                for m in &history.items {
                    body =
                        body.child(self.live_card(name, tint, m, actions && connected, held, cx));
                }
                column.child(body)
            }
        }
    }

    /// One message: the peer's on the left with its tint, main's on the right,
    /// and a dashed outline while it waits for review.
    fn live_card(
        &self,
        name: &str,
        tint: gpui::Rgba,
        m: &api::Message,
        actions: bool,
        held: bool,
        cx: &Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let outgoing = m.direction == Direction::Outgoing;
        let waiting = m.state == "pending_review";
        let meta = |text: String| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let mut card = div()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .max_w(gpui::relative(if waiting { 0.88 } else { 0.82 }))
            .p(px(t.space.s10))
            .rounded(px(t.radius.card))
            .border(px(t.size.hairline))
            .bg(if outgoing && !waiting {
                c.outgoing_surface
            } else {
                c.panel
            })
            .border_color(if outgoing { c.outgoing_border } else { c.line })
            .when(outgoing, |el| el.self_end().rounded_tr(px(t.radius.key)))
            .when(!outgoing, |el| {
                el.self_start()
                    .rounded_tl(px(t.radius.key))
                    .border_l(px(t.space.s2))
                    .border_color(tint)
            })
            .when(waiting, |el| {
                el.border(px(t.size.hairline))
                    .border_dashed()
                    .border_color(c.attention)
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .gap(px(t.space.s12))
                    .child(
                        ui::mono(if outgoing {
                            "main".to_owned()
                        } else {
                            name.to_owned()
                        })
                        .text_color(if outgoing {
                            c.text_2
                        } else {
                            tint
                        }),
                    )
                    .child(
                        ui::text(
                            div().text_color(if waiting { c.attention_text } else { c.muted }),
                            &t.text.meta,
                        )
                        .child(if waiting {
                            format!("review · {}", clock(&m.submitted_at))
                        } else {
                            clock(&m.submitted_at)
                        }),
                    ),
            )
            .child(ui::text(div().text_color(c.text), &t.text.body).child(m.text.clone()));
        let receipt = match &m.receipt {
            _ if waiting => None,
            api::Receipt::Missing => Some(match m.state.as_str() {
                "received" => "Stored by peer · terminal receipt pending".into(),
                "uploaded" => "Uploaded to relay · terminal receipt pending".into(),
                "queued" => "Queued · terminal receipt pending".into(),
                "upload_unconfirmed" => "Upload unconfirmed".into(),
                "outcome_unknown" => "Delivery outcome unknown".into(),
                s => s.replace('_', " "),
            }),
            api::Receipt::Uncertain => Some("Delivery uncertain".into()),
            api::Receipt::Confirmed {
                outcome,
                recorded_at,
            } => Some(format!(
                "{} · receipt received {}",
                outcome.replace('_', " "),
                clock(recorded_at)
            )),
        };
        card = card.children(receipt.map(meta));
        if let Some(f) = &m.upload_failure {
            card = card.child(ui::mono(f.code.clone()).text_color(c.attention_text));
        }
        let ours = matches!(
            &self.live.form,
            Some(Form::Decline(x) | Form::Revise(x)) if x.message_id == m.message_id
        );
        if waiting && ours && held {
            return card.child(self.live_form(cx));
        }
        if !(waiting && actions) {
            return card;
        }
        let pass = m.clone();
        let decline = m.clone();
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap(px(t.space.s8))
            .child(self.live_button(
                format!("pass-{}", m.message_id),
                "Pass through",
                ui::Button::Primary,
                cx,
                move |p, _, cx| {
                    p.change(
                        Change::Review(Box::new(pass.clone()), DecisionKind::Pass),
                        cx,
                    )
                },
            ));
        if outgoing {
            let msg = m.clone();
            row = row.child(self.live_button(
                format!("revise-{}", m.message_id),
                "Ask to revise",
                ui::Button::Secondary,
                cx,
                move |p, w, cx| p.show_form(Form::Revise(msg.clone()), w, cx),
            ));
        }
        card.child(row.child(self.live_button(
            format!("decline-{}", m.message_id),
            "Decline",
            ui::Button::Danger,
            cx,
            move |p, w, cx| p.show_form(Form::Decline(decline.clone()), w, cx),
        )))
        .child(meta(if outgoing {
            format!("Pass through sends it unchanged to {name}.")
        } else {
            "Pass through makes it available to main and notifies it.".into()
        }))
    }

    /// The peer column for the service's invitations and connections.
    fn live_rail(&self, s: &Snapshot, cx: &Context<Self>) -> Div {
        let c = colors();
        let mut rows = div()
            .id("live-peers")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll();
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let mut shown = 0;
        let mut add = |rows: gpui::Stateful<Div>,
                       selection: Selection,
                       name: Option<&str>,
                       peer: &str,
                       (mark, label, color): (&'static str, String, gpui::Rgba),
                       needs: bool| {
            let name = name.unwrap_or(peer);
            if (self.needs_you && !needs)
                || !(name.to_lowercase().contains(&query) || peer.to_lowercase().contains(&query))
            {
                return rows;
            }
            shown += 1;
            let id = match &selection {
                Selection::Peer(g) => format!("peer-{}", g.generation),
                Selection::Request(r) => format!("invite-{r}"),
            };
            let selected = self.live.selected.as_ref() == Some(&selection);
            rows.child(
                peer_row(id, selected, name.to_owned(), mark, label, color)
                    .on_click(cx.listener(move |p, _, _, cx| p.pick(Some(selection.clone()), cx))),
            )
        };
        for r in &s.summary.overview.invitations {
            if r.state == "accepted" {
                continue;
            }
            let incoming = r.direction == Direction::Incoming;
            let status = match (incoming, r.state.as_str()) {
                (true, "pending") => ("!", "request".into(), c.attention_text),
                (false, "declined") => ("!", "declined".into(), c.attention_text),
                (false, "pending") => ("◷", "pending".into(), c.accent_text),
                (_, state) => ("·", state.replace('_', " "), c.muted),
            };
            let needs = status.0 == "!";
            rows = add(
                rows,
                Selection::Request(r.request_id.clone()),
                r.display_name.as_deref(),
                &r.peer_id,
                status,
                needs,
            );
        }
        for p in &s.summary.overview.connections {
            let count = s.summary.count(&p.connection.generation);
            let letter = |policy: Policy| match policy {
                Policy::Review => "R",
                Policy::Auto => "A",
                Policy::Unknown => "?",
            };
            let status = if count > 0 {
                ("!", format!("{count} review"), c.attention_text)
            } else if p.state == "connected" {
                (
                    "●",
                    format!("{}/{}", letter(p.gates.incoming), letter(p.gates.outgoing)),
                    c.ok,
                )
            } else if p.state == "revocation_pending" {
                ("⊘", "stopped".into(), c.muted)
            } else {
                ("○", p.state.replace('_', " "), c.muted)
            };
            rows = add(
                rows,
                Selection::Peer(p.connection.clone()),
                p.display_name.as_deref(),
                &p.connection.peer_id,
                status,
                count > 0,
            );
        }
        if shown == 0 {
            rows = rows.child(no_rows("No matching peers"));
        }
        self.rail_frame(rows, true, cx, |p, w, cx| p.show_form(Form::Connect, w, cx))
    }

    pub(super) fn live_view(&self, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut view = div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col();
        let entity = cx.entity().downgrade();
        view = view.child(
            canvas(
                move |bounds, _, cx| {
                    let compact = f32::from(bounds.size.width) < tokens().size.connections_split;
                    let _ = entity.update(cx, |p, cx| {
                        if p.compact != compact {
                            p.compact = compact;
                            cx.notify();
                        }
                    });
                },
                |_, (), _, _| (),
            )
            .absolute()
            .size_full(),
        );
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
        let Some(s) = &self.live.snapshot else {
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
        let actions = self.live.stream_ok
            && !self.live.reading
            && !self.live.sending
            && self.live.status.is_none()
            && self.live.retry.is_none();
        let local = s.summary.overview.local_id.clone();
        let overview = &s.summary.overview;
        // An invitation waits only for the service, not for a read in flight.
        let can_send = self.live.stream_ok
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
            if !self.compact || !self.show_thread {
                main = main.child(self.live_rail(s, cx));
            }
            if !self.compact || self.show_thread {
                main = main.child(self.live_thread(s, actions, cx));
            }
        }
        if matches!(self.live.form, Some(Form::Connect)) {
            main = main.child(self.connect_panel(&local, can_send, cx));
        }
        view = view.child(main);
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
        let pending = s.summary.overview.pending_reviews;
        let mut footer = div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(t.space.s8))
            .px(px(t.space.s12))
            .py(px(t.space.s6))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .child(
                ui::text(div().min_w_0().text_color(c.muted), &t.text.meta).child(format!(
                    "Agent-to-agent via Agent Backbone · read {}{}",
                    self.live.read_at.as_deref().unwrap_or("—"),
                    if pending > 0 {
                        format!(" · {pending} waiting for review")
                    } else {
                        String::new()
                    }
                )),
            );
        if let Some((next, name, _)) = s
            .summary
            .attention_rows()
            .into_iter()
            .find(|(id, _, _)| Some(id) != self.live.selected.as_ref())
        {
            footer = footer.child(
                ui::text(
                    div()
                        .id("next-waiting")
                        .focusable()
                        .tab_stop(true)
                        .cursor_pointer()
                        .text_color(c.accent_text)
                        .hover(|el| el.underline()),
                    &t.text.meta,
                )
                .child(format!("Next waiting: {name} →"))
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

/// A change's repeat key: Agent Backbone takes only a UUIDv7 in 32 hex digits
/// and refuses any other request ID before routing it.
fn request_key() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// A service timestamp as local HH:MM, or as sent when it does not parse.
fn clock(stamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| stamp.to_owned())
}

// Local revocation has stopped delivery; the peer has not yet acknowledged it.
fn link_status(state: &str) -> String {
    match state {
        "revocation_pending" => "Stopped here, remote confirmation pending".into(),
        s => s.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let state = |key: &str| {
            serde_json::from_value::<api::Peer>(actions[key].clone())
                .unwrap()
                .state
        };
        assert_eq!(
            link_status(&state("disconnect_requested")),
            "Stopped here, remote confirmation pending"
        );
        assert_eq!(link_status(&state("disconnect_confirmed")), "disconnected");
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
                }],
            })),
            selection: Some(Selection::Peer(generation)),
            history: None,
        };
        assert!(!Change::Answer("req_1".into(), true).still_pending(&accepted));
        assert!(!Change::Disconnect("gen_1".into()).still_pending(&accepted));
    }
}
