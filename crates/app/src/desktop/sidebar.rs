//! The navigation panel: workspace block, jump field, tabs, agents and the
//! workspace switcher. Layout and colours come from the design tokens.

use std::collections::HashMap;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};

use super::{
    Browser, Popover, components as ui,
    design::{colors, tokens},
    icons, settings,
};
use crate::desktop::layout_edit::Create;
use ui::AgentState;

impl Browser {
    /// Each agent's state, or none while Agent Backbone is unreachable.
    pub(super) fn agent_states(&self) -> Option<HashMap<String, AgentState>> {
        let snapshot = self.agents.as_ref()?;
        Some(
            snapshot
                .active_entries()?
                .into_iter()
                .map(|(name, state)| (name.to_owned(), AgentState::from_report(state)))
                .collect(),
        )
    }

    /// Whether any pane of this tab shows an agent that needs you.
    pub(super) fn tab_needs_you(
        tab: &tmux_workspaces_core::Tab,
        states: Option<&HashMap<String, AgentState>>,
    ) -> bool {
        states.is_some_and(|states| {
            tab.tree.leaves().into_iter().any(|leaf| {
                leaf.attachment.as_ref().is_some_and(|attachment| {
                    states.get(&attachment.session) == Some(&AgentState::NeedsYou)
                })
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
                    .h(px(t.size.titlebar))
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
            .children(self.agents_block(states.as_ref(), window, cx))
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
            list = list.child(
                ui::row(SharedString::from(format!("tab-{id}")), selected)
                    .group(group.clone())
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
                    .when(Self::tab_needs_you(tab, states), |el| el.child(ui::dot(c.attention)))
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
    fn agents_block(
        &mut self,
        states: Option<&HashMap<String, AgentState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
                    entries
                        .into_iter()
                        .map(|(name, state)| (name.to_owned(), AgentState::from_report(state)))
                        .collect()
                })
            });
        let needing = entries.as_ref().map_or(0, |entries| {
            entries
                .iter()
                .filter(|(_, state)| *state == AgentState::NeedsYou)
                .count()
        });
        let (summary, summary_color) = match &entries {
            None if self.agents.is_some() => ("unavailable".to_owned(), c.attention_text),
            None => (String::new(), c.muted),
            Some(_) if needing > 0 => (format!("{needing} need you"), c.attention_text),
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
            .max_h(window.viewport_size().height * 0.4)
            .overflow_y_scroll();
        // Where each agent's session is shown, for the row's meta.
        let mut tabs_by_session: HashMap<String, String> = HashMap::new();
        if let Some(layout) = &self.layout {
            for space in &layout.model().workspaces {
                for tab in &space.tabs {
                    for leaf in tab.tree.leaves() {
                        if let Some(attachment) = &leaf.attachment {
                            tabs_by_session
                                .entry(attachment.session.clone())
                                .or_insert_with(|| tab.name.clone());
                        }
                    }
                }
            }
        }
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
                            .child(ui::agent_symbol(
                                *state,
                                SharedString::from(format!("agent-spin-{name}")),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(if calm { c.muted } else { c.text })
                                    .child(name.clone()),
                            )
                            .children(tabs_by_session.get(name).map(|tab| {
                                ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.meta)
                                    .child(tab.clone())
                            }))
                            .on_click(cx.listener(move |browser, _, window, cx| {
                                browser.go_to_agent(&agent, window, cx)
                            })),
                    );
                }
            }
        }
        let _ = states;
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
        let mut row = div()
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
        if let Some(layout) = &self.layout {
            let model = layout.model();
            let room = (f32::from(width) - t.space.s20 - t.size.slot) / (t.size.slot + t.space.s4);
            let capacity = room.floor().max(1.0) as usize;
            let overflow = model.workspaces.len() > capacity;
            let shown = if overflow { capacity - 1 } else { capacity };
            for (index, space) in model.workspaces.iter().take(shown).enumerate() {
                let id = space.id.clone();
                let current = model.selected == id;
                let needs_you = space
                    .tabs
                    .iter()
                    .any(|tab| Self::tab_needs_you(tab, states));
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
                row = row.child(
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
            if overflow {
                row = row.child(
                    ui::text(
                        slot("workspace-more".into()).text_color(c.muted),
                        &t.text.row_selected,
                    )
                    .hover(|el| el.bg(colors().hover))
                    .child("⋯")
                    .on_click(cx.listener(
                        |browser, event: &gpui::ClickEvent, window, cx| {
                            browser.open_menu(Popover::Workspace, event.position(), window, cx);
                        },
                    )),
                );
            }
        }
        row.child(div().flex_1())
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
