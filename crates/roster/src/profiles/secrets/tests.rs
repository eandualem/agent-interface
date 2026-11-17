use super::*;
use crate::profiles::Profile;
use serde_json::{Value as Json, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    thread,
    time::Duration,
};

const REQUEST: &str = "0190c3a2c1107a3b8c9d0e1f2a3b4c5d";

/// The service's own fixtures, copied byte for byte.
fn fixture(name: &str) -> Json {
    serde_json::from_str(match name {
        "secrets" => include_str!("fixtures/secrets.json"),
        "secret-value" => include_str!("fixtures/secret-value.json"),
        _ => include_str!("fixtures/secret-changed.json"),
    })
    .unwrap()
}

/// A served profile on a fake socket answering `replies` in turn, after the
/// identity its connection asks for first; the requests it got come back.
pub(in crate::profiles) fn served(
    replies: Vec<(u16, Json)>,
) -> (Connection, thread::JoinHandle<Vec<String>>) {
    let (directory, listener, url) = crate::transport::tests::local_listener();
    let entry = json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": url.strip_prefix("unix:").unwrap(), "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    });
    let mut identity = entry.clone();
    for field in ["state", "socket", "tmux_socket", "credential_reference"] {
        identity.as_object_mut().unwrap().remove(field);
    }
    let server = serve(
        listener,
        [vec![(200, identity)], replies].concat(),
        directory,
    );
    let profile = Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
    (profile.connect().unwrap(), server)
}

fn serve(
    listener: UnixListener,
    replies: Vec<(u16, Json)>,
    directory: tempfile::TempDir,
) -> thread::JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let _directory = directory;
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let mut request = String::from_utf8(request).unwrap();
            let length = request.lines().find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            });
            if let Some(length) = length {
                let mut body = vec![0; length.trim().parse().unwrap()];
                stream.read_exact(&mut body).unwrap();
                request.push_str(&String::from_utf8(body).unwrap());
            }
            requests.push(request);
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    })
}

#[test]
fn the_list_has_names_only_set_or_not() {
    let (connection, server) = served(vec![(200, fixture("secrets"))]);
    assert_eq!(
        connection.secrets(),
        Ok(vec![
            Secret {
                name: "GITHUB_TOKEN".into(),
                set: true
            },
            Secret {
                name: "TELEGRAM_TOKEN".into(),
                set: false
            },
        ])
    );
    let requests = server.join().unwrap();
    assert!(requests[1].starts_with("GET /api/secrets HTTP/1.1\r\n"));
}

#[test]
fn a_value_is_sent_once_trimmed_and_a_removal_has_no_body() {
    let removed = json!({"name": "TELEGRAM_TOKEN", "set": false, "was_set": true});
    let (connection, server) = served(vec![(200, fixture("secret-changed")), (200, removed)]);
    let value = fixture("secret-value")["value"]
        .as_str()
        .unwrap()
        .to_owned();
    let changed = connection
        .set_secret("TELEGRAM_TOKEN", &format!("  {value} "), REQUEST)
        .unwrap();
    assert_eq!(
        (changed.set, changed.was_set),
        (true, false),
        "a new secret is set"
    );
    let removed = connection.remove_secret("TELEGRAM_TOKEN", REQUEST).unwrap();
    assert_eq!((removed.set, removed.was_set), (false, true));
    let requests = server.join().unwrap();
    let (head, body) = requests[1].split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("PUT /api/secrets/TELEGRAM_TOKEN HTTP/1.1\r\n"));
    assert!(
        format!("{head}\r\n")
            .to_ascii_lowercase()
            .contains(&format!("x-backbone-request-id: {REQUEST}\r\n"))
    );
    let sent: Json = serde_json::from_str(body).unwrap();
    assert_eq!(sent, fixture("secret-value"));
    let (head, body) = requests[2].split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("DELETE /api/secrets/TELEGRAM_TOKEN HTTP/1.1\r\n"));
    assert!(
        format!("{head}\r\n")
            .to_ascii_lowercase()
            .contains(&format!("x-backbone-request-id: {REQUEST}\r\n"))
    );
    assert!(body.is_empty());
}

#[test]
fn a_refusal_keeps_the_services_message() {
    let message = "the value of GITHUB_TOKEN has a control character";
    let refused = json!({"detail": {"code": "invalid_request", "message": message}});
    let (connection, server) = served(vec![(400, refused), (403, json!({}))]);
    assert_eq!(
        connection.set_secret("GITHUB_TOKEN", "token", REQUEST),
        Err(AddError::Refused(message.into()))
    );
    assert_eq!(
        connection.remove_secret("GITHUB_TOKEN", REQUEST),
        Err(AddError::Connection(Error::Refused(403)))
    );
    server.join().unwrap();
}

#[test]
fn a_name_or_value_the_service_would_refuse_is_never_sent() {
    let (connection, server) = served(Vec::new());
    for (name, value) in [
        ("github_token", "token"),
        ("9X", "token"),
        ("GITHUB_TOKEN", "   "),
        ("GITHUB_TOKEN", "two\nlines"),
        ("GITHUB_TOKEN", "a\ttab"),
    ] {
        assert_eq!(
            connection.set_secret(name, value, REQUEST),
            Err(AddError::Connection(Error::Invalid)),
            "{name}"
        );
    }
    let long = "x".repeat(BODY_LIMIT);
    assert_eq!(
        connection.set_secret("GITHUB_TOKEN", &long, REQUEST),
        Err(AddError::Connection(Error::Invalid))
    );
    assert_eq!(
        connection.remove_secret("A-B", REQUEST),
        Err(AddError::Connection(Error::Invalid))
    );
    // Only the connection's own identity read reached the service.
    assert_eq!(server.join().unwrap().len(), 1);
}
