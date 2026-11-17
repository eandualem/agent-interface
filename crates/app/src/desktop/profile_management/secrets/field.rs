//! A one-line field for a secret's value. It shows a dot for each character
//! and nothing else: there is no copy or cut, selection, undo, input method
//! or accessible text, so the value can't be read back from it. It leaves
//! only through [`Masked::take`], and is wiped when the field closes.

use super::ui;
use crate::desktop::design::colors;
use gpui::{
    Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton, Window, div, prelude::*,
};

/// At most this many dots are drawn, whatever the value's length.
const DOTS: usize = 32;

pub(super) enum Event {
    Save,
    Cancel,
}

pub(super) struct Masked {
    value: String,
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Masked {}

impl Masked {
    /// An empty field that takes the keyboard focus.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            value: String::new(),
            focus,
        }
    }

    /// Whether the service would take what was typed.
    pub fn takes(&self) -> bool {
        agent_interface_roster::profiles::secrets::is_value(&self.value)
    }

    /// The value, leaving the field empty.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.value)
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let mods = key.modifiers;
        let command = if cfg!(target_os = "macos") {
            mods.platform
        } else {
            mods.control
        };
        match key.key.as_str() {
            "escape" => cx.emit(Event::Cancel),
            "enter" => cx.emit(Event::Save),
            "backspace" => {
                self.value.pop();
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    // A copied token often ends with a line break.
                    insert(&mut self.value, text.trim());
                }
            }
            _ if command || mods.control => return,
            _ => match &key.key_char {
                Some(typed) => insert(&mut self.value, typed),
                None => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Add typed or pasted `text` to `value`. A control character is never part
/// of a value, so text with one inside is left out.
fn insert(value: &mut String, text: &str) {
    if !text.chars().any(char::is_control) {
        value.push_str(text);
    }
}

/// Overwrite `value`'s bytes before it is freed.
pub(super) fn wipe(value: String) {
    let mut bytes = value.into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

impl Drop for Masked {
    fn drop(&mut self) {
        wipe(self.take());
    }
}

impl Render for Masked {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let shown = self.value.chars().count();
        ui::setup_field(self.focus.is_focused(window))
            .id("secret-value")
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|field, _, window, cx| {
                    window.focus(&field.focus, cx);
                    cx.stop_propagation();
                }),
            )
            .child(if shown == 0 {
                div().text_color(c.faint).child("Value")
            } else {
                div()
                    .text_color(c.text)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child("•".repeat(shown.min(DOTS)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_with_a_control_character_is_never_typed_in() {
        let mut value = String::new();
        for typed in ["ghp_", "a\nb", "\t", "x9"] {
            insert(&mut value, typed);
        }
        assert_eq!(value, "ghp_x9");
    }
}
