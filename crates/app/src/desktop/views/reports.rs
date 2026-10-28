//! Read authored updates and answers in the same native pane as other views.

use agent_interface_roster::views::{
    ReportContent, ReportItem, ReportPage, ReportQuery, epoch_seconds,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use super::{Shown, ViewPane, clock::Local, line, markdown, seen, since};
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
    /// The CLI each configured agent runs in, for the reader's badge.
    runtimes: std::collections::BTreeMap<String, String>,
    pub(super) previous: Vec<ReportQuery>,
    /// Records are immutable: keep the opened answer through list refreshes.
    pub open: Option<ReportItem>,
    /// The list the open record was chosen from, for moving to its
    /// neighbours; an open answer's own refresh reads only that answer.
    feed: Option<ReportPage>,
}

impl State {
    fn select_agent(&mut self, agent: Option<String>) {
        self.agent = agent;
        self.query = match &self.query {
            ReportQuery::Documents(..) => ReportQuery::Documents(None, None),
            _ => first(&self.query),
        };
        self.previous.clear();
        self.open = None;
        self.choosing_agent = false;
    }

    /// Authored text is immutable; correction metadata can change after opening.
    pub fn observe(&mut self, page: &ReportPage) {
        if !page.agents.is_empty() {
            self.agents = page.agents.clone();
        }
        // The open record's registration and the runtimes come from the same
        // read, so a name re-registered meanwhile cannot lend its runtime; a
        // read without the roster clears them rather than keep older ones.
        if self.open.is_none() {
            self.runtimes = page.runtimes.clone();
            self.feed = Some(page.clone());
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
        ReportQuery::Documents(_, author) => ReportQuery::Documents(None, author.clone()),
    }
}

/// A section's name and the first page it reads.
type Section = (&'static str, fn() -> ReportQuery);

/// The sections of Reports & updates, in the order the bar shows them.
const SECTIONS: [Section; 4] = [
    ("Waiting on you", || ReportQuery::Latest(None)),
    ("Recent updates", || ReportQuery::Recent(None)),
    ("Responses", || ReportQuery::Responses(None)),
    ("Documents", || ReportQuery::Documents(None, None)),
];

/// The name of the section `query` reads.
fn section_name(query: &ReportQuery) -> &'static str {
    SECTIONS
        .iter()
        .find(|(_, make)| std::mem::discriminant(&make()) == std::mem::discriminant(query))
        .map_or("Reports & updates", |(name, _)| name)
}

/// The view's header: its title, and the live note once data has arrived.
fn header(live: bool) -> gpui::Div {
    ui::view_header("message-square", "Reports & updates")
        .when(live, |el| el.child(ui::live_note()))
}

/// A section with its own view, such as Sheets or Current plans: the same
/// header and section bar above it.
pub(super) fn section_frame(
    pane: &ViewPane,
    section: impl IntoElement,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header(true))
        .child(controls(pane, &pane.reports.agents, cx))
        .child(div().flex_1().min_h_0().flex().flex_col().child(section))
        .into_any_element()
}

/// Something opened from a section, such as a plan: the header, its own
/// toolbar in place of the section bar, and the content.
pub(super) fn plain_frame(toolbar: gpui::Div, content: impl IntoElement) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header(false))
        .child(toolbar)
        .child(content)
        .into_any_element()
}

pub(super) fn refresh_toolbar(pane: &ViewPane, cx: &mut Context<ViewPane>) -> gpui::Div {
    controls(pane, &pane.reports.agents, cx)
}

fn label(item: &ReportItem) -> String {
    match &item.content {
        ReportContent::Document(_) => "document".into(),
        ReportContent::Progress { status, .. } => status.clone(),
        ReportContent::Response {
            superseded_by: Some(id),
            ..
        } => format!("corrected by #{id}"),
        ReportContent::Response { .. } => "response".into(),
    }
}

/// A report's authored state as a mark: shape first, colour second.
pub(super) fn mark(item: &ReportItem) -> Option<gpui::Div> {
    let ReportContent::Progress { status, .. } = &item.content else {
        return None;
    };
    Some(if item.needs_owner() || status == "blocked" {
        ui::agent_symbol(ui::AgentState::NeedsYou)
    } else {
        match status.as_str() {
            "active" => ui::agent_symbol(ui::AgentState::Working),
            "complete" => ui::done_symbol(),
            "inactive" => ui::agent_symbol(ui::AgentState::Idle),
            _ => ui::agent_symbol(ui::AgentState::Unknown),
        }
    })
}

/// A report's authored state as a tag.
fn state_tag(item: &ReportItem) -> gpui::Div {
    let ReportContent::Progress { status, .. } = &item.content else {
        return ui::tag(label(item), ui::Tone::Quiet);
    };
    if item.needs_owner() {
        return ui::tag("blocked · you", ui::Tone::Attention);
    }
    let tone = match status.as_str() {
        "active" => ui::Tone::Tag,
        "blocked" => ui::Tone::Attention,
        "complete" => ui::Tone::Done,
        _ => ui::Tone::Quiet,
    };
    ui::tag(status.clone(), tone)
}

/// "Today 14:32", "Mon 27 Oct 09:05".
fn when(created_at: &str) -> String {
    let local = epoch_seconds(created_at).and_then(Local::at);
    match (local, Local::now()) {
        (Some(local), Some(now)) => format!("{} {}", local.day_name(&now), local.time()),
        _ => created_at.to_owned(),
    }
}

/// Records grouped by their local day, newest first as the feed gives them.
fn by_day<'a>(
    items: impl Iterator<Item = &'a ReportItem>,
) -> Vec<(Option<Local>, Vec<&'a ReportItem>)> {
    let mut days: Vec<(Option<Local>, Vec<&ReportItem>)> = Vec::new();
    for item in items {
        let local = epoch_seconds(&item.created_at).and_then(Local::at);
        match days.last_mut() {
            Some((day, items))
                if match (&*day, &local) {
                    (Some(day), Some(local)) => day.same_day(local),
                    (None, None) => true,
                    _ => false,
                } =>
            {
                items.push(item)
            }
            _ => days.push((local, vec![item])),
        }
    }
    days
}

/// A day's label above its card: the day's name, then its date and a count.
fn day_label(day: Option<Local>, count: usize, noun: &str) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let now = Local::now();
    let (name, note) = match (day, now) {
        (Some(day), Some(now)) => {
            let name = day.day_name(&now);
            let note = if name == day.date() {
                format!("{count} {noun}")
            } else {
                format!("{} · {count} {noun}", day.date())
            };
            (name, note)
        }
        _ => ("Undated".to_owned(), format!("{count} {noun}")),
    };
    ui::group_label(
        div()
            .text_color(c.text_2)
            .font_weight(t.text.row_selected.weight())
            .child(name),
        note,
    )
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

fn open_record(pane: &mut ViewPane, item: ReportItem, cx: &mut Context<ViewPane>) {
    seen::mark(&item);
    pane.documents.clear();
    pane.reports.open = Some(item);
    pane.refetch(false, cx);
}

/// The records of `page` a reader can step through, in list order.
pub(super) fn listed<'a>(pane: &ViewPane, page: &'a ReportPage) -> Vec<&'a ReportItem> {
    let waiting = matches!(pane.reports.query, ReportQuery::Latest(_));
    page.items
        .iter()
        .filter(|item| !waiting || item.needs_owner())
        .collect()
}

/// Open the newer (`by < 0`) or older record beside the open one.
pub(super) fn step(pane: &mut ViewPane, page: &ReportPage, by: isize, cx: &mut Context<ViewPane>) {
    let Some(open) = &pane.reports.open else {
        return;
    };
    let page = pane.reports.feed.clone().unwrap_or_else(|| page.clone());
    let listed = listed(pane, &page);
    let Some(at) = listed.iter().position(|item| item.id == open.id) else {
        return;
    };
    if let Some(item) = at.checked_add_signed(by).and_then(|next| listed.get(next)) {
        let item = (*item).clone();
        open_record(pane, item, cx);
    }
}

/// A reader's frame: the header and toolbar across the top, then the record
/// with its footer, and its conversation on the right.
pub(super) fn reader_frame(
    pane: &ViewPane,
    toolbar: gpui::Div,
    content: impl IntoElement,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header(false))
        .child(toolbar)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_1().min_w_0().flex().flex_col().child(content))
                .children(pane.conversation.clone()),
        )
        .into_any_element()
}

/// The reader's toolbar: back to the section, the record's place in it, more
/// from its author and newer and older.
fn reader_toolbar(
    pane: &ViewPane,
    page: &ReportPage,
    item: &ReportItem,
    cx: &mut Context<ViewPane>,
) -> gpui::Div {
    let page = pane.reports.feed.as_ref().unwrap_or(page);
    let listed = listed(pane, page);
    let at = listed.iter().position(|listed| listed.id == item.id);
    let author = item.author.clone();
    ui::view_toolbar()
        .child(
            ui::back_button("reports-back", section_name(&pane.reports.query)).on_click(
                cx.listener(|pane, _, _, cx| {
                    pane.reports.open = None;
                    pane.refetch(true, cx);
                }),
            ),
        )
        .children(at.map(|at| super::mono(format!("{} of {}", at + 1, listed.len()))))
        .child(div().flex_1())
        .child(
            ui::chip("reports-more", format!("More from {author}"), false).on_click(cx.listener(
                move |pane, _, _, cx| {
                    pane.reports.open = None;
                    pane.reports.select_agent(Some(author.clone()));
                    pane.refetch(true, cx);
                },
            )),
        )
        .child(
            ui::icon_button("reports-newer", "chevron-up")
                .on_click(cx.listener(|pane, _, _, cx| pane.step_record(-1, cx))),
        )
        .child(
            ui::icon_button("reports-older", "chevron-down")
                .on_click(cx.listener(|pane, _, _, cx| pane.step_record(1, cx))),
        )
}

/// The CLI the record's author runs in, while the provider resolves the
/// author to a current agent: a retained name may since belong to another.
fn runtime(pane: &ViewPane, item: &ReportItem) -> Option<gpui::Div> {
    let runtime = pane
        .reports
        .runtimes
        .get(&item.author)
        .filter(|_| item.registered)?;
    (!runtime.is_empty()).then(|| ui::badge(runtime.clone()))
}

/// Where a link leads, short enough for the right of its row: the site and
/// the last part of the path, such as `github.com · issues/516`.
fn place(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.strip_prefix("www.").unwrap_or(host);
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.len() {
        0 => host.to_owned(),
        1 => format!("{host} · {}", parts[0]),
        n => format!("{host} · {}/{}", parts[n - 2], parts[n - 1]),
    }
}

/// Only web addresses open from a report.
fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// An open report or answer, read-only, with its conversation beside it.
fn reader(
    pane: &ViewPane,
    page: &ReportPage,
    item: &ReportItem,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    // The board's reading styles: body text, the goal as a title, and the
    // person's question; each paragraph 8 px apart.
    let look = |style, line_height| markdown::Look {
        style,
        color: c.text,
        line_height,
        gap: t.space.s8,
    };
    let body = look(&t.text.doc_body, 1.55);
    let authored = |text: &str, look: &markdown::Look, cx: &mut Context<ViewPane>| {
        markdown::render_in(&markdown::parse_authored(text), look, pane, cx)
    };
    let mut column = ui::reader_column().gap(px(t.space.s16));
    match &item.content {
        ReportContent::Document(_) => unreachable!("document has an isolated reader"),
        ReportContent::Progress { sections, .. } => {
            column = column.gap_0().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .pb(px(t.space.s10))
                    .children(mark(item))
                    .child(super::mono(item.author.clone()).text_color(c.text))
                    .children(runtime(pane, item))
                    .child(state_tag(item))
                    .child(div().flex_1())
                    .child(
                        ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                            "{} · #{}",
                            when(&item.created_at),
                            item.id
                        )),
                    ),
            );
            let mut links = Vec::new();
            for (name, value) in sections {
                links.extend(value.links.iter().cloned());
                if *name == "Goal" {
                    column = column.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s4))
                            .pb(px(t.space.s16))
                            .child(ui::reader_label("GOAL"))
                            .child(authored(&value.text, &look(&t.text.doc_title, 1.3), cx)),
                    );
                    continue;
                }
                let body = if *name == "Blocker" && value.kind == "owner" {
                    ui::callout(
                        ui::Callout::Attention,
                        Some("Waiting on you"),
                        authored(&value.text, &body, cx),
                    )
                    .into_any_element()
                } else if value.text.trim().is_empty() {
                    div().text_color(c.muted).child("None.").into_any_element()
                } else {
                    authored(&value.text, &body, cx).into_any_element()
                };
                column = column.child(ui::reader_section(name.to_uppercase(), body));
            }
            if !links.is_empty() {
                let mut list = div().flex().flex_col().gap(px(t.space.s6));
                for (index, link) in links.iter().enumerate() {
                    let url = link.url.clone();
                    list = list.child(
                        ui::reference(
                            SharedString::from(format!("report-link-{index}")),
                            link.title.clone(),
                            place(&link.url),
                        )
                        .when(web(&url), |row| {
                            row.on_click(move |_, _, cx| cx.open_url(&url))
                        }),
                    );
                }
                column = column.child(ui::reader_section("LINKS", list));
            }
        }
        ReportContent::Response {
            question,
            answer,
            action,
            superseded_by,
        } => {
            if let Some(id) = superseded_by {
                column = column.child(ui::notice(
                    "history",
                    format!(
                        "{} published a correction to this answer: #{id}.",
                        item.author
                    ),
                    None,
                ));
            }
            let labelled = |label: &'static str, body: AnyElement| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .child(ui::reader_label(label))
                    .child(body)
            };
            column =
                column
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s8))
                            .child(super::mono(item.author.clone()).text_color(c.text))
                            .children(runtime(pane, item))
                            .child(div().flex_1())
                            .child(ui::text(div().text_color(c.muted), &t.text.meta).child(
                                format!("answered {} · #{}", when(&item.created_at), item.id),
                            )),
                    )
                    .child(labelled(
                        "YOU ASKED",
                        div()
                            .px(px(t.space.s14))
                            .py(px(t.space.s12))
                            .rounded(px(t.radius.popover))
                            .bg(c.field)
                            .border(px(t.size.hairline))
                            .border_color(c.line)
                            .child(authored(question, &look(&t.text.quote, 1.45), cx))
                            .into_any_element(),
                    ))
                    .child(labelled(
                        "ANSWER",
                        authored(answer, &body, cx).into_any_element(),
                    ))
                    .child(labelled(
                        "WHAT FOLLOWS",
                        ui::callout(ui::Callout::Follows, None, authored(action, &body, cx))
                            .into_any_element(),
                    ));
        }
    }
    let note = match item.content {
        ReportContent::Response { .. } => "authored answer · read-only",
        _ => "authored record · read-only",
    };
    let mut hints = vec![("esc", "back"), ("[ ]", "newer · older")];
    if pane.conversation.is_some() {
        hints.push(("C", "conversation"));
    }
    let content = div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .id("report-document")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .justify_center()
                .items_start()
                .px(px(t.space.s24))
                .pt(px(t.space.s18))
                .pb(px(t.space.s24))
                .child(column),
        )
        .child(ui::view_footer(&hints, note));
    reader_frame(pane, reader_toolbar(pane, page, item, cx), content)
}

pub(super) fn body(pane: &ViewPane, page: &ReportPage, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    if let Some(item) = &pane.reports.open {
        return reader(pane, page, item, cx);
    }
    let listed = listed(pane, page);
    let mut column = div();
    let rows = |items: &[&ReportItem], cx: &mut Context<ViewPane>| {
        let mut card = ui::group_card();
        for (index, item) in items.iter().enumerate() {
            let open = (*item).clone();
            let ago = since(epoch_seconds(&item.created_at));
            let id = SharedString::from(format!("report-{}", item.id));
            let row = match &item.content {
                ReportContent::Progress { sections, .. } => {
                    let summary = if item.needs_owner() {
                        sections
                            .iter()
                            .find(|(name, _)| *name == "Blocker")
                            .map_or(item.title(), |(_, value)| value.text.as_str())
                    } else {
                        item.title()
                    };
                    ui::record_row(
                        id,
                        index == 0,
                        false,
                        !seen::is_seen(item),
                        mark(item).unwrap_or_else(|| div()),
                        item.author.clone(),
                        line(summary),
                        state_tag(item),
                        ago,
                    )
                }
                ReportContent::Response {
                    question,
                    answer,
                    superseded_by,
                    ..
                } => ui::question_row(
                    id,
                    index == 0,
                    false,
                    !seen::is_seen(item),
                    item.author.clone(),
                    line(question),
                    line(answer),
                    superseded_by.map(|id| ui::tag(format!("corrected · #{id}"), ui::Tone::Tag)),
                    ago,
                ),
                ReportContent::Document(_) => unreachable!("documents are on a shelf"),
            };
            card =
                card.child(row.on_click(
                    cx.listener(move |pane, _, _, cx| open_record(pane, open.clone(), cx)),
                ));
        }
        card
    };
    match pane.reports.query {
        ReportQuery::Latest(_) => {
            column = column.child(ui::group_label(
                div()
                    .text_color(c.text_2)
                    .font_weight(t.text.row_selected.weight())
                    .child("Waiting on you"),
                "owner blockers from each agent's latest report",
            ));
            if listed.is_empty() {
                column = column.child(ui::group_note("Nothing is waiting on you on this page."));
            } else {
                column = column.child(rows(&listed, cx));
            }
        }
        ReportQuery::Documents(..) => {
            let now = agent_interface_roster::views::now() as f64;
            let (recent, earlier): (Vec<&ReportItem>, Vec<&ReportItem>) =
                listed.iter().partition(|item| {
                    epoch_seconds(&item.created_at).is_some_and(|at| now - at < 7.0 * 86_400.0)
                });
            for (name, note, items) in [
                ("This week", "latest revision of each document", recent),
                ("Earlier", "", earlier),
            ] {
                if items.is_empty() {
                    continue;
                }
                let count = items.len();
                let note = if note.is_empty() {
                    format!("{count} {}", plural(count, "document", "documents"))
                } else {
                    note.to_owned()
                };
                column = column.child(ui::group_label(
                    div()
                        .text_color(c.text_2)
                        .font_weight(t.text.row_selected.weight())
                        .child(name),
                    note,
                ));
                let mut shelf = ui::shelf();
                for item in items {
                    let ReportContent::Document(metadata) = &item.content else {
                        continue;
                    };
                    let open = (*item).clone();
                    let mut tags = Vec::new();
                    if !seen::is_seen(item) {
                        let label = if seen::is_update(item) {
                            "updated"
                        } else {
                            "new"
                        };
                        tags.push(ui::tag(label, ui::Tone::Tag));
                    }
                    shelf = shelf.child(
                        ui::shelf_card(
                            SharedString::from(format!("document-{}", item.id)),
                            "file-text",
                            false,
                            tags,
                            metadata.title.clone(),
                            metadata.author().to_owned(),
                            format!("{} ago", since(epoch_seconds(&item.created_at))),
                            format!("{} KB", metadata.content_bytes.div_ceil(1024)),
                        )
                        .on_click(
                            cx.listener(move |pane, _, _, cx| open_record(pane, open.clone(), cx)),
                        ),
                    );
                }
                column = column.child(shelf);
            }
            if listed.is_empty() {
                column = column.child(ui::group_note("No documents on this page."));
            }
        }
        ReportQuery::Recent(_) | ReportQuery::Responses(_) => {
            let (one, many) = if matches!(pane.reports.query, ReportQuery::Responses(_)) {
                ("answer", "answers")
            } else {
                ("update", "updates")
            };
            for (day, items) in by_day(listed.iter().copied()) {
                column = column
                    .child(day_label(day, items.len(), plural(items.len(), one, many)))
                    .child(rows(&items, cx));
            }
            if listed.is_empty() {
                column = column.child(ui::group_note("No records on this page."));
            }
        }
    }
    let mut paging = div()
        .flex()
        .justify_center()
        .gap(px(t.space.s8))
        .pt(px(t.space.s14));
    if !pane.reports.previous.is_empty() {
        paging = paging.child(
            ui::chip("reports-previous", "Newer", false).on_click(cx.listener(|pane, _, _, cx| {
                if let Some(query) = pane.reports.previous.pop() {
                    pane.reports.query = query;
                    pane.refetch(true, cx);
                }
            })),
        );
    }
    if let Some(next) = page.next.clone() {
        paging = paging.child(
            ui::chip("reports-next", "Show older", false).on_click(cx.listener(
                move |pane, _, _, cx| {
                    pane.reports.previous.push(pane.reports.query.clone());
                    pane.reports.query = next.clone();
                    pane.refetch(true, cx);
                },
            )),
        );
    }
    let note = match pane.reports.query {
        ReportQuery::Latest(_) => "latest authored owner blockers · age is not resolution",
        ReportQuery::Recent(_) => "authored progress reports · newest first · unread in bold",
        ReportQuery::Responses(_) => "your questions, newest answer first · unread in bold",
        ReportQuery::Documents(..) => "new and updated revisions are marked until you open them",
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header(true))
        .child(controls(pane, &page.agents, cx))
        .child(ui::grouped_page("reports-rows", column.child(paging)))
        .child(ui::view_footer(&[], note))
        .into_any_element()
}

/// The section bar, the agent filter and refresh.
fn controls(pane: &ViewPane, agents: &[String], cx: &mut Context<ViewPane>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut sections = ui::segments();
    for (index, (name, make)) in SECTIONS.into_iter().enumerate() {
        if index == 1 && pane.backbone.is_some() {
            sections = sections.child(
                ui::segment(
                    "reports-Current plans",
                    "Current plans",
                    pane.shown == Shown::Plans,
                )
                .on_click(cx.listener(|pane, _, _, cx| pane.show_plans(cx))),
            );
        }
        let query = make();
        sections = sections.child(
            ui::segment(
                SharedString::from(format!("reports-{name}")),
                name,
                pane.shown == Shown::Records
                    && std::mem::discriminant(&pane.reports.query)
                        == std::mem::discriminant(&query),
            )
            .on_click(cx.listener(move |pane, _, _, cx| pane.show_records(make(), cx))),
        );
    }
    if pane.backbone.is_some() {
        sections = sections.child(
            ui::segment("reports-Sheets", "Sheets", pane.shown == Shown::Sheets)
                .on_click(cx.listener(|pane, _, window, cx| pane.show_sheets(window, cx))),
        );
    }
    let agent = pane.reports.agent.as_deref().unwrap_or(
        if matches!(pane.reports.query, ReportQuery::Documents(_, Some(_))) {
            "Selected author"
        } else if matches!(pane.reports.query, ReportQuery::Documents(..)) {
            "All authors"
        } else {
            "All agents"
        },
    );
    let records = pane.shown == Shown::Records;
    // Current plans filter by agent too; their list refreshes on its own.
    let by_agent = records || pane.shown == Shown::Plans;
    let toolbar =
        ui::view_toolbar()
            .h_auto()
            .min_h(px(t.size.view_toolbar))
            .py(px(t.space.s6))
            .flex_wrap()
            .child(sections)
            .child(div().flex_1())
            .when(by_agent, |toolbar| {
                toolbar.child(
                    ui::chip("reports-agent", "", pane.reports.agent.is_some())
                        .gap(px(t.space.s6))
                        .child(ui::icon("user", t.size.icon - 2.0, c.muted))
                        .child(agent.to_owned())
                        .child(ui::icon("chevron-down", t.size.icon - 2.0, c.muted))
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.reports.choosing_agent = !pane.reports.choosing_agent;
                            cx.notify();
                        })),
                )
            })
            // Sheets has its own refresh beside the workbook.
            .when(records, |toolbar| {
                toolbar.child(ui::icon_button("reports-refresh", "refresh-cw").on_click(
                    cx.listener(|pane, _, _, cx| {
                        pane.reports.query = first(&pane.reports.query);
                        pane.reports.previous.clear();
                        pane.refetch(true, cx);
                    }),
                ))
            });
    let mut agent_choices = div().flex().flex_col().flex_shrink_0();
    if pane.reports.choosing_agent && by_agent {
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
            author_id: None,
            registered: false,
            provider_url: String::new(),
            created_at: "2026-10-24T10:00:00Z".into(),
            conversation_id: None,
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
            runtimes: Default::default(),
        });
        let open = state.open.unwrap();
        assert_eq!(label(&open), "corrected by #9");
        assert!(
            matches!(open.content, ReportContent::Response { answer, .. } if answer == "Original answer")
        );
    }

    #[test]
    fn an_open_answer_keeps_the_list_it_was_chosen_from() {
        let item = |id| ReportItem {
            id,
            author: "worker".into(),
            author_id: None,
            registered: false,
            provider_url: String::new(),
            created_at: "2026-10-24T10:00:00Z".into(),
            conversation_id: None,
            content: ReportContent::Response {
                question: "What happened?".into(),
                answer: "It worked.".into(),
                action: "Nothing.".into(),
                superseded_by: None,
            },
        };
        let page = |ids: &[u64]| ReportPage {
            items: ids.iter().map(|id| item(*id)).collect(),
            next: None,
            agents: Vec::new(),
            runtimes: Default::default(),
        };
        let mut state = State::default();
        state.observe(&page(&[3, 2, 1]));
        state.open = Some(item(2));
        state.observe(&page(&[2]));
        assert_eq!(state.feed.unwrap().items.len(), 3);
    }

    #[test]
    fn an_open_record_keeps_the_runtimes_its_feed_read() {
        let page = |runtime: &str| ReportPage {
            items: Vec::new(),
            next: None,
            agents: vec!["worker".into()],
            runtimes: [("worker".into(), runtime.into())].into(),
        };
        let mut state = State::default();
        state.observe(&page("codex"));
        state.open = Some(ReportItem {
            id: 1,
            author: "worker".into(),
            author_id: None,
            registered: true,
            provider_url: String::new(),
            created_at: String::new(),
            conversation_id: None,
            content: ReportContent::Response {
                question: String::new(),
                answer: String::new(),
                action: String::new(),
                superseded_by: None,
            },
        });
        // The name was re-registered to another agent while the record was open.
        state.observe(&page("claude"));
        assert_eq!(state.runtimes["worker"], "codex");
        // Closed again, a read whose roster failed leaves no runtime to show.
        state.open = None;
        state.observe(&ReportPage {
            runtimes: Default::default(),
            ..page("claude")
        });
        assert!(state.runtimes.is_empty());
    }

    #[test]
    fn a_link_names_its_site_and_where_on_it() {
        assert_eq!(
            place("https://github.com/eandualem/agent-backbone/issues/516"),
            "github.com · issues/516"
        );
        assert_eq!(place("https://www.example.com/"), "example.com");
        assert_eq!(place("https://example.com/docs"), "example.com · docs");
        assert!(web("https://example.com") && !web("file:///etc/passwd"));
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
