//! Board E: the requests for access, waiting first, each card with what it
//! asks for, why, and Backbone's warning; then the decided ones.
use super::{Menu, Pane, model};
use crate::desktop::{
    components::{self as ui, Button},
    design::{colors, tokens},
    views::connections::hinted,
};
use agent_interface_roster::profiles::access::{Request, RequestState};
use chrono::{DateTime, FixedOffset};
use gpui::{Context, Div, SharedString, Stateful, div, prelude::*, px};

/// Which requests the view lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Waiting,
    Decided,
}

/// What a card's requests are decided with when Backbone's records say
/// approving changes nothing.
const NOT_NEEDED: &str = "Not needed: it already reaches this.";

impl Pane {
    pub(super) fn requests_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = &self.model else {
            return div();
        };
        let t = tokens();
        let now = chrono::Local::now().fixed_offset();
        let mut column = div()
            .id("access-requests")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.space.s12))
            .px(px(t.space.s20))
            .pt(px(t.space.s12))
            .pb(px(t.space.s16));
        match self.tab {
            Tab::Waiting => {
                let pending = self.listed_pending();
                let (on_you, on_agent) = model::waiting(&pending);
                if !on_you.is_empty() {
                    column = column.child(heading("WAITING ON YOU", model::from_agents(&on_you)));
                    for card in model::cards(&on_you) {
                        column = column.child(self.waiting_card(&card, model, true, &now, cx));
                    }
                }
                if !on_agent.is_empty() {
                    column = column.child(heading(
                        "WAITING ON AN AGENT",
                        format!("{} · you can decide these too", on_agent.len()),
                    ));
                    for card in model::cards(&on_agent) {
                        column = column.child(self.waiting_card(&card, model, false, &now, cx));
                    }
                }
                if on_you.is_empty() && on_agent.is_empty() {
                    column = column.child(ui::group_note("Nothing waits for a decision."));
                }
            }
            Tab::Decided => {
                let decided: Vec<&Request> = model
                    .decided
                    .iter()
                    .flatten()
                    .filter(|r| self.agent_filter.as_ref().is_none_or(|a| &r.agent == a))
                    .collect();
                if decided.is_empty() && model.decided.is_some() {
                    column = column.child(ui::group_note("Nothing has been decided yet."));
                }
                for request in decided {
                    column = column.child(decided_card(request, &now));
                }
                if let Some(before) = self.decided_before {
                    column = column.child(
                        ui::link(div().id("access-decided-older").child("Show older")).on_click(
                            cx.listener(move |pane, _, _, cx| {
                                pane.read_decided(Some(before), cx);
                            }),
                        ),
                    );
                }
            }
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.requests_header())
            .child(self.requests_toolbar(model, &now, cx))
            .children(self.notice_band(cx))
            .child(column)
            .child(ui::view_footer(
                &[
                    ("↑↓", "move"),
                    ("a", "approve"),
                    ("r", "refuse"),
                    ("n", "note"),
                ],
                "backbone access requests --pending",
            ))
    }

    fn requests_header(&self) -> Div {
        let (t, c) = (tokens(), colors());
        let profile = self.profile_name.as_deref().unwrap_or("this profile");
        ui::view_header("shield", "Access")
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(format!("what agents in {profile} have asked for")),
            )
            .children(self.live_note().map(|note| ui::text(note, &t.text.meta)))
    }

    fn requests_toolbar(
        &self,
        model: &model::Model,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let (on_you, on_agent) = model::waiting(&model.pending);
        let count = on_you.len() + on_agent.len();
        let mut asking: Vec<String> = model
            .pending
            .iter()
            .chain(model.decided.iter().flatten())
            .map(|r| r.agent.clone())
            .collect();
        asking.sort();
        asking.dedup();
        let choices = std::iter::once(None)
            .chain(asking.into_iter().map(Some))
            .map(|choice| {
                let label = choice.clone().unwrap_or_else(|| "All agents".into());
                let current = choice == self.agent_filter;
                ui::menu_item(
                    SharedString::from(format!("access-agent-filter-{label}")),
                    current.then(|| ui::icon("check", t.size.icon, c.accent)),
                    label,
                    None,
                )
                .when(choice.is_some(), |el| el.font_family(t.font.mono.clone()))
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.agent_filter = choice.clone();
                    pane.menu = None;
                    pane.card = None;
                    cx.notify();
                }))
            })
            .collect();
        let filter = ui::chip(
            "access-agent-filter",
            self.agent_filter
                .clone()
                .unwrap_or_else(|| "All agents".into()),
            false,
        )
        .gap(px(t.space.s6))
        .text_color(c.text_2)
        .child(ui::icon("chevron-down", t.size.icon - 3.0, c.muted));
        ui::view_toolbar()
            .child(
                ui::segments()
                    .child(
                        ui::segment(
                            "access-tab-waiting",
                            format!("Waiting · {count}"),
                            self.tab == Tab::Waiting,
                        )
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.tab = Tab::Waiting;
                            cx.notify();
                        })),
                    )
                    .child(
                        ui::segment("access-tab-decided", "Decided", self.tab == Tab::Decided)
                            .on_click(cx.listener(|pane, _, _, cx| {
                                if pane.tab != Tab::Decided {
                                    pane.tab = Tab::Decided;
                                    pane.read_decided(None, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(self.with_menu(Menu::AgentFilter, filter, choices, cx))
            .child(div().flex_1())
            .children(model::oldest_wait(&model.pending, now))
            .child(self.waiting_button(count, cx))
    }

    fn waiting_card(
        &self,
        card: &[&Request],
        model: &model::Model,
        on_you: bool,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let first = card[0];
        let ids: Vec<i64> = card.iter().map(|r| r.id).collect();
        let key = first.id;
        let focused = self.card == Some(key);
        let not_needed = model
            .agent(&first.agent)
            .and_then(|agent| model::not_needed(agent, card));
        let frame = div()
            .flex()
            .flex_shrink_0()
            .flex_col()
            .rounded(px(t.radius.card))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(match (focused, on_you) {
                (true, _) => c.focus,
                (false, true) => c.accent,
                (false, false) => c.line,
            })
            .overflow_hidden();
        let frame = self.busy(&format!("access-decide-{key}"), frame);
        let strip = on_you.then(|| {
            ui::text(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .h(px(t.size.view_footer))
                    .px(px(t.space.s14))
                    .bg(c.accent_soft)
                    .whitespace_nowrap(),
                &t.text.status,
            )
            .child(ui::icon("clock", t.size.icon - 1.0, c.accent_text))
            .child(
                div()
                    .text_color(c.accent_text)
                    .font_weight(t.text.row_selected.weight())
                    .child("Waiting on you"),
            )
            .child(div().flex_1())
            .child(
                ui::text(div().text_color(c.text_2), &t.text.meta)
                    .child(model::asked(first, now, false)),
            )
        });
        let (verb, object) = model::phrase(card);
        let who = ui::text(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .px(px(t.space.s14))
                .pt(px(t.space.s12))
                .whitespace_nowrap(),
            &t.text.row,
        )
        .child(ui::agent_mark(&first.agent, t.size.icon_large + t.space.s4))
        .child(ui::text(
            ui::mono(first.agent.clone()).text_color(c.text),
            &t.text.status,
        ))
        .child(div().text_color(c.muted).child(verb))
        .children(object.map(object_chip))
        .children(model::machine_of(&first.grant).map(ui::machine_chip))
        .when(!on_you, |el| {
            el.child(div().flex_1())
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                        "waits on {} · {}",
                        first.waits_on.clone().unwrap_or_default(),
                        model::asked(first, now, true)
                    )),
                )
        });
        let indent = indent();
        let reasons = if card.len() == 1 {
            quoted(&first.reason).pl(px(indent)).pt(px(t.space.s8))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .pl(px(indent))
                .pr(px(t.space.s14))
                .pt(px(t.space.s10))
                .children(card.iter().map(|request| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s2 + 1.0))
                        .children(model::thing(&request.grant).map(|thing| {
                            ui::text(ui::mono(thing).text_color(c.text), &t.text.status)
                        }))
                        .child(quoted(&request.reason).pr_0())
                }))
        };
        let warning = first
            .warning
            .clone()
            .map(|warning| note_line("triangle-alert", c.accent_text, warning).pl(px(indent)));
        let reach = not_needed
            .clone()
            .map(|note| note_line("info", c.muted, note).pl(px(indent)));
        let note = (self.noting == Some(key)).then(|| {
            div()
                .pl(px(indent))
                .pr(px(t.space.s14))
                .pt(px(t.space.s10))
                .child(ui::field_frame(true).h(px(t.size.button)).child(hinted(
                    &self.note_field,
                    "A note for the agent",
                    cx,
                )))
        });
        let approve = |label: &'static str, kind: Button| {
            let ids = ids.clone();
            ui::button(
                SharedString::from(format!("access-approve-{key}")),
                label,
                kind,
            )
            .h(px(t.size.button - t.space.s2))
            .on_click(cx.listener(move |pane, _, _, cx| pane.decide(ids.clone(), true, cx)))
        };
        let refuse = {
            let ids = ids.clone();
            text_button(format!("access-refuse-{key}"), "Refuse")
                .on_click(cx.listener(move |pane, _, _, cx| pane.decide(ids.clone(), false, cx)))
        };
        let mut actions = ui::text(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .mt(px(t.space.s12))
                .px(px(t.space.s14))
                .py(px(t.space.s10))
                .border_t(px(t.size.hairline))
                .border_color(c.line)
                .whitespace_nowrap(),
            &t.text.row,
        );
        if not_needed.is_some() {
            let close = match card.len() {
                1 => "Close as not needed".to_owned(),
                2 => "Close both as not needed".to_owned(),
                n => format!("Close all {n} as not needed"),
            };
            let closed = ids.clone();
            actions = actions
                .child(
                    ui::button(
                        SharedString::from(format!("access-close-{key}")),
                        close,
                        Button::Secondary,
                    )
                    .h(px(t.size.button - t.space.s2))
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.decide_with(closed.clone(), false, NOT_NEEDED.into(), cx);
                    })),
                )
                .child(approve("Approve anyway", Button::Secondary).bg(gpui::transparent_black()))
                .child(refuse);
        } else if on_you {
            actions = actions
                .child(approve("Approve", Button::Primary))
                .child(refuse)
                .child(
                    text_button(format!("access-note-{key}"), "Add a note")
                        .text_color(c.muted)
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            pane.open_note(key, window, cx);
                        })),
                );
        } else {
            actions = actions
                .child(approve("Approve", Button::Secondary))
                .child(refuse);
        }
        frame
            .children(strip)
            .child(who)
            .child(reasons)
            .children(warning)
            .children(reach)
            .children(note)
            .child(actions)
    }
}

/// Where a card's lines start, under the agent's name.
fn indent() -> f32 {
    let t = tokens();
    t.space.s14 + t.size.icon_large + t.space.s4 + t.space.s8
}

/// A section label over the cards, with its count.
fn heading(label: &'static str, note: String) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .items_baseline()
        .gap(px(t.space.s8))
        .px(px(t.space.s2))
        .child(ui::text(div().text_color(c.muted), &t.text.label).child(label))
        .child(ui::text(div().text_color(c.muted), &t.text.meta).child(note))
}

/// What a request asks for, on a tinted chip, as the board draws a browser.
fn object_chip(object: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .h(px(t.size.chip))
            .px(px(t.space.s8))
            .rounded(px(t.radius.icon_button))
            .bg(c.selected)
            .font_family(t.font.mono.clone())
            .text_color(c.text),
        &t.text.status,
    )
    .child(object)
}

/// The agent's reason, in quotes.
fn quoted(reason: &str) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div().pr(px(t.space.s14)).text_color(c.text_2),
        &t.text.status,
    )
    .child(format!("“{}”", reason.trim()))
}

/// A line under the reason with its icon: Backbone's warning, or why
/// approving changes nothing.
fn note_line(icon: &'static str, tint: gpui::Rgba, note: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .items_start()
            .gap(px(t.space.s8))
            .pr(px(t.space.s14))
            .pt(px(t.space.s8))
            .text_color(c.text_2),
        &t.text.status,
    )
    .child(ui::icon(icon, t.size.icon - 1.0, tint).mt(px(t.space.s2)))
    .child(div().flex_1().min_w_0().child(note))
}

/// A quiet button that is only its label.
fn text_button(id: String, label: &'static str) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(t.size.button - t.space.s2))
            .px(px(t.space.s10))
            .rounded(px(t.radius.row))
            .text_color(c.text_2)
            .cursor_pointer()
            .hover(|el| el.bg(colors().hover)),
        &t.text.row,
    )
    .child(label)
}

/// A decided request: who asked for what, how it ended and when.
fn decided_card(request: &Request, now: &DateTime<FixedOffset>) -> Div {
    let (t, c) = (tokens(), colors());
    let (verb, object) = model::phrase(&[request]);
    let when = model::decided_at(request, now)
        .map(|at| format!(" · {at}"))
        .unwrap_or_default();
    let tint = if request.state == RequestState::Approved {
        c.ok
    } else {
        c.muted
    };
    div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .pb(px(t.space.s12))
        .rounded(px(t.radius.card))
        .bg(c.panel)
        .border(px(t.size.hairline))
        .border_color(c.line)
        .child(
            ui::text(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .px(px(t.space.s14))
                    .pt(px(t.space.s12))
                    .whitespace_nowrap(),
                &t.text.row,
            )
            .child(ui::agent_mark(
                &request.agent,
                t.size.icon_large + t.space.s4,
            ))
            .child(ui::text(
                ui::mono(request.agent.clone()).text_color(c.text),
                &t.text.status,
            ))
            .child(div().text_color(c.muted).child(verb))
            .children(object.map(object_chip))
            .child(div().flex_1())
            .child(
                ui::text(div().text_color(tint), &t.text.meta)
                    .child(format!("{}{when}", model::decided_label(request))),
            ),
        )
        .child(quoted(&request.reason).pl(px(indent())).pt(px(t.space.s8)))
        .when(!request.note.trim().is_empty(), |card| {
            card.child(note_line("message-square", c.muted, request.note.clone()).pl(px(indent())))
        })
}
