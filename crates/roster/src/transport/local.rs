//! HTTP over an authenticated local Unix connection. No TCP fallback.

use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    time::Duration,
};

use http_body_util::{BodyExt, Full};
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

struct Authentication<'a> {
    key: &'a str,
    identity: &'a crate::profiles::Identity,
    /// Sent as `x-backbone-request-id`, so a repeat after a lost reply gets
    /// the first result. Left out, the Rust service makes one.
    request_id: Option<&'a str>,
}

/// Bounded status and body, including structured refusal details.
pub(crate) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub sheet_warnings: Option<Vec<u8>>,
}

pub(crate) fn profile_response(
    socket: &str,
    path: &str,
    credentials: (&str, &crate::profiles::Identity),
    body: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
) -> Result<Response, Option<u16>> {
    let auth = Authentication {
        key: credentials.0,
        identity: credentials.1,
        request_id: None,
    };
    exchange(socket, method(&body), path, auth, body, limit, timeout)
}

/// Remove a resource on a verified profile connection.
pub(crate) fn profile_delete(
    socket: &str,
    path: &str,
    credentials: (&str, &crate::profiles::Identity),
    limit: usize,
    timeout: Duration,
) -> Result<Response, Option<u16>> {
    let auth = Authentication {
        key: credentials.0,
        identity: credentials.1,
        request_id: None,
    };
    exchange(socket, "DELETE", path, auth, None, limit, timeout)
}

/// Change a resource on a verified profile connection with `method`, under
/// `request_id` when one is given.
#[allow(clippy::too_many_arguments)]
pub(crate) fn profile_write(
    socket: &str,
    method: &'static str,
    path: &str,
    credentials: (&str, &crate::profiles::Identity),
    body: Option<Vec<u8>>,
    request_id: Option<&str>,
    limit: usize,
    timeout: Duration,
) -> Result<Response, Option<u16>> {
    let auth = Authentication {
        key: credentials.0,
        identity: credentials.1,
        request_id,
    };
    exchange(socket, method, path, auth, body, limit, timeout)
}

fn method(body: &Option<Vec<u8>>) -> &'static str {
    if body.is_some() { "POST" } else { "GET" }
}

/// The effective user, once `socket` is this user's private socket in this
/// user's private folder.
fn owned(socket: &Path) -> Result<u32, Option<u16>> {
    let uid = rustix::process::geteuid().as_raw();
    let metadata = fs::symlink_metadata(socket).map_err(|error| {
        tracing::debug!(event = "provider_socket_metadata_failed", error_kind = ?error.kind(), os_error = error.raw_os_error(), "Provider socket metadata unavailable");
        None
    })?;
    let parent = fs::metadata(socket.parent().ok_or(None)?).map_err(|_| None)?;
    if !socket.is_absolute()
        || !metadata.file_type().is_socket()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o600
        || !parent.is_dir()
        || parent.uid() != uid
        || parent.mode() & 0o777 != 0o700
    {
        tracing::debug!(
            event = "provider_socket_rejected",
            "Provider socket ownership or permissions invalid"
        );
        return Err(None);
    }
    Ok(uid)
}

fn exchange(
    socket: &str,
    method: &'static str,
    path: &str,
    auth: Authentication<'_>,
    body: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
) -> Result<Response, Option<u16>> {
    let socket = Path::new(socket);
    let uid = owned(socket)?;
    // The runtime is scoped to this request. Dropping it also cancels the HTTP
    // connection task on timeout, including a peer that never finishes its body.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| None)?;
    runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let stream = UnixStream::connect(socket).await.map_err(|error| {
                tracing::debug!(event = "provider_socket_connect_failed", error_kind = ?error.kind(), os_error = error.raw_os_error(), "Provider socket connection failed");
                None
            })?;
            if stream.peer_cred().map_err(|_| None)?.uid() != uid {
                tracing::debug!(event = "provider_peer_rejected", "Provider peer identity rejected");
                return Err(None);
            }
            // Check the connected peer before constructing/sending credentials.
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "localhost")
                .header("Connection", "close");
            if body.is_some() {
                request = request.header("Content-Type", "application/json");
            }
            if !auth.key.is_empty() {
                let mut value =
                    http::HeaderValue::from_str(&format!("Bearer {}", auth.key)).map_err(|_| None)?;
                value.set_sensitive(true);
                request = request.header("Authorization", value);
            }
            request = request.header("X-Backbone-Profile-ID", &auth.identity.profile_id);
            // A connection without a key is to the Rust service, which
            // takes the profile alone.
            if !auth.key.is_empty() {
                request = request
                    .header("X-Backbone-Installation-ID", &auth.identity.installation_id)
                    .header("X-Backbone-Lifecycle-ID", &auth.identity.lifecycle_id);
            }
            if let Some(id) = auth.request_id {
                request = request.header("X-Backbone-Request-ID", id);
            }
            let request = request
                .body(Full::new(Bytes::from(body.unwrap_or_default())))
                .map_err(|_| None)?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| None)?;
            let connection = tokio::spawn(connection);
            let result = async {
                let response = sender.send_request(request).await.map_err(|_| None)?;
                let status = response.status().as_u16();
                let sheet_warnings = match response.headers().get("X-Backbone-Sheet-Warnings") {
                    Some(header) if header.as_bytes().len() <= 4096 => Some(header.as_bytes().to_vec()),
                    Some(_) => return Err(None),
                    None => None,
                };
                let mut body = response.into_body();
                let mut bytes = Vec::new();
                while let Some(frame) = body.frame().await {
                    if let Ok(data) = frame.map_err(|_| None)?.into_data() {
                        let remaining = limit.saturating_add(1).saturating_sub(bytes.len());
                        bytes.extend_from_slice(&data[..data.len().min(remaining)]);
                        if bytes.len() > limit {
                            break;
                        }
                    }
                }
                Ok(Response { status, body: bytes, sheet_warnings })
            }
            .await;
            connection.abort();
            result
        })
        .await
        .map_err(|_| None)?
    })
}

/// Read a response body that never ends, such as Server-Sent Events, until
/// `live` is false, handing each chunk to `chunk` as it arrives. An error is a
/// refusal's status, or none when the stream broke, `chunk` returned false or
/// nothing arrived for `quiet`. The peer is the Rust service, which takes the
/// profile alone.
pub(crate) fn stream(
    socket: &str,
    path: &str,
    profile_id: &str,
    quiet: Duration,
    live: &dyn Fn() -> bool,
    chunk: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), Option<u16>> {
    // How long a stopped reader may take to notice.
    const STEP: Duration = Duration::from_millis(250);
    let socket = Path::new(socket);
    let uid = owned(socket)?;
    // Dropping the runtime closes the connection, as for a single request.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| None)?;
    runtime.block_on(async {
        let open = async {
            let stream = UnixStream::connect(socket).await.map_err(|_| None)?;
            if stream.peer_cred().map_err(|_| None)?.uid() != uid {
                return Err(None);
            }
            let request = Request::builder()
                .method("GET")
                .uri(path)
                .header("Host", "localhost")
                .header("Accept", "text/event-stream")
                .header("X-Backbone-Profile-ID", profile_id)
                .body(Full::new(Bytes::new()))
                .map_err(|_| None)?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| None)?;
            tokio::spawn(connection);
            let response = sender.send_request(request).await.map_err(|_| None)?;
            Ok((sender, response))
        };
        // A peer that accepts and then says nothing must not hold a stopped
        // reader for the whole deadline.
        let mut open = std::pin::pin!(open);
        let mut waited = Duration::ZERO;
        let (_sender, response) = loop {
            match tokio::time::timeout(STEP, &mut open).await {
                Ok(opened) => break opened?,
                Err(_) if !live() => return Ok(()),
                Err(_) => {
                    waited += STEP;
                    if waited >= Duration::from_secs(15) {
                        return Err(None);
                    }
                }
            }
        };
        if !response.status().is_success() {
            return Err(Some(response.status().as_u16()));
        }
        let mut body = response.into_body();
        let mut silent = Duration::ZERO;
        while live() {
            match tokio::time::timeout(STEP, body.frame()).await {
                Err(_) => {
                    silent += STEP;
                    if silent >= quiet {
                        return Err(None);
                    }
                }
                Ok(None) => return Err(None),
                Ok(Some(frame)) => {
                    if let Ok(data) = frame.map_err(|_| None)?.into_data() {
                        silent = Duration::ZERO;
                        if !chunk(&data) {
                            return Err(None);
                        }
                    }
                }
            }
        }
        Ok(())
    })
}
