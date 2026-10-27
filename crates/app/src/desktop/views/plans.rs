//! Each agent's current plan in Reports & updates: one card per agent with
//! its steps, what is being done now and what waits. Plans are the agents'
//! declared intent; this view only reads them.

use std::{path::PathBuf, time::Duration};

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
    page: Option<Result<PlansPage, Error>>,
    _poll: Task<()>,
}

impl Plans {
    pub fn new(directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok(directory) = this.read_with(cx, |plans, _| plans.directory.clone()) else {
                    return;
                };
                let page = cx
                    .background_executor()
                    .spawn(async move { plans::list(&directory, None) })
                    .await;
                if this
                    .update(cx, |plans, cx| {
                        // Keep the last plans when a later read fails.
                        if page.is_ok() || !matches!(plans.page, Some(Ok(_))) {
                            plans.page = Some(page);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        Self {
            directory,
            page: None,
            _poll: poll,
        }
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
        let body = match &self.page {
            None => ui::empty_state(
                ui::empty_tile("activity", c.selected, c.muted),
                "Reading current plans…",
                "",
            )
            .into_any_element(),
            Some(Err(error)) => ui::empty_state(
                ui::empty_tile("unplug", c.selected, c.muted),
                "No current plans",
                error.message(),
            )
            .into_any_element(),
            Some(Ok(page)) => {
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
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .child(ui::view_footer(
                &[],
                "each agent's declared plan · it changes as the agent works",
            ))
    }
}
