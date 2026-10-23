//! Commands a control-mode client writes to tmux.

use std::fmt::Write;

use crate::control::PaneId;

/// Returns the command that types `bytes` into `pane`, with its line end,
/// ready to write to a control-mode client's standard input.
///
/// `send-keys -H` delivers each byte as given to that exact pane, without
/// tmux's key tables (`docs/ARCHITECTURE.md`).
pub fn send_bytes(pane: PaneId, bytes: &[u8]) -> String {
    append_bytes(format!("send-keys -H -t %{}", pane.0), bytes)
}

/// Types only while the pane belongs to this window and source session.
pub fn send_bytes_in_session(session: u32, window: u32, pane: PaneId, bytes: &[u8]) -> String {
    append_bytes(
        format!("send-keys -H -t '${session}:@{window}.%{}'", pane.0),
        bytes,
    )
}

fn append_bytes(mut command: String, bytes: &[u8]) -> String {
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(command, " {byte:02x}");
    }
    command.push('\n');
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_every_byte_in_hexadecimal_to_the_exact_pane() {
        assert_eq!(
            send_bytes(PaneId(3), b"a\r\x1b\xff"),
            "send-keys -H -t %3 61 0d 1b ff\n"
        );
    }
}
