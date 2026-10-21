//! Attach to an existing session without starting a server or changing its size.

use crate::{control::PaneId, layout};
use std::{
    io,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// An existing window, tied to the server and session that listed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionWindow {
    /// Server-wide window identity.
    pub id: u32,
    /// Index displayed by tmux within the session.
    pub index: u32,
    /// Window name, for display only; never a command target.
    pub name: String,
    source: String,
}

/// A control client and the selected window discovered before attaching.
/// Pass the child straight to the terminal worker, which validates its seed.
pub struct Connection {
    /// Owned control client, with piped input and output.
    pub client: Child,
    /// Selected window and visible pane rectangles.
    pub layout: layout::Change,
    /// Initially active pane, before the user chooses another.
    pub input: PaneId,
    /// Existing windows available for local navigation.
    pub windows: Vec<SessionWindow>,
    /// Numeric session identity verified for this control client.
    pub session: u32,
}

/// Connect to an exact existing session on an explicit socket. This uses `-E`
/// to preserve the session environment and `ignore-size` to leave other clients'
/// geometry alone. It never creates a helper, session, window or pane.
/// Call on a worker: each discovery query is bounded to three seconds.
pub fn connect(socket: &Path, session: &str) -> io::Result<Connection> {
    connect_selected(socket, session, None)
}

/// View a previously listed window without selecting it on the source session.
/// A changed server/session identity or a removed window is rejected.
pub fn connect_window(
    socket: &Path,
    session: &str,
    window: &SessionWindow,
) -> io::Result<Connection> {
    connect_selected(socket, session, Some(window))
}

fn connect_selected(
    socket: &Path,
    session: &str,
    selected: Option<&SessionWindow>,
) -> io::Result<Connection> {
    if !socket.is_absolute() || session.is_empty() || session.contains([':', '\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session address",
        ));
    }
    let target = format!("={session}:");
    let mut query = command(socket)?;
    query.args(["display-message", "-p", "-t", &target,
        "#{pid}/#{session_created}/#{session_id} #{session_id} #{pane_id} #{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}"]);
    let result = crate::discovery::capture(query, Duration::from_secs(3))?;
    if !result.status.success() {
        return Err(io::Error::other("Session unavailable"));
    }
    let text = std::str::from_utf8(&result.stdout)
        .map_err(io::Error::other)?
        .trim();
    let (source, text) = text
        .split_once(' ')
        .ok_or_else(|| io::Error::other("invalid source identity"))?;
    if selected.is_some_and(|window| window.source != source) {
        return Err(io::Error::other(
            "Session changed; reconnect before selecting a window",
        ));
    }
    let mut fields = text.splitn(3, ' ');
    let identity = fields
        .next()
        .filter(|id| {
            id.strip_prefix('$')
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .ok_or_else(|| io::Error::other("invalid session identity"))?;
    let mut input = fields
        .next()
        .and_then(|id| id.strip_prefix('%'))
        .and_then(|id| id.parse().ok())
        .map(PaneId)
        .ok_or_else(|| io::Error::other("invalid pane identity"))?;
    let mut layout =
        layout::parse(format!("%layout-change {}", fields.next().unwrap_or("")).as_bytes())
            .map_err(io::Error::other)?;
    let mut query = command(socket)?;
    query.args([
        "list-windows",
        "-t",
        identity,
        "-F",
        "#{pid}/#{session_created}/#{session_id} #{window_id} #{window_index} #{q:window_name}",
    ]);
    let result = crate::discovery::capture(query, Duration::from_secs(3))?;
    if !result.status.success() {
        return Err(io::Error::other("Window list unavailable"));
    }
    let windows = parse_windows(&result.stdout, source)?;
    if let Some(selected) = selected {
        if !windows.iter().any(|window| window.id == selected.id) {
            return Err(io::Error::other("Window no longer belongs to this session"));
        }
        let target = format!("{identity}:@{}", selected.id);
        let mut query = command(socket)?;
        query.args(["display-message", "-p", "-t", &target,
            "#{pid}/#{session_created}/#{session_id} #{pane_id} #{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}"]);
        let result = crate::discovery::capture(query, Duration::from_secs(3))?;
        if !result.status.success() {
            return Err(io::Error::other("Window unavailable"));
        }
        let text = std::str::from_utf8(&result.stdout)
            .map_err(io::Error::other)?
            .trim();
        let mut fields = text.splitn(3, ' ');
        if fields.next() != Some(source) {
            return Err(io::Error::other("Session changed"));
        }
        input = fields
            .next()
            .and_then(|id| id.strip_prefix('%'))
            .and_then(|id| id.parse().ok())
            .map(PaneId)
            .ok_or_else(|| io::Error::other("invalid pane identity"))?;
        layout =
            layout::parse(format!("%layout-change {}", fields.next().unwrap_or("")).as_bytes())
                .map_err(io::Error::other)?;
        if layout.window != selected.id {
            return Err(io::Error::other("Window changed"));
        }
    }
    let mut client = command(socket)?
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
    if let Err(error) = verify_client(socket, &mut client, source) {
        let _ = client.kill();
        let _ = client.wait();
        return Err(error);
    }
    Ok(Connection {
        client,
        layout,
        input,
        windows,
        session: identity[1..].parse().map_err(io::Error::other)?,
    })
}

fn verify_client(socket: &Path, client: &mut Child, source: &str) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = client.id().to_string();
    loop {
        if client.try_wait()?.is_some() || Instant::now() >= deadline {
            return Err(io::Error::other("Control client did not attach"));
        }
        let mut query = command(socket)?;
        query.args([
            "list-clients",
            "-F",
            "#{client_pid} #{pid}/#{session_created}/#{session_id}",
        ]);
        let result =
            crate::discovery::capture(query, deadline.saturating_duration_since(Instant::now()))?;
        if !result.status.success() {
            return Err(io::Error::other("Session unavailable"));
        }
        let text = std::str::from_utf8(&result.stdout).map_err(io::Error::other)?;
        for line in text.lines() {
            if let Some((candidate, attached_source)) = line.split_once(' ')
                && candidate == pid
            {
                return if attached_source == source {
                    Ok(())
                } else {
                    Err(io::Error::other("Session changed while attaching"))
                };
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn parse_windows(bytes: &[u8], source: &str) -> io::Result<Vec<SessionWindow>> {
    let mut windows = Vec::new();
    let mut row = Vec::new();
    let mut escaped = false;
    for &byte in bytes {
        if escaped {
            row.push(byte);
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'\n' {
            let text = String::from_utf8_lossy(&row);
            let mut fields = text.splitn(4, ' ');
            if fields.next() != Some(source) {
                return Err(io::Error::other("Session changed"));
            }
            let id = fields
                .next()
                .and_then(|id| id.strip_prefix('@'))
                .and_then(|id| id.parse().ok())
                .ok_or_else(|| io::Error::other("invalid window identity"))?;
            let index = fields
                .next()
                .and_then(|index| index.parse().ok())
                .ok_or_else(|| io::Error::other("invalid window index"))?;
            let name = fields
                .next()
                .ok_or_else(|| io::Error::other("missing window name"))?
                .to_owned();
            windows.push(SessionWindow {
                id,
                index,
                name,
                source: source.to_owned(),
            });
            row.clear();
        } else {
            row.push(byte);
        }
    }
    if escaped || !row.is_empty() {
        return Err(io::Error::other("incomplete window list"));
    }
    Ok(windows)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_names_cannot_inject_records_and_old_sources_are_rejected() {
        let bytes = b"42/100/$0 @1 2 name\\ with\\ \\\\slash\\\n43/200/$1\\ @9\\ 8\\ fake\n42/100/$0 @2 5 other\n";
        let windows = parse_windows(bytes, "42/100/$0").unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, 1);
        assert_eq!(windows[0].name, "name with \\slash\n43/200/$1 @9 8 fake");
        assert_eq!(windows[1].index, 5);
        assert!(parse_windows(bytes, "43/200/$0").is_err());
        assert!(parse_windows(b"42/100/$0 @1 2 incomplete\\", "42/100/$0").is_err());
    }
}
