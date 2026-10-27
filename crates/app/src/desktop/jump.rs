//! Jump (Command-K): search tabs, panes, agents and commands across every
//! workspace, and go to the chosen one.

use gpui::{
    Context, Entity, Pixels, Point, ScrollHandle, SharedString, Subscription, Window, div,
    prelude::*, px,
};

use super::{
    Browser, components as ui,
    design::{colors, tokens},
    layout_edit::Create,
    name::{NameEvent, NameField},
};
use agent_interface_core::Direction;
use ui::AgentState;

/// The open jump popover.
pub(super) struct Jump {
    field: Entity<NameField>,
    query: String,
    index: usize,
    scroll: ScrollHandle,
    _events: Subscription,
}

impl Jump {
    /// Whether an input method is composing in the search field.
    pub(super) fn composing(&self, cx: &gpui::App) -> bool {
        self.field.read(cx).composing()
    }
}

/// Where a result goes.
#[derive(Clone)]
pub(super) enum Target {
    Tab {
        workspace: String,
        tab: String,
    },
    Pane {
        workspace: String,
        tab: String,
        pane: String,
    },
    Agent(String),
    Command(Create),
    NewWorkspace,
    Settings,
}

/// One result row.
#[derive(Clone)]
pub(super) struct Hit {
    pub group: &'static str,
    pub label: String,
    pub meta: String,
    pub agent: Option<AgentState>,
    pub target: Target,
}

impl Browser {
    /// Open the jump popover at `at`, or close it if it is open.
    pub(super) fn open_jump(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.jump.is_some() {
            self.close_jump(window, cx);
            return;
        }
        self.close_menus();
        self.menu_at = Some(at);
        let field = cx.new(|cx| NameField::search(window, cx));
        let events = cx.subscribe_in(&field, window, |browser, _, event, window, cx| {
            match event {
                NameEvent::Changed(query) => {
                    if let Some(jump) = &mut browser.jump {
                        jump.query = query.clone();
                        jump.index = 0;
                        jump.scroll = ScrollHandle::new();
                    }
                }
                NameEvent::Save => browser.jump_go(window, cx),
                NameEvent::Cancel => browser.close_jump(window, cx),
            }
            cx.notify();
        });
        let focus = field.read(cx).focus.clone();
        window.focus(&focus, cx);
        self.jump = Some(Jump {
            field,
            query: String::new(),
            index: 0,
            scroll: ScrollHandle::new(),
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.jump = None;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    /// Move the highlighted result.
    pub(super) fn jump_move(&mut self, step: isize, cx: &mut Context<Self>) {
        let hits = self.jump_hits();
        if let Some(jump) = &mut self.jump
            && !hits.is_empty()
        {
            jump.index = (jump.index as isize + step).rem_euclid(hits.len() as isize) as usize;
            // Each group's heading is a row before its results; reveal the
            // highlighted result's row.
            let headings = hits[..=jump.index]
                .iter()
                .enumerate()
                .filter(|(at, hit)| *at == 0 || hits[at - 1].group != hit.group)
                .count();
            jump.scroll.scroll_to_item(jump.index + headings);
            cx.notify();
        }
    }

    /// Every result matching the query, in group order.
    pub(super) fn jump_hits(&self) -> Vec<Hit> {
        let query = self
            .jump
            .as_ref()
            .map(|jump| jump.query.to_lowercase())
            .unwrap_or_default();
        let matches = |text: &str| query.is_empty() || text.to_lowercase().contains(&query);
        let mut hits = Vec::new();
        let states = self.agent_states();
        if let Some(layout) = &self.layout {
            for space in &layout.model().workspaces {
                for tab in &space.tabs {
                    if matches(&tab.name) {
                        hits.push(Hit {
                            group: "TABS",
                            label: tab.name.clone(),
                            meta: space.name.clone(),
                            agent: None,
                            target: Target::Tab {
                                workspace: space.id.clone(),
                                tab: tab.id.clone(),
                            },
                        });
                    }
                }
            }
            for space in &layout.model().workspaces {
                for tab in &space.tabs {
                    for leaf in tab.tree.leaves() {
                        let Some(attachment) = &leaf.attachment else {
                            continue;
                        };
                        if matches(&attachment.session) {
                            hits.push(Hit {
                                group: "PANES",
                                label: attachment.session.clone(),
                                meta: format!("{} · {}", tab.name, space.name),
                                agent: self.attachment_state(attachment, states.as_ref()),
                                target: Target::Pane {
                                    workspace: space.id.clone(),
                                    tab: tab.id.clone(),
                                    pane: leaf.id.clone(),
                                },
                            });
                        }
                    }
                }
            }
        }
        if let Some(states) = &states {
            let mut agents: Vec<_> = states.iter().collect();
            agents.sort_by_key(|(name, state)| (!state.needs_you(), name.to_lowercase()));
            for (name, state) in agents {
                if matches(name) {
                    hits.push(Hit {
                        group: "AGENTS",
                        label: name.clone(),
                        meta: state.word().into(),
                        agent: Some(*state),
                        target: Target::Agent(name.clone()),
                    });
                }
            }
        }
        for (label, target) in [
            ("New tab", Target::Command(Create::Tab)),
            (
                "Split right",
                Target::Command(Create::Split(Direction::Right)),
            ),
            (
                "Split below",
                Target::Command(Create::Split(Direction::Below)),
            ),
            ("New workspace", Target::NewWorkspace),
            ("Settings", Target::Settings),
        ] {
            if matches(label) {
                hits.push(Hit {
                    group: "COMMANDS",
                    label: label.into(),
                    meta: String::new(),
                    agent: None,
                    target,
                });
            }
        }
        hits
    }

    /// Go to the highlighted result.
    pub(super) fn jump_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.jump.as_ref().map_or(0, |jump| jump.index);
        let Some(hit) = self.jump_hits().into_iter().nth(index) else {
            return;
        };
        self.jump = None;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        match hit.target {
            Target::Tab { workspace, tab } => {
                if let Some(layout) = &mut self.layout {
                    let model = layout.model_mut();
                    model.choose_workspace(&workspace);
                    model.choose_tab(&tab);
                }
            }
            Target::Pane {
                workspace,
                tab,
                pane,
            } => {
                if let Some(layout) = &mut self.layout {
                    let model = layout.model_mut();
                    model.choose_workspace(&workspace);
                    model.choose_tab(&tab);
                    model.choose_pane(&pane);
                }
            }
            Target::Agent(name) => self.go_to_agent(&name, window, cx),
            Target::Command(action) => self.create(action, window, cx),
            Target::NewWorkspace => self.begin_workspace(window, cx),
            Target::Settings => cx.defer(super::settings_window::show),
        }
        cx.notify();
    }

    /// The popover's contents.
    pub(super) fn jump_popover(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let jump = self.jump.as_ref()?;
        let (t, c) = (tokens(), colors());
        let field = jump.field.clone();
        let index = jump.index;
        let hits = self.jump_hits();
        let mut list = div()
            .id("jump-results")
            .p(px(t.size.popover_padding))
            .flex()
            .flex_col()
            .max_h(px(t.size.window_min_height - t.size.titlebar * 2.0))
            .overflow_y_scroll()
            .track_scroll(&jump.scroll);
        let mut group = "";
        for (position, hit) in hits.iter().enumerate() {
            if hit.group != group {
                group = hit.group;
                list = list.child(ui::menu_heading(group));
            }
            let chosen = position == index;
            list = list.child(
                ui::menu_item(
                    SharedString::from(format!("jump-{position}")),
                    None,
                    hit.label.clone(),
                    None,
                )
                .when(chosen, |el| el.bg(colors().selected))
                .when_some(hit.agent, |el, state| el.child(ui::agent_symbol(state)))
                .when(!hit.meta.is_empty(), |el| {
                    el.child(
                        ui::text(
                            div().flex_shrink_0().text_color(colors().muted),
                            &tokens().text.meta,
                        )
                        .child(hit.meta.clone()),
                    )
                })
                .on_click(cx.listener(move |browser, _, window, cx| {
                    if let Some(jump) = &mut browser.jump {
                        jump.index = position;
                    }
                    browser.jump_go(window, cx);
                })),
            );
        }
        if hits.is_empty() {
            list = list.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .h(px(t.size.row))
                        .flex()
                        .items_center()
                        .text_color(c.muted),
                    &t.text.body,
                )
                .child("Nothing matches"),
            );
        }
        let hint = |key: &'static str, word: &'static str| {
            div()
                .flex()
                .gap(px(tokens().space.s4))
                .child(div().text_color(colors().text_2).child(key))
                .child(word)
        };
        Some(
            ui::menu()
                .p(px(0.0))
                .min_w(px(t.size.popover_min_width * 2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s10))
                        .h(px(t.size.row + t.space.s10))
                        .px(px(t.space.s12))
                        .border_b(px(t.size.hairline))
                        .border_color(c.line)
                        .child(ui::icon("search", t.size.icon_large, c.muted))
                        .child(div().flex_1().min_w_0().child(field))
                        .child(
                            ui::text(div().text_color(c.muted), &t.text.meta)
                                .child("all workspaces"),
                        ),
                )
                .child(list)
                .child(
                    ui::text(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s16))
                            .h(px(t.size.row))
                            .px(px(t.space.s12))
                            .border_t(px(t.size.hairline))
                            .border_color(c.line)
                            .text_color(c.muted),
                        &t.text.meta,
                    )
                    .child(hint("↑↓", "move"))
                    .child(hint("↵", "go"))
                    .child(div().flex_1())
                    .child(hint("esc", "close")),
                ),
        )
    }
}
