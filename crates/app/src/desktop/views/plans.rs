//! Each agent's current plan in Reports & updates: one card per agent with
//! its steps, what is being done now and what waits. Plans are the agents'
//! declared intent; this view only reads them.

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use agent_interface_roster::plans::{self, Entry, Error, Item, PlansPage};
use gpui::{Context, SharedString, Task, Window, div, prelude::*, px};

use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// How often an open list rereads the plans.
const REFRESH: Duration = Duration::from_secs(20);

pub(super) struct Plans {
    directory: PathBuf,
    /// The plans last read, kept when a later read fails.
    page: Option<PlansPage>,
    /// Why the latest read failed, shown beside the plans last read.
    error: Option<Error>,
    /// Reads are numbered; a result older than one applied is dropped.
    asked: u64,
    applied: u64,
    /// Whether this section is selected and its pane visible; it reads only then.
    shown: bool,
    visible: Rc<Cell<bool>>,
    _poll: Task<()>,
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
            page: None,
            error: None,
            asked: 0,
            applied: 0,
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

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.shown && self.visible.get() {
            self.refresh(cx);
        }
    }

    fn receive(&mut self, number: u64, page: Result<PlansPage, Error>, cx: &mut Context<Self>) {
        if number <= self.applied {
            return;
        }
        self.applied = number;
        match page {
            Ok(page) => {
                self.page = Some(page);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Read the plans now, as well as on the regular refresh.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.asked += 1;
        let number = self.asked;
        let directory = self.directory.clone();
        let read = cx
            .background_executor()
            .spawn(async move { plans::list_all(&directory) });
        cx.spawn(async move |this, cx| {
            let page = read.await;
            let _ = this.update(cx, |plans, cx| plans.receive(number, page, cx));
        })
        .detach();
    }
}

/// A step's place in its plan, as a [`ui::plan_step`] shows it.
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

fn card(entry: &Entry) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let Some(record) = &entry.record else {
        return ui::group_card().child(
            ui::text(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.group_row))
                    .px(px(t.space.s14))
                    .text_color(c.muted),
                &t.text.status,
            )
            .child(ui::agent_symbol(ui::AgentState::Idle))
            .child(super::mono(entry.agent_name.clone()).text_color(c.text_2))
            .child("No current plan"),
        );
    };
    let plan = &record.plan;
    let current = plan.current().map(|item| item.item_id.clone());
    let state = if plan.blocked() {
        ui::agent_symbol(ui::AgentState::NeedsYou)
    } else if plan.items.iter().all(Item::done) {
        ui::done_symbol()
    } else {
        ui::agent_symbol(ui::AgentState::Working)
    };
    let mut steps = div().flex().flex_wrap().items_center().gap(px(t.space.s6));
    for item in &plan.items {
        steps = steps.child(ui::plan_step(
            item.text.clone(),
            step(item, current.as_deref()),
        ));
    }
    let waiting: Vec<_> = plan.items.iter().filter(|item| item.blocked()).collect();
    let mut waits = ui::text(div().flex().flex_col().gap(px(t.space.s4)), &t.text.meta);
    if waiting.is_empty() {
        waits = waits
            .text_color(c.muted)
            .child("Nothing waiting · every step can continue");
    } else {
        for item in waiting {
            let why = if item.detail.is_empty() {
                "blocked".to_owned()
            } else {
                super::line(&item.detail)
            };
            waits = waits.child(
                div()
                    .flex()
                    .gap(px(t.space.s6))
                    .text_color(c.attention_text)
                    .child(why)
                    .child(div().text_color(c.muted).child(format!("→ {}", item.text))),
            );
        }
    }
    let updated = format!("updated {} ago", super::ago(record.age_seconds as f64));
    ui::group_card()
        .gap(px(t.space.s10))
        .px(px(t.space.s14))
        .py(px(t.space.s12))
        .child(
            ui::text(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(state)
            .child(super::mono(entry.agent_name.clone()).text_color(c.text))
            .child(div().flex_1())
            .when(record.stale, |el| {
                el.child(ui::tag("stale", ui::Tone::Quiet))
            })
            .child(updated),
        )
        .child(ui::text(div().text_color(c.text), &t.text.workspace).child(plan.title.clone()))
        .child(steps)
        .child(waits)
}

impl Render for Plans {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let body = match (&self.page, self.error) {
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
            (Some(page), _) => {
                let mut entries: Vec<&Entry> = page.items.iter().collect();
                // Waiting on someone first, then the most recently updated.
                entries.sort_by_key(|entry| match &entry.record {
                    Some(record) => (u8::from(!record.plan.blocked()), record.age_seconds),
                    None => (2, u64::MAX),
                });
                let mut column =
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s10))
                        .child(ui::group_label(
                            div()
                                .text_color(c.text_2)
                                .font_weight(t.text.row_selected.weight())
                                .child("Now"),
                            "one plan per agent · waiting first",
                        ));
                for entry in entries {
                    column = column.child(
                        card(entry).id(SharedString::from(format!("plan-{}", entry.author_id))),
                    );
                }
                ui::grouped_page("plans", column).into_any_element()
            }
        };
        // A failed refresh says so above the plans last read.
        let stale = self
            .page
            .is_some()
            .then_some(self.error)
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
                &[],
                "each agent's declared plan · it changes as the agent works",
            ))
    }
}
