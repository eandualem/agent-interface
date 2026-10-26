mod local;

use std::{
    io::Read,
    net::{Ipv6Addr, SocketAddr},
    time::Duration,
};

use http::Uri;

use crate::{RESPONSE_LIMIT, Snapshot};

thread_local! {
    // One runtime and connection pool per polling worker. Credentials and
    // endpoint validation remain per request, including after config changes.
    static CLIENT: std::cell::RefCell<Option<reqwest::blocking::Client>> = const { std::cell::RefCell::new(None) };
}

impl Snapshot {
    /// Read one agents response from an explicitly supplied local API address.
    ///
    /// This blocks for at most three seconds and belongs on an I/O worker.
    /// It reads no configuration or credentials itself. Credentials require a
    /// `unix:/absolute/socket` endpoint with verified ownership and peer identity.
    /// Anonymous loopback HTTP reads remain supported; redirects and proxies are
    /// disabled. Every
    /// failure marks the existing snapshot stale with the same fixed diagnostic.
    pub fn fetch(&mut self, url: &str, key: &str) {
        match get(url, "/api/agents", key, RESPONSE_LIMIT) {
            Some(bytes) => self.update(&bytes),
            None => self.unavailable(),
        }
    }
}

/// The validated loopback address of `path`, which the caller builds from
/// fixed text and validated values only.
pub(super) fn endpoint(url: &str, path: &str) -> Option<Uri> {
    let uri: Uri = url.parse().ok()?;
    let authority = uri.authority()?;
    if uri.scheme_str() != Some("http")
        || !["localhost", "127.0.0.1", "[::1]"]
            .iter()
            .any(|host| authority.host().eq_ignore_ascii_case(host))
        || authority.as_str().contains('@')
        || (authority.as_str() != authority.host() && authority.port_u16().is_none())
        || uri.query().is_some()
        || url.contains('#')
    {
        return None;
    }
    format!("{}{path}", url.trim_end_matches('/')).parse().ok()
}

/// One bounded GET of a fixed read path, returning at most `limit` plus one
/// bytes so the caller can reject an oversized body.
pub(super) fn get(url: &str, path: &str, key: &str, limit: usize) -> Option<Vec<u8>> {
    fetch(url, path, key, limit).ok()
}

/// As [`get`], but a reply that is not a success reports its status.
pub(super) fn fetch(
    url: &str,
    path: &str,
    key: &str,
    limit: usize,
) -> Result<Vec<u8>, Option<u16>> {
    let started = std::time::Instant::now();
    let result = fetch_inner(url, path, key, limit).unwrap_or(Err(None));
    if let Err(status) = &result {
        tracing::debug!(
            event = "provider_request_failed",
            status = *status,
            "Provider request failed"
        );
    }
    tracing::trace!(
        event = "provider_request_finished",
        elapsed_ms = started.elapsed().as_millis() as u64,
        success = result.is_ok(),
        "Provider request finished"
    );
    result
}

fn fetch_inner(
    url: &str,
    path: &str,
    key: &str,
    limit: usize,
) -> Option<Result<Vec<u8>, Option<u16>>> {
    if let Some(socket) = url.strip_prefix("unix:") {
        return Some(local::request(
            socket,
            path,
            key,
            None,
            limit,
            Duration::from_secs(3),
        ));
    }
    // A loopback TCP listener has no authenticated owner. Never send a key there.
    if !key.is_empty() {
        return None;
    }
    let uri = endpoint(url, path)?;
    let client = CLIENT.with(|cached| {
        let mut cached = cached.borrow_mut();
        if cached.is_none() {
            *cached = client();
        }
        cached.clone()
    })?;
    // Set the request's total deadline, including the response body, rather
    // than only a timeout renewed for each blocking read.
    let request = client.get(uri.to_string()).timeout(Duration::from_secs(3));
    let response = request.send().ok()?;
    if !response.status().is_success() {
        return Some(Err(Some(response.status().as_u16())));
    }
    // Read one extra byte to distinguish an exact-size body from an oversized
    // one. Snapshot::update keeps the inclusive limit when decoding.
    let mut bytes = Vec::new();
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(Ok(bytes))
}

fn client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connection_verbose(false)
        // Never start an uncancellable system lookup for the only allowed
        // hostname. Client shutdown must not wait for DNS after the deadline.
        .resolve_to_addrs(
            "localhost",
            &[
                SocketAddr::from(([127, 0, 0, 1], 0)),
                SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
            ],
        )
        .build()
        .ok()
}

/// One explicit feedback submission. Never retry a POST with an unknown outcome.
pub(super) fn post_message(url: &str, key: &str, body: Vec<u8>) -> Result<Vec<u8>, Option<u16>> {
    if let Some(socket) = url.strip_prefix("unix:") {
        let bytes = local::request(
            socket,
            "/api/messages",
            key,
            Some(body),
            65536,
            Duration::from_secs(10),
        )?;
        return (bytes.len() <= 65536).then_some(bytes).ok_or(None);
    }
    if !key.is_empty() {
        return Err(None);
    }
    let uri = endpoint(url, "/api/messages").ok_or(None)?;
    let client = client().ok_or(None)?;
    let request = client
        .post(uri.to_string())
        .timeout(Duration::from_secs(10))
        .header("Content-Type", "application/json")
        .body(body);
    let response = request.send().map_err(|_| None)?;
    if !response.status().is_success() {
        return Err(Some(response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    response
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| None)?;
    if bytes.len() > 65536 {
        return Err(None);
    }
    Ok(bytes)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    const BODY: &str = r#"{"items":[{"name":"worker","state":"busy"}]}"#;

    pub(crate) fn local_listener() -> (tempfile::TempDir, UnixListener, String) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("api.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let url = format!("unix:{}", path.display());
        (directory, listener, url)
    }

    #[test]
    fn credentials_require_a_private_socket_and_never_fall_back_to_tcp() {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let url = format!("http://{}", tcp.local_addr().unwrap());
        assert!(get(&url, "/api/agents", "secret", 20).is_none());
        assert!(post_message(&url, "secret", vec![]).is_err());
        assert_eq!(
            tcp.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let (directory, listener, url) = local_listener();
        listener.set_nonblocking(true).unwrap();
        for (path, mode) in [
            (directory.path().join("api.sock"), 0o666),
            (directory.path().to_owned(), 0o755),
        ] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(get(&url, "/api/agents", "secret", 20).is_none());
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if path == directory.path() {
                    0o700
                } else {
                    0o600
                }),
            )
            .unwrap();
        }
    }

    pub(crate) fn server(reply: String, pause: Duration) -> (String, thread::JoinHandle<String>) {
        let (directory, listener, url) = local_listener();
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            let _directory = directory;
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture received no request");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            let sent = socket.write_all(reply.as_bytes());
            // Rejecting the declared oversized body can close the connection
            // before this fixture finishes writing it; OS error kinds differ.
            if reply.len() <= RESPONSE_LIMIT {
                sent.unwrap();
            }
            thread::sleep(pause);
            String::from_utf8(request).unwrap()
        });
        (url, thread)
    }

    fn tcp_server(reply: String) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture received no request");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            socket.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, worker)
    }

    pub(crate) fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn only_literal_loopback_http_addresses_are_accepted() {
        for url in [
            "http://127.0.0.1:7120",
            "http://localhost/",
            "http://LOCALHOST/prefix/",
            "http://[::1]:7120",
        ] {
            assert!(
                endpoint(url, "/api/agents")
                    .unwrap()
                    .path()
                    .ends_with("/api/agents")
            );
        }
        for url in [
            "https://localhost",
            "http://example.com",
            "http://localhost.example.com",
            "http://127.1",
            "http://2130706433",
            "http://0.0.0.0",
            "http://user@localhost",
            "http://localhost/?key=value",
            "http://localhost/#fragment",
            "/relative",
            "http://[::ffff:127.0.0.1]",
            "http://127.0.0.1:99999",
            "http://localhost:invalid",
            "http://[::1]:65536",
            "http://localhost:",
        ] {
            assert!(endpoint(url, "/api/agents").is_none(), "{url}");
        }
    }

    #[test]
    fn reads_are_bounded_and_failures_preserve_stale_metadata_until_recovery() {
        let mut snapshot = Snapshot::default();
        let (url, worker) = server(ok(BODY), Duration::ZERO);
        snapshot.fetch(&url, "fixture-token");
        let request = worker.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/agents http/1.1\r\n"));
        assert!(request.contains("authorization: bearer fixture-token\r\n"));
        assert_eq!(snapshot.states["worker"], "busy");

        let boundary = format!("{BODY}{}", " ".repeat(RESPONSE_LIMIT - BODY.len()));
        let (url, worker) = server(ok(&boundary), Duration::ZERO);
        snapshot.fetch(&url, "");
        worker.join().unwrap();
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.states["worker"], "busy");
        let observed = snapshot.observed_at;

        let redirect = TcpListener::bind("127.0.0.1:0").unwrap();
        redirect.set_nonblocking(true).unwrap();
        for reply in [
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{}/api/agents\r\nContent-Length: 0\r\n\r\n",
                redirect.local_addr().unwrap()
            ),
            ok("private response is not JSON"),
            ok(&"x".repeat(RESPONSE_LIMIT + 1)),
        ] {
            let (url, worker) = server(reply, Duration::ZERO);
            snapshot.fetch(&url, "fixture-token");
            worker.join().unwrap();
            assert_eq!(snapshot.error, Some(crate::UNAVAILABLE));
            assert_eq!(snapshot.states["worker"], "busy");
            assert_eq!(snapshot.observed_at, observed);
        }
        let (url, worker) = tcp_server(format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{}/api/agents\r\nContent-Length: 0\r\n\r\n",
            redirect.local_addr().unwrap()
        ));
        snapshot.fetch(&url, "");
        worker.join().unwrap();
        assert_eq!(snapshot.error, Some(crate::UNAVAILABLE));
        assert_eq!(snapshot.observed_at, observed);
        assert_eq!(
            redirect.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );

        let (url, worker) = server(
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".into(),
            Duration::from_secs(4),
        );
        let started = Instant::now();
        snapshot.fetch(&url, "");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(snapshot.error, Some(crate::UNAVAILABLE));
        assert_eq!(snapshot.observed_at, observed);
        worker.join().unwrap();

        let (url, worker) = server(ok(r#"{"items":[]}"#), Duration::ZERO);
        snapshot.fetch(&url, "");
        assert!(
            !worker
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("authorization:")
        );
        assert!(snapshot.error.is_none() && snapshot.states.is_empty());
        assert!(snapshot.observed_at > observed);
    }

    #[test]
    fn proxy_environment_cannot_receive_the_request() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        for (url, worker) in [server(ok(BODY), Duration::ZERO), tcp_server(ok(BODY))] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "transport::tests::proxy_fixture_child",
                    "--nocapture",
                ])
                .env("ROSTER_FIXTURE_URL", url.replace("127.0.0.1", "localhost"));
            for name in [
                "ALL_PROXY",
                "all_proxy",
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
            ] {
                child.env(name, format!("http://{}", proxy.local_addr().unwrap()));
            }
            child.env_remove("NO_PROXY").env_remove("no_proxy");
            assert!(child.status().unwrap().success());
            assert_eq!(
                worker.join().unwrap().contains("fixture-token"),
                url.starts_with("unix:")
            );
        }
        assert_eq!(
            proxy.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn proxy_fixture_child() {
        let Ok(url) = std::env::var("ROSTER_FIXTURE_URL") else {
            return;
        };
        struct Logs(std::sync::Mutex<String>);
        impl log::Log for Logs {
            fn enabled(&self, _: &log::Metadata<'_>) -> bool {
                true
            }
            fn log(&self, record: &log::Record<'_>) {
                use std::fmt::Write as _;
                writeln!(self.0.lock().unwrap(), "{}", record.args()).unwrap();
            }
            fn flush(&self) {}
        }
        static LOGS: Logs = Logs(std::sync::Mutex::new(String::new()));
        log::set_logger(&LOGS).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        // The Unix client may emit no logs; prove the capture is active anyway.
        log::trace!("fixture log capture active");
        let mut snapshot = Snapshot::default();
        let key = if url.starts_with("unix:") {
            "fixture-token-ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
        } else {
            ""
        };
        snapshot.fetch(&url, key);
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.states["worker"], "busy");
        let logs = LOGS.0.lock().unwrap();
        assert!(!logs.is_empty(), "trace logging must be exercised");
        assert!(!logs.contains("fixture-token") && !logs.contains("ZZZZ"));
        assert!(!logs.to_ascii_lowercase().contains("5a5a"));
    }
}
