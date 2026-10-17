//! Drops GNU screen's window-rename sequence, `ESC k name ESC \`, before the
//! engine sees it.
//!
//! tmux reads the sequence as a window name and never draws it, whether or
//! not its `allow-rename` option lets the name through. The engine knows no
//! such sequence and would print the name as text. Shells set this name when
//! `TERM` is `screen` or `tmux`, so without the filter their panes show stray
//! names.
//!
//! The filter follows tmux's input parser, which is the same in tmux 3.3a to
//! 3.8 (`input.c`). An escape starts a rename when the next byte is `k`. C0
//! controls between the two still run, and DEL or bytes with the top bit set
//! between them are ignored. Everything after the `k` is dropped up to the
//! next escape, CAN or SUB. BEL does not end a rename. A CAN or SUB that
//! ends it is a cancel with no effect, so it is dropped too. The escape that
//! ends it is read afresh, so an escape sequence right after the name still
//! applies.
//!
//! Two differences from tmux remain. tmux reads `ESC k` inside a DCS
//! string's data as data, while the filter starts a rename there; the engine
//! itself ends a DCS string at any escape, so it would have printed what
//! follows. And from tmux 3.6, a rename left open for five seconds ends by
//! itself, which a filter of bytes cannot follow.

const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;

/// Where the filter is between writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Bytes pass through.
    #[default]
    Pass,
    /// An escape is held back until its next byte says whether it starts a
    /// rename.
    Escape,
    /// Bytes are a name and are dropped.
    Name,
}

/// Removes rename sequences from pane output, also when one arrives split
/// across writes.
#[derive(Debug, Default)]
pub(crate) struct Rename {
    state: State,
}

impl Rename {
    /// Hands `bytes`, without rename sequences, to `write` in order.
    pub(crate) fn filter(&mut self, bytes: &[u8], mut write: impl FnMut(&[u8])) {
        // The first byte not yet written or dropped.
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            match self.state {
                State::Pass => {
                    let Some(offset) = memchr::memchr(ESC, &bytes[index..]) else {
                        break;
                    };
                    index += offset;
                    // Most escapes start an ordinary sequence: pass them on
                    // without splitting the write.
                    if bytes.get(index + 1).is_some_and(|&next| ordinary(next)) {
                        index += 2;
                        continue;
                    }
                    flush(&mut write, &bytes[start..index]);
                    start = index + 1;
                    self.state = State::Escape;
                }
                State::Escape => match byte {
                    b'k' => {
                        start = index + 1;
                        self.state = State::Name;
                    }
                    ESC => {
                        write(&[ESC]);
                        start = index + 1;
                    }
                    0x00..=0x17 | 0x19 | 0x1c..=0x1f => {
                        write(&[byte]);
                        start = index + 1;
                    }
                    0x7f..=0xff => start = index + 1,
                    _ => {
                        write(&[ESC]);
                        start = index;
                        self.state = State::Pass;
                    }
                },
                State::Name => match byte {
                    ESC => {
                        start = index + 1;
                        self.state = State::Escape;
                    }
                    CAN | SUB => {
                        start = index + 1;
                        self.state = State::Pass;
                    }
                    _ => start = index + 1,
                },
            }
            index += 1;
        }
        if self.state == State::Pass {
            flush(&mut write, &bytes[start..]);
        }
    }
}

/// Returns whether an escape followed by `next` is neither a rename nor an
/// escape tmux keeps waiting on.
fn ordinary(next: u8) -> bool {
    matches!(next, CAN | SUB) || ((0x20..=0x7e).contains(&next) && next != b'k')
}

fn flush(write: &mut impl FnMut(&[u8]), bytes: &[u8]) {
    if !bytes.is_empty() {
        write(bytes);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Color, Screen, Terminal};

    /// Writes `bytes` to one terminal whole and to another a byte at a time,
    /// checks both draw the same screen, and returns it.
    fn drawn(bytes: &[u8]) -> Screen {
        let mut screens = [Screen::default(), Screen::default()];
        let mut whole = Terminal::new(10, 2, 0).unwrap();
        whole.write(bytes);
        whole.read_screen(&mut screens[0]).unwrap();
        let mut split = Terminal::new(10, 2, 0).unwrap();
        for byte in bytes {
            split.write(std::slice::from_ref(byte));
        }
        split.read_screen(&mut screens[1]).unwrap();
        let [screen, other] = screens;
        assert_eq!(screen, other, "{bytes:?}");
        screen
    }

    fn text(screen: &Screen, row: u16) -> String {
        (0..screen.cols())
            .map(|col| screen.cell(row, col).unwrap().0)
            .collect()
    }

    #[test]
    fn a_rename_is_not_drawn() {
        let screen = drawn(b"a\x1bkname\x1b\\b");
        assert_eq!(text(&screen, 0), "ab");
        assert_eq!((screen.cursor().col, screen.cursor().row), (2, 0));
    }

    #[test]
    fn a_rename_ends_at_the_next_escape_cancel_or_substitute() {
        let screen = drawn(b"a\x1bkn\x07a\nme\x1b[31mb\x1bkname\x18c");
        assert_eq!(text(&screen, 0), "abc");
        assert_eq!(
            screen.cell(0, 1).unwrap().1.style.foreground,
            Color::Palette(1)
        );
    }

    #[test]
    fn controls_between_the_escape_and_k_still_run() {
        let screen = drawn(b"a\x1b\n\x7fkname\x1b\\b");
        assert_eq!(text(&screen, 0), "a");
        assert_eq!(screen.cell(1, 1).unwrap().0, "b");
    }
}
