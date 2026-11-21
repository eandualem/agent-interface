//! Exercise the native worker and profile opener together without using live agents.
use super::*;
use agent_interface_roster::profiles::Profile;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    thread,
};

const WAIT: Duration = Duration::from_secs(8);

fn receive(listener: &UnixListener, route: &str) -> BufReader<UnixStream> {
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut request = BufReader::new(stream);
    let mut line = String::new();
    request.read_line(&mut line).unwrap();
    assert_eq!(line.trim_end(), format!("{route} HTTP/1.1"));
    let mut profile = false;
    let mut length = 0;
    loop {
        line.clear();
        assert!(request.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        let header = line.trim_end().to_ascii_lowercase();
        profile |= header == "x-backbone-profile-id: fixture-profile";
        if let Some(n) = header.strip_prefix("content-length: ") {
            length = n.parse().unwrap();
        }
    }
    assert!(profile);
    if length > 0 {
        let mut body = vec![0; length];
        request.read_exact(&mut body).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["agent"],
            "worker"
        );
    }
    request
}
fn reply(listener: &UnixListener, path: &str, body: &Value) {
    let mut request = receive(listener, path);
    let body = body.to_string();
    write!(
        request.get_mut(),
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}
fn command(stream: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    stream.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn saved_remote_attachment_drives_ghostty_with_the_granted_access() {
    for grant in ["type", "watch"] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("api.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let info = json!({"state":"serving","protocol_version":1,"profile_id":"fixture-profile","display_name":"Fixture","instance_id":"instance-a","machine":{"id":"local","name":"Mac"},"home":true,"socket":socket,"tmux_socket":dir.path().join("unused.sock"),"credential_reference":{"type":"socket"}});
        let profile = Profile::parse(&serde_json::to_vec(&info).unwrap()).unwrap();
        let server = thread::spawn(move || {
            reply(&listener, "GET /api/profile", &info);
            reply(&listener, "GET /api/profile", &info);
            let mut stream = receive(&listener, "POST /api/terminals/open");
            write!(stream.get_mut(),"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: backbone-terminal-v1\r\n\r\n{{\"type\":\"opened\",\"agent\":\"worker\",\"access\":\"{grant}\",\"cols\":80,\"rows\":24}}\n").unwrap();
            reply(&listener, "GET /api/profile", &info);
            stream.get_mut().write_all(b"{\"type\":\"output\",\"data_b64\":\"cmVhZHkg4g==\"}\n{\"type\":\"output\",\"data_b64\":\"gqw=\"}\n{\"type\":\"repaint_end\"}\n").unwrap();
            if grant == "type" {
                assert_eq!(
                    command(&mut stream),
                    json!({"type":"input","data_b64":"eA=="})
                );
                assert_eq!(
                    command(&mut stream),
                    json!({"type":"resize","cols":90,"rows":28})
                );
                stream
                    .get_mut()
                    .write_all(b"{\"type\":\"resized\",\"cols\":90,\"rows\":28}\n")
                    .unwrap();
            }
            assert_eq!(command(&mut stream), json!({"type":"pause"}));
            stream
                .get_mut()
                .write_all(b"{\"type\":\"ended\",\"reason\":\"access_ended\"}\n")
                .unwrap();
        });
        let store = agent_interface_store::Store::open_or_create(dir.path()).unwrap();
        let doc = store.load().unwrap();
        let mut model = doc.model().clone();
        model.workspaces[0].profile_id = Some("fixture-profile".into());
        assert!(model.add_tab(Some("worker".into()), "/".into(), true));
        assert!(model.attach(Some(agent_interface_core::Attachment {
            session: "worker".into(),
            source_socket: None,
            remote_profile: Some("fixture-profile".into())
        })));
        let path = dir.path().join("layouts.db");
        let saved = doc.with_model(&model, &path).unwrap();
        let restored =
            agent_interface_store::Document::parse(saved.raw_json().into(), &path).unwrap();
        assert_eq!(restored.model().pane(), model.pane());
        let attachment = restored
            .model()
            .pane()
            .unwrap()
            .attachment
            .as_ref()
            .unwrap();
        let connection = profile.connect().unwrap();
        let (stream, writer, close) = connection.open_terminal(&attachment.session).unwrap();
        let (wake, woken) = mpsc::channel();
        let remote = Remote::start(stream, writer, close, move || {
            let _ = wake.send(());
        })
        .unwrap();
        woken.recv_timeout(WAIT).unwrap();
        let update = remote.take_update().unwrap();
        let screen = &update.screens[0].1;
        let row: String = (0..screen.cols())
            .filter_map(|col| screen.cell(0, col).map(|(text, _)| text))
            .collect();
        assert!(row.starts_with("ready €"), "{row}");
        let input = remote.send_reserved_input(PANE, Input::Paste("x".into()), None);
        assert_eq!(input.is_ok(), grant == "type");
        remote.set_size(90, 28).unwrap();
        if grant == "type" {
            loop {
                woken.recv_timeout(WAIT).unwrap();
                if remote.take_update().is_some_and(|u| u.layout[0].cols == 90) {
                    break;
                }
            }
        }
        remote.set_visible(false);
        while !remote.is_finished() {
            woken.recv_timeout(WAIT).unwrap();
        }
        assert_eq!(
            remote.failure().as_deref(),
            Some("Remote terminal access ended")
        );
        let mut detached = restored.model().clone();
        assert!(detached.detach_pane());
        assert!(
            !restored
                .with_model(&detached, &path)
                .unwrap()
                .raw_json()
                .contains("remote_profile")
        );
        server.join().unwrap();
    }
}
