//! Each agent's current plan in Reports & updates: one card per agent with
//! its steps on a track and what they wait on, and an opened plan drawn as
//! a graph with the selected step's detail under it. Plans are the agents'
//! declared intent; whether an agent is working comes from its reported
//! state, not the plan. This view only reads them.

use std::{cell::Cell, collections::BTreeMap, path::PathBuf, rc::Rc, time::Duration};

use agent_interface_roster::{
    conversations,
    plans::{self, Entry, Error, Item, PlansPage},
    views::Member,
};
use gpui::{AnyElement, Context, Entity, SharedString, Task, Window, div, prelude::*, px};

use super::{ViewPane, reports, selection};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// How often an open list rereads the plans.
const REFRESH: Duration = Duration::from_secs(20);

pub(super) struct Plans {
    directory: PathBuf,
    frame: Frame,
    /// Reads are numbered; a result older than one applied is dropped.
    asked: u64,
    applied: u64,
    /// A read is in flight; another waits for it rather than pile up.
    reading: bool,
    /// Whether this section is selected and its pane visible; it reads only then.
    shown: bool,
    visible: Rc<Cell<bool>>,
    _poll: Task<()>,
}

/// What the section shows, copied for each frame it draws.
#[derive(Clone, Default)]
struct Frame {
    /// The plans last read, kept when a later read fails.
    page: Option<PlansPage>,
    provider: Option<String>,
    /// Drafts and unresolved deliveries keep their observed plans accessible.
    held: bool,
    /// Each agent's runtime and reported state, by name, from the same read.
    members: BTreeMap<String, Member>,
    /// Why the latest read failed, shown beside the plans last read.
    error: Option<Error>,
    /// The card the keyboard is on, by author.
    selected: Option<String>,
    /// The opened plan, by author, and the step selected in it.
    open: Option<String>,
    step: Option<String>,
    /// The step whose detail shows under its card, by author and step.
    card_step: Option<(String, String)>,
}

impl Plans {
    pub fn new(directory: PathBuf, visible: Rc<Cell<bool>>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |plans, cx| plans.poll(cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        Self {
            directory,
            frame: Frame::default(),
            asked: 0,
            applied: 0,
            reading: false,
            shown: true,
            visible,
            _poll: poll,
        }
    }

    /// Whether the section is selected; leaving it pauses the reads.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        let returning = shown && !self.shown;
        self.shown = shown;
        if returning {
            self.refresh(cx);
        }
    }

    /// Whether a plan is open rather than the list.
    pub fn is_open(&self) -> bool {
        // As the section draws it: a plan that has since gone shows the list.
        self.frame.opened().is_some()
    }

    pub fn conversation(&self) -> Option<ConversationTarget> {
        self.frame.conversation()
    }

    pub fn hold_refresh(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.frame.held != held {
            self.frame.held = held;
            if !held && self.shown && self.visible.get() {
                self.refresh(cx);
            }
            cx.notify();
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.shown && self.visible.get() {
            self.refresh(cx);
        }
    }

    fn receive(
        &mut self,
        number: u64,
        read: Result<(PlansPage, BTreeMap<String, Member>, String), Error>,
        cx: &mut Context<Self>,
    ) {
        if number == self.asked {
            self.reading = false;
        }
        if number <= self.applied || self.frame.held {
            return;
        }
        self.applied = number;
        match read {
            Ok((page, members, provider)) => {
                self.frame.provider = Some(provider);
                self.frame.page = Some(page);
                self.frame.members = members;
                self.frame.error = None;
            }
            Err(error) => self.frame.error = Some(error),
        }
        cx.notify();
    }

    /// Read the plans now, as well as on the regular refresh.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading || self.frame.held {
            return;
        }
        self.reading = true;
        self.asked += 1;
        let number = self.asked;
        let directory = self.directory.clone();
        let read = cx.background_executor().spawn(async move {
            let provider = conversations::provider(&directory).ok_or(Error::Unavailable)?;
            let page = plans::list_all(&directory)?;
            if conversations::provider(&directory).as_ref() != Some(&provider) {
                return Err(Error::Unavailable);
            }
            Ok((
                page,
                agent_interface_roster::views::members(&directory).ok_or(Error::Unavailable)?,
                provider,
            ))
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |plans, cx| plans.receive(number, read, cx));
        })
        .detach();
    }

    /// Open `author`'s plan; an agent without one stays on the list.
    fn open(&mut self, author: String, cx: &mut Context<Self>) {
        if self.frame.open(author) {
            cx.notify();
        }
    }

    /// Show `step`'s detail under `author`'s card, or hide it when shown.
    fn choose_card_step(&mut self, author: String, step: String, cx: &mut Context<Self>) {
        let frame = &mut self.frame;
        let chosen = (author.clone(), step);
        frame.card_step = (frame.card_step.as_ref() != Some(&chosen)).then_some(chosen);
        frame.selected = Some(author);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.frame.open = None;
        self.frame.step = None;
        cx.notify();
    }

    /// Move the keyboard `by` cards on the list as `agent` filters it.
    fn move_card(&mut self, by: isize, agent: Option<&str>, cx: &mut Context<Self>) {
        let frame = &mut self.frame;
        let authors: Vec<String> = frame
            .entries(agent)
            .iter()
            .map(|entry| entry.author_id.clone())
            .collect();
        if authors.is_empty() {
            return;
        }
        let at = frame
            .selected
            .as_ref()
            .and_then(|selected| authors.iter().position(|author| author == selected));
        let next = match at {
            Some(at) => (at as isize + by).clamp(0, authors.len() as isize - 1) as usize,
            None => 0,
        };
        frame.selected = Some(authors[next].clone());
        cx.notify();
    }

    /// Move the selection `by` steps in the opened plan.
    fn move_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let frame = &mut self.frame;
        let Some(items) = frame
            .opened()
            .and_then(|entry| entry.record.as_ref())
            .map(|record| &record.plan.items)
        else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let at = frame
            .step
            .as_ref()
            .and_then(|step| items.iter().position(|item| &item.item_id == step));
        let next = match at {
            Some(at) => (at as isize + by).clamp(0, items.len() as isize - 1) as usize,
            None => 0,
        };
        let next = items[next].item_id.clone();
        frame.step = Some(next);
        cx.notify();
    }
}

/// Immutable context for a draft and every retry of a sent message.
pub(super) struct ConversationTarget {
    pub provider: String,
    pub author: String,
    pub agent: String,
    pub anchor: conversations::Anchor,
}

impl ConversationTarget {
    pub fn key(&self) -> String {
        format!(
            "plan\n{}\n{}\n{}\n{}",
            self.provider,
            self.author,
            self.anchor.revision_id.as_deref().unwrap_or_default(),
            self.anchor.item_id.as_deref().unwrap_or_default()
        )
    }
}

impl Frame {
    fn open(&mut self, author: String) -> bool {
        let Some(entry) = self
            .page
            .as_ref()
            .and_then(|page| page.items.iter().find(|entry| entry.author_id == author))
        else {
            return false;
        };
        let Some(record) = &entry.record else {
            return false;
        };
        // A remembered card choice can disappear when its plan refreshes.
        let chosen = self
            .card_step
            .take()
            .filter(|(card, step)| {
                *card == author && record.plan.items.iter().any(|item| &item.item_id == step)
            })
            .map(|(_, step)| step);
        let step = chosen.or_else(|| At::of(&record.plan, self.state(&entry.agent_name)).step);
        self.selected = Some(author.clone());
        self.open = Some(author);
        self.step = step;
        true
    }

    fn conversation(&self) -> Option<ConversationTarget> {
        let entry = self.opened()?;
        let record = entry.record.as_ref()?;
        let item = self
            .step
            .as_ref()
            .and_then(|id| record.plan.items.iter().find(|item| &item.item_id == id));
        Some(ConversationTarget {
            provider: self.provider.clone()?,
            author: entry.author_id.clone(),
            agent: entry.agent_name.clone(),
            anchor: conversations::Anchor {
                revision_id: Some(record.revision_id.clone()),
                item_id: item.map(|item| item.item_id.clone()),
                label: Some(
                    item.map_or_else(|| record.plan.title.clone(), |item| item.text.clone()),
                ),
                ..Default::default()
            },
        })
    }

    /// The agents in list order, only `agent` when one is chosen: waiting on
    /// someone first, then the most recently updated, then those without a
    /// plan.
    fn entries(&self, agent: Option<&str>) -> Vec<&Entry> {
        let mut entries: Vec<&Entry> = self
            .page
            .as_ref()
            .map(|page| {
                page.items
                    .iter()
                    .filter(|entry| agent.is_none_or(|agent| agent == entry.agent_name))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|entry| match &entry.record {
            Some(record) => {
                let waiting = record.plan.blocked() || self.state(&entry.agent_name).needs_you();
                (u8::from(!waiting), record.age_seconds)
            }
            None => (2, u64::MAX),
        });
        entries
    }

    /// `agent`'s reported state; one the roster does not list is unknown.
    fn state(&self, agent: &str) -> ui::AgentState {
        self.members
            .get(agent)
            .map_or(ui::AgentState::Unknown, |member| {
                ui::AgentState::from_report(&member.state)
            })
    }

    fn opened(&self) -> Option<&Entry> {
        let open = self.open.as_deref()?;
        self.page
            .as_ref()?
            .items
            .iter()
            .find(|entry| entry.author_id == open && entry.record.is_some())
    }
}

/// Keys the section understands: ↑↓ and ↵ on the list; ←→ and esc in an
/// opened plan.
pub(super) fn key(
    plans: &Entity<Plans>,
    agent: Option<&str>,
    key: &gpui::Keystroke,
    cx: &mut Context<ViewPane>,
) -> bool {
    plans.update(cx, |plans, cx| {
        if plans.is_open() {
            match key.key.as_str() {
                "escape" => plans.close(cx),
                "left" | "up" => plans.move_step(-1, cx),
                "right" | "down" => plans.move_step(1, cx),
                _ => return false,
            }
            return true;
        }
        match key.key.as_str() {
            "up" => plans.move_card(-1, agent, cx),
            "down" => plans.move_card(1, agent, cx),
            "enter" => {
                // Only a card the filter shows.
                let shown = plans.frame.selected.clone().filter(|selected| {
                    plans
                        .frame
                        .entries(agent)
                        .iter()
                        .any(|entry| &entry.author_id == selected)
                });
                match shown {
                    Some(author) => plans.open(author, cx),
                    None => return false,
                }
            }
            _ => return false,
        }
        true
    })
}

/// Where an agent is in its plan: the step it works on while its reported
/// state says it is working, else the step it stopped at, the first waiting
/// one before any other. An agent whose state is unknown is not working.
struct At {
    step: Option<String>,
    working: bool,
}

impl At {
    fn of(plan: &plans::Plan, state: ui::AgentState) -> Self {
        let working = state.working();
        let step = if working {
            plan.current()
        } else {
            plan.items
                .iter()
                .find(|item| item.blocked())
                .or_else(|| plan.current())
        };
        Self {
            step: step.map(|item| item.item_id.clone()),
            working,
        }
    }

    fn is(&self, item: &Item) -> bool {
        self.step.as_deref() == Some(item.item_id.as_str())
    }

    /// Where the agent stopped, while it is not working.
    fn stopped_at(&self, item: &Item) -> bool {
        !self.working && self.is(item)
    }
}

/// A step's place in its plan, as [`ui::plan_step`] and [`ui::plan_node`]
/// show it: only a working agent's step turns.
fn step(item: &Item, at: &At) -> ui::Step {
    if item.done() {
        ui::Step::Done
    } else if item.blocked() {
        ui::Step::Waits
    } else if at.is(item) && at.working {
        ui::Step::Now
    } else if at.is(item) {
        ui::Step::Stopped
    } else {
        ui::Step::Ahead
    }
}

/// The agent's mark from its reported state; an agent at rest with every
/// step done shows the plan finished.
fn mark(plan: &plans::Plan, state: ui::AgentState) -> gpui::Div {
    let resting = !state.working() && !state.needs_you();
    if resting && plan.items.iter().all(Item::done) {
        ui::done_symbol()
    } else {
        ui::agent_symbol(state)
    }
}

fn runtime(plans: &Frame, agent: &str) -> Option<gpui::Div> {
    plans
        .members
        .get(agent)
        .filter(|member| !member.runtime.is_empty())
        .map(|member| ui::badge(member.runtime.clone()))
}

/// The first non-empty line of an agent's words, such as who or what a
/// step waits on.
fn first_line(text: &str) -> Option<SharedString> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| super::line(line).into())
}

/// A reference's target as a reader recognises it: an identifier by its
/// first characters, anything else as written.
fn short(target: &str) -> String {
    let id = target.len() > 12 && target.bytes().all(|b| b.is_ascii_hexdigit());
    if id {
        format!("{}…", &target[..8])
    } else {
        super::line(target)
    }
}

fn updated(age_seconds: u64) -> String {
    format!("updated {} ago", super::ago(age_seconds as f64))
}

/// One agent's card: its mark, name and runtime, the plan's title, the steps
/// on a track, the chosen step's detail and what the steps wait on.
fn card(pane: &ViewPane, plans: &Frame, entry: &Entry, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let author = entry.author_id.clone();
    let selected = plans.selected.as_deref() == Some(author.as_str());
    let id = SharedString::from(format!("plan-{author}"));
    let name = super::mono(entry.agent_name.clone());
    let Some(record) = &entry.record else {
        // No plan: one line, nothing to open.
        return ui::plan_card(id, selected)
            .flex_row()
            .items_center()
            .gap(px(t.space.s8))
            .py(px(t.space.s10))
            .cursor_default()
            .child(ui::agent_symbol(ui::AgentState::Idle))
            .child(ui::text(name.text_color(c.text_2), &t.text.body))
            .child(ui::text(div().text_color(c.muted), &t.text.status).child("No current plan"))
            .into_any_element();
    };
    let plan = &record.plan;
    let state = plans.state(&entry.agent_name);
    let at = At::of(plan, state);
    let chosen = plans
        .card_step
        .as_ref()
        .filter(|(card, _)| *card == author)
        .and_then(|(_, step)| plan.items.iter().find(|item| &item.item_id == step));
    let mut track = div()
        .flex()
        .items_center()
        .gap(px(t.space.s6))
        .w_full()
        .min_w_0();
    for (index, item) in plan.items.iter().enumerate() {
        if index > 0 {
            track = track.child(ui::plan_link(plan.items[index - 1].done()));
        }
        let picked = chosen.is_some_and(|chosen| chosen.item_id == item.item_id);
        let (author, step_id) = (author.clone(), item.item_id.clone());
        track = track.child(
            ui::plan_step(super::line(&item.text), step(item, &at), picked)
                .id(SharedString::from(format!("plan-{author}-step-{step_id}")))
                .on_click(cx.listener(move |pane, _, _, cx| {
                    // The step's detail, not the plan.
                    cx.stop_propagation();
                    if let Some(plans) = &pane.plans {
                        plans.update(cx, |plans, cx| {
                            plans.choose_card_step(author.clone(), step_id.clone(), cx)
                        });
                    }
                })),
        );
    }
    let waits = waits(plans, entry, plan, &at);
    let detail = chosen.map(|item| {
        div()
            .id(SharedString::from(format!("plan-{author}-detail")))
            .pt(px(t.space.s10))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .cursor_default()
            // Selecting its words does not open the plan.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(detail(pane, plans, entry, item, &at, cx))
    });
    ui::plan_card(id, selected)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .w_full()
                .child(mark(plan, state))
                .child(ui::text(name.text_color(c.text), &t.text.body))
                .children(runtime(plans, &entry.agent_name))
                .child(div().flex_1())
                .when(record.stale, |el| {
                    el.child(ui::tag("stale", ui::Tone::Quiet))
                })
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child(updated(record.age_seconds)),
                )
                .child(ui::icon("chevron-right", t.size.icon, c.faint)),
        )
        .child(
            ui::text(div().text_color(c.text), &t.text.card_title)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(super::line(&plan.title)),
        )
        .child(track)
        .children(detail)
        .child(waits)
        .on_click(cx.listener(move |pane, _, _, cx| {
            if let Some(plans) = &pane.plans {
                plans.update(cx, |plans, cx| plans.open(author.clone(), cx));
            }
        }))
        .into_any_element()
}

fn list(pane: &ViewPane, plans: &Frame, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let body = match (&plans.page, plans.error) {
        (None, None) => ui::empty_state(
            ui::empty_tile("activity", c.selected, c.muted),
            "Reading current plans…",
            "",
        )
        .into_any_element(),
        (None, Some(error)) => ui::empty_state(
            ui::empty_tile("unplug", c.selected, c.muted),
            "No current plans",
            error.message(),
        )
        .into_any_element(),
        (Some(_), _) => {
            let mut column = div()
                .id("plans")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(t.space.s10))
                .px(px(t.space.s20))
                .pt(px(t.space.s2))
                .pb(px(t.space.s16))
                .child(
                    ui::group_label(
                        div()
                            .text_color(c.text_2)
                            .font_weight(t.text.row_selected.weight())
                            .child("Now"),
                        "one plan per agent · waiting on you first",
                    )
                    .pb_0(),
                );
            for entry in plans.entries(pane.reports.agent.as_deref()) {
                column = column.child(card(pane, plans, entry, cx));
            }
            column.into_any_element()
        }
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(body)
        .child(ui::view_footer(
            &[("↑↓", "move"), ("↵", "open plan")],
            "each agent's current undertaking · replaced when a new one begins",
        ))
        .into_any_element()
}

/// What a node says above its step's name, by its place in the plan and
/// whether the agent stopped there.
fn key_label(step: ui::Step, first_ahead: bool, stopped: bool) -> &'static str {
    match step {
        ui::Step::Done => "DONE",
        ui::Step::Now => "NOW",
        ui::Step::Stopped => "STOPPED HERE",
        ui::Step::Waits if stopped => "STOPPED · WAITS",
        ui::Step::Waits => "WAITS",
        ui::Step::Ahead if first_ahead => "NEXT",
        ui::Step::Ahead => "THEN",
    }
}

/// What the steps wait on, under a card's track: the agent itself when its
/// state needs someone, then each blocked step with the agent's first words
/// about it, who or what it waits on.
fn waits(plans: &Frame, entry: &Entry, plan: &plans::Plan, at: &At) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let state = plans.state(&entry.agent_name);
    let mut tags = Vec::new();
    if state.needs_you() {
        let reason = plans
            .members
            .get(&entry.agent_name)
            .and_then(|member| member.reason.as_deref())
            .and_then(first_line);
        let held = plan
            .items
            .iter()
            .find(|item| at.is(item))
            .map_or_else(|| super::line(&plan.title), |item| super::line(&item.text));
        let you = state == ui::AgentState::NeedsYou;
        tags.push(ui::waits_tag(
            if you { "You" } else { "Blocked" },
            reason,
            held,
            you,
        ));
    }
    for item in plan.items.iter().filter(|item| item.blocked()) {
        tags.push(ui::waits_tag(
            "Waits",
            first_line(&item.detail),
            super::line(&item.text),
            false,
        ));
    }
    if tags.is_empty() {
        return ui::text(div().text_color(c.muted), &t.text.meta)
            .child("Nothing waiting · every step can continue");
    }
    div().flex().flex_wrap().gap(px(t.space.s6)).children(tags)
}

/// The opened plan's steps as a graph: one node per step in order, joined
/// by arrows, `muted` after a finished step and `faint` ahead.
fn graph(plans: &Frame, plan: &plans::Plan, at: &At, cx: &mut Context<ViewPane>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let pitch = t.size.plan_column;
    let width = t.size.plan_node_width;
    let height = t.size.plan_node_height;
    let count = plan.items.len().max(1) as f32;
    let mut canvas = div()
        .relative()
        .flex_shrink_0()
        .w(px(pitch * (count - 1.0) + width))
        .h(px(height + t.size.focus_ring * 2.0));
    let mut seen_ahead = false;
    for (index, item) in plan.items.iter().enumerate() {
        let x = pitch * index as f32;
        let place = step(item, at);
        let first_ahead = place == ui::Step::Ahead && !seen_ahead;
        seen_ahead |= place == ui::Step::Ahead;
        if index > 0 {
            let done = plan.items[index - 1].done();
            canvas = canvas.child(ui::plan_edge(
                x - pitch + width,
                t.size.focus_ring + height / 2.0,
                pitch - width,
                if done { c.muted } else { c.faint },
            ));
        }
        // A waiting step names what it waits on.
        let note = match place {
            ui::Step::Waits => first_line(&item.detail),
            ui::Step::Ahead if item.section == "later" => Some("later".into()),
            _ => None,
        };
        let id = item.item_id.clone();
        canvas = canvas.child(
            ui::plan_node(
                SharedString::from(format!("plan-step-{}", item.item_id)),
                place,
                key_label(place, first_ahead, at.stopped_at(item)),
                super::line(&item.text),
                note,
                plans.step.as_deref() == Some(item.item_id.as_str()),
            )
            .left(px(x))
            .top(px(t.size.focus_ring))
            .on_click(cx.listener(move |pane, _, _, cx| {
                if let Some(plans) = &pane.plans {
                    plans.update(cx, |plans, cx| {
                        plans.frame.step = Some(id.clone());
                        cx.notify();
                    });
                }
            })),
        );
    }
    canvas
}

/// The legend under the graph, and what selecting does.
fn legend() -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let entry = |swatch: gpui::Div, label: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .child(swatch)
            .child(label)
    };
    ui::text(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s18))
            .mt(px(t.space.s14))
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(entry(
        div().w(px(t.space.s18)).h(px(t.size.plan_edge)).bg(c.muted),
        "step order",
    ))
    .child(entry(
        div()
            .w(px(t.space.s12))
            .h(px(t.space.s10))
            .rounded(px(3.0))
            .border(px(t.size.hairline))
            .border_dashed()
            .border_color(c.attention)
            .bg(c.attention_soft),
        "blocked",
    ))
    .child(entry(
        div()
            .w(px(t.space.s12))
            .h(px(t.space.s10))
            .rounded(px(3.0))
            .border(px(t.size.hairline))
            .border_color(c.line_strong),
        "can continue",
    ))
}

/// Where a step stands, as its detail and the conversation beside it say.
fn place_label(item: &Item, at: &At) -> (&'static str, gpui::Rgba) {
    let c = colors();
    match step(item, at) {
        ui::Step::Waits if at.stopped_at(item) => ("STOPPED HERE · WAITS", c.attention_text),
        ui::Step::Waits => ("WAITS", c.attention_text),
        ui::Step::Now => ("NOW", c.accent_text),
        ui::Step::Stopped => ("STOPPED HERE", c.accent_text),
        ui::Step::Done => ("DONE", c.ok),
        ui::Step::Ahead => ("AHEAD", c.muted),
    }
}

/// A step's detail: where it stands, its name, the agent's words about it,
/// why the agent waits when it stopped there, and what it points to.
fn detail(
    pane: &ViewPane,
    plans: &Frame,
    entry: &Entry,
    item: &Item,
    at: &At,
    cx: &mut Context<ViewPane>,
) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let (label, color) = place_label(item, at);
    let mut column = div()
        .flex()
        .flex_col()
        .gap(px(t.space.s8))
        .child(ui::text(div().text_color(color), &t.text.label).child(label))
        .child(
            ui::text(div().text_color(c.text), &t.text.card_title).child(selection::text(
                pane,
                &super::line(&item.text),
                &[],
                cx,
            )),
        );
    if item.detail.trim().is_empty() {
        column = column.child(
            ui::text(div().text_color(c.muted), &t.text.body).child(format!(
                "{} gave no detail for this step.",
                entry.agent_name
            )),
        );
    } else {
        column = column.child(
            ui::text(div().text_color(c.text_2), &t.text.body)
                .line_height(px(t.text.body.size * 1.45))
                .child(selection::text(pane, item.detail.trim(), &[], cx)),
        );
    }
    let fact = |label: &'static str, value: AnyElement| {
        div()
            .flex()
            .items_start()
            .gap(px(t.space.s8))
            .child(
                div()
                    .w(px(t.size.fact_label / 2.0 + t.space.s8))
                    .flex_shrink_0()
                    .text_color(c.muted)
                    .child(label),
            )
            .child(div().min_w_0().child(value))
    };
    let mut facts = ui::text(div().flex().flex_col().gap(px(t.space.s6)), &t.text.status);
    let state = plans.state(&entry.agent_name);
    if at.stopped_at(item) && state.needs_you() {
        let reason = plans
            .members
            .get(&entry.agent_name)
            .and_then(|member| member.reason.as_deref())
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map_or_else(
                || state.word().to_owned(),
                |reason| format!("{} · {reason}", state.word()),
            );
        facts = facts.child(fact(
            "Agent",
            div()
                .text_color(c.attention_text)
                .child(selection::text(pane, &super::line(&reason), &[], cx))
                .into_any_element(),
        ));
    }
    facts = facts
        .child(fact(
            "Status",
            div()
                .text_color(c.text_2)
                .child(item.status.replace('_', " "))
                .into_any_element(),
        ))
        .child(fact(
            "Section",
            div()
                .text_color(c.text_2)
                .child(item.section.clone())
                .into_any_element(),
        ));
    for reference in &item.references {
        facts = facts.child(fact(
            "Evidence",
            ui::anchor(format!("{} · {}", reference.kind, short(&reference.target)))
                .into_any_element(),
        ));
    }
    column.child(facts)
}

/// The conversation beside an opened plan, headed by the step it is about.
fn step_panel(
    pane: &ViewPane,
    plans: &Frame,
    plan: &plans::Plan,
    at: &At,
    window: &mut Window,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let chosen = plans
        .step
        .as_ref()
        .and_then(|id| plan.items.iter().find(|item| &item.item_id == id));
    let mut context = div()
        .flex()
        .flex_col()
        .gap(px(t.space.s4))
        .px(px(t.space.s14))
        .py(px(t.space.s12))
        .border_b(px(t.size.hairline))
        .border_color(c.line);
    if let Some(item) = chosen {
        let (label, color) = place_label(item, at);
        context = context
            .child(ui::text(div().text_color(color), &t.text.label).child(label))
            .child(
                ui::text(div().text_color(c.text), &t.text.body)
                    .font_weight(t.text.workspace.weight())
                    .child(super::line(&item.text)),
            );
    }
    match &pane.conversation {
        Some(conversation) => conversation.update(cx, |conversation, cx| {
            conversation.panel(Some(context.into_any_element()), window, cx)
        }),
        None => ui::conversation_panel().child(context).into_any_element(),
    }
}

/// An opened plan: its agent, title and summary, the steps as a graph with
/// the selected step's detail under it, and the conversation beside them.
fn opened(
    pane: &ViewPane,
    plans: &Frame,
    entry: &Entry,
    window: &mut Window,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let record = entry.record.as_ref().expect("an opened plan has a record");
    let plan = &record.plan;
    let state = plans.state(&entry.agent_name);
    let at = At::of(plan, state);
    let done = plan.items.iter().filter(|item| item.done()).count();
    let waiting = plan.items.iter().filter(|item| item.blocked()).count();
    let ahead = plan
        .items
        .iter()
        .filter(|item| !item.done() && !item.blocked() && !item.in_progress())
        .count();
    let now = plan.items.iter().find(|item| at.is(item));
    let strong = |value: String, color| {
        div()
            .font_weight(t.text.workspace.weight())
            .text_color(color)
            .child(value)
    };
    let figure = |value: gpui::Div, after: &'static str| {
        div().flex().gap(px(t.space.s4)).child(value).child(after)
    };
    let mut summary = ui::text(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s14))
            .text_color(c.text_2),
        &t.text.body,
    )
    .child(figure(strong(done.to_string(), c.ok), "done"));
    if let Some(now) = now {
        let (said, color) = match (at.working, now.blocked()) {
            (true, _) => ("Now:", c.accent_text),
            (false, false) => ("Stopped at:", c.accent_text),
            (false, true) => ("Stopped at:", c.attention_text),
        };
        summary = summary.child(
            div()
                .flex()
                .min_w_0()
                .gap(px(t.space.s4))
                .child(said)
                .child(strong(super::line(&now.text), color).text_ellipsis()),
        );
    }
    summary = summary.child(figure(strong(ahead.to_string(), c.text_2), "ahead"));
    if waiting > 0 {
        summary = summary.child(figure(
            strong(waiting.to_string(), c.attention_text),
            "waiting",
        ));
    }
    let header = ui::text(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(mark(plan, state))
    .child(ui::text(
        super::mono(entry.agent_name.clone()).text_color(c.text),
        &t.text.body,
    ))
    .children(runtime(plans, &entry.agent_name))
    .child(format!("· current plan · {}", updated(record.age_seconds)));
    let main = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("plan-graph")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .pt(px(t.space.s18))
                .px(px(t.space.s28 + 7.0))
                .pb(px(t.space.s16))
                .child(header)
                .child(
                    ui::text(
                        div()
                            .mt(px(t.space.s8))
                            .mb(px(t.space.s6))
                            .text_color(c.text),
                        &t.text.doc_title,
                    )
                    .child(selection::text(
                        pane,
                        &super::line(&plan.title),
                        &[],
                        cx,
                    )),
                )
                .child(summary)
                .child(
                    div()
                        .id("plan-steps")
                        .mt(px(t.space.s24 + 2.0))
                        .overflow_x_scroll()
                        .flex_shrink_0()
                        .child(graph(plans, plan, &at, cx)),
                )
                .child(legend())
                .children(
                    plans
                        .step
                        .as_ref()
                        .and_then(|id| plan.items.iter().find(|item| &item.item_id == id))
                        .map(|item| {
                            div()
                                .mt(px(t.space.s18))
                                .px(px(t.space.s14))
                                .py(px(t.space.s12))
                                .rounded(px(t.radius.popover))
                                .bg(c.panel)
                                .border(px(t.size.hairline))
                                .border_color(c.line)
                                .child(detail(pane, plans, entry, item, &at, cx))
                        }),
                ),
        )
        .child(ui::view_footer(
            &[("←→", "move between steps"), ("esc", "back")],
            format!("a snapshot · it changes as {} works", entry.agent_name),
        ));
    let toolbar = ui::view_toolbar()
        .child(
            ui::back_button("plans-back", "Current plans").on_click(cx.listener(
                |pane, _, _, cx| {
                    if let Some(plans) = &pane.plans {
                        plans.update(cx, |plans, cx| plans.close(cx));
                    }
                },
            )),
        )
        .child(div().flex_1())
        .child(
            ui::icon_button("plans-refresh", "refresh-cw").on_click(cx.listener(
                |pane, _, _, cx| {
                    if let Some(plans) = &pane.plans {
                        plans.update(cx, |plans, cx| plans.refresh(cx));
                    }
                },
            )),
        );
    reports::plain_frame(
        Some(toolbar),
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(main)
            .child(step_panel(pane, plans, plan, &at, window, cx)),
    )
}

/// The section: the list of plans, or the opened plan.
pub(super) fn body(
    pane: &ViewPane,
    plans: &Entity<Plans>,
    window: &mut Window,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let frame = plans.read(cx).frame.clone();
    let body = match frame.opened().cloned() {
        Some(entry) => opened(pane, &frame, &entry, window, cx),
        None => reports::section_frame(pane, list(pane, &frame, cx), cx),
    };
    // A failed refresh says so above the plans last read.
    let stale = frame
        .page
        .is_some()
        .then_some(frame.error)
        .flatten()
        .map(|error| {
            ui::notice(
                "unplug",
                format!("{} These are the plans last read.", error.message()),
                None,
            )
        });
    div()
        .flex()
        .flex_col()
        .size_full()
        .when(frame.held, |el| {
            el.child(ui::notice(
                "message-square",
                "Plan refresh is paused while you have a draft or a message to finish.",
                None,
            ))
        })
        .children(stale)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_refreshed_plan_discards_a_removed_card_step() {
        let page: PlansPage = serde_json::from_value(serde_json::json!({
            "items": [{"author_id": "author", "agent_name": "builder", "record": {
                "plan_id": "plan", "revision_id": "revision", "updated_at": "2026-10-28T10:00:00Z",
                "plan": {"title": "Build", "items": [
                    {"item_id": "old", "text": "Finished work", "section": "now", "status": "done"},
                    {"item_id": "wait", "text": "Check the build", "section": "now", "status": "blocked"}
                ]}
            }}], "next_after_author_id": null
        })).unwrap();
        let mut frame = Frame {
            page: Some(page),
            provider: Some("unix:/tmp/provider.sock".into()),
            card_step: Some(("author".into(), "old".into())),
            ..Default::default()
        };
        assert!(frame.open("author".into()));
        assert_eq!(frame.step.as_deref(), Some("old"));
        // The user returns to the list and selects the same card step. A
        // refreshed plan removes it before the card is opened again.
        frame.open = None;
        frame.card_step = Some(("author".into(), "old".into()));
        frame.page.as_mut().unwrap().items[0]
            .record
            .as_mut()
            .unwrap()
            .plan
            .items
            .remove(0);
        assert!(frame.open("author".into()));
        assert_eq!(frame.step.as_deref(), Some("wait"));
        assert_eq!(
            frame.conversation().unwrap().anchor.item_id.as_deref(),
            Some("wait")
        );
        assert!(frame.card_step.is_none());
    }

    #[test]
    fn step_conversation_keeps_its_observed_revision_when_the_plan_changes() {
        let page: PlansPage = serde_json::from_value(serde_json::json!({
            "items": [{"author_id": "author", "agent_name": "builder", "record": {
                "plan_id": "plan", "revision_id": "first", "updated_at": "2026-10-28T10:00:00Z",
                "plan": {"title": "Build", "items": [{"item_id": "step", "text": "Check the build",
                    "section": "now", "status": "in_progress"}]}
            }}], "next_after_author_id": null
        }))
        .unwrap();
        let mut frame = Frame {
            page: Some(page),
            provider: Some("unix:/tmp/provider.sock".into()),
            open: Some("author".into()),
            step: Some("step".into()),
            ..Default::default()
        };
        let observed = frame.conversation().unwrap();
        assert_eq!(observed.author, "author");
        assert_eq!(observed.anchor.item_id.as_deref(), Some("step"));
        assert_eq!(observed.anchor.label.as_deref(), Some("Check the build"));
        let record = frame.page.as_mut().unwrap().items[0]
            .record
            .as_mut()
            .unwrap();
        record.revision_id = "second".into();
        record.plan.items.clear();
        let refreshed = frame.conversation().unwrap();
        assert_eq!(observed.anchor.revision_id.as_deref(), Some("first"));
        assert_eq!(refreshed.anchor.revision_id.as_deref(), Some("second"));
        assert_eq!(refreshed.anchor.item_id, None);
        assert_ne!(observed.key(), refreshed.key());
        frame.open = None;
        assert!(frame.conversation().is_none());
    }

    #[test]
    fn only_a_working_agent_turns_and_a_stopped_one_shows_where() {
        let plan: plans::Plan = serde_json::from_value(serde_json::json!({
            "title": "Ship", "items": [
                {"item_id": "a", "text": "Implement", "section": "now", "status": "done"},
                {"item_id": "b", "text": "Review", "section": "now", "status": "in_progress"},
                {"item_id": "c", "text": "Check with you", "section": "next", "status": "blocked"}
            ]
        }))
        .unwrap();
        let places = |state| {
            let at = At::of(&plan, state);
            plan.items
                .iter()
                .map(|item| step(item, &at))
                .collect::<Vec<_>>()
        };
        use ui::Step::*;
        assert_eq!(places(ui::AgentState::Working), [Done, Now, Waits]);
        // Stopped: at the step that waits, and nothing turns.
        assert_eq!(places(ui::AgentState::Idle), [Done, Ahead, Waits]);
        let at = At::of(&plan, ui::AgentState::Unknown);
        assert!(at.stopped_at(&plan.items[2]));
        let mut free = plan.clone();
        free.items[2].status = "pending".into();
        let at = At::of(&free, ui::AgentState::Idle);
        let free: Vec<_> = free.items.iter().map(|item| step(item, &at)).collect();
        assert_eq!(free, [Done, Stopped, Ahead]);
    }
}
