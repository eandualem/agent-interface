//! The navigation panel: workspace block, jump field, tabs, agents and the
//! workspace switcher. Layout and colours come from the design tokens.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use gpui::{AnyElement, Context, EntityId, SharedString, Window, div, prelude::*, px};

use super::{
    Browser, Popover, components as ui,
    design::{colors, tokens},
    icons, settings,
};
use crate::desktop::layout_edit::Create;
use ui::AgentState;

#[derive(Clone)]
struct TabDrag {
    browser: EntityId,
    workspace: String,
    tab: String,
    name: String,
}

impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::row("dragged-tab", true)
            .bg(colors().panel)
            .text_color(colors().text)
            .child(self.name.clone())
    }
}

fn reorder_tab_proposal(
    layout: &crate::library::WindowLayout,
    workspace: &str,
    source: &str,
    destination: &str,
) -> Option<(
    agent_interface_store::Document,
    agent_interface_core::Layout,
)> {
    let (base, mut proposed) = layout.save_snapshot();
    if proposed.selected != workspace || source == destination {
        return None;
    }
    let space = proposed
        .workspaces
        .iter_mut()
        .find(|space| space.id == workspace)?;
    let from = space.tabs.iter().position(|tab| tab.id == source)?;
    let to = space.tabs.iter().position(|tab| tab.id == destination)?;
    let tab = space.tabs.remove(from);
    space.tabs.insert(to, tab);
    Some((base, proposed))
}

/// Socket identities resolved by a background worker, never while drawing.
#[derive(Default)]
pub(super) struct SourceMatches {
    inherited: bool,
    explicit: HashSet<String>,
}

impl SourceMatches {
    fn resolve(source: Option<PathBuf>, sockets: HashSet<String>) -> Self {
        let resolve = |path: &std::path::Path| crate::library::location::resolve_path(path).ok();
        let source = source.as_deref().and_then(resolve);
        Self {
            inherited: source.is_some(),
            explicit: sockets
                .into_iter()
                .filter(|socket| source.is_some() && resolve(socket.as_ref()) == source)
                .collect(),
        }
    }

    pub(super) fn contains(&self, socket: Option<&str>) -> bool {
        socket.map_or(self.inherited, |socket| self.explicit.contains(socket))
    }
}

impl Browser {
    fn drop_tab(&mut self, dragged: &TabDrag, destination: &str, cx: &mut Context<Self>) {
        if dragged.browser != cx.entity_id() || self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(proposal) =
            reorder_tab_proposal(layout, &dragged.workspace, &dragged.tab, destination)
        else {
            return;
        };
        self.pending_save = Some(proposal);
        self.name_click = None;
        self.close_menus();
        self.save(cx);
    }

    /// Each agent's state, or none while Agent Backbone is unreachable.
    pub(super) fn agent_states(&self) -> Option<HashMap<String, AgentState>> {
        let snapshot = self.agents.as_ref()?;
        if snapshot.error.is_some() {
            return None;
        }
        Some(
            snapshot
                .states
                .iter()
                .filter(|(_, state)| state.as_str() != "offline")
                .map(|(name, state)| (name.to_owned(), AgentState::from_report(state)))
                .collect(),
        )
    }

    /// Refresh identities after library or roster updates. Keep at most one
    /// lookup running; a later roster poll picks up changes made while it ran.
    pub(super) fn refresh_source_matches(&mut self, cx: &mut Context<Self>) {
        if self.roster.is_none() || self.source_match_updates.is_some() {
            return;
        }
        let source = self
            .source
            .as_ref()
            .and_then(|source| source.as_ref().ok())
            .cloned();
        let sockets = self
            .layout
            .iter()
            .flat_map(|layout| &layout.model().workspaces)
            .flat_map(|space| &space.tabs)
            .flat_map(|tab| tab.tree.leaves())
            .filter_map(|leaf| leaf.attachment.as_ref()?.source_socket.clone())
            .collect();
        let work = cx
            .background_executor()
            .spawn(async move { SourceMatches::resolve(source, sockets) });
        self.source_match_updates = Some(cx.spawn(async move |this, cx| {
            let matches = work.await;
            let _ = this.update(cx, |browser, cx| {
                browser.source_matches = matches;
                browser.source_match_updates = None;
                cx.notify();
            });
        }));
    }

    /// Both sockets must have resolved successfully to the same path.
    pub(super) fn is_window_source(&self, socket: Option<&str>) -> bool {
        self.source_matches.contains(socket)
    }

    /// The state of the agent an attachment shows. Agent Backbone's agents
    /// run on this window's source server, so another server's session with
    /// the same name is not that agent.
    pub(super) fn attachment_state(
        &self,
        attachment: &agent_interface_core::Attachment,
        states: Option<&HashMap<String, AgentState>>,
    ) -> Option<AgentState> {
        let state = states?.get(&attachment.session).copied()?;
        self.is_window_source(attachment.source_socket.as_deref())
            .then_some(state)
    }

    /// Whether any pane of this tab shows an agent that needs you.
    pub(super) fn tab_needs_you(
        &self,
        tab: &agent_interface_core::Tab,
        states: Option<&HashMap<String, AgentState>>,
    ) -> bool {
        tab.tree.leaves().into_iter().any(|leaf| {
            leaf.attachment.as_ref().is_some_and(|attachment| {
                self.attachment_state(attachment, states)
                    .is_some_and(AgentState::needs_you)
            })
        })
    }

    /// The panel's width: the dragged width, else the design's default.
    pub(super) fn sidebar_width(&self, window: &Window) -> gpui::Pixels {
        let t = tokens();
        let width = settings::sidebar_width().map_or(t.size.panel_width, |width| width as f32);
        px(width)
            .min((window.viewport_size().width - px(t.size.panel_width)).max(px(t.size.panel_min)))
    }

    /// Leave room for window controls only while they are visible.
    pub(super) fn sidebar_top(&self, window: &Window) -> f32 {
        let t = tokens();
        if window.is_fullscreen() {
            t.size.titlebar / 2.0
        } else {
            t.size.titlebar - t.space.s4
        }
    }

    pub(super) fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let width = self.sidebar_width(window);
        let states = self.agent_states();
        div()
            .id("sidebar")
            .relative()
            .w(width)
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(c.panel)
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .font_family(t.font.ui.clone())
            .text_color(c.text)
            // The window controls sit here; the strip moves the window.
            .child(
                div()
                    .id("titlebar")
                    .h(px(self.sidebar_top(window)))
                    .flex_shrink_0()
                    .on_mouse_down(gpui::MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(self.workspace_block(cx))
            .child(self.jump_field(cx))
            .child(
                ui::section("TABS").child(ui::icon_button("new-tab", "plus").on_click(
                    cx.listener(|browser, _, window, cx| browser.create(Create::Tab, window, cx)),
                )),
            )
            .child(self.tab_list(states.as_ref(), cx))
            .children(self.agents_block(window, cx))
            .child(self.switcher(states.as_ref(), width, cx))
            .child(
                // The right edge resizes the panel.
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-t.space.s2))
                    .w(px(t.space.s4))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |browser, event: &gpui::MouseDownEvent, _, cx| {
                            browser.sidebar_drag = Some((event.position.x, width));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The current workspace: icon tile, name and tab count, and a chooser.
    fn workspace_block(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let space = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace());
        let name = space.map_or("Workspaces".to_owned(), |space| space.name.clone());
        let symbol = space
            .and_then(|space| icons::workspace(space.symbol.as_deref(), space.icon.as_deref()));
        let number = self.layout.as_ref().and_then(|layout| {
            let model = layout.model();
            model
                .workspaces
                .iter()
                .position(|space| space.id == model.selected)
        });
        let detail = space.map(|space| match space.tabs.len() {
            0 => "no tabs".to_owned(),
            1 => "1 tab".to_owned(),
            count => format!("{count} tabs"),
        });
        let editing = self.editing.as_ref().is_some_and(|edit| !edit.tab);
        let tile = div()
            .size(px(t.size.workspace_tile))
            .flex_shrink_0()
            .rounded(px(t.radius.slot))
            .bg(c.accent_soft)
            .flex()
            .items_center()
            .justify_center()
            .text_color(c.accent);
        let tile = match symbol {
            Some(symbol) => tile.child(ui::icon(symbol, t.size.icon_large, c.accent)),
            None => ui::text(tile, &t.text.row_selected)
                .child(number.map_or(String::new(), |n| (n + 1).to_string())),
        };
        div()
            .id("workspace-block")
            .mx(px(t.space.s8))
            .p(px(t.size.workspace_block_padding))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .rounded(px(t.radius.slot))
            .when(!editing, |el| {
                el.cursor_pointer().hover(|el| el.bg(colors().hover))
            })
            .child(tile)
            .child(if editing {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.name_editor(cx))
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        ui::text(
                            div().overflow_hidden().whitespace_nowrap().text_ellipsis(),
                            &t.text.workspace,
                        )
                        .child(name),
                    )
                    .children(detail.map(|detail| {
                        ui::text(div().text_color(c.muted).whitespace_nowrap(), &t.text.meta)
                            .child(detail)
                    }))
                    .into_any_element()
            })
            .when(!editing, |el| {
                el.child(ui::icon("chevrons-up-down", t.size.icon, c.muted))
            })
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(|browser, event: &gpui::MouseDownEvent, window, cx| {
                    browser.open_menu(Popover::Workspace, event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_click(
                cx.listener(|browser, event: &gpui::ClickEvent, window, cx| {
                    if browser.editing.is_some() {
                        return;
                    }
                    if event.click_count() == 2 {
                        browser.close_menus();
                        browser.begin_name(false, window, cx);
                    } else if browser.workspace_menu {
                        browser.close_menus();
                    } else {
                        browser.open_menu(Popover::Workspace, event.position(), window, cx);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The field that opens the jump popover.
    fn jump_field(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        ui::field_frame(self.jump.is_some())
            .id("jump-field")
            .mx(px(t.space.s12))
            .mt(px(t.space.s8))
            .mb(px(t.space.s6))
            .flex_shrink_0()
            .cursor_text()
            .child(ui::icon("search", t.size.icon, c.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child("Jump to tab, pane, agent"),
            )
            .child(ui::keycap(super::shortcuts::label(
                super::shortcuts::Command::Jump,
            )))
            .on_click(
                cx.listener(|browser, event: &gpui::ClickEvent, window, cx| {
                    browser.open_jump(event.position(), window, cx)
                }),
            )
            .into_any_element()
    }

    /// The current workspace's tabs.
    fn tab_list(
        &mut self,
        states: Option<&HashMap<String, AgentState>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let mut list = div()
            .id("tabs")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline))
            .px(px(t.space.s8))
            .overflow_y_scroll()
            .track_scroll(&self.tabs_scroll);
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
            .cloned()
        else {
            return list.into_any_element();
        };
        for (index, tab) in space.tabs.iter().enumerate() {
            let id = tab.id.clone();
            let selected = space.selected == id;
            let editing = selected && self.editing.as_ref().is_some_and(|edit| edit.tab);
            let group: SharedString = format!("tab-group-{id}").into();
            let mark = if selected { c.accent } else { c.faint };
            let name: AnyElement = if editing {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.name_editor(cx))
                    .into_any_element()
            } else {
                let tab_id = id.clone();
                ui::text(
                    div()
                        .id(SharedString::from(format!("tab-name-{id}")))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(if selected { c.text } else { c.text_2 }),
                    if selected {
                        &t.text.row_selected
                    } else {
                        &t.text.row
                    },
                )
                .child(tab.name.clone())
                .on_click(
                    cx.listener(move |browser, event: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        let active = browser.layout.as_ref().is_some_and(|layout| {
                            layout.model().tab().is_some_and(|tab| tab.id == tab_id)
                        });
                        if event.click_count() == 1 {
                            browser.name_click = Some((tab_id.clone(), active));
                        }
                        let edit = event.click_count() == 2
                            && browser
                                .name_click
                                .as_ref()
                                .is_some_and(|(id, active)| id == &tab_id && *active);
                        if let Some(layout) = &mut browser.layout {
                            layout.model_mut().choose_tab(&tab_id);
                        }
                        if edit {
                            browser.begin_name(true, window, cx);
                        } else if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        cx.notify();
                    }),
                )
                .into_any_element()
            };
            let options_id = id.clone();
            let dragged = TabDrag {
                browser: cx.entity_id(),
                workspace: space.id.clone(),
                tab: id.clone(),
                name: tab.name.clone(),
            };
            let destination = id.clone();
            list = list.child(
                ui::row(SharedString::from(format!("tab-{id}")), selected)
                    .group(group.clone())
                    .when(!editing && !self.saving && !self.save_failed, |el| {
                        el.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                            .drag_over::<TabDrag>(|style, _, _, _| style.bg(colors().accent_soft))
                            .on_drop(cx.listener(move |browser, dragged: &TabDrag, _, cx| {
                                browser.drop_tab(dragged, &destination, cx);
                            }))
                    })
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                        let id = id.clone();
                        move |browser, event: &gpui::MouseDownEvent, window, cx| {
                            browser.menu_at = Some(event.position);
                            browser.open_tab_menu(&id, window, cx)
                        }
                    }))
                    .child(
                        ui::text(
                            div()
                                .w(px(t.size.pane_number_width))
                                .flex_shrink_0()
                                .font_family(t.font.mono.clone())
                                .text_color(if selected { c.accent_text } else { c.muted }),
                            &t.text.key,
                        )
                        .child((index + 1).to_string()),
                    )
                    .child(name)
                    .when(self.tab_needs_you(tab, states), |el| el.child(ui::dot(c.attention)))
                    .when(!editing, |el| {
                        // The layout thumbnail gives way to the tab's options on hover.
                        el.child(
                            div()
                                .relative()
                                .flex_shrink_0()
                                .w(px(t.size.icon_button))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(
                                    ui::minimap(tab.tree.leaves().len(), mark)
                                        .group_hover(group.clone(), |style| style.invisible()),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .invisible()
                                        .group_hover(group.clone(), |style| style.visible())
                                        .child(
                                            ui::icon_button(SharedString::from(format!("tab-options-{id}")), "ellipsis")
                                                .on_click(cx.listener(move |browser, event: &gpui::ClickEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    browser.menu_at = Some(event.position());
                                                    browser.open_tab_menu(&options_id, window, cx);
                                                })),
                                        ),
                                ),
                        )
                    })
                    .on_click(cx.listener(move |browser, _, window, cx| {
                        if let Some(layout) = &mut browser.layout {
                            layout.model_mut().choose_tab(&id);
                        }
                        if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        cx.notify();
                    })),
            );
        }
        if space.tabs.is_empty() {
            list = list.child(
                ui::row("no-tabs", false)
                    .border(px(t.size.hairline))
                    .border_dashed()
                    .border_color(c.line_strong)
                    .text_color(c.muted)
                    .child(div().flex_1().child("No tabs yet"))
                    .child(ui::key_hint(super::shortcuts::label(
                        super::shortcuts::Command::NewTab,
                    )))
                    .on_click(cx.listener(|browser, _, window, cx| {
                        browser.create(Create::Tab, window, cx)
                    })),
            );
        }
        list.into_any_element()
    }

    /// Agents from Agent Backbone, pinned above the switcher.
    fn agents_block(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let show = self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.model().show_agents);
        if self.roster.is_none() || !show {
            return None;
        }
        let (t, c) = (tokens(), colors());
        let entries: Option<Vec<(String, AgentState)>> =
            self.agents.as_ref().and_then(|snapshot| {
                snapshot.active_entries().map(|entries| {
                    let mut entries: Vec<_> = entries
                        .into_iter()
                        .map(|(name, state)| (name.to_owned(), AgentState::from_report(state)))
                        .collect();
                    entries.sort_by_key(|(_, state)| !state.working());
                    entries
                })
            });
        let needing = entries.as_ref().map_or(0, |entries| {
            entries
                .iter()
                .filter(|(_, state)| state.needs_you())
                .count()
        });
        let working = entries.as_ref().map_or(0, |entries| {
            entries.iter().filter(|(_, state)| state.working()).count()
        });
        let (summary, summary_color) = match &entries {
            None if self.agents.is_some() => ("unavailable".to_owned(), c.attention_text),
            None => (String::new(), c.muted),
            Some(_) if needing > 0 => (format!("{needing} need you"), c.attention_text),
            Some(_) if working > 0 => (format!("{working} active"), c.accent_text),
            Some(entries)
                if entries
                    .iter()
                    .any(|(_, state)| *state == AgentState::Unknown) =>
            {
                ("status unknown".to_owned(), c.muted)
            }
            Some(_) => ("all quiet".to_owned(), c.muted),
        };
        let quiet = |label: &'static str, icon: Option<&'static str>| {
            ui::text(
                div()
                    .h(px(t.size.row))
                    .px(px(t.space.s10))
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .text_color(colors().muted),
                &tokens().text.body,
            )
            .children(icon.map(|name| ui::icon(name, tokens().size.icon, colors().muted)))
            .child(label)
        };
        let mut rows = div()
            .id("agents")
            .flex()
            .flex_col()
            .gap(px(t.size.hairline))
            .px(px(t.space.s8))
            .max_h(
                (window.viewport_size().height * 0.4)
                    .min(px(t.size.row * 9.0 + t.size.hairline * 8.0)),
            )
            .overflow_y_scroll();
        match &entries {
            None if self.agents.is_some() => {
                rows = rows.child(quiet("Agent Backbone unavailable", Some("unplug")))
            }
            None => rows = rows.child(quiet("Loading agents…", None)),
            Some(entries) if entries.is_empty() => {
                rows = rows.child(quiet("No active agents", None))
            }
            Some(entries) => {
                for (name, state) in entries {
                    let agent = name.clone();
                    let calm = matches!(state, AgentState::Idle | AgentState::Unknown);
                    rows = rows.child(
                        ui::row(SharedString::from(format!("agent-{name}")), false)
                            .child(ui::agent_symbol(*state))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(if calm { c.muted } else { c.text })
                                    .child(name.clone()),
                            )
                            .on_click(cx.listener(move |browser, _, window, cx| {
                                browser.go_to_agent(&agent, window, cx)
                            })),
                    );
                }
            }
        }
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .border_t(px(t.size.hairline))
                .border_color(c.line)
                .pt(px(t.space.s4))
                .pb(px(t.space.s8))
                .child(
                    ui::section("AGENTS").pr(px(t.space.s14)).child(
                        ui::text(div().text_color(summary_color), &t.text.meta).child(summary),
                    ),
                )
                .child(rows)
                .into_any_element(),
        )
    }

    /// One-click workspace switching, then Settings.
    fn switcher(
        &mut self,
        states: Option<&HashMap<String, AgentState>>,
        width: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let row = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s4))
            .h(px(t.size.slot_row))
            .px(px(t.space.s10))
            .border_t(px(t.size.hairline))
            .border_color(c.line);
        let slot = |id: SharedString| {
            div()
                .id(id)
                .relative()
                .flex_shrink_0()
                .size(px(tokens().size.slot))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(tokens().radius.slot))
                .cursor_pointer()
        };
        let mut slots = div()
            .id("workspace-slots")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(t.space.s4))
            .overflow_x_scroll()
            .track_scroll(&self.workspaces_scroll);
        if let Some(layout) = &self.layout {
            let model = layout.model();
            let selection = (model.selected.clone(), width);
            if self.shown_workspace.as_ref() != Some(&selection) {
                if let Some(index) = model
                    .workspaces
                    .iter()
                    .position(|space| space.id == model.selected)
                {
                    self.workspaces_scroll.scroll_to_item(index);
                }
                self.shown_workspace = Some(selection);
            }
            for (index, space) in model.workspaces.iter().enumerate() {
                let id = space.id.clone();
                let current = model.selected == id;
                let needs_you = space.tabs.iter().any(|tab| self.tab_needs_you(tab, states));
                let glyph = icons::workspace(space.symbol.as_deref(), space.icon.as_deref());
                let color = if current { c.accent } else { c.muted };
                let mut element = slot(SharedString::from(format!("workspace-slot-{id}")))
                    .when(current, |el| el.bg(c.accent_soft))
                    .when(!current, |el| el.hover(|el| el.bg(colors().hover)));
                element = match glyph {
                    Some(name) => element.child(ui::icon(name, t.size.icon_large, color)),
                    None => ui::text(element.text_color(color), &t.text.row_selected)
                        .child((index + 1).to_string()),
                };
                slots = slots.child(
                    element
                        .when(needs_you, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(px(t.space.s4))
                                    .right(px(t.space.s4))
                                    .child(
                                        ui::dot(c.attention)
                                            .border(px(t.space.s2))
                                            .border_color(c.panel),
                                    ),
                            )
                        })
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener({
                                let id = id.clone();
                                move |browser, event: &gpui::MouseDownEvent, window, cx| {
                                    if let Some(layout) = &mut browser.layout {
                                        layout.model_mut().choose_workspace(&id);
                                    }
                                    browser.open_menu(
                                        Popover::Workspace,
                                        event.position,
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                            }),
                        )
                        .on_click(cx.listener(move |browser, _, window, cx| {
                            if let Some(layout) = &mut browser.layout {
                                layout.model_mut().choose_workspace(&id);
                            }
                            browser.close_menus();
                            if let Some(focus) = &browser.navigation {
                                window.focus(focus, cx);
                            }
                            cx.notify();
                        })),
                );
            }
        }
        row.child(slots)
            .child(
                slot("open-settings".into())
                    .hover(|el| el.bg(colors().hover))
                    .child(ui::icon("sliders-horizontal", t.size.icon_large, c.muted))
                    .on_click(cx.listener(|browser, _, _, cx| {
                        browser.close_menus();
                        cx.defer(super::settings_window::show);
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceMatches, reorder_tab_proposal};

    #[test]
    fn dragging_tabs_reorders_in_both_directions_without_changing_selection() {
        let directory = tempfile::tempdir().unwrap();
        drop(agent_interface_store::Store::open_or_create(directory.path()).unwrap());
        let mut store =
            agent_interface_store::Store::open_existing_writable(directory.path()).unwrap();
        let mut base = store.load().unwrap();
        let mut model = base.model().clone();
        for name in ["First", "Second", "Third"] {
            model.add_tab(Some(name.into()), "/work".into(), true);
        }
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        let layout = crate::library::WindowLayout::new(store.load().unwrap());
        let space = model.workspace().unwrap();
        let (first, second, third) = (&space.tabs[0].id, &space.tabs[1].id, &space.tabs[2].id);
        let (mut base, down) = reorder_tab_proposal(&layout, &space.id, first, third).unwrap();
        assert_eq!(
            down.workspace()
                .unwrap()
                .tabs
                .iter()
                .map(|tab| &tab.id)
                .collect::<Vec<_>>(),
            [second, third, first]
        );
        assert_eq!(down.workspace().unwrap().selected, space.selected);
        assert!(!store.save(&mut base, &down).unwrap().conflict);
        let layout = crate::library::WindowLayout::new(store.load().unwrap());
        let (_, up) = reorder_tab_proposal(&layout, &space.id, first, second).unwrap();
        assert_eq!(up.workspace().unwrap().tabs, space.tabs);
        assert_eq!(up.workspace().unwrap().selected, space.selected);
        assert!(reorder_tab_proposal(&layout, &space.id, "removed", second).is_none());
        assert!(reorder_tab_proposal(&layout, "other-workspace", first, third).is_none());
        assert!(reorder_tab_proposal(&layout, &space.id, first, "removed").is_none());
    }

    #[test]
    fn socket_matches_cache_aliases_and_never_match_failed_resolutions() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("source");
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&socket, &alias).unwrap();
        let alias = alias.to_str().unwrap().to_owned();
        let invalid = "invalid\0socket".to_owned();
        let paths = [alias.clone(), invalid.clone(), "/other/socket".into()].into();
        let matches = SourceMatches::resolve(Some(socket), paths);
        // Rendering needs only the snapshot, even if the alias is gone later.
        std::fs::remove_file(&alias).unwrap();
        assert!(matches.contains(None));
        assert!(matches.contains(Some(&alias)));
        assert!(!matches.contains(Some(&invalid)));
        assert!(!matches.contains(Some("/other/socket")));
        let failed = SourceMatches::resolve(Some(invalid.clone().into()), [invalid.clone()].into());
        assert!(!failed.contains(None));
        assert!(!failed.contains(Some(&invalid)));
    }
}
