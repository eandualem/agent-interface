//! Ordinary-shell creation using the Python-compatible library lock and environment.
//! Call on a worker, and only for explicitly eligible ordinary leaves.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{OpenOptions, TryLockError},
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
}

impl Environment {
    /// Capture the creating window's context without changing the process environment.
    /// This follows the reference SHELL and working-directory policy, not PATH discovery.
    pub fn capture() -> io::Result<Self> {
        let variables: BTreeMap<_, _> = std::env::vars_os()
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
        let shell = variables
            .get(OsStr::new("SHELL"))
            .map(PathBuf::from)
            .filter(|shell| shell.is_file())
            .unwrap_or_else(|| "/bin/sh".into());
        Ok(Self {
            variables,
            cwd: std::env::current_dir()?,
            home,
            shell,
        })
    }

    fn shell_command(&self) -> OsString {
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
        words.extend([self.shell.as_os_str().to_owned(), "-l".into(), "-i".into()]);
        let mut command = Vec::new();
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
        let mut command = Command::new("tmux");
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
        crate::discovery::capture(command, Duration::from_secs(5))
    }
}

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
    let mut existing: HashSet<_> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    for request in requests {
        let name = format!("terminal-{}", request.id);
        if existing.contains(&name) {
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
                cwd.as_os_str().to_owned(),
                environment.shell_command(),
            ],
        )?;
        require_success(created)?;
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
    // File drop releases flock; the append-open lock file remains for other windows.
    Ok(())
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
