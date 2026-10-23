//! Tags: what each tag gives the agents that carry it. One card per tag,
//! `all` first; choosing a card lists the agents carrying that tag in this
//! pane, with a way back, since agent names live there and not on the cards.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::TagLibrary;

use super::{ViewPane, list, mono, team};
use crate::desktop::{
    components::{self as ui, Tone},
    design::{colors, tokens},
};

fn agents(count: usize) -> String {
    match count {
        0 => "no agent".into(),
        1 => "1 agent".into(),
        n => format!("{n} agents"),
    }
}

pub(super) fn body(
    pane: &ViewPane,
    library: &TagLibrary,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    match &pane.carriers_of {
        Some(tag) => carriers(library, tag, cx),
        None => cards(library, cx),
    }
}

fn cards(library: &TagLibrary, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header =
        ui::view_header("hash", "Tags").child("what each tag gives the agents that carry it");
    let code = |text: &'static str| mono(text).text_color(c.text_2);
    let intro = ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .gap_x(px(t.space.s4))
            .px(px(t.space.s16))
            .py(px(t.space.s12))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted),
        &t.text.status,
    )
    .child("Tags are how a team is organised in Backbone. An agent tagged")
    .child(code("coder python"))
    .child("gets everything tagged")
    .child(code("all"))
    .child(",")
    .child(code("coder"))
    .child("and")
    .child(code("python"))
    .child(", and nothing else.");
    let mut rows = list("tag-cards")
        .px(px(t.space.s12))
        .py(px(t.space.s10))
        .gap(px(t.space.s8));
    for card in library.cards() {
        let all = card.tag == "all";
        let carried = if all {
            "every agent".to_owned()
        } else {
            agents(card.carriers)
        };
        let tag = card.tag.clone();
        rows = rows.child(
            ui::tag_card(
                SharedString::from(format!("tag-{}", card.tag)),
                ui::tag(card.tag, if all { Tone::Plain } else { Tone::Tag }),
                carried,
                &[("SKILLS", card.skills), ("POLICIES", card.policies)],
                all,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.carriers_of = Some(tag.clone());
                cx.notify();
            })),
        );
    }
    let direct = library.direct();
    rows = rows.child(
        div()
            .flex()
            .flex_shrink_0()
            .gap(px(t.space.s14))
            .px(px(t.space.s12))
            .py(px(t.space.s10 + 1.0))
            .rounded(px(t.radius.slot))
            .border(px(t.size.hairline))
            .border_dashed()
            .border_color(c.line_strong)
            .child(
                ui::text(
                    div()
                        .flex_shrink_0()
                        .w(px(t.size.tag_card_label))
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s4))
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child(mono("agent:NAME").text_color(c.text_2))
                .child("one agent, by name"),
            )
            .child(
                ui::text(div().flex_1().min_w_0().text_color(c.muted), &t.text.status).child(
                    if direct.is_empty() {
                        "Skills only; no skill is given to an agent by name. Policies cannot \
                         name an agent."
                            .to_owned()
                    } else {
                        format!(
                            "Skills only. {} {} also given directly to agents by name. Policies \
                             cannot name an agent.",
                            direct
                                .iter()
                                .map(|(name, count)| format!("{name} ({})", agents(*count)))
                                .collect::<Vec<_>>()
                                .join(", "),
                            if direct.len() == 1 { "is" } else { "are" }
                        )
                    },
                ),
            ),
    );
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(intro)
        .child(rows)
        .child(ui::view_footer(
            &[("click", "the agents carrying it")],
            "tags set with backbone agent tag NAME TAG…",
        ))
        .into_any_element()
}

/// The agents carrying `tag`, or every agent for `all`.
fn carriers(library: &TagLibrary, tag: &str, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let members: Vec<_> = library
        .agents
        .iter()
        .filter(|agent| tag == "all" || agent.tags.iter().any(|t| t == tag))
        .collect();
    let header =
        ui::view_header("hash", "Tags").child(format!("carried by {}", agents(members.len())));
    let bar = ui::view_toolbar()
        .px(px(t.space.s10))
        .child(
            ui::back_button("tags-back", "Tags").on_click(cx.listener(|pane, _, _, cx| {
                pane.close(cx);
            })),
        )
        .child(ui::tag(
            tag.to_owned(),
            if tag == "all" { Tone::Plain } else { Tone::Tag },
        ));
    let mut rows = list("tag-agents");
    for member in &members {
        rows = rows.child(team::agent_row(member, "", tag, cx));
    }
    if members.is_empty() {
        rows = rows.child(
            ui::text(
                div()
                    .px(px(t.space.s8))
                    .py(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.body,
            )
            .child("No agent carries this tag."),
        );
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(bar)
        .child(rows)
        .child(ui::view_footer(
            &[("esc", "back"), ("click", "show its terminal")],
            "read-only",
        ))
        .into_any_element()
}
