//! Ordinary-shell creation using the library's shell lock and environment.
//! Call on a worker, and only for explicitly eligible ordinary leaves.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use uzers::os::unix::UserExt;

const CONTEXT: &[&str] = &[
    "SSH_AUTH_SOCK",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
];
const COMMAND_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "TMPDIR",
    "COLORTERM",
    "TERMINFO",
    "TERMINFO_DIRS",
];

/// A launch-time environment snapshot. Missing context stays missing; empty stays empty.
#[derive(Clone)]
pub struct Environment {
    variables: BTreeMap<OsString, OsString>,
    cwd: PathBuf,
    home: PathBuf,
    shell: PathBuf,
    prepare: Option<fn(&mut Command)>,
}

impl Environment {
    /// Capture the creating window's context without changing the process environment.
    /// SHELL and the working directory fall back as below; PATH is not searched.
    pub fn capture() -> io::Result<Self> {
        Self::from_variables(std::env::vars_os(), std::env::current_dir()?)
    }

    /// As [`Self::capture`], from these variables and working directory.
    fn from_variables(
        all: impl IntoIterator<Item = (OsString, OsString)>,
        current: PathBuf,
    ) -> io::Result<Self> {
        let variables: BTreeMap<_, _> = all
            .into_iter()
            .filter(|(name, _)| {
                COMMAND_ENV
                    .iter()
                    .chain(CONTEXT)
                    .any(|allowed| name == allowed)
                    || name.as_bytes().starts_with(b"LC_")
            })
            .collect();
        let home = match variables.get(OsStr::new("HOME")) {
            Some(home) if home.is_empty() => PathBuf::from("/"),
            Some(home) => PathBuf::from(home),
            None => uzers::get_user_by_uid(uzers::get_current_uid())
                .map(|user| user.home_dir().to_owned())
                .ok_or_else(|| io::Error::other("Home directory unavailable"))?,
        };
        let shell = login_shell(&variables);
        Ok(Self {
            variables,
            // An app opened from the Dock or Finder runs in "/"; use home instead.
            cwd: Some(current)
                .filter(|cwd| cwd.as_os_str() != "/")
                .unwrap_or_else(|| home.clone()),
            home,
            shell,
            prepare: None,
        })
    }

    /// Apply `prepare` to every tmux command before it starts. A command may
    /// start the server, whose shells inherit the state it was started with,
    /// such as the signal mask of the thread that started it.
    pub fn prepared(mut self, prepare: fn(&mut Command)) -> Self {
        self.prepare = Some(prepare);
        self
    }

    /// The command a new shell runs. With `guard`, the folder [`install_guard`]
    /// filled, zsh, bash and fish also define the Backbone guard.
    fn shell_command(&self, guard: Option<&Path>) -> OsString {
        let kind = guard.and_then(|guard| Some((guard, guarded(&self.shell)?)));
        let mut words: Vec<OsString> =
            ["env", "-u", "TMUX", "-u", "TMUX_PANE", "-u", "TMUX_TMPDIR"]
                .map(Into::into)
                .to_vec();
        for name in CONTEXT {
            words.extend(["-u".into(), (*name).into()]);
        }
        for name in CONTEXT {
            if let Some(value) = self.variables.get(OsStr::new(name)) {
                let mut assignment = OsString::from(name);
                assignment.push("=");
                assignment.push(value);
                words.push(assignment);
            }
        }
        let mut start: Vec<OsString> = vec!["-l".into(), "-i".into()];
        match kind {
            Some((guard, Guarded::Zsh)) => {
                let mut assignment = OsString::from("ZDOTDIR=");
                assignment.push(guard.join("zsh"));
                words.push(assignment);
            }
            Some((guard, Guarded::Bash)) => {
                // A login shell reads no rcfile; this one reads the login files.
                start = vec![
                    "--rcfile".into(),
                    guard.join("bash/start.bash").into(),
                    "-i".into(),
                ];
            }
            Some((guard, Guarded::Fish)) => {
                let mut source = b"source '".to_vec();
                for &byte in guard.join("fish/guard.fish").as_os_str().as_bytes() {
                    if matches!(byte, b'\\' | b'\'') {
                        source.push(b'\\');
                    }
                    source.push(byte);
                }
                source.push(b'\'');
                start.extend(["-C".into(), OsString::from_vec(source)]);
            }
            None => {}
        }
        words.push(self.shell.as_os_str().to_owned());
        words.extend(start);
        let mut command = b"exec".to_vec();
        for word in words {
            if !command.is_empty() {
                command.push(b' ');
            }
            command.push(b'\'');
            for byte in word.as_bytes() {
                if *byte == b'\'' {
                    command.extend_from_slice(b"'\\''");
                } else {
                    command.push(*byte);
                }
            }
            command.push(b'\'');
        }
        OsString::from_vec(command)
    }

    fn run(&self, socket: &Path, args: &[OsString]) -> io::Result<std::process::Output> {
        // Search the captured PATH first, then the standard macOS locations a
        // Finder launch's PATH omits. The command keeps its captured environment.
        let path = self
            .variables
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_default();
        let mut command = Command::new(crate::connection::tmux_binary_on(&path)?);
        command.env_clear();
        for (name, value) in &self.variables {
            if COMMAND_ENV.iter().any(|allowed| name == allowed)
                || name.as_bytes().starts_with(b"LC_")
            {
                command.env(name, value);
            }
        }
        // Fixture commands additionally require a private default namespace.
        // Production uses only the reference allowlist and the explicit socket.
        #[cfg(test)]
        command.env("TMUX_TMPDIR", socket.parent().unwrap());
        command.arg("-S").arg(socket);
        for arg in args {
            let bytes = arg.as_bytes();
            if bytes.last() == Some(&b';') {
                let mut escaped = bytes[..bytes.len() - 1].to_vec();
                escaped.extend_from_slice(b"\\;");
                command.arg(OsString::from_vec(escaped));
            } else {
                command.arg(arg);
            }
        }
        if let Some(prepare) = self.prepare {
            prepare(&mut command);
        }
        crate::discovery::capture(command, Duration::from_secs(5))
    }
}

/// Lock files a creator found held, so a test can see a creator waiting.
#[cfg(test)]
static LOCKS_FOUND_HELD: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// One eligible ordinary leaf; attachments and empty choosers are excluded by the caller.
pub struct Request {
    /// Stable twelve-character lowercase hexadecimal leaf identity.
    pub id: String,
    /// Saved ordinary-shell directory; absent or empty uses the launch directory.
    pub cwd: Option<PathBuf>,
}

/// Reuse existing sessions unchanged or create missing ones under the shared flock.
/// The caller supplies the existing private library socket parent. This never kills
/// a session, respawns an exited shell, replaces a lock inode, or changes server options.
/// Empty requests perform no I/O. All commands use the supplied socket explicitly.
pub fn ensure(socket: &Path, requests: &[Request], environment: &Environment) -> io::Result<()> {
    if requests.is_empty() {
        return Ok(());
    }
    if requests.iter().any(|request| {
        request.id.len() != 12
            || !request
                .id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(io::Error::other("Invalid ordinary-shell identity"));
    }
    let parent = socket
        .parent()
        .filter(|_| socket.is_absolute())
        .ok_or_else(|| io::Error::other("An absolute ordinary-shell socket is required"))?;
    let uid = uzers::get_current_uid();
    let metadata = parent.symlink_metadata()?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "Ordinary-shell socket parent must be a private owned directory",
        ));
    }
    match socket.symlink_metadata() {
        Ok(metadata) if !metadata.file_type().is_socket() || metadata.uid() != uid => {
            return Err(io::Error::other(
                "Ordinary-shell path is not an owned Unix socket",
            ));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut lock_path = socket.as_os_str().to_owned();
    lock_path.push(".viewer-lock");
    let lock = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&lock_path)?;
    let opened = lock.metadata()?;
    if !opened.is_file() || opened.uid() != uid || opened.nlink() != 1 {
        return Err(io::Error::other(
            "Ordinary-shell lock must be an owned regular file",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                #[cfg(test)]
                LOCKS_FOUND_HELD
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(PathBuf::from(&lock_path));
                thread::sleep(Duration::from_millis(10))
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Ordinary-shell lock is busy",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    let _held = Held(lock);
    let current = Path::new(&lock_path).symlink_metadata()?;
    if !current.is_file() || current.dev() != opened.dev() || current.ino() != opened.ino() {
        return Err(io::Error::other(
            "Ordinary-shell lock changed while opening",
        ));
    }
    let listed = environment.run(
        socket,
        &[
            "list-sessions".into(),
            "-F".into(),
            "#{session_name}".into(),
        ],
    )?;
    let mut guard = None;
    let mut existing: HashSet<_> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    for request in requests {
        let name = format!("terminal-{}", request.id);
        if existing.contains(&name) {
            tracing::debug!(event = "ordinary_shell_reused", pane_id = %request.id, "Existing ordinary shell reused");
            continue;
        }
        let requested_cwd = request
            .cwd
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(&environment.cwd);
        let cwd = if requested_cwd.is_dir() {
            requested_cwd
        } else {
            &environment.home
        };
        if guard.is_none() && guarded(&environment.shell).is_some() {
            // A shell without the guard is still a working terminal.
            guard = Some(install_guard(parent).map_err(|error| {
                tracing::warn!(event = "shell_guard_unavailable", %error, "Shell starts without the Backbone guard");
            }));
        }
        // tmux expands formats in -c even though this is a separate argv value.
        // Quote its marker while retaining non-UTF-8 Unix path bytes.
        let mut literal_cwd = Vec::new();
        for &byte in cwd.as_os_str().as_bytes() {
            literal_cwd.push(byte);
            if byte == b'#' {
                literal_cwd.push(byte);
            }
        }
        let created = environment.run(
            socket,
            &[
                "-f".into(),
                "/dev/null".into(),
                "new-session".into(),
                "-d".into(),
                "-s".into(),
                name.clone().into(),
                "-c".into(),
                OsString::from_vec(literal_cwd),
                environment.shell_command(guard.as_ref().and_then(|g| g.as_deref().ok())),
            ],
        )?;
        require_success(created)?;
        tracing::debug!(event = "ordinary_shell_created", pane_id = %request.id, directory_fallback = !requested_cwd.is_dir(), "Ordinary shell created");
        let target = format!("={name}:");
        for (command, option, value) in [
            ("set-option", "status", "off"),
            ("set-option", "mouse", "on"),
            ("set-window-option", "remain-on-exit", "on"),
        ] {
            require_success(environment.run(
                socket,
                &[
                    command.into(),
                    "-t".into(),
                    target.clone().into(),
                    option.into(),
                    value.into(),
                ],
            )?)?;
        }
        existing.insert(name);
    }
    // Dropping `_held` releases the flock; the append-open lock file remains for other windows.
    Ok(())
}

/// The shell lock, released when this is dropped on every return. Closing
/// the file alone leaves the lock held while a process another thread is
/// starting still has a copy of it.
struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// A shell that defines the Backbone guard for `claude`, `codex` and `agy`.
#[derive(Clone, Copy)]
enum Guarded {
    Zsh,
    Bash,
    Fish,
}

fn guarded(shell: &Path) -> Option<Guarded> {
    match shell.file_name()?.as_bytes() {
        b"zsh" => Some(Guarded::Zsh),
        b"bash" => Some(Guarded::Bash),
        b"fish" => Some(Guarded::Fish),
        _ => None,
    }
}

/// The guard's start-up files, by their path in the folder.
const GUARD_FILES: &[(&str, &str)] = &[
    ("guard.sh", include_str!("shells/integration/guard.sh")),
    (
        "zsh/.zshenv",
        include_str!("shells/integration/zsh/.zshenv"),
    ),
    (
        "bash/start.bash",
        include_str!("shells/integration/bash/start.bash"),
    ),
    (
        "fish/guard.fish",
        include_str!("shells/integration/fish/guard.fish"),
    ),
];

/// Write the guard's start-up files into `parent`, the private socket folder
/// that every library of this user shares, and return their folder.
fn install_guard(parent: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let root = parent.join("shell-guard");
    for (relative, contents) in GUARD_FILES {
        let path = root.join(relative);
        let folder = path.parent().expect("guard files are in a folder");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)?;
        if std::fs::read(&path).is_ok_and(|current| current == contents.as_bytes()) {
            continue;
        }
        // A shell starting now reads the old file or the new one, never half.
        // Each writer stages its own copy: another library's lock may be free.
        static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let partial = folder.join(format!(
            ".partial-{}-{}",
            std::process::id(),
            STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&partial, contents)?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(root)
}

/// The shell a new ordinary terminal runs: SHELL when it names a file, else `/bin/sh`.
fn login_shell(variables: &BTreeMap<OsString, OsString>) -> PathBuf {
    variables
        .get(OsStr::new("SHELL"))
        .map(PathBuf::from)
        .filter(|shell| shell.is_file())
        .unwrap_or_else(|| "/bin/sh".into())
}

fn require_success(output: std::process::Output) -> io::Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Ordinary-shell command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests;
