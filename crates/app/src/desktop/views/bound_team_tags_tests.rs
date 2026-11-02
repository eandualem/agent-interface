//! Native adapter fixtures for same-name team and tag observations.
use super::*;
use crate::roster::content::tests::{Reply, provider};
use serde_json::json;
use std::sync::mpsc;

fn agents(label: &str) -> serde_json::Value {
    json!({"items":[{"name":"worker","configured":true,"repo":label,"tags":["coder"]}]})
}
fn tags(label: &str) -> Vec<Reply> {
    vec![
        Reply::ok("/api/agents", agents(label)),
        Reply::ok("/api/skills", json!({"items":[]})),
        Reply::ok(
            "/api/config",
            json!({"agents.shared_policy":{"value":[label]}}),
        ),
    ]
}
fn start(connection: &Connection, request: views::Request) -> (Source, mpsc::Receiver<()>) {
    let (wake, ready) = mpsc::channel();
    let source = Source::start(
        None,
        Some(connection),
        request,
        Duration::from_secs(3600),
        move || {
            let _ = wake.send(());
        },
    )
    .unwrap();
    (source, ready)
}
fn take_tags(source: &Source, ready: &mpsc::Receiver<()>, label: &str) {
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (request, data) = source.take_update().unwrap();
    assert_eq!(request, views::Request::Tags);
    let views::Data::Tags(library) = data.unwrap() else {
        panic!("wrong data")
    };
    assert_eq!(library.agents[0].repo, label);
    assert_eq!(library.global, [label]);
}

#[test]
fn team_to_tags_navigation_and_visibility_keep_original_profile_generation() {
    let (arrived, reading) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut replies = vec![Reply {
        delay: Some((arrived, held)),
        ..Reply::ok("/api/agents", agents("old-a"))
    }];
    // Request change cancels the stale Team chain before its swarms enrichment.
    replies.extend(tags("a-tags"));
    replies.extend(tags("a-resumed"));
    let (_dir, a, server_a) = provider("a", replies);
    let (_other, b, server_b) = provider("b", tags("b-tags"));
    let (source_a, ready_a) = start(&a, views::Request::Team);
    reading.recv_timeout(Duration::from_secs(5)).unwrap();
    source_a.set_request(views::Request::Tags);
    std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    let (source_b, ready_b) = start(&b, views::Request::Tags);
    take_tags(&source_b, &ready_b, "b-tags");
    release.send(()).unwrap();
    take_tags(&source_a, &ready_a, "a-tags");
    source_a.set_active(false);
    assert!(source_a.take_update().is_none());
    source_a.set_active(true);
    take_tags(&source_a, &ready_a, "a-resumed");
    assert!(source_b.take_update().is_none());
    drop(source_a);
    drop(source_b);
    server_a.join().unwrap();
    server_b.join().unwrap();
}

#[test]
fn native_team_data_and_enrichment_refusals_stay_on_bound_transport() {
    let (_dir, connection, server) = provider(
        "a",
        vec![
            Reply::ok("/api/agents", agents("original")),
            Reply::ok("/api/swarms", json!({"items":[]})),
        ],
    );
    let (source, ready) = start(&connection, views::Request::Team);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (request, data) = source.take_update().unwrap();
    assert_eq!(request, views::Request::Team);
    let views::Data::Team(team) = data.unwrap() else {
        panic!("wrong data")
    };
    assert_eq!(team.members[0].repo, "original");
    drop(source);
    server.join().unwrap();
    for request in [views::Request::Team, views::Request::Tags] {
        let mut replies = vec![Reply::ok("/api/agents", agents("must-discard"))];
        let path = if request == views::Request::Tags {
            replies.push(Reply::ok("/api/skills", json!({"items":[]})));
            "/api/config"
        } else {
            "/api/swarms"
        };
        replies.push(Reply {
            status: 409,
            replace: true,
            ..Reply::ok(path, json!({"detail":{"code":"binding_mismatch"}}))
        });
        let (_dir, connection, server) = provider("a", replies);
        let (source, ready) = start(&connection, request.clone());
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (actual, data) = source.take_update().unwrap();
        assert_eq!(actual, request);
        assert_eq!(
            data.err(),
            Some("The original profile binding was refused. Reconnect to that profile.")
        );
        drop(source);
        server.join().unwrap();
    }
}
