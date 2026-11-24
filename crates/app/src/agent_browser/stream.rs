//! A watch-only client of an agent-browser session's frame stream.
//!
//! The session's daemon serves its page as JPEG frames over a local
//! WebSocket. The client asks for acknowledged pacing, so the daemon keeps at
//! most one frame in flight and sends the next only after the client
//! acknowledges one it has drawn. Frames are decoded on the client's own
//! thread into one slot that the newest frame replaces, so a slow pane shows
//! the newest frame and nothing queues. Acknowledgements are the only
//! messages the client sends: no mouse, keyboard or touch input ever reaches
//! the page. Dropping the [`Stream`] closes the connection.

use std::{
    io::ErrorKind,
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use base64::Engine as _;
use gpui::RenderImage;
use serde_json::Value;
use tungstenite::{Message, WebSocket};

/// The highest frame rate the client asks for.
const MAX_FPS: u32 = 30;
/// How long a read waits before the client checks for an acknowledgement
/// to send or a request to stop. Short, so the next frame is asked for soon
/// after the view draws one.
const POLL: Duration = Duration::from_millis(5);
/// How long the client waits before it tries an unreachable stream again.
const RETRY: Duration = Duration::from_secs(2);

/// Whether frames are arriving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Connecting,
    Live,
    /// The stream refused the connection or closed it; the client tries again.
    Unreachable,
}

/// One decoded frame, ready to draw.
#[derive(Clone)]
pub struct Frame {
    pub seq: u64,
    pub image: Arc<RenderImage>,
}

/// What the client last heard from the stream.
#[derive(Default)]
struct Latest {
    state: State,
    /// The newest frame not yet taken.
    frame: Option<Frame>,
    /// The newest frame taken and not yet acknowledged.
    drawn: Option<u64>,
    address: Option<String>,
    title: Option<String>,
}

struct Shared {
    latest: Mutex<Latest>,
    stop: AtomicBool,
}

/// The only message the client sends.
enum Outgoing {
    /// The newest frame drawn; acknowledgements are cumulative.
    Ack(u64),
}

impl Outgoing {
    fn text(&self) -> String {
        match self {
            Self::Ack(seq) => serde_json::json!({"type": "ack", "seq": seq}).to_string(),
        }
    }
}

/// A connection to one session's stream, open while the value lives.
pub struct Stream {
    shared: Arc<Shared>,
}

impl Stream {
    /// Watch the stream on this local `port`. `wake` is called from the
    /// client's thread whenever something new can be taken.
    pub fn open(port: u16, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            latest: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let worker = shared.clone();
        let _ = thread::Builder::new()
            .name("agent-browser-stream".into())
            .spawn(move || run(port, &worker, &wake));
        Self { shared }
    }

    pub fn state(&self) -> State {
        self.latest().state
    }

    /// The page's address and title, as the stream last reported them.
    pub fn page(&self) -> (Option<String>, Option<String>) {
        let latest = self.latest();
        (latest.address.clone(), latest.title.clone())
    }

    /// The newest frame, to draw now. Taking it lets the stream send the next.
    pub fn take_frame(&self) -> Option<Frame> {
        let mut latest = self.latest();
        let frame = latest.frame.take()?;
        latest.drawn = Some(frame.seq);
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

fn run(port: u16, shared: &Shared, wake: &dyn Fn()) {
    while !shared.stop.load(Ordering::Relaxed) {
        if let Some(mut socket) = connect(port) {
            restart(shared);
            wake();
            watch(&mut socket, shared, wake);
            let _ = socket.close(None);
            let _ = socket.flush();
        }
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut latest = lock(shared);
            latest.state = State::Unreachable;
            latest.drawn = None;
        }
        wake();
        let mut waited = Duration::ZERO;
        while waited < RETRY && !shared.stop.load(Ordering::Relaxed) {
            thread::sleep(POLL);
            waited += POLL;
        }
    }
}

/// Start a new connection: a frame from the previous one is neither drawn
/// nor acknowledged on it.
fn restart(shared: &Shared) {
    let mut latest = lock(shared);
    latest.state = State::Connecting;
    latest.frame = None;
    latest.drawn = None;
}

fn connect(port: u16) -> Option<WebSocket<TcpStream>> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, RETRY).ok()?;
    stream.set_read_timeout(Some(RETRY)).ok()?;
    stream.set_nodelay(true).ok()?;
    // Pacing set on the address governs the first frame too.
    let url = format!("ws://127.0.0.1:{port}/?pacing=ack&maxFps={MAX_FPS}");
    let (socket, _) = tungstenite::client(url, stream).ok()?;
    socket.get_ref().set_read_timeout(Some(POLL)).ok()?;
    Some(socket)
}

fn watch(socket: &mut WebSocket<TcpStream>, shared: &Shared, wake: &dyn Fn()) {
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let drawn = lock(shared).drawn.take();
        if let Some(seq) = drawn
            && socket
                .send(Message::text(Outgoing::Ack(seq).text()))
                .is_err()
        {
            return;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                if receive(&text, shared) {
                    wake();
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                // Answer a ping that arrived during the wait.
                if let Err(tungstenite::Error::Io(error)) = socket.flush()
                    && !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
                {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Take in one message from the stream. Returns whether it changed what the
/// view shows.
fn receive(text: &str, shared: &Shared) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let string =
        |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    match message.get("type").and_then(Value::as_str) {
        Some("frame") => {
            let Some(seq) = message.get("seq").and_then(Value::as_u64) else {
                return false;
            };
            let frame = message
                .get("data")
                .and_then(Value::as_str)
                .and_then(|data| decode(seq, data));
            let mut latest = lock(shared);
            latest.state = State::Live;
            match frame {
                // A newer frame replaces one that was never drawn.
                Some(frame) => latest.frame = Some(frame),
                // A frame that cannot be drawn is acknowledged at once, so the
                // stream moves on to the next.
                None => latest.drawn = Some(seq),
            }
            true
        }
        Some("url") => {
            lock(shared).address = string(&message, "url");
            true
        }
        Some("tabs") => {
            let active = message
                .get("tabs")
                .and_then(Value::as_array)
                .and_then(|tabs| {
                    tabs.iter()
                        .find(|tab| tab.get("active").and_then(Value::as_bool) == Some(true))
                });
            let Some(tab) = active else {
                return false;
            };
            let mut latest = lock(shared);
            latest.address = string(tab, "url").or(latest.address.take());
            latest.title = string(tab, "title").filter(|title| !title.is_empty());
            true
        }
        _ => false,
    }
}

/// A frame's JPEG, decoded into the BGRA pixels the renderer draws.
fn decode(seq: u64, data: &str) -> Option<Frame> {
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let mut pixels = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
        .ok()?
        .into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Frame {
        seq,
        image: Arc::new(RenderImage::new([image::Frame::new(pixels)])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc, time::Instant};

    fn jpeg() -> String {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 3)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn frame(seq: u64, data: &str) -> String {
        serde_json::json!({"type": "frame", "seq": seq, "data": data}).to_string()
    }

    #[test]
    fn frames_arriving_faster_than_drawn_leave_only_the_newest() {
        let shared = Shared {
            latest: Mutex::default(),
            stop: AtomicBool::new(false),
        };
        let data = jpeg();
        for seq in 1..=5 {
            assert!(receive(&frame(seq, &data), &shared));
        }
        let stream = Stream {
            shared: Arc::new(shared),
        };
        let newest = stream.take_frame().expect("a frame to draw");
        assert_eq!(newest.seq, 5);
        assert!(stream.take_frame().is_none());
        assert_eq!(stream.latest().drawn, Some(5));
    }

    #[test]
    fn a_new_connection_forgets_the_last_frame() {
        let shared = Shared {
            latest: Mutex::default(),
            stop: AtomicBool::new(false),
        };
        let data = jpeg();
        receive(&frame(1, &data), &shared);
        receive(&frame(2, "not a picture"), &shared);
        restart(&shared);
        let latest = lock(&shared);
        assert!(latest.frame.is_none() && latest.drawn.is_none());
        assert_eq!(latest.state, State::Connecting);
    }

    #[test]
    // tungstenite's handshake callback returns its HTTP response as the error.
    #[allow(clippy::result_large_err)]
    fn the_client_sends_only_acknowledgements() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sent, received) = mpsc::channel();
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
            let data = jpeg();
            for seq in 1..=3 {
                socket.send(Message::text(frame(seq, &data))).unwrap();
            }
            socket
                .send(Message::text(
                    r#"{"type":"url","url":"https://example.com/"}"#,
                ))
                .unwrap();
            let mut messages = Vec::new();
            while let Ok(message) = socket.read() {
                match message {
                    Message::Text(text) => messages.push(text.to_string()),
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            sent.send((path, messages)).unwrap();
        });

        let stream = Stream::open(port, || {});
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut drawn = Vec::new();
        while drawn.last() != Some(&3) || stream.page().0.is_none() {
            assert!(Instant::now() < deadline, "the stream never delivered");
            if let Some(frame) = stream.take_frame() {
                drawn.push(frame.seq);
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Give the last acknowledgement time to leave.
        thread::sleep(POLL * 4);
        drop(stream);
        let (path, messages) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        server.join().unwrap();

        assert!(path.contains("pacing=ack"), "{path}");
        assert!(!messages.is_empty());
        for message in &messages {
            let message: Value = serde_json::from_str(message).unwrap();
            assert_eq!(message["type"], "ack", "{message}");
        }
    }
}
