//! Test fixtures shared by the Agent Interface crates.
//!
//! [`TmuxServer`] is the only way tests, benchmarks and spikes start tmux
//! (`docs/ARCHITECTURE.md`), so tmux
//! isolation can be reviewed in this one file. This package is used only as a
//! development dependency.

#![forbid(unsafe_code)]

use std::ffi::c_int;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use signal_hook::low_level::emulate_default_handler;
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
/// Dropping it stops this server only, together with any of its clients
/// that would keep it running, and waits for them to exit.
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
        Self::start_by(|tmpdir, socket_name| output(command(tmpdir, socket_name), &START).map(drop))
    }

    /// Starts a private server as [`Self::start`] does, but has `launch` run
    /// the command that starts it: a shell command line with the same
    /// isolation as [`Self::shell_command`]. `launch` returns once the
    /// command has finished, or an error if it failed.
    ///
    /// A server keeps the scheduling of the process that started it, and so
    /// do the programs its panes run. This lets a benchmark have the system
    /// start the server as it starts the programs compared with it.
    pub fn start_with(launch: impl FnOnce(&str) -> io::Result<()>) -> io::Result<Self> {
        Self::start_by(|tmpdir, socket_name| {
            launch(&shell_command(tmpdir, socket_name, &[], &START))?;
            run(tmpdir, socket_name, &["has-session", "-t", Self::SESSION]).map(drop)
        })
    }

    /// Let product startup create the first session on this fixture's socket.
    /// The private parent exists before `start` runs; the callback must use this
    /// exact socket and must not start any other server. Normal cleanup and
    /// interrupt tracking own it before the callback begins.
    pub fn start_on_socket(start: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<Self> {
        Self::start_by(|tmpdir, name| {
            let tmpdir = tmpdir.canonicalize()?;
            let parent = tmpdir.join(format!("tmux-{}", tmpdir.metadata()?.uid()));
            std::fs::DirBuilder::new().mode(0o700).create(&parent)?;
            start(&parent.join(name))?;
            run(&tmpdir, name, &["list-sessions"]).map(drop)
        })
    }

    /// Starts a private server with `start`, which runs the one command that
    /// may start it, [`START`], on the private directory and socket name it
    /// is given.
    fn start_by(start: impl FnOnce(&Path, &str) -> io::Result<()>) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // The list stays locked until the server runs, so an interrupt
        // either finds the server in it or stops it from starting.
        let mut started = started();
        if started.interrupted {
            return Err(io::Error::other(
                "an interrupt is stopping this process's tmux servers",
            ));
        }
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
        started
            .servers
            .push((tmpdir.path().to_owned(), socket_name.clone()));
        let server = Self {
            socket_name,
            tmpdir,
        };
        // Dropping a server takes the lock too, so a `start` that panics
        // must not drop the server while the lock is held.
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            start(server.tmpdir.path(), &server.socket_name)
        }));
        drop(started);
        result.unwrap_or_else(|panic| panic::resume_unwind(panic))?;
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
        shell_command(self.tmpdir.path(), &self.socket_name, &["-N"], args)
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
        let mut state = started();
        let ours = |(_, socket_name): &(PathBuf, String)| *socket_name == self.socket_name;
        let Some(position) = state.servers.iter().position(ours) else {
            // Only an interrupt takes a server from the list. It stops the
            // server and then ends the process, which must not end first.
            while state.interrupted {
                state = STOPPED.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            return;
        };
        state.servers.swap_remove(position);
        state.stopping += 1;
        drop(state);
        stop(self.tmpdir.path(), &self.socket_name);
        started().stopping -= 1;
        STOPPED.notify_all();
    }
}

/// Waits, if an interrupt is stopping this process's servers, until it ends
/// the process, so that the process does not end first.
pub fn wait_if_interrupted() {
    let mut started = started();
    // A signal sets `RECEIVED` as it arrives, possibly before the thread
    // that stops the servers has marked the list.
    while started.interrupted || RECEIVED.load(Ordering::SeqCst) {
        started = STOPPED
            .wait(started)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Makes an interrupt stop the servers this process started before the
/// process ends.
///
/// A process ended by a signal runs no `Drop`, so its servers would keep
/// running. After this call, `SIGINT`, `SIGTERM` or `SIGHUP` stops every
/// [`TmuxServer`] the process still holds, as dropping it does, waits for
/// any that are being dropped, and then ends the process as the signal
/// would have. No server starts after that. A second signal ends the
/// process at once.
///
/// `first` runs once the interrupt has begun, before the servers are
/// stopped, so that the process can stop what it started that is not its
/// child, such as a program attached to one of the servers. A process that
/// can end on its own meanwhile calls [`wait_if_interrupted`] before it
/// ends.
pub fn stop_servers_when_interrupted(first: fn()) -> io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    // After the iterator, so that a signal that sets the flag also reaches
    // the iterator. One that arrives in between is stopped as usual, before
    // the process has started anything.
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(&RECEIVED))?;
    }
    std::thread::Builder::new()
        .name("stop-tmux-servers".to_owned())
        .spawn(move || {
            let mut signals = signals.forever();
            let Some(signal) = signals.next() else {
                return;
            };
            let stopping = std::thread::Builder::new()
                .name("stopping-tmux-servers".to_owned())
                .spawn(move || stop_all_and_end(signal, first));
            match stopping {
                Ok(_) => {
                    if let Some(again) = signals.next() {
                        let _ = emulate_default_handler(again);
                    }
                }
                Err(_) => {
                    let _ = emulate_default_handler(signal);
                }
            }
        })?;
    Ok(())
}

/// Stops every server this process holds, waits for those being dropped,
/// and ends the process as `signal` would have.
fn stop_all_and_end(signal: c_int, first: fn()) {
    let servers = {
        let mut started = started();
        started.interrupted = true;
        std::mem::take(&mut started.servers)
    };
    first();
    for (tmpdir, socket_name) in &servers {
        stop(tmpdir, socket_name);
    }
    let mut started = started();
    while started.stopping > 0 {
        started = STOPPED
            .wait(started)
            .unwrap_or_else(PoisonError::into_inner);
    }
    let _ = emulate_default_handler(signal);
}

/// Stops one private server and waits for it and its clients to exit.
///
/// `kill-server` alone is not enough: a server exits only once its clients
/// have, and tmux 3.7c leaves a client running when its standard output
/// closes while a pane is still writing to it, as when the program reading
/// a control client is killed. So whatever still runs a second later is
/// sent `SIGTERM`, then `SIGKILL`. Only processes that named this server
/// when asked, and still name its socket, are signalled.
fn stop(tmpdir: &Path, socket_name: &str) {
    let pids = [
        &["display-message", "-p", "#{pid}"][..],
        &["list-clients", "-F", "#{client_pid}"],
    ]
    .iter()
    .filter_map(|args| run(tmpdir, socket_name, args).ok())
    .collect::<String>();
    let pids: Vec<&str> = pids.split_whitespace().collect();
    // The result is ignored because the server may already have exited.
    let _ = run(tmpdir, socket_name, &["kill-server"]);
    for signal in [None, Some("TERM"), Some("KILL")] {
        let mut left = running(&pids, socket_name);
        if let Some(signal) = signal {
            for pid in &left {
                let _ = Command::new("kill")
                    .args([&format!("-{signal}"), pid.as_str()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while !left.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            left = running(&pids, socket_name);
        }
        if left.is_empty() {
            return;
        }
    }
}

/// Returns those of `pids` that are still running a tmux command on the
/// server named `socket_name`.
fn running(pids: &[&str], socket_name: &str) -> Vec<String> {
    if pids.is_empty() {
        return Vec::new();
    }
    let Ok(output) = Command::new("ps")
        .args(["-o", "pid=,args=", "-p", &pids.join(",")])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?;
            let words: Vec<&str> = words.collect();
            words
                .windows(2)
                .any(|pair| pair == ["-L", socket_name])
                .then(|| pid.to_owned())
        })
        .collect()
}

/// The servers this process has started, so that an interrupt can stop
/// them.
struct Started {
    /// The servers nothing is stopping yet, by private directory and socket
    /// name.
    servers: Vec<(PathBuf, String)>,
    /// How many servers are being dropped.
    stopping: usize,
    /// Whether an interrupt has taken the servers to stop them.
    interrupted: bool,
}

static STARTED: Mutex<Started> = Mutex::new(Started {
    servers: Vec::new(),
    stopping: 0,
    interrupted: false,
});

/// Notified whenever a server being dropped has stopped.
static STOPPED: Condvar = Condvar::new();

/// Set by the signal handler itself once an interrupt has arrived.
static RECEIVED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

fn started() -> MutexGuard<'static, Started> {
    STARTED.lock().unwrap_or_else(PoisonError::into_inner)
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

/// The default fixture startup command, creating [`TmuxServer::SESSION`].
const START: [&str; 9] = [
    "new-session",
    "-d",
    "-s",
    TmuxServer::SESSION,
    "-x",
    "200",
    "-y",
    "50",
    "sh",
];

/// Returns a shell command line that runs one tmux command, `args`, on one
/// private server, with tmux's `flags` before them.
fn shell_command(tmpdir: &Path, socket_name: &str, flags: &[&str], args: &[&str]) -> String {
    let tmpdir = format!("TMUX_TMPDIR={}", tmpdir.display());
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
    .chain([socket_name, "-f", "/dev/null"])
    .chain(flags.iter().copied())
    .chain(["--"])
    .chain(args.iter().copied());
    words.map(quote).collect::<Vec<_>>().join(" ")
}

/// Quotes `word` for a POSIX shell.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Runs one tmux command with `-N`, so that it cannot start the server:
/// only fixture constructors start servers, and nothing starts one again
/// once it has been stopped.
fn run(tmpdir: &Path, socket_name: &str, args: &[&str]) -> io::Result<String> {
    let mut command = command(tmpdir, socket_name);
    command.arg("-N");
    output(command, args)
}

/// Runs `command` with `args` as its tmux command and returns its standard
/// output, or an error carrying its standard error.
fn output(mut command: Command, args: &[&str]) -> io::Result<String> {
    let output = command.arg("--").args(args).output()?;
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
        .args(["-N", "-C", "--"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::process::ExitStatusExt;
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
    fn another_program_can_start_the_server() {
        let server = TmuxServer::start_with(|line| {
            let status = Command::new("sh").args(["-c", line]).status()?;
            if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(format!("{line} failed")))
            }
        })
        .unwrap();
        let socket_path = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let private_dir = server.tmpdir.path().canonicalize().unwrap();
        assert!(socket_path.starts_with(private_dir.to_str().unwrap()));
        let sessions = server
            .run(&["list-sessions", "-F", "#{session_name}"])
            .unwrap();
        assert_eq!(sessions, "fixture\n");
    }

    #[test]
    fn a_launch_that_panics_leaves_the_servers_usable() {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let started =
                panic::catch_unwind(|| TmuxServer::start_with(|_| panic!("the launch failed")));
            let _ = done.send(started.is_err());
        });
        // A server dropped while the list is locked would wait forever.
        assert_eq!(finished.recv_timeout(Duration::from_secs(10)), Ok(true));
        assert!(TmuxServer::start().is_ok());
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
    fn dropping_a_server_stops_a_client_that_lost_its_reader() {
        // A control client whose standard output closes while a pane is
        // writing, as when the program reading it is killed: tmux 3.7c then
        // leaves the client running, and a server waits for its clients.
        let server = TmuxServer::start().unwrap();
        server
            .run(&["new-session", "-d", "-s", "busy", "sh", "-c", "exec yes"])
            .unwrap();
        let mut client = server.control(&["attach-session", "-t", "busy"]).unwrap();
        let mut stdout = BufReader::new(client.stdout.take().unwrap());
        let mut line = String::new();
        while !line.starts_with("%output") {
            line.clear();
            assert_ne!(stdout.read_line(&mut line).unwrap(), 0);
        }
        // The pipe fills while nothing reads it.
        std::thread::sleep(Duration::from_millis(200));
        drop(client.stdin.take());
        drop(stdout);
        let pid = server.run(&["display-message", "-p", "#{pid}"]).unwrap();
        drop(server);

        let deadline = Instant::now() + Duration::from_secs(10);
        let stopped = loop {
            if client.try_wait().unwrap().is_some() && !process_exists(pid.trim()) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // A failed run must not leave the client behind either.
        let _ = client.kill();
        let _ = client.wait();
        assert!(stopped, "tmux server {pid} or its client still running");
    }

    /// Set for the process that
    /// `an_interrupt_stops_the_servers_the_process_started` interrupts, to
    /// the file that its interrupt handler's `first` creates.
    const INTERRUPTED: &str = "AGENT_INTERFACE_TEST_INTERRUPTED";

    #[test]
    fn an_interrupt_stops_the_servers_the_process_started() {
        let work = tempfile::tempdir().unwrap();
        let first = work.path().join("first");
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::a_process_waiting_for_an_interrupt",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(INTERRUPTED, &first)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap());
        let (ready, started) = mpsc::channel();
        std::thread::spawn(move || {
            let pid = stdout
                .lines()
                .find_map(|line| Some(line.ok()?.split_once("tmux server ")?.1.to_owned()));
            let _ = ready.send(pid);
        });
        let Ok(Some(pid)) = started.recv_timeout(Duration::from_secs(10)) else {
            // An interrupt lets the process stop any server it did start.
            let _ = Command::new("kill")
                .args(["-INT", &process.id().to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(10);
            while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = process.kill();
            let _ = process.wait();
            panic!("the process started no server");
        };
        Command::new("kill")
            .args(["-INT", &process.id().to_string()])
            .status()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut status = None;
        while (status.is_none() || process_exists(&pid)) && Instant::now() < deadline {
            status = status.or(process.try_wait().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
        // A failed run must not leave the process or its server behind.
        let _ = process.kill();
        let _ = process.wait();
        let stopped = !process_exists(&pid);
        if !stopped {
            let _ = Command::new("kill").args(["-KILL", &pid]).status();
        }
        assert!(stopped, "tmux server {pid} still running");
        let status = status.expect("the process did not end");
        assert_eq!(status.signal(), Some(SIGINT), "{status}");
        assert!(first.exists(), "the interrupt did not run `first`");
    }

    /// Run by `an_interrupt_stops_the_servers_the_process_started` in a
    /// process of its own; does nothing otherwise.
    #[test]
    fn a_process_waiting_for_an_interrupt() {
        if std::env::var_os(INTERRUPTED).is_none() {
            return;
        }
        static BEGUN: AtomicBool = AtomicBool::new(false);
        stop_servers_when_interrupted(|| {
            BEGUN.store(true, Ordering::Relaxed);
            // Long enough for the process to end first, if it did not wait.
            std::thread::sleep(Duration::from_millis(300));
            if let Some(path) = std::env::var_os(INTERRUPTED) {
                let _ = std::fs::write(path, "");
            }
        })
        .unwrap();
        let server = TmuxServer::start().unwrap();
        let pid = server.run(&["display-message", "-p", "#{pid}"]).unwrap();
        println!("tmux server {}", pid.trim());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !BEGUN.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Dropping the server would wait for the interrupt too. The process
        // ends here without it, as one whose program failed to start does.
        std::mem::forget(server);
        wait_if_interrupted();
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
