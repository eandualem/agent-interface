//! One remote terminal, with its Ghostty engine owned by a single worker.
//! The socket reader backpressures the host when the rendering worker falls behind.

use super::{Input, InputReservation, PaneId, PaneLayout, Rgba, Screen, backend::Backend};
use crate::panes::Update;
use agent_interface_roster::profiles::terminal::{Access, Close, Event, Stream, Writer};
use agent_interface_term::Terminal;
use std::{
    io,
    ops::Range,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

const REPAINT_TIMEOUT: Duration = Duration::from_secs(15);
const CLOSE_GRACE: Duration = Duration::from_secs(1);

pub(crate) const PANE: PaneId = PaneId(0);
pub(crate) fn layout(size: (u16, u16)) -> PaneLayout {
    PaneLayout {
        id: PANE,
        left: 0,
        top: 0,
        cols: size.0,
        rows: size.1,
    }
}

enum Work {
    Wire(io::Result<Event>),
    Input(Input, mpsc::Sender<io::Result<()>>),
    Rows(Range<usize>, mpsc::Sender<io::Result<Screen>>),
    Viewport,
}

#[derive(Default)]
struct State {
    latest: Option<Update>,
    spare: Option<Screen>,
    ended: Option<String>,
    paused: bool,
    size: Option<(u16, u16)>,
}

/// Why a stream ended. Only a lost connection may be opened again by itself.
pub(crate) const CONNECTION_LOST: &str = "Remote terminal connection lost";
pub(crate) const PROCESS_EXITED: &str = "Remote agent process exited";
pub(crate) const ACCESS_ENDED: &str = "Remote terminal access ended";

pub(crate) struct Remote {
    state: Arc<Mutex<State>>,
    work: SyncSender<Work>,
    close: Arc<Close>,
}

impl Remote {
    pub(crate) fn start(
        mut stream: Stream,
        writer: Writer,
        close: Close,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(State::default()));
        let close = Arc::new(close);
        let (work, pending) = mpsc::sync_channel(32);
        let size = stream.size;
        let access = stream.access;
        let worker_state = state.clone();
        let worker_close = close.clone();
        std::thread::Builder::new()
            .name("remote-terminal-grid".into())
            .spawn(move || {
                let reason = run(pending, writer, size, access, &worker_state, &wake)
                    .unwrap_or_else(|error| match error.kind() {
                        // The transport went away; the service may answer again.
                        io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe => CONNECTION_LOST.to_owned(),
                        _ => error.to_string(),
                    });
                worker_close.detach();
                worker_state.lock().unwrap().ended = Some(reason);
                wake();
            })?;
        let incoming = work.clone();
        std::thread::Builder::new()
            .name("remote-terminal-read".into())
            .spawn(move || {
                loop {
                    let frame = stream.read();
                    let ended = matches!(frame, Err(_) | Ok(Event::Ended(_)));
                    if incoming.send(Work::Wire(frame)).is_err() || ended {
                        break;
                    }
                }
            })?;
        Ok(Self { state, work, close })
    }

    fn viewport(&self, paused: Option<bool>, size: Option<(u16, u16)>) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(paused) = paused {
            state.paused = paused;
        }
        if size.is_some() {
            state.size = size;
        }
        // If full, the next event already wakes the worker to read these values.
        match self.work.try_send(Work::Viewport) {
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(io::Error::other("Remote terminal closed"))
            }
            _ => Ok(()),
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.close.detach();
    }
}

fn publish(
    terminal: &mut Terminal,
    size: (u16, u16),
    state: &Mutex<State>,
    wake: &dyn Fn(),
) -> io::Result<()> {
    let mut screen = state.lock().unwrap().spare.take().unwrap_or_default();
    terminal
        .read_screen(&mut screen)
        .map_err(io::Error::other)?;
    let mut shared = state.lock().unwrap();
    let old = shared.latest.replace(Update {
        layout: vec![layout(size)],
        screens: vec![(PANE, screen)],
    });
    let notify = old.is_none();
    if let Some(mut old) = old {
        shared.spare = old.screens.pop().map(|(_, screen)| screen);
    }
    drop(shared);
    if notify {
        wake();
    }
    Ok(())
}

fn run(
    pending: Receiver<Work>,
    mut writer: Writer,
    mut size: (u16, u16),
    access: Access,
    state: &Mutex<State>,
    wake: &dyn Fn(),
) -> io::Result<String> {
    let mut terminal = Terminal::for_stream(size.0, size.1).map_err(io::Error::other)?;
    let mut repaint = true;
    // A healthy live or paused view may be silent indefinitely.
    let mut repaint_deadline = Some(Instant::now() + REPAINT_TIMEOUT);
    let mut write_failure: Option<(Instant, io::Error)> = None;
    let mut dirty = false;
    let mut paused = false;
    let mut sent_size = Some(size);
    let mut last = Instant::now();
    let interval = Duration::from_millis(16);
    loop {
        let (hide, requested) = {
            let shared = state.lock().unwrap();
            (shared.paused, shared.size)
        };
        if write_failure.is_none() {
            let sent = (|| {
                if hide != paused {
                    writer.pause(hide)?;
                    paused = hide;
                    repaint_deadline =
                        (repaint && !paused).then(|| Instant::now() + REPAINT_TIMEOUT);
                }
                if !paused
                    && let Some(wanted) = requested
                    && sent_size != Some(wanted)
                {
                    writer.resize(wanted.0, wanted.1)?;
                    sent_size = Some(wanted);
                }
                Ok(())
            })();
            if let Err(error) = sent {
                write_failure = Some((Instant::now() + CLOSE_GRACE, error));
            }
        }
        let now = Instant::now();
        if write_failure
            .as_ref()
            .is_some_and(|(until, _)| now >= *until)
        {
            return Err(write_failure.take().unwrap().1);
        }
        if repaint_deadline.is_some_and(|until| now >= until) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Remote terminal repaint timed out",
            ));
        }
        if dirty && !repaint && now >= last + interval {
            publish(&mut terminal, size, state, wake)?;
            dirty = false;
            last = now;
        }
        let deadline = [
            repaint_deadline,
            write_failure.as_ref().map(|(until, _)| *until),
            (dirty && !repaint).then_some(last + interval),
        ]
        .into_iter()
        .flatten()
        .min();
        let work = if let Some(until) = deadline {
            match pending.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(work) => work,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        } else {
            let Ok(work) = pending.recv() else { break };
            work
        };
        match work {
            Work::Wire(Ok(Event::Output(bytes))) => {
                terminal.write_stream(&bytes);
                dirty = true;
            }
            Work::Wire(Ok(Event::Resized(cols, rows))) => {
                terminal.resize(cols, rows).map_err(io::Error::other)?;
                size = (cols, rows);
                dirty = true;
            }
            Work::Wire(Ok(Event::Dropped)) => {
                let mut replacement =
                    Terminal::for_stream(size.0, size.1).map_err(io::Error::other)?;
                replacement.invalidate_history_after(&terminal);
                terminal = replacement;
                if !repaint && !paused {
                    repaint_deadline = Some(Instant::now() + REPAINT_TIMEOUT);
                }
                repaint = true;
            }
            Work::Wire(Ok(Event::RepaintEnd)) => {
                repaint_deadline = None;
                repaint = false;
                dirty = true;
            }
            Work::Wire(end) => {
                if dirty && !repaint {
                    publish(&mut terminal, size, state, wake)?;
                }
                return match end? {
                    Event::Ended(reason) => Ok(match reason.as_str() {
                        "process_exited" => PROCESS_EXITED,
                        "access_ended" => ACCESS_ENDED,
                        _ => CONNECTION_LOST,
                    }
                    .to_owned()),
                    _ => unreachable!(),
                };
            }
            Work::Rows(rows, reply) => {
                let mut screen = Screen::default();
                let result = terminal
                    .read_rows(rows, &mut screen)
                    .map(|()| screen)
                    .map_err(io::Error::other);
                let _ = reply.send(result);
            }
            Work::Input(input, reply) => {
                let result = if write_failure.is_some() {
                    Err(io::Error::other("Remote terminal stopped accepting input"))
                } else if repaint {
                    Err(io::Error::other("Waiting for terminal repaint"))
                } else if access == Access::Watch
                    && !matches!(input, Input::Scroll(_) | Input::Wheel(..))
                {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Watch-only terminal",
                    ))
                } else {
                    match terminal.encode_input(&input).map_err(io::Error::other) {
                        Ok(bytes) if !bytes.is_empty() && access == Access::Type => {
                            // A partial write cannot be retried without duplicating input.
                            writer.input(&bytes).inspect_err(|error| {
                                write_failure = Some((
                                    Instant::now() + CLOSE_GRACE,
                                    io::Error::new(error.kind(), error.to_string()),
                                ));
                            })
                        }
                        result => result.map(|_| ()),
                    }
                };
                dirty = true;
                let _ = reply.send(result);
            }
            Work::Viewport => {}
        }
    }
    Ok("Remote terminal detached".into())
}

impl Backend for Remote {
    fn set_visible(&self, visible: bool) {
        let _ = self.viewport(Some(!visible), None);
    }
    fn take_update(&self) -> Option<Update> {
        self.state.lock().unwrap().latest.take()
    }
    fn recycle(&self, mut screens: Vec<Screen>) {
        if let Some(screen) = screens.pop() {
            self.state.lock().unwrap().spare = Some(screen);
        }
    }
    fn send_reserved_input(
        &self,
        _: PaneId,
        input: Input,
        _reservation: Option<Arc<InputReservation>>,
    ) -> io::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.work
            .send(Work::Input(input, tx))
            .map_err(|_| io::Error::other("Remote terminal closed"))?;
        rx.recv()
            .map_err(|_| io::Error::other("Remote terminal closed"))?
    }
    fn read_rows(&self, _: PaneId, rows: Range<usize>) -> io::Result<Screen> {
        let (tx, rx) = mpsc::channel();
        self.work
            .send(Work::Rows(rows, tx))
            .map_err(|_| io::Error::other("Remote terminal closed"))?;
        rx.recv()
            .map_err(|_| io::Error::other("Remote terminal closed"))?
    }
    fn set_size(&self, cols: u16, rows: u16) -> io::Result<()> {
        if !agent_interface_roster::profiles::terminal::valid_dimensions(cols, rows) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported terminal dimensions",
            ));
        }
        self.viewport(None, Some((cols, rows)))
    }
    fn ignore_size(&self) -> io::Result<()> {
        Ok(())
    }
    fn set_colors(&self, _: Rgba, _: Rgba) {}
    fn failure(&self) -> Option<String> {
        self.state.lock().unwrap().ended.clone()
    }
    fn is_finished(&self) -> bool {
        self.state.lock().unwrap().ended.is_some()
    }
    fn take_windows_changed(&self) -> bool {
        false
    }
    fn shutdown(&self) {
        self.close.detach();
    }
}

#[cfg(test)]
mod tests;
