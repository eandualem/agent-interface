//! Recording what a drained control-mode client receives while a pane prints
//! a payload, on a private tmux server.
//!
//! The pane runs a small producer script that waits on a FIFO, prints a start
//! marker, the payload and an end marker, then sleeps. The recorder attaches a
//! control-mode client, sets its size, opens the FIFO and reads the client's
//! output as fast as it arrives. tmux's own throughput is the payload size
//! divided by the time between the two markers reaching the client.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tmux_workspaces_test_support::TmuxServer;
use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};

use crate::engine::Grid;

/// The text the producer prints before the payload. The payload generator
/// never writes `~`, so neither marker can appear in a payload.
pub const START_MARKER: &[u8] = b"~tw-bench:start~";
/// The text the producer prints after the payload: the last output of the
/// pane, so every run's output ends at the same byte.
pub const END_MARKER: &[u8] = b"~tw-bench:end~";

/// The session that runs the producer.
const SESSION: &str = "bench";
/// The reply that shows the client has applied every command before it.
const READY: &[u8] = b"tw-bench:ready";
/// How long a run may take before the recorder gives up.
const TIMEOUT: Duration = Duration::from_secs(300);

const PRODUCER: &str = r#"read -r _ < "$1"
printf '%s\n' '~tw-bench:start~'
cat "$2"
printf '%s' '~tw-bench:end~'
exec sleep 86400
"#;

/// One run of a payload through tmux.
pub struct Recording {
    /// Everything the control-mode client wrote, from attaching until the
    /// line that completed the end marker.
    pub wire: Vec<u8>,
    /// The pane that printed the payload.
    pub pane: PaneId,
    /// The time from the start marker to the end marker reaching the client.
    pub elapsed: Duration,
    /// tmux's own view of the pane once the end marker arrived.
    pub capture: Capture,
}

/// The pane as tmux holds it: `capture-pane -p -e -N` output and the cursor.
pub struct Capture {
    /// The visible screen, one line per row, with escape sequences for the
    /// text's style.
    pub text: String,
    /// The cursor's column and row.
    pub cursor: (u16, u16),
    /// Whether the cursor is shown.
    pub cursor_visible: bool,
}

/// What the reader thread reports.
struct Received {
    wire: Vec<u8>,
    elapsed: Duration,
}

/// Runs the payload at `payload` once through a fresh private tmux server,
/// in a pane of `grid`'s size. `work` is a private directory for the
/// producer script and its FIFO.
pub fn record(payload: &Path, grid: Grid, work: &Path) -> io::Result<Recording> {
    // The wire is larger than the payload: tmux escapes control bytes.
    let capacity = 2 * usize::try_from(fs::metadata(payload)?.len()).unwrap_or(0);
    let script = work.join("producer.sh");
    let fifo = work.join("gate");
    fs::write(&script, PRODUCER)?;
    let _ = fs::remove_file(&fifo);
    let status = Command::new("mkfifo").arg(&fifo).status()?;
    if !status.success() {
        return Err(io::Error::other("mkfifo failed"));
    }

    let server = TmuxServer::start()?;
    let (cols, rows) = (grid.cols.to_string(), grid.rows.to_string());
    server.run(&[
        "new-session",
        "-d",
        "-s",
        SESSION,
        "-x",
        &cols,
        "-y",
        &rows,
        "sh",
        path_str(&script)?,
        path_str(&fifo)?,
        path_str(payload)?,
    ])?;
    let pane = server.run(&["display-message", "-p", "-t", SESSION, "#{pane_id}"])?;
    let pane = pane
        .trim()
        .strip_prefix('%')
        .and_then(|number| number.parse().ok())
        .map(PaneId)
        .ok_or_else(|| io::Error::other(format!("unexpected pane id {pane:?}")))?;

    let mut client = server.control(&["attach-session", "-t", SESSION])?;
    let mut stdin = client.stdin.take().expect("piped standard input");
    let mut stdout = client.stdout.take().expect("piped standard output");
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let received = read_client(&mut stdout, pane, capacity, ready_tx);
        let _ = done_tx.send(received);
        // Keep draining, so the client never blocks on a full pipe before it
        // exits.
        let _ = io::copy(&mut stdout, &mut io::sink());
    });

    writeln!(stdin, "refresh-client -C {}x{}", grid.cols, grid.rows)?;
    writeln!(
        stdin,
        "display-message -p {}",
        String::from_utf8_lossy(READY)
    )?;
    ready_rx
        .recv_timeout(TIMEOUT)
        .map_err(|_| io::Error::other("the control client did not become ready"))?;
    let size = server.run(&[
        "display-message",
        "-p",
        "-t",
        SESSION,
        "#{window_width}x#{window_height}",
    ])?;
    if size.trim() != format!("{}x{}", grid.cols, grid.rows) {
        return Err(io::Error::other(format!(
            "the pane is {}, not the benchmark size",
            size.trim()
        )));
    }

    // Opening the FIFO releases the producer.
    fs::OpenOptions::new()
        .write(true)
        .open(&fifo)?
        .write_all(b"\n")?;
    let received = done_rx
        .recv_timeout(TIMEOUT)
        .map_err(|_| io::Error::other("the end marker did not arrive in time"))??;

    let text = server.run(&["capture-pane", "-p", "-e", "-N", "-t", SESSION])?;
    let cursor = server.run(&[
        "display-message",
        "-p",
        "-t",
        SESSION,
        "#{cursor_x} #{cursor_y} #{cursor_flag}",
    ])?;
    let mut fields = cursor.split_whitespace().map(str::parse::<u16>);
    let (Some(Ok(col)), Some(Ok(row)), Some(Ok(flag))) =
        (fields.next(), fields.next(), fields.next())
    else {
        return Err(io::Error::other(format!("unexpected cursor {cursor:?}")));
    };

    drop(stdin);
    client.wait()?;
    let _ = reader.join();
    drop(server);
    Ok(Recording {
        wire: received.wire,
        pane,
        elapsed: received.elapsed,
        capture: Capture {
            text,
            cursor: (col, row),
            cursor_visible: flag == 1,
        },
    })
}

/// Reads and decodes the client's output until the end marker arrives.
///
/// This is the drained client whose rate is tmux's throughput, so it does
/// only what any client must: read, keep the bytes and decode them. The
/// payload holds no `~`, so the markers are found by counting the `~` in the
/// pane's output: two end the start marker and four end the end marker.
/// [`check_markers`] confirms afterwards that they were the markers.
fn read_client(
    stdout: &mut impl Read,
    pane: PaneId,
    capacity: usize,
    ready: mpsc::Sender<()>,
) -> io::Result<Received> {
    let mut decoder = Decoder::new();
    let mut wire = Vec::with_capacity(capacity);
    let mut buffer = vec![0; 64 * 1024];
    let mut tildes = 0;
    let mut start = None;
    let mut end = None;
    while end.is_none() {
        let read = stdout.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::other(
                "the control client exited before the end marker",
            ));
        }
        let now = Instant::now();
        wire.extend_from_slice(&buffer[..read]);
        decoder
            .decode(&buffer[..read], |message| match message {
                Message::ReplyLine(line) if line == READY => {
                    let _ = ready.send(());
                }
                Message::Output { pane: from, data } if from == pane => {
                    tildes += data.iter().filter(|&&byte| byte == b'~').count();
                    if tildes >= 2 && start.is_none() {
                        start = Some(now);
                    }
                    if tildes >= 4 && end.is_none() {
                        end = Some(now);
                    }
                }
                _ => {}
            })
            .map_err(io::Error::other)?;
    }
    let (Some(start), Some(end)) = (start, end) else {
        unreachable!("the loop ends only once both markers arrived");
    };
    Ok(Received {
        wire,
        elapsed: end - start,
    })
}

/// The pane's output in a recording, after tmux's escaping is removed.
pub fn pane_output(wire: &[u8], pane: PaneId) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    Decoder::new()
        .decode(wire, |message| {
            if let Message::Output { pane: from, data } = message
                && from == pane
            {
                output.extend_from_slice(data);
            }
        })
        .map_err(io::Error::other)?;
    Ok(output)
}

/// Checks that the only `~` in the pane's output are the two markers, in
/// order, with the payload between them.
pub fn check_markers(output: &[u8]) -> io::Result<()> {
    let tildes = output.iter().filter(|&&byte| byte == b'~').count();
    let start = find(output, START_MARKER);
    let end = find(output, END_MARKER);
    match (tildes, start, end) {
        (4, Some(start), Some(end)) if start < end => Ok(()),
        _ => Err(io::Error::other(
            "the pane output does not hold exactly the two markers",
        )),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn path_str(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other(format!("{} is not UTF-8", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{compare, screen_from_capture};
    use crate::display::Screen;
    use crate::dump::dump;
    use crate::engines::Ghostty;
    use crate::replay::{Schedule, replay};

    #[test]
    fn a_recording_replays_to_the_screen_tmux_shows() {
        let work = tempfile::tempdir().unwrap();
        let payload = work.path().join("payload");
        fs::write(&payload, "plain\n\x1b[1;31mbold red\x1b[0m 界 e\u{301}\n").unwrap();
        let grid = Grid {
            cols: 40,
            rows: 10,
            scrollback: 100,
        };
        let recording = record(&payload, grid, work.path()).unwrap();
        let output = pane_output(&recording.wire, recording.pane).unwrap();
        check_markers(&output).unwrap();

        let schedule = Schedule {
            chunk: 7,
            read_every: 5,
        };
        let mut screen = Screen::default();
        replay::<Ghostty>(&recording.wire, recording.pane, grid, schedule, &mut screen).unwrap();
        let tmux = dump(&screen_from_capture(&recording.capture, grid));
        let difference = compare(&tmux, &dump(&screen));
        assert!(difference.is_none(), "{difference:?}");
    }
}
