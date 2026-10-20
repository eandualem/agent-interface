//! Initial snapshots on a fresh no-output control connection.

use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};
use tmux_workspaces_tmux::recovery::{self, Attach, Reply, Snapshot, Step};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Seed<I = ChildStdin, O = ChildStdout> {
    pub stdin: I,
    pub stdout: O,
    pub decoder: Decoder,
    pub snapshots: Vec<Snapshot>,
    pub output: Vec<(PaneId, Vec<u8>)>,
}

/// Reads the attach reply, checks recovery hooks and captures before enabling
/// output. The process must have been started with no-output,ignore-size.
/// A deadline bounds startup even if the control client stops replying.
pub(crate) fn attach(client: &mut Child, panes: Vec<PaneId>) -> io::Result<Seed> {
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
            let _ = send.send(capture(stdin, stdout, panes));
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
    panes: Vec<PaneId>,
) -> io::Result<Seed<I, O>> {
    let (mut attach, commands) = Attach::new(panes);
    let mut decoder = Decoder::new();
    let mut buffer = [0; 64 * 1024];
    let mut reply = Reply::default();
    // The initial attach reply, then two show-hooks replies.
    let mut preliminary = 3;
    let mut snapshots = None;
    let mut output = Vec::new();
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
                        Message::Begin { .. } => reply = Reply::default(),
                        Message::ReplyLine(line) => reply.lines.push(line.to_vec()),
                        Message::End { .. } | Message::Error { .. } => {
                            reply.ok = matches!(message, Message::End { .. });
                            let reply = std::mem::take(&mut reply);
                            if !reply.ok {
                                return Err(io::Error::other(
                                    "initial pane capture command failed",
                                ));
                            }
                            if preliminary > 0 {
                                if preliminary < 3
                                    && recovery::hooks_break_recovery(
                                        reply.lines.iter().map(Vec::as_slice),
                                    )
                                {
                                    return Err(io::Error::other(
                                        "tmux recovery hooks prevent an exact pane snapshot",
                                    ));
                                }
                                preliminary -= 1;
                                let send = match preliminary {
                                    2 => Some("show-hooks -g ; show-hooks\n"),
                                    0 => Some(commands.as_str()),
                                    _ => None,
                                };
                                if let Some(line) = send {
                                    stdin.write_all(line.as_bytes())?;
                                    stdin.flush()?;
                                }
                            } else if snapshots.is_none() {
                                match attach.reply(reply).map_err(io::Error::other)? {
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
                            output.push((pane, data.to_vec()));
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
                output,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_capture_keeps_coalesced_output_and_decoder_tail_even_at_exit() {
        // Attach, two hook checks, then the snapshots for an 80x24 pane.
        let mut replies = vec!["", "", "", "SEEDED", ""];
        replies.extend([
            "80", "24", "0", "0", "0", "0", "6", "0", "1", "0", "23", "8,16", "0", "0", "0", "1",
            "0", "0", "0", "0", "0", "0", "", "", "",
        ]);
        // Pending bytes and the command enabling output.
        replies.extend(["", ""]);
        let mut wire = String::new();
        for (id, reply) in replies.iter().enumerate() {
            wire.push_str(&format!("%begin 1 {id} 1\n{reply}\n%end 1 {id} 1\n"));
        }
        wire.push_str("%output %7 more\n");
        let mut exited = wire.clone();
        exited.push_str("%exit\n");
        let seed = capture(Vec::new(), exited.as_bytes(), vec![PaneId(7)]).unwrap();
        assert_eq!(seed.snapshots[0].state.cursor, (6, 0));
        assert_eq!(seed.output, [(PaneId(7), b"more".to_vec())]);

        wire.push_str("%output %7 par");
        let mut seed = capture(Vec::new(), wire.as_bytes(), vec![PaneId(7)]).unwrap();
        assert_eq!(seed.output, [(PaneId(7), b"more".to_vec())]);
        seed.decoder
            .decode(b"tial\n", |message| {
                if let Message::Output { pane, data } = message {
                    seed.output.push((pane, data.to_vec()));
                }
            })
            .unwrap();
        assert_eq!(
            seed.output,
            [
                (PaneId(7), b"more".to_vec()),
                (PaneId(7), b"partial".to_vec())
            ]
        );
    }
}
