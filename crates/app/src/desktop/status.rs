//! The status row along the bottom of the window: where you are, the keys
//! for the current mode, the agents toggle and the saved state.

use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*, px};

use super::{
    Browser, components as ui,
    design::{colors, tokens},
    shortcuts::{self, Command},
};

impl Browser {
    /// Show or hide the agents section; saved with the library.
    pub(super) fn toggle_agents(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = self
            .layout
            .as_ref()
            .map(|layout| layout.model().show_agents)
        {
            self.set_show_agents(!shown, cx);
        }
    }

    /// Show the agents section or not; saved with the library when it changes.
    pub(super) fn set_show_agents(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(layout) = &mut self.layout
            && layout.model().show_agents != shown
        {
            layout.model_mut().show_agents = shown;
            self.save(cx);
        }
        cx.notify();
    }

    pub(super) fn status_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let chevron = || ui::icon("chevron-right", t.size.icon - t.space.s4, c.muted);
        // Left: where you are, or the mode you are in.
        let mut left = div().flex().items_center().gap(px(t.space.s6)).min_w_0();
        let mut keys: Vec<(String, &'static str)> = Vec::new();
        let editing = self.editing.as_ref().map(|edit| edit.tab);
        if self.save_failed {
            left = left
                .child(ui::agent_symbol(ui::AgentState::NeedsYou))
                .child(
                    div()
                        .text_color(c.attention_text)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(match &self.error {
                            Some(error) => format!("Changes not saved: {error}"),
                            None => "Changes not saved".to_owned(),
                        }),
                );
        } else if let Some(tab) = editing {
            left = left
                .child(div().text_color(c.text).child(if tab {
                    "Rename tab"
                } else {
                    "Rename workspace"
                }))
                .child("type a new name");
            keys = vec![
                ("↵".into(), "save"),
                ("esc".into(), "cancel"),
                (
                    if cfg!(target_os = "macos") {
                        "⌃U"
                    } else {
                        "Ctrl+U"
                    }
                    .into(),
                    "clear",
                ),
            ];
        } else if self.chooser.is_some() {
            left = left.child(div().text_color(c.text).child("Attach session"));
            keys = vec![
                ("↑↓".into(), "move"),
                ("↵".into(), "attach"),
                ("esc".into(), "back"),
            ];
        } else if let Some(model) = self.layout.as_ref().map(|layout| layout.model()) {
            if let Some(space) = model.workspace() {
                left = left.child(div().text_color(c.text).child(space.name.clone()));
                match model.tab() {
                    Some(tab) => {
                        let panes = tab.tree.leaves();
                        let position = panes
                            .iter()
                            .position(|leaf| leaf.id == tab.focus)
                            .unwrap_or(0);
                        left = left
                            .child(chevron())
                            .child(div().text_color(c.text).child(tab.name.clone()))
                            .child(chevron())
                            .child(format!("pane {} of {}", position + 1, panes.len()));
                        keys = vec![
                            (shortcuts::label(Command::SplitRight), "split"),
                            (shortcuts::label(Command::SplitBelow), "split below"),
                            (shortcuts::label(Command::FocusPane), "expand"),
                            (shortcuts::label(Command::Jump), "jump"),
                        ];
                    }
                    None => {
                        left = left.child(chevron()).child("no tabs");
                        keys = vec![
                            (shortcuts::label(Command::NewTab), "new tab"),
                            (shortcuts::label(Command::Jump), "jump"),
                        ];
                    }
                }
            }
        } else if !self.loading {
            left = left.child("No library open");
        }
        // Keys give way when the window is too narrow for them.
        let room = window.viewport_size().width > px(t.size.panel_width * 4.0);
        let mut centre = div().flex().items_center().gap(px(t.space.s18));
        if room {
            for (key, word) in keys {
                centre = centre.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s6))
                        .child(ui::keycap(key))
                        .child(word),
                );
            }
        }
        // Right: the agents toggle and the saved state or a message.
        let mut right = div().flex().items_center().gap(px(t.space.s18));
        if self.roster.is_some() {
            let shown = self
                .layout
                .as_ref()
                .is_some_and(|layout| layout.model().show_agents);
            right = right.child(
                div()
                    .id("toggle-agents")
                    .cursor_pointer()
                    .hover(|el| el.text_color(colors().text))
                    .child(if shown {
                        "agents shown"
                    } else {
                        "agents hidden"
                    })
                    .on_click(cx.listener(|browser, _, _, cx| browser.toggle_agents(cx))),
            );
        }
        let message = self
            .save_notice
            .clone()
            .or_else(|| (!self.save_failed).then(|| self.error.clone()).flatten())
            .or_else(|| self.loading.then(|| "Opening library…".to_owned()));
        right = if self.save_failed {
            right
                .child(
                    ui::button("retry-save", "Retry saving", ui::Button::Secondary)
                        .h(px(t.size.status_row - t.space.s6))
                        .on_click(cx.listener(|browser, _, _, cx| browser.save(cx))),
                )
                .child(
                    ui::button("discard-save", "Discard", ui::Button::Danger)
                        .h(px(t.size.status_row - t.space.s6))
                        .on_click(cx.listener(|browser, _, _, cx| browser.discard_save(cx))),
                )
        } else if self.saving {
            right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .child(ui::dot(c.accent))
                    .child("Saving changes…"),
            )
        } else if let Some(message) = message {
            right.child(div().text_color(c.attention_text).child(message))
        } else {
            right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .child(ui::dot(c.ok))
                    .child("Saved"),
            )
        };
        ui::text(
            div()
                .relative()
                .h(px(t.size.status_row))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_between()
                .px(px(t.space.s14))
                .bg(c.panel)
                .border_t(px(t.size.hairline))
                .border_color(c.line)
                .font_family(t.font.ui.clone())
                .text_color(c.muted),
            &t.text.status,
        )
        .child(left)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(centre),
        )
        .child(right)
        .into_any_element()
    }
}
