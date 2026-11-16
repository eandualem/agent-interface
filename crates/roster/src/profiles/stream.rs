//! The Rust service's push stream, read as a signal that agent states changed.

use std::time::Duration;

use super::{Connection, Error};
use crate::transport::local;

/// The service writes a comment every 15 seconds while nothing else happens,
/// so three missed ones mean a broken stream.
const QUIET: Duration = Duration::from_secs(45);
/// Far above any frame of the contract.
const LINE_LIMIT: usize = 64 * 1024;

impl Connection {
    /// Follow this profile's push stream until `live` is false, calling
    /// `changed` whenever agent states should be read again: when the stream
    /// opens, when an agent's state changes and when the service says events
    /// were missed. The events' own values are not used, so no cursor is kept:
    /// a stream that is opened again reads the states again.
    ///
    /// Only the Rust service has the stream; any other connection is
    /// `Unsupported`. A stream that ends while `live` is an error. Blocks its
    /// caller, which is an I/O worker of its own.
    pub fn watch(&self, live: &dyn Fn() -> bool, changed: &mut dyn FnMut()) -> Result<(), Error> {
        if !self.served {
            return Err(Error::Unsupported);
        }
        let mut frames = Frames::default();
        local::stream(
            self.socket.to_str().ok_or(Error::Invalid)?,
            "/api/stream?kinds=agent.state,resync",
            &self.identity.profile_id,
            QUIET,
            live,
            &mut |bytes| frames.push(bytes, changed),
        )
        .map_err(|status| match status {
            Some(404) => Error::Unsupported,
            Some(status) => Error::Refused(status),
            None => Error::Unavailable,
        })
    }
}

/// Server-Sent Events, read only as far as each frame's kind.
#[derive(Default)]
struct Frames {
    line: Vec<u8>,
    /// Whether the frame being read is one that changes agent states.
    relevant: bool,
}

impl Frames {
    /// Read more of the stream. False when a line outgrows the limit.
    fn push(&mut self, bytes: &[u8], changed: &mut dyn FnMut()) -> bool {
        for &byte in bytes {
            if byte != b'\n' {
                self.line.push(byte);
                if self.line.len() > LINE_LIMIT {
                    return false;
                }
                continue;
            }
            let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
            if line.is_empty() {
                // A blank line ends the frame.
                if std::mem::take(&mut self.relevant) {
                    changed();
                }
            } else if let Some(kind) = line.strip_prefix(b"event:") {
                self.relevant = matches!(kind.trim_ascii(), b"hello" | b"agent.state" | b"resync");
            }
            self.line.clear();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_signal_state_changes_across_chunks_and_skip_the_rest() {
        let mut frames = Frames::default();
        let mut count = 0;
        let mut push = |bytes: &[u8]| frames.push(bytes, &mut || count += 1);
        assert!(push(
            b"event: hello\ndata: {\"instance_id\":\"a\",\"cursor\":\"a:1\"}\n\n"
        ));
        assert!(push(b": heartbeat\n\nid: a:2\r\nevent: agent.st"));
        assert!(push(b"ate\r\ndata: {}\r\n"));
        assert!(push(b"\r\nid: a:3\nevent: join.progress\ndata: {}\n\n"));
        assert!(push(b"id: a:0\nevent:resync\ndata: {}\n\n"));
        assert!(!push(&vec![b'x'; LINE_LIMIT + 1]));
        assert_eq!(count, 3);
    }
}
