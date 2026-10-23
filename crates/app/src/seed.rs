//! Initial snapshots on a fresh no-output control connection.

use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};
use tmux_workspaces_tmux::layout;
use tmux_workspaces_tmux::recovery::{self, Attach, Reply, Snapshot, Step};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Seed<I = ChildStdin, O = ChildStdout> {
    pub stdin: I,
    pub stdout: O,
    pub decoder: Decoder,
    pub snapshots: Vec<Snapshot>,
    pub layout: layout::Change,
    pub events: Vec<Event>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Output(PaneId, Vec<u8>),
    Layout(Vec<u8>),
}

/// Reads the attach reply, checks recovery hooks and server version, then
/// captures before enabling output. The client must use no-output,ignore-size.
/// A deadline bounds startup even if the control client stops replying.
pub(crate) fn attach(
    client: &mut Child,
    window: u32,
    session: Option<u32>,
    scrollback: usize,
) -> io::Result<Seed> {
    let stdin = client
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("control stdin is not piped"))?;
    let stdout = client
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("control stdout is not piped"))?;
    let (send, receive) = mpsc::channel();
    let reader = thread::Builder::new()
        .name("pane snapshot".into())
        .spawn(move || {
            let _ = send.send(capture(stdin, stdout, window, session, scrollback));
        })?;
    let result = receive
        .recv_timeout(TIMEOUT)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("waiting for initial pane snapshots: {error}"),
            )
        })
        .and_then(|result| result);
    if result.is_err() {
        // This is only our control client, never the server or its panes.
        let _ = client.kill();
        let _ = client.wait();
    }
    let _ = reader.join();
    result
}

fn capture<I: Write, O: Read>(
    mut stdin: I,
    mut stdout: O,
    window: u32,
    session: Option<u32>,
    scrollback: usize,
) -> io::Result<Seed<I, O>> {
    let target = session.map_or_else(
        || format!("@{window}"),
        |session| format!("${session}:@{window}"),
    );
    let removed = format!("%unlinked-window-close @{window}");
    let mut attach = None;
    let mut layout = None;
    let mut decoder = Decoder::new();
    let mut buffer = [0; 64 * 1024];
    let mut reply = Reply::default();
    let mut expected_reply = false;
    // Attach, two hook checks, server version, then the attached layout query.
    let mut preliminary = 5;
    let mut snapshots = None;
    let mut events = Vec::new();
    loop {
        let count = stdout.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "control client exited during pane capture",
            ));
        }
        let mut failure = None;
        decoder
            .decode(&buffer[..count], |message| {
                if failure.is_some() {
                    return;
                }
                let result = (|| -> io::Result<()> {
                    match message {
                        Message::Begin { flags, .. } => {
                            reply = Reply::default();
                            // The command-line attach has no control flag;
                            // subsequent requests do. Ignore hook replies.
                            expected_reply = preliminary == 5 || flags & 1 != 0;
                        }
                        Message::ReplyLine(line) if expected_reply => {
                            reply.lines.push(line.to_vec())
                        }
                        Message::End { .. } | Message::Error { .. } => {
                            if !expected_reply {
                                return Ok(());
                            }
                            reply.ok = matches!(message, Message::End { .. });
                            let reply = std::mem::take(&mut reply);
                            if !reply.ok {
                                return Err(io::Error::other(
                                    "initial pane capture command failed",
                                ));
                            }
                            if preliminary > 0 {
                                if matches!(preliminary, 3 | 4)
                                    && recovery::hooks_break_recovery(
                                        reply.lines.iter().map(Vec::as_slice),
                                    )
                                {
                                    return Err(io::Error::other(
                                        "tmux recovery hooks prevent an exact pane snapshot",
                                    ));
                                }
                                let send = match preliminary {
                                    5 => Some("show-hooks -g ; show-hooks\n".to_owned()),
                                    3 => Some("display-message -p '#{version}'\n".to_owned()),
                                    2 => {
                                        let [line] = reply.lines.as_slice() else {
                                            return Err(io::Error::other("invalid tmux server version reply"));
                                        };
                                        let version = std::str::from_utf8(line).map_err(|_| io::Error::other("invalid tmux server version reply"))?;
                                        tmux_workspaces_tmux::preflight::check_server_version(version)?;
                                        Some(format!(
                                            "display-message -p -t '{target}' '#{{window_id}} #{{window_layout}} #{{window_visible_layout}} #{{window_raw_flags}}'\n"
                                        ))
                                    }
                                    1 => {
                                        let [line] = reply.lines.as_slice() else {
                                            return Err(io::Error::other("invalid initial window layout reply"));
                                        };
                                        let mut notification = b"%layout-change ".to_vec();
                                        notification.extend_from_slice(line);
                                        let current = layout::parse(&notification).map_err(io::Error::other)?;
                                        if current.window != window {
                                            return Err(io::Error::other("initial window layout target changed"));
                                        }
                                        let panes = current.panes.iter().map(|p| p.id).collect();
                                        let (capture, commands) = match session {
                                            Some(session) => Attach::in_session(panes, session, window, scrollback),
                                            None => Attach::new(panes, scrollback),
                                        };
                                        attach = Some(capture);
                                        layout = Some(current);
                                        // This query supersedes layout notifications that
                                        // preceded it. Later notifications stay ordered.
                                        events.retain(|event| !matches!(event, Event::Layout(_)));
                                        Some(commands)
                                    }
                                    _ => None,
                                };
                                preliminary -= 1;
                                if let Some(line) = send {
                                    stdin.write_all(line.as_bytes())?;
                                    stdin.flush()?;
                                }
                            } else if snapshots.is_none() {
                                match attach.as_mut().expect("layout queried before capture").reply(reply).map_err(io::Error::other)? {
                                    Step::Done(seeds) => snapshots = Some(seeds),
                                    Step::Wait => {}
                                    Step::Send(_) | Step::Failed => {
                                        return Err(io::Error::other(
                                            "initial pane capture did not complete",
                                        ));
                                    }
                                }
                            }
                        }
                        Message::Output { pane, data } => {
                            if snapshots.is_none() {
                                return Err(io::Error::other(
                                    "output before snapshot: use no-output,ignore-size",
                                ));
                            }
                            events.push(Event::Output(pane, data.to_vec()));
                        }
                        Message::Notification(line) if line.starts_with(b"%layout-change ") => {
                            events.push(Event::Layout(line.to_vec()));
                        }
                        Message::Notification(line) if line == removed.as_bytes() => {
                            return Err(io::Error::other("selected window left the source session during capture"));
                        }
                        Message::Notification(line) if line.starts_with(b"%session-changed ") && session.is_some() => {
                            let actual = line.split(|byte| *byte == b' ').nth(1)
                                .and_then(|id| id.strip_prefix(b"$"))
                                .and_then(|id| std::str::from_utf8(id).ok())
                                .and_then(|id| id.parse::<u32>().ok());
                            if actual != session { return Err(io::Error::other("source session changed during capture")); }
                        }
                        Message::Notification(line)
                            if line.starts_with(b"%exit") && snapshots.is_none() =>
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "exit during capture",
                            ));
                        }
                        _ => {}
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    failure = Some(error);
                }
            })
            .map_err(io::Error::other)?;
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(snapshots) = snapshots {
            return Ok(Seed {
                stdin,
                stdout,
                decoder,
                snapshots,
                layout: layout.expect("layout queried before capture"),
                events,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_capture_keeps_only_requested_history_without_moving_the_screen() {
        let server = tmux_workspaces_test_support::TmuxServer::start().unwrap();
        server
            .run(&[
                "respawn-pane",
                "-k",
                "-t",
                tmux_workspaces_test_support::TmuxServer::SESSION,
                "sh",
                "-c",
                "i=0; while [ $i -lt 200 ]; do printf 'row%03d\\n' $i; i=$((i+1)); done; exec cat",
            ])
            .unwrap();
        let height: usize = server
            .run(&["display-message", "-p", "#{pane_height}"])
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        while server
            .run(&["display-message", "-p", "#{history_size}"])
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap()
            < 201 - height
        {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        for scrollback in [0, 10] {
            let mut connection = tmux_workspaces_tmux::connection::connect(
                std::path::Path::new(socket.trim()),
                tmux_workspaces_test_support::TmuxServer::SESSION,
            )
            .unwrap();
            let seed = attach(
                &mut connection.client,
                connection.layout.window,
                Some(connection.session),
                scrollback,
            )
            .unwrap();
            let snapshot = &seed.snapshots[0];
            assert_eq!(snapshot.state.history, scrollback);
            assert_eq!(
                snapshot.rows.len(),
                scrollback + usize::from(snapshot.state.height)
            );
            let penultimate = &snapshot.rows[snapshot.rows.len() - 2];
            assert_eq!(
                penultimate
                    .segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    .trim(),
                "row199"
            );
            let _ = connection.client.kill();
            let _ = connection.client.wait();
        }
    }

    #[test]
    fn window_moved_after_layout_reply_cannot_be_captured() {
        use tmux_workspaces_test_support::TmuxServer;
        struct MoveOnCapture<'a> {
            stdin: ChildStdin,
            server: &'a TmuxServer,
            window: String,
        }
        impl Write for MoveOnCapture<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.starts_with(b"capture-pane ") {
                    self.server
                        .run(&["move-window", "-s", &self.window, "-t", "=fixture:1"])?;
                }
                self.stdin.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.stdin.flush()
            }
        }
        let server = TmuxServer::start().unwrap();
        server
            .run(&["new-session", "-d", "-s", "capture-source", "cat"])
            .unwrap();
        server
            .run(&["new-window", "-d", "-t", "=capture-source:", "cat"])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let mut connection = tmux_workspaces_tmux::connection::connect(
            std::path::Path::new(socket.trim()),
            "capture-source",
        )
        .unwrap();
        let stdin = MoveOnCapture {
            stdin: connection.client.stdin.take().unwrap(),
            server: &server,
            window: format!("@{}", connection.layout.window),
        };
        let result = capture(
            stdin,
            connection.client.stdout.take().unwrap(),
            connection.layout.window,
            Some(connection.session),
            100,
        );
        let _ = connection.client.kill();
        let _ = connection.client.wait();
        assert!(
            result.is_err(),
            "captured a window after it left the source"
        );
    }

    #[test]
    fn unsupported_servers_stop_before_layout_capture_or_input_is_enabled() {
        for version in ["3.2a", "next", "", "3.3\nextra"] {
            let wire = format!(
                "%begin 1 0 0\n%end 1 0 0\n%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\n%end 1 2 1\n%begin 1 3 1\n{version}\n%end 1 3 1\n"
            );
            let mut commands = Vec::new();
            let error = capture(&mut commands, wire.as_bytes(), 1, None, 100)
                .err()
                .unwrap();
            assert!(error.to_string().contains("server"), "{error}");
            assert_eq!(
                commands,
                b"show-hooks -g ; show-hooks\ndisplay-message -p '#{version}'\n"
            );
        }
    }

    #[test]
    fn completed_capture_keeps_coalesced_output_and_decoder_tail_even_at_exit() {
        // Attach, two hook checks, version, layout, then an 80x24 snapshot.
        let mut replies = vec![
            "",
            "",
            "",
            "3.3a",
            "@1 abcd,80x24,0,0,7 abcd,80x24,0,0,7 *",
            "SEEDED",
            "",
        ];
        replies.extend([
            "80", "24", "0", "0", "0", "0", "6", "0", "1", "0", "23", "8,16", "0", "0", "0", "1",
            "0", "0", "0", "0", "0", "0", "", "", "xterm", "",
        ]);
        // Pending bytes and the command enabling output.
        replies.extend(["0", "0", "", ""]);
        let mut wire = String::new();
        for (id, reply) in replies.iter().enumerate() {
            if id == 4 {
                wire.push_str("%layout-change @1 abcd,80x24,0,0,8 abcd,80x24,0,0,8 *\n");
            }
            wire.push_str(&format!("%begin 1 {id} 1\n{reply}\n%end 1 {id} 1\n"));
        }
        wire.push_str("%output %7 more\n");
        let mut exited = wire.clone();
        exited.push_str("%exit\n");
        let seed = capture(Vec::new(), exited.as_bytes(), 1, None, 100).unwrap();
        assert_eq!(seed.snapshots[0].state.cursor, (6, 0));
        assert_eq!(seed.layout.panes[0].id, PaneId(7));
        assert_eq!(seed.events, [Event::Output(PaneId(7), b"more".to_vec())]);

        let layout = b"%layout-change @1 abcd,80x24,0,0,7 abcd,80x24,0,0,7 *";
        let with_layout = format!(
            "{wire}{}\n%output %7 later\n",
            String::from_utf8_lossy(layout)
        );
        let seeded = capture(Vec::new(), with_layout.as_bytes(), 1, None, 100).unwrap();
        assert_eq!(
            seeded.events,
            [
                Event::Output(PaneId(7), b"more".to_vec()),
                Event::Layout(layout.to_vec()),
                Event::Output(PaneId(7), b"later".to_vec()),
            ]
        );
        wire.push_str("%output %7 par");
        let mut seed = capture(Vec::new(), wire.as_bytes(), 1, None, 100).unwrap();
        assert_eq!(seed.events, [Event::Output(PaneId(7), b"more".to_vec())]);
        seed.decoder
            .decode(b"tial\n", |message| {
                if let Message::Output { pane, data } = message {
                    seed.events.push(Event::Output(pane, data.to_vec()));
                }
            })
            .unwrap();
        assert_eq!(
            seed.events,
            [
                Event::Output(PaneId(7), b"more".to_vec()),
                Event::Output(PaneId(7), b"partial".to_vec())
            ]
        );
    }
}
