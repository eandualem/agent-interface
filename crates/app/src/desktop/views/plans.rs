//! Each agent's current plan in Reports & updates: one card per agent with
//! its steps on a track and what they wait on, and an opened plan drawn as
//! a graph with the selected step beside it. Plans are the agents' declared
//! intent; this view only reads them.

use std::{cell::Cell, collections::BTreeMap, path::PathBuf, rc::Rc, time::Duration};

use agent_interface_roster::{
    conversations,
    plans::{self, Entry, Error, Item, PlansPage},
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
    /// The CLI each agent runs in, from the same read.
    runtimes: BTreeMap<String, String>,
    /// Why the latest read failed, shown beside the plans last read.
    error: Option<Error>,
    /// The card the keyboard is on, by author.
    selected: Option<String>,
    /// The opened plan, by author, and the step selected in it.
    open: Option<String>,
    step: Option<String>,
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
        read: Result<(PlansPage, BTreeMap<String, String>, String), Error>,
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
            Ok((page, runtimes, provider)) => {
                self.frame.provider = Some(provider);
                self.frame.page = Some(page);
                self.frame.runtimes = runtimes;
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
                agent_interface_roster::views::runtimes(&directory),
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
        let frame = &mut self.frame;
        let has_plan = frame.page.as_ref().is_some_and(|page| {
            page.items
                .iter()
                .any(|entry| entry.author_id == author && entry.record.is_some())
        });
        if !has_plan {
            return;
        }
        frame.selected = Some(author.clone());
        frame.open = Some(author);
        frame.step = frame
            .opened()
            .and_then(|entry| entry.record.as_ref())
            .and_then(|record| record.plan.current())
            .map(|item| item.item_id.clone());
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
            Some(record) => (u8::from(!record.plan.blocked()), record.age_seconds),
            None => (2, u64::MAX),
        });
        entries
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

/// A step's place in its plan, as [`ui::plan_step`] and [`ui::plan_node`]
/// show it.
fn step(item: &Item, current: Option<&str>) -> ui::Step {
    if item.done() {
        ui::Step::Done
    } else if item.blocked() {
        ui::Step::Waits
    } else if Some(item.item_id.as_str()) == current {
        ui::Step::Now
    } else {
        ui::Step::Ahead
    }
}

/// The plan's state as the agent's mark: waiting, finished or working.
fn mark(plan: &plans::Plan) -> gpui::Div {
    if plan.blocked() {
        ui::agent_symbol(ui::AgentState::NeedsYou)
    } else if plan.items.iter().all(Item::done) {
        ui::done_symbol()
    } else {
        ui::agent_symbol(ui::AgentState::Working)
    }
}

fn runtime(plans: &Frame, agent: &str) -> Option<gpui::Div> {
    plans
        .runtimes
        .get(agent)
        .filter(|runtime| !runtime.is_empty())
        .map(|runtime| ui::badge(runtime.clone()))
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
/// on a track and what they wait on.
fn card(plans: &Frame, entry: &Entry, cx: &mut Context<ViewPane>) -> AnyElement {
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
    let current = plan.current().map(|item| item.item_id.clone());
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
        track = track.child(ui::plan_step(
            super::line(&item.text),
            step(item, current.as_deref()),
        ));
    }
    let waiting: Vec<&Item> = plan.items.iter().filter(|item| item.blocked()).collect();
    let waits = if waiting.is_empty() {
        ui::text(div().text_color(c.muted), &t.text.meta)
            .child("Nothing waiting · every step can continue")
    } else {
        let mut tags = div().flex().flex_wrap().gap(px(t.space.s6));
        for item in waiting {
            tags = tags.child(ui::waits_tag("Waits", None, super::line(&item.text), false));
        }
        tags
    };
    ui::plan_card(id, selected)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .w_full()
                .child(mark(plan))
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
                column = column.child(card(plans, entry, cx));
            }
            column.into_any_element()
        }
    };
    // A failed refresh says so above the plans last read.
    let stale = plans
        .page
        .is_some()
        .then_some(plans.error)
        .flatten()
        .map(|error| {
            ui::notice(
                "unplug",
                format!("{} These are the plans last read.", error.message()),
                None,
            )
        });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(stale)
        .child(body)
        .child(ui::view_footer(
            &[("↑↓", "move"), ("↵", "open plan")],
            "each agent's current undertaking · replaced when a new one begins",
        ))
        .into_any_element()
}

/// What a node says above its step's name, by its place in the plan.
fn key_label(step: ui::Step, first_ahead: bool) -> &'static str {
    match step {
        ui::Step::Done => "DONE",
        ui::Step::Now => "NOW",
        ui::Step::Waits => "WAITS",
        ui::Step::Ahead if first_ahead => "NEXT",
        ui::Step::Ahead => "THEN",
    }
}

/// The opened plan's steps as a graph: one node per step in order, joined
/// by arrows, `muted` after a finished step and `faint` ahead.
fn graph(plans: &Frame, plan: &plans::Plan, cx: &mut Context<ViewPane>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let current = plan.current().map(|item| item.item_id.clone());
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
        let place = step(item, current.as_deref());
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
        let note = (item.section == "later" && place == ui::Step::Ahead)
            .then(|| SharedString::from("later"));
        let id = item.item_id.clone();
        canvas = canvas.child(
            ui::plan_node(
                SharedString::from(format!("plan-step-{}", item.item_id)),
                place,
                key_label(place, first_ahead),
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
    .child(div().flex_1())
    .child("Select a step to see why and what it points to.")
}

/// The selected step beside the graph: where it stands, its name, the
/// agent's words about it and what it points to.
fn step_panel(
    pane: &ViewPane,
    plans: &Frame,
    plan: &plans::Plan,
    window: &mut Window,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let current = plan.current().map(|item| item.item_id.clone());
    let chosen = plans
        .step
        .as_ref()
        .and_then(|id| plan.items.iter().find(|item| &item.item_id == id));
    let mut context = div()
        .id("plan-step-context")
        .min_h_0()
        .max_h(gpui::relative(0.35))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(t.space.s8))
        .px(px(t.space.s14))
        .py(px(t.space.s12))
        .border_b(px(t.size.hairline))
        .border_color(c.line);
    if let Some(item) = chosen {
        let place = step(item, current.as_deref());
        let (label, color) = match place {
            ui::Step::Waits => ("WAITS · SELECTED", c.attention_text),
            ui::Step::Now => ("NOW · SELECTED", c.accent_text),
            ui::Step::Done => ("DONE · SELECTED", c.ok),
            ui::Step::Ahead => ("AHEAD · SELECTED", c.muted),
        };
        context = context
            .child(ui::text(div().text_color(color), &t.text.label).child(label))
            .child(
                ui::text(div().text_color(c.text), &t.text.empty_title).child(selection::text(
                    pane,
                    &super::line(&item.text),
                    &[],
                    cx,
                )),
            );
        if !item.detail.trim().is_empty() {
            context = context.child(
                ui::text(div().text_color(c.text_2), &t.text.body)
                    .line_height(px(t.text.body.size * 1.45))
                    .child(selection::text(pane, item.detail.trim(), &[], cx)),
            );
        }
        let fact = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_center()
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
        let mut facts = ui::text(div().flex().flex_col().gap(px(t.space.s6)), &t.text.status)
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
        context = context.child(facts);
    }
    match &pane.conversation {
        Some(conversation) => conversation.update(cx, |conversation, cx| {
            conversation.panel(Some(context.into_any_element()), window, cx)
        }),
        None => ui::conversation_panel().child(context).into_any_element(),
    }
}

/// An opened plan: its agent, title and summary, the steps as a graph and
/// the selected step beside it.
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
    let done = plan.items.iter().filter(|item| item.done()).count();
    let waiting = plan.items.iter().filter(|item| item.blocked()).count();
    let ahead = plan
        .items
        .iter()
        .filter(|item| !item.done() && !item.blocked() && !item.in_progress())
        .count();
    let now = plan.current().filter(|item| !item.blocked());
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
        summary = summary.child(
            div()
                .flex()
                .min_w_0()
                .gap(px(t.space.s4))
                .child("Now:")
                .child(strong(super::line(&now.text), c.accent_text).text_ellipsis()),
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
    .child(mark(plan))
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
                        .child(graph(plans, plan, cx)),
                )
                .child(legend()),
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
            .child(step_panel(pane, plans, plan, window, cx)),
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
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
