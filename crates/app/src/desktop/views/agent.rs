//! Agent: everything about one agent, in sections drawn inside this pane.
//! Its state and work, what its CLI lets it do, its connections, the skills it
//! loads and why each reaches it, what it receives at its next start, and its
//! usage. Choosing a skill or policy reads it here; nothing opens elsewhere.

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};
use tmux_workspaces_core::AgentSection;
use tmux_workspaces_roster::views::{
    Agent, Data, Instructions, Loadout, LocalSkill, Reason, Received, Usage,
};

use super::{
    Target, ViewEvent, ViewPane, ago, count, home, line, list, mono, policies, reader, since,
    skills, usage,
};
use crate::desktop::{
    components::{self as ui, AgentState, Tone},
    design::{colors, tokens},
};

fn held(message: &Received) -> bool {
    matches!(
        message.outcome.as_str(),
        "queued" | "held" | "stored" | "agent_working" | "deferred"
    )
}

pub(super) fn body(pane: &ViewPane, agent: &Agent, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    if !agent.known {
        return div()
            .size_full()
            .flex()
            .flex_col()
            .child(ui::view_header("user", agent.name.clone()))
            .child(ui::empty_state(
                ui::empty_tile("user", c.selected, c.muted),
                "Backbone does not know this agent",
                format!("{} may have been renamed or removed.", agent.name),
            ))
            .into_any_element();
    }
    let state = AgentState::from_report(&agent.state);
    let (word, color) = match (agent.online, state) {
        (false, _) => ("offline".to_owned(), c.muted),
        (true, AgentState::NeedsYou) => (
            match &agent.reason {
                Some(reason) => format!("waiting for a person · {reason}"),
                None => "waiting for a person".to_owned(),
            },
            c.attention_text,
        ),
        (true, AgentState::Working) => ("working".to_owned(), c.accent_text),
        (true, state) => (state.word().to_owned(), c.muted),
    };
    let name = agent.name.clone();
    let header = ui::view_header("user", agent.name.clone())
        .pr(px(t.space.s6))
        .child(ui::badge(agent.runtime.clone()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s4 + 1.0))
                .text_color(color)
                .when(agent.online, |el| el.child(ui::agent_symbol(state)))
                .child(word),
        )
        .child(div().flex_1())
        .child(
            ui::text(
                div()
                    .id("agent-terminal")
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .h(px(t.size.chip - t.space.s2))
                    .px(px(t.space.s8))
                    .rounded(px(t.radius.key + 1.0))
                    .border(px(t.size.hairline))
                    .border_color(c.line_strong)
                    .bg(c.field)
                    .text_color(c.text_2)
                    .cursor_pointer()
                    .hover(|el| el.bg(colors().hover)),
                &t.text.meta,
            )
            .child("Its terminal")
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ViewEvent::ShowAgent(name.clone())))),
        );

    if let Some(page) = page(pane, agent) {
        return reader::render(pane, header, page, cx);
    }
    // In a narrow pane the tabs scroll sideways rather than hide a section.
    let mut nav = div()
        .id("agent-sections")
        .flex()
        .flex_shrink_0()
        .items_end()
        .gap(px(t.space.s2))
        .h(px(t.size.view_toolbar))
        .px(px(t.space.s10))
        .overflow_x_scroll()
        .border_b(px(t.size.hairline))
        .border_color(c.line);
    for section in AgentSection::ALL {
        // A count is known once its section has been read.
        let count = match section {
            AgentSection::Skills => agent
                .skills
                .as_ref()
                .and_then(|loadout| loadout.as_ref().ok())
                .map(|loadout| {
                    loadout.skills.len() + loadout.user.len() + loadout.repository.len()
                }),
            AgentSection::Policies => agent
                .brief
                .as_ref()
                .and_then(|brief| brief.as_ref().ok())
                .map(|brief| brief.sources.len()),
            _ => None,
        };
        nav = nav.child(
            tab(
                SharedString::from(format!("agent-section-{}", section.as_str())),
                label(section),
                count.map(|n| n.to_string()).unwrap_or_default(),
                section == pane.section,
            )
            .on_click(cx.listener(move |pane, _, _, cx| pane.set_section(section, cx))),
        );
    }

    let content = match pane.section {
        AgentSection::Overview => overview(agent, state),
        AgentSection::Permissions => permissions(agent),
        AgentSection::Connections => connections(agent),
        AgentSection::Skills => skills_section(agent, cx),
        AgentSection::Policies => policies_section(agent, cx),
        AgentSection::Usage => usage_section(agent),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(nav)
        .child(content)
        .into_any_element()
}

fn label(section: AgentSection) -> &'static str {
    match section {
        AgentSection::Overview => "Overview",
        AgentSection::Permissions => "Permissions",
        AgentSection::Connections => "Connections",
        AgentSection::Skills => "Skills",
        AgentSection::Policies => "Policies",
        AgentSection::Usage => "Usage",
    }
}

/// What the shown section's list can open, in its order.
pub(super) fn targets(pane: &ViewPane, agent: &Agent) -> Vec<Target> {
    match pane.section {
        AgentSection::Skills => loaded(&agent.skills)
            .map(|loadout| {
                skill_rows(loadout, &agent.runtime)
                    .into_iter()
                    .flat_map(|(_, rows)| rows)
                    .map(|row| Target::Skill(row.path.to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
        AgentSection::Policies => loaded(&agent.brief)
            .map(|brief| {
                brief
                    .sources
                    .iter()
                    .map(|source| Target::Policy(source.path.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// When a refresh's section read failed but the last data had that section,
/// keep the last section and return the failure to show as stale.
pub(super) fn keep_section(last: Option<&Data>, new: &mut Data) -> Option<&'static str> {
    fn keep<T: Clone>(
        last: &Option<Result<T, &'static str>>,
        new: &mut Option<Result<T, &'static str>>,
    ) -> Option<&'static str> {
        let (Some(Ok(kept)), Some(Err(error))) = (last, &new) else {
            return None;
        };
        let error = *error;
        *new = Some(Ok(kept.clone()));
        Some(error)
    }
    let (Some(Data::Agent(last)), Data::Agent(new)) = (last, new) else {
        return None;
    };
    if last.name != new.name {
        return None;
    }
    keep(&last.skills, &mut new.skills)
        .or(keep(&last.brief, &mut new.brief))
        .or(keep(&last.usage, &mut new.usage))
}

fn loaded<'a, T>(part: &'a Option<Result<T, &'static str>>) -> Option<&'a T> {
    part.as_ref().and_then(|part| part.as_ref().ok())
}

/// The open skill or policy as a reader page, when this section lists it.
fn page<'a>(pane: &'a ViewPane, agent: &'a Agent) -> Option<reader::Page<'a>> {
    let open = pane.open.as_ref()?;
    match &open.target {
        Target::Policy(path) => {
            let brief = loaded(&agent.brief)?;
            let sources: Vec<_> = brief.sources.iter().collect();
            let at = sources.iter().position(|source| &source.path == path)?;
            Some(policies::page(
                &sources,
                at,
                format!("{}'s brief", agent.name),
                true,
            ))
        }
        Target::Skill(folder) => {
            let loadout = loaded(&agent.skills)?;
            let rows: Vec<_> = skill_rows(loadout, &agent.runtime)
                .into_iter()
                .flat_map(|(_, rows)| rows)
                .collect();
            let at = rows.iter().position(|row| row.path == folder)?;
            let row = &rows[at];
            let (t, c) = (tokens(), colors());
            let card = ui::info_card()
                .child(div().text_color(c.text_2).child(line(row.description)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(px(t.space.s14))
                        .gap_y(px(t.space.s6))
                        .text_color(c.muted)
                        .child(why(&row.why))
                        .child(mono(row.path.to_owned()).text_color(c.muted)),
                );
            Some(reader::Page {
                back: format!("{} · Skills", agent.name),
                name: Some(row.name.to_owned()),
                at: Some(at),
                count: rows.len(),
                card,
                text: skills::text(open),
            })
        }
    }
}

/// How a skill reaches the agent, as its row's pill says.
enum Why {
    Store(Option<Reason>),
    Plain(String),
}

fn why(why: &Why) -> Div {
    match why {
        Why::Store(Some(reason @ Reason::Tag(_))) => ui::tag(reason.label(), Tone::Tag),
        Why::Store(Some(reason)) => ui::tag(reason.label(), Tone::Plain),
        Why::Store(None) => ui::tag("linked", Tone::Quiet),
        Why::Plain(label) => ui::tag(label.clone(), Tone::Plain),
    }
}

struct SkillRow<'a> {
    name: &'a str,
    description: &'a str,
    error: Option<&'a str>,
    path: &'a str,
    why: Why,
}

/// The agent's skills by source, with each group's heading and note:
/// the store, user-level, repository.
fn skill_rows<'a>(
    loadout: &'a Loadout,
    runtime: &str,
) -> Vec<((&'static str, String), Vec<SkillRow<'a>>)> {
    let store = loadout
        .skills
        .iter()
        .map(|skill| SkillRow {
            name: &skill.name,
            description: &skill.description,
            error: skill.error.as_deref(),
            path: &skill.path,
            why: Why::Store(None),
        })
        .collect();
    fn local<'a>(skills: &'a [LocalSkill], label: &str) -> Vec<SkillRow<'a>> {
        skills
            .iter()
            .map(|skill| SkillRow {
                name: &skill.name,
                description: &skill.description,
                error: skill.error.as_deref(),
                path: &skill.path,
                why: Why::Plain(label.to_owned()),
            })
            .collect()
    }
    vec![
        (
            (
                "SHARED STORE",
                format!(
                    "{}, chosen by its tags",
                    loadout.store.clone().unwrap_or_default()
                ),
            ),
            store,
        ),
        (
            ("USER-LEVEL", loadout.user_directories.join(" · ")),
            local(&loadout.user, runtime),
        ),
        (
            ("REPOSITORY", String::new()),
            local(&loadout.repository, "repository"),
        ),
    ]
}

fn waiting(text: &'static str) -> AnyElement {
    let (t, c) = (tokens(), colors());
    list("agent-section-state")
        .child(
            ui::text(
                div()
                    .px(px(t.space.s10))
                    .py(px(t.space.s10))
                    .text_color(c.muted),
                &t.text.body,
            )
            .child(text),
        )
        .into_any_element()
}

fn failed(error: &'static str) -> AnyElement {
    let (t, c) = (tokens(), colors());
    list("agent-section-state")
        .child(
            ui::text(
                div()
                    .px(px(t.space.s10))
                    .py(px(t.space.s10))
                    .text_color(c.attention_text),
                &t.text.body,
            )
            .child(error),
        )
        .into_any_element()
}

fn skills_section(agent: &Agent, cx: &mut Context<ViewPane>) -> AnyElement {
    let loadout = match &agent.skills {
        None => return waiting("Reading the skills its next launch links…"),
        Some(Err(error)) => return failed(error),
        Some(Ok(loadout)) => loadout,
    };
    let c = colors();
    let groups = skill_rows(loadout, &agent.runtime);
    let total: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
    let mut column = div().child(ui::group_note(format!(
        "{total} skill{} load for {}. Each says why it reaches this agent.",
        if total == 1 { "" } else { "s" },
        agent.name
    )));
    let mut at = 0;
    for ((heading, note), group) in groups {
        let (title, note) = match heading {
            "SHARED STORE" => ("Shared skills", home(&note)),
            "USER-LEVEL" => (
                "User-level skills",
                format!("in {}, for every {} agent", home(&note), agent.runtime),
            ),
            _ => (
                "Repository skills",
                if group.is_empty() {
                    "its directory has none".to_owned()
                } else {
                    format!("in {}", home(&agent.dir))
                },
            ),
        };
        column = column.child(ui::group_title(title, note));
        if group.is_empty() {
            continue;
        }
        let mut card = ui::group_card();
        for (index, row) in group.into_iter().enumerate() {
            at += 1;
            let why = match row.why {
                Why::Store(_) => {
                    let skill = loadout.skills.iter().find(|skill| skill.path == row.path);
                    Why::Store(
                        skill.and_then(|skill| Reason::of(&skill.tags, &agent.name, &loadout.tags)),
                    )
                }
                other => other,
            };
            let target = Target::Skill(row.path.to_owned());
            card = card.child(
                ui::group_row(
                    SharedString::from(format!("agent-skill-{at}")),
                    index == 0,
                    None,
                    row.name.to_owned(),
                    match row.error {
                        Some(error) => div().text_color(c.attention_text).child(line(error)),
                        None => div().child(self::why(&why)),
                    },
                )
                .on_click(cx.listener(move |pane, _, _, cx| pane.open(target.clone(), cx))),
            );
        }
        column = column.child(card);
    }
    column = column.child(ui::group_title(
        "Plugin skills",
        format!(
            "Loaded by {} itself; Backbone does not list them.",
            agent.runtime
        ),
    ));
    for notice in &loadout.notices {
        column = column.child(ui::group_note(notice.clone()));
    }
    ui::grouped_page("agent-skills", column).into_any_element()
}

/// The group a source of a brief belongs to, as the design heads them.
fn group(scope: &str) -> (&'static str, &'static str) {
    let parts: Vec<&str> = scope.split(", ").collect();
    if parts.contains(&"base") {
        ("Base brief", "every agent starts with it")
    } else if parts.contains(&"swarm") {
        ("Swarm role", "only for swarm members")
    } else if parts.contains(&"global") {
        ("Global policies", "every agent, in this order")
    } else {
        ("Tag policies", "from the tags it carries")
    }
}

fn policies_section(agent: &Agent, cx: &mut Context<ViewPane>) -> AnyElement {
    let brief: &Instructions = match &agent.brief {
        None => return waiting("Reading what it receives at its next start…"),
        Some(Err(error)) => return failed(error),
        Some(Ok(brief)) => brief,
    };
    let mut column = div().child(ui::group_note(
        "What it receives at its next start, in order. Each says why.",
    ));
    if !brief.enabled {
        column = column.child(ui::group_note(
            "Backbone does not inject a brief into this agent.",
        ));
    }
    let mut card: Option<(Div, bool)> = None;
    let mut previous = "";
    for (at, source) in brief.sources.iter().enumerate() {
        let (heading, note) = group(&source.scope);
        if heading != previous {
            if let Some((done, _)) = card.take() {
                column = column.child(done);
            }
            column = column.child(ui::group_title(heading, note));
            card = Some((ui::group_card(), true));
            previous = heading;
        }
        let path = source.path.clone();
        let Some((mut current, first)) = card.take() else {
            continue;
        };
        current = current.child(
            ui::group_row(
                SharedString::from(format!("agent-policy-{at}")),
                first,
                Some(ui::number_tile((at + 1).to_string()).into_any_element()),
                source.name.clone(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens().space.s8))
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(policies::title(source)),
                    )
                    .child(policies::scope(source)),
            )
            .when(!source.applied, |el| el.opacity(0.45))
            .on_click(
                cx.listener(move |pane, _, _, cx| pane.open(Target::Policy(path.clone()), cx)),
            ),
        );
        card = Some((current, false));
    }
    if let Some((done, _)) = card {
        column = column.child(done);
    }
    for notice in &brief.notices {
        column = column.child(ui::group_note(notice.clone()));
    }
    ui::grouped_page("agent-policies", column).into_any_element()
}

fn usage_section(agent: &Agent) -> AnyElement {
    let usage: &Usage = match &agent.usage {
        None => return waiting("Reading its usage…"),
        Some(Err(error)) => return failed(error),
        Some(Ok(usage)) => usage,
    };
    let (t, c) = (tokens(), colors());
    let mut models: Vec<&str> = Vec::new();
    for model in usage.sessions.iter().flat_map(|session| &session.models) {
        if !models.contains(&model.as_str()) {
            models.push(model);
        }
    }
    let sessions = usage.sessions.len();
    let headline = div()
        .flex()
        .flex_shrink_0()
        .items_baseline()
        .gap(px(t.space.s12))
        .child(ui::text(div(), &t.text.figure).child(count(usage.totals.total)))
        .child(
            ui::text(div().flex_1().min_w_0().text_color(c.muted), &t.text.body).child(format!(
                "tokens in the last 24 h, {sessions} CLI session{}{}{}",
                if sessions == 1 { "" } else { "s" },
                // The team read stops at its limit; say so rather than claim a total.
                if usage.more {
                    " among the team's first 1000 sessions"
                } else {
                    ""
                },
                if models.is_empty() {
                    String::new()
                } else {
                    format!(", {}", models.join(", "))
                }
            )),
        )
        .child(usage::coverage(&usage.coverage));
    let mut list = list("agent-usage")
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .gap(px(t.space.s16))
        .child(headline)
        .child(usage::breakdown(usage.totals))
        .child(ui::view_heading("SESSIONS", "newest first").px(px(0.0)));
    let mut ordered: Vec<_> = usage.sessions.iter().collect();
    ordered.sort_by(|a, b| b.first_seen.cmp(&a.first_seen));
    for session in ordered {
        let short: String = session.id.chars().take(6).collect();
        let kind = if session.parent.is_some() {
            "subagent"
        } else if session.current {
            "current"
        } else {
            "ended"
        };
        list = list.child(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s12))
                    .h(px(t.size.row))
                    .text_color(c.muted),
                &t.text.status,
            )
            .child(mono(short).text_color(c.text))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!(
                        "{kind} · started {} ago · {}",
                        since(tmux_workspaces_roster::views::epoch_seconds(
                            &session.first_seen
                        )),
                        session.models.join(", ")
                    )),
            )
            .child(mono(count(session.tokens.total)).text_color(c.text_2))
            .child(usage::coverage(&session.coverage)),
        );
    }
    if usage.sessions.is_empty() {
        list = list.child(ui::text(div().text_color(c.muted), &t.text.body).child(
            match usage.unmeasured.first() {
                Some(unmeasured) => format!("No readable record: {}.", unmeasured.reason),
                None if usage.more => {
                    "None among the team's first 1000 sessions in the last 24 h.".into()
                }
                None => "No sessions in the last 24 h.".into(),
            },
        ));
    }
    list.into_any_element()
}

fn tab(id: SharedString, label: &'static str, mark: String, on: bool) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s4 + 1.0))
            .h(px(t.size.row))
            .px(px(t.space.s10))
            .border_b(px(t.size.focus_ring))
            .border_color(if on {
                c.accent
            } else {
                gpui::transparent_black().into()
            })
            .text_color(if on { c.text } else { c.muted })
            .cursor_pointer()
            .hover(|el| el.text_color(colors().text)),
        &t.text.row,
    )
    .when(on, |el| el.font_weight(t.text.row_selected.weight()))
    .child(label)
    .when(!mark.is_empty(), |el| {
        el.child(ui::text(div().text_color(c.muted), &t.text.meta).child(mark))
    })
}

fn overview(agent: &Agent, state: AgentState) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut body = list("agent-overview")
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .gap(px(t.space.s16));
    if agent.online && state == AgentState::NeedsYou {
        body = body.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s10))
                .p(px(t.space.s14))
                .rounded(px(t.radius.pane))
                .bg(c.attention_soft)
                .border(px(t.size.hairline))
                .border_color(c.attention)
                .child(
                    ui::text(div().flex().gap(px(t.space.s8)), &t.text.body)
                        .child(
                            div()
                                .text_color(c.attention_text)
                                .font_weight(t.text.label.weight())
                                .child("It asked you a question"),
                        )
                        .child(div().text_color(c.muted).child(format!(
                            "{} ago · it does nothing more until someone answers in its terminal",
                            agent.state_age_seconds.map(ago).unwrap_or_default()
                        ))),
                )
                .children(agent.last_message.as_ref().map(|message| {
                    ui::text(div().text_color(c.text), &t.text.doc_body).child(message.clone())
                }))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child("From its last reply, as its hook reported it."),
                ),
        );
    } else if let Some(detail) = &agent.detail {
        body = body
            .child(ui::text(div().text_color(c.attention_text), &t.text.body).child(line(detail)));
    }
    let model = agent
        .model
        .clone()
        .map(|model| format!(" · {model}"))
        .unwrap_or_else(|| " · its default model".into());
    let working = match (agent.current_issue, &agent.current_repo) {
        (Some(issue), Some(repo)) => format!("{repo}#{issue}"),
        (Some(issue), None) => format!("#{issue}"),
        _ => "nothing reported".into(),
    };
    let mut tags = div().flex().gap(px(t.space.s4));
    for tag in &agent.tags {
        tags = tags.child(ui::pill(tag.clone()));
    }
    body = body.child(
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .child(ui::fact(
                "CLI and model",
                div()
                    .flex()
                    .child(mono(agent.runtime.clone()).text_color(c.text))
                    .child(div().text_color(c.muted).child(model)),
            ))
            .child(ui::fact("Directory", mono(agent.dir.clone())))
            .child(ui::fact(
                "Watches",
                mono(if agent.watches.is_empty() {
                    "nothing".into()
                } else {
                    agent.watches.join(" · ")
                }),
            ))
            .child(ui::fact("Tags", tags))
            .child(ui::fact("Working on", mono(working)))
            .child(ui::fact(
                "In this state",
                div().child(
                    agent
                        .state_age_seconds
                        .map(ago)
                        .map(|age| format!("for {age}"))
                        .unwrap_or_else(|| "unknown".into()),
                ),
            )),
    );
    let holding: Vec<_> = agent.received.iter().filter(|m| held(m)).collect();
    body = body.child(messages(
        &format!("HELD FOR IT · {}", holding.len()),
        "delivered once it is free",
        &holding,
    ));
    let recent: Vec<_> = agent.received.iter().filter(|m| !held(m)).collect();
    body = body.child(messages(
        &format!("RECENT DELIVERIES · {}", recent.len()),
        "newest first",
        &recent,
    ));
    body.into_any_element()
}

fn messages(label: &str, note: &'static str, messages: &[&Received]) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut section = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s4))
        .child(ui::view_heading(label.to_owned(), note).px(px(0.0)));
    if messages.is_empty() {
        section =
            section.child(ui::text(div().text_color(c.muted), &t.text.status).child("nothing"));
    }
    for message in messages {
        section = section.child(
            ui::text(
                div().flex().items_baseline().gap(px(t.space.s10)),
                &t.text.status,
            )
            .child(
                mono(message.kind.replace('_', " "))
                    .flex_shrink_0()
                    .w(px(t.size.fact_label))
                    .text_color(c.muted),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(c.text_2)
                    .child(since(message.at)),
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
    section
}

fn connections(agent: &Agent) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let card = |title: &'static str, state: String, main: String, sub: String| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .p(px(t.space.s12))
            .rounded(px(t.radius.slot))
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .child(
                ui::text(
                    div().flex().items_center().text_color(c.muted),
                    &t.text.label,
                )
                .child(title)
                .child(div().flex_1())
                .child(ui::text(div().text_color(c.ok), &t.text.meta).child(state)),
            )
            .child(ui::text(div().text_color(c.text), &t.text.body).child(main))
            .child(ui::text(div().text_color(c.muted), &t.text.meta).child(sub))
    };
    let github = card(
        "GITHUB",
        if agent.watches.is_empty() {
            String::new()
        } else {
            "watching".into()
        },
        match (agent.watches.len(), agent.repo.is_empty()) {
            (0, true) => "Watches nothing, owns no repository".into(),
            (n, true) => format!("Watches {n} repositories, owns none"),
            (n, false) => format!("Owns {}, watches {n}", agent.repo),
        },
        format!("Issues labelled for:{} reach its queue.", agent.name),
    );
    let sources = card(
        "SOURCES",
        format!(
            "{} filter{}",
            agent.subscriptions.len(),
            if agent.subscriptions.len() == 1 {
                ""
            } else {
                "s"
            }
        ),
        if agent.subscriptions.is_empty() {
            "No subscriptions".into()
        } else {
            agent
                .subscriptions
                .iter()
                .map(|s| format!("{} · {}", s.source, s.filter))
                .collect::<Vec<_>>()
                .join(", ")
        },
        agent
            .subscriptions
            .first()
            .map(|s| format!("Priority {}.", s.priority))
            .unwrap_or_else(|| "Added with backbone agent subscribe.".into()),
    );
    list("agent-connections")
        .px(px(t.space.s18))
        .py(px(t.space.s14))
        .gap(px(t.space.s16))
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(ui::view_heading("MCP SERVERS", "not served by Backbone yet").px(px(0.0))),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .gap(px(t.space.s10))
                .child(github)
                .child(sources),
        )
        .into_any_element()
}

/// Whether the CLI's own settings let it act without any prompt.
fn skips_prompts(rules: &tmux_workspaces_roster::views::Permissions) -> bool {
    rules.modes.iter().any(|(name, value)| {
        matches!(
            (name.as_str(), value.as_str()),
            ("defaultMode", "bypassPermissions") | ("approval_policy", "never")
        )
    })
}

fn summary(title: String, sub: String) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s2 + 1.0))
        .px(px(t.space.s12))
        .py(px(t.space.s10))
        .rounded(px(t.radius.slot))
        .bg(c.field)
        .border(px(t.size.hairline))
        .border_color(c.line_strong)
        .child(ui::text(div().text_color(c.text), &t.text.row_selected).child(title))
        .child(ui::text(div().text_color(c.muted), &t.text.meta).child(sub))
}

fn permissions(agent: &Agent) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let rules = &agent.permissions;
    let launch = agent.launch.clone().unwrap_or_default();
    let mode = rules
        .modes
        .iter()
        .map(|(name, value)| format!("{name} {value}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let cards = div()
        .flex()
        .flex_shrink_0()
        .gap(px(t.space.s8))
        .child(if launch.unattended {
            summary(
                "Runs unattended".into(),
                "launched without permission prompts".into(),
            )
        } else if skips_prompts(rules) {
            summary(
                "Does not ask".into(),
                "its CLI settings skip prompts".into(),
            )
        } else {
            summary(
                "Asks before acting".into(),
                "except what its rules allow".into(),
            )
        })
        .child(summary(
            format!("{} rules pre-approved", rules.allow.len()),
            if rules.deny.is_empty() {
                "no deny rules".into()
            } else {
                format!("{} deny rules", rules.deny.len())
            },
        ))
        .child(summary(
            if mode.is_empty() {
                "CLI defaults".into()
            } else {
                mode
            },
            if launch.always_on {
                "kept running by Backbone".into()
            } else {
                "started on demand".into()
            },
        ));
    let row = |key: &'static str, value: &'static str, why: &'static str, warn: bool| {
        ui::text(
            div()
                .flex()
                .items_baseline()
                .gap(px(t.space.s12))
                .py(px(t.space.s6 + 1.0))
                .border_t(px(t.size.hairline))
                .border_color(c.line),
            &t.text.body,
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(t.size.fact_label + t.space.s24 * 2.0))
                .text_color(c.text_2)
                .child(key),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(t.size.fact_label))
                .font_weight(t.text.row_selected.weight())
                .text_color(if warn { c.accent_text } else { c.text })
                .child(value),
        )
        .child(ui::text(div().flex_1().min_w_0().text_color(c.muted), &t.text.meta).child(why))
    };
    let set = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(
            ui::view_heading(
                "SET BY BACKBONE AT LAUNCH",
                "changes apply at the next start",
            )
            .px(px(0.0)),
        )
        .child(row(
            "Approval mode",
            if launch.unattended {
                "Unattended"
            } else {
                "Attended"
            },
            if launch.unattended {
                "Started so it acts without asking."
            } else {
                "Backbone does not start it unattended; its CLI's own settings below still apply."
            },
            launch.unattended,
        ))
        .child(row(
            "Always on",
            if launch.always_on { "Yes" } else { "No" },
            "Backbone restarts it when it stops.",
            false,
        ))
        .child(row(
            "Inbox only",
            if launch.inbox_only { "Yes" } else { "No" },
            "Messages wait in its inbox instead of its terminal.",
            false,
        ));
    let files: Vec<String> = rules
        .files
        .iter()
        .filter(|file| file.read)
        .map(|file| file.path.clone())
        .collect();
    let chips = |items: &[String]| {
        let mut chips = div().flex_1().flex().flex_wrap().gap(px(t.space.s4 + 1.0));
        if items.is_empty() {
            chips = chips.child(ui::text(div().text_color(c.muted), &t.text.status).child("none"));
        }
        for item in items {
            chips = chips.child(
                ui::text(
                    div()
                        .px(px(t.space.s6 + 1.0))
                        .py(px(t.space.s2))
                        .rounded(px(t.radius.key + 1.0))
                        .bg(c.hover)
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                        .font_family(t.font.mono.clone())
                        .text_color(c.text_2),
                    &t.text.meta,
                )
                .child(item.clone()),
            );
        }
        chips
    };
    let kind = |label: &'static str, color: gpui::Rgba| {
        ui::text(
            div()
                .flex_shrink_0()
                .w(px(t.size.view_choice + t.space.s8))
                .pt(px(t.space.s2))
                .text_color(color),
            &t.text.status,
        )
        .child(label)
    };
    let own = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s8))
        .child(
            ui::view_heading(format!("{}'S OWN RULES", agent.runtime.to_uppercase()), "")
                .px(px(0.0))
                .child(mono(files.join(" · ")).text_color(c.muted)),
        )
        .child(if rules.files.is_empty() {
            ui::text(div().text_color(c.muted), &t.text.status).child(format!(
                "This app does not read {}'s settings.",
                agent.runtime
            ))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .child(
                    div()
                        .flex()
                        .child(kind("allow", c.ok))
                        .child(chips(&rules.allow)),
                )
                .child(
                    div()
                        .flex()
                        .child(kind("deny", c.attention_text))
                        .child(chips(&rules.deny)),
                )
                .child(
                    div()
                        .flex()
                        .child(kind("ask", c.muted))
                        .child(chips(&rules.ask)),
                )
        });
    list("agent-permissions")
        .px(px(t.space.s18))
        .py(px(t.space.s14))
        .gap(px(t.space.s14))
        .child(cards)
        .child(set)
        .child(own)
        .into_any_element()
}
