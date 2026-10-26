//! Policies: the layers a brief is built from, team-wide. The base brief,
//! the global policies in order, each tag's policies and the swarm briefs;
//! policies never name an agent. Choosing one reads it in this pane.
//!
//! The policy reader here is shared with an agent's Policies section.

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{PREVIEW_LIMIT, PolicyLibrary, Source};

use super::{Target, ViewPane, markdown, mono, reader};
use crate::desktop::{
    components::{self as ui, Tone},
    design::{colors, tokens},
};

/// A policy's title: its first heading, else its name.
pub(super) fn title(source: &Source) -> String {
    source
        .text
        .as_deref()
        .and_then(|text| {
            markdown::parse(text)
                .into_iter()
                .find_map(|block| match block {
                    markdown::Block::Heading(_, text) => Some(text),
                    _ => None,
                })
        })
        .unwrap_or_else(|| source.name.to_string())
}

/// Who a source reaches, as the pill on its row: `every agent`, `tag X`,
/// `swarm`.
pub(super) fn scope(source: &Source) -> Div {
    let parts: Vec<&str> = source.scope.split(", ").collect();
    let mut pills = div().flex().gap(px(tokens().space.s4));
    if parts.iter().any(|part| matches!(*part, "base" | "global")) {
        return pills.child(ui::tag("every agent", Tone::Plain));
    }
    for part in parts {
        pills = pills.child(match part.strip_prefix("tag:") {
            Some(tag) => ui::tag(format!("tag {tag}"), Tone::Tag),
            None => ui::tag(part.to_owned(), Tone::Quiet),
        });
    }
    pills
}

pub(super) fn targets(library: &PolicyLibrary) -> Vec<Target> {
    library
        .all()
        .into_iter()
        .map(|source| Target::Policy(source.path.to_string()))
        .collect()
}

pub(super) fn body(
    pane: &ViewPane,
    library: &PolicyLibrary,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let header = ui::view_header("align-left", "Policies").child("the layers of a brief");
    if let Some(open) = &pane.open
        && let Target::Policy(path) = &open.target
    {
        let all = library.all();
        if let Some(at) = all.iter().position(|source| source.path.as_ref() == path) {
            return reader::render(pane, header, page(&all, at, "Policies".into(), false), cx);
        }
    }
    index(library, header, cx)
}

fn index(library: &PolicyLibrary, header: Div, cx: &mut Context<ViewPane>) -> AnyElement {
    let mut number = 0;
    let mut card = |sources: &[Source], numbered: bool, cx: &mut Context<ViewPane>| {
        let mut card = ui::group_card();
        for (at, source) in sources.iter().enumerate() {
            number += 1;
            let path = source.path.to_string();
            card = card.child(
                ui::group_row(
                    SharedString::from(format!("policy-{number}")),
                    at == 0,
                    numbered.then(|| ui::number_tile((at + 1).to_string()).into_any_element()),
                    source.name.to_string(),
                    title(source),
                )
                .on_click(
                    cx.listener(move |pane, _, _, cx| pane.open(Target::Policy(path.clone()), cx)),
                ),
            );
        }
        card
    };
    let mut column = div()
        .child(ui::group_title(
            "Base brief",
            "Every ordinary agent starts with it. Policies never name an agent: each is \
             global or belongs to a tag.",
        ))
        .child(card(&library.base, false, cx))
        .child(ui::group_title(
            "Global policies",
            "Every agent receives these after the base brief, in this order.",
        ))
        .child(card(&library.global, true, cx))
        .child(ui::group_title(
            "Tag policies",
            "Added for the agents that carry the tag, after the global ones.",
        ));
    for (tag, sources) in &library.tags {
        let carried = library.carriers.get(tag).copied().unwrap_or(0);
        column = column
            .child(ui::group_label(
                ui::tag(tag.clone(), Tone::Tag),
                format!("{carried} agent{}", if carried == 1 { "" } else { "s" }),
            ))
            .child(card(sources, true, cx));
    }
    column = column.child(ui::group_title(
        "Swarm briefs",
        "Replace the base brief for the members of a swarm.",
    ));
    column = if library.swarm.is_empty() {
        column.child(ui::group_note("Shown once an agent is in a swarm."))
    } else {
        column.child(card(&library.swarm, false, cx))
    };
    if library.read < library.agents {
        column = column.child(ui::group_note(format!(
            "Gathered from the briefs of {} of {} agents (at most {PREVIEW_LIMIT}).",
            library.read, library.agents
        )));
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(ui::grouped_page("policy-rows", column))
        .child(ui::view_footer(
            &[("click", "read the policy")],
            "assigned with backbone templates use NAME [--tag TAG]",
        ))
        .into_any_element()
}

/// The policy at `at` of `sources` as a reader page, returning to `back`.
/// Whether it applies is one agent's fact, so only `one_agent` says it.
pub(super) fn page<'a>(
    sources: &[&'a Source],
    at: usize,
    back: String,
    one_agent: bool,
) -> reader::Page<'a> {
    let (t, c) = (tokens(), colors());
    let source = sources[at];
    let card = ui::info_card().child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(t.space.s14))
            .gap_y(px(t.space.s6))
            .text_color(c.muted)
            .child(mono(source.path.to_string()).text_color(c.text_2))
            .child(scope(source))
            .when(one_agent, |facts| {
                facts.child(if source.applied {
                    "applied at the next start"
                } else {
                    "not applied"
                })
            }),
    );
    reader::Page {
        back,
        name: Some(source.name.to_string()),
        at: Some(at),
        count: sources.len(),
        card,
        text: match &source.text {
            Some(text) => reader::Text::Ready { text, cut: false },
            None => reader::Text::Failed("Backbone could not read this file."),
        },
    }
}
