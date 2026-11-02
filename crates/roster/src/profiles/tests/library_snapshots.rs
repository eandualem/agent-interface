use super::*;
use crate::library_snapshots::{self as library, Error as ReadError, Kind};
use sha2::{Digest, Sha256};

fn listing() -> Value {
    json!({"items":[{"name":"example","path":"/display/only/example","artifact_id":"artifact-1"}]})
}
fn versions() -> Value {
    json!({"items":[{"revision_id":"7"}],"has_more":false,"next_before_id":null})
}
fn snapshot(text: &str) -> Value {
    json!({"revision_id":"7","label":"version 1","created_at":"2026-01-01T00:00:00Z", "sha256":format!("{:x}",Sha256::digest(text.as_bytes())), "content":text})
}

#[test]
fn retained_artifacts_keep_delayed_reads_credentials_and_same_id_origins_separate() {
    let (a_dir, a_listener, a_url) = crate::transport::tests::local_listener();
    let (a_profile, a_identity) = profile_at(a_dir.path(), a_url.strip_prefix("unix:").unwrap());
    let (b_dir, b_listener, b_url) = crate::transport::tests::local_listener();
    let (_, mut b_identity) = profile_at(b_dir.path(), b_url.strip_prefix("unix:").unwrap());
    let mut b_record = fixture()["discovery"]["show_ready"].clone();
    b_record["profile_id"] = json!("profile-b");
    b_record["binding"]["endpoint"]["path"] = json!(b_url.strip_prefix("unix:").unwrap());
    b_record["binding"]["credential_reference"]["path"] = json!(b_dir.path().join("profile.env"));
    b_record["binding"]["tmux"] = b_identity["tmux"].clone();
    b_identity["profile_id"] = json!("profile-b");
    let b_profile = Profile::parse(&serde_json::to_vec(&b_record).unwrap()).unwrap();
    let (arrived, wait) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let a_server = serve_with_hook(
        a_listener,
        vec![
            (200, a_identity),
            (200, listing()),
            (200, versions()),
            (200, snapshot("A original")),
            (
                409,
                fixture()["http_errors"]["binding_mismatch"]["body"].clone(),
            ),
        ],
        move |at| {
            if at == 2 {
                arrived.send(()).unwrap();
                held.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        },
    );
    let b_server = serve(
        b_listener,
        vec![
            (200, b_identity),
            (200, listing()),
            (200, versions()),
            (200, snapshot("B original")),
        ],
    );
    let a = a_profile.connect().unwrap();
    let a_target = library::list(&a, Kind::Skill, &|| true)
        .unwrap()
        .remove(0)
        .artifact
        .unwrap();
    let delayed = a_target.clone();
    let job = thread::spawn(move || delayed.latest(&|| true).unwrap());
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
    fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
    let b = b_profile.connect().unwrap();
    let b_target = library::list(&b, Kind::Skill, &|| true)
        .unwrap()
        .remove(0)
        .artifact
        .unwrap();
    let b_snapshot = b_target.latest(&|| true).unwrap();
    assert_eq!(b_snapshot.content.as_deref(), Some("B original"));
    release.send(()).unwrap();
    let a_snapshot = job.join().unwrap();
    assert_eq!(a_snapshot.content.as_deref(), Some("A original"));
    assert_ne!(
        a_snapshot.artifact.identity(),
        b_snapshot.artifact.identity()
    );
    assert_eq!(
        a_target.latest(&|| true).err(),
        Some(ReadError::Connection(Error::BindingMismatch))
    );
    for (requests, kind, profile) in [
        (a_server.join().unwrap(), "skills", "profile-a"),
        (b_server.join().unwrap(), "skills", "profile-b"),
    ] {
        assert!(requests[1].starts_with(&format!("get /api/{kind} ")));
        assert!(requests[2].starts_with(&format!("get /api/{kind}/artifact-1/versions?limit=1 ")));
        assert!(requests[3].starts_with(&format!("get /api/{kind}/artifact-1/versions/7 ")));
        for request in requests {
            assert!(request.contains("authorization: bearer fixture-original\r\n"));
            assert!(request.contains(&format!("x-backbone-profile-id: {profile}\r\n")));
            assert!(request.contains("x-backbone-installation-id: installation-a\r\n"));
            assert!(request.contains("x-backbone-lifecycle-id: lifecycle-a1\r\n"));
        }
    }
}

#[test]
fn malformed_snapshots_cannot_be_returned_as_selected_content() {
    let mut bad = Vec::new();
    let mut wrong = snapshot("text");
    wrong["revision_id"] = json!("8");
    bad.push(wrong);
    let mut wrong = snapshot("text");
    wrong["sha256"] = json!("0".repeat(64));
    bad.push(wrong);
    let mut wrong = snapshot("text");
    wrong.as_object_mut().unwrap().remove("content");
    bad.push(wrong);
    bad.push(json!(["7", "label", "date", null, null]));
    for body in bad {
        let (dir, listener, url) = crate::transport::tests::local_listener();
        let (profile, identity) = profile_at(dir.path(), url.strip_prefix("unix:").unwrap());
        let server = serve(
            listener,
            vec![
                (200, identity),
                (200, listing()),
                (200, versions()),
                (200, body),
            ],
        );
        let connection = profile.connect().unwrap();
        let target = library::list(&connection, Kind::Skill, &|| true)
            .unwrap()
            .remove(0)
            .artifact
            .unwrap();
        assert_eq!(target.latest(&|| true).err(), Some(ReadError::Invalid));
        assert_eq!(server.join().unwrap().len(), 4);
    }
}

#[test]
fn cancellation_after_version_selection_stops_before_snapshot_read() {
    let (dir, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), url.strip_prefix("unix:").unwrap());
    let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stop = live.clone();
    let server = serve_with_hook(
        listener,
        vec![(200, identity), (200, listing()), (200, versions())],
        move |at| {
            if at == 2 {
                stop.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        },
    );
    let connection = profile.connect().unwrap();
    let target = library::list(&connection, Kind::Policy, &|| true)
        .unwrap()
        .remove(0)
        .artifact
        .unwrap();
    assert_eq!(
        target
            .latest(&|| live.load(std::sync::atomic::Ordering::SeqCst))
            .err(),
        Some(ReadError::Cancelled)
    );
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn catalogs_reject_ambiguous_or_unsafe_targets_and_missing_snapshots_remain_missing() {
    let mut duplicate = listing();
    duplicate["items"]
        .as_array_mut()
        .unwrap()
        .push(listing()["items"][0].clone());
    let mut traversal = listing();
    traversal["items"][0]["artifact_id"] = json!("../other");
    for body in [
        duplicate,
        traversal,
        json!({"items":[["name","path","id"]]}),
    ] {
        let (dir, listener, url) = crate::transport::tests::local_listener();
        let (profile, identity) = profile_at(dir.path(), url.strip_prefix("unix:").unwrap());
        let server = serve(listener, vec![(200, identity), (200, body)]);
        let connection = profile.connect().unwrap();
        assert_eq!(
            library::list(&connection, Kind::Skill, &|| true).err(),
            Some(ReadError::Invalid)
        );
        assert_eq!(server.join().unwrap().len(), 2);
    }
    let (dir, listener, url) = crate::transport::tests::local_listener();
    let (profile, identity) = profile_at(dir.path(), url.strip_prefix("unix:").unwrap());
    let mut missing = snapshot("unused");
    missing["content"] = Value::Null;
    missing["sha256"] = Value::Null;
    let server = serve(
        listener,
        vec![
            (200, identity),
            (200, listing()),
            (200, versions()),
            (200, missing),
        ],
    );
    let connection = profile.connect().unwrap();
    let artifact = library::list(&connection, Kind::Policy, &|| true)
        .unwrap()
        .remove(0)
        .artifact
        .unwrap();
    let missing = artifact.latest(&|| true).unwrap();
    assert_eq!(missing.revision_id, "7");
    assert!(missing.content.is_none());
    assert_eq!(missing.artifact.kind(), Kind::Policy);
    assert_eq!(missing.artifact.id(), "artifact-1");
    assert_eq!(server.join().unwrap().len(), 4);
}
