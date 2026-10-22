//! Native selection of an existing session on one captured source server.

use std::path::PathBuf;

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, ScrollHandle, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};

use super::{
    components as ui, design,
    name::{NameEvent, NameField},
};
use crate::sessions::SessionSource;
use tmux_workspaces_core::SessionChoices;

pub(super) enum Choice {
    Session(String),
    Cancel,
}

pub(super) struct SessionChooser {
    field: Entity<NameField>,
    choices: SessionChoices,
    loading: bool,
    error: Option<String>,
    scroll: ScrollHandle,
    source: Option<SessionSource>,
    _updates: Task<()>,
    _input: Subscription,
}

impl EventEmitter<Choice> for SessionChooser {}

impl SessionChooser {
    pub fn new(socket: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| NameField::search(window, cx));
        let input = cx.subscribe_in(&field, window, |chooser, _, event, _, cx| {
            match event {
                NameEvent::Changed(query) => {
                    chooser.choices.edit('\u{15}');
                    for ch in query.chars() {
                        chooser.choices.edit(ch);
                    }
                    chooser.scroll.scroll_to_item(0);
                }
                NameEvent::Save => {
                    chooser.refresh(cx);
                    if chooser.error.is_none()
                        && let Some(name) = chooser.choices.choice()
                    {
                        cx.emit(Choice::Session(name.into()));
                    }
                }
                NameEvent::Cancel => cx.emit(Choice::Cancel),
            }
            cx.notify();
        });
        let (wake, mut woken) = mpsc::unbounded();
        let (source, error) = match SessionSource::start(socket, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(source) => (Some(source), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let updates = cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this.update(cx, |chooser, cx| chooser.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            field,
            choices: SessionChoices::default(),
            loading: source.is_some(),
            error,
            scroll: ScrollHandle::new(),
            source,
            _updates: updates,
            _input: input,
        }
    }

    pub fn composing(&self, cx: &gpui::App) -> bool {
        self.field.read(cx).composing()
    }

    pub fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.field.read(cx).focus.clone()
    }

    pub fn focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.field.read(cx).focus.is_focused(window)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(observation) = self.source.as_ref().and_then(SessionSource::take_update) {
            self.choices.update(observation.sessions);
            self.error = observation.error;
            self.loading = false;
            // Discovery does not move the viewport: mouse browsing must survive
            // both unchanged polls and insertions above the current selection.
            cx.notify();
        }
    }

    fn choose(&mut self, name: &str, cx: &mut Context<Self>) {
        self.refresh(cx);
        // A click may come from an older frame. Failed discovery must not leave
        // a selectable row that claims the old source observation is current.
        if self.error.is_none() && self.choices.rows().iter().any(|item| item == name) {
            cx.emit(Choice::Session(name.into()));
        }
    }
}

impl Drop for SessionChooser {
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            // A bounded discovery command may still be running. Joining its
            // worker must not delay the window's next terminal frame.
            std::thread::spawn(move || drop(source));
        }
    }
}

impl Render for SessionChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (design::tokens(), design::colors());
        self.choices.presented();
        let names = self.choices.rows().to_vec();
        let status = if let Some(error) = &self.error {
            Some(error.clone())
        } else if self.loading {
            Some("Loading sessions…".into())
        } else if names.is_empty() {
            Some("No session matches the filter".into())
        } else {
            None
        };
        let mut rows = div()
            .id("session-rows")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline))
            .p(px(t.space.s8))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for (index, name) in names.into_iter().enumerate() {
            let chosen = Some(index) == self.choices.index();
            rows = rows.child(
                ui::row(SharedString::from(format!("session-{name}")), chosen)
                    .font_family(t.font.mono.clone())
                    .text_color(c.text)
                    .child(name.clone())
                    .on_click(cx.listener(move |chooser, _, _, cx| {
                        cx.stop_propagation();
                        chooser.choose(&name, cx);
                    })),
            );
        }
        if let Some(status) = status {
            rows = rows.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .h(px(t.size.row))
                        .flex()
                        .items_center()
                        .text_color(c.muted),
                    &t.text.body,
                )
                .child(status),
            );
        }
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(c.surface)
            .font_family(t.font.ui.clone())
            // Keep the filter focused during a row's mouse-down, until its
            // click can submit the choice. Otherwise blur cancels it first.
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .capture_key_down(cx.listener(|chooser, event: &gpui::KeyDownEvent, _, cx| {
                if chooser.field.read(cx).composing() {
                    return;
                }
                let key = &event.keystroke;
                if key.modifiers.platform || key.modifiers.control || key.modifiers.alt {
                    return;
                }
                match key.key.as_str() {
                    "up" => chooser.choices.move_by(-1),
                    "down" => chooser.choices.move_by(1),
                    _ => return,
                }
                if let Some(index) = chooser.choices.index() {
                    chooser.scroll.scroll_to_item(index);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                ui::text(
                    div()
                        .h(px(t.size.pane_header))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .px(px(t.space.s12))
                        .border_b(px(t.size.hairline))
                        .border_color(c.line)
                        .text_color(c.muted),
                    &t.text.status,
                )
                .child(
                    div()
                        .text_color(c.text)
                        .font_weight(t.text.row_selected.weight())
                        .child("Attach session"),
                )
                .child("from the source tmux server"),
            )
            .child(
                div().px(px(t.space.s12)).pt(px(t.space.s12)).child(
                    ui::field_frame(true)
                        .child(ui::icon("search", t.size.icon, c.muted))
                        .child(div().flex_1().min_w_0().child(self.field.clone())),
                ),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_end()
                    .gap(px(t.space.s8))
                    .h(px(t.size.button + t.space.s16))
                    .px(px(t.space.s12))
                    .border_t(px(t.size.hairline))
                    .border_color(c.line)
                    .child(
                        ui::button("cancel-attach", "Cancel", ui::Button::Secondary).on_click(
                            cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(Choice::Cancel);
                            }),
                        ),
                    )
                    .child(
                        ui::button("confirm-attach", "Attach session", ui::Button::Primary)
                            .on_click(cx.listener(|chooser, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(name) = chooser.choices.choice().map(str::to_owned) {
                                    chooser.choose(&name, cx);
                                }
                            })),
                    ),
            )
    }
}
