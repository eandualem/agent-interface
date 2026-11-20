use super::*;
use serde_json::Value;

/// A profile the Rust service lists as serving.
pub(super) fn served() -> Profile {
    Profile::parse(
        &serde_json::to_vec(&json!({
            "state": "serving", "protocol_version": 1, "profile_id": "0a1b2c3d4e5f",
            "display_name": "Work", "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
            "machine": {"id": "5f2c9e0d1b7a4e38"}, "socket": "/run/profile/api.sock",
            "tmux_socket": "/run/profile/tmux.sock", "credential_reference": {"type": "socket"}
        }))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_create_request_is_its_canonical_body_and_refuses_invalid_input() {
    let create = Prepared::create("operation-create-a", "Work").unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(create.bytes()).unwrap(),
        json!({"request_id": "operation-create-a", "action": "create", "display_name": "Work"})
    );
    assert_eq!(create.action(), Action::Create);
    assert_eq!(create.request_id(), "operation-create-a");
    assert_eq!(create.display_name().as_deref(), Some("Work"));
    assert!(create.identity().is_none() && create.terminal().is_none());
    for (id, name) in [("", "Work"), ("operation", " "), ("operation", "a\nb")] {
        assert!(matches!(Prepared::create(id, name), Err(Error::Invalid)));
    }
    assert!(matches!(
        Prepared::create(&"x".repeat(REQUEST_ID_LIMIT + 1), "Work"),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Prepared::create("request", &"x".repeat(crate::RESPONSE_LIMIT)),
        Err(Error::Invalid)
    ));
}

#[test]
fn the_first_result_is_kept_and_cannot_change() {
    let profile = served();
    let mut create = Prepared::create("operation-create-a", "Work").unwrap();
    let made = create.observe_created(&profile).unwrap();
    assert_eq!(made.status, Status::Succeeded);
    assert_eq!(
        made.identity
            .as_ref()
            .map(|identity| identity.profile_id.as_str()),
        Some("0a1b2c3d4e5f")
    );
    assert_eq!(create.observe_created(&profile).unwrap(), made);
    assert_eq!(
        create.observe_refused("unavailable").err(),
        Some(Error::BindingMismatch)
    );
    let mut refused = Prepared::create("operation-create-b", "Work").unwrap();
    let failed = refused.observe_refused("unavailable").unwrap();
    assert_eq!(
        (failed.status, failed.error_code.as_deref()),
        (Status::Failed, Some("unavailable"))
    );
    assert_eq!(refused.observe_refused("other").err(), Some(Error::Invalid));
    assert_eq!(
        Prepared::create("operation-create-c", "Work")
            .unwrap()
            .observe_refused("not a code")
            .err(),
        Some(Error::Invalid)
    );
}

#[test]
fn a_create_copying_git_identity_says_so_and_restores_exactly() {
    let create = Prepared::create_copying_git_identity("operation-create-g", "Work").unwrap();
    let body: Value = serde_json::from_slice(create.bytes()).unwrap();
    assert_eq!(body["git_identity"], "copy");
    assert!(create.copies_git_identity());
    let plain = Prepared::create("operation-create-p", "Work").unwrap();
    let body: Value = serde_json::from_slice(plain.bytes()).unwrap();
    assert!(body.get("git_identity").is_none());
    let restored = Prepared::restore(&create.checkpoint().unwrap()).unwrap();
    assert_eq!(restored.bytes(), create.bytes());
}
