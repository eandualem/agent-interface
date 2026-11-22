//! Profile-bound, byte-preserving remote terminal over HTTP/1.1 Upgrade.

use super::Connection;
use crate::transport::local;
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::{BodyExt, Full};
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Cursor, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    time::Duration,
};

const CARRIER: &str = "backbone-terminal-v1";
const FRAME: usize = 65536;
const CHUNK: usize = 32768;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Authority for this one open view.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// May receive output and pause or resume this view.
    Watch,
    /// May also type, paste and declare a size.
    Type,
}

/// One validated frame from the terminal service.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Exact terminal bytes, possibly splitting a multibyte character.
    Output(Vec<u8>),
    /// All repaint bytes have arrived.
    RepaintEnd,
    /// Prior output was lost. A fresh repaint follows.
    Dropped,
    /// Size actually applied by the service.
    Resized(u16, u16),
    /// The terminal ended with the service's reason.
    Ended(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    Opened {
        agent: String,
        access: Access,
        cols: u16,
        rows: u16,
    },
    Output {
        data_b64: String,
    },
    RepaintEnd,
    Dropped,
    Resized {
        cols: u16,
        rows: u16,
    },
    Ended {
        reason: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Command {
    Input { data_b64: String },
    Resize { cols: u16, rows: u16 },
    Pause,
    Resume,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid remote terminal response",
    )
}

/// Whether a terminal size fits the supported visible grid.
pub fn valid_dimensions(cols: u16, rows: u16) -> bool {
    cols > 0 && rows > 0 && usize::from(cols) * usize::from(rows) <= 1_048_576
}

type Reader = BufReader<std::io::Chain<Cursor<Bytes>, UnixStream>>;

/// The receiving half, with the grant and real dimensions from the opener.
pub struct Stream {
    reader: Reader,
    /// Granted access, before any input can be submitted.
    pub access: Access,
    /// Initial size reported by the host.
    pub size: (u16, u16),
    repaint: bool,
}

/// Owned sending half. Calls are serialized by its owner.
pub struct Writer {
    socket: UnixStream,
    access: Access,
}

/// Closing a pane interrupts either half, including a blocked reader.
pub struct Close(UnixStream);

impl Close {
    /// Detach this view.
    pub fn detach(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

impl Writer {
    fn send(&mut self, command: Command) -> io::Result<()> {
        let mut line = serde_json::to_vec(&command)?;
        line.push(b'\n');
        self.socket.write_all(&line)
    }
    /// Send keyboard or paste bytes under a typing grant.
    pub fn input(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.access != Access::Type {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Watch-only terminal",
            ));
        }
        for chunk in bytes.chunks(CHUNK) {
            self.send(Command::Input {
                data_b64: STANDARD.encode(chunk),
            })?;
        }
        Ok(())
    }
    /// Declare this view's dimensions under a typing grant.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        if self.access != Access::Type {
            return Ok(());
        }
        if !valid_dimensions(cols, rows) {
            return Err(invalid());
        }
        self.send(Command::Resize { cols, rows })
    }
    /// Control output for this view independently of the running agent.
    pub fn pause(&mut self, paused: bool) -> io::Result<()> {
        self.send(if paused {
            Command::Pause
        } else {
            Command::Resume
        })
    }
}

fn read_frame(reader: &mut Reader) -> io::Result<Frame> {
    let mut bytes = Vec::new();
    reader.take(FRAME as u64).read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Remote terminal connection lost",
        ));
    }
    if bytes.last() != Some(&b'\n') || bytes.first() != Some(&b'{') {
        return Err(invalid());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

impl Stream {
    /// Read one frame. A clean end has a reason. An unexpected EOF is a loss.
    pub fn read(&mut self) -> io::Result<Event> {
        match read_frame(&mut self.reader)? {
            Frame::Output { data_b64 } if data_b64.len() <= CHUNK.div_ceil(3) * 4 => {
                let bytes = STANDARD.decode(data_b64).map_err(|_| invalid())?;
                if bytes.len() > CHUNK {
                    return Err(invalid());
                }
                Ok(Event::Output(bytes))
            }
            Frame::Dropped => {
                self.repaint = true;
                Ok(Event::Dropped)
            }
            Frame::RepaintEnd if self.repaint => {
                self.repaint = false;
                Ok(Event::RepaintEnd)
            }
            Frame::Resized { cols, rows } if valid_dimensions(cols, rows) => {
                Ok(Event::Resized(cols, rows))
            }
            Frame::Ended { reason }
                if matches!(
                    reason.as_str(),
                    "process_exited" | "access_ended" | "connection_lost"
                ) =>
            {
                Ok(Event::Ended(reason))
            }
            _ => Err(invalid()),
        }
    }
}

#[derive(Serialize)]
struct Open<'a> {
    agent: &'a str,
    cols: u16,
    rows: u16,
}

impl Connection {
    /// Verify the selected instance before opening the named agent on its current host.
    /// The profile service performs the actual watch/type authorization.
    pub fn open_terminal(&self, agent: &str) -> io::Result<(Stream, Writer, Close)> {
        if !crate::valid_agent(agent) {
            return Err(invalid());
        }
        let verify = || {
            self.handshake(Some(&self.instance))
                .map_err(|error| match error {
                    super::Error::BindingMismatch => io::Error::new(
                        io::ErrorKind::AddrNotAvailable,
                        "Workspace profile instance changed",
                    ),
                    _ => {
                        io::Error::new(io::ErrorKind::NotConnected, "Workspace profile unavailable")
                    }
                })
        };
        verify()?;
        let stream = open(&self.socket, &self.identity.profile_id, agent, 80, 24)?;
        verify()?;
        Ok(stream)
    }
}

/// A connection that closed or was cancelled before the terminal opened is a
/// lost connection the pane may retry; other HTTP failures are not.
fn transport(error: hyper::Error) -> io::Error {
    let kind = if error.is_closed() || error.is_incomplete_message() || error.is_canceled() {
        io::ErrorKind::UnexpectedEof
    } else {
        // A socket error, such as a reset, keeps its own kind.
        std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<io::Error>())
            .map_or(io::ErrorKind::Other, io::Error::kind)
    };
    io::Error::new(kind, error)
}

fn open(
    socket: &std::path::Path,
    profile: &str,
    agent: &str,
    cols: u16,
    rows: u16,
) -> io::Result<(Stream, Writer, Close)> {
    let uid = local::owned(socket)
        .map_err(|_| io::Error::new(io::ErrorKind::NotConnected, "Profile socket unavailable"))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (prefix, socket, opened) = rt.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let stream = tokio::net::UnixStream::connect(socket).await?;
            // Configure the blocking writer before the service can send a final
            // frame and close. Darwin rejects some socket options after EOF.
            use std::os::fd::AsFd;
            let socket = UnixStream::from(stream.as_fd().try_clone_to_owned()?);
            socket.set_write_timeout(Some(TIMEOUT))?;
            if stream.peer_cred()?.uid() != uid {
                return Err(invalid());
            }
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(transport)?;
            tokio::spawn(connection.with_upgrades());
            let request = Request::post("/api/terminals/open")
                .header("Host", "localhost")
                .header("x-backbone-profile-id", profile)
                .header("Connection", "Upgrade")
                .header("Upgrade", CARRIER)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(serde_json::to_vec(&Open {
                    agent,
                    cols,
                    rows,
                })?)))
                .map_err(io::Error::other)?;
            let mut response = sender.send_request(request).await.map_err(transport)?;
            let status = response.status();
            if status != hyper::StatusCode::SWITCHING_PROTOCOLS {
                #[derive(Deserialize)]
                struct Refusal {
                    detail: Detail,
                }
                #[derive(Deserialize)]
                struct Detail {
                    code: String,
                    message: String,
                }
                let body = http_body_util::Limited::new(response.into_body(), FRAME)
                    .collect()
                    .await
                    .map_err(|_| invalid())?
                    .to_bytes();
                let reason = serde_json::from_slice::<Refusal>(&body)
                    .ok()
                    .map(|r| format!("{} ({})", r.detail.message, r.detail.code))
                    .unwrap_or_else(|| format!("Terminal service returned {status}"));
                // 503: the home or the agent's machine cannot be reached now.
                let kind = if status == hyper::StatusCode::SERVICE_UNAVAILABLE {
                    io::ErrorKind::HostUnreachable
                } else {
                    io::ErrorKind::Other
                };
                return Err(io::Error::new(kind, reason));
            }
            if response
                .headers()
                .get("upgrade")
                .and_then(|v| v.to_str().ok())
                != Some(CARRIER)
                || !response
                    .headers()
                    .get_all("connection")
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .flat_map(|v| v.split(','))
                    .any(|v| v.trim().eq_ignore_ascii_case("upgrade"))
            {
                return Err(invalid());
            }
            let parts = hyper::upgrade::on(&mut response)
                .await
                .map_err(transport)?
                .downcast::<TokioIo<tokio::net::UnixStream>>()
                .map_err(|_| invalid())?;
            // Hyper may have read part or all of the first frames with the 101.
            // Keep those bytes and include the opening grant in the handshake deadline.
            let stream = parts.io.into_inner();
            let mut bytes = parts.read_buf.to_vec();
            let mut searched = 0;
            let end = loop {
                if let Some(lf) = bytes[searched..].iter().position(|&b| b == b'\n') {
                    let end = searched + lf + 1;
                    if end > FRAME {
                        return Err(invalid());
                    }
                    break end;
                }
                searched = bytes.len();
                if searched >= FRAME {
                    return Err(invalid());
                }
                stream.readable().await?;
                let mut buf = [0; 8192];
                match stream.try_read(&mut buf) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Remote terminal connection lost",
                        ));
                    }
                    Ok(n) => bytes.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            };
            let opened = serde_json::from_slice::<Frame>(&bytes[..end]).map_err(|_| invalid())?;
            Ok((Bytes::from(bytes).slice(end..), socket, opened))
        })
        .await
        .map_err(|elapsed| io::Error::new(io::ErrorKind::TimedOut, elapsed))?
    })?;
    socket.set_nonblocking(false)?;
    let reader = BufReader::new(Cursor::new(prefix).chain(socket.try_clone()?));
    let Frame::Opened {
        agent: opened,
        access,
        cols,
        rows,
    } = opened
    else {
        return Err(invalid());
    };
    if opened != agent || !valid_dimensions(cols, rows) {
        return Err(invalid());
    }
    let close = Close(socket.try_clone()?);
    Ok((
        Stream {
            reader,
            access,
            size: (cols, rows),
            repaint: true,
        },
        Writer { socket, access },
        close,
    ))
}

#[cfg(test)]
mod tests;
