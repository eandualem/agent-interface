//! Board A: the agents grouped by role, and the chosen agent's place and
//! access, one section per kind.
use super::{Pane, model};
use crate::desktop::{
    components::{self as ui, Tone},
    design::{colors, tokens},
};
use agent_interface_roster::profiles::access::{Access, Agent, Grant, Mode, Reach, Source};
use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px};

/// How many main agents the list shows before "N more".
const MAIN_SHOWN: usize = 8;

/// The machine of this app's own profile, named as the board names it.
fn this_machine() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    }
}

impl Pane {
    pub(super) fn agents_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = &self.model else {
            return div();
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.header())
            .child(self.toolbar(model, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.list(model, cx))
                    .child(self.detail(model)),
            )
    }

    fn header(&self) -> Div {
        let (t, c) = (tokens(), colors());
        let profile = self.profile_name.as_deref().unwrap_or("this profile");
        ui::view_header("shield", "Access")
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(format!(
                        "what each agent in {profile} may reach, and who is above whom"
                    )),
            )
            .children(self.live_note().map(|note| ui::text(note, &t.text.meta)))
    }

    fn toolbar(&self, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let agents = &model.overview.agents;
        let chips = model::Filter::ALL.into_iter().map(|filter| {
            let label = match filter {
                model::Filter::All => "All",
                model::Filter::AboveOthers => "Above others",
                model::Filter::Browser => "Uses a browser",
                model::Filter::Unsandboxed => "Unsandboxed",
            };
            ui::chip(
                SharedString::from(format!("access-filter-{label}")),
                format!("{label} · {}", filter.count(agents)),
                self.filter == filter,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.filter = filter;
                cx.notify();
            }))
        });
        let waiting = model
            .pending
            .iter()
            .filter(|r| r.state == agent_interface_roster::profiles::access::RequestState::Pending)
            .count();
        ui::view_toolbar()
            .child(
                ui::field_frame(false)
                    .w(px(t.size.access_filter))
                    .h(px(t.size.chip + t.space.s2))
                    .child(ui::icon("search", t.size.icon - 1.0, c.muted))
                    .child(super::super::connections::hinted(
                        &self.query,
                        "Filter agents",
                        cx,
                    )),
            )
            .children(chips)
            .child(div().flex_1())
            .when(waiting > 0, |bar| {
                bar.child(
                    ui::text(
                        div()
                            .id("access-requests-waiting")
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(t.space.s6))
                            .cursor_pointer()
                            .text_color(c.accent_text),
                        &t.text.meta,
                    )
                    .child(ui::icon("clock", t.size.icon - 1.0, c.accent_text))
                    .child(format!(
                        "{waiting} request{} waiting",
                        if waiting == 1 { "" } else { "s" }
                    )),
                )
            })
    }

    fn list(&self, model: &model::Model, cx: &mut Context<Self>) -> Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let query = self.query.read(cx).value().to_owned();
        let groups = model::matching(&model.overview, self.filter, &query);
        let heading = |label: &'static str, note: String, first: bool| {
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s6))
                    .h(px(t.size.row))
                    .px(px(t.space.s8))
                    .when(!first, |el| el.mt(px(t.space.s4)))
                    .text_color(c.muted),
                &t.text.label,
            )
            .child(label)
            .child(ui::text(div(), &t.text.meta).child(note))
        };
        let mut list = div()
            .id("access-agents")
            .w(px(t.size.access_list))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline))
            .px(px(t.space.s8))
            .py(px(t.space.s6))
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .overflow_y_scroll();
        let mut first = true;
        if !groups.orchestrators.is_empty() {
            list = list.child(heading(
                "ORCHESTRATORS",
                format!("{} · above every other agent", groups.orchestrators.len()),
                first,
            ));
            first = false;
            list = list.children(
                groups
                    .orchestrators
                    .iter()
                    .map(|a| self.agent_row(a, model, cx)),
            );
        }
        if !groups.coordinators.is_empty() {
            list = list.child(heading(
                "COORDINATORS",
                format!(
                    "{} · above the agents under them",
                    groups.coordinators.len()
                ),
                first,
            ));
            first = false;
            list = list.children(
                groups
                    .coordinators
                    .iter()
                    .map(|a| self.agent_row(a, model, cx)),
            );
        }
        if !groups.main.is_empty() {
            list = list.child(heading("MAIN AGENTS", groups.main.len().to_string(), first));
            first = false;
            let shown = if self.all_main {
                groups.main.len()
            } else {
                MAIN_SHOWN
            };
            list = list.children(
                groups
                    .main
                    .iter()
                    .take(shown)
                    .map(|a| self.agent_row(a, model, cx)),
            );
            let more = groups.main.len().saturating_sub(shown);
            if more > 0 {
                list = list.child(
                    ui::row("access-more-main", false)
                        .h(px(t.size.row))
                        .pl(px(t.space.s10 + t.size.access_mark + t.space.s8))
                        .text_color(c.muted)
                        .child(ui::text(div().flex_1(), &t.text.meta).child(format!("{more} more")))
                        .child(ui::icon("chevron-right", t.size.icon - 1.0, c.muted))
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.all_main = true;
                            cx.notify();
                        })),
                );
            }
        }
        if !groups.swarms.is_empty() {
            list = list.child(heading(
                "SWARMS",
                format!(
                    "{} · {} agent{}",
                    groups.swarms.len(),
                    groups.swarm_agents(),
                    if groups.swarm_agents() == 1 { "" } else { "s" }
                ),
                first,
            ));
            for swarm in &groups.swarms {
                let open = self.open_swarms.contains(&swarm.name);
                let name = swarm.name.clone();
                let members = swarm.members.len();
                let note = match &swarm.initiator {
                    Some(by) => format!(
                        "{members} agent{} · started by {by}",
                        if members == 1 { "" } else { "s" }
                    ),
                    None => format!("{members} agent{}", if members == 1 { "" } else { "s" }),
                };
                list = list.child(
                    self.row_frame(SharedString::from(format!("access-swarm-{name}")), false)
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(t.size.access_mark))
                                .rounded(px(t.radius.icon_button))
                                .bg(c.selected)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui::icon("users", t.size.icon - 1.0, c.muted)),
                        )
                        .child(two_lines(&swarm.name, note, false))
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            if !pane.open_swarms.remove(&name) {
                                pane.open_swarms.insert(name.clone());
                            }
                            cx.notify();
                        })),
                );
                if open {
                    list =
                        list.children(swarm.members.iter().map(|a| self.agent_row(a, model, cx)));
                }
            }
        }
        list
    }

    fn row_frame(&self, id: SharedString, selected: bool) -> Stateful<Div> {
        let t = tokens();
        ui::row(id, selected)
            .h(px(t.size.access_row))
            .px(px(t.space.s8))
    }

    fn agent_row(
        &self,
        agent: &Agent,
        model: &model::Model,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let selected = self.selected.as_deref() == Some(agent.name.as_str());
        let asks = model::asks(&model.pending, &agent.name);
        let browsers = model::browsers(agent);
        let secrets = model::secrets(agent);
        let name = agent.name.clone();
        let count = |icon: &'static str, n: usize| {
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s2 + 1.0))
                .child(ui::icon(icon, t.size.icon - 2.0, c.muted))
                .child(n.to_string())
        };
        self.row_frame(
            SharedString::from(format!("access-agent-{}", agent.name)),
            selected,
        )
        .child(ui::agent_mark(&agent.name, t.size.access_mark))
        .child(two_lines(
            &agent.name,
            format!(
                "{} · {}",
                model::answers_to(agent),
                model::reach_label(&agent.reach)
            ),
            selected,
        ))
        .when(asks > 0, |row| {
            row.child(
                ui::text(
                    div().flex_shrink_0().text_color(c.accent_text),
                    &t.text.meta,
                )
                .child(format!("asks {asks}")),
            )
        })
        .when(agent.mode == Mode::Unsandboxed, |row| {
            row.child(ui::tag("unsandboxed", Tone::Quiet))
        })
        .when(browsers > 0 || secrets > 0, |row| {
            row.child(
                ui::text(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .when(browsers > 0, |el| el.child(count("globe", browsers)))
                .when(secrets > 0, |el| el.child(count("lock", secrets))),
            )
        })
        .on_click(cx.listener(move |pane, _, _, cx| {
            pane.selected = Some(name.clone());
            cx.notify();
        }))
    }

    fn detail(&self, model: &model::Model) -> Div {
        let (t, c) = (tokens(), colors());
        let Some(agent) = self.selected.as_deref().and_then(|name| model.agent(name)) else {
            return div().flex_1();
        };
        let chain = model::chain(&model.overview.agents, &agent.name);
        let mut above = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s4 + 1.0))
            .text_color(c.muted)
            .child("answers to");
        for name in &chain {
            above = above
                .child(ui::mono(name.clone()).text_color(c.text_2))
                .child(ui::icon("chevron-right", t.size.icon - 3.0, c.muted));
        }
        above = above.child(div().text_color(c.text_2).child("you"));
        let header = ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s10))
                .h(px(t.size.access_header))
                .pl(px(t.space.s24))
                .pr(px(t.space.s16))
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .overflow_hidden()
                .whitespace_nowrap(),
            &t.text.meta,
        )
        .child(ui::agent_mark(&agent.name, t.size.access_mark_large))
        .child(ui::text(
            ui::mono(agent.name.clone()).text_color(c.text),
            &t.text.empty_title,
        ))
        .children(agent.role.map(|role| ui::pill(model::role_label(role))))
        .child(above);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("access-detail")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s16))
                    .px(px(t.space.s24))
                    .pt(px(t.space.s16))
                    .pb(px(t.space.s20))
                    .child(role_and_place(agent))
                    .child(confinement(agent))
                    .child(github(agent))
                    .child(folders(agent))
                    .child(secrets_and_docker(agent)),
            )
            .child(ui::note_footer(
                "Saved to Agent Backbone as you change it. You are the owner: nothing here is above you.",
                format!("backbone access show {}", agent.name),
            ))
    }
}

/// A row's name in mono over its note.
fn two_lines(name: &str, note: String, selected: bool) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s2 + 1.0))
        .child(
            ui::text(ui::mono(name.to_owned()), &t.text.status)
                .text_color(if selected { c.text } else { c.text_2 })
                .when(selected, |el| el.font_weight(t.text.row_selected.weight())),
        )
        .child(
            ui::text(
                div().overflow_hidden().text_ellipsis().text_color(c.muted),
                &t.text.meta,
            )
            .child(note),
        )
}

/// A section of the detail: its uppercase label and note, then its card.
fn section(label: &'static str, note: Option<String>, card: Div) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .gap(px(t.space.s6))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(t.space.s8))
                .px(px(t.space.s2))
                .child(ui::text(div().text_color(c.muted), &t.text.label).child(label))
                .children(
                    note.map(|note| ui::text(div().text_color(c.muted), &t.text.meta).child(note)),
                ),
        )
        .child(card)
}

/// One line on a section's card, separated from the line above by a
/// hairline.
fn line(first: bool) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .pl(px(t.space.s12))
            .pr(px(t.space.s8))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(c.text_2)
            .when(!first, |el| {
                el.border_t(px(t.size.hairline)).border_color(c.line)
            }),
        &t.text.row,
    )
}

/// A labelled line, such as Role or Runs.
fn labelled(first: bool, label: &'static str) -> Div {
    let t = tokens();
    line(first).child(
        div()
            .w(px(t.size.access_label))
            .flex_shrink_0()
            .child(label),
    )
}

/// A choice drawn as the board's dropdown: what is chosen and the up-down
/// chevrons.
fn dropdown(id: SharedString, value: String, mono: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.icon_button))
            .px(px(t.space.s8))
            .rounded(px(t.radius.icon_button))
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .text_color(c.text)
            .cursor_pointer()
            .when(mono, |el| el.font_family(t.font.mono.clone())),
        &t.text.status,
    )
    .child(value)
    .child(ui::icon("chevrons-up-down", t.size.icon - 2.0, c.muted))
}

/// Read and Write as a segmented control, the agent's access chosen.
fn access_segments(id: &str, access: Access) -> Div {
    ui::segments()
        .child(ui::segment(
            SharedString::from(format!("{id}-read")),
            "Read",
            access == Access::Read,
        ))
        .child(ui::segment(
            SharedString::from(format!("{id}-write")),
            "Write",
            access == Access::Write,
        ))
}

/// A card's last line, which adds a thing: a plus and what it adds.
fn add_line(id: SharedString, label: &'static str) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .px(px(t.space.s12))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted)
            .cursor_pointer()
            .hover(|el| el.bg(colors().hover)),
        &t.text.row,
    )
    .child(ui::icon("plus", t.size.icon, c.muted))
    .child(label)
}

/// The mark that takes a grant away.
fn remove(id: SharedString) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::icon_button(id, "x").child(ui::icon("x", t.size.icon - 1.0, c.muted))
}

/// The space a row without a remove mark keeps, so access controls line up.
fn no_remove() -> Div {
    div().w(px(tokens().size.icon_button)).flex_shrink_0()
}

fn role_and_place(agent: &Agent) -> Div {
    let role = agent
        .role
        .map(model::role_label)
        .unwrap_or("Role not known");
    let superior = agent.superior.clone().unwrap_or_else(|| "you".into());
    section(
        "ROLE AND PLACE",
        None,
        ui::group_card()
            .child(labelled(true, "Role").child(dropdown("access-role".into(), role.into(), false)))
            .child(labelled(false, "Answers to").child(dropdown(
                "access-superior".into(),
                superior.clone(),
                agent.superior.is_some(),
            ))),
    )
}

fn confinement(agent: &Agent) -> Div {
    let c = colors();
    let reaches = match agent.mode {
        Mode::Sandboxed => Some("Reaches only what is listed below.".to_owned()),
        Mode::Unsandboxed => Some(format!(
            "Reaches everything your account can on {}.",
            this_machine()
        )),
        Mode::Unknown => None,
    };
    section(
        "CONFINEMENT",
        Some("only you can change this".into()),
        ui::group_card().child(
            labelled(true, "Runs")
                .child(
                    ui::segments()
                        .child(ui::segment(
                            "access-sandboxed",
                            "Sandboxed",
                            agent.mode == Mode::Sandboxed,
                        ))
                        .child(ui::segment(
                            "access-unsandboxed",
                            "Unsandboxed",
                            agent.mode == Mode::Unsandboxed,
                        )),
                )
                .children(reaches.map(|line| {
                    ui::text(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .flex()
                            .justify_end()
                            .text_color(c.muted),
                        &tokens().text.meta,
                    )
                    .child(line)
                })),
        ),
    )
}

fn github(agent: &Agent) -> Div {
    let (t, c) = (tokens(), colors());
    let (items, note) = match &agent.reach {
        Reach::Profile { items } => (
            &items[..],
            "every repository, through the profile's GitHub connection".to_owned(),
        ),
        Reach::Repositories { items } => (
            &items[..],
            format!(
                "{}, through the profile's GitHub connection",
                model::reach_label(&agent.reach)
            ),
        ),
        Reach::Unknown => (&[][..], "what it reaches is not known".to_owned()),
    };
    let mut card = ui::group_card();
    for (at, item) in items.iter().enumerate() {
        let id = format!("access-repo-{}", item.repo);
        card = card.child(
            line(at == 0)
                .child(ui::icon("git-branch", t.size.icon, c.muted))
                .child(ui::text(
                    ui::mono(item.repo.clone()).text_color(c.text),
                    &t.text.status,
                ))
                .when(item.source == Source::OwnRepository, |el| {
                    el.child(ui::tag("its own repository", Tone::Quiet))
                })
                .child(div().flex_1())
                .child(access_segments(&id, item.access))
                .child(if item.source == Source::Grant {
                    remove(SharedString::from(format!("{id}-remove"))).into_any_element()
                } else {
                    no_remove().into_any_element()
                }),
        );
    }
    let card = if items.is_empty() {
        card.child(add_line("access-add-repo".into(), "Add a repository").border_t_0())
    } else {
        card.child(add_line("access-add-repo".into(), "Add a repository"))
    };
    section("GITHUB", Some(note), card)
}

/// A grant's machine as its chip, named "this Mac" for this app's own.
fn machine(name: Option<&String>, id: Option<&String>) -> Option<Div> {
    let _ = id;
    Some(ui::machine_chip(
        name.cloned().unwrap_or_else(|| this_machine().to_owned()),
    ))
}

fn folders(agent: &Agent) -> Div {
    let (t, c) = (tokens(), colors());
    let mut card = ui::group_card();
    let mut first = true;
    for grant in &agent.grants {
        let Grant::Folder {
            machine: id,
            machine_name,
            path,
            access,
        } = grant
        else {
            continue;
        };
        let key = format!("access-folder-{path}");
        card = card.child(
            line(first)
                .child(ui::icon("folder", t.size.icon, c.muted))
                .child(ui::text(
                    ui::mono(path.clone()).text_color(c.text),
                    &t.text.status,
                ))
                .children(machine(machine_name.as_ref(), id.as_ref()))
                .child(div().flex_1())
                .child(access_segments(&key, *access))
                .child(remove(SharedString::from(format!("{key}-remove")))),
        );
        first = false;
    }
    let add = add_line("access-add-folder".into(), "Add a folder");
    section(
        "FOLDERS",
        Some("beyond its own folder".into()),
        card.child(if first { add.border_t_0() } else { add }),
    )
}

fn secrets_and_docker(agent: &Agent) -> Div {
    let (t, c) = (tokens(), colors());
    let mut rows: Vec<AnyElement> = Vec::new();
    for grant in &agent.grants {
        let (label, id, name) = match grant {
            Grant::SecretVariable {
                machine,
                machine_name,
                name,
            } => (name, machine, machine_name),
            Grant::SecretFile {
                machine,
                machine_name,
                path,
            } => (path, machine, machine_name),
            _ => continue,
        };
        rows.push(
            line(rows.is_empty())
                .pr(px(t.space.s12))
                .child(ui::icon("lock", t.size.icon, c.muted))
                .child(ui::text(
                    ui::mono(label.clone()).text_color(c.text),
                    &t.text.status,
                ))
                .when(name.is_some(), |el| {
                    el.children(machine(name.as_ref(), id.as_ref()))
                })
                .child(div().flex_1())
                .child(ui::switch(
                    SharedString::from(format!("access-secret-{label}")),
                    true,
                ))
                .into_any_element(),
        );
    }
    let docker = agent
        .grants
        .iter()
        .any(|grant| matches!(grant, Grant::Docker { .. }));
    rows.push(
        line(rows.is_empty())
            .pr(px(t.space.s12))
            .child(ui::icon("package", t.size.icon, c.muted))
            .child("Docker")
            .child(
                ui::text(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child("Only you can allow this. No agent can hand it down."),
            )
            .child(ui::switch("access-docker", docker))
            .into_any_element(),
    );
    section(
        "SECRETS AND DOCKER",
        Some(format!("on {}", this_machine())),
        ui::group_card().children(rows),
    )
}
