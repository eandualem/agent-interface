//! Agent: one agent's card. Its state and what it is waiting for, its work,
//! what Backbone holds for it, and its connections, in sections; ↗ sections
//! open beside this pane.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_core::{View, ViewKind};
use tmux_workspaces_roster::views::{Agent, Received};

use super::{Section, ViewEvent, ViewPane, ago, line, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

fn held(message: &Received) -> bool {
    matches!(
        message.outcome.as_str(),
        "queued" | "held" | "stored" | "agent_working" | "deferred"
    )
}

pub(super) fn body(pane: &ViewPane, agent: &Agent, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    if !agent.known {
        return div()
            .size_full()
            .flex()
            .flex_col()
            .child(ui::view_header("user", agent.name.clone()))
            .child(ui::empty_state(
                ui::empty_tile("user", c.selected, c.muted),
                "Backbone does not know this agent",
                format!("{} may have been renamed or removed.", agent.name),
            ))
            .into_any_element();
    }
    let state = AgentState::from_report(&agent.state);
    let (word, color) = match (agent.online, state) {
        (false, _) => ("offline".to_owned(), c.muted),
        (true, AgentState::NeedsYou) => (
            match &agent.reason {
                Some(reason) => format!("waiting for a person · {reason}"),
                None => "waiting for a person".to_owned(),
            },
            c.attention_text,
        ),
        (true, AgentState::Working) => ("working".to_owned(), c.accent_text),
        (true, state) => (state.word().to_owned(), c.muted),
    };
    let name = agent.name.clone();
    let header = ui::view_header("user", agent.name.clone())
        .pr(px(t.space.s6))
        .child(ui::badge(agent.runtime.clone()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s4 + 1.0))
                .text_color(color)
                .when(agent.online, |el| {
                    el.child(ui::agent_symbol(state, "agent-card-state"))
                })
                .child(word),
        )
        .child(div().flex_1())
        .child(
            ui::text(
                div()
                    .id("agent-terminal")
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .h(px(t.size.chip - t.space.s2))
                    .px(px(t.space.s8))
                    .rounded(px(t.radius.key + 1.0))
                    .border(px(t.size.hairline))
                    .border_color(c.line_strong)
                    .bg(c.field)
                    .text_color(c.text_2)
                    .cursor_pointer()
                    .hover(|el| el.bg(colors().hover)),
                &t.text.meta,
            )
            .child("Its terminal")
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ViewEvent::ShowAgent(name.clone())))),
        );

    // Sections, then the views that open beside this pane.
    let mut nav = div()
        .flex()
        .flex_shrink_0()
        .items_end()
        .gap(px(t.space.s2))
        .h(px(t.size.view_toolbar))
        .px(px(t.space.s10))
        .border_b(px(t.size.hairline))
        .border_color(c.line);
    for (section, label) in [
        (Section::Overview, "Overview"),
        (Section::Permissions, "Permissions"),
        (Section::Connections, "Connections"),
    ] {
        nav = nav.child(
            tab(
                SharedString::from(format!("agent-section-{label}")),
                label,
                "",
                section == pane.section,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.section = section;
                cx.notify();
            })),
        );
    }
    for (kind, label) in [
        (Some(ViewKind::Skills), "Skills"),
        (None, "Policies"),
        (Some(ViewKind::Usage), "Usage"),
    ] {
        let link = tab(
            SharedString::from(format!("agent-open-{label}")),
            label,
            "↗",
            false,
        );
        nav = nav.child(match kind {
            Some(kind) => link.on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ViewEvent::OpenBeside(View { kind, agent: None }))
            })),
            None => link.opacity(0.45),
        });
    }

    let content = match pane.section {
        Section::Overview => overview(agent, state),
        Section::Permissions => unavailable(
            "Permissions are not served yet",
            "Backbone does not report an agent's launch settings or its CLI's rules yet.",
        ),
        Section::Connections => connections(agent),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(nav)
        .child(content)
        .into_any_element()
}

fn tab(
    id: SharedString,
    label: &'static str,
    mark: &'static str,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(t.space.s4 + 1.0))
            .h(px(t.size.row))
            .px(px(t.space.s10))
            .border_b(px(t.size.focus_ring))
            .border_color(if on {
                c.accent
            } else {
                gpui::transparent_black().into()
            })
            .text_color(if on { c.text } else { c.muted })
            .cursor_pointer()
            .hover(|el| el.text_color(colors().text)),
        &t.text.row,
    )
    .when(on, |el| el.font_weight(t.text.row_selected.weight()))
    .child(label)
    .when(!mark.is_empty(), |el| {
        el.child(ui::text(div().text_color(c.muted), &t.text.meta).child(mark))
    })
}

fn overview(agent: &Agent, state: AgentState) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut body = list("agent-overview")
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .gap(px(t.space.s16));
    if agent.online && state == AgentState::NeedsYou {
        body = body.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s10))
                .p(px(t.space.s14))
                .rounded(px(t.radius.pane))
                .bg(c.attention_soft)
                .border(px(t.size.hairline))
                .border_color(c.attention)
                .child(
                    ui::text(div().flex().gap(px(t.space.s8)), &t.text.body)
                        .child(
                            div()
                                .text_color(c.attention_text)
                                .font_weight(t.text.label.weight())
                                .child("It asked you a question"),
                        )
                        .child(div().text_color(c.muted).child(format!(
                            "{} ago · it does nothing more until someone answers in its terminal",
                            agent.state_age_seconds.map(ago).unwrap_or_default()
                        ))),
                )
                .children(agent.last_message.as_ref().map(|message| {
                    ui::text(div().text_color(c.text), &t.text.doc_body).child(message.clone())
                }))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child("From its last reply, as its hook reported it."),
                ),
        );
    } else if let Some(detail) = &agent.detail {
        body = body
            .child(ui::text(div().text_color(c.attention_text), &t.text.body).child(line(detail)));
    }
    let model = agent
        .model
        .clone()
        .map(|model| format!(" · {model}"))
        .unwrap_or_else(|| " · its default model".into());
    let working = match (agent.current_issue, &agent.current_repo) {
        (Some(issue), Some(repo)) => format!("{repo}#{issue}"),
        (Some(issue), None) => format!("#{issue}"),
        _ => "nothing reported".into(),
    };
    let mut tags = div().flex().gap(px(t.space.s4));
    for tag in &agent.tags {
        tags = tags.child(ui::pill(tag.clone()));
    }
    body = body.child(
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .child(ui::fact(
                "CLI and model",
                div()
                    .flex()
                    .child(mono(agent.runtime.clone()).text_color(c.text))
                    .child(div().text_color(c.muted).child(model)),
            ))
            .child(ui::fact("Directory", mono(agent.dir.clone())))
            .child(ui::fact(
                "Watches",
                mono(if agent.watches.is_empty() {
                    "nothing".into()
                } else {
                    agent.watches.join(" · ")
                }),
            ))
            .child(ui::fact("Tags", tags))
            .child(ui::fact("Working on", mono(working)))
            .child(ui::fact(
                "In this state",
                div().child(
                    agent
                        .state_age_seconds
                        .map(ago)
                        .map(|age| format!("for {age}"))
                        .unwrap_or_else(|| "unknown".into()),
                ),
            )),
    );
    let holding: Vec<_> = agent.received.iter().filter(|m| held(m)).collect();
    body = body.child(messages(
        &format!("HELD FOR IT · {}", holding.len()),
        "delivered once it is free",
        &holding,
    ));
    let recent: Vec<_> = agent.received.iter().filter(|m| !held(m)).collect();
    body = body.child(messages(
        &format!("RECENT DELIVERIES · {}", recent.len()),
        "newest first",
        &recent,
    ));
    body.into_any_element()
}

fn messages(label: &str, note: &'static str, messages: &[&Received]) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut section = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s4))
        .child(ui::view_heading(label.to_owned(), note).px(px(0.0)));
    if messages.is_empty() {
        section =
            section.child(ui::text(div().text_color(c.muted), &t.text.status).child("nothing"));
    }
    for message in messages {
        section = section.child(
            ui::text(
                div().flex().items_baseline().gap(px(t.space.s10)),
                &t.text.status,
            )
            .child(
                mono(message.kind.replace('_', " "))
                    .flex_shrink_0()
                    .w(px(t.size.fact_label))
                    .text_color(c.muted),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(c.text_2)
                    .child(since(message.at)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(line(&message.preview)),
            ),
        );
    }
    section
}

fn connections(agent: &Agent) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let card = |title: &'static str, state: String, main: String, sub: String| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .p(px(t.space.s12))
            .rounded(px(t.radius.slot))
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .child(
                ui::text(
                    div().flex().items_center().text_color(c.muted),
                    &t.text.label,
                )
                .child(title)
                .child(div().flex_1())
                .child(ui::text(div().text_color(c.ok), &t.text.meta).child(state)),
            )
            .child(ui::text(div().text_color(c.text), &t.text.body).child(main))
            .child(ui::text(div().text_color(c.muted), &t.text.meta).child(sub))
    };
    let github = card(
        "GITHUB",
        if agent.watches.is_empty() {
            String::new()
        } else {
            "watching".into()
        },
        match (agent.watches.len(), agent.repo.is_empty()) {
            (0, true) => "Watches nothing, owns no repository".into(),
            (n, true) => format!("Watches {n} repositories, owns none"),
            (n, false) => format!("Owns {}, watches {n}", agent.repo),
        },
        format!("Issues labelled for:{} reach its queue.", agent.name),
    );
    let sources = card(
        "SOURCES",
        format!(
            "{} filter{}",
            agent.subscriptions.len(),
            if agent.subscriptions.len() == 1 {
                ""
            } else {
                "s"
            }
        ),
        if agent.subscriptions.is_empty() {
            "No subscriptions".into()
        } else {
            agent
                .subscriptions
                .iter()
                .map(|s| format!("{} · {}", s.source, s.filter))
                .collect::<Vec<_>>()
                .join(", ")
        },
        agent
            .subscriptions
            .first()
            .map(|s| format!("Priority {}.", s.priority))
            .unwrap_or_else(|| "Added with backbone agent subscribe.".into()),
    );
    list("agent-connections")
        .px(px(t.space.s18))
        .py(px(t.space.s14))
        .gap(px(t.space.s16))
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(ui::view_heading("MCP SERVERS", "not served by Backbone yet").px(px(0.0))),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .gap(px(t.space.s10))
                .child(github)
                .child(sources),
        )
        .into_any_element()
}

fn unavailable(title: &'static str, body: &'static str) -> AnyElement {
    let c = colors();
    ui::empty_state(ui::empty_tile("lock", c.selected, c.muted), title, body).into_any_element()
}
