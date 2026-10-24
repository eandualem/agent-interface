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
    previous: Vec<ReportQuery>,
    /// Records are immutable: keep the opened answer through list refreshes.
    pub open: Option<ReportItem>,
}

impl State {
    /// Authored text is immutable; correction metadata can change after opening.
    pub fn observe(&mut self, page: &ReportPage) {
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

pub(super) fn refresh_toolbar(cx: &mut Context<ViewPane>) -> gpui::Div {
    ui::view_toolbar().child(
        ui::chip("reports-reset", "Refresh", false).on_click(cx.listener(|pane, _, _, cx| {
            pane.reports.query = first(&pane.reports.query);
            pane.reports.previous.clear();
            pane.refetch(true, cx);
        })),
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
                .child(markdown::render(&markdown::parse(text)))
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
    let mut toolbar = ui::view_toolbar().flex_wrap().gap(px(t.space.s6));
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
        .child(toolbar)
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
        });
        let open = state.open.unwrap();
        assert_eq!(label(&open), "corrected by #9");
        assert!(
            matches!(open.content, ReportContent::Response { answer, .. } if answer == "Original answer")
        );
    }
}
