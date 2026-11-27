//! The navigation panel: workspace block, jump field, tabs, agents and the
//! workspace switcher. Layout and colours come from the design tokens.

use std::collections::HashMap;

use gpui::{AnyElement, Context, EntityId, SharedString, Window, div, prelude::*, px};

use super::{
    Browser, agent_groups, components as ui,
    design::{colors, tokens},
    icons, settings,
};
use crate::desktop::layout_edit::Create;
use ui::AgentState;

#[derive(Clone)]
struct TabDrag {
    origin: Option<super::navigation_origin::Origin>,
    browser: EntityId,
    workspace: String,
    tab: String,
    name: String,
}

impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_preview("dragged-tab", self.name.clone())
    }
}

#[derive(Clone)]
struct WorkspaceDrag {
    browser: EntityId,
    workspace: String,
    name: String,
}

impl Render for WorkspaceDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A slot is too small to see a line through the copy: the copy rides
        // a switcher row higher, clear of the strip, so the line at the slot
        // under the pointer shows at full strength. GPUI lays the drag view
        // out as a root, which ignores its own offset, so the copy is a child.
        div().child(
            drag_preview("dragged-workspace", self.name.clone())
                .relative()
                .top(px(-tokens().size.slot_row)),
        )
    }
}

/// What follows the pointer: see-through, so the line under it shows where
/// the item will land.
fn drag_preview(id: &'static str, name: String) -> gpui::Stateful<gpui::Div> {
    ui::row(id, true)
        .bg(colors().panel)
        .text_color(colors().text)
        .opacity(colors().pane_dim)
        .child(name)
}

/// A dropped tab or workspace: where it goes, by id, so a drop made while a
/// save runs can be applied to the layout that save leaves.
pub(super) enum Reorder {
    Tab {
        origin: Option<super::navigation_origin::Origin>,
        workspace: String,
        tab: String,
        destination: String,
        after: bool,
    },
    Workspace {
        source: String,
        destination: String,
        after: bool,
    },
}

/// The line that shows where a dragged item will land: along the top or
/// bottom edge of a row, or the left or right edge of a workspace slot.
fn landing_line(vertical: bool, after: bool) -> gpui::Div {
    let width = px(tokens().size.focus_ring);
    let line = div().absolute().bg(colors().focus);
    match (vertical, after) {
        (false, false) => line.left_0().right_0().top_0().h(width),
        (false, true) => line.left_0().right_0().bottom_0().h(width),
        (true, false) => line.top_0().bottom_0().left_0().w(width),
        (true, true) => line.top_0().bottom_0().right_0().w(width),
    }
}

/// Begin a drag: the copy that follows the pointer, and no landing left
/// over from a drag released away from every row or slot. Until the next
/// press, the press is a drag, not a click (see `dragged_since_press`).
fn start_drag<T: Clone + Render>(
    cx: &Context<Browser>,
) -> impl Fn(&T, gpui::Point<gpui::Pixels>, &mut Window, &mut gpui::App) -> gpui::Entity<T> + 'static
{
    let browser = cx.entity().downgrade();
    move |dragged, _, _, cx| {
        let _ = browser.update(cx, |browser, _| {
            browser.reorder_target = None;
            browser.dragged_since_press = true;
        });
        cx.new(|_| dragged.clone())
    }
}

fn reorder_tab_proposal(
    layout: &crate::library::WindowLayout,
    workspace: &str,
    source: &str,
    destination: &str,
    after: bool,
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
    space.tabs.iter().position(|tab| tab.id == destination)?;
    let tab = space.tabs.remove(from);
    let to = space.tabs.iter().position(|tab| tab.id == destination)? + usize::from(after);
    space.tabs.insert(to, tab);
    (to != from).then_some((base, proposed))
}

fn reorder_workspace_proposal(
    layout: &crate::library::WindowLayout,
    source: &str,
    destination: &str,
    after: bool,
) -> Option<(
    agent_interface_store::Document,
    agent_interface_core::Layout,
)> {
    let (base, mut proposed) = layout.save_snapshot();
    proposed
        .move_workspace(source, destination, after)
        .then_some((base, proposed))
}

fn section_toggle(label: &'static str, collapsed: bool) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(label)
            .role(gpui::accesskit::Role::Button)
            .aria_label(if collapsed {
                format!("Expand {label}")
            } else {
                format!("Collapse {label}")
            })
            .focusable()
            .tab_stop(true)
            .focus_visible(|el| {
                el.border(px(t.size.focus_ring))
                    .border_color(colors().focus)
            })
            .h(px(t.space.s28))
            .px(px(t.space.s10))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s4))
            .cursor_pointer()
            .text_color(c.muted)
            .hover(|el| el.bg(colors().hover)),
        &t.text.label,
    )
    .child(ui::icon(
        if collapsed {
            "chevron-right"
        } else {
            "chevron-down"
        },
        t.size.icon,
        c.muted,
    ))
    .child(div().flex_1().child(label))
}

/// Whether a tab row can be dragged. A save that is running does not stop
/// it: the drop waits for that save. A failed save still waiting for Retry
/// does, as it stops every other edit, and so does editing the tab's name.
fn tab_row_draggable(editing: bool, save_failed: bool) -> bool {
    !editing && !save_failed
}

/// Whether a click inside a tab row ends a drag rather than activating
/// what it is on. Only a mouse release can end the press that started a
/// drag; Enter, Space and taps always activate.
fn click_ends_drag(dragged_since_press: bool, click: &gpui::ClickEvent) -> bool {
    dragged_since_press && matches!(click, gpui::ClickEvent::Mouse(_))
}

/// Where a drop on `bounds` at `pointer` lands: after it past its middle,
/// across for a workspace slot (`vertical` line) and down for a tab row.
/// None outside the part of it its list shows (`visible`), where a drop
/// cannot land.
fn landing_at(
    bounds: gpui::Bounds<gpui::Pixels>,
    visible: gpui::Bounds<gpui::Pixels>,
    pointer: gpui::Point<gpui::Pixels>,
    vertical: bool,
) -> Option<bool> {
    (bounds.contains(&pointer) && visible.contains(&pointer)).then(|| {
        if vertical {
            pointer.x > bounds.center().x
        } else {
            pointer.y > bounds.center().y
        }
    })
}

impl Browser {
    /// Apply a drop now, or once the save that is running finishes.
    pub(super) fn drop_reorder(&mut self, reorder: Reorder, cx: &mut Context<Self>) {
        let Some(proposal) = self.reorder_proposal(reorder) else {
            return;
        };
        self.pending_save = Some(proposal);
        self.name_click = None;
        self.close_menus();
        self.save(cx);
    }

    /// The layout a drop saves, or none: a drop made while a save runs is
    /// kept for when it finishes; after a failed save, a moved or missing
    /// item or a changed workspace connection, nothing happens.
    fn reorder_proposal(
        &mut self,
        reorder: Reorder,
    ) -> Option<(
        agent_interface_store::Document,
        agent_interface_core::Layout,
    )> {
        self.reorder_target = None;
        if self.save_failed {
            return None;
        }
        if self.saving {
            self.queued_reorder = Some(reorder);
            return None;
        }
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        let layout = self.layout.as_ref()?;
        match &reorder {
            Reorder::Tab {
                origin,
                workspace,
                tab,
                destination,
                after,
            } => self
                .selected_origin_is_current(origin.as_ref())
                .then(|| reorder_tab_proposal(layout, workspace, tab, destination, *after))
                .flatten(),
            Reorder::Workspace {
                source,
                destination,
                after,
            } => reorder_workspace_proposal(layout, source, destination, *after),
        }
    }

    /// When a save finishes, the drop made during it: applied after a save
    /// that was stored, discarded after one that failed (`None`) or
    /// conflicted, so its notice that the edit was not saved stays shown.
    pub(super) fn finish_queued_reorder(
        &mut self,
        saved: Option<&agent_interface_store::Reconciliation>,
    ) -> Option<Reorder> {
        self.queued_reorder
            .take()
            .filter(|_| saved.is_some_and(|result| !result.conflict))
    }

    /// Where a drag over `target` would land: before it, or after it when the
    /// pointer is past its middle. Shown only while the pointer is over it.
    fn track_landing(
        &mut self,
        target: &str,
        bounds: gpui::Bounds<gpui::Pixels>,
        position: gpui::Point<gpui::Pixels>,
        vertical: bool,
        cx: &mut Context<Self>,
    ) {
        let visible = if vertical {
            self.workspaces_scroll.bounds()
        } else {
            self.tabs_scroll.bounds()
        };
        let landing = if let Some(after) = landing_at(bounds, visible, position, vertical) {
            Some((target.to_owned(), after))
        } else if self
            .reorder_target
            .as_ref()
            .is_some_and(|(shown, _)| shown == target)
        {
            None
        } else {
            return;
        };
        if self.reorder_target != landing {
            self.reorder_target = landing;
            cx.notify();
        }
    }

    /// Whether a dragged tab can still land among the tabs shown: they are
    /// its workspace's, on the same connection, and the tab is still there.
    /// After switching workspace mid-drag, no line promises a drop that
    /// `reorder_proposal` would refuse.
    fn tab_drag_lands_here(
        &self,
        origin: Option<&super::navigation_origin::Origin>,
        workspace: &str,
        tab: &str,
    ) -> bool {
        self.selected_origin_is_current(origin)
            && self
                .layout
                .as_ref()
                .and_then(|layout| layout.model().workspace())
                .is_some_and(|space| {
                    space.id == workspace && space.tabs.iter().any(|shown| shown.id == tab)
                })
    }

    /// Whether a dragged workspace is still in the library.
    fn workspace_exists(&self, workspace: &str) -> bool {
        self.layout.as_ref().is_some_and(|layout| {
            layout
                .model()
                .workspaces
                .iter()
                .any(|space| space.id == workspace)
        })
    }

    /// Whether the drop on `target` lands after it. GPUI ends the drag
    /// before it calls a drop listener, so this reads the last landing shown.
    fn dropped_after(&self, target: &str) -> bool {
        self.reorder_target
            .as_ref()
            .is_some_and(|(shown, after)| shown == target && *after)
    }

    /// The landing line for `target`, while a drag is over it.
    fn landing(&self, target: &str, cx: &Context<Self>) -> Option<bool> {
        self.reorder_target
            .as_ref()
            .filter(|(shown, _)| shown == target && cx.has_active_drag())
            .map(|(_, after)| *after)
    }

    /// Each agent's state, or none in a workspace without a profile or while
    /// its profile's roster is unavailable.
    pub(super) fn agent_states(&self) -> Option<std::rc::Rc<HashMap<String, AgentState>>> {
        self.active_agents()?.active()
    }

    /// Whether `socket` is the selected workspace's profile's tmux server,
    /// where its agents run. A workspace without a profile has none.
    pub(super) fn is_profile_server(&self, socket: Option<&str>) -> bool {
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
        else {
            return false;
        };
        match self.workspace_profiles.route(space) {
            super::workspace_profiles::Route::Bound(connection) => {
                socket.is_none_or(|path| std::path::Path::new(path) == connection.tmux_socket())
            }
            _ => false,
        }
    }

    /// The state of the agent an attachment shows. A profile's agents run on
    /// its tmux server, so a same-name session on another server is not that
    /// agent.
    pub(super) fn attachment_state(
        &self,
        attachment: &agent_interface_core::Attachment,
        states: Option<&HashMap<String, AgentState>>,
    ) -> Option<AgentState> {
        let state = states?.get(&attachment.session).copied()?;
        self.is_profile_server(attachment.source_socket.as_deref())
            .then_some(state)
    }

    pub(super) fn attachment_state_in(
        &self,
        space: &agent_interface_core::Workspace,
        attachment: &agent_interface_core::Attachment,
    ) -> Option<AgentState> {
        let states = self.workspace_agents(space)?.active()?;
        let matches = match self.workspace_profiles.route(space) {
            super::workspace_profiles::Route::Bound(connection) => attachment
                .source_socket
                .as_deref()
                .is_none_or(|path| std::path::Path::new(path) == connection.tmux_socket()),
            super::workspace_profiles::Route::Standalone
            | super::workspace_profiles::Route::Unavailable => false,
        };
        matches
            .then(|| states.get(&attachment.session).copied())
            .flatten()
    }

    pub(super) fn tab_needs_you(&self, tab: &agent_interface_core::Tab) -> bool {
        let Some(space) = self.layout.as_ref().and_then(|layout| {
            layout
                .model()
                .workspaces
                .iter()
                .find(|space| space.tabs.iter().any(|candidate| candidate.id == tab.id))
        }) else {
            return false;
        };
        tab.tree.leaves().iter().any(|leaf| {
            leaf.attachment
                .as_ref()
                .and_then(|attachment| self.attachment_state_in(space, attachment))
                .is_some_and(AgentState::needs_you)
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
        let creation = self.creation_target();
        let (t, c) = (tokens(), colors());
        let width = self.sidebar_width(window);
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
                    cx.listener(move |browser, _, window, cx| {
                        browser.create_from(Create::Tab, creation.as_ref(), window, cx)
                    }),
                )),
            )
            .child(self.tab_list(cx))
            .children(self.agents_block(window, cx))
            .children(self.connections_block(cx))
            .child(self.switcher(width, cx))
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
                            browser.sidebar_drag =
                                Some((event.position.x, width, settings::sidebar_width()));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The current workspace: icon tile, name and tab count, and a chooser.
    fn workspace_block(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let origin = self.selected_navigation_origin();
        let menu_origin = origin.clone();
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
                cx.listener(move |browser, event: &gpui::MouseDownEvent, window, cx| {
                    if !browser.selected_origin_is_current(menu_origin.as_ref()) {
                        return;
                    }
                    browser.open_workspace_menu(event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_click(
                cx.listener(move |browser, event: &gpui::ClickEvent, window, cx| {
                    if !browser.selected_origin_is_current(origin.as_ref()) {
                        return;
                    }
                    if browser.editing.is_some() {
                        return;
                    }
                    if event.click_count() == 2 {
                        browser.close_menus();
                        browser.begin_name_from(false, origin.as_ref(), window, cx);
                    } else if browser.workspace_menu {
                        browser.close_menus();
                    } else {
                        browser.open_workspace_menu(event.position(), window, cx);
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
    fn tab_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let creation = self.creation_target();
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
        let origin = self.navigation_origin(&space.id);
        for (index, tab) in space.tabs.iter().enumerate() {
            let origin = origin.clone();
            let options_origin = origin.clone();
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
                let origin = origin.clone();
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
                        // The row owns the drag, so GPUI holds back only the
                        // row's click after one, not this name's.
                        if click_ends_drag(browser.dragged_since_press, event) {
                            return;
                        }
                        let active = browser.layout.as_ref().is_some_and(|layout| {
                            layout.model().tab().is_some_and(|tab| tab.id == tab_id)
                        });
                        if !origin
                            .as_ref()
                            .is_some_and(|origin| browser.select_navigation(origin, &tab_id, None))
                        {
                            return;
                        }
                        if event.click_count() == 1 {
                            browser.name_click = Some((tab_id.clone(), active));
                        }
                        let edit = event.click_count() == 2
                            && browser
                                .name_click
                                .as_ref()
                                .is_some_and(|(id, active)| id == &tab_id && *active);
                        if edit {
                            browser.begin_name_from(true, origin.as_ref(), window, cx);
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
                origin: origin.clone(),
                browser: cx.entity_id(),
                workspace: space.id.clone(),
                tab: id.clone(),
                name: tab.name.clone(),
            };
            let destination = id.clone();
            let target = id.clone();
            let landing = self.landing(&id, cx);
            list = list.child(
                ui::row(SharedString::from(format!("tab-{id}")), selected)
                    .relative()
                    .group(group.clone())
                    .when(tab_row_draggable(editing, self.save_failed), |el| {
                        el.on_drag(dragged, start_drag(cx))
                            .on_drag_move(cx.listener(
                                move |browser, event: &gpui::DragMoveEvent<TabDrag>, _, cx| {
                                    let drag = event.drag(cx);
                                    if drag.browser == cx.entity_id()
                                        && browser.tab_drag_lands_here(
                                            drag.origin.as_ref(),
                                            &drag.workspace,
                                            &drag.tab,
                                        )
                                    {
                                        let position = event.event.position;
                                        browser.track_landing(&target, event.bounds, position, false, cx);
                                    }
                                },
                            ))
                            .on_drop(cx.listener(move |browser, dragged: &TabDrag, _, cx| {
                                if dragged.browser != cx.entity_id() {
                                    return;
                                }
                                let after = browser.dropped_after(&destination);
                                browser.drop_reorder(
                                    Reorder::Tab {
                                        origin: dragged.origin.clone(),
                                        workspace: dragged.workspace.clone(),
                                        tab: dragged.tab.clone(),
                                        destination: destination.clone(),
                                        after,
                                    },
                                    cx,
                                );
                            }))
                    })
                    .children(landing.map(|after| landing_line(false, after)))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                        let id = id.clone();
                        let origin = origin.clone();
                        move |browser, event: &gpui::MouseDownEvent, window, cx| {
                            browser.open_tab_menu(origin.as_ref(), &id, event.position, window, cx)
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
                    .when(self.tab_needs_you(tab), |el| el.child(ui::dot(c.attention)))
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
                                                    if click_ends_drag(browser.dragged_since_press, event) {
                                                        return;
                                                    }
                                                    browser.open_tab_menu(options_origin.as_ref(), &options_id, event.position(), window, cx);
                                                })),
                                        ),
                                ),
                        )
                    })
                    .on_click(cx.listener(move |browser, _, window, cx| {
                        if !origin.as_ref().is_some_and(|origin| browser.select_navigation(origin, &id, None)) {
                            return;
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
                    .on_click(cx.listener(move |browser, _, window, cx| {
                        browser.create_from(Create::Tab, creation.as_ref(), window, cx)
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
        if self.active_agents().is_none() || !show {
            return None;
        }
        let (t, c) = (tokens(), colors());
        let entries = self.agent_states().map(|states| {
            let mut entries: Vec<_> = states
                .iter()
                .map(|(name, state)| (name.clone(), *state))
                .collect();
            if let Some(agents) = self.active_agents() {
                // An offline agent on another machine is an ordinary row.
                let local_machine = agents.local_machine_id();
                let (machines, machines_read) = agents.machines();
                for (name, machine) in agents.offline_with_machines() {
                    if entries.iter().all(|(listed, _)| *listed != name)
                        && agent_groups::machine_tag(
                            local_machine.as_deref(),
                            &machines,
                            machines_read,
                            machine.as_ref(),
                        )
                        .is_some()
                    {
                        entries.push((name, AgentState::Offline));
                    }
                }
                // An offline agent whose reviews go on keeps a row to carry
                // them; other runs have nothing to show under it.
                for name in agents.offline_with_reviews() {
                    if entries.iter().all(|(listed, _)| *listed != name) {
                        entries.push((name, AgentState::Offline));
                    }
                }
                // Last, so a swarm whose listed members are only the agents
                // added above still keeps the agent that started it.
                let leads = agent_groups::offline_leads(&entries, &agents.swarms(), |name| {
                    agents.get(name).state
                });
                entries.extend(leads.into_iter().map(|name| (name, AgentState::Offline)));
            }
            entries.sort_by_cached_key(|(name, state)| {
                (
                    !state.needs_you(),
                    !state.working(),
                    name.to_ascii_lowercase(),
                    name.clone(),
                )
            });
            entries
        });
        let observed = self.active_agents().is_some_and(|roster| roster.observed());
        let not_answering = self
            .active_agents()
            .is_some_and(|roster| roster.not_answering());
        let is_stale = |name: &str| {
            self.active_agents()
                .is_some_and(|agents| agents.get(name).stale)
        };
        let needing = entries.as_ref().map_or(0, |entries| {
            entries
                .iter()
                .filter(|(name, state)| !is_stale(name) && state.needs_you())
                .count()
        });
        let working = entries.as_ref().map_or(0, |entries| {
            entries
                .iter()
                .filter(|(name, state)| !is_stale(name) && state.working())
                .count()
        });
        let stale_count = entries.as_ref().map_or(0, |entries| {
            entries.iter().filter(|(name, _)| is_stale(name)).count()
        });
        let (summary, summary_color) = match &entries {
            None if observed && not_answering => ("not answering".to_owned(), c.attention_text),
            None if observed => ("unavailable".to_owned(), c.attention_text),
            None => (String::new(), c.muted),
            Some(_) if needing > 0 => (format!("{needing} need you"), c.attention_text),
            Some(_) if working > 0 => (format!("{working} active"), c.accent_text),
            Some(_) if stale_count > 0 => (format!("{stale_count} stale"), c.attention_text),
            Some(entries)
                if entries
                    .iter()
                    .any(|(_, state)| *state == AgentState::Unknown) =>
            {
                ("status unknown".to_owned(), c.muted)
            }
            // Quiet agents can still have work running in the background.
            Some(_) => (
                self.active_agents()
                    .and_then(|agents| agents.background())
                    .map_or_else(|| "all quiet".to_owned(), |runs| runs.to_string()),
                c.muted,
            ),
        };
        let heading = section_toggle("AGENTS", self.agents_collapsed)
            .child(ui::text(div().text_color(summary_color), &t.text.meta).child(summary))
            .on_click(cx.listener(|browser, _, _, cx| {
                browser.agents_collapsed = !browser.agents_collapsed;
                cx.notify();
            }));
        if self.agents_collapsed {
            return Some(heading.into_any_element());
        }
        let quiet = |label: SharedString, icon: Option<&'static str>| {
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
            None if observed && not_answering => {
                let read = self.active_agents().and_then(|agents| agents.observed_at());
                rows = rows.child(quiet(
                    super::agent_status::not_answering(read).into(),
                    Some("unplug"),
                ))
            }
            None if observed => {
                rows = rows.child(quiet("Agent Backbone unavailable".into(), Some("unplug")))
            }
            None => rows = rows.child(quiet("Loading agents…".into(), None)),
            Some(entries) => {
                let swarms = self
                    .active_agents()
                    .map(|agents| agents.swarms())
                    .unwrap_or_default();
                let local_machine = self
                    .active_agents()
                    .and_then(|agents| agents.local_machine_id());
                let (machines, machines_read) = self
                    .active_agents()
                    .map(|agents| agents.machines())
                    .unwrap_or_default();
                // Each local agent's browser, a session named as it is.
                let browsers = crate::agent_browser::by_agent(
                    &self.agent_browsers,
                    entries
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .filter(|name| {
                            self.agent_target(name)
                                .is_some_and(|target| target.remote_profile.is_none())
                        }),
                );
                let tree = agent_groups::tree(
                    entries,
                    &swarms,
                    |name| agent_groups::Started {
                        reviews: self
                            .active_agents()
                            .and_then(|agents| agents.activity(name))
                            .map(|activity| activity.reviews.clone())
                            .unwrap_or_default(),
                        browser: browsers
                            .get(name)
                            .map(|session| self.agent_browser_host(session, cx)),
                    },
                    |name| {
                        let machine = self.active_agents()?.machine(name);
                        agent_groups::machine_tag(
                            local_machine.as_deref(),
                            &machines,
                            machines_read,
                            machine.as_ref(),
                        )
                    },
                    &self.expanded_agents,
                    &self.collapsed_swarms,
                );
                if tree.is_empty() {
                    rows = rows.child(quiet("No active agents".into(), None));
                } else {
                    let creation = self.creation_target();
                    // An agent row; `swarm` marks a member under its swarm,
                    // shown by `label`.
                    #[allow(clippy::too_many_arguments)]
                    let agent = |browser: &Self,
                                 cx: &mut Context<Self>,
                                 name: &str,
                                 label: &str,
                                 state: AgentState,
                                 swarm: Option<&str>,
                                 machine: Option<agent_groups::MachineHeader>,
                                 counts: agent_groups::Counts,
                                 chevron: Option<AnyElement>| {
                        let target = browser.agent_target(name);
                        let status = browser.active_agents().map(|agents| agents.get(name));
                        let stale = status.as_ref().is_some_and(|status| status.stale);
                        let calm = matches!(
                            state,
                            AgentState::Idle | AgentState::Unknown | AgentState::Offline
                        ) || stale;
                        let id = match swarm {
                            // Names never hold a slash, so these cannot collide.
                            Some(swarm) => format!("agent-{swarm}/{name}"),
                            None => format!("agent-{name}"),
                        };
                        let group: SharedString = format!("agent-group-{id}").into();
                        let menu_name = name.to_owned();
                        let options_name = name.to_owned();
                        let click_name = name.to_owned();
                        // Why its deliveries are held, as the row's tooltip.
                        let held = status.as_ref().and_then(|status| status.held);
                        let tag = machine.map(|machine| {
                            let offline = machine.presence.attention();
                            // The full "last seen" text is the tag's tooltip.
                            let tip = offline.then(|| SharedString::from(machine.presence.label()));
                            div()
                                .id(SharedString::from(format!("machine-{id}")))
                                .flex()
                                .min_w_0()
                                .overflow_hidden()
                                .child(ui::machine_tag(&machine.name, offline))
                                .when_some(tip, |tag, tip| {
                                    tag.tooltip(move |_, cx| {
                                        let tip = tip.clone();
                                        cx.new(|_| ui::Tip(tip)).into()
                                    })
                                })
                        });
                        // Beside the name, what gives way before it does; drawn
                        // only when there is something to show.
                        let glance = ui::glance_counts(&counts);
                        // Its space from the name is inside it, so once the name
                        // has squeezed it to nothing it costs the name nothing.
                        let beside = (tag.is_some() || glance.is_some()).then(|| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(
                                    div()
                                        .min_w_0()
                                        .pl(px(t.space.s10))
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .children(tag)
                                        .children(glance),
                                )
                        });
                        // On hover the row's options cover the end of what is
                        // left of the chevron, as a tab's take its thumbnail's
                        // place, so they reserve no width.
                        let options_right = t.space.s10
                            + if chevron.is_some() {
                                t.size.icon_button + t.space.s10
                            } else {
                                0.0
                            };
                        let options = div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(options_right))
                            .flex()
                            .items_center()
                            .bg(c.hover)
                            .invisible()
                            .group_hover(group.clone(), |style| style.visible())
                            .child(
                                ui::icon_button(
                                    SharedString::from(format!("agent-options-{id}")),
                                    "ellipsis",
                                )
                                .on_click(cx.listener(
                                    move |browser, event: &gpui::ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        browser.open_agent_menu(
                                            &options_name,
                                            event.position(),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            );
                        ui::row(SharedString::from(id.clone()), false)
                            .relative()
                            .group(group)
                            .when_some(held, |row, why| {
                                row.tooltip(move |_, cx| cx.new(|_| ui::Tip(why.into())).into())
                            })
                            .when(stale || state == AgentState::Offline, |row| {
                                row.opacity(0.75)
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(
                                    move |browser, event: &gpui::MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        browser.open_agent_menu(
                                            &menu_name,
                                            event.position,
                                            window,
                                            cx,
                                        );
                                    },
                                ),
                            )
                            .child(ui::agent_symbol(ui::shown_state(state, stale)))
                            // The name and what is beside it share one place with
                            // no gap between them; the name takes the room it needs.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_color(if calm { c.muted } else { c.text })
                                            .child(label.to_owned()),
                                    )
                                    .children(beside),
                            )
                            .children(chevron)
                            .child(options)
                            .on_click(cx.listener(
                                move |browser, event: &gpui::ClickEvent, window, cx| {
                                    if state == AgentState::Offline {
                                        browser.open_agent_menu(
                                            &click_name,
                                            event.position(),
                                            window,
                                            cx,
                                        );
                                    } else if let Some(target) = &target {
                                        browser.go_to_agent_target(target, window, cx);
                                    }
                                },
                            ))
                    };
                    // Only the chevron folds; a click elsewhere on its row
                    // still goes where the row goes.
                    let chevron = |id: String, open: bool| {
                        ui::icon_button(
                            SharedString::from(id),
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                        )
                    };
                    // The depth of the row above: a level's line goes on from it
                    // through the gap only while that row is at that level or deeper.
                    let mut above = 0;
                    for row in tree {
                        let depth = row.depth();
                        // A review's, browser's or swarm's symbol, and its label
                        // taking the room left.
                        let (symbol, label) = ui::tree_line(&row)
                            .map(|(symbol, label)| {
                                (
                                    symbol,
                                    label.flex_1().min_w_0().overflow_hidden().text_ellipsis(),
                                )
                            })
                            .unzip();
                        let line = match row {
                            agent_groups::Row::Agent {
                                name,
                                state,
                                machine,
                                counts,
                                children,
                                expanded,
                            } => {
                                let toggled = name.clone();
                                let fold = children.then(|| {
                                    chevron(format!("agent-swarms/{name}"), expanded)
                                        .on_click(cx.listener(move |browser, _, _, cx| {
                                            cx.stop_propagation();
                                            if !browser.expanded_agents.remove(&toggled) {
                                                browser.expanded_agents.insert(toggled.clone());
                                            }
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                });
                                agent(self, cx, &name, &name, state, None, machine, counts, fold)
                                    .into_any_element()
                            }
                            agent_groups::Row::Review {
                                agent,
                                swarm,
                                index,
                                review,
                            } => ui::text(
                                div()
                                    .id(SharedString::from(format!(
                                        "review-{}/{agent}/{index}",
                                        swarm.as_deref().unwrap_or_default()
                                    )))
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(px(t.space.s10))
                                    .h(px(t.size.row))
                                    .px(px(t.space.s10))
                                    .overflow_hidden()
                                    .whitespace_nowrap(),
                                &t.text.row,
                            )
                            .children(symbol)
                            .children(label)
                            .children(review.effort.map(|effort| {
                                ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.meta)
                                    .child(effort)
                            }))
                            .into_any_element(),
                            agent_groups::Row::Browser { agent, swarm, .. } => ui::row(
                                SharedString::from(format!(
                                    "browser-{}/{agent}",
                                    swarm.as_deref().unwrap_or_default()
                                )),
                                false,
                            )
                            .children(symbol)
                            .children(label)
                            .on_click(cx.listener(move |browser, _, window, cx| {
                                browser.open_agent_browser(&agent, window, cx);
                            }))
                            .into_any_element(),
                            agent_groups::Row::Swarm {
                                initiator,
                                name,
                                members,
                                listed,
                                expanded,
                            } => {
                                let key = agent_groups::swarm_key(&initiator, &name);
                                // With nobody listed under it there is nothing to
                                // count or fold; the row still opens the swarm.
                                let fold = (listed > 0).then(|| {
                                    chevron(format!("swarm-fold-{key}"), expanded).on_click(
                                        cx.listener(move |browser, _, _, cx| {
                                            cx.stop_propagation();
                                            if !browser.collapsed_swarms.remove(&key) {
                                                browser.collapsed_swarms.insert(key.clone());
                                            }
                                            cx.notify();
                                        }),
                                    )
                                });
                                let creation = creation.clone();
                                ui::row(
                                    SharedString::from(format!("swarm-{initiator}/{name}")),
                                    false,
                                )
                                .children(symbol)
                                .children(label)
                                .children((listed > 0).then(|| {
                                    ui::text(
                                        div().flex_shrink_0().text_color(c.muted),
                                        &t.text.meta,
                                    )
                                    .child(listed.to_string())
                                }))
                                .children(fold)
                                .on_click(cx.listener(move |browser, _, window, cx| {
                                    browser.open_swarm_tab(
                                        (&initiator, &name, &members),
                                        creation.as_ref(),
                                        window,
                                        cx,
                                    );
                                }))
                                .into_any_element()
                            }
                            agent_groups::Row::Member {
                                swarm,
                                name,
                                short,
                                state,
                                machine,
                            } => agent(
                                self,
                                cx,
                                &name,
                                &short,
                                state,
                                Some(&swarm),
                                machine,
                                agent_groups::Counts::default(),
                                None,
                            )
                            .into_any_element(),
                        };
                        rows = rows.child(ui::nested(
                            depth,
                            line,
                            0.0,
                            (t.size.hairline, above.min(depth)),
                        ));
                        above = depth;
                    }
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
                .child(heading)
                .child(rows)
                .into_any_element(),
        )
    }

    fn connections_block(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use super::views::peer_rows;
        let space = self.connections_workspace()?;
        let (t, c) = (tokens(), colors());
        let target = self.creation_target();
        let inbox = self.peer_inboxes.get(&space.id).map(|(_, e, _)| e.read(cx));
        let summary = inbox.and_then(|i| i.summary.clone());
        let unavailable = inbox.is_some_and(|i| i.error.is_some());
        let parts: Vec<(String, gpui::Rgba)> = if inbox.is_some_and(|i| i.not_answering()) {
            vec![("not answering".into(), c.attention_text)]
        } else if unavailable {
            vec![("unavailable".into(), c.attention_text)]
        } else {
            summary
                .as_deref()
                .map(super::connections::heading)
                .unwrap_or_default()
                .into_iter()
                .map(|(label, badge)| (label, peer_rows::badge_color(badge)))
                .collect()
        };
        // One line that sends a part that does not fit onto a second, hidden
        // line, so whole parts give way, the last first: what waits on the
        // person is kept longest and no part is cut. The empty first item
        // lets the first part go too.
        let line = t.size.keycap;
        let summary_parts = parts
            .into_iter()
            .enumerate()
            .map(|(index, (label, color))| {
                div()
                    .flex_shrink_0()
                    .h(px(line))
                    .flex()
                    .items_center()
                    .gap(px(t.space.s4))
                    .when(index > 0, |part| {
                        part.pl(px(t.space.s4))
                            .child(div().text_color(c.muted).child("·"))
                    })
                    .child(div().text_color(color).child(label))
            });
        let summary_line = ui::text(
            div()
                .min_w_0()
                .h(px(line))
                .overflow_hidden()
                .flex()
                .flex_wrap()
                .justify_end()
                .whitespace_nowrap(),
            &t.text.meta,
        )
        .child(div().w(px(0.)).h(px(line)))
        .children(summary_parts);
        let heading = section_toggle("CONNECTIONS", self.connections_collapsed)
            .child(summary_line)
            .on_click(cx.listener(|b, _, _, cx| {
                b.connections_collapsed = !b.connections_collapsed;
                cx.notify();
            }));
        let mut section = div().flex_shrink_0().child(heading);
        if self.connections_collapsed {
            return Some(section.into_any_element());
        }
        let key = self
            .peer_inboxes
            .get(&space.id)
            .map(|(key, _, _)| key.clone());
        // Opening a row: Option-click opens Connections in a new tab.
        let open = |id: SharedString, opening: Option<super::peer_inbox::Opening>| {
            let workspace = space.id.clone();
            let key = key.clone();
            let target = target.clone();
            ui::row(id, false).mx(px(t.space.s8)).on_click(cx.listener(
                move |b, event: &gpui::ClickEvent, window, cx| {
                    if !b.saving
                        && !b.save_failed
                        && target.as_ref().is_some_and(|t| b.creation_is_current(t))
                    {
                        b.peer_to_open = opening
                            .clone()
                            .zip(key.clone())
                            .map(|(opening, key)| (workspace.clone(), key, opening));
                        let new_tab = event.modifiers().alt;
                        b.open_connections(target.as_ref(), None, new_tab, window, cx);
                    }
                },
            ))
        };
        let entries = summary
            .as_deref()
            .map(peer_rows::entries)
            .unwrap_or_default();
        if entries.is_empty() {
            if !super::connections::offers_connect(summary.is_some(), unavailable) {
                return Some(section.into_any_element());
            }
            section = section.child(
                open(
                    "connect-a-profile".into(),
                    Some(super::peer_inbox::Opening::Connect),
                )
                .text_color(c.text_2)
                .child(ui::icon_slot("plus"))
                .child("Connect a profile"),
            );
            return Some(section.into_any_element());
        }
        let (shown, more) = super::connections::panel_rows(entries, |entry| entry.needs);
        for entry in shown {
            let id = match &entry.selection {
                super::peer_inbox::Selection::Peer(g) => format!("connection-{}", g.generation),
                super::peer_inbox::Selection::Request(r) => format!("connection-request-{r}"),
            };
            let name = peer_rows::display_name(entry.name.as_deref()).to_owned();
            section = section.child(
                open(
                    id.into(),
                    Some(super::peer_inbox::Opening::Peer(entry.selection.clone())),
                )
                .child(peer_rows::peer_mark(
                    entry.name.as_deref(),
                    t.size.agent_symbol,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(peer_rows::name_color(entry.name.as_deref()))
                        .child(name),
                )
                .children(
                    entry
                        .badge
                        .map(|(label, badge)| peer_rows::badge(&label, badge)),
                ),
            );
        }
        if more > 0 {
            section = section.child(
                open("connections-more".into(), None)
                    .text_color(c.muted)
                    .child(ui::symbol_slot(div()))
                    .child(format!("{more} more")),
            );
        }
        Some(section.into_any_element())
    }

    /// One-click workspace switching, then Settings.
    fn switcher(&mut self, width: gpui::Pixels, cx: &mut Context<Self>) -> AnyElement {
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
                let origin = self.navigation_origin(&space.id);
                let id = space.id.clone();
                let current = model.selected == id;
                let needs_you = space.tabs.iter().any(|tab| self.tab_needs_you(tab))
                    || self
                        .peer_inboxes
                        .get(&id)
                        .and_then(|(_, inbox, _)| inbox.read(cx).summary.as_deref())
                        .is_some_and(|s| s.attention());
                let glyph = icons::workspace(space.symbol.as_deref(), space.icon.as_deref());
                let color = if current { c.accent } else { c.muted };
                let dragged = WorkspaceDrag {
                    browser: cx.entity_id(),
                    workspace: id.clone(),
                    name: space.name.clone(),
                };
                let (target, destination) = (id.clone(), id.clone());
                let mut element = slot(SharedString::from(format!("workspace-slot-{id}")))
                    .when(current, |el| el.bg(c.accent_soft))
                    .when(!current, |el| el.hover(|el| el.bg(colors().hover)))
                    .when(!self.save_failed, |el| {
                        el.on_drag(dragged, start_drag(cx))
                            .on_drag_move(cx.listener(
                                move |browser,
                                      event: &gpui::DragMoveEvent<WorkspaceDrag>,
                                      _,
                                      cx| {
                                    let drag = event.drag(cx);
                                    if drag.browser == cx.entity_id()
                                        && browser.workspace_exists(&drag.workspace)
                                    {
                                        let position = event.event.position;
                                        browser.track_landing(
                                            &target,
                                            event.bounds,
                                            position,
                                            true,
                                            cx,
                                        );
                                    }
                                },
                            ))
                            .on_drop(cx.listener(move |browser, dragged: &WorkspaceDrag, _, cx| {
                                if dragged.browser != cx.entity_id() {
                                    return;
                                }
                                let after = browser.dropped_after(&destination);
                                browser.drop_reorder(
                                    Reorder::Workspace {
                                        source: dragged.workspace.clone(),
                                        destination: destination.clone(),
                                        after,
                                    },
                                    cx,
                                );
                            }))
                    })
                    .children(self.landing(&id, cx).map(|after| landing_line(true, after)));
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
                                let origin = origin.clone();
                                move |browser, event: &gpui::MouseDownEvent, window, cx| {
                                    if !origin.as_ref().is_some_and(|origin| {
                                        browser.select_workspace_origin(origin)
                                    }) {
                                        return;
                                    }
                                    browser.open_workspace_menu(event.position, window, cx);
                                    cx.stop_propagation();
                                }
                            }),
                        )
                        .on_click(cx.listener(move |browser, _, window, cx| {
                            if !origin
                                .as_ref()
                                .is_some_and(|origin| browser.select_workspace_origin(origin))
                            {
                                return;
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
    use super::reorder_tab_proposal;

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
        let (mut base, down) =
            reorder_tab_proposal(&layout, &space.id, first, third, true).unwrap();
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
        let (_, up) = reorder_tab_proposal(&layout, &space.id, first, second, false).unwrap();
        assert_eq!(up.workspace().unwrap().tabs, space.tabs);
        assert_eq!(up.workspace().unwrap().selected, space.selected);
        assert!(reorder_tab_proposal(&layout, &space.id, "removed", second, false).is_none());
        assert!(reorder_tab_proposal(&layout, "other-workspace", first, third, false).is_none());
        assert!(reorder_tab_proposal(&layout, &space.id, first, "removed", false).is_none());
        // Third is already just before First: dropping it there changes nothing.
        assert!(reorder_tab_proposal(&layout, &space.id, third, first, false).is_none());
    }

    use super::{
        Reorder, click_ends_drag, landing_at, reorder_workspace_proposal, tab_row_draggable,
    };

    fn library(workspaces: &[(&str, &[&str])]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        drop(agent_interface_store::Store::open_or_create(directory.path()).unwrap());
        let mut store =
            agent_interface_store::Store::open_existing_writable(directory.path()).unwrap();
        let mut base = store.load().unwrap();
        let mut model = base.model().clone();
        for (index, (name, tabs)) in workspaces.iter().enumerate() {
            if index == 0 {
                model.workspaces[0].name = (*name).into();
            } else {
                model.add_workspace((*name).into());
            }
            for tab in *tabs {
                model.add_tab(Some((*tab).into()), "/work".into(), true);
            }
        }
        let first = model.workspaces[0].id.clone();
        model.choose_workspace(&first);
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        directory
    }

    fn store(directory: &std::path::Path) -> agent_interface_store::Store {
        agent_interface_store::Store::open_existing_writable(directory).unwrap()
    }

    fn workspace_names(model: &agent_interface_core::Layout) -> Vec<&str> {
        model
            .workspaces
            .iter()
            .map(|space| space.name.as_str())
            .collect()
    }

    fn tab_names(model: &agent_interface_core::Layout) -> Vec<&str> {
        let space = model.workspace().unwrap();
        space.tabs.iter().map(|tab| tab.name.as_str()).collect()
    }

    #[test]
    fn after_a_drag_only_a_mouse_release_is_held_back_and_keys_still_activate() {
        let at = gpui::point(gpui::px(20.), gpui::px(20.));
        let mouse = gpui::ClickEvent::Mouse(gpui::MouseClickEvent {
            down: gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: at,
                modifiers: gpui::Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            },
            up: gpui::MouseUpEvent {
                button: gpui::MouseButton::Left,
                position: at,
                modifiers: gpui::Modifiers::none(),
                click_count: 1,
            },
        });
        let bounds = gpui::Bounds::new(at, gpui::size(gpui::px(40.), gpui::px(30.)));
        let enter = gpui::ClickEvent::Keyboard(gpui::KeyboardClickEvent {
            button: gpui::KeyboardButton::Enter,
            bounds,
        });
        let space = gpui::ClickEvent::Keyboard(gpui::KeyboardClickEvent {
            button: gpui::KeyboardButton::Space,
            bounds,
        });
        assert!(click_ends_drag(true, &mouse));
        assert!(!click_ends_drag(false, &mouse));
        for key in [&enter, &space] {
            assert!(!click_ends_drag(true, key));
            assert!(!click_ends_drag(false, key));
        }
    }

    #[test]
    fn a_dragged_tab_shows_a_landing_only_among_its_own_workspaces_tabs() {
        let directory = library(&[("Work", &["First", "Second"]), ("Other", &["Third"])]);
        let mut browser = super::Browser {
            layout: Some(crate::library::WindowLayout::new(
                store(directory.path()).load().unwrap(),
            )),
            directory: Some(directory.path().to_owned()),
            ..Default::default()
        };
        let model = browser.layout.as_ref().unwrap().model().clone();
        let (work, other) = (&model.workspaces[0], &model.workspaces[1]);
        let first = work.tabs[0].id.clone();
        let origin = browser.selected_navigation_origin();
        assert!(browser.tab_drag_lands_here(origin.as_ref(), &work.id, &first));
        assert!(!browser.tab_drag_lands_here(origin.as_ref(), &work.id, "removed"));
        assert!(browser.workspace_exists(&other.id));
        assert!(!browser.workspace_exists("removed"));
        // Switched to another workspace mid-drag: its tabs show no landing.
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_workspace(&other.id);
        assert!(!browser.tab_drag_lands_here(origin.as_ref(), &work.id, &first));
    }

    #[test]
    fn a_running_save_leaves_tab_rows_draggable() {
        assert!(tab_row_draggable(false, false));
        assert!(!tab_row_draggable(true, false));
        assert!(!tab_row_draggable(false, true));
    }

    #[test]
    fn a_drop_lands_after_an_item_past_its_middle_and_only_where_its_list_shows_it() {
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(10.), gpui::px(100.)),
            gpui::size(gpui::px(40.), gpui::px(30.)),
        );
        let list = gpui::Bounds::new(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(200.), gpui::px(400.)),
        );
        let at = |x: f32, y: f32| gpui::point(gpui::px(x), gpui::px(y));
        assert_eq!(landing_at(bounds, list, at(45., 105.), false), Some(false));
        assert_eq!(landing_at(bounds, list, at(15., 125.), false), Some(true));
        assert_eq!(landing_at(bounds, list, at(15., 125.), true), Some(false));
        assert_eq!(landing_at(bounds, list, at(45., 105.), true), Some(true));
        assert_eq!(landing_at(bounds, list, at(5., 105.), false), None);
        // The row's top is scrolled under the section header: no landing there.
        let scrolled = gpui::Bounds::new(
            gpui::point(gpui::px(0.), gpui::px(110.)),
            gpui::size(gpui::px(200.), gpui::px(290.)),
        );
        assert_eq!(landing_at(bounds, scrolled, at(15., 105.), false), None);
        assert_eq!(
            landing_at(bounds, scrolled, at(15., 125.), false),
            Some(true)
        );
    }

    /// Save `change` to the library as another save would, and return its result.
    fn save_change(
        library: &mut agent_interface_store::Store,
        change: impl FnOnce(&mut agent_interface_core::Layout),
    ) -> agent_interface_store::Reconciliation {
        let mut base = library.load().unwrap();
        let mut changed = base.model().clone();
        change(&mut changed);
        library.save(&mut base, &changed).unwrap()
    }

    #[test]
    fn a_drop_made_during_a_save_lands_after_it_unless_it_fails_conflicts_or_is_gone() {
        let directory = library(&[("Work", &["First", "Second", "Third"]), ("Other", &[])]);
        let mut library = store(directory.path());
        let mut browser = super::Browser {
            layout: Some(crate::library::WindowLayout::new(library.load().unwrap())),
            directory: Some(directory.path().to_owned()),
            ..Default::default()
        };
        let model = browser.layout.as_ref().unwrap().model().clone();
        let space = model.workspace().unwrap();
        let tab = |index: usize| space.tabs[index].id.clone();
        let origin = browser.selected_navigation_origin();
        let drop = |source: usize, destination: usize| Reorder::Tab {
            origin: origin.clone(),
            workspace: space.id.clone(),
            tab: tab(source),
            destination: tab(destination),
            after: true,
        };
        // First after Third, dropped while a save runs: kept, nothing saved.
        browser.saving = true;
        assert!(browser.reorder_proposal(drop(0, 2)).is_none());
        assert!(browser.queued_reorder.is_some());
        // That save is stored: the drop is applied to the layout it left.
        let stored = save_change(&mut library, |layout| {
            layout.workspaces[0].name = "Working".into()
        });
        assert!(!stored.conflict);
        browser.saving = false;
        browser.layout = Some(crate::library::WindowLayout::new(library.load().unwrap()));
        let queued = browser.finish_queued_reorder(Some(&stored)).unwrap();
        let (mut base, proposed) = browser.reorder_proposal(queued).unwrap();
        assert!(!library.save(&mut base, &proposed).unwrap().conflict);
        let saved = library.load().unwrap();
        assert_eq!(tab_names(saved.model()), ["Second", "Third", "First"]);
        assert_eq!(saved.model().workspace().unwrap().selected, space.selected);
        browser.layout = Some(crate::library::WindowLayout::new(saved));

        // A workspace dropped during a save is applied the same way.
        browser.saving = true;
        let other_first = Reorder::Workspace {
            source: model.workspaces[1].id.clone(),
            destination: model.workspaces[0].id.clone(),
            after: false,
        };
        assert!(browser.reorder_proposal(other_first).is_none());
        browser.saving = false;
        let queued = browser.finish_queued_reorder(Some(&stored)).unwrap();
        let (_, proposed) = browser.reorder_proposal(queued).unwrap();
        assert_eq!(workspace_names(&proposed), ["Other", "Working"]);

        // A drop made during a save that fails is discarded.
        browser.saving = true;
        assert!(browser.reorder_proposal(drop(1, 0)).is_none());
        browser.saving = false;
        assert!(browser.finish_queued_reorder(None).is_none());
        assert!(browser.queued_reorder.is_none());
        // So is one made during a save that conflicts: the window's notice
        // that its edit was not saved stays, and no further save hides it.
        browser.saving = true;
        assert!(browser.reorder_proposal(drop(1, 0)).is_none());
        let mut stale = library.load().unwrap();
        let latest = save_change(&mut library, |layout| {
            layout.workspaces[0].tabs.rotate_right(1)
        });
        assert!(!latest.conflict);
        let mut mine = stale.model().clone();
        mine.workspaces[0].tabs.swap(0, 1);
        let conflicted = library.save(&mut stale, &mine).unwrap();
        assert!(conflicted.conflict);
        browser.saving = false;
        assert!(browser.finish_queued_reorder(Some(&conflicted)).is_none());
        assert!(browser.queued_reorder.is_none());
        // After a failed save waiting for Retry, a drop does nothing.
        browser.layout = Some(crate::library::WindowLayout::new(library.load().unwrap()));
        browser.save_failed = true;
        assert!(browser.reorder_proposal(drop(1, 0)).is_none());
        assert!(browser.queued_reorder.is_none());
        browser.save_failed = false;

        // The running save removed the dropped tab: nothing happens.
        browser.saving = true;
        assert!(browser.reorder_proposal(drop(1, 2)).is_none());
        let second = tab(1);
        let removed = save_change(&mut library, |layout| {
            layout.workspaces[0].tabs.retain(|tab| tab.id != second)
        });
        browser.apply(crate::library::Update {
            document: Some(library.load().unwrap()),
            error: None,
        });
        browser.saving = false;
        let queued = browser.finish_queued_reorder(Some(&removed)).unwrap();
        assert!(browser.reorder_proposal(queued).is_none());
    }

    #[test]
    fn a_moved_workspace_is_stored_and_merges_or_conflicts_with_another_writer() {
        let directory = library(&[
            ("Alpha", &["One"]),
            ("Beta", &["Two"]),
            ("Gamma", &["Three"]),
        ]);
        let mut library = store(directory.path());
        let layout = crate::library::WindowLayout::new(library.load().unwrap());
        let ids: Vec<String> = layout
            .model()
            .workspaces
            .iter()
            .map(|space| space.id.clone())
            .collect();
        let selected = layout.model().selected.clone();
        // Beta before Alpha.
        let (mut base, proposed) =
            reorder_workspace_proposal(&layout, &ids[1], &ids[0], false).unwrap();
        assert!(!library.save(&mut base, &proposed).unwrap().conflict);
        let saved = library.load().unwrap();
        assert_eq!(workspace_names(saved.model()), ["Beta", "Alpha", "Gamma"]);
        assert_eq!(saved.model().selected, selected);
        let layout = crate::library::WindowLayout::new(saved);

        // Another writer renames Beta: Alpha after Gamma merges with it.
        let mut other = store(directory.path());
        let mut latest = other.load().unwrap();
        let mut renamed = latest.model().clone();
        renamed.workspaces[0].name = "Bravo".into();
        assert!(!other.save(&mut latest, &renamed).unwrap().conflict);
        let (mut base, proposed) =
            reorder_workspace_proposal(&layout, &ids[0], &ids[2], true).unwrap();
        assert!(!library.save(&mut base, &proposed).unwrap().conflict);
        assert_eq!(
            workspace_names(library.load().unwrap().model()),
            ["Bravo", "Gamma", "Alpha"]
        );
        let layout = crate::library::WindowLayout::new(library.load().unwrap());

        // Another writer moves Alpha first: a different move conflicts, and
        // that writer's order stays.
        let mut latest = other.load().unwrap();
        let mut moved = latest.model().clone();
        assert!(moved.move_workspace(&ids[0], &ids[1], false));
        assert!(!other.save(&mut latest, &moved).unwrap().conflict);
        let (mut base, proposed) =
            reorder_workspace_proposal(&layout, &ids[1], &ids[0], true).unwrap();
        assert!(library.save(&mut base, &proposed).unwrap().conflict);
        assert_eq!(
            workspace_names(library.load().unwrap().model()),
            ["Alpha", "Bravo", "Gamma"]
        );
    }
}
