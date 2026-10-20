//! Read-only session discovery on one explicit tmux socket.

use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

const TIMEOUT: Duration = Duration::from_secs(3);

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
    if !socket.is_absolute() {
        return Err(unavailable());
    }
    let mut command = Command::new("tmux");
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
        .env("TMUX_TMPDIR", socket.parent().ok_or_else(unavailable)?)
        .args(["-N", "-S"])
        .arg(socket)
        .args([
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
        return if !socket.exists() || stderr.contains("no server running") {
            Ok(Vec::new())
        } else {
            Err(unavailable())
        };
    }
    Ok(names(stdout))
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
fn capture(mut command: Command, timeout: Duration) -> io::Result<Output> {
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
        loop {
            drain(&mut stdout, &mut out, deadline)?;
            drain(&mut stderr, &mut err, deadline)?;
            if let Some(status) = child.try_wait()? {
                // Include bytes written between the last drain and process exit.
                drain(&mut stdout, &mut out, deadline)?;
                drain(&mut stderr, &mut err, deadline)?;
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

fn drain(stream: &mut UnixStream, bytes: &mut Vec<u8>, deadline: Instant) -> io::Result<()> {
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
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
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
        assert_eq!(
            read_sessions(&invalid).unwrap_err().to_string(),
            "Session list unavailable"
        );
        assert_eq!(std::fs::read_to_string(invalid).unwrap(), "keep");
        assert_eq!(
            names(" a \n=leading\ncafé\nbad:target\nbad\tname\n\u{a0}\n"),
            [" a ", "=leading", "café"]
        );
    }

    #[test]
    fn capture_drains_both_streams_and_bounds_a_stalled_query() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf output; printf diagnostic >&2"]);
        let result = capture(command, TIMEOUT).unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"output");
        assert_eq!(result.stderr, b"diagnostic");
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
