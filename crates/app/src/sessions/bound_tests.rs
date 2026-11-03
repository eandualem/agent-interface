use super::*;
use crate::roster::content::tests::{Reply, provider_with_tmux};
use agent_interface_test_support::TmuxServer;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

fn socket(server: &TmuxServer) -> PathBuf {
    server
        .run(&["display-message", "-p", "#{socket_path}"])
        .unwrap()
        .trim_end()
        .into()
}
fn identity(label: &str, socket: &std::path::Path) -> Reply {
    Reply::ok(
        "/api/profile",
        json!({"schema_version":1,"profile_id":format!("profile-{label}"),
        "installation_id":format!("installation-{label}"),"lifecycle_id":format!("lifecycle-{label}"),
        "baseline_version":"fixture","tmux":{"socket_path":socket}}),
    )
}
fn take(source: &SessionSource, ready: &mpsc::Receiver<()>) -> Observation {
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    source.take_update().unwrap()
}

#[test]
fn bound_session_polling_keeps_same_names_on_original_sockets_and_credentials() {
    let tmux_a = TmuxServer::start().unwrap();
    let tmux_b = TmuxServer::start().unwrap();
    tmux_a.run(&["new-session", "-d", "-s", "only-a"]).unwrap();
    tmux_b.run(&["new-session", "-d", "-s", "only-b"]).unwrap();
    let sa = socket(&tmux_a);
    let sb = socket(&tmux_b);
    let (arrived, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let (_dir_a, a, server_a) = provider_with_tmux(
        "a",
        vec![
            Reply {
                delay: Some((arrived, held)),
                ..identity("a", &sa)
            },
            identity("a", &sa),
            identity("a", &sa),
            identity("a", &sa),
        ],
        Some(sa),
    );
    let (_dir_b, b, server_b) =
        provider_with_tmux("b", vec![identity("b", &sb), identity("b", &sb)], Some(sb));
    let (wake_a, ready_a) = mpsc::channel();
    let (wake_b, ready_b) = mpsc::channel();
    let source_a = SessionSource::start_bound(a.clone(), move || {
        let _ = wake_a.send(());
    })
    .unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    let source_b = SessionSource::start_bound(b.clone(), move || {
        let _ = wake_b.send(());
    })
    .unwrap();
    let observed_b = take(&source_b, &ready_b);
    assert_eq!(observed_b.identity.as_ref(), Some(b.identity()));
    assert_eq!(observed_b.sessions, ["fixture", "only-b"]);
    assert!(observed_b.error.is_none());
    release.send(()).unwrap();
    let observed_a = take(&source_a, &ready_a);
    assert_eq!(observed_a.identity.as_ref(), Some(a.identity()));
    assert_eq!(observed_a.sessions, ["fixture", "only-a"]);
    assert!(observed_a.error.is_none());
    source_a.set_active(false);
    assert!(source_a.take_update().is_none());
    source_a.set_active(true);
    let observed = take(&source_a, &ready_a);
    assert_eq!(observed.sessions, ["fixture", "only-a"]);
    assert_eq!(observed.identity.as_ref(), Some(a.identity()));
    assert!(observed.observed_at.is_some());
    drop(source_a);
    drop(source_b);
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn post_query_binding_refusal_discards_even_successful_private_session_data() {
    let tmux = TmuxServer::start().unwrap();
    let socket = socket(&tmux);
    let (_dir, connection, server) = provider_with_tmux(
        "a",
        vec![
            identity("a", &socket),
            Reply {
                status: 409,
                replace: true,
                ..Reply::ok(
                    "/api/profile",
                    json!({"detail":{"code":"binding_mismatch"}}),
                )
            },
        ],
        Some(socket),
    );
    let source = Source::Bound(connection.clone());
    let observed = source.observe(&|| true).unwrap();
    assert_eq!(observed.identity.as_ref(), Some(connection.identity()));
    assert_eq!(observed.profile_error, Some(ProfileError::BindingMismatch));
    assert!(observed.sessions.is_empty());
    assert!(observed.observed_at.is_none());
    assert!(observed.error.is_some());
    server.join().unwrap();
}

#[test]
fn cancelled_identity_probe_never_publishes_or_runs_a_later_query_stage() {
    let tmux = TmuxServer::start().unwrap();
    let socket = socket(&tmux);
    let (arrived, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let (_dir, connection, server) = provider_with_tmux(
        "a",
        vec![Reply {
            delay: Some((arrived, held)),
            ..identity("a", &socket)
        }],
        Some(socket),
    );
    let live = Arc::new(AtomicBool::new(true));
    let worker_live = live.clone();
    let job = thread::spawn(move || {
        Source::Bound(connection).observe(&|| worker_live.load(Ordering::Acquire))
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    live.store(false, Ordering::Release);
    release.send(()).unwrap();
    assert!(job.join().unwrap().is_none());
    server.join().unwrap();
}
