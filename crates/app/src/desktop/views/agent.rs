//! Agent: everything about one agent, in sections drawn inside this pane.
//! Its state and work, what its CLI lets it do, its connections, the skills it
//! loads and why each reaches it, what it receives at its next start, and its
//! usage. Choosing a skill or policy reads it here; nothing opens elsewhere.

use agent_interface_core::AgentSection;
use agent_interface_roster::{
    Background, Waiting,
    profiles::runs::{Run, Runs},
    views::{
        Agent, Instructions, Loadout, LocalSkill, Process, Reason, Received, Usage, epoch_seconds,
        now,
    },
};
use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};

use super::{
    Target, ViewPane, ago, count, home, line, list, mono, policies, reader, since, skills, usage,
};
use crate::desktop::{
    agent_status::counted,
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
    // A skill or policy opened from the profile's agent card shows in the
    // reader; going back shows the card. Only that card sets this data.
    let header =
        ui::view_header("user", agent.name.clone()).child(ui::badge(agent.runtime.clone()));
    match page(pane, agent) {
        Some(page) => reader::render(pane, header, page, cx),
        None => div()
            .size_full()
            .children(pane.bound_agent.clone())
            .into_any_element(),
    }
}

pub(super) fn label(section: AgentSection) -> &'static str {
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
                    .map(|source| Target::Policy(source.path.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
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
            let at = sources
                .iter()
                .position(|source| source.path.as_ref() == path)?;
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
                        // A skill kept without a folder has only its name.
                        .when(row.path != row.name, |el| {
                            el.child(mono(row.path.to_owned()).text_color(c.muted))
                        }),
                );
            Some(reader::Page {
                back: format!("{} · Skills", agent.name),
                name: row.name.to_owned(),
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

/// Rows open in the reader through `open`; without it they only list.
pub(super) fn skills_section<V: 'static>(
    agent: &Agent,
    cx: &mut Context<V>,
    open: Option<fn(&mut V, Target, &mut Context<V>)>,
) -> AnyElement {
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
                .when_some(open, |row, open| {
                    row.on_click(cx.listener(move |host, _, _, cx| open(host, target.clone(), cx)))
                }),
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

/// Rows open in the reader through `open`; without it they only list.
pub(super) fn policies_section<V: 'static>(
    agent: &Agent,
    cx: &mut Context<V>,
    open: Option<fn(&mut V, Target, &mut Context<V>)>,
) -> AnyElement {
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
        let path = source.path.to_string();
        let Some((mut current, first)) = card.take() else {
            continue;
        };
        current = current.child(
            ui::group_row(
                SharedString::from(format!("agent-policy-{at}")),
                first,
                Some(ui::number_tile((at + 1).to_string()).into_any_element()),
                source.name.to_string(),
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
            .when_some(open, |row, open| {
                row.on_click(
                    cx.listener(move |host, _, _, cx| open(host, Target::Policy(path.clone()), cx)),
                )
            }),
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

pub(super) fn usage_section(agent: &Agent) -> AnyElement {
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
                        since(agent_interface_roster::views::epoch_seconds(
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

pub(super) fn tab(
    id: SharedString,
    label: &'static str,
    mark: String,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
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

/// `state` is the agent's state as the pane's header shows it: the card's
/// details apply only while it is the state they were read with. `runs` are
/// the agent's from its profile's roster.
pub(super) fn overview(
    agent: &Agent,
    state: AgentState,
    runs: &Runs,
    machine_label: Option<String>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let matching = state == AgentState::from_runtime(&agent.state);
    let age = matching.then_some(agent.state_age_seconds).flatten();
    let stale_evidence = (agent.state_source.as_deref() == Some("stale")
        && state != AgentState::Offline)
        .then(|| {
            crate::desktop::agent_status::stale_note(&agent.evidence)
                .unwrap_or_else(|| "State report is stale.".into())
        });
    let mut body = list("agent-overview")
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .gap(px(t.space.s16));
    if let Some(stale) = &stale_evidence {
        body = body.child(
            ui::text(div().text_color(c.attention_text), &t.text.body)
                .child(format!("Stale state · {}", line(stale))),
        );
    }
    if state.needs_you() && matching {
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
                            age.map(ago).unwrap_or_default()
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
    } else if matching && let Some(detail) = &agent.detail {
        body = body
            .child(ui::text(div().text_color(c.attention_text), &t.text.body).child(line(detail)));
    }
    let working = match (agent.current_issue, &agent.current_repo) {
        (Some(issue), Some(repo)) => format!("{repo}#{issue}"),
        (Some(issue), None) => format!("#{issue}"),
        _ => "nothing reported".into(),
    };
    let mut tags = div().flex().gap(px(t.space.s4));
    for tag in &agent.tags {
        tags = tags.child(ui::pill(tag.clone()));
    }
    let waiting = agent
        .waiting
        .as_ref()
        .map(|waiting| ui::fact("Waiting on", div().child(waits(waiting, now() as f64))));
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
                    .child(div().text_color(c.muted).child(format!(
                        " · {}",
                        model_label(agent.model.as_deref(), agent.model_source.as_deref())
                    ))),
            ))
            .children(machine_label.map(|machine| ui::fact("Machine", div().child(machine))))
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
            .children(waiting)
            .child(ui::fact(
                "In this state",
                div().child(
                    age.map(ago)
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
    body = body.children(background(agent, runs));
    body.into_any_element()
}

/// Finished runs the Background section lists.
const FINISHED_SHOWN: usize = 5;

/// What an agent waits on, why and for how long, on one line.
fn waits(waiting: &Waiting, now: f64) -> String {
    let mut parts = vec![match &waiting.on_agent {
        Some(agent) => format!("{} {agent}", waiting.on),
        None => waiting.on.clone(),
    }];
    parts.extend(waiting.reason.clone().filter(|reason| !reason.is_empty()));
    let since = age(waiting.since.as_deref(), now);
    if !since.is_empty() {
        parts.push(format!("for {since}"));
    }
    line(&parts.join(" · "))
}

/// How long before `now` an RFC 3339 time was; empty when it doesn't parse.
fn age(at: Option<&str>, now: f64) -> String {
    at.and_then(epoch_seconds)
        .map(|at| ago(now - at))
        .unwrap_or_default()
}

/// One row of the Background section: what it is, a time, its result, which
/// always shows, and the rest, which gives way first.
#[derive(Debug, PartialEq)]
struct Work {
    what: String,
    when: String,
    result: String,
    text: String,
}

/// A run's label, what a queued run waits on, and its last progress line.
fn described(run: &Run) -> String {
    let mut parts = vec![run.label.clone()];
    parts.extend(run.waiting_on.as_ref().map(|on| format!("waits on {on}")));
    parts.extend(run.progress.clone());
    line(&parts.join(" · "))
}

/// A queued or running run, with how long it has queued or run.
fn active(run: &Run, now: f64) -> Work {
    let since = if run.state == "queued" {
        &run.queued_at
    } else {
        &run.started_at
    };
    Work {
        what: run.kind.clone(),
        when: age(since.as_deref(), now),
        result: line(&run.state),
        text: described(run),
    }
}

/// A run that ended, with when it ended and how long it ran.
fn ended(run: &Run, now: f64) -> Work {
    let mut result = vec![run.state.clone()];
    result.extend(run.exit_code.map(|code| format!("exit {code}")));
    let time = |at: &Option<String>| at.as_deref().and_then(epoch_seconds);
    if let (Some(start), Some(end)) = (time(&run.started_at), time(&run.finished_at)) {
        result.push(format!("ran {}", ago(end - start)));
    }
    Work {
        what: run.kind.clone(),
        when: age(run.finished_at.as_deref(), now),
        result: line(&result.join(" · ")),
        text: described(run),
    }
}

fn process(process: &Process, now: f64) -> Work {
    Work {
        what: format!("pid {}", process.pid),
        when: age(process.started_at.as_deref(), now),
        result: String::new(),
        text: line(&process.command),
    }
}

/// The Background section's rows: active runs, the latest finished ones, then
/// processes.
fn background_rows(agent: &Agent, runs: &Runs, now: f64) -> Vec<Work> {
    runs.active
        .iter()
        .map(|run| active(run, now))
        .chain(
            runs.finished
                .iter()
                .take(FINISHED_SHOWN)
                .map(|run| ended(run, now)),
        )
        .chain(agent.processes.iter().map(|each| process(each, now)))
        .collect()
}

/// What the counts say when there are no rows to list. The counts come with
/// the card's own read and the runs with the roster's, so counts without rows
/// are runs not read yet.
fn quiet(counts: Background) -> String {
    if counts == Background::default() {
        return "Nothing runs in the background.".into();
    }
    let processes = match counts.processes {
        0 => None,
        1 => Some("1 process".to_owned()),
        many => Some(format!("{many} processes")),
    };
    counted(counts.running, counts.queued, ", ")
        .into_iter()
        .chain(processes)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line above the Background rows: with no rows, what the counts say;
/// with fewer active runs listed than the counts report, the running and
/// queued counts, since the list has not caught up or isn't shown.
fn reported(counts: Background, rows: usize, active: usize) -> Option<String> {
    if rows == 0 {
        return Some(quiet(counts));
    }
    if counts.running.saturating_add(counts.queued) > active as u64 {
        return counted(counts.running, counts.queued, ", ");
    }
    None
}

/// What the agent runs in the background; nothing for a service that doesn't
/// report it.
fn background(agent: &Agent, runs: &Runs) -> Option<gpui::Div> {
    let counts = agent.background?;
    let (t, c) = (tokens(), colors());
    let mut section = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s4))
        .child(ui::view_heading("BACKGROUND", "finished ones since this app opened").px(px(0.0)));
    let rows = background_rows(agent, runs, now() as f64);
    if let Some(said) = reported(counts, rows.len(), runs.active.len()) {
        section = section.child(ui::text(div().text_color(c.muted), &t.text.status).child(said));
    }
    for row in rows {
        section = section.child(
            ui::text(
                div().flex().items_baseline().gap(px(t.space.s10)),
                &t.text.status,
            )
            .child(
                mono(row.what)
                    .flex_shrink_0()
                    .w(px(t.size.fact_label))
                    .text_color(c.muted),
            )
            .child(div().flex_shrink_0().text_color(c.text_2).child(row.when))
            .when(!row.result.is_empty(), |el| {
                el.child(div().flex_shrink_0().text_color(c.text_2).child(row.result))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(row.text),
            ),
        );
    }
    Some(section)
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

pub(super) fn connections(agent: &Agent) -> AnyElement {
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

fn model_label(model: Option<&str>, source: Option<&str>) -> String {
    match (model, source) {
        (Some(model), Some("observed")) => format!("{model} (reported by the runtime)"),
        (Some(model), Some("configured")) => format!("{model} (configured)"),
        (Some(model), Some("runtime_default")) => format!("{model} (runtime default)"),
        (Some(model), _) => model.into(),
        (None, Some("runtime_default")) => "its default model".into(),
        (None, _) => "model unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The service's fixture of its active runs, and of a run that ended.
    fn fixture_runs() -> (Vec<Run>, Run) {
        let listed: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/runs/fixtures/runs.json"
        )))
        .unwrap();
        let changed: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../roster/src/profiles/runs/fixtures/run-changed.json"
        )))
        .unwrap();
        (
            serde_json::from_str(&listed["items"].to_string()).unwrap(),
            serde_json::from_str(&changed["run"].to_string()).unwrap(),
        )
    }

    fn at(time: &str) -> f64 {
        epoch_seconds(time).unwrap()
    }

    #[test]
    fn model_labels_distinguish_reported_and_configured_values() {
        assert_eq!(
            model_label(Some("m"), Some("observed")),
            "m (reported by the runtime)"
        );
        assert_eq!(model_label(Some("m"), Some("configured")), "m (configured)");
        assert_eq!(
            model_label(None, Some("runtime_default")),
            "its default model"
        );
        assert_eq!(model_label(Some("m"), Some("future")), "m");
    }

    #[test]
    fn a_wait_says_on_what_why_and_for_how_long() {
        let mut waiting = Waiting {
            on: "agent".into(),
            on_agent: Some("reviewer".into()),
            reason: Some("Waiting for the review of the protocol change.".into()),
            since: Some("2026-11-10T10:20:00Z".into()),
        };
        assert_eq!(
            waits(&waiting, at("2026-11-10T10:23:00Z")),
            "agent reviewer · Waiting for the review of the protocol change. · for 3m"
        );
        // A time that doesn't parse shows no duration.
        waiting.since = Some("soon".into());
        waiting.on_agent = None;
        waiting.reason = None;
        assert_eq!(waits(&waiting, at("2026-11-10T10:23:00Z")), "agent");
    }

    #[test]
    fn runs_say_what_they_do_and_for_how_long() {
        let (runs, failed) = fixture_runs();
        assert_eq!(
            active(&runs[0], at("2026-11-10T10:22:30Z")),
            Work {
                what: "review".into(),
                when: "10m".into(),
                result: "running".into(),
                text: "Review round 2, gpt-example · Reviewing crates/protocol/src/runs.rs".into(),
            }
        );
        // A queued run counts from when it was queued.
        assert_eq!(
            active(&runs[1], at("2026-11-10T10:14:05Z")),
            Work {
                what: "build".into(),
                when: "1m".into(),
                result: "queued".into(),
                text: "cargo build --workspace · waits on build slot".into(),
            }
        );
        assert_eq!(
            ended(&failed, at("2026-11-10T10:23:47Z")),
            Work {
                what: "test".into(),
                when: "2m".into(),
                result: "failed · exit 101 · ran 7m".into(),
                text: "cargo test --workspace · test result: FAILED. 211 passed; 1 failed".into(),
            }
        );
        let mut unknown = failed.clone();
        unknown.finished_at = Some("later".into());
        let work = ended(&unknown, at("2026-11-10T10:23:47Z"));
        assert_eq!(work.when, "");
        assert_eq!(work.result, "failed · exit 101");
    }

    #[test]
    fn the_background_lists_active_runs_five_finished_then_processes() {
        let (runs, failed) = fixture_runs();
        let agent = Agent {
            background: Some(Background {
                running: 1,
                queued: 0,
                processes: 1,
            }),
            processes: vec![Process {
                pid: 48213,
                command: "cargo test --workspace".into(),
                started_at: Some("2026-11-10T10:14:02Z".into()),
            }],
            ..Agent::default()
        };
        let book = Runs {
            active: vec![runs[0].clone()],
            finished: vec![failed; 7],
            ..Runs::default()
        };
        let rows = background_rows(&agent, &book, at("2026-11-10T10:24:02Z"));
        let what: Vec<_> = rows.iter().map(|row| row.what.as_str()).collect();
        assert_eq!(
            what,
            [
                "review",
                "test",
                "test",
                "test",
                "test",
                "test",
                "pid 48213"
            ]
        );
        assert_eq!(rows[6].when, "10m");
        assert!(background_rows(&Agent::default(), &Runs::default(), 0.0).is_empty());
        // With nothing to list, the line says what the counts say.
        assert_eq!(
            quiet(Background::default()),
            "Nothing runs in the background."
        );
        assert_eq!(quiet(agent.background.unwrap()), "1 running, 1 process");
        let processes = Background {
            queued: 3,
            processes: 2,
            ..Background::default()
        };
        assert_eq!(quiet(processes), "3 queued, 2 processes");
        // Processes alone still say something rather than nothing.
        let processes = Background {
            processes: 2,
            ..Background::default()
        };
        assert_eq!(quiet(processes), "2 processes");
        // Rows that don't list every reported run say so above them.
        let counts = Background {
            running: 1,
            queued: 1,
            processes: 1,
        };
        assert_eq!(
            reported(counts, 1, 0).as_deref(),
            Some("1 running, 1 queued")
        );
        assert_eq!(reported(counts, 3, 2), None);
        assert_eq!(
            reported(counts, 0, 0).as_deref(),
            Some("1 running, 1 queued, 1 process")
        );
    }
}
