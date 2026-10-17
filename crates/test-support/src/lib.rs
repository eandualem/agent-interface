//! Test fixtures shared by the tmux-workspaces crates.
//!
//! [`TmuxServer`] is the only way tests, benchmarks and spikes start tmux
//! (`docs/decisions/0012-testing-strategy-and-tmux-isolation.md`), so tmux
//! isolation can be reviewed in this one file. This package is used only as a
//! development dependency.

#![forbid(unsafe_code)]

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
            .prefix(TMPDIR_PREFIX)
            .tempdir_in("/tmp")?;
        let socket_name = format!(
            "{SOCKET_PREFIX}{}-{}",
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

    /// Returns what another process needs to reach this server with
    /// [`ServerRef::new`]: its private `TMUX_TMPDIR` and its socket name.
    pub fn address(&self) -> (&Path, &str) {
        (self.tmpdir.path(), &self.socket_name)
    }

    /// Returns a shell command line that runs one tmux command on this
    /// server, with the same isolation as [`Self::run`], for a program the
    /// harness starts that runs the command itself: a terminal attaching an
    /// ordinary client, for example.
    pub fn shell_command(&self, args: &[&str]) -> String {
        let tmpdir = format!("TMUX_TMPDIR={}", self.tmpdir.path().display());
        let words = [
            "env",
            "-u",
            "TMUX",
            "-u",
            "TMUX_PANE",
            &tmpdir,
            "tmux",
            "-L",
        ]
        .into_iter()
        .chain([self.socket_name.as_str(), "-f", "/dev/null", "--"])
        .chain(args.iter().copied());
        words.map(quote).collect::<Vec<_>>().join(" ")
    }

    /// Runs one tmux command on this server and returns its standard output.
    ///
    /// A non-zero exit status is an error carrying tmux's standard error.
    /// `args` can only be a tmux command: `--` ends tmux's own options, so a
    /// socket option in `args` cannot point the command at another server.
    pub fn run(&self, args: &[&str]) -> io::Result<String> {
        run(self.tmpdir.path(), &self.socket_name, args)
    }

    /// Starts a control-mode client (`tmux -C`) on this server that runs one
    /// tmux command, such as `attach-session -t fixture`, with its standard
    /// input and output piped and its standard error discarded.
    ///
    /// The caller owns the client: closing its standard input detaches it.
    /// As with [`Self::run`], `args` can only be a tmux command.
    pub fn control(&self, args: &[&str]) -> io::Result<Child> {
        control(self.tmpdir.path(), &self.socket_name, args)
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        // The command carries this server's private TMUX_TMPDIR and socket
        // name, so it cannot reach any other server. The result is ignored
        // because the server may already have exited.
        let _ = command(self.tmpdir.path(), &self.socket_name)
            .arg("kill-server")
            .output();
    }
}

/// A private server that a [`TmuxServer`] started in another process, such
/// as the benchmark harness, for a benchmark program that runs in its own
/// process.
///
/// It runs commands exactly as [`TmuxServer`] does. Dropping it leaves the
/// server running: the process that started the server stops it.
pub struct ServerRef {
    socket_name: String,
    tmpdir: PathBuf,
}

impl ServerRef {
    /// Refers to the server that [`TmuxServer::address`] described.
    ///
    /// `tmpdir` must be a private directory that [`TmuxServer::start`]
    /// created, and `socket_name` a name it gave, so a mistaken address
    /// cannot reach the default server or anyone else's.
    pub fn new(tmpdir: &Path, socket_name: &str) -> io::Result<Self> {
        // tmux joins the name to its socket directory, so a name with a path
        // in it could point outside the private directory.
        let generated = socket_name.strip_prefix(SOCKET_PREFIX).is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        });
        if !generated {
            return Err(io::Error::other(format!(
                "{socket_name:?} is not a socket name made by TmuxServer"
            )));
        }
        let canonical = tmpdir.canonicalize()?;
        let private = canonical
            .parent()
            .zip(canonical.file_name().and_then(|name| name.to_str()))
            .is_some_and(|(parent, name)| {
                name.starts_with(TMPDIR_PREFIX)
                    && Path::new("/tmp").canonicalize().ok().as_deref() == Some(parent)
            });
        if !private {
            return Err(io::Error::other(format!(
                "{} is not a private tmux directory made by TmuxServer",
                tmpdir.display()
            )));
        }
        // The socket and its directory must be the files tmux made there, not
        // links to another server's.
        let directory = canonical.join(format!("tmux-{}", canonical.metadata()?.uid()));
        let socket = directory.join(socket_name);
        let is = |path: &Path, kind: fn(&std::fs::FileType) -> bool| {
            std::fs::symlink_metadata(path).is_ok_and(|metadata| kind(&metadata.file_type()))
        };
        if !is(&directory, std::fs::FileType::is_dir) || !is(&socket, FileTypeExt::is_socket) {
            return Err(io::Error::other(format!(
                "{} is not a tmux socket that TmuxServer made",
                socket.display()
            )));
        }
        Ok(Self {
            socket_name: socket_name.to_owned(),
            tmpdir: canonical,
        })
    }

    /// Runs one tmux command on the server, as [`TmuxServer::run`] does.
    pub fn run(&self, args: &[&str]) -> io::Result<String> {
        run(&self.tmpdir, &self.socket_name, args)
    }

    /// Starts a control-mode client on the server, as
    /// [`TmuxServer::control`] does.
    pub fn control(&self, args: &[&str]) -> io::Result<Child> {
        control(&self.tmpdir, &self.socket_name, args)
    }
}

/// The start of the name of every private directory [`TmuxServer::start`]
/// creates.
const TMPDIR_PREFIX: &str = "tw-tmux-";

/// The start of every socket name [`TmuxServer::start`] gives; the rest is
/// two numbers joined by a hyphen.
const SOCKET_PREFIX: &str = "tw-";

/// Returns a `tmux` command bound to one private server, before any command
/// arguments. It stays private so that no caller can add `-L` or `-S` and
/// point it at another server.
fn command(tmpdir: &Path, socket_name: &str) -> Command {
    let mut command = Command::new("tmux");
    command
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env("TMUX_TMPDIR", tmpdir)
        .args(["-L", socket_name, "-f", "/dev/null"]);
    command
}

/// Quotes `word` for a POSIX shell.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn run(tmpdir: &Path, socket_name: &str, args: &[&str]) -> io::Result<String> {
    let output = command(tmpdir, socket_name).arg("--").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

fn control(tmpdir: &Path, socket_name: &str, args: &[&str]) -> io::Result<Child> {
    command(tmpdir, socket_name)
        .arg("-C")
        .arg("--")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;
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

    #[test]
    fn a_control_client_runs_commands_on_its_server() {
        let server = TmuxServer::start().unwrap();
        let mut client = server
            .control(&["attach-session", "-t", TmuxServer::SESSION])
            .unwrap();
        let mut stdin = client.stdin.take().unwrap();
        let stdout = BufReader::new(client.stdout.take().unwrap());
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines() {
                if lines.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        writeln!(stdin, "display-message -p '#{{session_name}} via control'").unwrap();
        loop {
            let line = received.recv_timeout(Duration::from_secs(10)).unwrap();
            if line == "fixture via control" {
                break;
            }
        }
        drop(stdin);
        assert!(client.wait().unwrap().success());
    }

    #[test]
    fn a_shell_command_runs_on_this_server_only() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&["new-session", "-d", "-s", "it's quoted", "sh"])
            .unwrap();
        let line = server.shell_command(&["list-sessions", "-F", "#{session_name}"]);
        let output = Command::new("sh")
            .args(["-c", &line])
            .env("TMUX", "/tmp/not-this-one,1,0")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "fixture\nit's quoted\n"
        );
    }

    #[test]
    fn a_server_ref_reaches_the_server_and_leaves_it_running() {
        let server = TmuxServer::start().unwrap();
        let (tmpdir, socket_name) = server.address();
        let shared = ServerRef::new(tmpdir, socket_name).unwrap();
        assert_eq!(
            shared
                .run(&["list-sessions", "-F", "#{session_name}"])
                .unwrap(),
            "fixture\n"
        );
        drop(shared);
        assert!(server.run(&["list-sessions"]).is_ok());
    }

    #[test]
    fn a_server_ref_refuses_other_directories() {
        let other = tempfile::Builder::new()
            .prefix("not-tw-")
            .tempdir_in("/tmp")
            .unwrap();
        assert!(ServerRef::new(other.path(), "tw-1-0").is_err());
        let nested = tempfile::Builder::new()
            .prefix(TMPDIR_PREFIX)
            .tempdir_in(other.path())
            .unwrap();
        assert!(ServerRef::new(nested.path(), "tw-1-0").is_err());
    }

    #[test]
    fn a_server_ref_refuses_a_link_to_another_server() {
        let server = TmuxServer::start().unwrap();
        let other = TmuxServer::start().unwrap();
        let socket = |server: &TmuxServer| {
            PathBuf::from(
                server
                    .run(&["display-message", "-p", "#{socket_path}"])
                    .unwrap()
                    .trim(),
            )
        };
        let link = socket(&server).with_file_name("tw-9-9");
        std::os::unix::fs::symlink(socket(&other), &link).unwrap();
        let (tmpdir, _) = server.address();
        assert!(ServerRef::new(tmpdir, "tw-9-9").is_err());
    }

    #[test]
    fn a_server_ref_refuses_names_it_did_not_make() {
        let server = TmuxServer::start().unwrap();
        let (tmpdir, _) = server.address();
        for name in [
            "default",
            "../../tmux-501/default",
            "tw-1/../x",
            "tw-",
            "tw-1-0 ",
        ] {
            assert!(ServerRef::new(tmpdir, name).is_err(), "{name:?}");
        }
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
