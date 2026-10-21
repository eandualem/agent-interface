//! Attach to an existing session without starting a server or changing its size.

use crate::{control::PaneId, layout};
use std::{
    io,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

/// A control client and the selected window discovered before attaching.
/// Pass the child straight to the terminal worker, which validates its seed.
pub struct Connection {
    /// Owned control client, with piped input and output.
    pub client: Child,
    /// Selected window and visible pane rectangles.
    pub layout: layout::Change,
    /// Initially active pane, before the user chooses another.
    pub input: PaneId,
}

/// Connect to an exact existing session on an explicit socket. This uses `-E`
/// to preserve the session environment and `ignore-size` to leave other clients'
/// geometry alone. It never creates a helper, session, window or pane.
/// Call on a worker: discovery is bounded to three seconds.
pub fn connect(socket: &Path, session: &str) -> io::Result<Connection> {
    if !socket.is_absolute() || session.is_empty() || session.contains([':', '\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session address",
        ));
    }
    let target = format!("={session}:");
    let mut query = command(socket)?;
    query.args(["display-message", "-p", "-t", &target,
        "#{session_id} #{pane_id} #{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}"]);
    let result = crate::discovery::capture(query, Duration::from_secs(3))?;
    if !result.status.success() {
        return Err(io::Error::other("Session unavailable"));
    }
    let text = std::str::from_utf8(&result.stdout)
        .map_err(io::Error::other)?
        .trim();
    let mut fields = text.splitn(3, ' ');
    let identity = fields
        .next()
        .filter(|id| {
            id.strip_prefix('$')
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .ok_or_else(|| io::Error::other("invalid session identity"))?;
    let input = fields
        .next()
        .and_then(|id| id.strip_prefix('%'))
        .and_then(|id| id.parse().ok())
        .map(PaneId)
        .ok_or_else(|| io::Error::other("invalid pane identity"))?;
    let layout =
        layout::parse(format!("%layout-change {}", fields.next().unwrap_or("")).as_bytes())
            .map_err(io::Error::other)?;
    let client = command(socket)?
        .args([
            "-C",
            "attach-session",
            "-E",
            "-f",
            "no-output,ignore-size,new-layouts",
            "-t",
            identity,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Connection {
        client,
        layout,
        input,
    })
}

fn command(socket: &Path) -> io::Result<Command> {
    // Finder launches do not normally inherit the shell's Homebrew PATH.
    // Honour PATH first, then the two standard macOS installation locations.
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = std::env::split_paths(&path).map(|directory| directory.join("tmux"));
    let fallbacks = if cfg!(target_os = "macos") {
        vec![
            "/opt/homebrew/bin/tmux".into(),
            "/usr/local/bin/tmux".into(),
        ]
    } else {
        Vec::new()
    };
    let binary = candidates
        .chain(fallbacks)
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "tmux is not installed or is not on PATH",
            )
        })?;
    let mut command = Command::new(binary);
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
                        | "LANG"
                        | "TMPDIR"
                        | "TERMINFO"
                        | "TERMINFO_DIRS"
                ) || key.starts_with("LC_")
            })
        }))
        .env(
            "TMUX_TMPDIR",
            socket
                .parent()
                .ok_or_else(|| io::Error::other("socket has no parent"))?,
        )
        .args(["-N", "-S"])
        .arg(socket);
    Ok(command)
}
