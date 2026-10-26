//! Output boundaries shared by the app and fidelity harness.

use agent_interface_term::{Error, Terminal};

/// Feed output until pending wrap requires a new tmux snapshot. Return the
/// number of bytes fed and whether a capture is needed; the remainder must not
/// be streamed into this terminal. Check before control bytes because a cursor
/// edit can clear pending wrap inside a single output event.
pub(crate) fn write_until_wrap(
    terminal: &mut Terminal,
    bytes: &[u8],
) -> Result<(usize, bool), Error> {
    let mut fed = 0;
    while fed < bytes.len() {
        let remaining = &bytes[fed..];
        let end = remaining[1..]
            .iter()
            .position(|byte| *byte < 0x20 || *byte == 0x7f)
            .map_or(remaining.len(), |index| index + 1);
        terminal.write(&remaining[..end]);
        fed += end;
        if terminal.cursor_pending_wrap()? {
            return Ok((fed, true));
        }
    }
    Ok((fed, false))
}
