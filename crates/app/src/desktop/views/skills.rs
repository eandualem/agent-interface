//! Skills: every skill an agent can load, team-wide, by where it lives (the
//! shared store, repositories, user-level directories, plugins) or by tag.
//! Rows never name an agent; they say how a skill is delivered. Choosing a
//! skill reads its SKILL.md in this pane.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{
    LocalSkill, PREVIEW_LIMIT, Place, Reason, Skill, SkillLibrary,
};

use super::{Open, Target, ViewPane, line, list, mono, reader};
use crate::desktop::{
    components::{self as ui, Tone},
    design::{colors, tokens},
};

/// One line of the library, top to bottom.
enum Item<'a> {
    Layer {
        number: &'static str,
        label: &'static str,
        path: String,
        note: &'static str,
    },
    /// A repository or CLI directory under a layer, or a tag; a tag shows
    /// as a pill in its tone.
    Group {
        label: String,
        tone: Option<Tone>,
        note: String,
    },
    Store(&'a Skill),
    Local(&'a LocalSkill, &'a Place),
    Note(String),
}

fn matches(query: &str, name: &str, description: &str) -> bool {
    query.is_empty()
        || name.to_lowercase().contains(query)
        || description.to_lowercase().contains(query)
}

/// The store skills the tag filter and search let through.
fn store<'a>(pane: &ViewPane, library: &'a SkillLibrary, query: &str) -> Vec<&'a Skill> {
    library
        .store
        .items
        .iter()
        .filter(|skill| pane.tag.as_ref().is_none_or(|tag| skill.tags.contains(tag)))
        .filter(|skill| matches(query, &skill.name, &skill.description))
        .collect()
}

fn items<'a>(pane: &ViewPane, library: &'a SkillLibrary) -> Vec<Item<'a>> {
    let query = pane.query.trim().to_lowercase();
    let store = store(pane, library, &query);
    let mut items = Vec::new();
    if pane.by_tag {
        // `all` first, then tags by name, then skills given by name and
        // skills with no tag, so every skill appears at least once.
        let mut tags: BTreeMap<(u8, &str), Vec<&Skill>> = BTreeMap::new();
        for skill in store {
            for tag in skill.tags.iter().filter(|tag| !tag.starts_with("agent:")) {
                let rank = if tag == "all" { 0 } else { 1 };
                tags.entry((rank, tag)).or_default().push(skill);
            }
            if skill.tags.iter().any(|tag| tag.starts_with("agent:")) {
                tags.entry((2, "direct")).or_default().push(skill);
            }
            if skill.tags.is_empty() {
                tags.entry((3, "no tag")).or_default().push(skill);
            }
        }
        for ((rank, tag), skills) in tags {
            let tone = match rank {
                0 => Tone::Plain,
                1 => Tone::Tag,
                _ => Tone::Quiet,
            };
            items.push(Item::Group {
                label: tag.to_owned(),
                tone: Some(tone),
                note: format!("{} skill{}", skills.len(), plural(skills.len())),
            });
            items.extend(skills.into_iter().map(Item::Store));
        }
        items.push(Item::Note(
            "Repository and user-level skills have no tags; they are listed by layer.".into(),
        ));
        return items;
    }
    items.push(Item::Layer {
        number: "1",
        label: "SHARED STORE",
        path: library.store.store.clone().unwrap_or_else(|| "off".into()),
        note: "Backbone links each by its tags",
    });
    items.extend(store.into_iter().map(Item::Store));
    // Places hold no tags, so a tag filter hides them.
    let places = |items: &mut Vec<Item<'a>>, places: &'a [Place]| {
        if pane.tag.is_some() {
            return;
        }
        for place in places {
            let skills: Vec<_> = place
                .skills
                .iter()
                .filter(|skill| matches(&query, &skill.name, &skill.description))
                .collect();
            if skills.is_empty() {
                continue;
            }
            items.push(Item::Group {
                label: place.label.clone(),
                tone: None,
                note: place.directory.clone(),
            });
            items.extend(skills.into_iter().map(|skill| Item::Local(skill, place)));
        }
    };
    items.push(Item::Layer {
        number: "2",
        label: "REPOSITORIES",
        path: ".claude/skills · .agents/skills".into(),
        note: "every agent working in that project",
    });
    places(&mut items, &library.repositories);
    items.push(Item::Layer {
        number: "3",
        label: "USER-LEVEL",
        path: "per CLI, in the home folder".into(),
        note: "every agent on that CLI",
    });
    places(&mut items, &library.user);
    items.push(Item::Layer {
        number: "4",
        label: "PLUGINS",
        path: "installed with each CLI".into(),
        note: "not listed by Backbone",
    });
    if library.previewed < library.agents.len() {
        items.push(Item::Note(format!(
            "Repository and user skills from {} of {} agents' previews (at most {PREVIEW_LIMIT}).",
            library.previewed,
            library.agents.len()
        )));
    }
    items
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn sub_heading(label: impl IntoElement, note: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .pl(px(t.space.s8 + t.size.layer_tile + t.space.s10))
            .pr(px(t.space.s8))
            .pt(px(t.space.s8))
            .pb(px(t.space.s2))
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.meta,
    )
    .child(label)
    .child(
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .child(note),
    )
}

/// What the list can open, in its order, for previous and next.
pub(super) fn targets(pane: &ViewPane, library: &SkillLibrary) -> Vec<Target> {
    let mut targets = Vec::new();
    for item in items(pane, library) {
        let folder = match item {
            Item::Store(skill) => &skill.path,
            Item::Local(skill, _) => &skill.path,
            _ => continue,
        };
        let target = Target::Skill(folder.clone());
        // Under By tag a skill appears once per tag; step through it once.
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// A store skill's delivery as pills: `all`, its tags, and how many agents it
/// names directly.
pub(super) fn delivery(tags: &[String]) -> Div {
    let t = tokens();
    let direct = tags.iter().filter(|tag| tag.starts_with("agent:")).count();
    let mut pills = div().flex().flex_wrap().gap(px(t.space.s4));
    for tag in tags.iter().filter(|tag| !tag.starts_with("agent:")) {
        let tone = if tag == "all" { Tone::Plain } else { Tone::Tag };
        pills = pills.child(ui::tag(tag.clone(), tone));
    }
    if direct > 0 {
        pills = pills.child(ui::tag(format!("+{direct} direct"), Tone::Quiet));
    }
    pills
}

pub(super) fn body(
    pane: &ViewPane,
    library: &SkillLibrary,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    if let Some(open) = &pane.open {
        return detail(pane, library, open, cx);
    }
    index(pane, library, cx)
}

fn index(pane: &ViewPane, library: &SkillLibrary, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("diamond", "Skills")
        .child("every skill an agent can load, by where it lives");
    let mut grouping = ui::segments();
    for (by_tag, label) in [(false, "By layer"), (true, "By tag")] {
        grouping = grouping.child(
            ui::segment(
                SharedString::from(format!("skills-{label}")),
                label,
                pane.by_tag == by_tag,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.by_tag = by_tag;
                cx.notify();
            })),
        );
    }
    let mut counts: BTreeMap<(bool, &str), usize> = BTreeMap::new();
    for tag in library.store.items.iter().flat_map(|skill| &skill.tags) {
        if !tag.starts_with("agent:") {
            *counts.entry((tag != "all", tag)).or_default() += 1;
        }
    }
    let mut chips = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(t.space.s4 + 1.0))
        .child(ui::text(div().text_color(c.muted).mr(px(t.space.s2)), &t.text.meta).child("tags"))
        .child(
            ui::chip("skills-tag-any", "Any", pane.tag.is_none()).on_click(cx.listener(
                |pane, _, _, cx| {
                    pane.tag = None;
                    cx.notify();
                },
            )),
        );
    for ((_, tag), count) in counts {
        let name = tag.to_owned();
        chips = chips.child(
            ui::chip(
                SharedString::from(format!("skills-tag-{tag}")),
                format!("{tag} {count}"),
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
        .py(px(t.space.s12))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s10))
                .children(pane.search.clone().map(|field| {
                    ui::filter_frame()
                        .flex_1()
                        .h(px(t.size.jump_field))
                        .child(div().flex_1().min_w_0().child(field))
                }))
                .child(grouping),
        )
        .child(chips);
    let narrow = pane.narrow();
    let mut rows = list("skill-rows").pt(px(t.space.s2));
    let mut shown = 0;
    for (at, item) in items(pane, library).into_iter().enumerate() {
        rows = rows.child(match item {
            Item::Layer {
                number,
                label,
                path,
                note,
            } => ui::layer_heading(number, label, path, note, false).into_any_element(),
            Item::Group { label, tone, note } => sub_heading(
                match tone {
                    Some(tone) => ui::tag(label, tone),
                    None => mono(label).text_color(c.text_2),
                },
                note,
            )
            .into_any_element(),
            Item::Note(text) => ui::text(
                div()
                    .px(px(t.space.s8))
                    .py(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(text)
            .into_any_element(),
            Item::Store(skill) => {
                shown += 1;
                row(
                    at,
                    &skill.name,
                    &skill.description,
                    skill.error.as_deref(),
                    delivery(&skill.tags),
                    Target::Skill(skill.path.clone()),
                    narrow,
                    cx,
                )
            }
            Item::Local(skill, place) => {
                shown += 1;
                row(
                    at,
                    &skill.name,
                    &skill.description,
                    skill.error.as_deref(),
                    div().child(ui::tag(place.label.clone(), Tone::Plain)),
                    Target::Skill(skill.path.clone()),
                    narrow,
                    cx,
                )
            }
        });
    }
    if shown == 0 {
        rows = rows.child(
            ui::text(
                div()
                    .px(px(t.space.s8))
                    .py(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.body,
            )
            .child("No skill matches this search and tag."),
        );
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(controls)
        .child(rows)
        .child(ui::view_footer(
            &[("click", "read the skill")],
            "a repository skill wins over a store skill of the same name",
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn row(
    at: usize,
    name: &str,
    description: &str,
    error: Option<&str>,
    trailing: Div,
    target: Target,
    narrow: bool,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let c = colors();
    let (text, color) = match error {
        Some(error) => (line(error), c.attention_text),
        None => (line(description), c.muted),
    };
    ui::list_row(
        SharedString::from(format!("skill-{at}")),
        None,
        name.to_owned(),
        text,
        color,
        trailing,
        narrow,
    )
    .on_click(cx.listener(move |pane, _, _, cx| pane.open(target.clone(), cx)))
    .into_any_element()
}

/// A skill being read: its summary, and its SKILL.md.
fn detail(
    pane: &ViewPane,
    library: &SkillLibrary,
    open: &Open,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let Target::Skill(folder) = &open.target else {
        return div().into_any_element();
    };
    let header = ui::view_header("diamond", "Skills").child("reading SKILL.md");
    let targets = targets(pane, library);
    let at = targets.iter().position(|target| *target == open.target);
    let store = library
        .store
        .items
        .iter()
        .find(|skill| &skill.path == folder);
    let local = [&library.repositories, &library.user]
        .into_iter()
        .flatten()
        .find_map(|place| {
            let skill = place.skills.iter().find(|skill| &skill.path == folder)?;
            Some((skill, place))
        });
    let (name, description) = match (store, local) {
        (Some(skill), _) => (skill.name.clone(), skill.description.clone()),
        (None, Some((skill, _))) => (skill.name.clone(), skill.description.clone()),
        (None, None) => (String::new(), String::new()),
    };
    let facts = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(t.space.s14))
        .gap_y(px(t.space.s6))
        .text_color(c.muted);
    let facts = match (store, local) {
        (Some(skill), _) => facts
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .child("tags")
                    .child(delivery(&skill.tags)),
            )
            .child(reaches(skill, library.agents.len())),
        (None, Some((_, place))) if library.repositories.contains(place) => {
            facts.child(format!("every agent working in {}", place.label))
        }
        (None, Some((_, place))) => facts.child(format!("every {} agent", place.label)),
        (None, None) => facts,
    }
    .child(mono(folder.clone()).text_color(c.muted));
    let mut card = ui::info_card()
        .child(div().text_color(c.text_2).child(line(&description)))
        .child(facts);
    if let Some(skill) = store {
        card = card.child(check(pane, skill, library));
    }
    reader::render(
        pane,
        header,
        reader::Page {
            back: "Skills".into(),
            name: Some(name),
            at,
            count: targets.len(),
            card,
            text: text(open),
        },
        cx,
    )
}

fn reaches(skill: &Skill, agents: usize) -> String {
    match skill.reaches.len() {
        n if n == agents && n > 0 => "reaches every agent".into(),
        0 => "reaches no agent".into(),
        n => format!("reaches {n} agent{}", plural(n)),
    }
}

/// A store skill's "check an agent" line: type a name, read why it gets the
/// skill or that it does not.
fn check(pane: &ViewPane, skill: &Skill, library: &SkillLibrary) -> Div {
    let (t, c) = (tokens(), colors());
    let answer = if pane.check.is_empty() {
        ui::text(div().text_color(c.muted), &t.text.status).child("check an agent")
    } else {
        let agent = library.agents.iter().find(|agent| agent.name == pane.check);
        let reason = agent.and_then(|agent| Reason::of(&skill.tags, &agent.name, &agent.tags));
        match (agent, reason) {
            (None, _) => {
                ui::text(div().text_color(c.muted), &t.text.status).child("no agent by that name")
            }
            (Some(_), Some(reason)) => ui::text(div().text_color(c.text_2), &t.text.status)
                .child(format!("✓ gets it by {}", reason.label())),
            (Some(_), None) => {
                ui::text(div().text_color(c.muted), &t.text.status).child("does not get it")
            }
        }
    };
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s8))
        .children(pane.check_field.clone().map(|field| {
            ui::field_frame(false)
                .w(px(t.size.tag_card_label + t.space.s18))
                .h(px(t.size.chip + t.space.s4))
                .child(div().flex_1().min_w_0().child(field))
        }))
        .child(answer)
}

/// The reader's text for an open skill.
pub(super) fn text(open: &Open) -> reader::Text<'_> {
    match &open.text {
        None => reader::Text::Loading,
        Some(Err(error)) => reader::Text::Failed(error),
        Some(Ok((text, cut))) => reader::Text::Ready { text, cut: *cut },
    }
}
