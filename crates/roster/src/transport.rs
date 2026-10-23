use std::{
    io::Read,
    net::{Ipv6Addr, SocketAddr},
    time::Duration,
};

use http::{HeaderValue, Uri};

use crate::{RESPONSE_LIMIT, Snapshot};

impl Snapshot {
    /// Read one agents response from an explicitly supplied local API address.
    ///
    /// This blocks for at most three seconds and belongs on an I/O worker.
    /// It reads no configuration or credentials itself. Only HTTP loopback
    /// addresses are accepted; redirects and proxies are disabled. `key`, when
    /// nonempty, is sent only as the validated endpoint's bearer token. Every
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
    let uri = endpoint(url, path)?;
    let client = reqwest::blocking::Client::builder()
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
        .ok()?;
    // Set the request's total deadline, including the response body, rather
    // than only a timeout renewed for each blocking read.
    let mut request = client.get(uri.to_string()).timeout(Duration::from_secs(3));
    if !key.is_empty() {
        let mut value = HeaderValue::from_str(&format!("Bearer {key}")).ok()?;
        value.set_sensitive(true);
        request = request.header("Authorization", value);
    }
    let response = request.send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    // Read one extra byte to distinguish an exact-size body from an oversized
    // one. Snapshot::update keeps the inclusive limit when decoding.
    let mut bytes = Vec::new();
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    const BODY: &str = r#"{"items":[{"name":"worker","state":"busy"}]}"#;

    pub(crate) fn server(reply: String, pause: Duration) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
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
        snapshot.fetch(&format!("{url}/prefix/"), "fixture-token");
        let request = worker.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /prefix/api/agents http/1.1\r\n"));
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
        let (url, worker) = server(ok(BODY), Duration::ZERO);
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
        assert!(worker.join().unwrap().contains("fixture-token"));
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
        let mut snapshot = Snapshot::default();
        snapshot.fetch(&url, "fixture-token-ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ");
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.states["worker"], "busy");
        let logs = LOGS.0.lock().unwrap();
        assert!(!logs.is_empty(), "trace logging must be exercised");
        assert!(!logs.contains("fixture-token") && !logs.contains("ZZZZ"));
        assert!(!logs.to_ascii_lowercase().contains("5a5a"));
    }
}
