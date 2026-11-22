//! Owner-scoped data for the native Connections pane. Stream frames only invalidate reads.
use super::*;
use agent_interface_roster::profiles::{Connection, Error, peers as api};
use api::{Direction, Policy};
use futures::{StreamExt, channel::mpsc};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    Request(String),
    Peer(api::Generation),
}

#[derive(Clone)]
enum Change {
    Invite(String),
    Answer(String, bool),
    Gate(String, Direction, Policy),
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
    fn send(&self, connection: &Connection, key: &str) -> Result<(), Error> {
        match self {
            Self::Invite(id) => connection.invite_peer(id, key).map(drop),
            Self::Answer(id, accept) => connection.answer_peer(id, *accept, key).map(drop),
            Self::Gate(generation, dir, policy) => connection
                .set_peer_gate(generation, *dir, *policy, key)
                .map(drop),
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
    Gate(String, Direction, Policy),
}

struct Snapshot {
    overview: api::Overview,
    waiting: Vec<api::Review>,
    selection: Option<Selection>,
    history: Option<api::Page<api::Message>>,
}

impl Snapshot {
    fn read(c: &Connection, selected: Option<Selection>) -> Result<Self, Error> {
        let overview = c.peer_overview()?;
        let mut waiting = Vec::new();
        let mut cursors = HashSet::new();
        let mut before = None;
        if overview.pending_reviews > 0 {
            loop {
                let page = c.peer_reviews(before.as_deref())?;
                waiting.extend(page.items);
                let Some(cursor) = page.older else {
                    break;
                };
                if !cursors.insert(cursor.clone()) {
                    return Err(Error::Invalid);
                }
                before = Some(cursor);
            }
        }
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
            Some(Selection::Peer(id)) => Some(c.peer_history(id, None)?),
            _ => None,
        };
        Ok(Self {
            overview,
            waiting,
            selection,
            history,
        })
    }
}

pub(super) struct Live {
    connection: Connection,
    selected: Option<Selection>,
    snapshot: Option<Snapshot>,
    stream_ok: bool,
    reading: bool,
    dirty: bool,
    sending: bool,
    retry: Option<(Change, String)>,
    status: Option<String>,
    read_at: Option<String>,
    older_loading: bool,
    pub(super) form: Option<Form>,
    alive: Arc<AtomicBool>,
    _events: gpui::Task<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl Live {
    pub(super) fn new(connection: Connection, cx: &mut Context<Pane>) -> Self {
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        let service = connection.clone();
        let (mut tx, mut rx) = mpsc::channel(1);
        let online = Arc::new(AtomicBool::new(false));
        let opened = online.clone();
        let worker = std::thread::Builder::new()
            .name("peer-connections".into())
            .spawn(move || {
                let mut cursor = None;
                let live = || running.load(Ordering::Acquire);
                while live() {
                    let _ = service.watch_peers(&mut cursor, &live, &mut || {
                        opened.store(true, Ordering::Release);
                        let _ = tx.try_send(());
                    });
                    if !live() {
                        break;
                    }
                    opened.store(false, Ordering::Release);
                    let _ = tx.try_send(());
                    for _ in 0..12 {
                        if !live() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            });
        let events = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                let ok = online.load(Ordering::Acquire);
                if this
                    .update(cx, |pane, cx| {
                        pane.live.stream_ok = ok;
                        if ok {
                            pane.refresh_live(cx);
                        } else {
                            pane.live.status = Some("Agent Backbone stream unavailable".into());
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
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
            status: worker.err().map(|e| e.to_string()),
            read_at: None,
            older_loading: false,
            form: None,
            alive,
            _events: events,
        }
    }
}

impl Pane {
    fn refresh_live(&mut self, cx: &mut Context<Self>) {
        if !self.live.stream_ok {
            return;
        }
        if self.live.reading {
            self.live.dirty = true;
            return;
        }
        let connection = self.live.connection.clone();
        let selected = self.live.selected.clone();
        self.live.reading = true;
        let expected = selected.clone();
        let read = cx
            .background_executor()
            .spawn(async move { Snapshot::read(&connection, selected) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                let live = &mut pane.live;
                live.reading = false;
                if live.selected == expected && live.stream_ok {
                    match result {
                        Ok(snapshot) => {
                            if live.selected != snapshot.selection {
                                live.form = None;
                            }
                            live.selected = snapshot.selection.clone();
                            live.snapshot = Some(snapshot);
                            live.read_at = Some(chrono::Local::now().format("%H:%M").to_string());
                            if live.retry.is_none() {
                                live.status = None;
                            }
                        }
                        Err(e) => live.status = Some(format!("Agent Backbone unavailable · {e:?}")),
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
        self.live.selected = target;
        self.live.form = None;
        self.show_thread = self.live.selected.is_some();
        self.copied = false;
        self.refresh_live(cx);
        cx.notify();
    }

    fn change(&mut self, action: Change, cx: &mut Context<Self>) {
        if self.live.sending
            || self.live.retry.is_some()
            || self.live.reading
            || self.live.status.is_some()
            || !self.live.stream_ok
        {
            return;
        }
        self.live.retry = Some((action, uuid::Uuid::new_v4().simple().to_string()));
        self.send_live(cx);
    }

    fn send_live(&mut self, cx: &mut Context<Self>) {
        if self.live.sending || !self.live.stream_ok {
            return;
        }
        let Some((change, key)) = self.live.retry.clone() else {
            return;
        };
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
                    }
                    Err(error) => {
                        // A lost reply may already have committed. Retry the same bytes and key.
                        if !matches!(error, Error::Unavailable | Error::Refused(500..=599)) {
                            pane.live.retry = None;
                        }
                        pane.live.status = Some(format!("Action failed · {error:?}"));
                    }
                }
                if result.is_ok() {
                    pane.refresh_live(cx);
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
            Some(Form::Connect) => self.change(Change::Invite(value), cx),
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

    fn live_form(&self, cx: &Context<Self>) -> Div {
        let base = div()
            .flex()
            .flex_col()
            .gap(px(tokens().space.s6))
            .p(px(tokens().space.s8))
            .bg(colors().panel);
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
        let (label, action) = match form {
            Form::Connect | Form::Revise(_) => {
                let (hint, send) = if matches!(form, Form::Connect) {
                    ("Peer connection ID", "Send")
                } else {
                    ("Revision note", "Send note to main")
                };
                return base.child(hint).child(self.input.clone()).child(
                    div()
                        .flex()
                        .child(self.live_button(
                            "live-submit",
                            send,
                            ui::Button::Primary,
                            cx,
                            |pane, _, cx| pane.submit_live_form(cx),
                        ))
                        .child(cancel),
                );
            }
            Form::Decline(message) => (
                "Confirm decline",
                Change::Review(Box::new(message.clone()), DecisionKind::Decline),
            ),
            Form::Disconnect(id) => ("Confirm disconnect", Change::Disconnect(id.clone())),
            Form::Gate(id, direction, policy) => {
                let mut menu = ui::menu().child(ui::menu_heading("Delivery gate"));
                for d in [Direction::Incoming, Direction::Outgoing] {
                    let mut row = div().flex().flex_wrap().child(format!("{d:?}"));
                    for p in [Policy::Review, Policy::Auto] {
                        let id = id.clone();
                        row = row.child(self.live_button(
                            format!("gate-{d:?}-{p:?}"),
                            format!("{p:?}"),
                            if d == *direction && p == *policy {
                                ui::Button::Primary
                            } else {
                                ui::Button::Secondary
                            },
                            cx,
                            move |pane, _, cx| {
                                pane.live.form = Some(Form::Gate(id.clone(), d, p));
                                cx.notify();
                            },
                        ));
                    }
                    menu = menu.child(row);
                }
                let action = Change::Gate(id.clone(), *direction, *policy);
                return menu.when(*policy==Policy::Auto,|el|el.child("Future messages on an Auto direction skip review. Already-waiting messages stay in review.")).child(self.live_button("apply-live-gate","Apply gate",ui::Button::Primary,cx,move|pane,_,cx|pane.change(action.clone(),cx))).child(cancel);
            }
        };
        base.child(self.live_button(
            "live-confirm",
            label,
            ui::Button::Danger,
            cx,
            move |pane, _, cx| pane.change(action.clone(), cx),
        ))
        .child(cancel)
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

    fn live_thread(&self, snapshot: &Snapshot, actions: bool, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut thread = div()
            .id("peer-history")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s8));
        if self.compact {
            thread = thread.child(self.live_button(
                "peer-back",
                "← All",
                ui::Button::Secondary,
                cx,
                |p, _, cx| p.pick(None, cx),
            ));
        }
        if self.live.selected != snapshot.selection {
            return div().child(thread.child("Reading conversation…"));
        }
        match &snapshot.selection {
            None => return div().child(thread.child("Choose a peer to see its conversation.")),
            Some(Selection::Request(id)) => {
                if let Some(r) = snapshot
                    .overview
                    .invitations
                    .iter()
                    .find(|r| &r.request_id == id)
                {
                    thread = thread
                        .child(r.display_name.clone().unwrap_or_else(|| r.peer_id.clone()))
                        .child(ui::mono(r.peer_id.clone()))
                        .child(format!("{:?} request · {}", r.direction, r.state));
                    if r.display_name.is_some() {
                        thread = thread.child("Name provided by requester");
                    }
                    if r.direction == Direction::Incoming && r.state == "pending" {
                        thread=thread.child("Accepting opens this agent-to-agent connection. Both directions start on Review.");
                        if actions {
                            for (accept, label) in [(true, "Accept"), (false, "Decline")] {
                                let id = id.clone();
                                thread = thread.child(self.live_button(
                                    format!("answer-{accept}"),
                                    label,
                                    if accept {
                                        ui::Button::Primary
                                    } else {
                                        ui::Button::Danger
                                    },
                                    cx,
                                    move |p, _, cx| {
                                        p.change(Change::Answer(id.clone(), accept), cx)
                                    },
                                ));
                            }
                        }
                    }
                }
            }
            Some(Selection::Peer(id)) => {
                let Some(peer) = snapshot
                    .overview
                    .connections
                    .iter()
                    .find(|p| &p.connection == id)
                else {
                    return div();
                };
                let name = peer.display_name.as_deref().unwrap_or(&id.peer_id);
                let gates = peer.gates;
                let header = format!("↙ {:?} · ↗ {:?}", gates.incoming, gates.outgoing);
                thread = thread
                    .child(name.to_owned())
                    .child(ui::mono(id.peer_id.clone()))
                    .child(format!("Left: {name} (external) · Right: main"));
                if actions && peer.state == "connected" {
                    let generation = id.generation.clone();
                    let disconnect = generation.clone();
                    thread = thread.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .child(self.live_button(
                                "peer-gates",
                                header,
                                ui::Button::Secondary,
                                cx,
                                move |p, w, cx| {
                                    p.show_form(
                                        Form::Gate(
                                            generation.clone(),
                                            Direction::Incoming,
                                            gates.incoming,
                                        ),
                                        w,
                                        cx,
                                    )
                                },
                            ))
                            .child(self.live_button(
                                "peer-disconnect",
                                "Disconnect",
                                ui::Button::Danger,
                                cx,
                                move |p, w, cx| {
                                    p.show_form(Form::Disconnect(disconnect.clone()), w, cx)
                                },
                            )),
                    );
                } else {
                    thread = thread.child(format!("{} · {header}", peer.state));
                }
                let Some(history) = &snapshot.history else {
                    return div().child(thread);
                };
                if history.older.is_some() {
                    thread = thread.child(self.live_button(
                        "older-history",
                        "Older messages",
                        ui::Button::Secondary,
                        cx,
                        |pane, _, cx| pane.live_older(cx),
                    ));
                }
                for m in &history.items {
                    let outgoing = m.direction == Direction::Outgoing;
                    let waiting = m.state == "pending_review";
                    let mut card = div()
                        .min_w_0()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s6))
                        .max_w(gpui::relative(if waiting { 0.88 } else { 0.82 }))
                        .p(px(t.space.s10))
                        .rounded(px(t.radius.card))
                        .border_1()
                        .border_color(if waiting { c.attention } else { c.line })
                        .bg(if outgoing {
                            c.outgoing_surface
                        } else {
                            c.panel
                        })
                        .when(outgoing, |e| e.self_end())
                        .when(!outgoing, |e| {
                            e.self_start().border_l(px(2.5)).border_color(c.info)
                        })
                        .child(format!(
                            "{} · {}",
                            if outgoing { "main" } else { name },
                            m.submitted_at
                        ))
                        .child(m.text.clone());
                    let receipt = match &m.receipt {
                        api::Receipt::Missing => m.state.replace('_', " "),
                        api::Receipt::Uncertain => "Delivery uncertain".into(),
                        api::Receipt::Confirmed {
                            outcome,
                            recorded_at,
                        } => format!("{outcome} · receipt received {recorded_at}"),
                    };
                    card = card.child(ui::mono(receipt));
                    if let Some(f) = &m.upload_failure {
                        card = card.child(ui::mono(f.code.clone()));
                    }
                    if waiting && actions && peer.state == "connected" {
                        let pass = m.clone();
                        let decline = m.clone();
                        let mut row = div().flex().flex_wrap().child(self.live_button(
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
                        card = card
                            .child(row.child(self.live_button(
                                format!("decline-{}", m.message_id),
                                "Decline",
                                ui::Button::Danger,
                                cx,
                                move |p, w, cx| p.show_form(Form::Decline(decline.clone()), w, cx),
                            )))
                            .child(if outgoing {
                                format!("Sends unchanged to {name}")
                            } else {
                                "Makes available & notifies main".into()
                            });
                    }
                    thread = thread.child(card);
                }
            }
        }
        div().min_h_0().min_w_0().flex_1().flex().child(thread)
    }

    pub(super) fn live_view(&self, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut view = div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6));
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
            |pane, _, cx| pane.refresh_live(cx),
        );
        let Some(s) = &self.live.snapshot else {
            return view
                .child(ui::empty_state(
                    ui::empty_tile("unplug", c.selected, c.muted),
                    if self.live.status.is_some() {
                        "Agent Backbone unavailable"
                    } else {
                        "Reading profile connections…"
                    },
                    self.live.status.clone().unwrap_or_default(),
                ))
                .child(refresh)
                .child(
                    self.live_button(
                        "preview-connections",
                        "Preview with sample data",
                        ui::Button::Secondary,
                        cx,
                        |pane, _, cx| pane.preview(true, cx),
                    )
                    .track_focus(&self.entry),
                );
        };
        let actions = self.live.stream_ok
            && !self.live.reading
            && !self.live.sending
            && self.live.status.is_none()
            && self.live.retry.is_none();
        let local = s.overview.local_id.clone();
        view = view.child(
            div()
                .p(px(t.space.s6))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(t.space.s6))
                .child(ui::mono(local.clone()))
                .child(
                    self.live_button(
                        "copy-local-id",
                        if self.copied { "Copied" } else { "Copy ID" },
                        ui::Button::Secondary,
                        cx,
                        move |p, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(local.clone()));
                            p.copied = true;
                            cx.notify();
                        },
                    )
                    .track_focus(&self.entry),
                )
                .when(actions, |el| {
                    el.child(self.live_button(
                        "invite-peer",
                        "Connect a profile",
                        ui::Button::Secondary,
                        cx,
                        |p, w, cx| p.show_form(Form::Connect, w, cx),
                    ))
                }),
        );
        if self.live.form.is_some() {
            view = view.child(self.live_form(cx));
        }
        let mut content = div().flex().flex_1().min_h_0().min_w_0();
        if !self.compact || !self.show_thread {
            let mut rail = div()
                .w(px(t.size.connections_rail))
                .max_w_full()
                .min_h_0()
                .p(px(t.space.s6))
                .flex()
                .flex_col()
                .gap(px(t.space.s4));
            let mut filters = div().flex();
            for (needs, label) in [(false, "All"), (true, "Needs you")] {
                filters = filters.child(self.live_button(
                    format!("needs-{needs}"),
                    label,
                    if self.needs_you == needs {
                        ui::Button::Primary
                    } else {
                        ui::Button::Secondary
                    },
                    cx,
                    move |p, _, cx| {
                        p.needs_you = needs;
                        cx.notify();
                    },
                ));
            }
            rail = rail.child(filters).child(self.filter.clone());
            let mut rows = div().id("live-peers").min_h_0().overflow_y_scroll();
            let query = self.filter.read(cx).value().trim().to_lowercase();
            let mut add = |selection: Selection,
                           name: Option<&str>,
                           peer: &str,
                           mark: String,
                           needs: bool| {
                let name = name.unwrap_or(peer);
                if (!self.needs_you || needs)
                    && (name.to_lowercase().contains(&query)
                        || peer.to_lowercase().contains(&query))
                {
                    let id = match &selection {
                        Selection::Peer(g) => format!("peer-{}", g.generation),
                        Selection::Request(r) => format!("invite-{r}"),
                    };
                    let row = ui::row(id, self.live.selected.as_ref() == Some(&selection))
                        .focusable()
                        .tab_stop(true)
                        .child(name.to_owned())
                        .child(ui::mono(mark))
                        .on_click(
                            cx.listener(move |p, _, _, cx| p.pick(Some(selection.clone()), cx)),
                        );
                    rows = std::mem::replace(&mut rows, div().id("live-peers")).child(row);
                }
            };
            for r in &s.overview.invitations {
                if r.state == "accepted" {
                    continue;
                }
                let needs = (r.direction == Direction::Incoming && r.state == "pending")
                    || (r.direction == Direction::Outgoing && r.state == "declined");
                add(
                    Selection::Request(r.request_id.clone()),
                    r.display_name.as_deref(),
                    &r.peer_id,
                    if needs {
                        format!(
                            "! {}",
                            if r.state == "pending" {
                                "request"
                            } else {
                                &r.state
                            }
                        )
                    } else {
                        r.state.clone()
                    },
                    needs,
                );
            }
            for p in &s.overview.connections {
                let count = s
                    .waiting
                    .iter()
                    .filter(|r| r.message.connection == p.connection)
                    .count();
                add(
                    Selection::Peer(p.connection.clone()),
                    p.display_name.as_deref(),
                    &p.connection.peer_id,
                    if count > 0 {
                        format!("! {count} review")
                    } else {
                        p.state.clone()
                    },
                    count > 0,
                );
            }
            content = content.child(rail.child(rows));
        }
        if !self.compact || self.show_thread {
            content = content.child(self.live_thread(s, actions, cx));
        }
        if s.overview.connections.is_empty() && s.overview.invitations.is_empty() {
            view = view.child("No profile connections yet");
        }
        view = view.child(content);
        if let Some(error) = &self.live.status {
            view = view.child(
                div()
                    .bg(c.attention_soft)
                    .flex()
                    .flex_wrap()
                    .child(error.clone())
                    .child(if self.live.retry.is_some() {
                        self.live_button(
                            "retry-action",
                            "Retry action",
                            ui::Button::Secondary,
                            cx,
                            |p, _, cx| p.send_live(cx),
                        )
                    } else {
                        refresh
                    }),
            );
        }
        view.child(ui::mono(format!(
            "Agent-to-agent via Agent Backbone · read {} · {} pending review",
            self.live.read_at.as_deref().unwrap_or("—"),
            s.overview.pending_reviews
        )))
    }
}
