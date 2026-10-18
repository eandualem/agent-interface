//! Removes DEL only in tmux's ground state, where it has no action. Unlike
//! a C0 control, DEL does not interrupt a partly received UTF-8 character.
//! In strings it is payload data and must reach the engine unchanged.
//!
//! These transitions follow tmux 3.3a–3.8's `input.c`. Only the distinction
//! between ground and sequences matters; CSI parameter states can share
//! one state because they all finish at the same bytes. DCS needs more:
//! cancellation ends its header, but in its data only `ESC \` ends it.

const ESC: u8 = 0x1b;
const DEL: u8 = 0x7f;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Intermediate,
    Csi,
    Osc,
    String,
    DcsEntry,
    DcsParameter,
    DcsIntermediate,
    DcsIgnore,
    DcsData,
    DcsEscape,
}

/// Tracks sequence boundaries across writes without buffering payloads.
#[derive(Debug, Default)]
pub(crate) struct Del {
    state: State,
}

impl Del {
    /// Passes every byte through except DEL in ground state.
    pub(crate) fn filter(&mut self, bytes: &[u8], mut write: impl FnMut(&[u8])) {
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            if self.state == State::Ground {
                // Ordinary output needs only a vectorized scan, not a
                // parser transition for every text or UTF-8 byte.
                let Some(offset) = memchr::memchr2(ESC, DEL, &bytes[index..]) else {
                    break;
                };
                index += offset;
                if bytes[index] == DEL {
                    if start < index {
                        write(&bytes[start..index]);
                    }
                    index += 1;
                    start = index;
                    continue;
                }
            }
            if self.state == State::Csi {
                // SGR-heavy output spends most of its sequence bytes here.
                // Skip parameters without dispatching every parser state.
                let Some(offset) = bytes[index..]
                    .iter()
                    .position(|&byte| matches!(byte, ESC | 0x18 | 0x1a | 0x40..=0x7e))
                else {
                    break;
                };
                index += offset;
            }
            self.state = self.state.next(bytes[index]);
            index += 1;
        }
        if start < bytes.len() {
            write(&bytes[start..]);
        }
    }
}

impl State {
    fn next(self, byte: u8) -> Self {
        use State::*;

        // tmux's DCS data has no "anywhere" transitions. Even CAN/SUB
        // and an escape followed by anything but backslash are payload.
        if self == DcsData {
            return if byte == ESC { DcsEscape } else { DcsData };
        }
        if self == DcsEscape {
            return if byte == b'\\' { Ground } else { DcsData };
        }
        match byte {
            ESC => return Escape,
            0x18 | 0x1a => return Ground,
            _ => {}
        }
        match (self, byte) {
            (Escape, b'[') => Csi,
            (Escape, b']') => Osc,
            (Escape, b'P') => DcsEntry,
            (Escape, b'X' | b'^' | b'_' | b'k') => String,
            (Escape, 0x20..=0x2f) => Intermediate,
            (Escape | Intermediate, 0x30..=0x7e) | (Csi, 0x40..=0x7e) | (Osc, 7) => Ground,
            (DcsEntry | DcsParameter, 0x20..=0x2f) => DcsIntermediate,
            (DcsEntry, 0x30..=0x39 | 0x3b..=0x3f) => DcsParameter,
            (DcsEntry | DcsParameter, b':')
            | (DcsParameter, 0x3c..=0x3f)
            | (DcsIntermediate, 0x30..=0x3f) => DcsIgnore,
            (DcsEntry | DcsParameter | DcsIntermediate, 0x40..=0x7e) => DcsData,
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &[u8], expected: &[u8]) {
        // Every possible two-write boundary, including the unsplit input.
        for split in 0..=input.len() {
            let mut filter = Del::default();
            let mut actual = Vec::new();
            filter.filter(&input[..split], |part| actual.extend_from_slice(part));
            filter.filter(&input[split..], |part| actual.extend_from_slice(part));
            assert_eq!(actual, expected, "split at {split}");
        }
    }

    #[test]
    fn only_ground_del_is_removed_between_sequences_and_strings() {
        check(
            b"a\x7f\x1b[3\x7f1mb\x7f\x1b]4;1;#00\x7fff00\x07\x7f\x1b_abc\x07\x7f\x1b\\\x7fc",
            b"a\x1b[3\x7f1mb\x1b]4;1;#00\x7fff00\x07\x1b_abc\x07\x7f\x1b\\c",
        );
        // An intermediate prevents '[' or ']' from starting CSI or OSC.
        check(b"\x1b(]\x7fa\x1b]x\x18\x7fb", b"\x1b(]a\x1b]x\x18b");
    }

    #[test]
    fn dcs_keeps_del_in_data_but_cancellation_can_end_its_header() {
        check(
            b"\x1bPq\x18\x7f\x1bx\x7f\x1b\\\x7fa",
            b"\x1bPq\x18\x7f\x1bx\x7f\x1b\\a",
        );
        check(
            b"\x1bP1\x18\x7fa\x1bP:q\x1a\x7fb\x1bP1?q\x18\x7fc",
            b"\x1bP1\x18a\x1bP:q\x1ab\x1bP1?q\x18c",
        );
    }
}
