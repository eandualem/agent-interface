//! Keeps the engine from drawing C0 controls that tmux ignores.
//!
//! Outside a sequence, tmux runs the C0 controls 0x10 to 0x1f, other than
//! escape, as controls that do nothing (`input.c`, the same in tmux 3.3a to
//! 3.8). The engine draws each of them as a cell, so without the filter a
//! pane that receives one shows a character an ordinary tmux client does
//! not.
//!
//! The filter hands the engine NUL in place of each. The engine's parser
//! treats NUL as it treats these controls in every state, but never draws
//! it. Like the control it replaces, NUL ends a partly received UTF-8
//! character, which the engine then draws as U+FFFD, as tmux does from 3.6;
//! earlier versions draw nothing for it. CAN and SUB also cancel a sequence
//! they interrupt, so the filter hands the engine an escape before each
//! instead. The engine reads that escape as the start of a new sequence,
//! which ends the interrupted one as CAN or SUB would, and the CAN or SUB
//! then cancels the escape without drawing anything.
//!
//! The filter needs no state between writes. Two differences from tmux
//! remain, in bytes programs have no reason to send. Inside a DCS string
//! tmux keeps these controls as data, CAN and SUB included, while the engine
//! gets NUL in their place and ends the string at a CAN or SUB. And at each
//! of them tmux forgets the character `REP` repeats, while the engine keeps
//! it. DEL, which the engine also draws and tmux ignores outside strings, is
//! left alone: it is part of an OSC or APC string for both, so only a filter
//! that follows their parsers' state could drop it.

const NUL: u8 = 0x00;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;

/// Hands `bytes` to `write` in order, with NUL in place of each control tmux
/// ignores, and an escape before each CAN or SUB.
pub(crate) fn filter(mut bytes: &[u8], mut write: impl FnMut(&[u8])) {
    while let Some(index) = find(bytes) {
        if index > 0 {
            write(&bytes[..index]);
        }
        match bytes[index] {
            CAN | SUB => write(&[ESC, bytes[index]]),
            _ => write(&[NUL]),
        }
        bytes = &bytes[index + 1..];
    }
    if !bytes.is_empty() {
        write(bytes);
    }
}

/// Returns the position of the first control in `bytes` that the filter
/// replaces or changes.
fn find(bytes: &[u8]) -> Option<usize> {
    // Testing whole chunks first, without branches or an early exit, lets the
    // compiler test many bytes at once.
    const CHUNK: usize = 32;
    let mut offset = 0;
    for chunk in bytes.as_chunks::<CHUNK>().0 {
        if chunk
            .iter()
            .fold(0, |found, &byte| found | u8::from(control(byte)))
            != 0
        {
            break;
        }
        offset += CHUNK;
    }
    bytes[offset..]
        .iter()
        .position(|&byte| control(byte))
        .map(|index| offset + index)
}

/// Returns whether `byte` is 0x10 to 0x1f other than escape.
fn control(byte: u8) -> bool {
    ((byte & 0xf0) == 0x10) & (byte != ESC)
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
    fn controls_tmux_ignores_are_not_drawn() {
        for control in (0x10..=0x1a).chain(0x1c..=0x1f) {
            let screen = drawn(&[b'a', control, b'b']);
            assert_eq!(text(&screen, 0), "ab", "{control:#04x}");
            assert_eq!((screen.cursor().col, screen.cursor().row), (2, 0));
        }
    }

    #[test]
    fn a_control_inside_a_sequence_leaves_it_whole() {
        let screen = drawn(b"a\x1b\x10[3\x101mb");
        assert_eq!(text(&screen, 0), "ab");
        assert_eq!(
            screen.cell(0, 1).unwrap().1.style.foreground,
            Color::Palette(1)
        );
    }

    #[test]
    fn cancel_and_substitute_still_end_a_sequence() {
        let screen = drawn(b"a\x1b[3\x181mb\x1b[3\x1a1mc");
        assert_eq!(text(&screen, 0), "a1mb1mc");
        assert_eq!(
            screen.cell(0, 3).unwrap().1.style.foreground,
            Color::Default
        );
    }

    #[test]
    fn a_control_keeps_a_pending_wrap() {
        let screen = drawn(b"abcdefghij\x18\x10k");
        assert_eq!(text(&screen, 1), "k");
        assert_eq!((screen.cursor().col, screen.cursor().row), (1, 1));
    }

    #[test]
    fn a_control_ends_a_partly_received_character() {
        let screen = drawn(b"a\xe2\x82\x10\xacb\xe2\x82\x18c");
        assert_eq!(text(&screen, 0), "a\u{fffd}\u{fffd}b\u{fffd}c");
    }
}
