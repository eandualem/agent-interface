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
//! 3.8 (`input.c`). An escape starts a rename when its next byte is `k`; C0
//! controls between the two still run, and DEL or bytes with the top bit set
//! between them are ignored, so the filter drops those. Every escape reaches
//! the engine, so it still ends whatever sequence it interrupts, as in tmux;
//! only the `k` and the name after it are dropped. The name ends at the next
//! escape, CAN or SUB, which reaches the engine too: the engine reads an
//! escape as the start of the next sequence, and a CAN or SUB as cancelling
//! the escape before the name, which draws nothing. BEL does not end a name.
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
    /// The last bytes passed were an escape and any C0 controls after it, so
    /// a `k` starts a rename.
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
            match self.state {
                State::Pass => {
                    // Most escapes start an ordinary sequence: look past
                    // those, in one scan, to the first that might not.
                    let Some(after) = find_escape(&bytes[index..]).map(|offset| index + offset + 1)
                    else {
                        break;
                    };
                    index = after;
                    self.state = State::Escape;
                }
                State::Escape => {
                    match bytes[index] {
                        b'k' => {
                            flush(&mut write, &bytes[start..index]);
                            start = index + 1;
                            self.state = State::Name;
                        }
                        ESC | 0x00..=0x17 | 0x19 | 0x1c..=0x1f => {}
                        0x7f..=0xff => {
                            flush(&mut write, &bytes[start..index]);
                            start = index + 1;
                        }
                        _ => self.state = State::Pass,
                    }
                    index += 1;
                }
                State::Name => {
                    let Some(offset) = memchr::memchr3(ESC, CAN, SUB, &bytes[index..]) else {
                        start = bytes.len();
                        break;
                    };
                    index += offset;
                    start = index;
                    self.state = if bytes[index] == ESC {
                        State::Escape
                    } else {
                        State::Pass
                    };
                    index += 1;
                }
            }
        }
        flush(&mut write, &bytes[start..]);
    }
}

/// Finds an escape that might start a rename, including a trailing escape
/// whose next byte will arrive in another write.
fn find_escape(bytes: &[u8]) -> Option<usize> {
    // Plain text needs only the fast byte search. For escape-heavy output,
    // test adjacent bytes in whole chunks so the compiler can vectorize
    // past ordinary sequences without branching at every escape.
    let mut offset = memchr::memchr(ESC, bytes)?;
    const CHUNK: usize = 32;
    let pairs = bytes.len() - 1;
    while pairs - offset >= CHUNK {
        let interesting = bytes[offset..offset + CHUNK]
            .iter()
            .zip(&bytes[offset + 1..offset + CHUNK + 1])
            .fold(0, |found, (&byte, &next)| {
                // Bit 1 records any escape; bit 0 records one that might
                // start a rename.
                let escape = u8::from(byte == ESC);
                found | (escape << 1) | (escape & u8::from(!ordinary(next)))
            });
        if interesting & 1 != 0 {
            break;
        }
        offset += CHUNK;
        // Do not scan a long plain-text tail just because the write began
        // with a colour or cursor sequence.
        if interesting == 0 {
            offset += memchr::memchr(ESC, &bytes[offset..])?;
        }
    }
    memchr::memchr_iter(ESC, &bytes[offset..])
        .map(|at| offset + at)
        .find(|&at| !bytes.get(at + 1).is_some_and(|&next| ordinary(next)))
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
    use super::Rename;
    use crate::{Color, Screen, Terminal};

    #[test]
    fn rename_sequences_cross_scan_chunks_and_write_boundaries() {
        for padding in [0, 31, 32, 63, 64] {
            let prefix = format!("{}\x1b[0m{}", "\x1b[31m".repeat(padding), "x".repeat(128));
            let input = format!("{prefix}\x1bkname\x1b\\after\x1b");
            let expected = format!("{prefix}\x1b\x1b\\after\x1b");
            for split in 0..=input.len() {
                let mut filter = Rename::default();
                let mut actual = Vec::new();
                for part in [&input.as_bytes()[..split], &input.as_bytes()[split..]] {
                    filter.filter(part, |bytes| actual.extend_from_slice(bytes));
                }
                // The final escape remains pending across writes.
                filter.filter(b"khidden\x18end", |bytes| actual.extend_from_slice(bytes));
                assert_eq!(actual, format!("{expected}\x18end").as_bytes());
            }
        }
    }

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
        let screen = drawn(b"a\x1bkn\x07a\nme\x1b[31mb\x1bkname\x18c\x1bkname\x1ad");
        assert_eq!(text(&screen, 0), "abcd");
        assert_eq!(
            screen.cell(0, 1).unwrap().1.style.foreground,
            Color::Palette(1)
        );
    }

    #[test]
    fn the_escape_before_k_still_ends_the_sequence_it_interrupts() {
        let screen = drawn(b"a\x1b]0;t\x1bkname\x18b\x1b[31\x1bkname\x1amc");
        assert_eq!(text(&screen, 0), "abmc");
        assert_eq!(
            screen.cell(0, 2).unwrap().1.style.foreground,
            Color::Default
        );
    }

    #[test]
    fn controls_between_the_escape_and_k_still_run() {
        let screen = drawn(b"a\x1b]0;t\x1b\n\x7fkname\x1b\\b");
        assert_eq!(text(&screen, 0), "a");
        assert_eq!(screen.cell(1, 1).unwrap().0, "b");
    }
}
