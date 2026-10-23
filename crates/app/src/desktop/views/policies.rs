//! Policies: the layers a brief is built from, team-wide. The base brief,
//! the global policies in order, each tag's policies and the swarm briefs;
//! policies never name an agent. Choosing one reads it in this pane.
//!
//! The policy reader here is shared with an agent's Policies section.

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{PREVIEW_LIMIT, PolicyLibrary, Source};

use super::{Target, ViewPane, list, markdown, mono, reader};
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
        .unwrap_or_else(|| source.name.clone())
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
        .map(|source| Target::Policy(source.path.clone()))
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
        if let Some(at) = all.iter().position(|source| &source.path == path) {
            return reader::render(pane, header, page(&all, at, "Policies".into()), cx);
        }
    }
    index(pane, library, header, cx)
}

fn index(
    pane: &ViewPane,
    library: &PolicyLibrary,
    header: Div,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let arrow = || div().text_color(c.faint).child("→");
    let intro = ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .flex_col()
            .gap(px(t.space.s8))
            .px(px(t.space.s16))
            .py(px(t.space.s12))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted),
        &t.text.status,
    )
    .child(
        "How a brief is built, top to bottom. Policies never name an agent: each is global or \
         belongs to a tag.",
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s6))
            .child(ui::tag(
                "base, or a swarm brief for swarm members",
                Tone::Plain,
            ))
            .child(arrow())
            .child(ui::tag("global, in order", Tone::Plain))
            .child(arrow())
            .child(ui::tag("the agent's tags", Tone::Tag)),
    );
    let narrow = pane.narrow();
    let mut rows = list("policy-rows").pt(px(t.space.s2));
    let mut number = 0;
    let mut row = |rows: gpui::Stateful<Div>, order: String, source: &Source| {
        number += 1;
        let path = source.path.clone();
        rows.child(
            ui::list_row(
                SharedString::from(format!("policy-{number}")),
                (!order.is_empty()).then(|| ui::number_tile(order)),
                source.name.clone(),
                title(source),
                c.muted,
                div(),
                narrow,
            )
            .on_click(
                cx.listener(move |pane, _, _, cx| pane.open(Target::Policy(path.clone()), cx)),
            ),
        )
    };
    rows = rows.child(ui::layer_heading(
        "1",
        "BASE BRIEF",
        "base.md",
        "every ordinary agent",
        false,
    ));
    for source in &library.base {
        rows = row(rows, String::new(), source);
    }
    rows = rows.child(ui::layer_heading(
        "2",
        "GLOBAL POLICIES",
        "agents.shared_policy",
        "every ordinary agent, in this order",
        false,
    ));
    for (at, source) in library.global.iter().enumerate() {
        rows = row(rows, (at + 1).to_string(), source);
    }
    rows = rows.child(ui::layer_heading(
        "3",
        "TAG POLICIES",
        "agents.tag_policy",
        "agents carrying the tag",
        false,
    ));
    for (tag, sources) in &library.tags {
        let carried = library.carriers.get(tag).copied().unwrap_or(0);
        rows = rows.child(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s8))
                    .pl(px(t.space.s8 + t.size.layer_tile + t.space.s10))
                    .pt(px(t.space.s6))
                    .pb(px(t.space.s2))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(ui::tag(tag.clone(), Tone::Tag))
            .child(format!(
                "carried by {carried} agent{}",
                if carried == 1 { "" } else { "s" }
            )),
        );
        for source in sources {
            rows = row(rows, String::new(), source);
        }
    }
    rows = rows.child(ui::layer_heading(
        "1b",
        "SWARM BRIEFS",
        "swarm/",
        "replace the base brief for swarm members",
        false,
    ));
    for source in &library.swarm {
        rows = row(rows, String::new(), source);
    }
    if library.swarm.is_empty() {
        rows = rows.child(quiet("Shown once an agent is in a swarm."));
    }
    if library.read < library.agents {
        rows = rows.child(quiet(&format!(
            "Gathered from the briefs of {} of {} agents.",
            library.read.min(PREVIEW_LIMIT),
            library.agents
        )));
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(intro)
        .child(rows)
        .child(ui::view_footer(
            &[("click", "read the policy")],
            "assigned with backbone templates use NAME [--tag TAG]",
        ))
        .into_any_element()
}

fn quiet(text: &str) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .pl(px(t.space.s8 + t.size.layer_tile + t.space.s10))
            .py(px(t.space.s6))
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(text.to_owned())
}

/// The policy at `at` of `sources` as a reader page, returning to `back`.
pub(super) fn page<'a>(sources: &[&'a Source], at: usize, back: String) -> reader::Page<'a> {
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
            .child(mono(source.path.clone()).text_color(c.text_2))
            .child(scope(source))
            .child(if source.applied {
                "applied at the next start"
            } else {
                "not applied"
            }),
    );
    reader::Page {
        back,
        name: Some(source.name.clone()),
        at: Some(at),
        count: sources.len(),
        card,
        text: match &source.text {
            Some(text) => reader::Text::Ready { text, cut: false },
            None => reader::Text::Failed("Backbone could not read this file."),
        },
    }
}
