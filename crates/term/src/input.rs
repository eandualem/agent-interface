//! Input encoded against the destination terminal's current modes.

use crate::{Error, Terminal};
use libghostty_vt::{
    key::{Action, Encoder, Event, Key, Mods},
    mouse, paste,
    terminal::{Mode, ScrollViewport},
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

/// Local history navigation, without changing tmux copy mode.
#[derive(Clone, Copy, Debug)]
pub enum Scroll {
    /// Move by rows; negative moves toward earlier output.
    Lines(isize),
    /// Show the oldest retained output.
    Top,
    /// Return to the live screen.
    Bottom,
}

/// A pointer within a rendered pane, with device-pixel cell dimensions.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    /// Horizontal position in cell units, relative to the pane.
    pub x: f32,
    /// Vertical position in cell units, relative to the pane.
    pub y: f32,
    /// Columns at the time of the event.
    pub cols: u16,
    /// Rows at the time of the event.
    pub rows: u16,
    /// Rendered cell width in device pixels.
    pub cell_width: u32,
    /// Rendered cell height in device pixels.
    pub cell_height: u32,
    /// Shift keeps mouse gestures local to the viewer.
    pub shift: bool,
    /// Control modifier.
    pub control: bool,
    /// Alt/Option modifier.
    pub alt: bool,
    /// Identity of a continuous wheel gesture; absent for discrete wheel ticks.
    pub gesture: Option<u64>,
}

/// Mouse buttons forwarded for ordinary clicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
}

/// One phase of an application-owned pointer gesture.
#[derive(Clone, Copy, Debug)]
pub enum MouseAction {
    /// Start a button gesture in the application.
    Press(Button),
    /// Move, with the held button or none for hover tracking.
    Move(Option<Button>),
    /// End a previously accepted button gesture.
    Release(Button),
}

/// One ordered user input operation. App shortcuts are handled before this.
#[derive(Clone, Debug)]
pub enum Input {
    /// A key event to encode using the pane's current modes.
    Key(KeyPress),
    /// Clipboard text; the engine sanitizes control bytes and brackets it when enabled.
    Paste(String),
    /// Scroll this viewer through the pane's retained history.
    Scroll(Scroll),
    /// Wheel rows: negative is up. Programs with mouse reporting receive it;
    /// otherwise it navigates local history.
    Wheel(Pointer, isize),
    /// A completed click; drag selection is handled locally before this.
    Click(Pointer, Button),
    /// Pointer motion and button phases, encoded in order on the terminal worker.
    Mouse(Pointer, MouseAction),
}

impl Input {
    /// Text bytes or a conservative wire-size estimate, for bounded queues.
    pub fn size(&self) -> usize {
        match self {
            Self::Key(key) => key.key.len() + key.text.as_ref().map_or(0, String::len),
            Self::Paste(text) => text.len(),
            Self::Scroll(_) => 0,
            Self::Wheel(_, rows) => rows.unsigned_abs().saturating_mul(64),
            Self::Click(_, _) | Self::Mouse(_, _) => 64,
        }
    }
}

impl Terminal {
    /// Retain gesture ownership while replacing a terminal after capture recovery.
    pub fn inherit_pointer_gestures(&mut self, previous: &Self) {
        self.wheel_owner = previous.wheel_owner;
        self.mouse_button = previous.mouse_button;
    }

    /// Select the source server's extended-key wire format. False is xterm;
    /// true is CSI-u. This is independent of the program's negotiated level.
    pub fn set_extended_keys_csi_u(&mut self, enabled: bool) {
        self.extended_keys_csi_u = enabled;
    }

    /// Current history row, or none when following the live screen.
    pub fn scrolled_row(&self) -> Result<Option<usize>, Error> {
        let bar = self.engine.scrollbar().map_err(Error)?;
        Ok((bar.offset < bar.total.saturating_sub(bar.len)).then_some(bar.offset as usize))
    }

    /// Restore a history view after rebuilding terminal state. Rows beyond
    /// retained history are clamped by the engine.
    pub fn restore_scrolled_row(&mut self, row: usize) {
        self.engine.scroll_viewport(ScrollViewport::Row(row));
    }

    fn encode_mouse(
        &mut self,
        pointer: &Pointer,
        button: Option<mouse::Button>,
        action: mouse::Action,
        count: usize,
        release: bool,
    ) -> Result<Vec<u8>, Error> {
        if pointer.cols != self.engine.cols().map_err(Error)?
            || pointer.rows != self.engine.rows().map_err(Error)?
            || pointer.cell_width == 0
            || pointer.cell_height == 0
            || !pointer.x.is_finite()
            || !pointer.y.is_finite()
            || pointer.x < 0.0
            || pointer.y < 0.0
            || pointer.x >= f32::from(pointer.cols)
            || pointer.y >= f32::from(pointer.rows)
        {
            return Err(Error(libghostty_vt::Error::InvalidValue));
        }
        let mut encoder = mouse::Encoder::new().map_err(Error)?;
        encoder
            .set_options_from_terminal(&self.engine)
            .set_any_button_pressed(self.mouse_button.is_some())
            .set_size(mouse::EncoderSize {
                screen_width: u32::from(pointer.cols).saturating_mul(pointer.cell_width),
                screen_height: u32::from(pointer.rows).saturating_mul(pointer.cell_height),
                cell_width: pointer.cell_width,
                cell_height: pointer.cell_height,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
            });
        let mut mods = Mods::empty();
        mods.set(Mods::CTRL, pointer.control);
        mods.set(Mods::ALT, pointer.alt);
        let mut event = mouse::Event::new().map_err(Error)?;
        event
            .set_button(button)
            .set_mods(mods)
            .set_position(mouse::Position {
                x: pointer.x * pointer.cell_width as f32,
                y: pointer.y * pointer.cell_height as f32,
            });
        let mut bytes = Vec::new();
        for _ in 0..count {
            event.set_action(action);
            encoder.encode_to_vec(&event, &mut bytes).map_err(Error)?;
            if release {
                event.set_action(mouse::Action::Release);
                encoder.encode_to_vec(&event, &mut bytes).map_err(Error)?;
            }
        }
        if action == mouse::Action::Motion {
            // Applying terminal options resets libghostty's own last-cell cache.
            // Compare the encoded report instead; pixel-mode positions remain distinct.
            if bytes == self.last_mouse_motion {
                return Ok(Vec::new());
            }
            self.last_mouse_motion.clone_from(&bytes);
        } else {
            self.last_mouse_motion.clear();
        }
        Ok(bytes)
    }

    /// Encode input on this terminal's owning thread. This sends nothing and
    /// never reads another pane's modes or the system clipboard.
    pub fn encode_input(&mut self, input: &Input) -> Result<Vec<u8>, Error> {
        if matches!(input, Input::Key(_) | Input::Paste(_)) {
            self.engine.scroll_viewport(ScrollViewport::Bottom);
        }
        match input {
            Input::Wheel(pointer, rows) => {
                let local = pointer.shift
                    || self.scrolled_row()?.is_some()
                    || !self.engine.is_mouse_tracking().map_err(Error)?;
                let local = match pointer.gesture {
                    Some(id) => {
                        let local = local
                            || self
                                .wheel_owner
                                .is_some_and(|(previous, local)| previous == id && local);
                        self.wheel_owner = Some((id, local));
                        local
                    }
                    None => local,
                };
                if local {
                    self.engine.scroll_viewport(ScrollViewport::Delta(*rows));
                    return Ok(Vec::new());
                }
                let button = if *rows < 0 {
                    mouse::Button::Four
                } else {
                    mouse::Button::Five
                };
                self.encode_mouse(
                    pointer,
                    Some(button),
                    mouse::Action::Press,
                    rows.unsigned_abs(),
                    false,
                )
            }
            Input::Click(pointer, button) => {
                if pointer.shift
                    || self.scrolled_row()?.is_some()
                    || !self.engine.is_mouse_tracking().map_err(Error)?
                {
                    return Ok(Vec::new());
                }
                let button = match button {
                    Button::Left => mouse::Button::Left,
                    Button::Middle => mouse::Button::Middle,
                    Button::Right => mouse::Button::Right,
                };
                self.encode_mouse(pointer, Some(button), mouse::Action::Press, 1, true)
            }
            Input::Mouse(pointer, action) => {
                let mut pointer = *pointer;
                let (button, action) = match *action {
                    MouseAction::Press(button) => {
                        if pointer.shift
                            || self.scrolled_row()?.is_some()
                            || !self.engine.is_mouse_tracking().map_err(Error)?
                        {
                            return Ok(Vec::new());
                        }
                        self.mouse_button = Some(button);
                        (Some(button), mouse::Action::Press)
                    }
                    MouseAction::Move(button) => {
                        if button != self.mouse_button
                            || self.scrolled_row()?.is_some()
                            || (button.is_none() && pointer.shift)
                        {
                            return Ok(Vec::new());
                        }
                        (button, mouse::Action::Motion)
                    }
                    MouseAction::Release(button) => {
                        if self.mouse_button != Some(button) {
                            return Ok(Vec::new());
                        }
                        self.mouse_button = None;
                        // Release remains deliverable after a resize or entering history.
                        pointer.cols = self.engine.cols().map_err(Error)?;
                        pointer.rows = self.engine.rows().map_err(Error)?;
                        pointer.x = pointer.x.clamp(0.0, f32::from(pointer.cols) - 0.001);
                        pointer.y = pointer.y.clamp(0.0, f32::from(pointer.rows) - 0.001);
                        (Some(button), mouse::Action::Release)
                    }
                };
                let button = button.map(|button| match button {
                    Button::Left => mouse::Button::Left,
                    Button::Middle => mouse::Button::Middle,
                    Button::Right => mouse::Button::Right,
                });
                self.encode_mouse(&pointer, button, action, 1, false)
            }
            Input::Scroll(scroll) => {
                self.engine.scroll_viewport(match scroll {
                    Scroll::Lines(rows) => ScrollViewport::Delta(*rows),
                    Scroll::Top => ScrollViewport::Top,
                    Scroll::Bottom => ScrollViewport::Bottom,
                });
                Ok(Vec::new())
            }
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
                // The engine disambiguates a few legacy control keys using
                // CSI-u even without an extended protocol. Literal tmux input
                // must retain the traditional shell bytes in that case.
                if self
                    .engine
                    .kitty_keyboard_flags()
                    .map_err(Error)?
                    .is_empty()
                    && key.control
                    && !key.shift
                    && !key.alt
                    && encoded.starts_with(b"\x1b[")
                    && encoded.ends_with(b"u")
                    && let Some(ch) = character.map(|ch| ch.to_ascii_uppercase())
                {
                    let byte = match ch {
                        '@'..='_' => Some(ch as u8 & 0x1f),
                        ' ' | '2' => Some(0),
                        '?' => Some(0x7f),
                        _ => None,
                    };
                    if let Some(byte) = byte {
                        return Ok(vec![byte]);
                    }
                }
                if self.extended_keys_csi_u
                    && let Some(sequence) = encoded
                        .strip_prefix(b"\x1b[27;")
                        .and_then(|s| s.strip_suffix(b"~"))
                    && let Some(separator) = sequence.iter().position(|&byte| byte == b';')
                {
                    let (mods, code) = sequence.split_at(separator);
                    let mut csi = b"\x1b[".to_vec();
                    csi.extend_from_slice(&code[1..]);
                    csi.push(b';');
                    csi.extend_from_slice(mods);
                    csi.push(b'u');
                    return Ok(csi);
                }
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
    fn mouse_gestures_follow_tracking_modes_and_release_after_history_or_resize() {
        let mut terminal = Terminal::new(80, 24, 100).unwrap();
        for _ in 0..40 {
            terminal.write(b"history\r\n");
        }
        let pointer = Pointer {
            x: 2.5,
            y: 3.5,
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
            ..Default::default()
        };
        terminal.write(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            terminal
                .encode_input(&Input::Mouse(pointer, MouseAction::Press(Button::Left)))
                .unwrap(),
            b"\x1b[<0;3;4M"
        );
        let moved = Pointer { x: 4.5, ..pointer };
        assert!(
            terminal
                .encode_input(&Input::Mouse(moved, MouseAction::Move(Some(Button::Left))))
                .unwrap()
                .is_empty()
        );
        terminal.write(b"\x1b[?1002h");
        assert_eq!(
            terminal
                .encode_input(&Input::Mouse(moved, MouseAction::Move(Some(Button::Left))))
                .unwrap(),
            b"\x1b[<32;5;4M"
        );
        assert!(
            terminal
                .encode_input(&Input::Mouse(moved, MouseAction::Move(Some(Button::Left))))
                .unwrap()
                .is_empty()
        );
        terminal.encode_input(&Input::Scroll(Scroll::Top)).unwrap();
        // The worker keeps button ownership across capture recovery as well.
        let mut restored = Terminal::new(40, 12, 100).unwrap();
        restored.write(b"\x1b[?1002h\x1b[?1006h");
        restored.inherit_pointer_gestures(&terminal);
        assert_eq!(
            restored
                .encode_input(&Input::Mouse(moved, MouseAction::Release(Button::Left)))
                .unwrap(),
            b"\x1b[<0;5;4m"
        );
        assert_eq!(
            terminal
                .encode_input(&Input::Mouse(moved, MouseAction::Release(Button::Left)))
                .unwrap(),
            b"\x1b[<0;5;4m"
        );
        terminal
            .encode_input(&Input::Scroll(Scroll::Bottom))
            .unwrap();
        assert!(
            terminal
                .encode_input(&Input::Mouse(
                    Pointer {
                        shift: true,
                        ..pointer
                    },
                    MouseAction::Press(Button::Left)
                ))
                .unwrap()
                .is_empty()
        );
        assert!(
            terminal
                .encode_input(&Input::Mouse(moved, MouseAction::Move(Some(Button::Left))))
                .unwrap()
                .is_empty()
        );
        terminal.write(b"\x1b[?1003h");
        assert_eq!(
            terminal
                .encode_input(&Input::Mouse(
                    Pointer { x: 6.5, ..pointer },
                    MouseAction::Move(None)
                ))
                .unwrap(),
            b"\x1b[<35;7;4M"
        );
    }

    #[test]
    fn mouse_clicks_and_wheels_follow_program_modes_with_local_history_fallback() {
        let mut terminal = Terminal::new(80, 24, 100).unwrap();
        for _ in 0..40 {
            terminal.write(b"history\r\n");
        }
        let pointer = Pointer {
            x: 2.5,
            y: 3.5,
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
            ..Default::default()
        };
        assert!(
            terminal
                .encode_input(&Input::Click(pointer, Button::Left))
                .unwrap()
                .is_empty()
        );
        assert!(
            terminal
                .encode_input(&Input::Wheel(pointer, -1))
                .unwrap()
                .is_empty()
        );
        assert!(terminal.scrolled_row().unwrap().is_some());
        terminal
            .encode_input(&Input::Scroll(Scroll::Bottom))
            .unwrap();
        terminal.write(b"\x1b[?1000h\x1b[?1006h");
        let mut screen = crate::Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        assert!(screen.mouse_input());
        assert_eq!(
            terminal
                .encode_input(&Input::Click(pointer, Button::Left))
                .unwrap(),
            b"\x1b[<0;3;4M\x1b[<0;3;4m"
        );
        assert_eq!(
            terminal.encode_input(&Input::Wheel(pointer, -1)).unwrap(),
            b"\x1b[<64;3;4M"
        );
        assert!(
            terminal
                .encode_input(&Input::Wheel(
                    Pointer {
                        shift: true,
                        ..pointer
                    },
                    -1
                ))
                .unwrap()
                .is_empty()
        );
        assert!(terminal.scrolled_row().unwrap().is_some());
        terminal.read_screen(&mut screen).unwrap();
        assert!(!screen.mouse_input());
        let gesture = Pointer {
            gesture: Some(7),
            ..pointer
        };
        assert!(
            terminal
                .encode_input(&Input::Wheel(gesture, 100))
                .unwrap()
                .is_empty()
        );
        assert!(terminal.scrolled_row().unwrap().is_none());
        // Momentum reaching the live screen remains local until another gesture.
        assert!(
            terminal
                .encode_input(&Input::Wheel(gesture, 1))
                .unwrap()
                .is_empty()
        );
        let mut recovered = Terminal::new(80, 24, 100).unwrap();
        recovered.write(b"\x1b[?1000h\x1b[?1006h");
        recovered.inherit_pointer_gestures(&terminal);
        assert!(
            recovered
                .encode_input(&Input::Wheel(gesture, 1))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            recovered
                .encode_input(&Input::Wheel(
                    Pointer {
                        gesture: Some(8),
                        ..pointer
                    },
                    1
                ))
                .unwrap(),
            b"\x1b[<65;3;4M"
        );
        terminal
            .encode_input(&Input::Scroll(Scroll::Bottom))
            .unwrap();
        assert!(
            terminal
                .encode_input(&Input::Click(
                    Pointer {
                        cols: 79,
                        ..pointer
                    },
                    Button::Left
                ))
                .is_err()
        );
    }

    #[test]
    fn local_scroll_moves_the_viewport_and_input_returns_to_live_screen() {
        let mut terminal = Terminal::new(20, 3, 100).unwrap();
        terminal.write(b"first\r\nsecond\r\nthird\r\nfourth\r\nfifth");
        let mut screen = crate::Screen::default();
        terminal.encode_input(&Input::Scroll(Scroll::Top)).unwrap();
        terminal.read_screen(&mut screen).unwrap();
        assert_eq!(screen.cell(0, 0).unwrap().0, "f");
        assert_eq!(terminal.scrolled_row().unwrap(), Some(0));
        terminal
            .encode_input(&Input::Scroll(Scroll::Lines(1)))
            .unwrap();
        assert_eq!(terminal.scrolled_row().unwrap(), Some(1));
        terminal.encode_input(&Input::Paste("text".into())).unwrap();
        assert_eq!(terminal.scrolled_row().unwrap(), None);
        terminal.read_screen(&mut screen).unwrap();
        assert_eq!(screen.cell(0, 0).unwrap().0, "t");
    }

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
        for (name, byte) in [("i", 9), ("m", 13), ("[", 27)] {
            assert_eq!(
                terminal
                    .encode_input(&Input::Key(KeyPress {
                        key: name.into(),
                        control: true,
                        ..Default::default()
                    }))
                    .unwrap(),
                [byte]
            );
        }
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
        terminal.set_extended_keys_csi_u(true);
        assert_eq!(
            terminal
                .encode_input(&Input::Key(KeyPress {
                    key: "h".into(),
                    control: true,
                    shift: true,
                    ..Default::default()
                }))
                .unwrap(),
            b"\x1b[72;6u"
        );
        assert_eq!(
            terminal
                .encode_input(&Input::Key(KeyPress {
                    key: "i".into(),
                    control: true,
                    ..Default::default()
                }))
                .unwrap(),
            b"\x1b[105;5u"
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
