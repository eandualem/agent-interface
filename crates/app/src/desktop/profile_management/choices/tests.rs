use super::*;
use agent_interface_roster::profiles::setup::Action;
use serde_json::{Value, json};
use std::path::Path;
fn fixtures() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn profile() -> Profile {
    Profile::parse(&serde_json::to_vec(&fixtures()["discovery"]["show_ready"]).unwrap()).unwrap()
}
fn catalog(wire: &Value) -> Catalog {
    Catalog::parse(
        &serde_json::to_vec(wire).unwrap(),
        &profile().expected_identity().unwrap(),
    )
    .unwrap()
}
fn choice() -> Selection<'static> {
    Selection {
        name: "worker",
        project: Path::new("/example/project"),
        runtime: "codex",
        model: None,
        brief: "Do the project work",
        hooks: false,
        greeting: false,
    }
}
#[test]
fn explicit_choices_freeze_original_tuple_and_identical_cancel_body() {
    let mut wire = fixtures()["runtime_catalog"]["catalog"].clone();
    wire["items"][0]["models"] = json!(["model-a"]);
    let catalog = catalog(&wire);
    let mut selected = choice();
    selected.model = Some("model-a");
    selected.hooks = true;
    selected.greeting = true;
    let prepared = freeze("chosen-request", &profile(), &catalog, selected).unwrap();
    let first: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    let mut cancel: Value =
        serde_json::from_slice(prepared.bytes(Action::CancelFirstAgent)).unwrap();
    assert_eq!(first["profile_id"], "profile-a");
    assert_eq!(first["expected_installation_id"], "installation-a");
    assert_eq!(first["expected_lifecycle_id"], "lifecycle-a1");
    assert_eq!(first["agent"]["model"], "model-a");
    assert_eq!(first["agent"]["directory"], "/example/project");
    assert_eq!(first["agent"]["create_directory"], false);
    assert_eq!(
        first["agent"]["brief"],
        json!({"text":"Do the project work"})
    );
    assert_eq!(first["hooks"], json!(["codex"]));
    assert_eq!(first["start_greeting"], true);
    cancel["action"] = json!("first_agent");
    assert_eq!(first, cancel);
    // Unauthenticated is an observation, never silently converted to account proof.
    assert_eq!(
        catalog.items()[0].authentication,
        Authentication::NotAuthenticated
    );
}
#[test]
fn changed_origin_or_stale_runtime_model_cannot_prepare() {
    let wire = fixtures()["runtime_catalog"]["catalog"].clone();
    let catalog = catalog(&wire);
    for field in ["profile_id", "installation_id", "lifecycle_id"] {
        let mut other = wire.clone();
        other[field] = json!("replacement");
        let mut expected = profile().expected_identity().unwrap();
        match field {
            "profile_id" => expected.profile_id = "replacement".into(),
            "installation_id" => expected.installation_id = "replacement".into(),
            _ => expected.lifecycle_id = "replacement".into(),
        }
        let other = Catalog::parse(&serde_json::to_vec(&other).unwrap(), &expected).unwrap();
        assert!(freeze("request", &profile(), &other, choice()).is_none());
    }
    let mut selected = choice();
    selected.runtime = "missing";
    assert!(freeze("request", &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.model = Some("missing");
    assert!(freeze("request", &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.project = Path::new("relative");
    assert!(freeze("request", &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.brief = " ";
    assert!(freeze("request", &profile(), &catalog, selected).is_none());
}
#[test]
fn unavailable_setup_is_not_selectable_and_no_optional_action_is_implied() {
    let mut wire = fixtures()["runtime_catalog"]["catalog"].clone();
    let prepared = freeze("request", &profile(), &catalog(&wire), choice()).unwrap();
    let body: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    assert_eq!(body["hooks"], json!([]));
    assert_eq!(body["start_greeting"], false);
    assert_eq!(body["agent"]["model"], Value::Null);
    wire["items"][0]["setup_supported"] = json!(false);
    wire["items"][0]["setup_compatibility"]["compatible"] = json!(false);
    assert!(freeze("request", &profile(), &catalog(&wire), choice()).is_none());
    wire["items"][0]["setup_supported"] = json!(true);
    wire["items"][0]["setup_compatibility"]["compatible"] = json!(false);
    assert!(freeze("request", &profile(), &catalog(&wire), choice()).is_none());
}

#[test]
fn pending_or_unreadable_saved_intent_precedes_any_catalog_command() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executable = root.join("provider");
    let calls = root.join("calls");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf invoked > '{}'\nexit 9\n",
            calls.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    let controller = Controller::new(
        journal::Journal::open(&root.join("profile-setup")).unwrap(),
        &client,
    );
    let prepared = freeze(
        "original",
        &profile(),
        &catalog(&fixtures()["runtime_catalog"]["catalog"]),
        choice(),
    )
    .unwrap();
    let saved = controller.enqueue(prepared).unwrap();
    let state = load_choices(
        client.clone(),
        root.clone(),
        &profile().expected_identity().unwrap(),
        &|| true,
    )
    .unwrap();
    let State::Pending(retained) = state else {
        panic!("pending request must not expose choices")
    };
    assert_eq!(retained.read(&saved.key).unwrap().request_id, "original");
    assert!(!calls.exists());
    assert!(
        load_choices(
            client.clone(),
            root.clone(),
            &profile().expected_identity().unwrap(),
            &|| false
        )
        .is_err()
    );
    assert!(!calls.exists());
    // A malformed second record cannot be treated as an empty pending list.
    let bad = journal::Journal::open(&root.join("profile-setup")).unwrap();
    bad.save("ffffffffffffffffffffffffffffffff", None, b"not a capsule")
        .unwrap();
    let mut other = profile().expected_identity().unwrap();
    other.lifecycle_id = "other-life".into();
    assert!(load_choices(client, root, &other, &|| true).is_err());
    assert!(!calls.exists());
}
