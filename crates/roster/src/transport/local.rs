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
    identity: Option<&'a crate::profiles::Identity>,
}

pub(super) fn request(
    socket: &str,
    path: &str,
    key: &str,
    body: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, Option<u16>> {
    let auth = Authentication {
        key,
        identity: None,
    };
    let response = exchange(socket, path, auth, body, limit, timeout, false)?;
    if (200..300).contains(&response.status) {
        Ok(response.body)
    } else {
        Err(Some(response.status))
    }
}

/// Bounded status and body, including structured refusal details.
pub(crate) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub sheet_warnings: Option<Vec<u8>>,
}

pub(crate) fn response(
    socket: &str,
    path: &str,
    key: &str,
    body: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
) -> Result<Response, Option<u16>> {
    let auth = Authentication {
        key,
        identity: None,
    };
    exchange(socket, path, auth, body, limit, timeout, true)
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
        identity: Some(credentials.1),
    };
    exchange(socket, path, auth, body, limit, timeout, true)
}

fn exchange(
    socket: &str,
    path: &str,
    auth: Authentication<'_>,
    body: Option<Vec<u8>>,
    limit: usize,
    timeout: Duration,
    retain_errors: bool,
) -> Result<Response, Option<u16>> {
    let socket = Path::new(socket);
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
                .method(if body.is_some() { "POST" } else { "GET" })
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
            if let Some(identity) = auth.identity {
                request = request
                    .header("X-Backbone-Profile-ID", &identity.profile_id)
                    .header("X-Backbone-Installation-ID", &identity.installation_id)
                    .header("X-Backbone-Lifecycle-ID", &identity.lifecycle_id);
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
                if !retain_errors && !response.status().is_success() {
                    return Ok(Response { status, body: Vec::new(), sheet_warnings: None });
                }
                let sheet_warnings = if retain_errors {
                    match response.headers().get("X-Backbone-Sheet-Warnings") {
                        Some(header) if header.as_bytes().len() <= 4096 => Some(header.as_bytes().to_vec()),
                        Some(_) => return Err(None),
                        None => None,
                    }
                } else { None };
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
