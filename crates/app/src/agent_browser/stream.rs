//! A watch-only screencast of an agent's page, read from its Chrome.
//!
//! The client connects to the page's DevTools endpoint on `127.0.0.1`, at the
//! port the agent's Chrome wrote in its own `DevToolsActivePort` file, and
//! asks it to screencast. Chrome keeps a few frames in flight and sends more
//! only after the client acknowledges them. The client acknowledges frames
//! only when the view takes one to draw, then also those a newer frame
//! replaced, so a view that is not painting pauses Chrome after a few frames
//! instead of decoding a full-rate stream. A frame that cannot be decoded is
//! acknowledged at once. Frames come at the page's own pixel
//! size, with its size in CSS pixels, and are decoded on the client's own
//! thread into one slot that the newest frame replaces, so nothing queues.
//! The client sends three methods only: `Page.startScreencast`,
//! `Page.screencastFrameAck` and `Page.stopScreencast`. No input, script or
//! emulation reaches the page. The page's address and title are read from
//! Chrome's target list. After some navigations Chrome sends no frame of the
//! new page until it changes again, so when the address changes the client
//! restarts the screencast once, which sends the page as it is. Dropping the
//! [`Stream`] stops the screencast.

use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base64::Engine as _;
use gpui::RenderImage;
use serde_json::Value;
use tungstenite::{Message, WebSocket};

/// JPEG quality of the frames, agent-browser's own default.
const QUALITY: u32 = 80;
/// How long a read waits before the client checks for an acknowledgement
/// to send or a request to stop. Short, so the next frame is asked for soon
/// after the view draws one.
const POLL: Duration = Duration::from_millis(5);
/// How long a read waits while no frame waits to be drawn or acknowledged.
const IDLE: Duration = Duration::from_millis(100);
/// How long a waiting frame keeps reads short. A view that has not taken it
/// by then is not painting, so the client looks less often.
const EAGER: Duration = Duration::from_secs(1);
/// How long the client waits before it tries an unreachable page again.
const RETRY: Duration = Duration::from_secs(2);
/// How often the client reads the page's address and title.
const PAGE: Duration = Duration::from_secs(2);
/// How long the client waits for Chrome's target list.
const LISTING: Duration = Duration::from_millis(500);
/// The largest target list the client reads. Chrome's is a few kilobytes.
const LISTING_BYTES: usize = 1 << 20;
/// How long after the page's address changes a frame has to arrive before
/// the client restarts the screencast.
const REFRESH: Duration = Duration::from_millis(1500);

/// Whether frames are arriving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Connecting,
    Live,
    /// The page could not be reached or closed the connection; the client
    /// tries again.
    Unreachable,
}

/// One decoded frame, ready to draw.
#[derive(Clone)]
pub struct Frame {
    /// What acknowledges it.
    id: i64,
    pub image: Arc<RenderImage>,
    /// The page's size in CSS pixels, which the frame shows.
    pub width: f32,
    pub height: f32,
}

/// What the client last heard from the page.
#[derive(Default)]
struct Latest {
    state: State,
    /// The newest frame not yet taken.
    frame: Option<Frame>,
    /// Frames to acknowledge now.
    acks: Vec<i64>,
    /// Frames replaced before they were drawn, acknowledged when the view
    /// next takes one.
    replaced: Vec<i64>,
    /// Whether the view took a frame since the client last looked.
    taken: bool,
    address: Option<String>,
    title: Option<String>,
}

struct Shared {
    latest: Mutex<Latest>,
    stop: AtomicBool,
}

/// Every message the client sends.
enum Outgoing {
    Start,
    /// A frame drawn or replaced.
    Ack(i64),
    Stop,
}

impl Outgoing {
    fn text(&self, id: u64) -> String {
        let (method, params) = match self {
            Self::Start => (
                "Page.startScreencast",
                serde_json::json!({"format": "jpeg", "quality": QUALITY, "everyNthFrame": 1}),
            ),
            Self::Ack(frame) => (
                "Page.screencastFrameAck",
                serde_json::json!({"sessionId": frame}),
            ),
            Self::Stop => ("Page.stopScreencast", serde_json::json!({})),
        };
        serde_json::json!({"id": id, "method": method, "params": params}).to_string()
    }
}

/// A screencast of one page, running while the value lives.
pub struct Stream {
    shared: Arc<Shared>,
}

impl Stream {
    /// Watch the page `target` of the Chrome that the agent-browser daemon
    /// `daemon` started. `wake` is called from the client's thread whenever
    /// something new can be taken.
    pub fn open(daemon: u32, target: String, wake: impl Fn() + Send + 'static) -> Self {
        Self::open_on(move || super::devtools_port(daemon), target, wake)
    }

    /// Watch `target` on the DevTools port `port` finds, asked again before
    /// each connection.
    fn open_on(
        port: impl Fn() -> Option<u16> + Send + 'static,
        target: String,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            latest: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let worker = shared.clone();
        let started = thread::Builder::new()
            .name("agent-browser-stream".into())
            .spawn(move || run(&port, &target, &worker, &wake));
        if started.is_err() {
            lock(&shared).state = State::Unreachable;
        }
        Self { shared }
    }

    pub fn state(&self) -> State {
        self.latest().state
    }

    /// The page's address and title, as Chrome last listed them.
    pub fn page(&self) -> (Option<String>, Option<String>) {
        let latest = self.latest();
        (latest.address.clone(), latest.title.clone())
    }

    /// The newest frame, to draw now. Taking it lets Chrome send the next.
    pub fn take_frame(&self) -> Option<Frame> {
        let mut latest = self.latest();
        let frame = latest.frame.take()?;
        let replaced = std::mem::take(&mut latest.replaced);
        latest.acks.extend(replaced);
        latest.acks.push(frame.id);
        latest.taken = true;
        Some(frame)
    }

    fn latest(&self) -> std::sync::MutexGuard<'_, Latest> {
        lock(&self.shared)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Latest> {
    shared
        .latest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn run(port: &dyn Fn() -> Option<u16>, target: &str, shared: &Shared, wake: &dyn Fn()) {
    while !shared.stop.load(Ordering::Relaxed) {
        if let Some(port) = port()
            && let Some(mut socket) = connect(port, target)
        {
            restart(shared);
            wake();
            let ended = watch(&mut socket, port, target, shared, wake);
            let _ = socket.close(None);
            let _ = socket.flush();
            // The page is there: connect again at once, and the view keeps
            // its picture unless that fails.
            if ended == Ended::RestartRefused {
                continue;
            }
        }
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut latest = lock(shared);
            latest.state = State::Unreachable;
            latest.acks.clear();
            latest.replaced.clear();
        }
        wake();
        let mut waited = Duration::ZERO;
        while waited < RETRY && !shared.stop.load(Ordering::Relaxed) {
            thread::sleep(IDLE);
            waited += IDLE;
        }
    }
}

/// Start a new connection: a frame from the previous one is neither drawn
/// nor acknowledged on it.
fn restart(shared: &Shared) {
    let mut latest = lock(shared);
    latest.state = State::Connecting;
    latest.frame = None;
    latest.acks.clear();
    latest.replaced.clear();
}

/// A local TCP connection to `port`, waiting at most `RETRY` for each read.
fn local(port: u16) -> Option<TcpStream> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, RETRY).ok()?;
    stream.set_read_timeout(Some(RETRY)).ok()?;
    Some(stream)
}

/// Whether `target` can name a page: Chrome's ids are hexadecimal, so
/// nothing in one can lead the address to another endpoint.
fn page_id(target: &str) -> bool {
    !target.is_empty() && target.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn connect(port: u16, target: &str) -> Option<WebSocket<TcpStream>> {
    if !page_id(target) {
        return None;
    }
    let stream = local(port)?;
    stream.set_nodelay(true).ok()?;
    let url = format!("ws://127.0.0.1:{port}/devtools/page/{target}");
    let (socket, _) = tungstenite::client(url, stream).ok()?;
    socket.get_ref().set_read_timeout(Some(IDLE)).ok()?;
    Some(socket)
}

/// How watching a page over one connection ended.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// Chrome refused the client's own restart, as during a change of
    /// renderer.
    RestartRefused,
    /// The view went, or the page refused or closed the connection.
    Lost,
}

fn watch(
    socket: &mut WebSocket<TcpStream>,
    port: u16,
    target: &str,
    shared: &Shared,
    wake: &dyn Fn(),
) -> Ended {
    let mut sent = 0;
    let mut send = |socket: &mut WebSocket<TcpStream>, message: Outgoing| {
        sent += 1;
        socket.send(Message::text(message.text(sent))).is_ok()
    };
    // A view that went while this connected asks for nothing.
    if shared.stop.load(Ordering::Relaxed) || !send(socket, Outgoing::Start) {
        return Ended::Lost;
    }
    let mut timeout = IDLE;
    let mut listed: Option<Instant> = None;
    let mut waiting_since: Option<Instant> = None;
    let mut refresh = Refresh::default();
    // Whether a restart was sent and no frame has come since.
    let mut restarting = false;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            send(socket, Outgoing::Stop);
            return Ended::Lost;
        }
        if refresh.due(Instant::now()) {
            if !(send(socket, Outgoing::Stop) && send(socket, Outgoing::Start)) {
                return Ended::Lost;
            }
            restarting = true;
        }
        if listed.is_none_or(|at| at.elapsed() >= PAGE) {
            listed = Some(Instant::now());
            // Unlisted, the view shows the address agent-browser recorded,
            // which follows navigation; a stale listed one would not.
            let (address, title) = page(port, target).unwrap_or_default();
            refresh.listed(address.as_deref(), Instant::now());
            {
                let mut latest = lock(shared);
                if (&latest.address, &latest.title) != (&address, &title) {
                    (latest.address, latest.title) = (address, title);
                    drop(latest);
                    wake();
                }
            }
        }
        let (acks, waiting, taken) = {
            let mut latest = lock(shared);
            (
                std::mem::take(&mut latest.acks),
                latest.frame.is_some(),
                std::mem::take(&mut latest.taken),
            )
        };
        let wanted = if eager(&mut waiting_since, waiting, taken, Instant::now()) {
            POLL
        } else {
            IDLE
        };
        if wanted != timeout {
            if socket.get_ref().set_read_timeout(Some(wanted)).is_err() {
                return Ended::Lost;
            }
            timeout = wanted;
        }
        for frame in acks {
            if !send(socket, Outgoing::Ack(frame)) {
                return Ended::Lost;
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => match receive(&text, shared) {
                Received::Changed => {
                    restarting = false;
                    wake();
                }
                Received::Nothing => {}
                Received::Refused if restarting => return Ended::RestartRefused,
                Received::Refused => return Ended::Lost,
            },
            Ok(Message::Close(_)) => return Ended::Lost,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                // Answer a ping that arrived during the wait.
                if let Err(tungstenite::Error::Io(error)) = socket.flush()
                    && !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
                {
                    return Ended::Lost;
                }
            }
            Err(_) => return Ended::Lost,
        }
    }
}

/// When to restart the screencast: once, `REFRESH` after the listing shows a
/// new address. A frame does not cancel it, since it may still be the old
/// page's. A page that keeps its address is never restarted, however long
/// it sends nothing.
#[derive(Default)]
struct Refresh {
    /// The address Chrome last listed.
    address: Option<String>,
    /// When the address changed, until the restart is due.
    changed: Option<Instant>,
}

impl Refresh {
    /// Chrome listed `address`; none when the list could not be read.
    fn listed(&mut self, address: Option<&str>, now: Instant) {
        let Some(address) = address else {
            return;
        };
        if self.address.as_deref() != Some(address) {
            // The first address is no change: a new screencast sends a frame.
            if self.address.is_some() {
                self.changed = Some(now);
            }
            self.address = Some(address.to_owned());
        }
    }

    /// Whether to restart now. Each change restarts at most once.
    fn due(&mut self, now: Instant) -> bool {
        let due = self
            .changed
            .is_some_and(|at| now.duration_since(at) >= REFRESH);
        if due {
            self.changed = None;
        }
        due
    }
}

/// Whether reads should be short: while a frame has waited for the view less
/// than `EAGER`, so its acknowledgement leaves soon after it is drawn. A view
/// that takes a frame is painting, so the wait starts again even when a newer
/// frame already waits.
fn eager(since: &mut Option<Instant>, waiting: bool, taken: bool, now: Instant) -> bool {
    if taken || !waiting {
        *since = None;
    }
    waiting && now.duration_since(*since.get_or_insert(now)) < EAGER
}

/// What one message from Chrome did.
#[derive(Debug, PartialEq, Eq)]
enum Received {
    /// It changed what the view shows.
    Changed,
    Nothing,
    /// Chrome refused the screencast, as for a page that has closed.
    Refused,
}

/// Take in one message from Chrome.
fn receive(text: &str, shared: &Shared) -> Received {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Received::Nothing;
    };
    if message.get("error").is_some() {
        return Received::Refused;
    }
    if message.get("method").and_then(Value::as_str) != Some("Page.screencastFrame") {
        return Received::Nothing;
    }
    let params = &message["params"];
    let Some(id) = params.get("sessionId").and_then(Value::as_i64) else {
        return Received::Nothing;
    };
    let frame = params
        .get("data")
        .and_then(Value::as_str)
        .and_then(|data| decode(id, data, &params["metadata"]));
    let mut latest = lock(shared);
    latest.state = State::Live;
    match frame {
        // A newer frame replaces one that was never drawn. Its
        // acknowledgement waits for the view to take a frame, so Chrome
        // pauses while the view is not painting.
        Some(frame) => {
            if let Some(replaced) = latest.frame.replace(frame) {
                latest.replaced.push(replaced.id);
            }
        }
        // A frame that cannot be drawn is acknowledged at once.
        None => latest.acks.push(id),
    }
    Received::Changed
}

/// A frame's JPEG, decoded into the BGRA pixels the renderer draws, with
/// the page's CSS size from its `metadata`.
fn decode(id: i64, data: &str, metadata: &Value) -> Option<Frame> {
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let mut pixels = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
        .ok()?
        .into_rgba8();
    let css = |key: &str, pixels: u32| {
        metadata
            .get(key)
            .and_then(Value::as_f64)
            .filter(|size| *size > 0.0)
            .map_or(pixels as f32, |size| size as f32)
    };
    let (width, height) = (
        css("deviceWidth", pixels.width()),
        css("deviceHeight", pixels.height()),
    );
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Frame {
        id,
        image: Arc::new(RenderImage::new([image::Frame::new(pixels)])),
        width,
        height,
    })
}

/// The address and title of the page `target`, from Chrome's target list.
fn page(port: u16, target: &str) -> Option<(Option<String>, Option<String>)> {
    let mut stream = local(port)?;
    // Frames wait while this reads, so it gives up soon.
    stream.set_read_timeout(Some(LISTING)).ok()?;
    write!(
        stream,
        "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    // Chrome keeps the connection open, so read only the stated length.
    let mut response = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        let read = stream.read(&mut chunk).ok().filter(|read| *read > 0)?;
        response.extend_from_slice(&chunk[..read]);
        if response.len() > 2 * LISTING_BYTES {
            return None;
        }
        if let Some(listing) = listing(&response, target) {
            return listing;
        }
    }
}

/// Read the page `target` from a response to `GET /json/list`, or `None`
/// while the response is still incomplete. A page Chrome does not list, or a
/// response that is not a target list or is larger than `LISTING_BYTES`,
/// gives `Some(None)`.
fn listing(response: &[u8], target: &str) -> Option<Option<(Option<String>, Option<String>)>> {
    let end = response.windows(4).position(|bytes| bytes == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&response[..end]);
    let Some(length) = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    }) else {
        return Some(None);
    };
    let Some(stop) = (end + 4)
        .checked_add(length)
        .filter(|_| length <= LISTING_BYTES)
    else {
        return Some(None);
    };
    let body = response.get(end + 4..stop)?;
    let Ok(targets) = serde_json::from_slice::<Vec<Value>>(body) else {
        return Some(None);
    };
    Some(
        targets
            .iter()
            .find(|listed| listed.get("id").and_then(Value::as_str) == Some(target))
            .map(|listed| {
                let text = |key: &str| {
                    listed
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                (text("url"), text("title"))
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc};

    fn jpeg() -> String {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn frame(id: i64, data: &str) -> String {
        serde_json::json!({
            "method": "Page.screencastFrame",
            "params": {
                "data": data,
                "sessionId": id,
                "metadata": {"deviceWidth": 2.0, "deviceHeight": 1.0, "pageScaleFactor": 1},
            },
        })
        .to_string()
    }

    fn shared() -> Shared {
        Shared {
            latest: Mutex::default(),
            stop: AtomicBool::new(false),
        }
    }

    #[test]
    fn frames_arriving_faster_than_drawn_leave_only_the_newest() {
        let shared = shared();
        let data = jpeg();
        for id in 1..=5 {
            assert_eq!(receive(&frame(id, &data), &shared), Received::Changed);
        }
        let stream = Stream {
            shared: Arc::new(shared),
        };
        // Until the view takes one, Chrome hears nothing and pauses.
        assert!(stream.latest().acks.is_empty());
        let newest = stream.take_frame().expect("a frame to draw");
        assert_eq!(newest.id, 5);
        // Drawn at the page's CSS size, not its pixel size.
        assert_eq!((newest.width, newest.height), (2.0, 1.0));
        assert!(stream.take_frame().is_none());
        // Then every frame is acknowledged, so Chrome sends again.
        assert_eq!(stream.latest().acks, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_page_is_read_from_a_listing_that_arrives_in_pieces() {
        let body = r#"[{"id":"OTHER","url":"https://example.org/","title":"Other"},{"id":"PAGE","url":"https://example.com/","title":"Café"}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = response.as_bytes();
        // Cut inside the two-byte "é": the listing waits for the rest.
        let cut = response.len() - body.len() + body.find('é').unwrap() + 1;
        assert_eq!(listing(&response[..cut], "PAGE"), None);
        assert_eq!(
            listing(response, "PAGE"),
            Some(Some((
                Some("https://example.com/".into()),
                Some("Café".into())
            )))
        );
        // A tab Chrome no longer lists leaves the address to agent-browser's.
        assert_eq!(listing(response, "GONE"), Some(None));
        // A stated length that overflows, or is far beyond any target list,
        // is refused rather than waited for.
        for length in [usize::MAX, LISTING_BYTES + 1] {
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n[]");
            assert_eq!(listing(response.as_bytes(), "PAGE"), Some(None));
        }
    }

    #[test]
    fn taking_a_frame_keeps_reads_short_while_newer_frames_wait() {
        let start = Instant::now();
        let mut since = None;
        assert!(eager(&mut since, true, false, start));
        // A frame left waiting past `EAGER`: the view is not painting.
        assert!(!eager(&mut since, true, false, start + EAGER));
        // The view took one and a newer frame waits already.
        assert!(eager(&mut since, true, true, start + EAGER));
        assert!(eager(&mut since, true, false, start + EAGER + POLL));
        assert!(!eager(&mut since, false, false, start + 2 * EAGER));
    }

    #[test]
    fn a_new_address_restarts_once() {
        let start = Instant::now();
        let mut refresh = Refresh::default();
        refresh.listed(Some("https://example.com/a"), start);
        // The first address, and the same one listed again, are no change.
        refresh.listed(Some("https://example.com/a"), start + PAGE);
        assert!(!refresh.due(start + PAGE + REFRESH));
        // A new address: one restart, not a second.
        let moved = start + 2 * PAGE;
        refresh.listed(Some("https://example.com/b"), moved);
        refresh.listed(None, moved + PAGE);
        assert!(!refresh.due(moved + REFRESH / 2));
        assert!(refresh.due(moved + REFRESH));
        refresh.listed(Some("https://example.com/b"), moved + PAGE);
        assert!(!refresh.due(moved + 2 * REFRESH));
    }

    /// How the fake Chrome answers the client's restart.
    #[derive(Clone, Copy, PartialEq)]
    enum Restart {
        Answered,
        Refused,
        /// Refused, and the page can't be reached afterwards.
        RefusedThenGone,
    }

    /// Watch a fake Chrome whose target list shows the page moving from `/a`
    /// to `/b`, with a frame of the old page just after the move, until
    /// `done` holds for the frames taken and the state. Returns each
    /// connection's methods, the frames taken and every state seen.
    // tungstenite's handshake returns its HTTP response as the error.
    #[allow(clippy::result_large_err)]
    fn watch_a_move(
        restart: Restart,
        done: impl Fn(&[i64], State) -> bool,
    ) -> (Vec<(usize, String)>, Vec<i64>, Vec<State>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (said, heard) = mpsc::channel();
        thread::spawn(move || {
            let (moved, old_frame) = mpsc::channel();
            let mut old_frame = Some(old_frame);
            let ids = Arc::new(std::sync::atomic::AtomicI64::new(0));
            let (mut listings, mut connections, mut open) = (0, 0, Vec::new());
            for mut stream in listener.incoming().flatten() {
                let mut head = [0; 9];
                let _ = stream.peek(&mut head);
                if &head == b"GET /json" {
                    let _ = stream.read(&mut [0; 1024]);
                    let address = if listings == 0 { "a" } else { "b" };
                    let body = format!(
                        r#"[{{"id":"PAGE","url":"https://example.com/{address}","title":"Example"}}]"#
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if listings == 1 {
                        let _ = moved.send(());
                    }
                    listings += 1;
                    open.push(stream);
                    continue;
                }
                let connection = connections;
                connections += 1;
                if connection > 0 && restart == Restart::RefusedThenGone {
                    continue;
                }
                let (said, ids, old_frame) = (said.clone(), ids.clone(), old_frame.take());
                thread::spawn(move || {
                    let mut socket = tungstenite::accept(stream).unwrap();
                    socket
                        .get_ref()
                        .set_read_timeout(Some(Duration::from_millis(10)))
                        .unwrap();
                    let data = jpeg();
                    let send_frame = |socket: &mut WebSocket<TcpStream>| {
                        let id = ids.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = socket.send(Message::text(frame(id, &data)));
                    };
                    let mut stopped = false;
                    loop {
                        if old_frame
                            .as_ref()
                            .is_some_and(|moved| moved.try_recv().is_ok())
                        {
                            send_frame(&mut socket);
                        }
                        let text = match socket.read() {
                            Ok(Message::Text(text)) => text,
                            Ok(Message::Close(_)) => break,
                            Ok(_) => continue,
                            Err(tungstenite::Error::Io(error))
                                if error.kind() == ErrorKind::WouldBlock
                                    || error.kind() == ErrorKind::TimedOut =>
                            {
                                continue;
                            }
                            Err(_) => break,
                        };
                        let message: Value = serde_json::from_str(&text).unwrap();
                        let method = message["method"].as_str().unwrap().to_owned();
                        let _ = said.send((connection, method.clone()));
                        match method.as_str() {
                            "Page.stopScreencast" => stopped = true,
                            "Page.startScreencast" if stopped && restart != Restart::Answered => {
                                let refusal = serde_json::json!({
                                    "id": message["id"],
                                    "error": {"code": -32000, "message": "Not attached to an active page"},
                                });
                                let _ = socket.send(Message::text(refusal.to_string()));
                            }
                            "Page.startScreencast" => send_frame(&mut socket),
                            _ => {}
                        }
                    }
                });
            }
        });

        let stream = Stream::open_on(move || Some(port), "PAGE".into(), || {});
        let deadline = Instant::now() + Duration::from_secs(15);
        let (mut taken, mut states) = (Vec::new(), Vec::new());
        while !done(&taken, stream.state()) {
            assert!(Instant::now() < deadline, "took {taken:?}, saw {states:?}");
            taken.extend(stream.take_frame().map(|frame| frame.id));
            states.push(stream.state());
            thread::sleep(Duration::from_millis(10));
        }
        let methods = heard
            .try_iter()
            .filter(|(_, method)| method != "Page.screencastFrameAck")
            .collect();
        (methods, taken, states)
    }

    fn methods(said: &[(usize, String)], connection: usize) -> Vec<&str> {
        said.iter()
            .filter(|(on, _)| *on == connection)
            .map(|(_, method)| method.as_str())
            .collect()
    }

    #[test]
    fn a_restart_stops_and_starts_the_screencast_and_it_goes_on() {
        // Frames 1, the first; 2, the old page's after the move; 3, the restart's.
        let (said, taken, _) = watch_a_move(Restart::Answered, |taken, _| taken.contains(&3));
        assert_eq!(
            methods(&said, 0),
            [
                "Page.startScreencast",
                "Page.stopScreencast",
                "Page.startScreencast"
            ]
        );
        assert!(methods(&said, 1).is_empty());
        assert!(taken.contains(&2), "the old page's frame came: {taken:?}");
    }

    #[test]
    fn a_refused_restart_reconnects_without_a_word() {
        // Frame 3 is the new connection's.
        let (said, _, states) = watch_a_move(Restart::Refused, |taken, _| taken.contains(&3));
        assert_eq!(methods(&said, 1).first(), Some(&"Page.startScreencast"));
        assert!(!states.contains(&State::Unreachable), "{states:?}");
    }

    #[test]
    fn a_failed_reconnect_after_a_refused_restart_says_so() {
        watch_a_move(Restart::RefusedThenGone, |_, state| {
            state == State::Unreachable
        });
    }

    #[test]
    fn a_new_connection_forgets_the_last_frame() {
        let shared = shared();
        let data = jpeg();
        receive(&frame(1, &data), &shared);
        receive(&frame(2, "not a picture"), &shared);
        receive(&frame(3, &data), &shared);
        {
            let latest = lock(&shared);
            // A frame that cannot be drawn is acknowledged at once.
            assert_eq!(latest.acks, [2]);
            assert_eq!(latest.replaced, [1]);
        }
        restart(&shared);
        let latest = lock(&shared);
        assert!(latest.frame.is_none() && latest.acks.is_empty() && latest.replaced.is_empty());
        assert_eq!(latest.state, State::Connecting);
    }

    #[test]
    fn only_a_page_id_reaches_the_address() {
        assert!(page_id("429D38A9FA3BEDB8E4D6258FAD43F481"));
        for target in ["", "../json/version", "PAGE?x=1", "PAGE/../x"] {
            assert!(!page_id(target), "{target}");
        }
    }

    #[test]
    // tungstenite's handshake callback returns its HTTP response as the error.
    #[allow(clippy::result_large_err)]
    fn the_client_only_screencasts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sent, received) = mpsc::channel();
        let (acked, first_ack) = mpsc::channel();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut path = String::new();
            let mut socket = tungstenite::accept_hdr(
                stream,
                |request: &tungstenite::handshake::server::Request, response| {
                    path = request.uri().to_string();
                    Ok(response)
                },
            )
            .unwrap();
            // Answer the target list as Chrome does, keeping the connection open.
            thread::spawn(move || {
                let mut open = Vec::new();
                for mut stream in listener.incoming().flatten() {
                    let mut request = [0; 1024];
                    let _ = stream.read(&mut request);
                    let body = r#"[{"id":"PAGE","url":"https://example.com/","title":"Example"}]"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    open.push(stream);
                }
            });
            let data = jpeg();
            for id in 1..=3 {
                socket.send(Message::text(frame(id, &data))).unwrap();
            }
            let mut messages = Vec::new();
            while let Ok(message) = socket.read() {
                match message {
                    Message::Text(text) => {
                        let message: Value = serde_json::from_str(&text).unwrap();
                        if message["method"] == "Page.screencastFrameAck" {
                            let _ = acked.send(());
                        }
                        messages.push(message);
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            sent.send((path, messages)).unwrap();
        });

        let stream = Stream::open_on(move || Some(port), "PAGE".into(), || {});
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut drawn = false;
        while !drawn || stream.page().0.is_none() {
            assert!(Instant::now() < deadline, "the page never delivered");
            drawn |= stream.take_frame().is_some();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            stream.page(),
            (Some("https://example.com/".into()), Some("Example".into()))
        );
        // Stop only once the page has heard an acknowledgement.
        first_ack
            .recv_timeout(Duration::from_secs(10))
            .expect("the client acknowledged a frame");
        drop(stream);
        let (path, messages) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        server.join().unwrap();

        assert_eq!(path, "/devtools/page/PAGE");
        let methods: Vec<&str> = messages
            .iter()
            .map(|message| message["method"].as_str().unwrap())
            .collect();
        assert_eq!(methods.first(), Some(&"Page.startScreencast"));
        assert_eq!(methods.last(), Some(&"Page.stopScreencast"));
        for method in &methods {
            assert!(
                [
                    "Page.startScreencast",
                    "Page.screencastFrameAck",
                    "Page.stopScreencast"
                ]
                .contains(method),
                "{method}"
            );
        }
    }
}
