//! Native selection of an existing session on one captured source server.

use std::path::PathBuf;

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, ScrollHandle, SharedString, Subscription, Task, Window, div,
    prelude::*, rgb,
};

use super::name::{NameEvent, NameField};
use crate::sessions::SessionSource;

pub(super) enum Choice {
    Session(String),
    Cancel,
}

pub(super) struct SessionChooser {
    field: Entity<NameField>,
    names: Vec<String>,
    query: String,
    selected: usize,
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
                    chooser.query = query.to_lowercase();
                    chooser.selected = 0;
                    chooser.scroll.scroll_to_item(0);
                }
                NameEvent::Save => {
                    if let Some(name) =
                        matches(&chooser.names, &chooser.query).get(chooser.selected)
                    {
                        chooser.choose(name, cx);
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
            names: Vec::new(),
            query: String::new(),
            selected: 0,
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

    pub fn focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.field.read(cx).focus.is_focused(window)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(observation) = self.source.as_ref().and_then(SessionSource::take_update) {
            let selected = matches(&self.names, &self.query)
                .get(self.selected)
                .cloned();
            self.names = observation.sessions;
            self.names.sort_by_key(|name| name.to_lowercase());
            self.error = observation.error;
            self.loading = false;
            self.selected = selected
                .and_then(|name| {
                    matches(&self.names, &self.query)
                        .iter()
                        .position(|item| *item == name)
                })
                .unwrap_or(0);
            self.scroll.scroll_to_item(self.selected);
            cx.notify();
        }
    }

    fn choose(&mut self, name: &str, cx: &mut Context<Self>) {
        self.refresh(cx);
        // A click may come from an older frame. Failed discovery must not leave
        // a selectable row that claims the old source observation is current.
        if self.error.is_none() && self.names.iter().any(|item| item == name) {
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

fn matches(names: &[String], query: &str) -> Vec<String> {
    names
        .iter()
        .filter(|name| name.to_lowercase().contains(query))
        .cloned()
        .collect()
}

impl Render for SessionChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let names = matches(&self.names, &self.query);
        let status = if let Some(error) = &self.error {
            error.clone()
        } else if self.loading {
            "Loading sessions…".into()
        } else if names.is_empty() {
            "No matching sessions".into()
        } else {
            "Type to filter · ↑↓ choose · Enter attach · Escape cancel".into()
        };
        let mut rows = div()
            .id("session-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for (index, name) in names.into_iter().enumerate() {
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("session-{name}")))
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .when(index == self.selected, |el| el.bg(rgb(0x2a3c58)))
                    .hover(|el| el.bg(rgb(0x25334a)))
                    .child(name.clone())
                    .on_click(cx.listener(move |chooser, _, _, cx| {
                        cx.stop_propagation();
                        chooser.choose(&name, cx);
                    })),
            );
        }
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .bg(rgb(0x161e2b))
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
                let count = matches(&chooser.names, &chooser.query).len();
                match key.key.as_str() {
                    "up" => chooser.selected = chooser.selected.saturating_sub(1),
                    "down" => {
                        chooser.selected = (chooser.selected + 1).min(count.saturating_sub(1))
                    }
                    _ => return,
                }
                chooser.scroll.scroll_to_item(chooser.selected);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child("Attach session")
                    .child(
                        div()
                            .id("cancel-attach")
                            .cursor_pointer()
                            .child("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(Choice::Cancel);
                            })),
                    ),
            )
            .child(self.field.clone())
            .child(div().text_xs().child(status))
            .child(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_keeps_exact_targets_with_spaces_unicode_and_long_names() {
        let names = vec!["My worker".into(), "Éditeur".into(), "x".repeat(100)];
        assert_eq!(matches(&names, "worker"), ["My worker"]);
        assert_eq!(matches(&names, &"ÉDI".to_lowercase()), ["Éditeur"]);
        assert_eq!(matches(&names, &"x".repeat(90)), ["x".repeat(100)]);
        assert!(matches(&names, "absent").is_empty());
    }
}
