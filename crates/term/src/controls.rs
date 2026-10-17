//! Keeps the engine from drawing control bytes that tmux ignores.
//!
//! Outside a sequence, tmux runs the C0 controls 0x10 to 0x1f, other than
//! escape, as controls that do nothing, and ignores DEL (`input.c`, the same
//! in tmux 3.3a to 3.8). The engine draws each of them as a cell, so without
//! the filter a pane that receives one shows a character an ordinary tmux
//! client does not.
//!
//! Inside a sequence, tmux and the engine both run or ignore these bytes, so
//! the filter drops them wherever they are. CAN and SUB are the exception:
//! they also cancel a sequence they interrupt, so the filter hands the engine
//! an escape before each instead. The engine reads that escape as the start
//! of a new sequence, which ends the interrupted one as CAN or SUB would, and
//! the CAN or SUB then cancels the escape without drawing anything.
//!
//! The filter needs no state between writes. Some differences from tmux
//! remain, in bytes programs have no reason to send. Inside a DCS string tmux
//! keeps all of these bytes as data, and inside an OSC or APC string it keeps
//! DEL; the filter drops them there, and a CAN or SUB cancels a DCS string,
//! as it would in the engine without the filter. And at each of these C0
//! controls tmux discards a partly received UTF-8 character and forgets the
//! character `REP` repeats. The engine keeps the character `REP` repeats,
//! joins a partly received character across a control it never sees, and
//! draws U+FFFD for one that a CAN or SUB cuts short.

const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;
const DEL: u8 = 0x7f;

/// Hands `bytes` to `write` in order, without the controls tmux ignores, and
/// with an escape before each CAN or SUB.
pub(crate) fn filter(mut bytes: &[u8], mut write: impl FnMut(&[u8])) {
    while let Some(index) = find(bytes) {
        if index > 0 {
            write(&bytes[..index]);
        }
        if matches!(bytes[index], CAN | SUB) {
            write(&[ESC, bytes[index]]);
        }
        bytes = &bytes[index + 1..];
    }
    if !bytes.is_empty() {
        write(bytes);
    }
}

/// Returns the position of the first control in `bytes` that the filter
/// drops or changes.
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

/// Returns whether `byte` is 0x10 to 0x1f other than escape, or DEL.
fn control(byte: u8) -> bool {
    ((byte & 0xf0) == 0x10) & (byte != ESC) | (byte == DEL)
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
        for control in (0x10..=0x1a).chain(0x1c..=0x1f).chain([0x7f]) {
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
        let screen = drawn(b"abcdefghij\x18\x7fk");
        assert_eq!(text(&screen, 1), "k");
        assert_eq!((screen.cursor().col, screen.cursor().row), (1, 1));
    }
}
