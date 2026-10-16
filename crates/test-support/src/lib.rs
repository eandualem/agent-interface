//! Test fixtures shared by the tmux-workspaces crates.
//!
//! [`TmuxServer`] is the only way tests, benchmarks and spikes start tmux
//! (`docs/decisions/0012-testing-strategy-and-tmux-isolation.md`), so tmux
//! isolation can be reviewed in this one file. This package is used only as a
//! development dependency.

#![forbid(unsafe_code)]

use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

/// A tmux server started for one test and stopped when dropped.
///
/// The server is isolated from every other tmux server on the machine,
/// including the default one that people and agents work in:
///
/// - it has its own `TMUX_TMPDIR`, created before tmux starts;
/// - every command names it with `-L` and a socket name unique to this
///   process;
/// - every command removes `TMUX` and `TMUX_PANE` from its environment,
///   because an unset `TMUX` alone falls back to the default server;
/// - it starts with no user configuration (`-f /dev/null`).
///
/// Dropping it runs `kill-server` against this server only.
pub struct TmuxServer {
    socket_name: String,
    tmpdir: TempDir,
}

impl TmuxServer {
    /// The detached session every server starts with.
    pub const SESSION: &str = "fixture";

    /// Starts a private server holding one detached 200×50 session,
    /// [`Self::SESSION`], that runs `sh`.
    pub fn start() -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // macOS limits socket paths to 104 bytes and its default temporary
        // directory is long, so the private directory goes in /tmp.
        let tmpdir = tempfile::Builder::new()
            .prefix("tw-tmux-")
            .tempdir_in("/tmp")?;
        let socket_name = format!(
            "tw-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let server = Self {
            socket_name,
            tmpdir,
        };
        server.run(&[
            "new-session",
            "-d",
            "-s",
            Self::SESSION,
            "-x",
            "200",
            "-y",
            "50",
            "sh",
        ])?;
        Ok(server)
    }

    /// Returns a `tmux` command bound to this server, for callers that manage
    /// the process themselves, such as a control-mode client.
    ///
    /// Callers may add client flags such as `-C`, but never `-L` or `-S`,
    /// which would point the command at another server.
    pub fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env("TMUX_TMPDIR", self.tmpdir.path())
            .args(["-L", &self.socket_name, "-f", "/dev/null"]);
        command
    }

    /// Runs one tmux command on this server and returns its standard output.
    ///
    /// A non-zero exit status is an error carrying tmux's standard error.
    /// `args` can only be a tmux command: `--` ends tmux's own options, so a
    /// socket option in `args` cannot point the command at another server.
    pub fn run(&self, args: &[&str]) -> io::Result<String> {
        let output = self.command().arg("--").args(args).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "tmux {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        // The command carries this server's private TMUX_TMPDIR and socket
        // name, so it cannot reach any other server. The result is ignored
        // because the server may already have exited.
        let _ = self.command().arg("kill-server").output();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn the_server_listens_in_its_private_directory() {
        let server = TmuxServer::start().unwrap();
        let socket_path = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let private_dir = server.tmpdir.path().canonicalize().unwrap();
        assert!(socket_path.starts_with(private_dir.to_str().unwrap()));
        let sessions = server
            .run(&["list-sessions", "-F", "#{session_name}"])
            .unwrap();
        assert_eq!(sessions, "fixture\n");
        assert!(server.run(&["-L", "other", "list-sessions"]).is_err());
    }

    #[test]
    fn dropping_a_server_stops_only_that_server() {
        let kept = TmuxServer::start().unwrap();
        let dropped = TmuxServer::start().unwrap();
        let pid = dropped.run(&["display-message", "-p", "#{pid}"]).unwrap();
        drop(dropped);

        let deadline = Instant::now() + Duration::from_secs(5);
        while process_exists(pid.trim()) {
            assert!(Instant::now() < deadline, "tmux server {pid} still running");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(kept.run(&["list-sessions"]).is_ok());
    }

    fn process_exists(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}
