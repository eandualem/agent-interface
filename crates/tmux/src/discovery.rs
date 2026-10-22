//! Bounded read-only session discovery, directory queries and attachment policy on an explicit socket.

use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

const TIMEOUT: Duration = Duration::from_secs(3);
const CAPTURE_LIMIT: usize = 1024 * 1024;

/// List attachable session names without starting a server or attaching a client.
/// `socket` must be an absolute path selected by the caller. Helper sessions
/// marked `@tmux_workspaces_attachment=1` are excluded. Names retain their exact
/// spelling, including spaces and a leading `=`; they are not command targets.
///
/// An absent server is a successful empty list. Other failures return the fixed
/// diagnostic "Session list unavailable", never stderr or environment values.
/// Call this on the app's worker, not its UI thread. Failed reads must replace
/// previous availability; cached names are not evidence that sessions are online.
pub fn read_sessions(socket: &Path) -> io::Result<Vec<String>> {
    let mut command = command(socket).ok_or_else(unavailable)?;
    command.args([
        "list-sessions",
        "-f",
        "#{!=:#{@tmux_workspaces_attachment},1}",
        "-F",
        "#{session_name}",
    ]);
    let output = capture(command, TIMEOUT).map_err(|_| unavailable())?;
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| unavailable())?;
    let stderr = std::str::from_utf8(&output.stderr).map_err(|_| unavailable())?;
    if !output.status.success() {
        return if !socket.try_exists().map_err(|_| unavailable())?
            || stderr.contains("no server running")
        {
            Ok(Vec::new())
        } else {
            Err(unavailable())
        };
    }
    Ok(names(stdout))
}

/// Read the current directory of an exact session's active pane.
///
/// The caller supplies an absolute socket and a raw session name. This starts
/// no server and attaches no client. A missing session, failed query, timeout or
/// invalid directory returns a fixed diagnostic; callers keep their saved cwd.
/// UTF-8 paths retain spaces and newlines, including a trailing newline. Like
/// discovery, this bounded query belongs on a worker, not the UI thread.
pub fn read_session_directory(socket: &Path, session: &str) -> io::Result<String> {
    let unavailable = || io::Error::other("Working directory unavailable");
    if names(session).as_slice() != [session] {
        return Err(unavailable());
    }
    let mut command = command(socket).ok_or_else(unavailable)?;
    command.args([
        "display-message",
        "-p",
        "-t",
        &format!("={session}:"),
        "#{pane_current_path}",
    ]);
    let output = capture(command, TIMEOUT).map_err(|_| unavailable())?;
    if !output.status.success() {
        return Err(unavailable());
    }
    let output = std::str::from_utf8(&output.stdout).map_err(|_| unavailable())?;
    let directory = output.strip_suffix('\n').ok_or_else(unavailable)?;
    if !Path::new(directory).is_absolute() || directory.contains('\0') {
        return Err(unavailable());
    }
    Ok(directory.to_owned())
}

/// A read-only attachment decision for a session on the queried server.
/// IDs belong to that server incarnation, not to a reusable session name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachmentPolicy {
    /// Numeric part of tmux's `$N` session ID.
    pub session_id: u32,
    /// Numeric part of the source's current `@N` window ID.
    pub window_id: u32,
    /// PID of the tmux server that supplied these IDs.
    pub server_pid: u32,
    /// Only an explicit `destroy-unattached=off` permits grouping. All other
    /// values, including unknown policies, require direct attachment.
    pub may_group: bool,
}

/// Resolve an exact raw session name and its effective attachment policy.
/// Names are compared literally in a bounded listing: tmux's target syntax can
/// interpret an ID-shaped name as an ID even with its exact-name prefix.
///
/// This bounded read starts no server, attaches no client and changes no options.
/// The format includes inherited `destroy-unattached` values. Call on a worker
/// with an absolute socket. Missing sessions or malformed replies return a fixed
/// diagnostic, never a guessed identity or permission to group. This is a
/// snapshot: attachment code must revalidate the identity and policy before any
/// lifecycle operation. A later session reusing the name is a different target.
pub fn read_attachment_policy(socket: &Path, session: &str) -> io::Result<AttachmentPolicy> {
    let unavailable = || io::Error::other("Attachment policy unavailable");
    if names(session).as_slice() != [session] {
        return Err(unavailable());
    }
    let mut command = command(socket).ok_or_else(unavailable)?;
    command.args([
        "list-sessions",
        "-F",
        "#{session_id}|#{window_id}|#{pid}|#{destroy-unattached}|#{session_name}",
    ]);
    let output = capture(command, TIMEOUT).map_err(|_| unavailable())?;
    if !output.status.success() {
        return Err(unavailable());
    }
    let output = std::str::from_utf8(&output.stdout).map_err(|_| unavailable())?;
    named_policy(output, session).ok_or_else(unavailable)
}

fn named_policy(output: &str, session: &str) -> Option<AttachmentPolicy> {
    let mut found = None;
    for line in output.strip_suffix('\n')?.split('\n') {
        // The four fixed fields come first; a raw name may itself contain '|'.
        let separator = line.match_indices('|').nth(3)?.0;
        if &line[separator + 1..] == session {
            if found.is_some() {
                return None;
            }
            found = Some(policy(&line[..separator])?);
        }
    }
    found
}

fn policy(line: &str) -> Option<AttachmentPolicy> {
    if line.contains('\n') || line.contains('\r') {
        return None;
    }
    fn number(value: &str) -> Option<u32> {
        value
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| value.parse().ok())
            .flatten()
    }
    let mut fields = line.split('|');
    let session_id = number(fields.next()?.strip_prefix('$')?)?;
    let window_id = number(fields.next()?.strip_prefix('@')?)?;
    let server_pid = number(fields.next()?)?;
    let may_group = fields.next()? == "off";
    if server_pid == 0 || fields.next().is_some() {
        return None;
    }
    Some(AttachmentPolicy {
        session_id,
        window_id,
        server_pid,
        may_group,
    })
}

/// Every query uses the same isolated, non-starting client and bounded capture.
fn command(socket: &Path) -> Option<Command> {
    if !socket.is_absolute() {
        return None;
    }
    let mut command = Command::new(crate::connection::tmux_binary().ok()?);
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            key.to_str().is_some_and(|key| {
                matches!(
                    key,
                    "PATH"
                        | "HOME"
                        | "USER"
                        | "LOGNAME"
                        | "SHELL"
                        | "TERM"
                        | "LANG"
                        | "TMPDIR"
                        | "COLORTERM"
                        | "TERMINFO"
                        | "TERMINFO_DIRS"
                ) || key.starts_with("LC_")
            })
        }))
        // -S is authoritative. Also keep temporary socket state in this explicit
        // namespace; fixture callers supply an existing private parent directory.
        .env("TMUX_TMPDIR", socket.parent()?)
        // These machine-readable replies are UTF-8, even in a POSIX locale.
        .args(["-u", "-N", "-S"])
        .arg(socket);
    Some(command)
}

fn unavailable() -> io::Error {
    io::Error::other("Session list unavailable")
}

fn names(output: &str) -> Vec<String> {
    output
        .split('\n')
        .filter(|name| {
            !name.is_empty()
                && !name.contains(':')
                && name.chars().all(|ch| {
                    ch == ' '
                        || !matches!(
                            ch.general_category_group(),
                            GeneralCategoryGroup::Other | GeneralCategoryGroup::Separator
                        )
                })
        })
        .map(str::to_owned)
        .collect()
}

// Nonblocking socket pairs let the worker drain both streams while enforcing
// one deadline, without reader threads that could outlive a failed query.
pub(crate) fn capture(mut command: Command, timeout: Duration) -> io::Result<Output> {
    let deadline = Instant::now() + timeout;
    let (mut stdout, output) = UnixStream::pair()?;
    let (mut stderr, error) = UnixStream::pair()?;
    stdout.set_nonblocking(true)?;
    stderr.set_nonblocking(true)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(output)))
        .stderr(Stdio::from(OwnedFd::from(error)))
        .spawn()?;
    drop(command);
    let result = (|| {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut remaining = CAPTURE_LIMIT;
        loop {
            drain(&mut stdout, &mut out, deadline, &mut remaining)?;
            drain(&mut stderr, &mut err, deadline, &mut remaining)?;
            if let Some(status) = child.try_wait()? {
                // Include bytes written between the last drain and process exit.
                drain(&mut stdout, &mut out, deadline, &mut remaining)?;
                drain(&mut stderr, &mut err, deadline, &mut remaining)?;
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn drain(
    stream: &mut UnixStream,
    bytes: &mut Vec<u8>,
    deadline: Instant,
    remaining: &mut usize,
) -> io::Result<()> {
    let mut buffer = [0; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "session query timed out",
            ));
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                *remaining = remaining.checked_sub(count).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "session list too large")
                })?;
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_workspaces_test_support::TmuxServer;

    #[test]
    fn discovery_preserves_names_excludes_helpers_and_never_attaches_or_creates() {
        let server = TmuxServer::start().unwrap();
        for name in [" a ", "=leading", "café", "tw-ordinary", "helper"] {
            server
                .run(&["new-session", "-d", "-s", name, "sleep 60"])
                .unwrap();
        }
        server
            .run(&[
                "set-option",
                "-t",
                "=helper:",
                "@tmux_workspaces_attachment",
                "1",
            ])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let mut found = read_sessions(Path::new(socket.trim_end())).unwrap();
        found.sort();
        let mut expected = vec![
            " a ",
            "=leading",
            "café",
            "tw-ordinary",
            TmuxServer::SESSION,
        ];
        expected.sort();
        assert_eq!(found, expected);
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
        assert!(server.run(&["has-session", "-t", "=helper:"]).is_ok());
        let missing = server.address().0.join("missing.sock");
        assert!(read_sessions(&missing).unwrap().is_empty());
        assert!(!missing.exists());
        let invalid = server.address().0.join("not-a-socket");
        std::fs::write(&invalid, "keep").unwrap();
        // Linux reports ECONNREFUSED (no server), while macOS reports ENOTSOCK.
        // Both outcomes preserve the path; the provider follows tmux's result.
        let _ = read_sessions(&invalid);
        assert_eq!(std::fs::read_to_string(invalid).unwrap(), "keep");
        // A metadata failure is not proof of absence, including when the user
        // cannot traverse the socket directory. A symlink loop tests that path
        // independently of whether the test runner can bypass file permissions.
        let inaccessible = server.address().0.join("loop");
        std::os::unix::fs::symlink("loop", &inaccessible).unwrap();
        assert_eq!(
            read_sessions(&inaccessible).unwrap_err().to_string(),
            "Session list unavailable"
        );
        assert_eq!(
            names(" a \n=leading\ncafé\nbad:target\nbad\tname\n\u{a0}\n"),
            [" a ", "=leading", "café"]
        );
    }
    #[test]
    fn queries_preserve_utf8_and_newline_paths_in_a_posix_locale() {
        let server = TmuxServer::start().unwrap();
        let cwd = server.address().0.join("shell café path\n");
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.canonicalize().unwrap();
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "café",
                "-c",
                cwd.to_str().unwrap(),
                "sleep 60",
            ])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let socket = Path::new(socket.trim_end());
        let mut query = command(socket).unwrap();
        query.env("LC_ALL", "C").args([
            "display-message",
            "-p",
            "-t",
            "=café:",
            "#{session_name}:#{pane_current_path}",
        ]);
        let output = capture(query, TIMEOUT).unwrap();
        assert!(output.status.success());
        assert_eq!(
            output.stdout,
            format!("café:{}\n", cwd.display()).as_bytes()
        );
        assert_eq!(
            read_session_directory(socket, "café").unwrap(),
            cwd.to_str().unwrap()
        );
    }

    #[test]
    fn attachment_policy_reads_inherited_values_without_changing_the_source() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let socket = Path::new(socket.trim_end());
        let original = read_attachment_policy(socket, TmuxServer::SESSION).unwrap();
        assert!(original.may_group);
        let mut owner = server
            .control(&[
                "attach-session",
                "-f",
                "read-only,ignore-size",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.run(&["list-clients"]).unwrap().is_empty() {
            assert!(Instant::now() < deadline, "fixture client did not attach");
            std::thread::sleep(Duration::from_millis(10));
        }
        // A literal ID-shaped name must not resolve as another session's ID.
        let literal_name = format!("${}", original.session_id);
        let literal_id = server
            .run(&[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_id}",
                "-s",
                &literal_name,
                "sleep 60",
            ])
            .unwrap();
        let literal_id = literal_id.trim_end();
        server
            .run(&["set-option", "-t", literal_id, "destroy-unattached", "off"])
            .unwrap();
        // An owned client keeps the fixture alive while automatic destruction is on.
        server
            .run(&["set-option", "-g", "destroy-unattached", "on"])
            .unwrap();
        let options = server.run(&["show-options", "-g"]).unwrap();
        let sessions = server
            .run(&["list-sessions", "-F", "#{session_id}"])
            .unwrap();
        let clients = server
            .run(&["list-clients", "-F", "#{client_pid}"])
            .unwrap();
        let direct = read_attachment_policy(socket, TmuxServer::SESSION).unwrap();
        assert_eq!(
            direct,
            AttachmentPolicy {
                may_group: false,
                ..original
            }
        );
        let literal = read_attachment_policy(socket, &literal_name).unwrap();
        assert_eq!(literal.session_id.to_string(), &literal_id[1..]);
        assert_ne!(literal.session_id, original.session_id);
        assert!(literal.may_group);
        assert_eq!(server.run(&["show-options", "-g"]).unwrap(), options);
        assert_eq!(
            server
                .run(&["list-sessions", "-F", "#{session_id}"])
                .unwrap(),
            sessions
        );
        assert_eq!(
            server
                .run(&["list-clients", "-F", "#{client_pid}"])
                .unwrap(),
            clients
        );
        server
            .run(&[
                "set-option",
                "-t",
                TmuxServer::SESSION,
                "destroy-unattached",
                "off",
            ])
            .unwrap();
        assert_eq!(
            read_attachment_policy(socket, TmuxServer::SESSION).unwrap(),
            original
        );
        server.run(&["kill-session", "-t", literal_id]).unwrap();
        assert!(read_attachment_policy(socket, &literal_name).is_err());
        assert!(read_attachment_policy(socket, "fixt").is_err());
        let missing = server.address().0.join("missing.sock");
        assert!(read_attachment_policy(&missing, TmuxServer::SESSION).is_err());
        assert!(!missing.exists());
        assert!(!policy("$0|@0|123|future-policy").unwrap().may_group);
        assert!(!policy("$0|@0|123|").unwrap().may_group);
        assert!(policy("$+0|@0|123|off").is_none());
        assert!(policy("$0|@0|123|off\nextra").is_none());
        assert!(policy("$0|@0|0|off").is_none());
        assert_eq!(
            named_policy("$9|@2|123|off|work|session\n", "work|session")
                .unwrap()
                .session_id,
            9
        );
        drop(owner.stdin.take());
        assert!(owner.wait().unwrap().success());
    }

    #[test]
    fn capture_drains_both_streams_and_bounds_a_stalled_query() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf output; printf diagnostic >&2"]);
        let result = capture(command, TIMEOUT).unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"output");
        assert_eq!(result.stderr, b"diagnostic");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "head -c 600000 /dev/zero; exec head -c 600000 /dev/zero >&2",
        ]);
        assert_eq!(
            // Exercise the byte limit independently of the production deadline;
            // a busy CI runner can take more than three seconds to emit the data.
            capture(command, Duration::from_secs(30))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        assert_eq!(
            capture(command, Duration::from_millis(20))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
