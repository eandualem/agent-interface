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

fn fixture(
    grant: &'static str,
    serve: impl FnOnce(BufReader<UnixStream>) + Send + 'static,
) -> (tempfile::TempDir, Profile, thread::JoinHandle<()>) {
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
        serve(stream);
    });
    (dir, profile, server)
}
#[test]
fn saved_remote_attachment_drives_ghostty_with_the_granted_access() {
    for grant in ["type", "watch"] {
        let (resize_checked, end_stream) = mpsc::channel();
        let (dir, profile, server) = fixture(grant, move |mut stream| {
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
            end_stream.recv_timeout(WAIT).unwrap();
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
        // A previous version retains unknown fields. Its known local
        // attachment and later chooser must both retire the stream target.
        let mut older: Value = serde_json::from_str(saved.raw_json()).unwrap();
        let tree = &mut older["workspaces"][0]["tabs"][0]["tree"];
        assert_eq!(tree["agent"], "worker");
        assert_eq!(tree["source_socket"], "/");
        tree["source_socket"] = json!("/tmp/fixture-local.sock");
        let replaced = agent_interface_store::Document::parse(older.to_string(), &path).unwrap();
        assert!(
            replaced
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .remote_profile
                .is_none()
        );
        let tree = &mut older["workspaces"][0]["tabs"][0]["tree"];
        tree.as_object_mut().unwrap().remove("source_socket");
        tree["agent"] = Value::Null;
        tree["empty"] = json!(true);
        let cleared = agent_interface_store::Document::parse(older.to_string(), &path).unwrap();
        assert!(cleared.model().pane().unwrap().empty);
        assert!(cleared.model().pane().unwrap().attachment.is_none());
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
        assert!(remote.set_size(0, 24).is_err());
        remote.set_size(100, 30).unwrap();
        assert!(remote.state.lock().unwrap().paused);
        resize_checked.send(()).unwrap();
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

#[test]
fn a_stalled_repaint_ends_the_view_and_detaches_the_socket() {
    let (_dir, profile, server) = fixture("type", |mut stream| {
        stream
            .get_mut()
            .set_read_timeout(Some(REPAINT_TIMEOUT + WAIT))
            .unwrap();
        let mut byte = [0; 1];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let (stream, writer, close) = profile.connect().unwrap().open_terminal("worker").unwrap();
    let (wake, done) = mpsc::channel();
    let view = Remote::start(stream, writer, close, move || {
        let _ = wake.send(());
    })
    .unwrap();
    done.recv_timeout(REPAINT_TIMEOUT + WAIT).unwrap();
    assert_eq!(
        view.failure().as_deref(),
        Some("Remote terminal repaint timed out")
    );
    server.join().unwrap();
}

#[test]
fn final_frames_survive_a_viewport_write_to_a_closed_peer() {
    let (_dir, profile, server) = fixture("type", |mut stream| {
        stream
            .get_mut()
            .write_all(
                b"{\"type\":\"repaint_end\"}\n{\"type\":\"ended\",\"reason\":\"process_exited\"}\n",
            )
            .unwrap();
    });
    let (mut stream, writer, _close) = profile.connect().unwrap().open_terminal("worker").unwrap();
    server.join().unwrap();
    let state = Mutex::new(State {
        paused: true,
        ..State::default()
    });
    let (tx, rx) = mpsc::sync_channel(3);
    tx.send(Work::Wire(stream.read())).unwrap();
    tx.send(Work::Wire(stream.read())).unwrap();
    assert_eq!(
        run(rx, writer, (80, 24), Access::Type, &state, &|| {}).unwrap(),
        "Remote agent process exited"
    );
}

#[test]
fn an_abrupt_close_reads_as_a_lost_connection() {
    // The service closes the socket without an ended frame.
    let (_dir, profile, server) = fixture("type", |mut stream| {
        stream
            .get_mut()
            .write_all(b"{\"type\":\"repaint_end\"}\n")
            .unwrap();
    });
    let (stream, writer, close) = profile.connect().unwrap().open_terminal("worker").unwrap();
    server.join().unwrap();
    let (wake, woken) = mpsc::channel();
    let view = Remote::start(stream, writer, close, move || {
        let _ = wake.send(());
    })
    .unwrap();
    while !view.is_finished() {
        woken.recv_timeout(WAIT).unwrap();
    }
    assert_eq!(view.failure().as_deref(), Some(CONNECTION_LOST));
}
