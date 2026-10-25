//! Read authored updates and answers in the same native pane as other views.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{
    ReportContent, ReportItem, ReportPage, ReportQuery, epoch_seconds,
};

use super::{ViewPane, line, list, markdown, since};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

#[derive(Default)]
pub(super) struct State {
    pub query: ReportQuery,
    pub agent: Option<String>,
    choosing_agent: bool,
    agents: Vec<String>,
    previous: Vec<ReportQuery>,
    /// Records are immutable: keep the opened answer through list refreshes.
    pub open: Option<ReportItem>,
}

impl State {
    fn select_agent(&mut self, agent: Option<String>) {
        self.agent = agent;
        self.query = first(&self.query);
        self.previous.clear();
        self.open = None;
        self.choosing_agent = false;
    }

    /// Authored text is immutable; correction metadata can change after opening.
    pub fn observe(&mut self, page: &ReportPage) {
        if !page.agents.is_empty() {
            self.agents = page.agents.clone();
        }
        let Some(open) = &mut self.open else { return };
        let Some(fresh) = page.items.iter().find(|item| item.id == open.id) else {
            return;
        };
        if let (
            ReportContent::Response { superseded_by, .. },
            ReportContent::Response {
                superseded_by: updated,
                ..
            },
        ) = (&mut open.content, &fresh.content)
        {
            *superseded_by = *updated;
        }
    }
}

fn first(query: &ReportQuery) -> ReportQuery {
    match query {
        ReportQuery::Latest(_) => ReportQuery::Latest(None),
        ReportQuery::Recent(_) => ReportQuery::Recent(None),
        ReportQuery::Responses(_) => ReportQuery::Responses(None),
    }
}

pub(super) fn refresh_toolbar(pane: &ViewPane, cx: &mut Context<ViewPane>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(controls(pane, &pane.reports.agents, cx))
        .child(
            ui::view_toolbar().child(ui::chip("reports-reset", "Refresh", false).on_click(
                cx.listener(|pane, _, _, cx| {
                    pane.reports.query = first(&pane.reports.query);
                    pane.reports.previous.clear();
                    pane.refetch(true, cx);
                }),
            )),
        )
}

fn label(item: &ReportItem) -> String {
    match &item.content {
        ReportContent::Progress { status, .. } => status.clone(),
        ReportContent::Response {
            superseded_by: Some(id),
            ..
        } => format!("corrected by #{id}"),
        ReportContent::Response { .. } => "response".into(),
    }
}

pub(super) fn body(pane: &ViewPane, page: &ReportPage, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("message-square", "Reports & updates");
    if let Some(item) = &pane.reports.open {
        let mut content = list("report-document")
            .px(px(t.space.s24))
            .py(px(t.space.s20))
            .gap(px(t.space.s16))
            .child(
                ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                    "{} · {} · {} · #{}",
                    item.author,
                    item.created_at,
                    label(item),
                    item.id
                )),
            );
        let section = |name: &'static str, text: &str| {
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .child(ui::text(div().text_color(c.text), &t.text.doc_heading).child(name))
                .child(markdown::render(&markdown::parse_authored(text)))
        };
        match &item.content {
            ReportContent::Progress { sections, .. } => {
                for (name, value) in sections {
                    let mut block = section(name, &value.text);
                    for link in &value.links {
                        block = block.child(
                            ui::text(div().text_color(c.muted), &t.text.meta)
                                .child(format!("{}: {}", link.title, link.url)),
                        );
                    }
                    content = content.child(block);
                }
            }
            ReportContent::Response {
                question,
                answer,
                action,
                ..
            } => {
                content = content
                    .child(section("Question", question))
                    .child(section("Answer", answer))
                    .child(section("Action", action));
            }
        }
        return div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(ui::view_toolbar().child(
                ui::back_button("reports-back", "Reports & updates").on_click(cx.listener(
                    |pane, _, _, cx| {
                        pane.reports.open = None;
                        pane.refetch(true, cx);
                    },
                )),
            ))
            .child(content)
            .child(ui::view_footer(
                &[("esc", "back")],
                "authored record · read-only",
            ))
            .into_any_element();
    }
    let waiting = matches!(pane.reports.query, ReportQuery::Latest(_));
    let mut rows = list("reports-rows").gap(px(t.space.s6));
    let mut count = 0;
    for item in page
        .items
        .iter()
        .filter(|item| !waiting || item.needs_owner())
    {
        count += 1;
        let open = item.clone();
        let summary = if waiting {
            match &item.content {
                ReportContent::Progress { sections, .. } => sections
                    .iter()
                    .find(|(name, _)| *name == "Blocker")
                    .map(|(_, value)| value.text.as_str())
                    .unwrap_or(""),
                _ => "",
            }
        } else {
            item.title()
        };
        rows = rows.child(
            div()
                .id(SharedString::from(format!("report-{}", item.id)))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s6))
                .px(px(t.space.s12))
                .py(px(t.space.s10))
                .rounded(px(t.radius.slot))
                .border(px(t.size.hairline))
                .border_color(c.line)
                .cursor_pointer()
                .hover(|style| style.bg(c.hover))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                        "{} · {} ago · {}",
                        item.author,
                        since(epoch_seconds(&item.created_at)),
                        label(item)
                    )),
                )
                .child(ui::text(div().text_color(c.text), &t.text.body).child(line(summary)))
                .child(ui::text(div().text_color(c.accent), &t.text.meta).child(
                    match item.content {
                        ReportContent::Progress { .. } => "Read full report →",
                        ReportContent::Response { .. } => "Read full response →",
                    },
                ))
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.reports.open = Some(open.clone());
                    pane.refetch(false, cx);
                })),
        );
    }
    if count == 0 {
        rows = rows.child(
            ui::text(div().p(px(t.space.s16)).text_color(c.muted), &t.text.body).child(
                if waiting {
                    "No owner blockers on this page."
                } else {
                    "No records on this page."
                },
            ),
        );
    }
    let mut navigation = ui::view_toolbar().gap(px(t.space.s8));
    if !pane.reports.previous.is_empty() {
        navigation = navigation.child(
            ui::chip("reports-previous", "Previous page", false).on_click(cx.listener(
                |pane, _, _, cx| {
                    if let Some(query) = pane.reports.previous.pop() {
                        pane.reports.query = query;
                        pane.refetch(true, cx);
                    }
                },
            )),
        );
    }
    if let Some(next) = page.next.clone() {
        navigation = navigation.child(ui::chip("reports-next", "Next page", false).on_click(
            cx.listener(move |pane, _, _, cx| {
                pane.reports.previous.push(pane.reports.query.clone());
                pane.reports.query = next.clone();
                pane.refetch(true, cx);
            }),
        ));
    }
    navigation = navigation.child(ui::chip("reports-refresh", "Refresh", false).on_click(
        cx.listener(|pane, _, _, cx| {
            pane.reports.query = first(&pane.reports.query);
            pane.reports.previous.clear();
            pane.refetch(true, cx);
        }),
    ));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(controls(pane, &page.agents, cx))
        .child(rows)
        .child(navigation)
        .child(ui::view_footer(
            &[],
            if waiting {
                "latest authored owner blockers · age is not resolution"
            } else {
                "authored records · newest first"
            },
        ))
        .into_any_element()
}

fn controls(pane: &ViewPane, agents: &[String], cx: &mut Context<ViewPane>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut toolbar = ui::view_toolbar()
        .h_auto()
        .min_h(px(t.size.view_toolbar))
        .py(px(t.space.s6))
        .flex_wrap()
        .gap(px(t.space.s6));
    for (query, name) in [
        (ReportQuery::Latest(None), "Waiting on you"),
        (ReportQuery::Recent(None), "Recent updates"),
        (ReportQuery::Responses(None), "Responses"),
    ] {
        toolbar = toolbar.child(
            ui::chip(
                SharedString::from(format!("reports-{name}")),
                name,
                first(&pane.reports.query) == query,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.reports.query = query.clone();
                pane.reports.previous.clear();
                pane.refetch(true, cx);
            })),
        );
    }
    toolbar = toolbar.child(
        ui::chip(
            "reports-agent",
            SharedString::from(format!(
                "{} ▾",
                pane.reports.agent.as_deref().unwrap_or("All agents")
            )),
            pane.reports.agent.is_some(),
        )
        .on_click(cx.listener(|pane, _, _, cx| {
            pane.reports.choosing_agent = !pane.reports.choosing_agent;
            cx.notify();
        })),
    );
    let mut agent_choices = div().flex().flex_col().flex_shrink_0();
    if pane.reports.choosing_agent {
        if let Some(search) = &pane.search {
            agent_choices = agent_choices.child(
                ui::view_toolbar()
                    .gap(px(t.space.s8))
                    .child(ui::text(div().text_color(c.muted), &t.text.meta).child("Find agent"))
                    .child(div().flex_1().min_w_0().child(search.clone())),
            );
        }
        let mut choices = div()
            .id("report-agents")
            .max_h(px(180.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(t.space.s12));
        let filter = pane.query.trim().to_lowercase();
        for agent in std::iter::once(None).chain(
            agents
                .iter()
                .filter(|name| name.to_lowercase().contains(&filter))
                .cloned()
                .map(Some),
        ) {
            let name = agent.as_deref().unwrap_or("All agents");
            choices = choices.child(
                ui::row(
                    SharedString::from(format!("reports-agent-{name}")),
                    pane.reports.agent == agent,
                )
                .flex_shrink_0()
                .child(name.to_owned())
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.reports.select_agent(agent.clone());
                    pane.query.clear();
                    if let Some(search) = &pane.search {
                        search.update(cx, |field, cx| field.clear(cx));
                    }
                    pane.refetch(true, cx);
                })),
            );
        }
        agent_choices = agent_choices.child(choices);
    }
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(toolbar)
        .child(agent_choices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_answer_keeps_its_text_but_observes_a_correction() {
        let item = |answer: &str, superseded_by| ReportItem {
            id: 7,
            author: "worker".into(),
            created_at: "2026-10-24T10:00:00Z".into(),
            content: ReportContent::Response {
                question: "What happened?".into(),
                answer: answer.into(),
                action: "Read the result.".into(),
                superseded_by,
            },
        };
        let mut state = State {
            open: Some(item("Original answer", None)),
            ..State::default()
        };
        state.observe(&ReportPage {
            items: vec![item("A different body", Some(9))],
            next: None,
            agents: Vec::new(),
        });
        let open = state.open.unwrap();
        assert_eq!(label(&open), "corrected by #9");
        assert!(
            matches!(open.content, ReportContent::Response { answer, .. } if answer == "Original answer")
        );
    }

    #[test]
    fn changing_agent_restarts_the_current_feed() {
        let mut state = State {
            query: ReportQuery::Responses(Some(7)),
            previous: vec![ReportQuery::Responses(None)],
            choosing_agent: true,
            ..State::default()
        };
        state.select_agent(Some("worker".into()));
        assert_eq!(state.query, ReportQuery::Responses(None));
        assert_eq!(state.agent.as_deref(), Some("worker"));
        assert!(state.previous.is_empty());
        assert!(!state.choosing_agent);
        state.select_agent(None);
        assert!(state.agent.is_none());
    }
}
