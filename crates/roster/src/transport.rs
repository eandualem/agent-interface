pub(crate) mod local;

#[cfg(test)]
pub(super) mod tests {
    use crate::RESPONSE_LIMIT;
    use std::io::{Read, Write};
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    use std::thread;
    use std::time::{Duration, Instant};

    pub(crate) fn local_listener() -> (tempfile::TempDir, UnixListener, String) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("api.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let socket = path.to_str().unwrap().to_owned();
        (directory, listener, socket)
    }

    /// A Rust-service profile connected through a private fixture socket. The
    /// listener is returned after the connection's handshake, for the test's
    /// own replies.
    pub(crate) fn served_connection()
    -> (tempfile::TempDir, crate::profiles::Connection, UnixListener) {
        let (directory, listener, socket) = local_listener();
        let mut entry = serde_json::json!({
            "state": "serving", "protocol_version": 1,
            "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
            "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
            "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
            "socket": &socket,
            "tmux_socket": directory.path().join("tmux.sock"),
            "credential_reference": {"type": "socket"}
        });
        let profile =
            crate::profiles::Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
        for field in ["state", "socket", "tmux_socket", "credential_reference"] {
            entry.as_object_mut().unwrap().remove(field);
        }
        let (health_listener, identity) =
            (listener.try_clone().unwrap(), listener.try_clone().unwrap());
        let handshake = thread::spawn(move || {
            let version = crate::profiles::MINIMUM_BACKBONE.to_string();
            let health = ok(&serde_json::json!({"version": version}).to_string());
            serve_on(health_listener, None, health, Duration::ZERO)
                .join()
                .unwrap();
            serve_on(identity, None, ok(&entry.to_string()), Duration::ZERO)
                .join()
                .unwrap()
        });
        let connection = profile.connect().unwrap();
        handshake.join().unwrap();
        (directory, connection, listener)
    }

    /// Answer one request on `listener`, keeping `directory` alive until then.
    pub(crate) fn serve_on(
        listener: UnixListener,
        directory: Option<tempfile::TempDir>,
        reply: String,
        pause: Duration,
    ) -> thread::JoinHandle<String> {
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
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
        })
    }

    #[test]
    fn a_socket_or_folder_open_to_others_gets_no_connection() {
        let (directory, listener, socket) = local_listener();
        listener.set_nonblocking(true).unwrap();
        for (path, mode, private) in [
            (directory.path().join("api.sock"), 0o666, 0o600),
            (directory.path().to_owned(), 0o755, 0o700),
        ] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let response = super::local::profile_response(
                &socket,
                "/api/profile",
                "0a1b2c3d4e5f",
                None,
                1024,
                Duration::from_secs(1),
            );
            assert!(response.is_err());
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(private)).unwrap();
        }
    }

    #[test]
    fn an_unfinished_body_fails_within_the_request_time() {
        let (directory, listener, socket) = local_listener();
        let worker = serve_on(
            listener,
            Some(directory),
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n".into(),
            Duration::from_secs(4),
        );
        let started = Instant::now();
        let response = super::local::profile_response(
            &socket,
            "/api/agents",
            "0a1b2c3d4e5f",
            None,
            1024,
            Duration::from_secs(1),
        );
        assert!(response.is_err());
        assert!(started.elapsed() < Duration::from_secs(4));
        worker.join().unwrap();
    }

    pub(crate) fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}
