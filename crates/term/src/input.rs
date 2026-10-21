//! Input encoded against the destination terminal's current modes.

use crate::{Error, Terminal};
use libghostty_vt::{
    key::{Action, Encoder, Event, Key, Mods},
    paste,
    terminal::Mode,
};

/// A keyboard event independent of the native UI framework.
#[derive(Clone, Debug, Default)]
pub struct KeyPress {
    /// Logical character or normalized name: enter, backspace, tab, escape,
    /// up/down/left/right, home/end, pageup/pagedown, delete/insert, f1 through f25.
    pub key: String,
    /// Committed text before Control transformations, when supplied by the OS.
    pub text: Option<String>,
    /// Shift modifier.
    pub shift: bool,
    /// Control modifier.
    pub control: bool,
    /// Alt/Option modifier.
    pub alt: bool,
    /// A held key is repeating.
    pub repeat: bool,
}

/// One ordered user input operation. App shortcuts are handled before this.
#[derive(Clone, Debug)]
pub enum Input {
    /// A key event to encode using the pane's current modes.
    Key(KeyPress),
    /// Clipboard text; the engine sanitizes control bytes and brackets it when enabled.
    Paste(String),
}

impl Input {
    /// Owned text bytes, for bounded queue accounting.
    pub fn size(&self) -> usize {
        match self {
            Self::Key(key) => key.key.len() + key.text.as_ref().map_or(0, String::len),
            Self::Paste(text) => text.len(),
        }
    }
}

impl Terminal {
    /// Encode input on this terminal's owning thread. This sends nothing and
    /// never reads another pane's modes or the system clipboard.
    pub fn encode_input(&self, input: &Input) -> Result<Vec<u8>, Error> {
        match input {
            Input::Key(key) => {
                let mut encoder = Encoder::new().map_err(Error)?;
                encoder.set_options_from_terminal(&self.engine);
                let mut event = Event::new().map_err(Error)?;
                let mut mods = Mods::empty();
                mods.set(Mods::SHIFT, key.shift);
                mods.set(Mods::CTRL, key.control);
                mods.set(Mods::ALT, key.alt);
                event
                    .set_action(if key.repeat {
                        Action::Repeat
                    } else {
                        Action::Press
                    })
                    .set_key(logical_key(&key.key))
                    .set_mods(mods);
                let character = if key.key == "space" {
                    Some(' ')
                } else {
                    single(&key.key)
                };
                if let Some(character) = character {
                    event.set_unshifted_codepoint(character);
                }
                let text = key
                    .text
                    .clone()
                    .or_else(|| {
                        character.map(|ch| {
                            if key.shift {
                                ch.to_uppercase().collect()
                            } else {
                                ch.to_string()
                            }
                        })
                    })
                    .filter(|text| {
                        !text
                            .chars()
                            .any(|ch| ch.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&ch))
                    });
                event.set_utf8(text);
                let mut encoded = Vec::with_capacity(32);
                encoder.encode_to_vec(&event, &mut encoded).map_err(Error)?;
                Ok(encoded)
            }
            Input::Paste(text) => {
                let bracketed = self.engine.mode(Mode::BRACKETED_PASTE).map_err(Error)?;
                let mut bytes = text.as_bytes().to_vec();
                let mut encoded = vec![0; bytes.len().saturating_add(12)];
                let length = paste::encode(&mut bytes, bracketed, &mut encoded).map_err(Error)?;
                encoded.truncate(length);
                Ok(encoded)
            }
        }
    }
}

fn single(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

fn logical_key(name: &str) -> Key {
    match name {
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "space" | " " => Key::Space,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "delete" => Key::Delete,
        "insert" => Key::Insert,
        "`" => Key::Backquote,
        "\\" => Key::Backslash,
        "[" => Key::BracketLeft,
        "]" => Key::BracketRight,
        "," => Key::Comma,
        "." => Key::Period,
        "=" => Key::Equal,
        "-" => Key::Minus,
        "'" => Key::Quote,
        ";" => Key::Semicolon,
        "/" => Key::Slash,
        _ => {
            if let Some(ch) = single(name) {
                let ch = ch.to_ascii_lowercase();
                if ch.is_ascii_lowercase() {
                    return [
                        Key::A,
                        Key::B,
                        Key::C,
                        Key::D,
                        Key::E,
                        Key::F,
                        Key::G,
                        Key::H,
                        Key::I,
                        Key::J,
                        Key::K,
                        Key::L,
                        Key::M,
                        Key::N,
                        Key::O,
                        Key::P,
                        Key::Q,
                        Key::R,
                        Key::S,
                        Key::T,
                        Key::U,
                        Key::V,
                        Key::W,
                        Key::X,
                        Key::Y,
                        Key::Z,
                    ][ch as usize - 'a' as usize];
                }
                if ch.is_ascii_digit() {
                    return [
                        Key::Digit0,
                        Key::Digit1,
                        Key::Digit2,
                        Key::Digit3,
                        Key::Digit4,
                        Key::Digit5,
                        Key::Digit6,
                        Key::Digit7,
                        Key::Digit8,
                        Key::Digit9,
                    ][ch as usize - '0' as usize];
                }
            }
            if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok())
                && (1..=25).contains(&number)
            {
                return [
                    Key::F1,
                    Key::F2,
                    Key::F3,
                    Key::F4,
                    Key::F5,
                    Key::F6,
                    Key::F7,
                    Key::F8,
                    Key::F9,
                    Key::F10,
                    Key::F11,
                    Key::F12,
                    Key::F13,
                    Key::F14,
                    Key::F15,
                    Key::F16,
                    Key::F17,
                    Key::F18,
                    Key::F19,
                    Key::F20,
                    Key::F21,
                    Key::F22,
                    Key::F23,
                    Key::F24,
                    Key::F25,
                ][number - 1];
            }
            Key::Unidentified
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_paste_follow_the_destination_modes_and_sanitize_paste() {
        let mut terminal = Terminal::new(80, 24, 0).unwrap();
        let key = |name: &str| {
            Input::Key(KeyPress {
                key: name.into(),
                ..Default::default()
            })
        };
        assert_eq!(terminal.encode_input(&key("up")).unwrap(), b"\x1b[A");
        terminal.write(b"\x1b[?1h");
        assert_eq!(terminal.encode_input(&key("up")).unwrap(), b"\x1bOA");
        assert_eq!(
            terminal
                .encode_input(&Input::Key(KeyPress {
                    key: "c".into(),
                    control: true,
                    ..Default::default()
                }))
                .unwrap(),
            b"\x03"
        );
        assert_eq!(
            terminal
                .encode_input(&Input::Paste("one\ntwo".into()))
                .unwrap(),
            b"one\rtwo"
        );
        terminal.write(b"\x1b[?2004h");
        assert_eq!(
            terminal
                .encode_input(&Input::Paste("one\ntwo\x1b[201~\0".into()))
                .unwrap(),
            b"\x1b[200~one\ntwo [201~ \x1b[201~"
        );
        terminal.write(b"\x1b[>4;2m");
        assert_eq!(
            terminal
                .encode_input(&Input::Key(KeyPress {
                    key: "h".into(),
                    control: true,
                    shift: true,
                    ..Default::default()
                }))
                .unwrap(),
            b"\x1b[27;6;72~"
        );
        terminal.write(b"\x1b[>4;0m\x1b[?1l\x1b[?2004l");
        assert_eq!(terminal.encode_input(&key("up")).unwrap(), b"\x1b[A");
        assert_eq!(
            terminal
                .encode_input(&Input::Paste("plain".into()))
                .unwrap(),
            b"plain"
        );
    }
}
