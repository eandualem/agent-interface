use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::thread;

#[derive(Deserialize)]
struct Step {
    direction: String,
    octets: usize,
    wire: String,
}
#[derive(Deserialize)]
struct Fixtures {
    cases: BTreeMap<String, Vec<Step>>,
}

fn request(socket: &mut UnixStream, expected: &str) {
    let (headers, body) = expected.split_once("\r\n\r\n").unwrap();
    let mut received = Vec::new();
    while !received.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        received.push(byte[0]);
        assert!(received.len() < 8192);
    }
    // HTTP field ordering and capitalization are interchangeable.
    let received = String::from_utf8(received).unwrap().to_ascii_lowercase();
    let expected = headers.to_ascii_lowercase();
    assert_eq!(received.lines().next(), expected.lines().next());
    for header in expected.lines().skip(1) {
        assert!(
            received.lines().any(|line| line == header),
            "{header} missing from {received}"
        );
    }
    let mut actual = vec![0; body.len()];
    socket.read_exact(&mut actual).unwrap();
    assert_eq!(actual, body.as_bytes());
}

#[test]
fn service_wire_examples_over_private_sockets() {
    let fixtures: Fixtures = serde_json::from_str(include_str!("wire.json")).unwrap();
    assert_eq!(fixtures.cases.len(), 10);
    for (name, steps) in fixtures.cases {
        let (_directory, listener, path) = crate::transport::tests::local_listener();
        let watch = name == "watch_pause_overrun_resume";
        for step in &steps {
            assert_eq!(step.octets, step.wire.len());
        }
        // Only a 503 tells the pane it may try again by itself.
        let transient = steps
            .iter()
            .any(|step| step.direction == "service" && step.wire.starts_with("HTTP/1.1 503"));
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(TIMEOUT)).unwrap();
            request(&mut socket, &steps[0].wire);
            for step in &steps[1..] {
                if step.direction == "service" {
                    for bytes in step.wire.as_bytes().chunks(11) {
                        socket.write_all(bytes).unwrap();
                    }
                } else {
                    // The service fixture deliberately injects forbidden watch input and
                    // resize. A conforming client suppresses both before the socket.
                    for line in step.wire.split_inclusive('\n') {
                        if watch && (line.contains("\"input\"") || line.contains("\"resize\"")) {
                            continue;
                        }
                        let mut actual = vec![0; line.len()];
                        socket.read_exact(&mut actual).unwrap();
                        assert_eq!(actual, line.as_bytes());
                    }
                }
            }
        });
        let (cols, rows) = if watch { (120, 40) } else { (80, 24) };
        let opened = open(Path::new(&path), "0a1b2c3d4e5f", "worker", cols, rows);
        if let Some(code) = name.strip_prefix("refused_") {
            let error = opened.err().expect("refusal");
            let reason = error.to_string();
            assert!(reason.ends_with(&format!("({code})")), "{reason}");
            assert_eq!(error.kind() == io::ErrorKind::HostUnreachable, transient);
        } else {
            let (mut stream, mut writer, _close) =
                opened.unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(stream.size, (80, 24));
            assert_eq!(
                stream.access,
                if watch { Access::Watch } else { Access::Type }
            );
            let initial = stream.read().unwrap();
            assert_eq!(
                initial,
                Event::Output(
                    STANDARD
                        .decode("G2MbWz8xMDQ5aBtbMkobW0hyZWFkeRtbMTs2SA==")
                        .unwrap()
                )
            );
            assert_eq!(stream.read().unwrap(), Event::RepaintEnd);
            match name.as_str() {
                "type_and_close" => {
                    writer.input(b"\x1b[200~a\xff\0\r\x1b[201~").unwrap();
                    assert_eq!(stream.read().unwrap(), Event::Output(vec![0xe2]));
                    assert_eq!(
                        stream.read().unwrap(),
                        Event::Output(vec![0x82, 0xac, 0xff])
                    );
                    writer.resize(100, 30).unwrap();
                    assert_eq!(stream.read().unwrap(), Event::Resized(100, 30));
                    assert_eq!(
                        stream.read().unwrap(),
                        Event::Ended("process_exited".into())
                    );
                }
                "watch_pause_overrun_resume" => {
                    assert!(writer.input(b"x").is_err());
                    writer.resize(110, 50).unwrap();
                    writer.pause(true).unwrap();
                    writer.pause(false).unwrap();
                    assert_eq!(stream.read().unwrap(), Event::Dropped);
                    assert_eq!(stream.read().unwrap(), initial);
                    assert_eq!(stream.read().unwrap(), Event::RepaintEnd);
                    assert_eq!(stream.read().unwrap(), Event::Output(vec![b'x']));
                    assert_eq!(stream.read().unwrap(), Event::Ended("access_ended".into()));
                }
                "ended_while_paused" => {
                    writer.pause(true).unwrap();
                    assert_eq!(
                        stream.read().unwrap(),
                        Event::Ended("connection_lost".into())
                    );
                }
                "abrupt_eof" => assert_eq!(
                    stream.read().unwrap_err().kind(),
                    io::ErrorKind::UnexpectedEof
                ),
                _ => panic!("unhandled fixture {name}"),
            }
        }
        server.join().unwrap();
    }
}

#[test]
fn a_service_that_closes_before_answering_reads_as_a_lost_connection() {
    let (_directory, listener, path) = crate::transport::tests::local_listener();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        // Drop the socket without a response, as a restarting service does.
    });
    let error = open(Path::new(&path), "0a1b2c3d4e5f", "worker", 80, 24)
        .err()
        .expect("closed before a response");
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "{error}");
    server.join().unwrap();
}

#[test]
fn only_an_unreachable_profile_service_reads_as_not_connected() {
    use super::super::Error;
    let kind = |error| verify_error(error).kind();
    assert_eq!(kind(Error::Unavailable), io::ErrorKind::NotConnected);
    assert_eq!(
        kind(Error::BindingMismatch),
        io::ErrorKind::AddrNotAvailable
    );
    for refusal in [
        Error::Invalid,
        Error::Unsupported,
        Error::NotReady,
        Error::BindingRequired,
        Error::RegistrationChanged,
        Error::Refused(403),
    ] {
        assert_eq!(kind(refusal), io::ErrorKind::PermissionDenied);
    }
}

#[test]
fn large_input_is_split_without_changing_any_bytes() {
    let (socket, peer) = UnixStream::pair().unwrap();
    let mut writer = Writer {
        socket,
        access: Access::Type,
    };
    let bytes = (0..=255u8).cycle().take(2 * CHUNK + 1).collect::<Vec<_>>();
    let sent = bytes.clone();
    let sending = thread::spawn(move || writer.input(&sent).unwrap());
    let mut received = Vec::new();
    for line in BufReader::new(peer).lines() {
        let line = line.unwrap();
        assert!(line.len() < FRAME);
        let frame: Value = serde_json::from_str(&line).unwrap();
        let chunk = STANDARD
            .decode(frame["data_b64"].as_str().unwrap())
            .unwrap();
        assert!(chunk.len() <= CHUNK);
        received.extend(chunk);
    }
    sending.join().unwrap();
    assert_eq!(received, bytes);
}
