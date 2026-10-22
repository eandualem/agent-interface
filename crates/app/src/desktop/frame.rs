//! The content area beside the panel, its empty states, and the layer that
//! places popovers over the window.

use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*, px};
use tmux_workspaces_core::Node;

use super::{
    Browser, components as ui,
    design::{colors, tokens},
    layout_edit::Create,
    shortcuts::{self, Command},
    split_resize,
};

impl Browser {
    /// The selected tab's panes, or the state that replaces them.
    pub(super) fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
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
                        "Choose a folder that holds layouts.db. The app never creates or repairs a library.",
                    )
                    .child(
                        ui::button("open-library", "Open Library…", ui::Button::Primary)
                            .child(ui::key_hint("⌘O").text_color(c.on_accent))
                            .on_click(cx.listener(|browser, _, _, cx| browser.pick_library(cx))),
                    ),
                )
                .into_any_element();
        };
        let model = layout.model();
        let Some(tab) = model.tab() else {
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
                            .on_click(cx.listener(|browser, _, window, cx| browser.create(Create::Tab, window, cx))),
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
            .focus
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
                    self.attachment_state(attachment, states.as_ref())?,
                ))
            })
            .collect();
        let cards = super::pane_cards::Cards {
            focus: &tab.focus,
            numbers: &numbers,
            terminals: &self.terminals,
            chooser: self.chooser.as_ref(),
            split_bounds: &self.split_bounds,
            leaf_states,
        };
        let node = tree
            .as_ref()
            .or_else(|| self.resize.as_ref().map(split_resize::Drag::tree))
            .unwrap_or(&tab.tree)
            .clone();
        let element = Self::tree(&node, &cards, cx);
        area.child(element).into_any_element()
    }

    /// The open menu or jump popover, placed where it was opened.
    pub(super) fn popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = tokens();
        let body = if let Some(jump) = self.jump_popover(cx) {
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
