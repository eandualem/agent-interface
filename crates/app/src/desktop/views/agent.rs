//! Agent: one agent's state, what it is waiting for, its work and messages.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::Agent;

use super::{ViewEvent, ViewPane, ago, line, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

pub(super) fn body(agent: &Agent, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    if !agent.known {
        return ui::empty_state(
            ui::empty_tile("user", c.selected, c.muted),
            "Backbone does not know this agent",
            format!("{} may have been renamed or removed.", agent.name),
        )
        .into_any_element();
    }
    let state = AgentState::from_report(&agent.state);
    let (word, color) = match (agent.online, state) {
        (false, _) => ("offline".to_owned(), c.muted),
        (true, AgentState::NeedsYou) => (
            format!(
                "needs you{}",
                agent
                    .reason
                    .as_ref()
                    .map(|reason| format!(" · {reason}"))
                    .unwrap_or_default()
            ),
            c.attention_text,
        ),
        (true, AgentState::Working) => ("working".to_owned(), c.accent_text),
        (true, state) => (state.word().to_owned(), c.muted),
    };
    let name = agent.name.clone();
    let toolbar = ui::view_toolbar()
        .child(ui::badge(agent.runtime.clone()))
        .child(div().text_color(color).child(word))
        .children(
            agent
                .state_age_seconds
                .map(|age| format!("for {}", ago(age))),
        )
        .child(div().flex_1())
        .child(
            ui::button("agent-terminal", "Its terminal", ui::Button::Secondary).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(ViewEvent::ShowAgent(name.clone()))),
            ),
        );

    let mut body = list("agent-card")
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .gap(px(t.space.s16));
    if state == AgentState::NeedsYou
        && agent.online
        && let Some(message) = &agent.last_message
    {
        body = body.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .p(px(t.space.s14))
                .rounded(px(t.radius.pane))
                .bg(c.attention_soft)
                .border(px(t.size.hairline))
                .border_color(c.attention)
                .child(
                    ui::text(div().text_color(c.attention_text), &t.text.row_selected)
                        .child("It is waiting for you"),
                )
                .child(ui::text(div().text_color(c.text), &t.text.row).child(message.clone()))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child("Its last reply, as its hook reported it. Answer in its terminal."),
                ),
        );
    } else if let Some(detail) = &agent.detail {
        body = body
            .child(ui::text(div().text_color(c.attention_text), &t.text.body).child(line(detail)));
    }
    let model = agent
        .model
        .as_ref()
        .map(|model| format!(" · {model}"))
        .unwrap_or_else(|| " · CLI default model".into());
    let working = match (agent.current_issue, &agent.current_repo) {
        (Some(issue), Some(repo)) => format!("{repo}#{issue}"),
        (Some(issue), None) => format!("#{issue}"),
        _ => "nothing reported".into(),
    };
    let facts = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s6))
        .child(ui::fact(
            "CLI and model",
            div().child(format!("{}{model}", agent.runtime)),
        ))
        .child(ui::fact("Directory", mono(agent.dir.clone())))
        .child(ui::fact(
            "Repository",
            mono(if agent.repo.is_empty() {
                "none".to_owned()
            } else {
                agent.repo.clone()
            }),
        ))
        .child(ui::fact(
            "Watches",
            mono(list_or_none(&agent.watches, " · ")),
        ))
        .child(ui::fact(
            "Tags",
            div().child(list_or_none(&agent.tags, " · ")),
        ))
        .child(ui::fact("Working on", mono(working)));
    body = body.child(facts);

    let mut received = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(ui::view_heading(
            format!("RECENT MESSAGES · {}", agent.received.len()),
            "what Backbone delivered or is holding",
        ));
    for (index, message) in agent.received.iter().enumerate() {
        received = received.child(
            ui::text(
                div()
                    .id(SharedString::from(format!("agent-message-{index}")))
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.row))
                    .px(px(t.space.s8))
                    .border_t(px(t.size.hairline))
                    .border_color(c.line),
                &t.text.status,
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(t.size.button + t.space.s6))
                    .text_color(c.muted)
                    .child(since(message.at)),
            )
            .child(
                mono(message.kind.clone())
                    .flex_shrink_0()
                    .text_color(c.text_2),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(c.muted)
                    .child(message.outcome.replace('_', " ")),
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
    body = body.child(received);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(toolbar)
        .child(body)
        .child(ui::view_footer(
            &[],
            "read-only · answer it in its terminal",
        ))
        .into_any_element()
}

fn list_or_none(items: &[String], separator: &str) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(separator)
    }
}
