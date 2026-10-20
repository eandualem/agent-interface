//! A control-mode connection that keeps everything it receives, in order, for
//! the harness to read at its own pace.

use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tmux_workspaces_test_support::TmuxServer;
use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};
use tmux_workspaces_tmux::recovery::Reply;

/// What a connection received, in order.
#[derive(Clone, Debug)]
pub enum Event {
    /// Output from one of the panes the connection watches.
    Output(PaneId, Vec<u8>),
    /// The whole reply to a command.
    Reply(Reply),
    /// A notification, such as `%layout-change`, as the whole line.
    Notification(Vec<u8>),
}

/// A control-mode client of a private server.
pub struct Connection {
    client: Child,
    stdin: Option<ChildStdin>,
    shared: Arc<Shared>,
    reader: Option<JoinHandle<()>>,
}

struct Shared {
    log: Mutex<Log>,
    changed: Condvar,
    /// While set, the connection stops reading, so tmux queues its output.
    stalled: AtomicBool,
}

#[derive(Default)]
struct Log {
    events: Vec<Event>,
    /// Bytes of output received from the watched panes.
    output_bytes: usize,
    /// Set once the client's output has ended, with any decoding error.
    ended: Option<Result<(), String>>,
}

impl Connection {
    /// Starts a control-mode client running `command`, such as
    /// `attach-session -t fixture`, and waits for that command's reply.
    /// It keeps the output of `watched` panes only.
    pub fn attach(
        server: &TmuxServer,
        command: &[&str],
        watched: &[PaneId],
    ) -> Result<Self, String> {
        let mut client = server
            .control(command)
            .map_err(|error| format!("control client: {error}"))?;
        let stdin = client.stdin.take();
        let mut stdout = client
            .stdout
            .take()
            .ok_or("the control client has no output")?;
        let shared = Arc::new(Shared {
            log: Mutex::new(Log::default()),
            changed: Condvar::new(),
            stalled: AtomicBool::new(false),
        });
        let watched = watched.to_vec();
        let reading = Arc::clone(&shared);
        let reader = std::thread::spawn(move || {
            let result = read(&mut stdout, &reading, &watched);
            reading.lock().ended = Some(result);
            reading.changed.notify_all();
        });
        let connection = Self {
            client,
            stdin,
            shared,
            reader: Some(reader),
        };
        let mut index = 0;
        loop {
            if let Event::Reply(reply) = connection.next(&mut index, TIMEOUT)? {
                if !reply.ok {
                    return Err(format!("{}: {}", command.join(" "), lines(&reply)));
                }
                return Ok(connection);
            }
        }
    }

    /// Writes one command line, which must end with a line end.
    pub fn send(&self, line: &str) -> Result<(), String> {
        let mut stdin = self.stdin.as_ref().ok_or("the connection is closed")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("writing to the control client: {error}"))
    }

    /// Returns the index the next event will have.
    pub fn mark(&self) -> usize {
        self.shared.lock().events.len()
    }

    /// Returns how many bytes of output the watched panes have sent.
    pub fn output_bytes(&self) -> usize {
        self.shared.lock().output_bytes
    }

    /// Stops or resumes reading the client's output.
    pub fn stall(&self, stalled: bool) {
        self.shared.stalled.store(stalled, Ordering::Relaxed);
    }

    /// Returns the event at `index` and moves `index` past it, waiting up to
    /// `timeout` for it to arrive.
    pub fn next(&self, index: &mut usize, timeout: Duration) -> Result<Event, String> {
        let deadline = Instant::now() + timeout;
        let mut log = self.shared.lock();
        loop {
            if let Some(event) = log.events.get(*index) {
                *index += 1;
                return Ok(event.clone());
            }
            if let Some(ended) = &log.ended {
                return Err(match ended {
                    Ok(()) => "the control client exited".to_owned(),
                    Err(error) => error.clone(),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("timed out waiting for the control client".to_owned());
            }
            log = self
                .shared
                .changed
                .wait_timeout(log, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Returns the events from `from` on that have arrived so far.
    pub fn events_from(&self, from: usize) -> Vec<Event> {
        self.shared
            .lock()
            .events
            .get(from..)
            .unwrap_or_default()
            .to_vec()
    }

    /// Returns the index of the first event from `from` on for which
    /// `matches` holds, waiting up to `timeout`.
    pub fn wait_for(
        &self,
        from: usize,
        timeout: Duration,
        matches: impl Fn(&Event) -> bool,
    ) -> Result<usize, String> {
        let deadline = Instant::now() + timeout;
        let mut index = from;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if matches(&self.next(&mut index, left)?) {
                return Ok(index - 1);
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing standard input detaches the client. Reading its output to
        // the end lets it exit: a tmux 3.7 client whose output is closed
        // while a pane writes can stop forever.
        self.stall(false);
        self.stdin = None;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = self.client.wait();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long the harness waits for tmux to answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Returns a reply's lines as text, for messages.
pub fn lines(reply: &Reply) -> String {
    reply
        .lines
        .iter()
        .map(|line| String::from_utf8_lossy(line))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Reads the client's output to its end into `shared`.
fn read(stdout: &mut impl Read, shared: &Shared, watched: &[PaneId]) -> Result<(), String> {
    let mut decoder = Decoder::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut reply: Option<Reply> = None;
    loop {
        if shared.stalled.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let read = match stdout.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("reading the control client: {error}")),
        };
        let mut events = Vec::new();
        let mut output_bytes = 0;
        decoder
            .decode(&buffer[..read], |message| match message {
                Message::Output { pane, data } => {
                    if watched.contains(&pane) {
                        output_bytes += data.len();
                        events.push(Event::Output(pane, data.to_vec()));
                    }
                }
                Message::Begin { .. } => reply = Some(Reply::default()),
                Message::ReplyLine(line) => {
                    if let Some(reply) = &mut reply {
                        reply.lines.push(line.to_vec());
                    }
                }
                Message::End { .. } | Message::Error { .. } => {
                    if let Some(mut reply) = reply.take() {
                        reply.ok = matches!(message, Message::End { .. });
                        events.push(Event::Reply(reply));
                    }
                }
                Message::Notification(line) => events.push(Event::Notification(line.to_vec())),
                Message::Notice(_) => {}
            })
            .map_err(|error| error.to_string())?;
        if !events.is_empty() {
            let mut log = shared.lock();
            log.events.append(&mut events);
            log.output_bytes += output_bytes;
            drop(log);
            shared.changed.notify_all();
        }
    }
}
