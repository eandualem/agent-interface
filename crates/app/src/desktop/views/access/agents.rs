//! Board A: the agents grouped by role, and the chosen agent's place and
//! access, one section per kind, each with its changes.
use super::{Adding, Change, Menu, Pane, model};
use crate::desktop::{
    components::{self as ui, Button, Tone},
    design::{colors, tokens},
    views::connections::hinted,
};
use agent_interface_roster::profiles::access::{
    Access, Agent, Grant, Mode, Reach, RequestState, Role, Source,
};
use gpui::{AnyElement, Context, Div, SharedString, Stateful, div, prelude::*, px};

/// How many main agents the list shows before "N more".
const MAIN_SHOWN: usize = 8;

/// The machine of this app's own profile, named as the board names it.
pub(super) fn this_machine() -> &'static str {
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
            .child(self.header("shield", "Access", |profile| {
                format!("what each agent in {profile} may reach, and who is above whom")
            }))
            .child(self.toolbar(model, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.list(model, cx))
                    .child(self.detail(model, cx)),
            )
    }

    /// A header of the pane's views: its icon and title, what it shows of
    /// this profile, by the profile's name, and when it was read.
    pub(super) fn header(
        &self,
        icon: &str,
        title: &'static str,
        subtitle: impl FnOnce(&str) -> String,
    ) -> Div {
        let c = colors();
        let profile = self.profile_name.as_deref().unwrap_or("this profile");
        ui::view_header(icon, title)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(subtitle(profile)),
            )
            .children(self.live_note())
    }

    /// The button that shows the waiting requests, and from them the agents
    /// again.
    pub(super) fn waiting_button(&self, waiting: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let (t, c) = (tokens(), colors());
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
        ))
        .on_click(cx.listener(|pane, _, window, cx| pane.toggle_requests(window, cx)))
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
            .filter(|r| r.state == RequestState::Pending)
            .count();
        ui::view_toolbar()
            .child(
                ui::field_frame(false)
                    .w(px(t.size.access_filter))
                    .h(px(t.size.chip + t.space.s2))
                    .child(ui::icon("search", t.size.icon - 1.0, c.muted))
                    .child(hinted(&self.query, "Filter agents", cx)),
            )
            .children(chips)
            .child(div().flex_1())
            .when(waiting > 0, |bar| {
                bar.child(self.waiting_button(waiting, cx))
            })
            .child(self.browsers_switch(cx))
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
            let agents = groups.swarm_agents();
            list = list.child(heading(
                "SWARMS",
                format!(
                    "{} · {agents} agent{}",
                    groups.swarms.len(),
                    if agents == 1 { "" } else { "s" }
                ),
                first,
            ));
            for swarm in &groups.swarms {
                let open = self.open_swarms.contains(&swarm.name);
                let name = swarm.name.clone();
                let members = swarm.members.len();
                let count = format!("{members} agent{}", if members == 1 { "" } else { "s" });
                let note = match &swarm.initiator {
                    Some(by) => format!("{count} · started by {by}"),
                    None => count,
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
        let secrets = model::secrets(agent);
        let browsers = model::browsers(agent);
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
            // Board A's chip on a row: smaller than a tag elsewhere, so the
            // name keeps its room beside the counts.
            row.child(
                ui::tag("unsandboxed", Tone::Quiet)
                    .flex()
                    .items_center()
                    .h(px(t.size.keycap))
                    .px(px(t.space.s6))
                    .text_size(t.text.key.size()),
            )
        })
        .when(browsers + secrets > 0, |row| {
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
        .on_click(cx.listener(move |pane, _, window, cx| {
            if pane.selected.as_deref() != Some(name.as_str()) {
                pane.selected = Some(name.clone());
                pane.newest_due = true;
                pane.read_newest(cx);
                pane.menu = None;
                pane.adding = None;
                pane.notice = None;
                pane.leave_field(window, cx);
            }
            cx.notify();
        }))
    }

    fn detail(&self, model: &model::Model, cx: &mut Context<Self>) -> Div {
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
            .children(self.notice_band(cx))
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
                    .child(self.role_and_place(agent, model, cx))
                    .child(self.confinement(agent, cx))
                    .child(self.github(agent, cx))
                    .children(self.browser_sign_ins(agent, model, cx))
                    .child(self.folders(agent, cx))
                    .child(self.secrets_and_docker(agent, cx)),
            )
            .child(ui::note_footer(
                "Saved to Agent Backbone as you change it. You are the owner: nothing here is above you.",
                format!("backbone access show {}", agent.name),
            ))
    }

    /// A control, dimmed while its change is under way.
    pub(super) fn busy<E: Styled>(&self, key: &str, element: E) -> E {
        if self.sending.as_deref() == Some(key) {
            element.opacity(colors().pane_dim)
        } else {
            element
        }
    }

    /// A menu's button and, while it is open, the menu below it.
    pub(super) fn with_menu(
        &self,
        menu: Menu,
        button: Stateful<Div>,
        items: Vec<Stateful<Div>>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens();
        // A menu with nothing to choose is not opened.
        if items.is_empty() {
            return div()
                .relative()
                .flex_shrink_0()
                .child(button.opacity(colors().pane_dim).cursor_default());
        }
        let open = self.menu == Some(menu);
        div()
            .relative()
            .flex_shrink_0()
            .child(button.on_click(cx.listener(move |pane, _, _, cx| {
                // A press on the button of the menu it just closed only closes it.
                let dismissed = ui::just_dismissed(&mut pane.dismissed, menu);
                pane.menu = if pane.menu == Some(menu) || dismissed {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            })))
            .when(open, |el| {
                el.child(
                    gpui::deferred(
                        ui::menu()
                            .id(SharedString::from(format!("access-menu-{menu:?}")))
                            .absolute()
                            .top(px(t.size.icon_button + t.space.s4))
                            // The More menu's button is at the right edge.
                            .when(menu == Menu::BrowserMore, |el| el.right_0())
                            .when(menu != Menu::BrowserMore, |el| el.left_0())
                            .max_h(px(t.size.access_menu))
                            .overflow_y_scroll()
                            .on_mouse_down_out(cx.listener(move |pane, _, _, cx| {
                                pane.menu = None;
                                pane.dismissed = Some((menu, std::time::Instant::now()));
                                cx.notify();
                            }))
                            .children(items),
                    )
                    .with_priority(1),
                )
            })
    }

    fn role_and_place(&self, agent: &Agent, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let c = colors();
        let role = agent.role.unwrap_or(Role::Unknown);
        let superior = agent.superior.clone();
        let roles = Role::ALL
            .into_iter()
            .map(|choice| {
                let (name, superior) = (agent.name.clone(), superior.clone());
                ui::menu_item(
                    SharedString::from(format!("access-role-{choice:?}")),
                    (choice == role).then(|| ui::icon("check", tokens().size.icon, c.accent)),
                    model::role_label(choice),
                    None,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.menu = None;
                    if choice != role {
                        pane.send(
                            "access-role".into(),
                            Change::Place(name.clone(), choice, superior.clone()),
                            cx,
                        );
                    }
                    cx.notify();
                }))
            })
            .collect();
        let others = std::iter::once(None).chain(
            model
                .overview
                .agents
                .iter()
                .filter(|other| other.name != agent.name)
                .map(|other| Some(other.name.clone())),
        );
        let superiors = others
            .map(|choice| {
                let name = agent.name.clone();
                let current = choice == superior;
                let label = choice.clone().unwrap_or_else(|| "you".into());
                ui::menu_item(
                    SharedString::from(format!("access-superior-{label}")),
                    current.then(|| ui::icon("check", tokens().size.icon, c.accent)),
                    label,
                    None,
                )
                .when(choice.is_some(), |el| {
                    el.font_family(tokens().font.mono.clone())
                })
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.menu = None;
                    if !current {
                        pane.send(
                            "access-superior".into(),
                            Change::Place(name.clone(), role, choice.clone()),
                            cx,
                        );
                    }
                    cx.notify();
                }))
            })
            .collect();
        section(
            "ROLE AND PLACE",
            None,
            ui::group_card()
                .child(labelled(true, "Role").child(self.with_menu(
                    Menu::Role,
                    self.busy(
                        "access-role",
                        dropdown("access-role".into(), model::role_label(role).into(), false),
                    ),
                    roles,
                    cx,
                )))
                .child(labelled(false, "Answers to").child(self.with_menu(
                    Menu::Superior,
                    self.busy(
                        "access-superior",
                        dropdown(
                            "access-superior".into(),
                            superior.clone().unwrap_or_else(|| "you".into()),
                            superior.is_some(),
                        ),
                    ),
                    superiors,
                    cx,
                ))),
        )
    }

    fn confinement(&self, agent: &Agent, cx: &mut Context<Self>) -> Div {
        let c = colors();
        let reaches = match agent.mode {
            Mode::Sandboxed => Some("Reaches only what is listed below.".to_owned()),
            Mode::Unsandboxed => Some(format!(
                "Reaches everything your account can on {}.",
                this_machine()
            )),
            Mode::Unknown => None,
        };
        let segment = |mode: Mode, label: &'static str| {
            let name = agent.name.clone();
            let on = agent.mode == mode;
            ui::segment(
                SharedString::from(format!("access-mode-{label}")),
                label,
                on,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                if !on {
                    pane.send("access-mode".into(), Change::Mode(name.clone(), mode), cx);
                }
            }))
        };
        section(
            "CONFINEMENT",
            Some("only you can change this".into()),
            ui::group_card().child(
                labelled(true, "Runs")
                    .child(
                        self.busy(
                            "access-mode",
                            ui::segments()
                                .child(segment(Mode::Sandboxed, "Sandboxed"))
                                .child(segment(Mode::Unsandboxed, "Unsandboxed")),
                        ),
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

    /// Read and Write as a segmented control. With `grant`, choosing the
    /// other access grants it again at that access, which replaces it.
    fn access_segments(
        &self,
        id: &str,
        access: Access,
        grant: Option<(String, Grant)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let segment = |choice: Access, label: &'static str| {
            let segment = ui::segment(
                SharedString::from(format!("{id}-{label}")),
                label,
                access == choice,
            );
            match grant.clone().filter(|_| access != choice) {
                Some((name, grant)) => {
                    let key = id.to_owned();
                    segment.on_click(cx.listener(move |pane, _, _, cx| {
                        let regranted = match grant.clone() {
                            Grant::Repository { repo, .. } => Grant::Repository {
                                repo,
                                access: choice,
                            },
                            Grant::Folder { machine, path, .. } => Grant::Folder {
                                machine,
                                machine_name: None,
                                path,
                                access: choice,
                            },
                            other => other,
                        };
                        pane.send(key.clone(), Change::Grant(name.clone(), regranted), cx);
                    }))
                }
                // Not changeable here: its own repository or its role.
                None => segment.cursor_default(),
            }
        };
        self.busy(
            id,
            ui::segments()
                .child(segment(Access::Read, "Read"))
                .child(segment(Access::Write, "Write")),
        )
    }

    /// The mark that takes `grant` away from the agent `name`.
    fn remove(
        &self,
        key: String,
        name: &str,
        grant: Grant,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let name = name.to_owned();
        let id = format!("{key}-remove");
        let busy = key.clone();
        self.busy(
            &busy,
            ui::icon_button(SharedString::from(id), "x").on_click(cx.listener(
                move |pane, _, _, cx| {
                    pane.send(key.clone(), Change::Revoke(name.clone(), grant.clone()), cx);
                },
            )),
        )
    }

    /// A card's last line: "Add a …", or while adding, the field, the access
    /// it will have and the Add button.
    fn add_line(&self, first: bool, repository: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let (label, hint, id) = if repository {
            ("Add a repository", "owner/name", "access-add-repo")
        } else {
            ("Add a folder", "/absolute/path", "access-add-folder")
        };
        let adding = match self.adding {
            Some(Adding::Repository(access)) if repository => Some(access),
            Some(Adding::Folder(access)) if !repository => Some(access),
            _ => None,
        };
        let line = ui::text(
            div()
                .id(SharedString::from(id))
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s10))
                .h(px(t.size.group_row))
                .pl(px(t.space.s12))
                .pr(px(t.space.s8))
                .when(!first, |el| {
                    el.border_t(px(t.size.hairline)).border_color(c.line)
                })
                .text_color(c.muted),
            &t.text.row,
        )
        .child(ui::icon("plus", t.size.icon, c.muted));
        let make = move |access: Access| {
            if repository {
                Adding::Repository(access)
            } else {
                Adding::Folder(access)
            }
        };
        match adding {
            None => line
                .child(label)
                .cursor_pointer()
                .hover(|el| el.bg(colors().hover))
                .on_click(cx.listener(move |pane, _, window, cx| {
                    pane.start_add(make(Access::Read), window, cx);
                })),
            Some(access) => {
                let segment = |choice: Access, label: &'static str| {
                    ui::segment(
                        SharedString::from(format!("{id}-{label}")),
                        label,
                        access == choice,
                    )
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        pane.start_add(make(choice), window, cx);
                    }))
                };
                line.child(
                    ui::field_frame(true)
                        .flex_1()
                        .h(px(t.size.chip + t.space.s2))
                        .child(hinted(&self.add_field, hint, cx)),
                )
                .child(
                    ui::segments()
                        .child(segment(Access::Read, "Read"))
                        .child(segment(Access::Write, "Write")),
                )
                .child(
                    self.busy(
                        "access-add",
                        ui::button(
                            SharedString::from(format!("{id}-submit")),
                            "Add",
                            Button::Secondary,
                        )
                        .h(px(t.size.icon_button))
                        .on_click(cx.listener(|pane, _, window, cx| {
                            if pane.submit_add(cx) {
                                pane.leave_field(window, cx);
                            }
                        })),
                    ),
                )
            }
        }
    }

    /// A grant's machine as its chip: this app's own as "this Mac", another
    /// by the name Backbone serves, or "another machine" once it names none.
    pub(super) fn machine_chip(&self, id: Option<&str>, name: Option<&str>) -> Div {
        let local = &self.connection.identity().installation_id;
        ui::machine_chip(model::machine_label(this_machine(), local, id, name))
    }

    /// Whether a grant is on this app's own machine, or names none.
    pub(super) fn on_this_machine(&self, id: Option<&String>) -> bool {
        id.is_none_or(|id| id == &self.connection.identity().installation_id)
    }

    fn github(&self, agent: &Agent, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let note = match &agent.reach {
            Reach::Profile { .. } => {
                "every repository, through the profile's GitHub connection".to_owned()
            }
            Reach::Repositories { .. } => format!(
                "{}, through the profile's GitHub connection",
                model::reach_label(&agent.reach)
            ),
            Reach::Unknown => "what it reaches is not known".to_owned(),
        };
        let rows = model::github_rows(agent);
        let mut card = ui::group_card();
        for (at, row) in rows.iter().enumerate() {
            let model::GithubRow {
                repo,
                access,
                source,
                grant,
            } = row;
            let key = format!("access-repo-{repo}");
            // Only a granted access can be changed here; its own repository's
            // and its role's follow from what the agent is.
            let changeable = grant.clone().filter(|_| *source == Source::Grant);
            card = card.child(
                line(at == 0)
                    .child(ui::icon("git-branch", t.size.icon, c.muted))
                    .child(ui::text(
                        ui::mono(repo.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .when(*source == Source::OwnRepository, |el| {
                        el.child(ui::tag("its own repository", Tone::Quiet))
                    })
                    .child(div().flex_1())
                    .child(self.access_segments(
                        &key,
                        *access,
                        changeable.map(|grant| (agent.name.clone(), grant)),
                        cx,
                    ))
                    .child(match grant {
                        Some(grant) => self
                            .remove(key.clone(), &agent.name, grant.clone(), cx)
                            .into_any_element(),
                        None => no_remove().into_any_element(),
                    }),
            );
        }
        section(
            "GITHUB",
            Some(note),
            card.child(self.add_line(rows.is_empty(), true, cx)),
        )
    }

    /// One row per named browser, its switch giving it to the agent or
    /// taking it away; none while there are no browsers.
    fn browser_sign_ins(
        &self,
        agent: &Agent,
        model: &model::Model,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        let items = &model.browsers.items;
        if items.is_empty() {
            return None;
        }
        let now = chrono::Local::now().fixed_offset();
        let mut card = ui::group_card();
        let mut given = 0;
        for (at, browser) in items.iter().enumerate() {
            let granted = agent
                .grants
                .iter()
                .find(|grant| super::browsers::names(grant, &browser.id))
                .cloned();
            given += usize::from(granted.is_some());
            let note = super::browsers::row_note(
                model.newest.get(&browser.id),
                &agent.name,
                browser,
                &now,
            );
            let key = format!("access-browser-switch-{}", browser.id);
            let change = match granted.clone() {
                Some(grant) => Change::Revoke(agent.name.clone(), grant),
                None => Change::Grant(
                    agent.name.clone(),
                    super::browsers::browser_grant(None, browser),
                ),
            };
            let switch_key = key.clone();
            card = card.child(
                line(at == 0)
                    .pr(px(t.space.s12))
                    .child(super::browsers::letter_mark(
                        &browser.name,
                        t.space.s20,
                        true,
                    ))
                    .child(div().text_color(c.text).child(browser.name.clone()))
                    .child(ui::text(
                        ui::mono(super::browsers::sites(browser)).text_color(c.muted),
                        &t.text.key,
                    ))
                    .when(!self.on_this_machine(Some(&browser.machine)), |el| {
                        el.child(self.machine_chip(
                            Some(browser.machine.as_str()),
                            browser.machine_name.as_deref(),
                        ))
                    })
                    .child(div().flex_1())
                    .child(ui::text(div().text_color(c.muted), &t.text.meta).child(note))
                    .child(self.busy(
                        &key,
                        ui::switch(SharedString::from(key.clone()), granted.is_some()).on_click(
                            cx.listener(move |pane, _, _, cx| {
                                pane.send(switch_key.clone(), change.clone(), cx);
                            }),
                        ),
                    )),
            );
        }
        let local = items.iter().all(|b| self.on_this_machine(Some(&b.machine)));
        let note = if local {
            format!(
                "{given} of {} sites you signed in to on {}",
                items.len(),
                this_machine()
            )
        } else {
            format!("{given} of {} sites you signed in to", items.len())
        };
        Some(section("BROWSER SIGN-INS", Some(note), card).child(
            ui::text(div().px(px(t.space.s2)).text_color(c.muted), &t.text.meta).child(
                "It opens these sites already signed in, in tabs of its own. It never sees \
                     a password, or another agent's tabs.",
            ),
        ))
    }

    fn folders(&self, agent: &Agent, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut card = ui::group_card();
        let mut first = true;
        for grant in &agent.grants {
            let Grant::Folder {
                machine,
                machine_name,
                path,
                access,
            } = grant
            else {
                continue;
            };
            let key = format!(
                "access-folder-{}-{path}",
                machine.as_deref().unwrap_or_default()
            );
            card = card.child(
                line(first)
                    .child(ui::icon("folder", t.size.icon, c.muted))
                    .child(ui::text(
                        ui::mono(path.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .child(self.machine_chip(machine.as_deref(), machine_name.as_deref()))
                    .child(div().flex_1())
                    .child(self.access_segments(
                        &key,
                        *access,
                        Some((agent.name.clone(), grant.clone())),
                        cx,
                    ))
                    .child(self.remove(key.clone(), &agent.name, grant.clone(), cx)),
            );
            first = false;
        }
        section(
            "FOLDERS",
            Some("beyond its own folder".into()),
            card.child(self.add_line(first, false, cx)),
        )
    }

    fn secrets_and_docker(&self, agent: &Agent, cx: &mut Context<Self>) -> Div {
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
            let key = format!(
                "access-secret-{}-{label}",
                id.as_deref().unwrap_or_default()
            );
            let (agent_name, revoked) = (agent.name.clone(), grant.clone());
            let switch_key = key.clone();
            rows.push(
                line(rows.is_empty())
                    .pr(px(t.space.s12))
                    .child(ui::icon("lock", t.size.icon, c.muted))
                    .child(ui::text(
                        ui::mono(label.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    // The section says it is on this machine; others are named.
                    .when(!self.on_this_machine(id.as_ref()), |el| {
                        el.child(self.machine_chip(id.as_deref(), name.as_deref()))
                    })
                    .child(div().flex_1())
                    .child(self.busy(
                        &key,
                        ui::switch(SharedString::from(key.clone()), true).on_click(cx.listener(
                            move |pane, _, _, cx| {
                                pane.send(
                                    switch_key.clone(),
                                    Change::Revoke(agent_name.clone(), revoked.clone()),
                                    cx,
                                );
                            },
                        )),
                    ))
                    .into_any_element(),
            );
        }
        // One row per Docker grant, each taken away by its switch; with none,
        // one row whose switch grants it on the agent's own machine.
        let dockers: Vec<Option<Grant>> = {
            let granted: Vec<Option<Grant>> = agent
                .grants
                .iter()
                .filter(|grant| matches!(grant, Grant::Docker { .. }))
                .cloned()
                .map(Some)
                .collect();
            if granted.is_empty() {
                vec![None]
            } else {
                granted
            }
        };
        for docker in dockers {
            let (id, name) = match &docker {
                Some(Grant::Docker {
                    machine,
                    machine_name,
                }) => (machine.clone(), machine_name.clone()),
                _ => (None, None),
            };
            let key = format!("access-docker-{}", id.as_deref().unwrap_or_default());
            let (agent_name, switch_key) = (agent.name.clone(), key.clone());
            let on = docker.is_some();
            rows.push(
                line(rows.is_empty())
                    .pr(px(t.space.s12))
                    .child(ui::icon("package", t.size.icon, c.muted))
                    .child("Docker")
                    .when(!self.on_this_machine(id.as_ref()), |el| {
                        el.child(self.machine_chip(id.as_deref(), name.as_deref()))
                    })
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
                    .child(self.busy(
                        &key,
                        ui::switch(SharedString::from(key.clone()), on).on_click(cx.listener(
                            move |pane, _, _, cx| {
                                let change = match docker.clone() {
                                    Some(grant) => Change::Revoke(agent_name.clone(), grant),
                                    None => Change::Grant(
                                        agent_name.clone(),
                                        Grant::Docker {
                                            machine: None,
                                            machine_name: None,
                                        },
                                    ),
                                };
                                pane.send(switch_key.clone(), change, cx);
                            },
                        )),
                    ))
                    .into_any_element(),
            );
        }
        section(
            "SECRETS AND DOCKER",
            Some(format!("on {}", this_machine())),
            ui::group_card().children(rows),
        )
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

/// The space a row without a remove mark keeps, so access controls line up.
fn no_remove() -> Div {
    div().w(px(tokens().size.icon_button)).flex_shrink_0()
}
