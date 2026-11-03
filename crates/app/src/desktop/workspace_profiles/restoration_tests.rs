//! Exercise persisted bindings through the command/handshake/desktop boundary.

use super::*;
use crate::{
    library::WindowLayout,
    roster::content::tests::{Reply, provider_with_tmux},
};
use agent_interface_store::Document;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::mpsc, thread, time::Duration};

fn handshake(label: &str, root: &Path) -> Reply {
    Reply::ok(
        "/api/profile",
        json!({
            "schema_version":1, "profile_id":format!("profile-{label}"),
            "installation_id":format!("installation-{label}"),
            "lifecycle_id":format!("lifecycle-{label}"), "baseline_version":"fixture",
            "tmux":{"socket_path":root.join(format!("{label}.sock"))}
        }),
    )
}

fn discovery(root: &Path, providers: &[(&Path, &Connection)]) -> Client {
    for (directory, connection) in providers {
        let identity = connection.identity();
        let record = json!({
            "schema_version":1, "profile_id":identity.profile_id,
            "display_name":"Same display name", "kind":"isolated", "state":"ready",
            "binding":{
                "installation_id":identity.installation_id, "lifecycle_id":identity.lifecycle_id,
                "endpoint":{"kind":"unix","path":directory.join("api.sock")},
                "credential_reference":{"kind":"dotenv","path":connection.credential_reference(),"key":"BACKBONE_API_KEY"},
                "tmux":{"socket_path":connection.tmux_socket()}
            }
        });
        fs::write(
            root.join(format!("{}.json", identity.profile_id)),
            record.to_string(),
        )
        .unwrap();
    }
    let executable = root.join("provider");
    // All paths derive from this disposable executable; inherited caller context
    // is neither cleared nor used to find another provider or credential.
    fs::write(
        &executable,
        r#"#!/bin/sh
base=$(/usr/bin/dirname "$0")
[ "$#" = 5 ] && [ "$1 $2 $3 $4" = 'profiles show --json --' ] || exit 9
case "$5" in profile-a|profile-b) ;; *) exit 9 ;; esac
printf '%s\n' "$5" >> "$base/calls"
/bin/cat "$base/$5.json"
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    Client::new(executable, "/usr/bin:/bin".into()).unwrap()
}

fn restored_browser(root: &Path, profiles: &[&str]) -> Browser {
    let path = root.join("layout.json");
    let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
    let base = Document::parse(raw.into(), &path).unwrap();
    let mut model = Layout::initial(root.to_string_lossy().into_owned());
    for (index, profile) in profiles.iter().enumerate() {
        if index > 0 {
            model.add_workspace("Same display name".into());
            model.add_tab(None, root.to_string_lossy().into_owned(), true);
        }
        model.workspaces[index].profile_id = Some((*profile).into());
    }
    fs::write(&path, base.with_model(&model, &path).unwrap().raw_json()).unwrap();
    let restored = Document::parse(fs::read_to_string(&path).unwrap(), &path).unwrap();
    Browser {
        layout: Some(WindowLayout::new(restored)),
        ..Browser::default()
    }
}

fn model(browser: &Browser) -> Layout {
    browser.layout.as_ref().unwrap().model().clone()
}

#[test]
fn restored_workspace_change_discards_in_flight_handshake_without_retargeting_sibling() {
    let root = tempfile::tempdir().unwrap();
    let (arrived, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut delayed = handshake("a", root.path());
    delayed.delay = Some((arrived, held));
    let (a_dir, a, a_server) =
        provider_with_tmux("a", vec![delayed], Some(root.path().join("a.sock")));
    let (b_dir, b, b_server) = provider_with_tmux(
        "b",
        vec![handshake("b", root.path()), handshake("b", root.path())],
        Some(root.path().join("b.sock")),
    );
    let client = discovery(root.path(), &[(a_dir.path(), &a), (b_dir.path(), &b)]);
    let mut browser = restored_browser(root.path(), &["profile-a", "profile-b"]);
    let saved = model(&browser);
    let pane_a = saved.workspaces[0].tabs[0].tree.leaves()[0].id.clone();
    let pane_b = saved.workspaces[1].tabs[0].tree.leaves()[0].id.clone();
    assert!(browser.pane_connection(&pane_a).is_err());
    assert!(browser.pane_connection(&pane_b).is_err());
    let mut work = browser
        .workspace_profiles
        .reconcile(&saved, Some(&client))
        .into_iter();
    let pending_a = work.next().unwrap();
    let pending_b = work.next().unwrap();
    assert!(work.next().is_none());
    let job = thread::spawn(move || pending_a.run());
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(browser.workspace_profiles.accept(pending_b.run()));
    let retained_b = browser.pane_connection(&pane_b).unwrap().unwrap();
    assert_eq!(retained_b.identity(), b.identity());

    browser.layout.as_mut().unwrap().model_mut().workspaces[0].profile_id =
        Some("profile-b".into());
    let changed = model(&browser);
    browser.workspace_profiles.reconcile(&changed, None);
    release.send(()).unwrap();
    assert!(!browser.workspace_profiles.accept(job.join().unwrap()));
    assert!(browser.pane_connection(&pane_a).is_err());
    assert!(
        browser
            .pane_connection(&pane_b)
            .unwrap()
            .unwrap()
            .connection_key()
            == retained_b.connection_key()
    );
    let next = browser
        .workspace_profiles
        .reconcile(&changed, Some(&client));
    assert_eq!(next.len(), 1);
    assert!(
        browser
            .workspace_profiles
            .accept(next.into_iter().next().unwrap().run())
    );
    let rebound = browser.pane_connection(&pane_a).unwrap().unwrap();
    assert_eq!(rebound.identity(), b.identity());
    assert!(rebound.connection_key() != retained_b.connection_key());
    assert_eq!(retained_b.tmux_socket(), b.tmux_socket());
    assert_eq!(
        fs::read_to_string(root.path().join("calls")).unwrap(),
        "profile-a\nprofile-b\nprofile-b\n"
    );
    a_server.join().unwrap();
    b_server.join().unwrap();
}

#[test]
fn restoration_refusal_requires_explicit_retry_and_leaves_other_workspace_connected() {
    let root = tempfile::tempdir().unwrap();
    let refusal = Reply {
        status: 409,
        ..Reply::ok(
            "/api/profile",
            json!({"detail":{"code":"binding_mismatch"}}),
        )
    };
    let (a_dir, a, a_server) = provider_with_tmux(
        "a",
        vec![refusal, handshake("a", root.path())],
        Some(root.path().join("a.sock")),
    );
    let (b_dir, b, b_server) = provider_with_tmux(
        "b",
        vec![handshake("b", root.path())],
        Some(root.path().join("b.sock")),
    );
    let client = discovery(root.path(), &[(a_dir.path(), &a), (b_dir.path(), &b)]);
    let mut browser = restored_browser(root.path(), &["profile-a", "profile-b"]);
    let saved = model(&browser);
    let mut attempts = browser.workspace_profiles.reconcile(&saved, Some(&client));
    assert_eq!(attempts.len(), 2);
    // Establish B first so accepting A's refusal must preserve an existing
    // sibling connection, rather than allowing B to connect after the failure.
    assert!(
        browser
            .workspace_profiles
            .accept(attempts.pop().unwrap().run())
    );
    let retained = match browser.workspace_profiles.route(&saved.workspaces[1]) {
        Route::Bound(c) => c.clone(),
        _ => panic!("B must remain connected"),
    };
    assert_eq!(retained.identity(), b.identity());
    assert!(
        browser
            .workspace_profiles
            .accept(attempts.pop().unwrap().run())
    );
    assert!(matches!(
        browser.workspace_profiles.route(&saved.workspaces[0]),
        Route::Unavailable
    ));
    assert!(
        matches!(browser.workspace_profiles.route(&saved.workspaces[1]), Route::Bound(c) if c.connection_key() == retained.connection_key())
    );
    assert!(
        browser
            .workspace_profiles
            .reconcile(&saved, Some(&client))
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("calls")).unwrap(),
        "profile-b\nprofile-a\n"
    );
    browser.workspace_profiles.retry(&saved.workspaces[0]);
    let retry = browser.workspace_profiles.reconcile(&saved, Some(&client));
    assert_eq!(retry.len(), 1);
    assert!(
        browser
            .workspace_profiles
            .accept(retry.into_iter().next().unwrap().run())
    );
    assert!(
        matches!(browser.workspace_profiles.route(&saved.workspaces[0]), Route::Bound(c) if c.identity() == a.identity())
    );
    assert!(
        matches!(browser.workspace_profiles.route(&saved.workspaces[1]), Route::Bound(c) if c.connection_key() == retained.connection_key())
    );
    assert_eq!(
        fs::read_to_string(root.path().join("calls")).unwrap(),
        "profile-b\nprofile-a\nprofile-a\n"
    );
    a_server.join().unwrap();
    b_server.join().unwrap();
}

#[test]
fn restore_budget_and_removed_workspace_invalidate_queued_work_before_discovery() {
    let root = tempfile::tempdir().unwrap();
    let (a_dir, a, server) = provider_with_tmux(
        "a",
        (0..3).map(|_| handshake("a", root.path())).collect(),
        Some(root.path().join("a.sock")),
    );
    let client = discovery(root.path(), &[(a_dir.path(), &a)]);
    let browser = restored_browser(root.path(), &["profile-a", "profile-a", "profile-a"]);
    let mut saved = model(&browser);
    let mut registry = Registry::default();
    let first = registry.reconcile(&saved, Some(&client));
    assert_eq!(first.len(), 2);
    assert!(registry.reconcile(&saved, Some(&client)).is_empty());
    for work in first {
        assert!(registry.accept(work.run()));
    }
    let mut queued = registry.reconcile(&saved, Some(&client));
    assert_eq!(queued.len(), 1);
    let removed = saved.workspaces.pop().unwrap();
    registry.reconcile(&saved, None);
    saved.workspaces.push(removed);
    let fresh = registry.reconcile(&saved, Some(&client));
    assert_eq!(fresh.len(), 1);
    assert!(!registry.accept(queued.pop().unwrap().run()));
    assert_eq!(
        fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(registry.accept(fresh.into_iter().next().unwrap().run()));
    for space in &saved.workspaces {
        assert!(matches!(registry.route(space), Route::Bound(c) if c.identity() == a.identity()));
    }
    assert_eq!(
        fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    server.join().unwrap();
}
