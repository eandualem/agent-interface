//! The panes of one control-mode connection, each kept as terminal state on a
//! terminal worker thread
//! (`docs/ARCHITECTURE.md`).
//!
//! A reader thread reads the tmux client's output and decodes it. Each pane's
//! output goes, in order, to the terminal worker, which owns every
//! [`Terminal`] and copies the panes that changed into owned [`Screen`]s. At
//! most one update waits for the user interface. While it waits, the worker
//! keeps applying output. A pane that changes and is not in the waiting update
//! joins it at once; the next update holds every other pane that changed
//! meanwhile, so the user interface copies at most one update per frame.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tmux_workspaces_term::{Screen, Terminal, input::Input};
use tmux_workspaces_tmux::control::{Decoder, Message, PaneId};
use tmux_workspaces_tmux::recovery::{Recovery, Reply, Snapshot, Step};
use tmux_workspaces_tmux::{command, layout};

/// The most pane output the reader queues for the worker before it stops
/// reading, so a slow worker holds back tmux instead of growing a queue.
const QUEUE_LIMIT: usize = 4 * 1024 * 1024;

/// How much of the client's output the reader reads at a time.
const READ_SIZE: usize = 64 * 1024;

/// Keep send-keys arguments below tmux's parser stack limit.
const INPUT_CHUNK: usize = 1024;

/// Maximum bytes in one keyboard or clipboard operation.
pub(crate) const INPUT_LIMIT: usize = QUEUE_LIMIT;

/// A pane's place in its tmux window, in terminal cells.
pub use tmux_workspaces_tmux::layout::Pane as PaneLayout;

/// The current visible rectangles and the screens changed since the previous update.
pub struct Update {
    /// Rectangles that match these screens.
    pub layout: Vec<PaneLayout>,
    /// The panes whose screen contents changed.
    pub screens: Vec<(PaneId, Screen)>,
}

/// The panes of one control-mode client, kept up to date in the background.
///
/// Dropping it detaches the client, waits for it to exit and stops its
/// threads. The tmux session keeps running.
pub struct Panes {
    shared: Arc<Shared>,
    control: Arc<Control>,
    input: Mutex<()>,
    source: Option<(u32, u32)>,
    client: Mutex<Child>,
    threads: Vec<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Pane output the worker has not applied yet, in the order tmux sent it.
    queue: VecDeque<Event>,
    /// The bytes in `queue`.
    queued: usize,
    /// The update waiting for the user interface.
    update: Option<Update>,
    /// Screens the user interface no longer needs, for the worker to reuse.
    spare: Vec<Screen>,
    /// Whether the client has exited: the reader has queued everything it
    /// will.
    closed: bool,
    /// Whether the panes are being dropped, so pending work is abandoned.
    cancelled: bool,
    /// Whether the worker has stopped, after handing over every change, or
    /// because the panes are being dropped.
    finished: bool,
    failure: Option<String>,
    /// A pipe failure is reported without abandoning already queued output.
    transport_failure: Option<String>,
    gone: Vec<PaneId>,
}

enum Event {
    Output(PaneId, Vec<u8>),
    Reply(Reply),
    Layout(layout::Change),
    Geometry,
    Encode(InputRequest),
}

impl Event {
    fn size(&self) -> usize {
        match self {
            Self::Output(_, bytes) => bytes.len(),
            Self::Reply(reply) => reply.lines.iter().map(Vec::len).sum(),
            Self::Layout(change) => change.panes.len() * std::mem::size_of::<PaneLayout>(),
            Self::Geometry => 0,
            Self::Encode(request) => request.input.size() + std::mem::size_of::<InputRequest>(),
        }
    }
}

struct InputRequest {
    pane: PaneId,
    input: Input,
    reply: mpsc::Sender<Result<Vec<u8>, String>>,
}

#[derive(Clone, Copy)]
enum ReplyOwner {
    Input(PaneId),
    Recovery,
    GeometrySubscription,
    SourceSubscription,
    Size,
}

struct Control {
    commands: mpsc::Sender<WriteRequest>,
    pending: Mutex<VecDeque<ReplyOwner>>,
}

enum WriteRequest {
    Command {
        line: String,
        owner: ReplyOwner,
        sent: Option<mpsc::Sender<io::Result<()>>>,
    },
    Close,
}

impl Control {
    fn send(&self, line: &str, owner: ReplyOwner) -> io::Result<()> {
        let (sent, received) = mpsc::channel();
        let wait = matches!(owner, ReplyOwner::Input(_));
        self.commands
            .send(WriteRequest::Command {
                line: line.into(),
                owner,
                sent: wait.then_some(sent),
            })
            .map_err(|_| io::Error::other("the client is detached"))?;
        if wait {
            received
                .recv_timeout(crate::seed::TIMEOUT)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out writing to the control client",
                    ),
                    mpsc::RecvTimeoutError::Disconnected => {
                        io::Error::other("the control writer exited")
                    }
                })??;
        }
        Ok(())
    }
}

// Writes must not block the terminal worker: while a large input fills the
// client's pipe, that worker still drains output and command replies.
fn write_commands(
    mut stdin: impl Write,
    requests: mpsc::Receiver<WriteRequest>,
    control: &Control,
    shared: &Shared,
) {
    while let Ok(WriteRequest::Command { line, owner, sent }) = requests.recv() {
        if shared.lock().cancelled {
            break;
        }
        control
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(owner);
        let result = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush());
        let failed = result.is_err();
        if let Err(error) = &result {
            shared
                .lock()
                .transport_failure
                .get_or_insert_with(|| format!("writing to control client: {error}"));
            shared.changed.notify_all();
        }
        if let Some(sent) = sent {
            let _ = sent.send(result);
        }
        if failed {
            // Closing stdin ends the reader. Let the worker drain its final
            // output. The UI can also report the retained write error.
            break;
        }
    }
}

struct LivePane {
    terminal: Terminal,
    size: (u16, u16),
    offset: (u16, u16),
    // The engine's display copy cannot express tmux's logical margin cursor.
    // Keep it in capture-only mode rather than streaming into that copy.
    captured: bool,
    needs_capture: bool,
    recovery_deadline: Option<Instant>,
}

impl LivePane {
    fn restored(snapshot: Snapshot, scrollback: usize) -> Result<Self, String> {
        let mut restore = crate::restore::restore(&snapshot);
        let captured = restore.cursor.0 >= restore.width;
        if captured {
            restore.cursor = (0, 0);
            restore.cursor_visible = false;
            restore.pending.clear();
        }
        let mut terminal = Terminal::restored(&restore, scrollback).map_err(|e| e.to_string())?;
        terminal.set_extended_keys_csi_u(snapshot.state.modes.extended_keys_csi_u.unwrap_or(false));
        Ok(Self {
            size: (snapshot.state.width, snapshot.state.height),
            offset: (snapshot.state.left, snapshot.state.top),
            terminal,
            captured,
            needs_capture: false,
            recovery_deadline: None,
        })
    }

    fn resume_after_recovery(
        &mut self,
        bytes: &[u8],
        deadline: Option<Instant>,
    ) -> Result<(), String> {
        self.output(bytes)?;
        // A snapshot with no pending output completes this recovery, including
        // an idle margin snapshot. Later output starts a fresh round trip.
        self.recovery_deadline = if self.needs_capture { deadline } else { None };
        Ok(())
    }

    fn output(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.captured || self.needs_capture {
            self.needs_capture |= !bytes.is_empty();
        } else {
            (_, self.needs_capture) = crate::stream::write_until_wrap(&mut self.terminal, bytes)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

struct WindowState {
    layout: layout::Change,
    terminals: Vec<(PaneId, LivePane)>,
    geometry_changed: bool,
    settle_at: Option<Instant>,
    generation: u64,
    inputs: Vec<InputRequest>,
}

impl WindowState {
    fn encode_ready_inputs(&mut self, changed: &mut Vec<PaneId>) {
        self.inputs.retain(|request| {
            let terminal = self
                .terminals
                .iter_mut()
                .find(|(id, _)| *id == request.pane)
                .filter(|(id, _)| self.layout.panes.iter().any(|pane| pane.id == *id));
            if let Some((_, terminal)) = terminal {
                // Output received during recovery has not yet been applied.
                // Wait for its authoritative modes rather than guessing.
                if terminal.needs_capture {
                    return true;
                }
                let before = terminal.terminal.scrolled_row();
                let result = terminal
                    .terminal
                    .encode_input(&request.input)
                    .map_err(|e| e.to_string());
                let after = terminal.terminal.scrolled_row();
                if before.ok() != after.ok() && !changed.contains(&request.pane) {
                    changed.push(request.pane);
                }
                let _ = request.reply.send(result);
            } else {
                let _ = request
                    .reply
                    .send(Err("The input pane is no longer visible".into()));
            }
            false
        });
    }

    fn ready(&self) -> bool {
        !self.geometry_changed
            || self.layout.panes.iter().all(|pane| {
                self.terminals
                    .iter()
                    .any(|(id, terminal)| *id == pane.id && !terminal.needs_capture)
            })
    }

    fn change_layout(&mut self, change: layout::Change, scrollback: usize) -> Result<(), String> {
        for pane in &change.panes {
            if let Some((_, terminal)) = self.terminals.iter_mut().find(|(id, _)| *id == pane.id) {
                terminal.needs_capture = true;
            } else {
                self.terminals.push((
                    pane.id,
                    LivePane {
                        terminal: Terminal::new(pane.cols, pane.rows, scrollback)
                            .map_err(|e| e.to_string())?,
                        size: (pane.cols, pane.rows),
                        offset: (pane.left, pane.top),
                        captured: false,
                        needs_capture: true,
                        recovery_deadline: None,
                    },
                ));
            }
        }
        self.layout = change;
        self.generation += 1;
        self.geometry_changed = true;
        // Match the quiet interval exercised by the resize fidelity harness.
        self.settle_at = Some(Instant::now() + Duration::from_millis(100));
        Ok(())
    }
}

impl Panes {
    /// Starts keeping the panes in `layout` up to date from `client`, a
    /// control-mode client (`tmux -C`) with piped standard input and output,
    /// freshly attached with `-f no-output,ignore-size,new-layouts`. Its initial reply
    /// must not have been read. Requires a numbered server version >=3.3,
    /// refreshes visible membership on that connection,
    /// then captures existing contents before enabling
    /// live output; startup is bounded by a ten-second capture deadline.
    /// Each pane keeps about `scrollback` lines of history.
    ///
    /// `wake` is called on the worker thread whenever an update is ready, and
    /// once more when the worker has finished.
    pub fn start(
        client: Child,
        layout: &layout::Change,
        scrollback: usize,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        Self::start_checked(client, layout, None, scrollback, wake)
    }

    /// Starts a product connection whose selected window must still belong to
    /// the verified source session when capture begins on that client.
    pub fn start_in_session(
        client: Child,
        layout: &layout::Change,
        session: u32,
        scrollback: usize,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        Self::start_checked(client, layout, Some(session), scrollback, wake)
    }

    fn start_checked(
        mut client: Child,
        layout: &layout::Change,
        session: Option<u32>,
        scrollback: usize,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let seed = match crate::seed::attach(&mut client, layout.window, session, scrollback) {
            Ok(seed) => seed,
            Err(error) => {
                let _ = client.kill();
                let _ = client.wait();
                return Err(error);
            }
        };
        let crate::seed::Seed {
            stdin,
            stdout,
            decoder,
            snapshots,
            events,
            layout,
        } = seed;
        let prefix = format!("%layout-change @{} ", layout.window);
        let mut queue = VecDeque::new();
        let mut visible: Vec<_> = layout.panes.iter().map(|pane| pane.id).collect();
        for event in events {
            match event {
                crate::seed::Event::Output(pane, bytes) if visible.contains(&pane) => {
                    queue.push_back(Event::Output(pane, bytes))
                }
                crate::seed::Event::Layout(line) if line.starts_with(prefix.as_bytes()) => {
                    match layout::parse(&line) {
                        Ok(change) => {
                            visible = change.panes.iter().map(|pane| pane.id).collect();
                            queue.push_back(Event::Layout(change));
                        }
                        Err(error) => {
                            let _ = client.kill();
                            let _ = client.wait();
                            return Err(io::Error::other(error));
                        }
                    }
                }
                _ => {}
            }
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queued: queue.iter().map(Event::size).sum(),
                queue,
                ..State::default()
            }),
            changed: Condvar::new(),
        });
        let (commands, requests) = mpsc::channel();
        // Should a thread fail to start, dropping `panes` stops what did.
        let mut panes = Self {
            shared,
            control: Arc::new(Control {
                commands,
                pending: Mutex::new(VecDeque::new()),
            }),
            input: Mutex::new(()),
            source: session.map(|session| (session, layout.window)),
            client: Mutex::new(client),
            threads: Vec::new(),
        };
        let writer = spawn("tmux writer", {
            let control = Arc::clone(&panes.control);
            let shared = Arc::clone(&panes.shared);
            move || write_commands(stdin, requests, &control, &shared)
        });
        match writer {
            Ok(writer) => panes.threads.push(writer),
            Err(error) => {
                let _ = panes
                    .client
                    .get_mut()
                    .unwrap_or_else(|e| e.into_inner())
                    .kill();
                return Err(error);
            }
        }
        let reader = spawn("tmux reader", {
            let shared = Arc::clone(&panes.shared);
            let window = layout.window;
            move || read(&shared, stdout, decoder, window, session, visible)
        });
        match reader {
            Ok(reader) => panes.threads.push(reader),
            Err(error) => {
                // Its output is closed unread, so the client might never exit.
                let _ = panes
                    .client
                    .get_mut()
                    .unwrap_or_else(|e| e.into_inner())
                    .kill();
                return Err(error);
            }
        }
        // tmux does not emit layout-change for every content-geometry change
        // (for example, switching top to bottom pane border status). Its format
        // subscriptions report these changes, at most once per second. Also
        // refresh the source server's extended-key format when it changes.
        panes.control.send(&format!(
            "refresh-client -B 'content-geometry:@{}:#{{P:#{{pane_id}}/#{{pane_left}}/#{{pane_top}}/#{{pane_width}}/#{{pane_height}};}}/#{{extended-keys-format}}'\n",
            layout.window,
        ), ReplyOwner::GeometrySubscription)?;
        if session.is_some() {
            panes.control.send(
                "refresh-client -B 'source-window::#{session_id}/#{W:#{window_id};}'\n",
                ReplyOwner::SourceSubscription,
            )?;
        }
        let (ready, initialized) = mpsc::channel();
        panes.threads.push(spawn("terminal worker", {
            let shared = Arc::clone(&panes.shared);
            let control = Arc::clone(&panes.control);
            move || {
                work(
                    &shared, &control, &layout, snapshots, scrollback, &wake, ready,
                )
            }
        })?);
        initialized
            .recv_timeout(crate::seed::TIMEOUT)
            .map_err(|error| io::Error::other(format!("initializing terminal worker: {error}")))?
            .map_err(io::Error::other)?;
        Ok(panes)
    }

    /// Takes the waiting update, if there is one.
    pub fn take_update(&self) -> Option<Update> {
        let update = self.shared.lock().update.take();
        if update.is_some() {
            // The worker may have changes waiting for this slot.
            self.shared.changed.notify_all();
        }
        update
    }

    /// Gives back screens the user interface has replaced, so the worker can
    /// reuse their allocations.
    pub fn recycle(&self, screens: impl IntoIterator<Item = Screen>) {
        self.shared.lock().spare.extend(screens);
    }

    /// Sends `bytes` to `pane` as literal input.
    pub fn send_input(&self, pane: PaneId, bytes: &[u8]) -> io::Result<()> {
        // A whole input call stays ordered with respect to other input,
        // even when recovery commands run between its chunks.
        let _input = self.input.lock().unwrap_or_else(|e| e.into_inner());
        for chunk in bytes.chunks(INPUT_CHUNK) {
            let unavailable = {
                let state = self.shared.lock();
                state.closed || state.cancelled
            };
            if unavailable {
                return Err(io::Error::other("the control client has exited"));
            }
            if self.shared.lock().gone.contains(&pane) {
                return Err(io::Error::other(format!("pane %{} has exited", pane.0)));
            }
            if let Some(failure) = self.failure() {
                return Err(io::Error::other(failure));
            }
            let line = match self.source {
                Some((session, window)) => {
                    command::send_bytes_in_session(session, window, pane, chunk)
                }
                None => command::send_bytes(pane, chunk),
            };
            if let Err(error) = self.control.send(&line, ReplyOwner::Input(pane)) {
                {
                    let mut state = self.shared.lock();
                    state
                        .transport_failure
                        .get_or_insert_with(|| format!("writing to control client: {error}"));
                }
                // A timed-out write may still be in the pipe. Kill this
                // connection before returning so it cannot deliver it later.
                if error.kind() == io::ErrorKind::TimedOut {
                    self.shared.lock().cancelled = true;
                    let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = client.kill();
                    let _ = client.wait();
                    drop(client);
                    self.shutdown();
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Encode a key or paste using this pane's modes on its owning worker,
    /// then deliver it as one ordered input operation. Call off the UI thread.
    /// Recovery may delay encoding, bounded by the same capture deadline.
    pub fn send_encoded_input(&self, pane: PaneId, input: Input) -> io::Result<()> {
        if input.size() > INPUT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Input exceeds the 4 MiB queue limit",
            ));
        }
        let (reply, encoded) = mpsc::channel();
        let event = Event::Encode(InputRequest { pane, input, reply });
        {
            let mut state = self.shared.lock();
            if state.closed || state.cancelled || state.finished || state.failure.is_some() {
                return Err(io::Error::other("Terminal connection is unavailable"));
            }
            state.queued += event.size();
            state.queue.push_back(event);
        }
        self.shared.changed.notify_all();
        let bytes = encoded
            .recv_timeout(crate::seed::TIMEOUT)
            .map_err(|error| io::Error::other(format!("Waiting for input encoding: {error}")))?
            .map_err(io::Error::other)?;
        self.send_input(pane, &bytes)
    }

    /// Declares this client's size, as an ordinary tmux client in a terminal
    /// of `cols` by `rows` would. tmux then sizes the session's windows by its
    /// `window-size` option, counting this client from now on.
    pub fn set_size(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.control.send(
            &format!(
                "refresh-client -f '!ignore-size' -C {}x{}\n",
                cols.max(1),
                rows.max(1)
            ),
            ReplyOwner::Size,
        )
    }

    /// Returns a terminal recovery or input transport failure.
    pub fn failure(&self) -> Option<String> {
        let state = self.shared.lock();
        state
            .transport_failure
            .clone()
            .or_else(|| state.failure.clone())
    }

    /// Returns whether the client has exited, the worker has handed over
    /// every change its output made, and the last update has been taken.
    pub fn is_finished(&self) -> bool {
        let state = self.shared.lock();
        state.finished && state.update.is_none()
    }

    /// Cancel before releasing the UI's reference: an input writer may hold
    /// another reference while blocked on this client's pipe. Only the owned
    /// control client is terminated if it cannot detach promptly.
    pub(crate) fn shutdown(&self) {
        self.shared.lock().cancelled = true;
        self.shared.changed.notify_all();
        let _ = self.control.commands.send(WriteRequest::Close);
        let mut client = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match client.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => break,
            }
        }
        let _ = client.kill();
        let _ = client.wait();
    }
}

impl Drop for Panes {
    fn drop(&mut self) {
        // Closing standard input detaches the client, which then exits and
        // ends the reader with end of file.
        self.shutdown();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking thread leaves the state consistent: every change is a
        // single assignment or queue operation.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed.wait(state).unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(body)
}

/// Reads and decodes the client's output, queueing each pane's output for the
/// worker, until the client exits.
fn read(
    shared: &Shared,
    mut stdout: impl Read,
    mut decoder: Decoder,
    window: u32,
    session: Option<u32>,
    mut visible: Vec<PaneId>,
) {
    let layout_prefix = format!("%layout-change @{window} ");
    let removed = format!("%unlinked-window-close @{window}");
    let mut buffer = vec![0; READ_SIZE];
    let mut reply = Reply::default();
    let mut from_client = false;
    loop {
        let read = match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                shared
                    .lock()
                    .transport_failure
                    .get_or_insert_with(|| format!("reading from control client: {error}"));
                break;
            }
        };
        let mut batch: Vec<Event> = Vec::new();
        let mut exited = false;
        let decoded = decoder.decode(&buffer[..read], |message| match message {
            Message::Output { pane, data } if visible.contains(&pane) => match batch.last_mut() {
                Some(Event::Output(last, bytes)) if *last == pane => bytes.extend_from_slice(data),
                _ => batch.push(Event::Output(pane, data.to_vec())),
            },
            Message::Begin { flags, .. } => {
                reply = Reply::default();
                from_client = flags & 1 != 0;
            }
            Message::ReplyLine(line) if from_client => reply.lines.push(line.to_vec()),
            Message::End { .. } | Message::Error { .. } => {
                reply.ok = matches!(message, Message::End { .. });
                if from_client {
                    batch.push(Event::Reply(std::mem::take(&mut reply)));
                }
            }
            Message::Notification(line) if line.starts_with(layout_prefix.as_bytes()) => {
                match layout::parse(line) {
                    Ok(change) => {
                        visible = change.panes.iter().map(|pane| pane.id).collect();
                        batch.push(Event::Layout(change));
                    }
                    Err(error) => {
                        shared.lock().failure = Some(format!("invalid window layout: {error}"));
                        exited = true;
                    }
                }
            }
            Message::Notification(line)
                if line.starts_with(b"%subscription-changed content-geometry ") =>
            {
                batch.push(Event::Geometry);
            }
            Message::Notification(line)
                if session.is_some()
                    && line.starts_with(b"%subscription-changed source-window ") =>
            {
                let value = std::str::from_utf8(line)
                    .ok()
                    .and_then(|line| line.split_once(" : "));
                let member = value
                    .and_then(|(_, value)| value.split_once('/'))
                    .is_some_and(|(source, windows)| {
                        source == format!("${}", session.unwrap())
                            && windows.split(';').any(|id| id == format!("@{window}"))
                    });
                if !member {
                    shared.lock().failure = Some("Selected window left the source session".into());
                    exited = true;
                }
            }
            Message::Notification(line) if line == b"%exit" || line.starts_with(b"%exit ") => {
                if let Some(reason) = line.strip_prefix(b"%exit ").filter(|s| !s.is_empty()) {
                    shared.lock().transport_failure.get_or_insert_with(|| {
                        format!("tmux disconnected: {}", String::from_utf8_lossy(reason))
                    });
                }
                exited = true;
            }
            Message::Notification(line) if line == removed.as_bytes() => {
                shared.lock().failure = Some("Selected window left the source session".into());
                exited = true;
            }
            _ => {}
        });
        if let Err(error) = decoded {
            shared
                .lock()
                .transport_failure
                .get_or_insert_with(|| format!("decoding control client output: {error}"));
            exited = true;
        }
        let size: usize = batch.iter().map(Event::size).sum();
        let mut state = shared.lock();
        while state.queued > QUEUE_LIMIT && !state.cancelled {
            state = shared.wait(state);
        }
        if state.cancelled {
            break;
        }
        state.queued += size;
        state.queue.extend(batch);
        drop(state);
        shared.changed.notify_all();
        if exited {
            break;
        }
    }
    shared.close();
    // A control client exits only once its output has been read: one whose
    // output is closed while it still has some to write stays attached and
    // keeps its server running (tmux 3.7c). So read to the end, discarding.
    while matches!(stdout.read(&mut buffer), Ok(read) if read > 0) {}
}

/// Applies queued output to each pane's terminal and publishes updates, until
/// the client has exited and the queue is empty.
fn work(
    shared: &Shared,
    control: &Control,
    layout: &layout::Change,
    mut snapshots: Vec<Snapshot>,
    scrollback: usize,
    wake: &dyn Fn(),
    ready: mpsc::Sender<Result<(), String>>,
) {
    // Engine handles are not `Send`, so they are made on this thread.
    let terminals: Result<Vec<_>, _> = layout
        .panes
        .iter()
        .map(|pane| {
            let index = snapshots
                .iter()
                .position(|snapshot| snapshot.pane == pane.id)
                .ok_or_else(|| format!("no initial snapshot for pane %{}", pane.id.0))?;
            LivePane::restored(snapshots.swap_remove(index), scrollback)
                .map(|terminal| (pane.id, terminal))
        })
        .collect();
    let terminals = match terminals {
        Ok(terminals) => terminals,
        Err(error) => {
            shared.lock().finished = true;
            shared.changed.notify_all();
            let _ = ready.send(Err(error));
            wake();
            return;
        }
    };
    drop(snapshots);
    let mut window = WindowState {
        layout: layout.clone(),
        terminals,
        geometry_changed: false,
        settle_at: None,
        generation: 0,
        inputs: Vec::new(),
    };
    let _ = ready.send(Ok(()));
    let mut recovering: Option<(Recovery, Instant, u64)> = None;
    let mut capture_unavailable = false;
    let mut changed: Vec<PaneId> = layout.panes.iter().map(|pane| pane.id).collect();
    let mut state = shared.lock();
    loop {
        if state.cancelled || state.failure.is_some() {
            break;
        }
        if !window.inputs.is_empty() {
            drop(state);
            window.encode_ready_inputs(&mut changed);
            state = shared.lock();
        }
        if capture_unavailable && state.closed && state.queue.is_empty() && !window.ready() {
            // The new geometry cannot be verified without another capture.
            // Preserve the previous display and report transport failure.
            changed.clear();
        }
        if window
            .settle_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            window.settle_at = None;
        }
        if !capture_unavailable
            && window.settle_at.is_none()
            && recovering.is_none()
            && let Some((pane, terminal)) = window.terminals.iter_mut().find(|(pane, terminal)| {
                terminal.needs_capture
                    && window
                        .layout
                        .panes
                        .iter()
                        .any(|visible| visible.id == *pane)
            })
        {
            let (recovery, line) = Recovery::start(*pane, scrollback);
            let deadline = *terminal
                .recovery_deadline
                .get_or_insert_with(|| Instant::now() + crate::seed::TIMEOUT);
            drop(state);
            let sent = control.send(&line, ReplyOwner::Recovery);
            state = shared.lock();
            if let Err(error) = sent {
                if state.transport_failure.is_some() {
                    capture_unavailable = true;
                } else {
                    state.failure = Some(error.to_string());
                    break;
                }
            } else {
                recovering = Some((recovery, deadline, window.generation));
            }
        }
        if !state.closed
            && recovering
                .as_ref()
                .is_some_and(|(_, deadline, _)| Instant::now() >= *deadline)
        {
            state.failure = Some("timed out recovering the pane".into());
            break;
        }
        if (!changed.is_empty() || window.geometry_changed)
            && state.update.is_none()
            && !state.cancelled
            && window.ready()
        {
            let mut spare = std::mem::take(&mut state.spare);
            drop(state);
            let mut update = Vec::with_capacity(changed.len());
            for pane in changed.drain(..) {
                let Some((_, terminal)) = window.terminals.iter_mut().find(|(id, _)| *id == pane)
                else {
                    continue;
                };
                let mut screen = spare.pop().unwrap_or_default();
                if terminal.terminal.read_screen(&mut screen).is_ok() {
                    update.push((pane, screen));
                }
            }
            state = shared.lock();
            state.spare.extend(spare);
            state.update = Some(Update {
                layout: window
                    .layout
                    .panes
                    .iter()
                    .filter_map(|pane| {
                        window.terminals.iter().find(|(id, _)| *id == pane.id).map(
                            |(_, terminal)| PaneLayout {
                                id: pane.id,
                                left: terminal.offset.0,
                                top: terminal.offset.1,
                                cols: terminal.size.0,
                                rows: terminal.size.1,
                            },
                        )
                    })
                    .collect(),
                screens: update,
            });
            window.geometry_changed = false;
            drop(state);
            wake();
            state = shared.lock();
        } else if !window.geometry_changed
            && let Some(joining) = joining(&state, &changed)
        {
            // A pane that changed after the waiting update was made, and is
            // not in it, joins it now, so that a quiet pane's change, such as
            // a key's echo, does not wait a frame behind a busy neighbour.
            let mut spare = std::mem::take(&mut state.spare);
            drop(state);
            let mut copies = Vec::with_capacity(joining.len());
            for &pane in &joining {
                let Some((_, terminal)) = window.terminals.iter_mut().find(|(id, _)| *id == pane)
                else {
                    continue;
                };
                let mut screen = spare.pop().unwrap_or_default();
                if terminal.terminal.read_screen(&mut screen).is_ok() {
                    copies.push((pane, screen));
                }
            }
            state = shared.lock();
            if let Some(waiting) = state.update.as_mut() {
                changed.retain(|pane| !joining.contains(pane));
                waiting.screens.extend(copies);
            } else {
                // Taken meanwhile: the panes stay changed, and the next
                // update carries them.
                spare.extend(copies.into_iter().map(|(_, screen)| screen));
            }
            state.spare.extend(spare);
        } else if !state.cancelled
            && let Some(event) = state.queue.pop_front()
        {
            state.queued -= event.size();
            drop(state);
            shared.changed.notify_all();
            let result = apply(
                event,
                &mut window,
                &mut recovering,
                control,
                scrollback,
                &mut changed,
            );
            state = shared.lock();
            match result {
                Ok(Some(pane)) => {
                    window.layout.panes.retain(|rect| rect.id != pane);
                    window.geometry_changed = true;
                    changed = window.layout.panes.iter().map(|rect| rect.id).collect();
                    if !state.gone.contains(&pane) {
                        state.gone.push(pane);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if state.transport_failure.is_some() {
                        // Recovery can no longer issue commands, but other
                        // panes may still have valid final output queued.
                        capture_unavailable = true;
                        recovering = None;
                    } else {
                        state.failure = Some(error);
                        break;
                    }
                }
            }
            window.terminals.retain(|(id, _)| {
                window.layout.panes.iter().any(|p| p.id == *id)
                    || recovering.as_ref().is_some_and(|(r, _, _)| r.pane() == *id)
            });
        } else if state.cancelled || (state.closed && changed.is_empty() && recovering.is_none()) {
            // Once the client has exited, the last changes still wait for the
            // user interface to take the update before them.
            state.finished = true;
            drop(state);
            wake();
            return;
        } else if let Some((_, deadline, _)) = &recovering {
            if state.closed {
                state
                    .transport_failure
                    .get_or_insert_with(|| "control client exited during pane recovery".into());
                capture_unavailable = true;
                recovering = None;
                continue;
            }
            state = shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        } else if let Some(deadline) = window.settle_at {
            state = shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        } else {
            state = shared.wait(state);
        }
    }
    state.finished = true;
    drop(state);
    wake();
}

fn apply(
    event: Event,
    window: &mut WindowState,
    recovering: &mut Option<(Recovery, Instant, u64)>,
    control: &Control,
    scrollback: usize,
    changed: &mut Vec<PaneId>,
) -> Result<Option<PaneId>, String> {
    let terminals = &mut window.terminals;
    match event {
        Event::Encode(request) => window.inputs.push(request),
        Event::Layout(change) => {
            if change.window == window.layout.window {
                window.change_layout(change, scrollback)?;
                *changed = window.layout.panes.iter().map(|pane| pane.id).collect();
            }
        }
        Event::Geometry => {
            window.change_layout(window.layout.clone(), scrollback)?;
            *changed = window.layout.panes.iter().map(|pane| pane.id).collect();
        }
        Event::Output(pane, bytes) => {
            if let Some((recovery, _, _)) = recovering.as_mut()
                && recovery.pane() == pane
            {
                recovery.output(&bytes);
                return Ok(None);
            }
            if let Some((_, terminal)) = terminals.iter_mut().find(|(id, _)| *id == pane) {
                terminal.output(&bytes)?;
                if terminal.needs_capture {
                    changed.retain(|id| *id != pane);
                } else if !changed.contains(&pane) {
                    changed.push(pane);
                }
            }
        }
        Event::Reply(reply) => {
            let mut pending = control.pending.lock().unwrap_or_else(|e| e.into_inner());
            match pending.front() {
                Some(ReplyOwner::Input(pane)) => {
                    let pane = *pane;
                    pending.pop_front();
                    if missing_pane(&reply, pane) {
                        terminals.retain(|(id, _)| *id != pane);
                        changed.retain(|id| *id != pane);
                        return Ok(Some(pane));
                    }
                    if !reply.ok {
                        let text = reply
                            .lines
                            .iter()
                            .map(|line| String::from_utf8_lossy(line))
                            .collect::<Vec<_>>()
                            .join("; ");
                        return Err(format!("tmux rejected pane input: {text}"));
                    }
                }
                Some(ReplyOwner::GeometrySubscription) => {
                    pending.pop_front();
                    if !reply.ok {
                        return Err("tmux rejected the content geometry subscription".into());
                    }
                }
                Some(ReplyOwner::SourceSubscription) => {
                    pending.pop_front();
                    if !reply.ok {
                        return Err("tmux rejected the source window subscription".into());
                    }
                }
                // A rejected size leaves the window as it was; the view scales.
                Some(ReplyOwner::Size) => {
                    pending.pop_front();
                }
                Some(ReplyOwner::Recovery) => {
                    let (recovery, _, generation) =
                        recovering.as_mut().ok_or("reply without a pane recovery")?;
                    let pane = recovery.pane();
                    let missing = missing_pane(&reply, pane);
                    match recovery.reply(reply).map_err(|e| e.to_string())? {
                        Step::Wait => {}
                        Step::Send(line) => {
                            pending.pop_front();
                            control
                                .send(&line, ReplyOwner::Recovery)
                                .map_err(|e| e.to_string())?;
                        }
                        Step::Done(rebuilt) => {
                            pending.pop_front();
                            let pane = rebuilt.snapshot.pane;
                            let (_, terminal) = terminals
                                .iter_mut()
                                .find(|(id, _)| *id == pane)
                                .ok_or("recovery completed for an unknown pane")?;
                            let deadline = terminal.recovery_deadline;
                            let geometry = (terminal.size, terminal.offset);
                            let scrolled = terminal
                                .terminal
                                .scrolled_row()
                                .map_err(|e| e.to_string())?;
                            let mut restored = LivePane::restored(rebuilt.snapshot, scrollback)?;
                            restored
                                .terminal
                                .inherit_pointer_gestures(&terminal.terminal);
                            *terminal = restored;
                            // A snapshot can observe an option-only resize before
                            // its once-per-second subscription notification. Do
                            // not join this screen to an older waiting rectangle.
                            window.geometry_changed |= geometry != (terminal.size, terminal.offset);
                            terminal.resume_after_recovery(&rebuilt.output, deadline)?;
                            if let Some(row) = scrolled {
                                terminal.terminal.restore_scrolled_row(row);
                            }
                            if *generation != window.generation {
                                terminal.needs_capture = true;
                                terminal.recovery_deadline = deadline;
                            }
                            if terminal.needs_capture {
                                changed.retain(|id| *id != pane);
                            } else if !changed.contains(&pane) {
                                changed.push(pane);
                            }
                            *recovering = None;
                        }
                        Step::Failed if missing => {
                            pending.pop_front();
                            terminals.retain(|(id, _)| *id != pane);
                            changed.retain(|id| *id != pane);
                            *recovering = None;
                            return Ok(Some(pane));
                        }
                        Step::Failed => return Err("tmux pane recovery command failed".into()),
                    }
                }
                None => return Err("unexpected control command reply".into()),
            }
        }
    }
    Ok(None)
}

fn missing_pane(reply: &Reply, pane: PaneId) -> bool {
    !reply.ok
        && reply
            .lines
            .iter()
            .any(|line| line == format!("can't find pane: %{}", pane.0).as_bytes())
}

/// Returns the panes in `changed` that the waiting update does not hold, or
/// `None` when there are none, no update waits or the panes are being dropped.
fn joining(state: &State, changed: &[PaneId]) -> Option<Vec<PaneId>> {
    let waiting = state.update.as_ref().filter(|_| !state.cancelled)?;
    let absent: Vec<PaneId> = changed
        .iter()
        .copied()
        .filter(|pane| !waiting.screens.iter().any(|(id, _)| id == pane))
        .collect();
    (!absent.is_empty()).then_some(absent)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use tmux_workspaces_test_support::TmuxServer;

    use super::*;

    #[test]
    fn shutdown_unblocks_a_stopped_clients_writer_without_stopping_its_session() {
        let server = TmuxServer::start().unwrap();
        let layout = layout(&server);
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let pid = client.id();
        let panes = Arc::new(Panes::start(client, &layout, 100, || {}).unwrap());
        assert!(
            std::process::Command::new("kill")
                .args(["-STOP", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
        let sending = Arc::clone(&panes);
        let pane = layout.panes[0].id;
        let (done, received) = mpsc::channel();
        let writer = thread::spawn(move || {
            let result = sending.send_input(pane, &vec![b'x'; 256 * 1024]);
            let _ = done.send(result);
        });
        assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
        panes.shutdown();
        assert!(
            received
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_err()
        );
        writer.join().unwrap();
        drop(panes);
        server
            .run(&["has-session", "-t", TmuxServer::SESSION])
            .unwrap();
    }

    #[test]
    fn a_timed_out_input_terminates_its_client_before_returning() {
        let server = TmuxServer::start().unwrap();
        let layout = layout(&server);
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let mut panes = Panes::start(client, &layout, 100, || {}).unwrap();
        // Hold the command before acknowledgement, independent of the OS
        // pipe capacity or the server resuming a stopped control client.
        let (commands, _pending) = mpsc::channel();
        let original = std::mem::replace(
            &mut panes.control,
            Arc::new(Control {
                commands,
                pending: Mutex::new(VecDeque::new()),
            }),
        );
        let pane = layout.panes[0].id;
        let error = panes.send_input(pane, b"timed-out input").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(panes.client.lock().unwrap().try_wait().unwrap().is_some());
        assert!(panes.failure().unwrap().contains("timed out writing"));
        assert!(panes.send_input(pane, b"later").is_err());
        // Let Drop signal the real writer as well as the held test queue.
        panes.control = original;
        server
            .run(&["has-session", "-t", TmuxServer::SESSION])
            .unwrap();
    }

    #[test]
    fn reader_queues_only_selected_window_output_in_layout_order() {
        let shared = Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        };
        let wire = b"%output %8 hidden\n%output %1 first\n%layout-change @0 abcd,80x24,0,0,2 abcd,80x24,0,0,2 *\n%output %1 removed\n%output %2 second\n";
        read(
            &shared,
            wire.as_slice(),
            Decoder::new(),
            0,
            None,
            vec![PaneId(1)],
        );
        let state = shared.lock();
        assert_eq!(state.queue.len(), 3);
        assert!(matches!(state.queue[0], Event::Output(PaneId(1), ref bytes) if bytes == b"first"));
        assert!(matches!(state.queue[1], Event::Layout(_)));
        assert!(
            matches!(state.queue[2], Event::Output(PaneId(2), ref bytes) if bytes == b"second")
        );
    }

    #[test]
    fn reader_keeps_exit_and_decode_reasons_and_preceding_output() {
        for (wire, reason) in [
            (
                "%output %1 final\n%exit too far behind\n",
                "tmux disconnected: too far behind",
            ),
            (
                "%output %1 final\n%output invalid\n",
                "decoding control client output: malformed control-mode line: %output without a pane",
            ),
        ] {
            let shared = Shared {
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            };
            read(
                &shared,
                wire.as_bytes(),
                Decoder::new(),
                0,
                None,
                vec![PaneId(1)],
            );
            let state = shared.lock();
            assert_eq!(state.transport_failure.as_deref(), Some(reason));
            assert!(state.closed);
            assert!(
                matches!(state.queue.front(), Some(Event::Output(PaneId(1), bytes)) if bytes == b"final")
            );
        }
    }

    #[test]
    fn reader_retries_interruption_and_reports_io_failure() {
        struct InterruptedThenFailed(bool);
        impl Read for InterruptedThenFailed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                if std::mem::take(&mut self.0) {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Err(io::Error::other("fixture read failed"))
                }
            }
        }
        let shared = Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        };
        read(
            &shared,
            InterruptedThenFailed(true),
            Decoder::new(),
            0,
            None,
            vec![],
        );
        assert_eq!(
            shared.lock().transport_failure.as_deref(),
            Some("reading from control client: fixture read failed")
        );
    }

    #[test]
    fn successful_recovery_does_not_lend_its_deadline_to_the_next_margin() {
        let expired = Instant::now() - Duration::from_secs(1);
        let mut terminal = LivePane {
            terminal: Terminal::new(80, 24, 0).unwrap(),
            size: (80, 24),
            offset: (0, 0),
            captured: false,
            needs_capture: false,
            recovery_deadline: None,
        };
        terminal
            .resume_after_recovery(b"restored\r\n", Some(expired))
            .unwrap();
        assert_eq!(terminal.recovery_deadline, None);
        // A new full-width row, already queued or newly received, gets a
        // fresh deadline when the worker starts its recovery.
        terminal.output(&[b'x'; 80]).unwrap();
        assert!(terminal.needs_capture);
        assert_eq!(terminal.recovery_deadline, None);
        terminal
            .resume_after_recovery(b"more", Some(expired))
            .unwrap();
        assert_eq!(terminal.recovery_deadline, Some(expired));
    }

    #[test]
    fn writer_failure_is_reported_without_discarding_queued_output() {
        use std::os::unix::net::UnixStream;
        let (stdin, peer) = UnixStream::pair().unwrap();
        drop(peer);
        let (commands, requests) = mpsc::channel();
        let control = Control {
            commands,
            pending: Mutex::new(VecDeque::new()),
        };
        let shared = Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        };
        shared
            .lock()
            .queue
            .push_back(Event::Output(PaneId(1), b"last frame".to_vec()));
        let (sent, received) = mpsc::channel();
        control
            .commands
            .send(WriteRequest::Command {
                line: "send-keys\n".into(),
                owner: ReplyOwner::Input(PaneId(1)),
                sent: Some(sent),
            })
            .unwrap();
        write_commands(stdin, requests, &control, &shared);
        assert!(received.recv().unwrap().is_err());
        let state = shared.lock();
        assert!(
            state
                .transport_failure
                .as_ref()
                .unwrap()
                .contains("writing to control client")
        );
        assert!(
            state.failure.is_none(),
            "the worker must still drain output"
        );
        assert_eq!(state.queue.len(), 1);
        assert!(!state.finished);
    }

    #[test]
    fn failed_recovery_write_still_drains_a_neighbors_final_output() {
        drains_after_writer_loss(false);
    }

    #[test]
    fn exit_during_recovery_publishes_a_neighbors_last_waiting_frame() {
        drains_after_writer_loss(true);
    }

    fn drains_after_writer_loss(in_flight: bool) {
        let server = TmuxServer::start().unwrap();
        server
            .run(&["split-window", "-h", "-t", TmuxServer::SESSION])
            .unwrap();
        let layout = layout(&server);
        let mut client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let seed = crate::seed::attach(&mut client, layout.window, None, 100).unwrap();
        let snapshots = seed.snapshots.clone();
        let mut margin_snapshot = snapshots[0].clone();
        margin_snapshot.state.cursor.0 = margin_snapshot.state.width;
        let mut idle_margin = LivePane::restored(margin_snapshot, 100).unwrap();
        idle_margin
            .resume_after_recovery(b"", Some(Instant::now() - Duration::from_secs(1)))
            .unwrap();
        assert_eq!(idle_margin.recovery_deadline, None);
        idle_margin.output(b"later").unwrap();
        assert!(idle_margin.needs_capture);
        assert_eq!(idle_margin.recovery_deadline, None);
        drop(seed);
        client.kill().unwrap();
        client.wait().unwrap();
        let mut margin = b"\x1b[2J\x1b[H".to_vec();
        margin.extend(vec![b'x'; usize::from(layout.panes[0].cols)]);
        let shared = Shared {
            state: Mutex::new(State {
                queue: VecDeque::from([
                    Event::Output(layout.panes[0].id, margin.clone()),
                    Event::Output(layout.panes[1].id, b"\x1b[2J\x1b[HFINAL".to_vec()),
                ]),
                queued: margin.len() + 12,
                closed: true,
                transport_failure: Some("writing to control client: Broken pipe".into()),
                ..State::default()
            }),
            changed: Condvar::new(),
        };
        let (commands, requests) = mpsc::channel();
        let _requests = in_flight.then_some(requests);
        let control = Control {
            commands,
            pending: Mutex::new(VecDeque::new()),
        };
        let seen = Mutex::new(Vec::new());
        let hold_first = std::sync::atomic::AtomicBool::new(in_flight);
        let wake = || {
            if hold_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if let Some(update) = shared.lock().update.take() {
                seen.lock().unwrap().extend(update.screens);
            }
        };
        let (ready, _) = mpsc::channel();
        thread::scope(|scope| {
            if in_flight {
                scope.spawn(|| {
                    // Simulate a UI that has not yet drawn its waiting frame
                    // when the connection exits during recovery.
                    thread::sleep(Duration::from_millis(100));
                    if let Some(update) = shared.lock().update.take() {
                        seen.lock().unwrap().extend(update.screens);
                    }
                    shared.changed.notify_all();
                });
            }
            work(&shared, &control, &layout, snapshots, 100, &wake, ready);
        });
        let seen = seen.lock().unwrap();
        let (_, screen) = seen
            .iter()
            .rev()
            .find(|(id, _)| *id == layout.panes[1].id)
            .unwrap();
        let text: String = (0..5).map(|col| screen.cell(0, col).unwrap().0).collect();
        assert_eq!(text, "FINAL");
        let state = shared.lock();
        assert!(state.finished);
        assert!(state.queue.is_empty());
        assert!(
            state
                .transport_failure
                .as_ref()
                .unwrap()
                .contains("Broken pipe")
        );
        assert!(state.failure.is_none());
    }

    #[test]
    fn attachment_refreshes_membership_after_the_initial_probe() {
        let server = TmuxServer::start().unwrap();
        let initial = layout(&server);
        let signal = server.shell_command(&["wait-for", "-S", "split-ready"]);
        server
            .run(&[
                "split-window",
                "-h",
                "-t",
                TmuxServer::SESSION,
                &format!("printf NEW; {signal}; exec sleep 60"),
            ])
            .unwrap();
        server.run(&["wait-for", "split-ready"]).unwrap();
        let current = layout(&server);
        let added = current
            .panes
            .iter()
            .find(|p| p.id != initial.panes[0].id)
            .unwrap()
            .id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &initial, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        // No later layout change may be needed to discover the added pane.
        wait_for_row(&panes, &woken, added, "NEW");
        assert_eq!(panes.failure(), None);
    }

    #[test]
    fn live_geometry_recovers_resize_split_zoom_and_removed_panes() {
        let server = TmuxServer::start().unwrap();
        let signal = server.shell_command(&["wait-for", "-S", "first-ready"]);
        server
            .run(&[
                "respawn-pane",
                "-k",
                "-t",
                TmuxServer::SESSION,
                &format!("printf FIRST; {signal}; exec sleep 60"),
            ])
            .unwrap();
        server.run(&["wait-for", "first-ready"]).unwrap();
        server
            .run(&["set-option", "-w", "pane-border-status", "top"])
            .unwrap();
        let initial = layout(&server);
        let first = initial.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &initial, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, first, "FIRST");
        let content_layout = || {
            let mut visible = layout(&server);
            for pane in &mut visible.panes {
                let values = server
                    .run(&[
                        "display-message",
                        "-p",
                        "-t",
                        &format!("%{}", pane.id.0),
                        "#{pane_left} #{pane_top} #{pane_width} #{pane_height}",
                    ])
                    .unwrap();
                let values: Vec<u16> = values
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                (pane.left, pane.top, pane.cols, pane.rows) =
                    (values[0], values[1], values[2], values[3]);
            }
            visible
        };
        let wait = |expected: &layout::Change| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                assert_eq!(panes.failure(), None);
                assert!(
                    Instant::now() < deadline,
                    "layout did not settle: {expected:?}"
                );
                if let Some(update) = panes.take_update() {
                    // No update may pair a rebuilt screen with stale geometry.
                    for (id, screen) in &update.screens {
                        let rect = update.layout.iter().find(|p| p.id == *id).unwrap();
                        assert_eq!((screen.cols(), screen.rows()), (rect.cols, rect.rows));
                    }
                    if update.layout == expected.panes
                        && update.screens.len() == expected.panes.len()
                    {
                        let (_, screen) =
                            update.screens.iter().find(|(id, _)| *id == first).unwrap();
                        let text: String =
                            (0..5).map(|col| screen.cell(0, col).unwrap().0).collect();
                        assert_eq!(text, "FIRST");
                        break;
                    }
                }
                let _ = woken.recv_timeout(Duration::from_millis(20));
            }
        };
        server
            .run(&["set-option", "-w", "pane-border-status", "bottom"])
            .unwrap();
        wait(&content_layout());
        server
            .run(&[
                "resize-window",
                "-t",
                TmuxServer::SESSION,
                "-x",
                "120",
                "-y",
                "30",
            ])
            .unwrap();
        wait(&content_layout());
        server
            .run(&[
                "split-window",
                "-h",
                "-t",
                TmuxServer::SESSION,
                "exec sleep 60",
            ])
            .unwrap();
        let split = layout(&server);
        wait(&content_layout());
        let second = split.panes.iter().find(|pane| pane.id != first).unwrap().id;
        server
            .run(&["resize-pane", "-Z", "-t", &format!("%{}", first.0)])
            .unwrap();
        wait(&content_layout());
        server
            .run(&["resize-pane", "-Z", "-t", &format!("%{}", first.0)])
            .unwrap();
        wait(&content_layout());
        server
            .run(&["kill-pane", "-t", &format!("%{}", second.0)])
            .unwrap();
        wait(&content_layout());
    }

    #[test]
    fn attachment_publishes_existing_styles_before_applying_new_output() {
        let server = TmuxServer::start().unwrap();
        // This server-side event hook prints outside any command reply guards.
        server
            .run(&[
                "set-hook",
                "-g",
                "client-attached",
                "display-message -p hook-printed",
            ])
            .unwrap();
        let session = TmuxServer::SESSION;
        let signal = server.shell_command(&["wait-for", "-S", "seed-ready"]);
        let script =
            format!("printf '\\033[2J\\033[H\\033[45mSEEDED\\033[0m'; {signal}; exec sleep 60");
        server
            .run(&["respawn-pane", "-k", "-t", session, &script])
            .unwrap();
        server.run(&["wait-for", "seed-ready"]).unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let update = panes.take_update().unwrap();
        let (_, screen) = update.screens.iter().find(|(id, _)| *id == pane).unwrap();
        let text: String = (0..6).map(|col| screen.cell(0, col).unwrap().0).collect();
        assert_eq!(text, "SEEDED");
        assert_eq!(
            screen.cell(0, 0).unwrap().1.style.background,
            tmux_workspaces_term::Color::Palette(5)
        );
        panes.send_input(pane, b"more").unwrap();
        wait_for_row(&panes, &woken, pane, "SEEDEDmore");
    }

    #[test]
    fn an_unsafe_capture_hook_fails_attachment_without_stopping_the_server() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&[
                "set-hook",
                "-g",
                "after-capture-pane",
                "display-message -p hooked",
            ])
            .unwrap();
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let result = Panes::start(client, &layout(&server), 100, || {});
        assert!(result.err().unwrap().to_string().contains("recovery hooks"));
        assert!(
            server
                .run(&["has-session", "-t", TmuxServer::SESSION])
                .is_ok()
        );
    }

    #[test]
    fn an_initial_margin_cursor_recovers_before_applying_a_cursor_left_edit() {
        margin_cursor_edit(true, false);
    }

    #[test]
    fn a_streamed_margin_cursor_recovers_while_input_replies_arrive() {
        margin_cursor_edit(false, false);
    }

    #[test]
    fn a_margin_transition_inside_one_write_recovers_before_publishing() {
        margin_cursor_edit(false, true);
    }

    fn margin_cursor_edit(initial: bool, combined: bool) {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        let signal = server.shell_command(&["wait-for", "-S", "seed-ready"]);
        let read = "dd bs=1 count=1 of=/dev/null 2>/dev/null";
        let fill = if combined {
            "printf '%0200d\\033[5DX' 0"
        } else {
            "printf '%0200d' 0"
        };
        let start = if initial {
            format!("{fill};")
        } else {
            String::new()
        };
        let after = if initial {
            String::new()
        } else {
            format!("{read}; {fill};")
        };
        let edit = if combined {
            String::new()
        } else {
            format!("{read}; printf '\\033[5DX';")
        };
        let script = format!(
            "stty -echo -icanon; printf '\\033[2J\\033[H'; {start} {signal}; {after} {edit} exec sleep 60"
        );
        server
            .run(&["respawn-pane", "-k", "-t", session, &script])
            .unwrap();
        server.run(&["wait-for", "seed-ready"]).unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        if !initial {
            wait_for_row(&panes, &woken, pane, "");
            panes.send_input(pane, b"g").unwrap();
        }
        if !combined {
            wait_for_row(&panes, &woken, pane, &"0".repeat(200));
            for _ in 0..8 {
                panes.send_input(pane, b"g").unwrap();
            }
        }
        let expected = format!("{}X{}", "0".repeat(195), "0".repeat(4));
        wait_for_row(&panes, &woken, pane, &expected);
        assert_eq!(panes.failure(), None);
        assert!(server.run(&["has-session", "-t", session]).is_ok());
    }

    #[test]
    fn input_does_not_block_output_draining_during_recovery() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        let signal = server.shell_command(&["wait-for", "-S", "seed-ready"]);
        let received_input = server.address().0.join("received-input");
        let producer_done = received_input.with_file_name("producer-done");
        let script = format!(
            "stty -echo -icanon; {signal}; head -c 1 >/dev/null; \
             (head -c 6291456 /dev/zero | tr '\\000' x; : > '{}') & producer=$!; \
             head -c 524288 > '{}'; wait $producer; \
             printf '\\033[2J\\033[HDONE'; exec sleep 60",
            producer_done.display(),
            received_input.display()
        );
        server
            .run(&["respawn-pane", "-k", "-t", session, &script])
            .unwrap();
        server.run(&["wait-for", "seed-ready"]).unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Arc::new(
            Panes::start(client, &layout, 100, move || {
                let _ = wake.send(());
            })
            .unwrap(),
        );
        wait_for_row(&panes, &woken, pane, "");
        panes.send_input(pane, b"g").unwrap();
        let (sent, received) = mpsc::channel();
        let writing = Arc::clone(&panes);
        let input: Vec<u8> = (0..524288).map(|i| b'a' + (i % 26) as u8).collect();
        let expected = input.clone();
        let writer = thread::spawn(move || {
            let _ = sent.send(writing.send_input(pane, &input));
        });
        received
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        wait_for_row_with_diagnostics(&panes, &woken, pane, "DONE", || {
            let actual = std::fs::read(&received_input);
            let received = actual.as_ref().map(|bytes| bytes.len());
            let mismatch = actual
                .as_ref()
                .ok()
                .and_then(|bytes| bytes.iter().zip(&expected).position(|(a, b)| a != b));
            let state = panes.shared.lock();
            format!(
                "received={received:?}, first_mismatch={mismatch:?}, producer_done={}, queued_bytes={}, queued_events={}, closed={}, finished={}, waiting_update={}",
                producer_done.exists(),
                state.queued,
                state.queue.len(),
                state.closed,
                state.finished,
                state.update.is_some()
            )
        });
        assert_eq!(std::fs::read(received_input).unwrap(), expected);
        assert_eq!(panes.failure(), None);
    }

    #[test]
    fn a_stalled_recovery_reports_its_deadline() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, pane, "");
        // Change only this private fixture after the startup hook check.
        // The hook's guard is ignored, but its wait stalls the capture line;
        // the recovery must report its deadline instead of waiting forever.
        server
            .run(&[
                "set-hook",
                "-g",
                "after-refresh-client",
                "wait-for recovery-release",
            ])
            .unwrap();
        panes.send_input(pane, &[b'x'; 200]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while panes.failure().is_none() && Instant::now() < deadline {
            let _ = woken.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        let failure = panes.failure();
        server.run(&["wait-for", "-S", "recovery-release"]).unwrap();
        assert!(
            failure
                .as_ref()
                .is_some_and(|failure| failure.contains("timed out recovering")),
            "{failure:?}"
        );
    }

    #[test]
    fn local_history_scroll_survives_capture_and_typing_returns_to_live_output() {
        use tmux_workspaces_term::input::Scroll;
        let server = TmuxServer::start().unwrap();
        let script = "stty raw -echo; printf 'history-first\\r\\n'; i=0; while [ \"$i\" -lt 60 ]; do printf 'history-%02d\\r\\n' \"$i\"; i=$((i+1)); done; printf '\\033[2J\\033[Hlive-ready'; exec cat";
        server
            .run(&[
                "respawn-pane",
                "-k",
                "-t",
                TmuxServer::SESSION,
                "sh",
                "-c",
                script,
            ])
            .unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, pane, "live-ready");
        panes
            .send_encoded_input(pane, Input::Scroll(Scroll::Top))
            .unwrap();
        wait_for_row(&panes, &woken, pane, "history-first");
        // Another private-fixture client produces a wrapping line while this
        // viewer reads history. Its capture recovery must keep the local view.
        server
            .run(&[
                "send-keys",
                "-l",
                "-t",
                &format!("%{}", pane.0),
                &"x".repeat(usize::from(layout.panes[0].cols) + 5),
            ])
            .unwrap();
        wait_for_row(&panes, &woken, pane, "history-first");
        panes
            .send_encoded_input(pane, Input::Paste("typed".into()))
            .unwrap();
        wait_for_row(&panes, &woken, pane, "live-readyxxx");
        assert_eq!(panes.failure(), None);
    }

    #[test]
    fn worker_encodes_application_keys_paste_and_mouse_in_stream_order() {
        use tmux_workspaces_term::input::{Button, KeyPress, MouseAction, Pointer};
        let server = TmuxServer::start().unwrap();
        server
            .run(&["set-option", "-s", "extended-keys", "on"])
            .unwrap();
        // tmux 3.3 and 3.4 already emit CSI-u. Newer versions expose a
        // format option; select CSI-u there without skipping any input checks.
        let configurable_extended_keys = server
            .run(&["show-options", "-s"])
            .unwrap()
            .lines()
            .any(|line| line.starts_with("extended-keys-format "));
        if configurable_extended_keys {
            server
                .run(&["set-option", "-s", "extended-keys-format", "csi-u"])
                .unwrap();
        }
        // Before tmux 3.5, shifted letters keep the lowercase codepoint.
        // Read the whole server-encoded key so no byte spills into the paste.
        let extended = if configurable_extended_keys {
            "1b5b37323b3675" // CSI 72;6u
        } else {
            "1b5b3130343b3675" // CSI 104;6u
        };
        let script = "stty raw -echo; printf '\\033[>4;2m\\033[?1h\\033[2J\\033[Harrow-ready'; arrow=$(dd bs=1 count=3 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\033[2J\\033[Hextended-ready%s' \"$arrow\"; extended=$(dd bs=1 count=$1 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\033[?2004h\\033[2J\\033[Hpaste-ready%s' \"$extended\"; pasted=$(dd bs=1 count=15 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\033[?1000h\\033[?1006h\\033[2J\\033[Hpaste:%s' \"$pasted\"; mouse=$(dd bs=1 count=28 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\033[?1002h\\033[2J\\033[Hmouse:%s' \"$mouse\"; drag=$(dd bs=1 count=28 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\033[2J\\033[Hdrag:%s' \"$drag\"; exec cat";
        server
            .run(&[
                "respawn-pane",
                "-k",
                "-t",
                TmuxServer::SESSION,
                "sh",
                "-c",
                script,
                "input-fixture",
                &(extended.len() / 2).to_string(),
            ])
            .unwrap();
        let layout = layout(&server);
        let pane = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                TmuxServer::SESSION,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, pane, "arrow-ready");
        panes
            .send_encoded_input(
                pane,
                Input::Key(KeyPress {
                    key: "up".into(),
                    ..Default::default()
                }),
            )
            .unwrap();
        wait_for_row(&panes, &woken, pane, "extended-ready1b4f41");
        panes
            .send_encoded_input(
                pane,
                Input::Key(KeyPress {
                    key: "h".into(),
                    control: true,
                    shift: true,
                    ..Default::default()
                }),
            )
            .unwrap();
        wait_for_row(&panes, &woken, pane, &format!("paste-ready{extended}"));
        panes
            .send_encoded_input(pane, Input::Paste("ok\n".into()))
            .unwrap();
        wait_for_row(&panes, &woken, pane, "paste:1b5b3230307e6f6b0a1b5b3230317e");
        let pointer = Pointer {
            x: 2.5,
            y: 3.5,
            cols: layout.panes[0].cols,
            rows: layout.panes[0].rows,
            cell_width: 8,
            cell_height: 16,
            ..Default::default()
        };
        panes
            .send_encoded_input(pane, Input::Click(pointer, Button::Left))
            .unwrap();
        panes
            .send_encoded_input(pane, Input::Wheel(pointer, -1))
            .unwrap();
        wait_for_row(
            &panes,
            &woken,
            pane,
            "mouse:1b5b3c303b333b344d1b5b3c303b333b346d1b5b3c36343b333b344d",
        );
        panes
            .send_encoded_input(
                pane,
                Input::Mouse(pointer, MouseAction::Press(Button::Left)),
            )
            .unwrap();
        let moved = Pointer { x: 4.5, ..pointer };
        panes
            .send_encoded_input(
                pane,
                Input::Mouse(moved, MouseAction::Move(Some(Button::Left))),
            )
            .unwrap();
        panes
            .send_encoded_input(
                pane,
                Input::Mouse(moved, MouseAction::Release(Button::Left)),
            )
            .unwrap();
        wait_for_row(
            &panes,
            &woken,
            pane,
            "drag:1b5b3c303b333b344d1b5b3c33323b353b344d1b5b3c303b353b346d",
        );
        assert!(
            panes
                .send_encoded_input(PaneId(u32::MAX), Input::Paste("wrong destination".into()))
                .is_err()
        );
        assert_eq!(panes.failure(), None);
    }

    #[test]
    fn a_pane_removed_during_recovery_does_not_stop_its_neighbor() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        server
            .run(&["split-window", "-h", "-t", session, "exec cat"])
            .unwrap();
        let layout = layout(&server);
        let gone = layout.panes[0].id;
        let alive = layout.panes[1].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, gone, "");
        // Remove the private fixture pane precisely between pause and capture.
        server
            .run(&[
                "set-hook",
                "-g",
                "after-refresh-client",
                &format!("kill-pane -t '%{}'", gone.0),
            ])
            .unwrap();
        panes
            .send_input(gone, &vec![b'x'; usize::from(layout.panes[0].cols)])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.shared.lock().gone.contains(&gone) {
            assert!(
                Instant::now() < deadline,
                "pane was not retired: {:?}",
                panes.failure()
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(panes.send_input(gone, b"lost").is_err());
        panes.send_input(alive, b"alive").unwrap();
        wait_for_row(&panes, &woken, alive, "alive");
        assert_eq!(panes.failure(), None);
    }

    /// Waits until the pane's screen shows `text` on its first row.
    fn wait_for_row(panes: &Panes, woken: &mpsc::Receiver<()>, pane: PaneId, text: &str) {
        wait_for_row_with_diagnostics(panes, woken, pane, text, String::new);
    }

    fn wait_for_row_with_diagnostics(
        panes: &Panes,
        woken: &mpsc::Receiver<()>,
        pane: PaneId,
        text: &str,
        diagnostics: impl Fn() -> String,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last_row = None;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                woken.recv_timeout(left).is_ok(),
                "no update showed {text:?}; last_row={last_row:?}; failure: {:?}; {}",
                panes.failure(),
                diagnostics()
            );
            let Some(update) = panes.take_update() else {
                continue;
            };
            for (id, screen) in update.screens {
                let row: String = (0..screen.cols())
                    .filter_map(|col| screen.cell(0, col).map(|(text, _)| text.to_owned()))
                    .collect();
                if id == pane {
                    if row.starts_with(text) {
                        return;
                    }
                    last_row = Some(row.chars().take(80).collect::<String>());
                }
            }
        }
    }

    #[test]
    fn changes_made_while_an_update_waits_arrive_after_the_client_exits() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        let id = server
            .run(&["display-message", "-p", "-t", session, "#{pane_id}"])
            .unwrap();
        let pane = PaneId(id.trim().trim_start_matches('%').parse().unwrap());
        let layout = layout(&server);
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        wait_for_row(&panes, &woken, pane, "");
        // The first update waits, untaken, while more output arrives and the
        // client exits.
        panes.send_input(pane, b"first").unwrap();
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        panes.send_input(pane, b" second").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        server.run(&["kill-server"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.shared.lock().closed {
            assert!(Instant::now() < deadline, "the reader did not close");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(panes.send_input(pane, b"too late").is_err());
        assert!(panes.take_update().is_some());
        wait_for_row(&panes, &woken, pane, "first second");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.is_finished() {
            assert!(Instant::now() < deadline, "the worker did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(panes.failure(), None);
    }

    /// Returns the visible panes of the server's session.
    fn layout(server: &TmuxServer) -> layout::Change {
        let fields = server
            .run(&[
                "display-message",
                "-p",
                "-t",
                TmuxServer::SESSION,
                "#{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}",
            ])
            .unwrap();
        layout::parse(format!("%layout-change {fields}").as_bytes()).unwrap()
    }

    #[test]
    fn a_cancelled_reader_reads_the_clients_output_to_its_end() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec yes streaming"])
            .unwrap();
        let mut client = server.control(&["attach-session", "-t", session]).unwrap();
        // The client's output fills its pipe while nothing reads it.
        std::thread::sleep(Duration::from_millis(500));
        let shared = Shared {
            state: Mutex::new(State {
                cancelled: true,
                ..State::default()
            }),
            changed: Condvar::new(),
        };
        let stdout = client.stdout.take().unwrap();
        drop(client.stdin.take());
        // Read on another thread, so that a client that never exits fails
        // the test instead of hanging it: killing the client ends the read.
        let reader =
            std::thread::spawn(move || read(&shared, stdout, Decoder::new(), 0, None, vec![]));

        let deadline = Instant::now() + Duration::from_secs(10);
        while client.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = client.kill();
                let _ = client.wait();
                panic!("the client did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        reader.join().unwrap();
    }

    #[test]
    fn a_quiet_panes_change_joins_the_waiting_update() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        // Paced, so the echo does not queue behind a flood in a slow build.
        let stream = "while :; do echo streaming; sleep 0.02; done";
        server
            .run(&["split-window", "-v", "-t", session, "sh", "-c", stream])
            .unwrap();
        let layout = layout(&server);
        let quiet = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        wait_for_row(&panes, &woken, quiet, "");
        // An update holding the streaming pane waits, untaken, when the quiet
        // pane echoes a key. Only joining changes a waiting update, so without
        // it the echo never shows in this one.
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        panes.send_input(quiet, b"x").unwrap();
        let echoed = |update: &Update| {
            update.screens.iter().any(|(id, screen)| {
                *id == quiet && screen.cell(0, 0).is_some_and(|(t, _)| t == "x")
            })
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.shared.lock().update.as_ref().is_some_and(echoed) {
            assert!(
                Instant::now() < deadline,
                "the echo waited for the next update"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn recovered_content_geometry_never_joins_an_old_waiting_rectangle() {
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        server
            .run(&[
                "split-window",
                "-v",
                "-t",
                session,
                "sh",
                "-c",
                "while :; do echo streaming; sleep 0.02; done",
            ])
            .unwrap();
        server
            .run(&["set-option", "-w", "pane-border-status", "top"])
            .unwrap();
        let layout = layout(&server);
        let quiet = layout.panes[0].id;
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();
        wait_for_row(&panes, &woken, quiet, "");
        // Remove the fixture's subscription to reproduce the interval before its
        // next notification deterministically. This changes only this client.
        panes
            .control
            .send(
                "refresh-client -B content-geometry\n",
                ReplyOwner::Input(quiet),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes.control.pending.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        // Drain any subscription-seeded frame, then hold a neighbor-only update.
        loop {
            if panes
                .shared
                .lock()
                .update
                .as_ref()
                .is_some_and(|update| !update.screens.iter().any(|(id, _)| *id == quiet))
            {
                break;
            }
            panes.take_update();
            assert!(Instant::now() < deadline);
            let _ = woken.recv_timeout(Duration::from_millis(10));
        }
        server
            .run(&["set-option", "-w", "pane-border-status", "bottom"])
            .unwrap();
        server
            .run(&[
                "set-hook",
                "-g",
                "after-capture-pane",
                "wait-for -S recovered-geometry",
            ])
            .unwrap();
        panes
            .send_input(quiet, &vec![b'x'; usize::from(layout.panes[0].cols)])
            .unwrap();
        server.run(&["wait-for", "recovered-geometry"]).unwrap();
        while !panes.control.pending.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        loop {
            assert_eq!(panes.failure(), None);
            assert!(Instant::now() < deadline);
            if let Some(update) = panes.take_update()
                && let Some((_, screen)) = update.screens.iter().find(|(id, _)| *id == quiet)
            {
                let rectangle = update.layout.iter().find(|pane| pane.id == quiet).unwrap();
                assert_eq!(rectangle.top, 0);
                assert_eq!(
                    (rectangle.cols, rectangle.rows),
                    (screen.cols(), screen.rows())
                );
                break;
            }
            let _ = woken.recv_timeout(Duration::from_millis(10));
        }
    }

    #[test]
    fn input_echo_and_reply_order_survive_input_hooks() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&[
                "set-hook",
                "-g",
                "after-send-keys",
                "display-message -p typed",
            ])
            .unwrap();
        let session = TmuxServer::SESSION;
        server
            .run(&["respawn-pane", "-k", "-t", session, "exec cat"])
            .unwrap();
        let id = server
            .run(&["display-message", "-p", "-t", session, "#{pane_id}"])
            .unwrap();
        let pane = PaneId(id.trim().trim_start_matches('%').parse().unwrap());
        let layout = layout(&server);
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let (wake, woken) = mpsc::channel();
        let panes = Panes::start(client, &layout, 100, move || {
            let _ = wake.send(());
        })
        .unwrap();

        panes.send_input(pane, b"hello").unwrap();
        wait_for_row(&panes, &woken, pane, "hello");
        panes.send_input(pane, b" again").unwrap();
        wait_for_row(&panes, &woken, pane, "hello again");
        assert_eq!(panes.failure(), None);
        assert!(!panes.is_finished());
        drop(panes);
        assert!(server.run(&["has-session", "-t", session]).is_ok());
    }
}
