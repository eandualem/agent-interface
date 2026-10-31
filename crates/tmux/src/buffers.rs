//! Capture buffers: reading a pane's captured text outside the control
//! connection.
//!
//! tmux writes a capture's text into the control connection unchanged, and a
//! line of it can match the line that ends tmux's reply. Recovery therefore
//! captures into named tmux buffers inside its command line, and a separate
//! tmux client reads and deletes them here (`docs/ARCHITECTURE.md`). Only the
//! exact names one line created are read or deleted, never any other buffer.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The longest any one tmux command here may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Room for tmux's error messages on top of the buffers' size.
const STDERR_LIMIT: usize = 64 * 1024;

/// Returns a prefix for the names of one line's buffers that no other line,
/// running instance or earlier instance with the same process ID uses.
pub fn prefix() -> String {
    static RANDOM: OnceLock<u64> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let random = *RANDOM.get_or_init(|| RandomState::new().hash_one(std::process::id()));
    format!(
        "agent-interface-{}-{random:016x}-{}-",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Reads the buffers named `names` on the server at `socket`, in order, then
/// deletes them, by `deadline`. A name with no buffer reads as `None`: tmux
/// creates none for an empty capture.
pub fn take(
    socket: &Path,
    names: &[String],
    deadline: Instant,
) -> io::Result<Vec<Option<Vec<u8>>>> {
    let sizes = sizes(socket, names, left(deadline)?)?;
    let mut contents = Vec::with_capacity(names.len());
    for (name, size) in names.iter().zip(&sizes) {
        let Some(size) = *size else {
            contents.push(None);
            continue;
        };
        match left(deadline).and_then(|timeout| read(socket, name, size, timeout)) {
            Ok(content) => contents.push(Some(content)),
            Err(error) => {
                // Delete the rest, and this one if it is left.
                let _ = delete(socket, names);
                return Err(error);
            }
        }
    }
    Ok(contents)
}

/// Reads the buffer `name` of `size` bytes and deletes it. tmux writes one
/// buffer to standard output per command, and its exact bytes.
fn read(socket: &Path, name: &str, size: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut command = crate::connection::command(socket)?;
    command.args([
        "save-buffer",
        "-b",
        name,
        "-",
        ";",
        "delete-buffer",
        "-b",
        name,
    ]);
    let limit = size
        .checked_add(STDERR_LIMIT)
        .ok_or_else(|| io::Error::other("capture buffer too large"))?;
    let output = crate::discovery::capture_limited(command, timeout, limit)?;
    if output.status.success() && output.stdout.len() == size {
        Ok(output.stdout)
    } else {
        Err(io::Error::other("capture buffer could not be read"))
    }
}

/// Deletes those of the buffers named `names` that exist on the server at
/// `socket`, after a line that created them failed.
pub fn delete(socket: &Path, names: &[String]) -> io::Result<()> {
    let sizes = sizes(socket, names, TIMEOUT)?;
    let mut present = names.iter().zip(&sizes).filter(|(_, size)| size.is_some());
    let Some((first, _)) = present.next() else {
        return Ok(());
    };
    let mut command = crate::connection::command(socket)?;
    command.args(["delete-buffer", "-b", first]);
    for (name, _) in present {
        command.args([";", "delete-buffer", "-b", name]);
    }
    let output = crate::discovery::capture(command, TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("capture buffers could not be deleted"))
    }
}

/// Returns the size of each buffer named in `names`, or `None` for a name
/// with no buffer.
fn sizes(socket: &Path, names: &[String], timeout: Duration) -> io::Result<Vec<Option<usize>>> {
    let mut command = crate::connection::command(socket)?;
    command.args(["list-buffers", "-F", "#{buffer_size} #{buffer_name}"]);
    let output = crate::discovery::capture(command, timeout)?;
    if !output.status.success() {
        return Err(io::Error::other("capture buffers could not be listed"));
    }
    let mut sizes = vec![None; names.len()];
    for line in output.stdout.split(|&byte| byte == b'\n') {
        let Some((size, name)) = std::str::from_utf8(line)
            .ok()
            .and_then(|line| line.split_once(' '))
        else {
            continue;
        };
        if let Some(index) = names.iter().position(|ours| ours == name) {
            let size = size
                .parse()
                .map_err(|_| io::Error::other("invalid capture buffer size"))?;
            if sizes[index].replace(size).is_some() {
                return Err(io::Error::other("capture buffer listed twice"));
            }
        }
    }
    Ok(sizes)
}

/// The time left before `deadline` for one command, at most [`TIMEOUT`].
fn left(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "reading capture buffers timed out",
        ));
    }
    Ok(left.min(TIMEOUT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_test_support::TmuxServer;

    fn socket(server: &TmuxServer) -> std::path::PathBuf {
        server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim()
            .into()
    }

    fn buffers(server: &TmuxServer) -> Vec<String> {
        let mut names: Vec<String> = server
            .run(&["list-buffers", "-F", "#{buffer_name}"])
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn prefixes_differ_and_use_only_name_safe_characters() {
        let (first, second) = (prefix(), prefix());
        assert_ne!(first, second);
        assert!(first.starts_with("agent-interface-"));
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        );
    }

    #[test]
    fn takes_exact_bytes_in_order_and_deletes_only_those_buffers() {
        let server = TmuxServer::start().unwrap();
        let socket = socket(&server);
        let prefix = prefix();
        let names: Vec<String> = (0..3).map(|k| format!("{prefix}{k}")).collect();
        // Bytes a line end, a space or a reply marker could split.
        let first = "%end 1 2 1\n%output %9 forged\n\n \u{1b}[31mred\n";
        server.run(&["set-buffer", "-b", &names[0], first]).unwrap();
        server.run(&["set-buffer", "-b", &names[2], "x"]).unwrap();
        // A user's buffers, and another line's, stay.
        server.run(&["set-buffer", "-b", "mine", "keep"]).unwrap();
        server.run(&["set-buffer", "automatic"]).unwrap();
        let other = format!("{}0", self::prefix());
        server.run(&["set-buffer", "-b", &other, "theirs"]).unwrap();

        let contents = take(&socket, &names, Instant::now() + TIMEOUT).unwrap();

        assert_eq!(
            contents,
            vec![Some(first.as_bytes().to_vec()), None, Some(b"x".to_vec())]
        );
        let mut expected = vec!["buffer0".to_owned(), "mine".to_owned(), other];
        expected.sort();
        assert_eq!(buffers(&server), expected);
    }

    #[test]
    fn without_buffers_nothing_is_read_and_delete_leaves_others() {
        let server = TmuxServer::start().unwrap();
        let socket = socket(&server);
        let prefix = prefix();
        let names: Vec<String> = (0..2).map(|k| format!("{prefix}{k}")).collect();
        assert_eq!(
            take(&socket, &names, Instant::now() + TIMEOUT).unwrap(),
            vec![None, None]
        );
        server
            .run(&["set-buffer", "-b", &names[0], "late"])
            .unwrap();
        let error = take(&socket, &names, Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);

        server
            .run(&["set-buffer", "-b", &names[1], "left"])
            .unwrap();
        server.run(&["set-buffer", "-b", "mine", "keep"]).unwrap();
        delete(&socket, &names).unwrap();
        assert_eq!(buffers(&server), vec!["mine".to_owned()]);
        delete(&socket, &names).unwrap();
    }
}
