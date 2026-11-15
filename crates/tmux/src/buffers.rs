//! Capture buffers: reading a pane's captured text outside the control
//! connection.
//!
//! tmux writes a capture's text into the control connection unchanged, and a
//! line of it can match the line that ends tmux's reply. Recovery therefore
//! captures into named tmux buffers inside its command line, lists them there
//! with [`list_command`], and a separate tmux client reads and deletes them
//! here (`docs/ARCHITECTURE.md`). Only the exact names one line created are
//! read or deleted, never any other buffer.

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

/// The command that lists the buffers named with `prefix`, for a line to
/// run after its captures; [`sizes_in`] reads its reply. Listing in the line
/// saves running another tmux client.
pub fn list_command(prefix: &str) -> String {
    format!("list-buffers -F '{FORMAT}' -f '#{{m:{prefix}*,#{{buffer_name}}}}'")
}

/// Each buffer's size and name, as [`sizes_in`] reads them.
const FORMAT: &str = "#{buffer_size} #{buffer_name}";

/// Reads the buffers in `listed`, each a name and its listed size, on the
/// server at `socket`, in order, then deletes them, by `deadline`. A name
/// with no size reads as `None`: tmux creates no buffer for an empty
/// capture. After an error, some may be left for the caller to [`delete`].
pub fn take(
    socket: &Path,
    listed: &[(String, Option<usize>)],
    deadline: Instant,
) -> io::Result<Vec<Option<Vec<u8>>>> {
    let mut contents = Vec::with_capacity(listed.len());
    for (name, size) in listed {
        let Some(size) = *size else {
            contents.push(None);
            continue;
        };
        let timeout = left(deadline)?;
        contents.push(Some(read(socket, name, size, timeout)?));
    }
    Ok(contents)
}

/// Reads the buffer `name` of `size` bytes and deletes it. tmux writes one
/// buffer to standard output per command, and its exact bytes.
fn read(socket: &Path, name: &str, size: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut command = crate::connection::command(socket)?;
    // An unattached, non-control client receives exact bytes from show-buffer.
    // Unlike save-buffer, it does not invoke the user's after-save-buffer hook,
    // which may run external commands or block this private capture read.
    command.args(["show-buffer", "-b", name, ";", "delete-buffer", "-b", name]);
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
    command.args(["list-buffers", "-F", FORMAT]);
    let output = crate::discovery::capture(command, timeout)?;
    if !output.status.success() {
        return Err(io::Error::other("capture buffers could not be listed"));
    }
    sizes_in(names, output.stdout.split(|&byte| byte == b'\n'))
}

/// Returns the size of each buffer named in `names` from the lines of a
/// listing made by [`list_command`], or `None` for a name with no buffer.
pub fn sizes_in<'a>(
    names: &[String],
    lines: impl IntoIterator<Item = &'a [u8]>,
) -> io::Result<Vec<Option<usize>>> {
    let mut sizes = vec![None; names.len()];
    for line in lines {
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

    /// Lists the buffers named with `prefix` as a line's [`list_command`] does.
    fn list(server: &TmuxServer, prefix: &str, names: &[String]) -> Vec<(String, Option<usize>)> {
        // The command quotes whole arguments, with no quote inside one.
        let line = list_command(prefix);
        let mut words = vec![String::new()];
        let mut quoted = false;
        for c in line.chars() {
            match c {
                '\'' => quoted = !quoted,
                ' ' if !quoted => words.push(String::new()),
                c => words.last_mut().unwrap().push(c),
            }
        }
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let listing = server.run(&args).unwrap();
        let sizes = sizes_in(names, listing.lines().map(str::as_bytes)).unwrap();
        names.iter().cloned().zip(sizes).collect()
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

        let listed = list(&server, &prefix, &names);
        assert_eq!(listed[1], (names[1].clone(), None));
        let contents = take(&socket, &listed, Instant::now() + TIMEOUT).unwrap();

        assert_eq!(
            contents,
            vec![Some(first.as_bytes().to_vec()), None, Some(b"x".to_vec())]
        );
        let mut expected = vec!["buffer0".to_owned(), "mine".to_owned(), other];
        expected.sort();
        assert_eq!(buffers(&server), expected);
    }

    #[test]
    fn capture_read_does_not_run_the_users_save_buffer_hook() {
        let server = TmuxServer::start().unwrap();
        let socket = socket(&server);
        let name = format!("{}0", prefix());
        let text = "%end 1 2 1\n\u{1b}[31mred\n";
        server.run(&["set-buffer", "-b", &name, text]).unwrap();
        server
            .run(&[
                "set-hook",
                "-g",
                "after-save-buffer",
                "set-buffer -b hook-fired saved",
            ])
            .unwrap();
        assert_eq!(
            take(
                &socket,
                &[(name, Some(text.len()))],
                Instant::now() + TIMEOUT
            )
            .unwrap(),
            vec![Some(text.as_bytes().to_vec())]
        );
        assert!(buffers(&server).is_empty());
        // The app neither changes the user's hook nor consumes their buffers.
        server.run(&["set-buffer", "-b", "mine", "keep"]).unwrap();
        assert_eq!(
            server.run(&["save-buffer", "-b", "mine", "-"]).unwrap(),
            "keep"
        );
        assert_eq!(
            buffers(&server),
            vec!["hook-fired".to_owned(), "mine".to_owned()]
        );
    }

    #[test]
    fn without_buffers_nothing_is_read_and_delete_leaves_others() {
        let server = TmuxServer::start().unwrap();
        let socket = socket(&server);
        let prefix = prefix();
        let names: Vec<String> = (0..2).map(|k| format!("{prefix}{k}")).collect();
        let listed = list(&server, &prefix, &names);
        assert_eq!(
            take(&socket, &listed, Instant::now() + TIMEOUT).unwrap(),
            vec![None, None]
        );
        server
            .run(&["set-buffer", "-b", &names[0], "late"])
            .unwrap();
        let listed = list(&server, &prefix, &names);
        let error = take(&socket, &listed, Instant::now()).unwrap_err();
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
