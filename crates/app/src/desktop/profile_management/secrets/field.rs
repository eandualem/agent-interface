//! A one-line field for a secret's value. It shows a dot for each character
//! and nothing else: there is no copy or cut, selection, undo, input method
//! or accessible text, so the value can't be read back from it. It leaves
//! only through [`Masked::take`]. A character removed with backspace is
//! overwritten first; text that outgrows its buffer moves to a larger one,
//! and the old one is overwritten; and the whole buffer is overwritten when
//! the field closes. Text pasted in passes through the clipboard and the
//! clipboard read's own copy, which the field doesn't hold.

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
            "backspace" => remove_last(&mut self.value),
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
    if text.chars().any(char::is_control) {
        return;
    }
    let needed = value.len() + text.len();
    if needed > value.capacity() {
        // Growing in place would free the old buffer as it is, so the text
        // moves to a larger one and the old one is overwritten.
        let mut larger = String::with_capacity(needed.max(64).next_power_of_two());
        larger.push_str(value);
        wipe(std::mem::replace(value, larger));
    }
    value.push_str(text);
}

/// Remove the last character, overwriting its bytes first.
fn remove_last(value: &mut String) {
    let Some(last) = value.chars().next_back() else {
        return;
    };
    let kept = value.len() - last.len_utf8();
    let mut bytes = zeroed_from(std::mem::take(value).into_bytes(), kept);
    bytes.truncate(kept);
    // The bytes kept end on a character boundary, so they are still text,
    // and the buffer is reused rather than copied.
    *value = String::from_utf8(bytes).unwrap_or_default();
}

/// `bytes` with every byte from `from` on set to zero, in the same buffer.
fn zeroed_from(mut bytes: Vec<u8>, from: usize) -> Vec<u8> {
    bytes[from..].fill(0);
    bytes
}

/// The buffer of `bytes`, its spare capacity included, all zero.
fn zeroed(mut bytes: Vec<u8>) -> Vec<u8> {
    let capacity = bytes.capacity();
    bytes.clear();
    bytes.resize(capacity, 0);
    bytes
}

/// Overwrite the whole buffer that held `value`, its spare capacity
/// included, before it is freed.
pub(super) fn wipe(value: String) {
    std::hint::black_box(zeroed(value.into_bytes()));
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

    #[test]
    fn backspace_overwrites_what_it_removes_in_the_same_buffer() {
        let mut value = String::with_capacity(64);
        insert(&mut value, "toké");
        let buffer = value.as_ptr();
        remove_last(&mut value);
        assert_eq!((value.as_str(), value.as_ptr()), ("tok", buffer));
        // é is two bytes; both are zero before the text is shortened.
        assert_eq!(zeroed_from(b"tok\xc3\xa9".to_vec(), 3), b"tok\0\0");
        for _ in 0..4 {
            remove_last(&mut value);
        }
        assert_eq!(value, "");
    }

    #[test]
    fn a_wipe_overwrites_the_whole_buffer() {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(b"ghp_secret");
        let buffer = bytes.as_ptr();
        let wiped = zeroed(bytes);
        assert_eq!(wiped.as_ptr(), buffer);
        assert_eq!(wiped.len(), wiped.capacity());
        assert!(wiped.len() >= 64 && wiped.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn text_grows_into_a_larger_buffer_without_reallocating_in_place() {
        let mut value = String::new();
        insert(&mut value, "ghp_");
        assert!(value.capacity() >= 64);
        let first = value.as_ptr();
        insert(&mut value, &"x".repeat(100));
        assert_eq!(value.len(), 104);
        assert_ne!(value.as_ptr(), first);
        assert!(value.capacity() >= 104);
    }
}
