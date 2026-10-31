//! Recorded interactions: list, row-aligned ledger, immutable reader and a
//! separate conversation. This entity owns selection and polls only while shown.

use super::{
    conversation::Conversation,
    selection::{self, Selectable, Selection},
};
use crate::desktop::name::NameField;
use agent_interface_roster::{
    interactions::{self as api, Actor, Error, Interaction, Snapshot},
    views::{epoch_seconds, now},
};
use gpui::{Context, Entity, ScrollHandle, Subscription, Task, Window, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

mod layout;
mod ledger;
mod observations;
mod reader;
mod render;
mod window;
use window::{Preset, TimeWindow};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Both,
    Sent,
    Received,
}

pub(super) struct Interactions {
    directory: PathBuf,
    visible: Rc<Cell<bool>>,
    shown: bool,
    snapshot: Option<Snapshot>,
    provider: Option<String>,
    error: Option<Error>,
    pending: Option<(Snapshot, TimeWindow, usize)>,
    reading: bool,
    dirty: bool,
    generation: u64,
    window: TimeWindow,
    agent: Option<String>,
    card: Option<String>,
    selected: Option<Interaction>,
    detail_error: Option<Error>,
    link_error: Option<&'static str>,
    link_generation: u64,
    detail_generation: u64,
    detail_reading: bool,
    detail_dirty: Option<String>,
    direction: Direction,
    peer: Option<String>,
    peer_menu: bool,
    menu_at: Option<gpui::Point<gpui::Pixels>>,
    origin: Rc<Cell<gpui::Point<gpui::Pixels>>>,
    expanded: HashSet<String>,
    back: Vec<(String, Option<Interaction>)>,
    scroll: ScrollHandle,
    reveal: bool,
    width: Rc<Cell<f32>>,
    custom: bool,
    custom_start: Entity<NameField>,
    custom_end: Entity<NameField>,
    custom_error: Option<&'static str>,
    technical: bool,
    hovered: Option<String>,
    ask_after_read: Option<String>,
    selection: RefCell<Selection>,
    conversations: HashMap<String, Entity<Conversation>>,
    conversation: Option<Entity<Conversation>>,
    _poll: Task<()>,
    subscriptions: Vec<Subscription>,
}

pub(super) enum Event {
    Activity,
    Report {
        id: u64,
        response: bool,
        request: u64,
        provider: String,
        anchor: String,
    },
}
impl gpui::EventEmitter<Event> for Interactions {}

impl Selectable for Interactions {
    fn selection(&self) -> &RefCell<Selection> {
        &self.selection
    }
}

fn epoch(text: &str) -> f64 {
    epoch_seconds(text).unwrap_or(0.)
}
fn stamp(at: f64) -> String {
    chrono::DateTime::from_timestamp(at as i64, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}
fn actor_name(actor: &Actor) -> &str {
    match actor.kind.as_str() {
        "owner" => "You",
        "unknown" => "sender not recorded",
        _ => &actor.name,
    }
}
fn actor_key(actor: &Actor) -> String {
    actor
        .id
        .clone()
        .unwrap_or_else(|| format!("{}:{}", actor.kind, actor.name_at_capture))
}
fn message_time(item: &Interaction) -> f64 {
    item.conversation_entry.as_ref().map_or_else(
        || epoch(&item.accepted_at),
        |entry| epoch(&entry.created_at),
    )
}
fn outgoing(item: &Interaction, agent: &str) -> bool {
    item.sender.id.as_deref() == Some(agent)
}
fn counterparty<'a>(item: &'a Interaction, agent: &str) -> &'a Actor {
    if outgoing(item, agent) {
        &item.recipient
    } else {
        &item.sender
    }
}

impl Interactions {
    pub fn new(
        directory: PathBuf,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let period = TimeWindow::new(Preset::SevenHours, now() as f64);
        let custom_start = cx.new(|cx| {
            NameField::new(
                &super::clock::Local::at(period.start)
                    .map(|t| t.input())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        let custom_end = cx.new(|cx| {
            NameField::new(
                &super::clock::Local::at(period.end)
                    .map(|t| t.input())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| {
                        if this.shown && this.visible.get() {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(20))
                    .await;
            }
        });
        Self {
            directory,
            visible,
            shown: true,
            snapshot: None,
            provider: None,
            error: None,
            pending: None,
            reading: false,
            dirty: false,
            generation: 0,
            window: period,
            agent: None,
            card: None,
            selected: None,
            detail_error: None,
            link_error: None,
            link_generation: 0,
            detail_generation: 0,
            detail_reading: false,
            detail_dirty: None,
            direction: Direction::Both,
            peer: None,
            peer_menu: false,
            menu_at: None,
            origin: Rc::new(Cell::new(gpui::point(gpui::px(0.), gpui::px(0.)))),
            expanded: HashSet::new(),
            back: vec![],
            scroll: ScrollHandle::new(),
            reveal: false,
            width: Rc::new(Cell::new(0.)),
            custom: false,
            custom_start,
            custom_end,
            custom_error: None,
            technical: false,
            hovered: None,
            ask_after_read: None,
            selection: RefCell::default(),
            conversations: HashMap::new(),
            conversation: None,
            _poll: poll,
            subscriptions: vec![],
        }
    }

    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        let returning = shown && !self.shown;
        self.shown = shown;
        if let Some(conversation) = &self.conversation {
            conversation.update(cx, |c, cx| c.set_active(shown, cx));
        }
        if returning {
            self.refresh(cx);
        }
    }

    fn open_report(&mut self, id: u64, response: bool, cx: &mut Context<Self>) {
        let (Some(provider), Some(selected)) = (&self.provider, &self.selected) else {
            return;
        };
        self.link_generation += 1;
        self.link_error = None;
        cx.emit(Event::Report {
            id,
            response,
            request: self.link_generation,
            provider: provider.clone(),
            anchor: selected.id.clone(),
        });
    }

    pub fn link_failed(&mut self, error: &'static str, cx: &mut Context<Self>) {
        self.link_error = Some(error);
        cx.notify();
    }

    pub fn is_selected(&self, provider: &str, id: &str, request: u64) -> bool {
        if request != self.link_generation {
            return false;
        }
        self.provider.as_deref() == Some(provider)
            && self.selected.as_ref().is_some_and(|m| m.id == id)
    }

    fn open_discussion(&mut self, id: String, cx: &mut Context<Self>) {
        self.choose(id.clone(), cx);
        self.ask_after_read = Some(id);
    }

    pub fn is_open(&self) -> bool {
        self.agent.is_some()
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.link_generation += 1;
        self.snapshot = None;
        self.pending = None;
        self.error = None;
        self.dirty = true;
        self.refresh(cx);
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        self.dirty = false;
        let generation = self.generation;
        let agent = self.agent.clone();
        let requested = if self.snapshot.is_some() {
            self.window.at(now() as f64)
        } else {
            self.window
        };
        let frozen = self
            .snapshot
            .as_ref()
            .map(|snapshot| (self.window, message_ids(snapshot)));
        let directory = self.directory.clone();
        let worker = cx.background_executor().spawn(async move {
            let read = |period: TimeWindow| {
                api::read(
                    &directory,
                    &stamp(period.start),
                    &stamp(period.end),
                    agent.as_deref(),
                )
            };
            let latest = read(requested)?;
            if let Some((frozen, ids)) = frozen
                && frozen != requested
                && message_ids(&latest).iter().all(|id| ids.contains(id))
            {
                // Refresh delivery/state evidence for the displayed window.
                // Merely aging out cannot remove rows or move the reading position.
                return read(frozen).map(|snapshot| (snapshot, frozen));
            }
            Ok((latest, requested))
        });
        cx.spawn(async move |this, cx| {
            let result = worker.await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                if generation != this.generation {
                    if this.shown && this.visible.get() {
                        this.refresh(cx);
                    }
                    return;
                }
                match result {
                    Ok((mut fresh, requested)) => {
                        if this
                            .provider
                            .as_ref()
                            .is_some_and(|provider| *provider != fresh.provider)
                        {
                            this.error = Some(Error::Unavailable);
                            cx.notify();
                            return;
                        }
                        if fresh.state_error.is_some() {
                            fresh.states = this
                                .snapshot
                                .as_ref()
                                .and_then(|previous| previous.states.clone());
                        }
                        this.provider = Some(fresh.provider.clone());
                        let count = this.new_count(&fresh);
                        if this.snapshot.is_some() && count > 0 {
                            this.pending = Some((fresh, requested, count));
                        } else {
                            let updated = this
                                .selected
                                .as_ref()
                                .and_then(|selected| {
                                    fresh.history.as_ref()?.items.iter().find(|item| {
                                        item.id == selected.id
                                            && (item.last_event_at != selected.last_event_at
                                                || item.status != selected.status)
                                    })
                                })
                                .map(|item| item.id.clone());
                            this.snapshot = Some(fresh);
                            if let Some(id) = updated {
                                this.load_detail(id, cx);
                            }
                            if this.selected.is_none() {
                                this.window = requested;
                            }
                        }
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                if this.dirty && this.shown && this.visible.get() {
                    this.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new_count(&self, fresh: &Snapshot) -> usize {
        let Some(old) = &self.snapshot else {
            return 0;
        };
        if old.provider != fresh.provider {
            return 0;
        }
        let ids = message_ids(old);
        message_ids(fresh).difference(&ids).count()
    }

    fn accept_new(&mut self, cx: &mut Context<Self>) {
        if let Some((snapshot, window, _)) = self.pending.take() {
            let selected = self.selected.as_ref().map(|item| item.id.clone());
            self.snapshot = Some(snapshot);
            self.window = window;
            if let Some(id) = selected {
                self.load_detail(id, cx);
            }
            // Keep scroll and the opened record untouched.
            cx.notify();
        }
    }

    fn change_window(&mut self, preset: Preset, cx: &mut Context<Self>) {
        if preset == Preset::Custom {
            self.custom = !self.custom;
            self.peer_menu = false;
            cx.notify();
            return;
        }
        self.window = TimeWindow::new(preset, now() as f64);
        self.custom = false;
        self.changed(cx);
    }

    fn apply_custom(&mut self, cx: &mut Context<Self>) {
        let history = self
            .snapshot
            .as_ref()
            .and_then(|s| s.summary.coverage.retained_from.as_deref())
            .map(epoch);
        match TimeWindow::custom(
            self.custom_start.read(cx).value(),
            self.custom_end.read(cx).value(),
            history,
        ) {
            Ok(window) => {
                self.window = window;
                self.custom = false;
                self.custom_error = None;
                self.changed(cx);
            }
            Err(error) => {
                self.custom_error = Some(error);
                cx.notify();
            }
        }
    }

    fn open_agent(&mut self, id: String, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        self.detail_dirty = None;
        self.detail_error = None;
        self.agent = Some(id);
        self.selected = None;
        self.back.clear();
        self.direction = Direction::Both;
        self.peer = None;
        self.expanded.clear();
        self.scroll = ScrollHandle::new();
        self.close_conversation(cx);
        self.changed(cx);
    }

    fn close_conversation(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.conversation.take() {
            c.update(cx, |c, cx| c.set_active(false, cx));
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.width.get() < crate::desktop::design::tokens().size.interaction_split
            && self.selected.is_some()
        {
            self.detail_generation += 1;
            self.detail_dirty = None;
            self.detail_error = None;
            self.close_conversation(cx);
            self.selected = None;
            cx.notify();
            return;
        }
        self.back_agent(cx);
    }

    fn back_agent(&mut self, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        self.detail_dirty = None;
        self.detail_error = None;
        self.link_error = None;
        self.close_conversation(cx);
        self.scroll = ScrollHandle::new();
        if let Some((agent, selected)) = self.back.pop() {
            self.agent = Some(agent);
            self.selected = selected;
            self.reveal = true;
        } else {
            self.card = self.agent.take();
            self.selected = None;
        }
        self.direction = Direction::Both;
        self.peer = None;
        self.changed(cx);
    }

    fn discussion_open(&self, cx: &gpui::App) -> bool {
        self.conversation
            .as_ref()
            .is_some_and(|c| c.read(cx).panel_is_open())
    }

    fn narrow_reader(&self, cx: &gpui::App) -> bool {
        use crate::desktop::{components as ui, design::tokens};
        let t = tokens();
        let open = self.discussion_open(cx);
        let discussion = if open {
            ui::conversation_panel_width(self.width.get())
        } else if self.conversation.is_some()
            || self
                .selected
                .as_ref()
                .is_some_and(|item| item.conversation.supported)
        {
            t.size.conversation_rail
        } else {
            0.
        };
        // Reserve the rail, padding, direction/chevron, five column gaps,
        // fixed metadata and a readable preview the width of an actor column.
        let ledger = t.size.interaction_rail
            + t.size.interaction_gutter
            + 4. * t.space.s14
            + 2. * t.size.icon
            + 5. * t.space.s10
            + t.size.interaction_time
            + 2. * t.size.interaction_who
            + t.size.interaction_status;
        let reader = if open {
            t.size.interaction_reader_chat
        } else {
            t.size.interaction_reader
        };
        !layout::split_fits(
            self.width.get(),
            t.size.interaction_split,
            reader,
            discussion,
            ledger,
        )
    }

    fn spans(&self, agent: &str) -> Vec<observations::Span> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.states.as_ref())
            .map(|page| {
                page.items
                    .iter()
                    .filter(|span| span.agent_id == agent)
                    .map(|span| observations::Span {
                        start: epoch(&span.since),
                        end: epoch(&span.until),
                        state: crate::desktop::components::Observation::from_state(&span.state),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn items(&self) -> &[Interaction] {
        self.snapshot
            .as_ref()
            .and_then(|s| s.history.as_ref())
            .map(|p| p.items.as_slice())
            .unwrap_or_default()
    }

    fn filtered(&self) -> Vec<usize> {
        let Some(agent) = &self.agent else {
            return vec![];
        };
        let mut indexes: Vec<_> = self
            .items()
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                (self.direction == Direction::Both
                    || (self.direction == Direction::Sent) == outgoing(item, agent))
                    && self
                        .peer
                        .as_ref()
                        .is_none_or(|peer| *peer == actor_key(counterparty(item, agent)))
            })
            .map(|(i, _)| i)
            .collect();
        indexes.sort_by(|a, b| {
            epoch(&self.items()[*b].accepted_at)
                .total_cmp(&epoch(&self.items()[*a].accepted_at))
                .then(a.cmp(b))
        });
        indexes
    }

    fn choose(&mut self, id: String, cx: &mut Context<Self>) {
        self.link_generation += 1;
        self.selected = self.items().iter().find(|i| i.id == id).cloned();
        self.ask_after_read = None;
        self.link_error = None;
        self.close_conversation(cx);
        self.technical = false;
        self.detail_error = None;
        self.load_detail(id, cx);
        cx.notify();
    }

    fn load_detail(&mut self, id: String, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        if self.detail_reading {
            self.detail_dirty = Some(id);
            return;
        }
        let generation = self.detail_generation;
        let Some(provider) = self.provider.clone() else {
            return;
        };
        self.detail_reading = true;
        let directory = self.directory.clone();
        let worker = cx
            .background_executor()
            .spawn(async move { api::detail(&directory, &provider, &id) });
        cx.spawn(async move |this, cx| {
            let result = worker.await;
            let _ = this.update(cx, |this, cx| {
                this.detail_reading = false;
                if generation != this.detail_generation {
                    if let Some(id) = this.detail_dirty.take() {
                        this.load_detail(id, cx);
                    }
                    return;
                }
                match result {
                    Ok(item) => this.selected = Some(item),
                    Err(error) => this.detail_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn pivot(&mut self, cx: &mut Context<Self>) {
        let (Some(item), Some(agent)) = (&self.selected, &self.agent) else {
            return;
        };
        let peer = counterparty(item, agent);
        if peer.kind != "agent" {
            return;
        }
        let Some(id) = peer.id.clone() else {
            return;
        };
        self.back.push((agent.clone(), self.selected.clone()));
        self.agent = Some(id);
        self.scroll = ScrollHandle::new();
        self.direction = Direction::Both;
        self.peer = None;
        self.reveal = true;
        self.close_conversation(cx);
        self.changed(cx);
    }

    fn open_reference(
        &mut self,
        kind: agent_interface_roster::conversations::Kind,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.provider.clone() else {
            return;
        };
        let key = format!("{}\n{}\n{}", provider, kind.name(), id);
        let conversation = if let Some(c) = self.conversations.get(&key) {
            c.clone()
        } else {
            let target = super::conversation::ReferenceTarget { provider, kind, id };
            let c = cx.new(|cx| {
                Conversation::about_reference(
                    target,
                    self.directory.clone(),
                    self.visible.clone(),
                    window,
                    cx,
                )
            });
            self.subscriptions
                .push(cx.observe(&c, |_, _, cx| cx.notify()));
            self.conversations.insert(key, c.clone());
            c
        };
        self.close_conversation(cx);
        conversation.update(cx, |c, cx| {
            c.set_active(true, cx);
            c.open_panel(window, cx);
        });
        self.conversation = Some(conversation);
        cx.notify();
    }

    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(item), Some(snapshot)) = (self.selected.clone(), &self.snapshot) else {
            return;
        };
        if !item.conversation.supported {
            return;
        }
        let key = format!("{}\n{}", snapshot.provider, item.id);
        let directory = self.directory.clone();
        let provider = snapshot.provider.clone();
        let visible = self.visible.clone();
        let focus = self.agent.clone().unwrap_or_default();
        let conversation = if let Some(c) = self.conversations.get(&key) {
            c.clone()
        } else {
            let c = cx.new(|cx| {
                Conversation::about_interaction(
                    &item, &focus, provider, directory, visible, window, cx,
                )
            });
            self.subscriptions
                .push(cx.observe(&c, |_, _, cx| cx.notify()));
            self.conversations.insert(key, c.clone());
            c
        };
        self.close_conversation(cx);
        conversation.update(cx, |c, cx| {
            c.set_active(true, cx);
            c.open_panel(window, cx);
        });
        self.conversation = Some(conversation);
        cx.notify();
    }

    pub fn key(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.key == "escape" && (self.custom || self.peer_menu) {
            self.custom = false;
            self.peer_menu = false;
            cx.notify();
            return true;
        }
        if self.custom_start.read(cx).focus.is_focused(window)
            || self.custom_end.read(cx).focus.is_focused(window)
            || self
                .conversation
                .as_ref()
                .is_some_and(|c| c.read(cx).has_focus(window, cx))
        {
            return false;
        }
        if self.selection.borrow_mut().key(key, cx) {
            return true;
        }
        if key.modifiers.alt && !key.modifiers.control && !key.modifiers.platform {
            if let Ok(n) = key.key.parse::<usize>()
                && (1..=4).contains(&n)
            {
                self.change_window(Preset::ALL[n - 1], cx);
                return true;
            }
            if key.key == "enter" {
                self.ask(window, cx);
                return true;
            }
        }
        if key.modifiers.modified() {
            return false;
        }
        match key.key.as_str() {
            "escape" if self.custom || self.peer_menu => {
                self.custom = false;
                self.peer_menu = false;
                cx.notify();
            }
            "escape" => self.go_back(cx),
            "p" => self.pivot(cx),
            "up" | "down" | "[" | "]" => {
                let delta = if matches!(key.key.as_str(), "up" | "[") {
                    -1
                } else {
                    1
                };
                if self.agent.is_none() {
                    let agents = render::ordered(self);
                    let pos = agents.iter().position(|a| a.agent.id == self.card);
                    let n = step(pos, delta, agents.len());
                    self.card = agents.get(n).and_then(|a| a.agent.id.clone());
                    cx.notify();
                } else {
                    let ids: Vec<_> = self
                        .filtered()
                        .iter()
                        .map(|i| self.items()[*i].id.clone())
                        .collect();
                    let pos = ids
                        .iter()
                        .position(|id| self.selected.as_ref().is_some_and(|s| s.id == *id));
                    let n = step(pos, delta, ids.len());
                    if let Some(id) = ids.get(n) {
                        self.choose(id.clone(), cx);
                    }
                }
            }
            "enter" => {
                if let Some(id) = self.card.clone().filter(|_| self.agent.is_none()) {
                    self.open_agent(id, cx);
                } else if self.selected.is_none()
                    && let Some(index) = self.filtered().first()
                {
                    self.choose(self.items()[*index].id.clone(), cx);
                }
            }
            _ => return false,
        }
        true
    }
}

fn step(position: Option<usize>, delta: isize, count: usize) -> usize {
    position.map_or(0, |pos| {
        (pos as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize
    })
}

fn message_ids(snapshot: &Snapshot) -> HashSet<String> {
    if let Some(history) = &snapshot.history {
        history.items.iter().map(|item| item.id.clone()).collect()
    } else {
        snapshot
            .summary
            .items
            .iter()
            .flat_map(|agent| agent.ticks.iter().map(|tick| tick.interaction_id.clone()))
            .collect()
    }
}
