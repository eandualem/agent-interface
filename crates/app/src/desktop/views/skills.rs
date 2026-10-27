//! Skills: every skill an agent can load, team-wide, by where it lives (the
//! shared store, repositories, user-level directories, plugins) or by tag.
//! Rows never name an agent; they say how a skill is delivered. Choosing a
//! skill reads its SKILL.md in this pane.

use agent_interface_roster::views::{
    LocalSkill, PREVIEW_LIMIT, Place, Reason, Skill, SkillLibrary,
};
use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};

use super::{Open, Target, ViewPane, home, line, mono, reader};
use crate::desktop::{
    components::{self as ui, Tone},
    design::{colors, tokens},
};

/// One skill on a card.
enum Row<'a> {
    Store(&'a Skill),
    Local(&'a LocalSkill),
}

impl Row<'_> {
    fn path(&self) -> &str {
        match self {
            Self::Store(skill) => &skill.path,
            Self::Local(skill) => &skill.path,
        }
    }
}

/// A subgroup: its label, a note after it, and its skills on one card.
struct Group<'a> {
    label: String,
    /// Shown as a pill in this tone, or as plain mono text.
    tone: Option<Tone>,
    note: String,
    rows: Vec<Row<'a>>,
}

/// A major section: what it is, and its subgroups.
struct Section<'a> {
    title: &'static str,
    note: String,
    groups: Vec<Group<'a>>,
}

fn matches(query: &str, name: &str, description: &str) -> bool {
    query.is_empty()
        || name.to_lowercase().contains(query)
        || description.to_lowercase().contains(query)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn agents(count: usize) -> String {
    match count {
        0 => "no agent carries it".into(),
        n => format!("{n} agent{}", plural(n)),
    }
}

/// The library as sections of subgroups: shared skills by who they are
/// shared with, repository skills by repository, user-level skills by CLI.
fn sections<'a>(pane: &ViewPane, library: &'a SkillLibrary) -> Vec<Section<'a>> {
    let query = pane.query.trim().to_lowercase();
    let store: Vec<&Skill> = library
        .store
        .items
        .iter()
        .filter(|skill| matches(&query, &skill.name, &skill.description))
        .collect();
    let by_tag = library.store.by_tag();
    let carriers = agent_interface_roster::views::tag_carriers(&library.agents);
    let tagged = |tag: &str| -> Vec<Row<'a>> {
        by_tag
            .get(tag)
            .into_iter()
            .flatten()
            .filter(|skill| matches(&query, &skill.name, &skill.description))
            .map(|skill| Row::Store(skill))
            .collect()
    };
    let mut shared = vec![Group {
        label: "all".into(),
        tone: Some(Tone::Plain),
        note: "every agent".into(),
        rows: tagged("all"),
    }];
    let mut tags: Vec<&str> = store
        .iter()
        .flat_map(|skill| &skill.tags)
        .map(String::as_str)
        .filter(|tag| *tag != "all" && !tag.starts_with("agent:"))
        .collect();
    tags.sort_unstable();
    tags.dedup();
    for tag in tags {
        let carriers = carriers.get(tag).copied().unwrap_or(0);
        shared.push(Group {
            label: tag.to_owned(),
            tone: Some(Tone::Tag),
            note: agents(carriers),
            rows: tagged(tag),
        });
    }
    shared.push(Group {
        label: "by name".into(),
        tone: Some(Tone::Quiet),
        note: "given to named agents".into(),
        rows: store
            .iter()
            .filter(|skill| skill.tags.iter().any(|tag| tag.starts_with("agent:")))
            .map(|skill| Row::Store(skill))
            .collect(),
    });
    shared.push(Group {
        label: "no tag".into(),
        tone: Some(Tone::Quiet),
        note: "reaches no agent".into(),
        rows: store
            .iter()
            .filter(|skill| skill.tags.is_empty())
            .map(|skill| Row::Store(skill))
            .collect(),
    });
    let places = |places: &'a [Place]| -> Vec<Group<'a>> {
        places
            .iter()
            .map(|place| Group {
                label: place.label.clone(),
                tone: None,
                note: home(&place.directory),
                rows: place
                    .skills
                    .iter()
                    .filter(|skill| matches(&query, &skill.name, &skill.description))
                    .map(Row::Local)
                    .collect(),
            })
            .collect()
    };
    let mut sections = vec![
        Section {
            title: "Shared skills",
            note: format!(
                "One copy in {}; Backbone links each to the agents whose tags it names.",
                library
                    .store
                    .store
                    .as_deref()
                    .map(home)
                    .unwrap_or_else(|| "the store".into())
            ),
            groups: shared,
        },
        Section {
            title: "Repository skills",
            note: "Kept in a project's .claude/skills or .agents/skills; every agent working \
                   there loads them."
                .into(),
            groups: places(&library.repositories),
        },
        Section {
            title: "User-level skills",
            note: "Kept in a CLI's own folder in the home directory; every agent on that CLI \
                   loads them."
                .into(),
            groups: places(&library.user),
        },
    ];
    for section in &mut sections {
        section.groups.retain(|group| !group.rows.is_empty());
    }
    sections
}

/// The other ways a shared skill is delivered than the group it is listed
/// under: "also coder · security · 2 by name".
fn also(tags: &[String], group: &str) -> String {
    let direct = tags.iter().filter(|tag| tag.starts_with("agent:")).count();
    let mut others: Vec<String> = tags
        .iter()
        .filter(|tag| tag.as_str() != group && !tag.starts_with("agent:"))
        .cloned()
        .collect();
    if direct > 0 && group != "by name" {
        others.push(format!("{direct} by name"));
    }
    if others.is_empty() {
        String::new()
    } else {
        format!("also {}", others.join(" · "))
    }
}

/// What the list can open, in its order, for previous and next.
pub(super) fn targets(pane: &ViewPane, library: &SkillLibrary) -> Vec<Target> {
    let mut targets = Vec::new();
    for section in sections(pane, library) {
        for row in section.groups.iter().flat_map(|group| &group.rows) {
            let target = Target::Skill(row.path().to_owned());
            // A skill shared by several tags appears under each; step through it once.
            if !targets.contains(&target) {
                targets.push(target);
            }
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
    let search = div()
        .flex_shrink_0()
        .px(px(t.space.s20))
        .pt(px(t.space.s14))
        .flex()
        .justify_center()
        .children(pane.search.clone().map(|field| {
            ui::filter_frame()
                .w_full()
                .max_w(px(t.size.grouped_width))
                .h(px(t.size.jump_field))
                .child(div().flex_1().min_w_0().child(field))
        }));
    let mut column = div();
    let mut shown = 0;
    for section in sections(pane, library) {
        column = column.child(ui::group_title(section.title, section.note));
        if section.groups.is_empty() {
            column = column.child(ui::group_note("None found."));
        }
        for (at, group) in section.groups.into_iter().enumerate() {
            let label = match group.tone {
                Some(tone) => ui::tag(group.label.clone(), tone).into_any_element(),
                None => mono(group.label.clone())
                    .text_color(c.text_2)
                    .into_any_element(),
            };
            column = column.child(ui::group_label(label, group.note));
            let mut card = ui::group_card();
            for (index, row) in group.rows.iter().enumerate() {
                shown += 1;
                let (name, error, trailing) = match row {
                    Row::Store(skill) => (
                        &skill.name,
                        skill.error.as_deref(),
                        also(&skill.tags, &group.label),
                    ),
                    Row::Local(skill) => (&skill.name, skill.error.as_deref(), String::new()),
                };
                let target = Target::Skill(row.path().to_owned());
                card = card.child(
                    ui::group_row(
                        SharedString::from(format!("skill-{}-{at}-{index}", section.title)),
                        index == 0,
                        None,
                        name.clone(),
                        match error {
                            Some(error) => div().text_color(c.attention_text).child(line(error)),
                            None => div().child(trailing),
                        },
                    )
                    .on_click(cx.listener(move |pane, _, _, cx| pane.open(target.clone(), cx))),
                );
            }
            column = column.child(card);
        }
    }
    column = column.child(ui::group_title(
        "Plugin skills",
        "Come with each CLI's plugins; Backbone does not list them.",
    ));
    if library.previewed < library.agents.len() {
        column = column.child(ui::group_note(format!(
            "Repository and user-level skills from {} of {} agents' previews (at most {PREVIEW_LIMIT}).",
            library.previewed,
            library.agents.len()
        )));
    }
    if shown == 0 && !pane.query.trim().is_empty() {
        column = div().child(ui::group_note("No skill matches this search."));
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(search)
        .child(ui::grouped_page("skill-rows", column))
        .child(ui::view_footer(
            &[("click", "read the skill")],
            "a repository skill wins over a store skill of the same name",
        ))
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
