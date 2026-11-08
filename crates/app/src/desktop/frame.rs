//! The content area beside the panel, its empty states, and the layer that
//! places popovers over the window.

use agent_interface_core::Node;
use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*, px};

use super::{
    Browser, components as ui,
    design::{colors, tokens},
    layout_edit::Create,
    shortcuts::{self, Command},
    split_resize,
};

impl Browser {
    /// The selected tab's panes, or the state that replaces them.
    pub(super) fn content(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let area = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(c.ground)
            .pt(px(t.size.window_inset))
            .pr(px(t.size.window_inset))
            .pb(px(t.size.pane_gap))
            .pl(px(t.size.pane_gap));
        let Some(layout) = &self.layout else {
            if self.loading {
                return area.into_any_element();
            }
            return area
                .child(
                    ui::empty_state(
                        ui::empty_tile("folder-open", c.selected, c.text_2),
                        "No library open",
                        "Choose a folder with layouts.db, or select a new folder to create an empty library.",
                    )
                    .child(
                        ui::button("open-library", "Open Library…", ui::Button::Primary)
                            .child(
                                ui::key_hint(
                                    shortcuts::Chord::parse(shortcuts::platform_key("cmd-o"))
                                        .unwrap()
                                        .label(),
                                )
                                .text_color(c.on_accent),
                            )
                            .on_click(cx.listener(|browser, _, _, cx| browser.pick_library(cx))),
                    ),
                )
                .into_any_element();
        };
        let model = layout.model();
        if self.profile_content_unavailable() {
            let origin = self.selected_navigation_origin();
            let start_origin = origin.clone();
            let failure = self.profile_failure().filter(|_| !self.profile_starting);
            let (title, note) = match failure {
                Some(failure) => ("Workspace profile unavailable", failure),
                None if self.profile_starting => (
                    "Starting this profile’s service…",
                    "This can take a moment.",
                ),
                None => (
                    "Connecting to this workspace’s profile…",
                    "This can take a moment.",
                ),
            };
            return area
                .child(
                    ui::empty_state(ui::empty_tile("unplug", c.selected, c.text_2), title, note)
                        .when(failure.is_some() && self.profile_can_start(), |state| {
                            state.child(
                                ui::button("start-profile", "Start service", ui::Button::Primary)
                                    .on_click(cx.listener(move |browser, _, _, cx| {
                                        browser.start_profile_service(start_origin.as_ref(), cx)
                                    })),
                            )
                        })
                        .when(failure.is_some(), |state| {
                            state.child(
                                ui::button(
                                    "retry-profile",
                                    "Retry connection",
                                    ui::Button::Secondary,
                                )
                                .on_click(cx.listener(
                                    move |browser, _, _, cx| {
                                        browser.retry_profile(origin.as_ref(), cx)
                                    },
                                )),
                            )
                        }),
                )
                .into_any_element();
        }
        let Some(tab) = model.tab() else {
            let creation = self.creation_target();
            let name = model
                .workspace()
                .map_or(String::new(), |space| space.name.clone());
            return area
                .child(
                    ui::empty_state(
                        ui::empty_tile("square-plus", c.accent_soft, c.accent),
                        format!("{name} has no tabs"),
                        "A tab holds a split layout of terminals. New tabs start in the launch directory.",
                    )
                    .child(
                        ui::button("first-tab", "New tab", ui::Button::Primary)
                            .child(ui::key_hint(shortcuts::label(Command::NewTab)).text_color(c.on_accent))
                            .on_click(cx.listener(move |browser, _, window, cx| browser.create_from(Create::Tab, creation.as_ref(), window, cx))),
                    ),
                )
                .into_any_element();
        };
        let leaves = tab.tree.leaves();
        let selected = leaves
            .iter()
            .position(|leaf| leaf.id == tab.focus)
            .unwrap_or(0);
        let tree = model
            .focus_mode()
            .then(|| leaves.get(selected).map(|leaf| Node::Leaf((*leaf).clone())))
            .flatten();
        let numbers = leaves
            .iter()
            .map(|leaf| leaf.id.clone())
            .collect::<Vec<_>>();
        let states = self.agent_states();
        let leaf_states = leaves
            .iter()
            .filter_map(|leaf| {
                let attachment = leaf.attachment.as_ref()?;
                Some((
                    leaf.id.clone(),
                    self.attachment_state(attachment, states.as_deref())?,
                ))
            })
            .collect();
        // Cached paint already contains inherited opacity. Repaint once
        // when pane focus changes so dimming follows the new selection.
        let focus_changed = self.painted_pane_focus.as_ref() != Some(&tab.focus);
        self.painted_pane_focus = Some(tab.focus.clone());
        let cards = super::pane_cards::Cards {
            origin: self.selected_navigation_origin(),
            focus: &tab.focus,
            focus_changed,
            accessibility_active: window.is_a11y_active(),
            numbers: &numbers,
            terminals: &self.terminals,
            views: &self.views,
            new_panes: &self.new_panes,
            chooser: self.chooser.as_ref(),
            split_bounds: &self.split_bounds,
            pane_bounds: &self.pane_bounds,
            drop_target: self
                .pane_drag
                .as_ref()
                .filter(|drag| drag.dragging)
                .and_then(|drag| drag.destination.as_ref()),
            leaf_states,
            unavailable: leaves
                .iter()
                .filter(|leaf| leaf.runs_terminal() && !self.terminal_available(&leaf.id))
                .map(|leaf| {
                    (
                        leaf.id.clone(),
                        "This terminal is unavailable for the workspace profile.",
                    )
                })
                .collect(),
        };
        let node = tree
            .as_ref()
            .or_else(|| {
                self.pane_drag
                    .as_ref()
                    .and_then(super::pane_drag::Drag::tree)
            })
            .or_else(|| self.resize.as_ref().map(split_resize::Drag::tree))
            .unwrap_or(&tab.tree);
        let element = Self::tree(node, &cards, cx);
        if self.profile_status_unavailable() {
            return area
                .flex()
                .flex_col()
                .child(ui::group_note(
                    "Agent status is unavailable right now. Terminals stay connected.",
                ))
                .child(div().flex_1().min_h_0().child(element))
                .into_any_element();
        }
        area.child(element).into_any_element()
    }

    /// New profile's steps: alone until its workspace shows a pane, then beside it.
    pub(super) fn beside_profile_flow(
        &self,
        content: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some((flow, _)) = &self.profile_flow else {
            return content;
        };
        let t = tokens();
        let flow_state = flow.read(cx);
        let profile = flow_state.profile_id();
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
        else {
            return content;
        };
        // It belongs to one workspace: the one bound to its profile, or before
        // that exists, the one it was opened from. Others show as they are.
        let here = match profile {
            Some(profile) => space.profile_id.as_deref() == Some(profile),
            None => flow_state.home.as_deref() == Some(space.id.as_str()),
        };
        if !here {
            return content;
        }
        let beside = flow_state.beside()
            && profile.is_some()
            && space
                .tabs
                .iter()
                .flat_map(|tab| tab.tree.leaves())
                .any(|leaf| leaf.attachment.is_some());
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .bg(colors().ground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pt(px(t.size.window_inset))
                    .pb(px(t.size.pane_gap))
                    .pl(px(t.size.pane_gap))
                    .when(!beside, |el| el.pr(px(t.size.window_inset)))
                    .child(flow.clone()),
            )
            .when(beside, |el| el.child(content))
            .into_any_element()
    }

    /// The open menu or jump popover, placed where it was opened.
    pub(super) fn popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = tokens();
        let body = if let Some((picker, _)) = &self.profile_picker {
            div().child(picker.clone())
        } else if let Some(jump) = self.jump_popover(cx) {
            jump
        } else if self.icon_picker {
            self.icon_picker(cx)
        } else if self.tab_menu.is_some() {
            self.tab_options(cx)
        } else if self.workspace_menu {
            self.workspace_options(cx)
        } else {
            return None;
        };
        // A popover taller than the window scrolls; it also takes every pointer
        // event over it, so nothing underneath reacts to its clicks.
        let height = window.viewport_size().height - px(t.space.s16);
        let body = body
            .id("popover")
            .max_h(height)
            .overflow_y_scroll()
            .occlude();
        let at = self
            .menu_at
            .unwrap_or_else(|| gpui::point(px(t.space.s8), px(t.size.titlebar)));
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(t.space.s8))
                    .child(
                        body.on_mouse_down_out(cx.listener(|browser, _, window, cx| {
                            if browser.jump.is_some() {
                                browser.close_jump(window, cx);
                            }
                            browser.close_menus();
                            cx.notify();
                        })),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
