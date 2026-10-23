//! Skills: the shared store as a list, then one skill's SKILL.md to read,
//! with a way back, previous and next, and its source.

use std::collections::BTreeSet;

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{Skill, Skills};

use super::{ViewPane, line, list, markdown, mono};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// The skills shown for a tag and a search. A search that names an agent
/// shows what that agent gets.
pub(super) fn shown<'a>(skills: &'a Skills, tag: Option<&str>, query: &str) -> Vec<&'a Skill> {
    let query = query.trim().to_lowercase();
    skills
        .items
        .iter()
        .filter(|skill| tag.is_none_or(|tag| skill.tags.iter().any(|t| t == tag)))
        .filter(|skill| {
            query.is_empty()
                || skill.name.to_lowercase().contains(&query)
                || skill.description.to_lowercase().contains(&query)
                || skill
                    .reaches
                    .iter()
                    .any(|agent| agent.to_lowercase() == query)
        })
        .collect()
}

fn reach(skill: &Skill, everyone: bool) -> String {
    match skill.reaches.len() {
        _ if everyone && skill.tags.iter().any(|tag| tag == "all") => "every agent".into(),
        0 => "no agent".into(),
        1 => "1 agent".into(),
        n => format!("{n} agents"),
    }
}

pub(super) fn body(pane: &ViewPane, skills: &Skills, cx: &mut Context<ViewPane>) -> AnyElement {
    let shown = shown(skills, pane.tag.as_deref(), &pane.query);
    match &pane.reading {
        Some(reading) if reading.index < shown.len() => {
            detail(pane, shown[reading.index], shown.len(), cx)
        }
        _ => index(pane, skills, &shown, cx),
    }
}

fn index(
    pane: &ViewPane,
    skills: &Skills,
    shown: &[&Skill],
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("diamond", "Skills").child("all agents");
    let tags: BTreeSet<&str> = skills
        .items
        .iter()
        .flat_map(|s| s.tags.iter().map(String::as_str))
        .collect();
    let mut chips = div().flex().flex_wrap().gap(px(t.space.s4 + 1.0)).child(
        ui::chip("skills-tag-any", "Any tag", pane.tag.is_none()).on_click(cx.listener(
            |pane, _, _, cx| {
                pane.tag = None;
                cx.notify();
            },
        )),
    );
    for tag in tags {
        let name = tag.to_owned();
        chips = chips.child(
            ui::chip(
                SharedString::from(format!("skills-tag-{tag}")),
                tag.to_owned(),
                pane.tag.as_deref() == Some(tag),
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.tag = Some(name.clone());
                cx.notify();
            })),
        );
    }
    let controls = div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .gap(px(t.space.s10))
        .px(px(t.space.s14))
        .pt(px(t.space.s12))
        .pb(px(t.space.s10))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .children(pane.search.clone().map(|field| {
            ui::filter_frame()
                .h(px(t.size.jump_field))
                .child(div().flex_1().min_w_0().child(field))
        }))
        .child(chips);
    let mut rows = list("skill-rows").child(ui::view_heading("SHARED STORE", ""));
    for (index, skill) in shown.iter().enumerate() {
        let folder = skill.path.clone();
        let (detail, detail_color) = match &skill.error {
            Some(error) => (line(error), c.attention_text),
            None => (line(&skill.description), c.muted),
        };
        rows = rows.child(
            div()
                .id(SharedString::from(format!("skill-{}", skill.name)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(t.space.s12))
                .min_h(px(t.size.slot_row + t.space.s4))
                .px(px(t.space.s8))
                .py(px(t.space.s6))
                .rounded(px(t.radius.row))
                .cursor_pointer()
                .hover(|el| el.bg(colors().hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s2 + 1.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(t.space.s8))
                                .child(mono(skill.name.clone()).text_color(c.text))
                                .child(
                                    ui::text(div().text_color(c.muted), &t.text.meta)
                                        .child(skill.tags.join(" · ")),
                                ),
                        )
                        .child(
                            ui::text(
                                div()
                                    .text_color(detail_color)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                                &t.text.status,
                            )
                            .child(detail),
                        ),
                )
                .child(
                    ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.meta)
                        .child(reach(skill, true)),
                )
                .child(ui::icon("chevron-right", t.size.icon - 2.0, c.faint))
                .on_click(
                    cx.listener(move |pane, _, _, cx| pane.read_skill(index, folder.clone(), cx)),
                ),
        );
    }
    if shown.is_empty() {
        rows = rows.child(
            ui::text(
                div()
                    .px(px(t.space.s8))
                    .py(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.body,
            )
            .child("No skill matches. Type a skill, or an agent's name to see what it gets."),
        );
    }
    let note = match &skills.store {
        Some(store) => format!("store {store} · {} skills", skills.items.len()),
        None => "the skills store is turned off".to_owned(),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(controls)
        .child(rows)
        .child(ui::view_footer(&[("click", "read")], note))
        .into_any_element()
}

fn detail(pane: &ViewPane, skill: &Skill, count: usize, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let Some(reading) = &pane.reading else {
        return div().into_any_element();
    };
    let header = ui::view_header("diamond", "Skills").child("reading SKILL.md");
    let mut display = ui::segments();
    for (source, label) in [(false, "Rendered"), (true, "Source")] {
        display = display.child(
            ui::segment(
                SharedString::from(format!("skill-display-{label}")),
                label,
                reading.source == source,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                if let Some(reading) = &mut pane.reading {
                    reading.source = source;
                }
                cx.notify();
            })),
        );
    }
    let step = |id: &'static str, icon: &'static str, by: isize, cx: &mut Context<ViewPane>| {
        ui::icon_button(id, icon)
            .on_click(cx.listener(move |pane, _, _, cx| pane.step_skill(by, cx)))
    };
    let bar = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.row + t.space.s10))
        .px(px(t.space.s10))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .child(
            ui::back_button("skill-back", "Skills").on_click(cx.listener(|pane, _, _, cx| {
                pane.reading = None;
                cx.notify();
            })),
        )
        .child(mono(skill.name.clone()).text_color(c.text))
        .child(div().flex_1())
        .child(step("skill-previous", "chevron-left", -1, cx))
        .child(
            ui::text(div().text_color(c.muted), &t.text.status)
                .child(format!("{} of {count}", reading.index + 1)),
        )
        .child(step("skill-next", "chevron-right", 1, cx))
        .child(display);
    let mut tags = div()
        .flex()
        .items_center()
        .gap(px(t.space.s6))
        .child("tags");
    for tag in &skill.tags {
        tags = tags.child(ui::pill(tag.clone()));
    }
    let card = ui::info_card()
        .child(div().text_color(c.text_2).child(line(&skill.description)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(t.space.s14))
                .text_color(c.muted)
                .child(tags)
                .child(format!("reaches {}", reach(skill, true)))
                .child(mono(skill.path.clone()).text_color(c.muted)),
        )
        .when(!skill.reaches.is_empty(), |card| {
            card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(t.space.s4))
                    .text_color(c.muted)
                    .child("agents:")
                    .child(skill.reaches.join(", ")),
            )
        });
    let content: AnyElement = match &reading.text {
        None => ui::text(div().text_color(c.muted), &t.text.body)
            .child("Reading SKILL.md…")
            .into_any_element(),
        Some(Err(error)) => ui::text(div().text_color(c.attention_text), &t.text.body)
            .child(*error)
            .into_any_element(),
        Some(Ok((text, cut))) => {
            let shown = if reading.source {
                ui::text(
                    div().font_family(t.font.mono.clone()).text_color(c.text_2),
                    &t.text.status,
                )
                .child(text.clone())
            } else {
                markdown::render(&markdown::parse(text))
            };
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s12))
                .child(shown)
                .when(*cut, |el| {
                    el.child(
                        ui::text(div().text_color(c.muted), &t.text.meta)
                            .child("The rest of this file is longer than this view reads."),
                    )
                })
                .into_any_element()
        }
    };
    let page = div()
        .id("skill-page")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s16))
        .overflow_y_scroll()
        .px(px(t.space.s24))
        .py(px(t.space.s20))
        .child(card)
        .child(content);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(bar)
        .child(page)
        .child(ui::view_footer(
            &[
                ("esc", "back to skills"),
                ("[ ]", "previous, next skill"),
                ("S", "source"),
            ],
            "read-only",
        ))
        .into_any_element()
}
