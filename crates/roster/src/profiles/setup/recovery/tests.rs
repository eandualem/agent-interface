use super::*;
use crate::profiles::{Profile, setup::Brief};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn prepared(brief: Brief) -> Prepared {
    let profile = Profile::parse(&bytes(&fixture()["discovery"]["show_ready"])).unwrap();
    Prepared::first_agent(
        "setup-a1",
        &profile,
        vec!["codex".into()],
        Agent {
            name: "worker".into(),
            directory: "/example/project".into(),
            create_directory: false,
            runtime: "codex".into(),
            model: None,
            brief,
        },
        false,
    )
    .unwrap()
}
fn original() -> Prepared {
    prepared(Brief::Text {
        text: "Project work".into(),
    })
}
fn round_trip(prepared: &Prepared) -> Prepared {
    let checkpoint = prepared.checkpoint().unwrap();
    let restored = Prepared::restore(&checkpoint).unwrap();
    assert_eq!(restored.checkpoint().unwrap(), checkpoint);
    for action in [Action::FirstAgent, Action::CancelFirstAgent] {
        assert_eq!(restored.bytes(action), prepared.bytes(action));
    }
    assert_eq!(restored.identity(), prepared.identity());
    assert_eq!(restored.request_id(), prepared.request_id());
    restored
}

#[test]
fn restored_intent_keeps_original_bytes_and_requires_fresh_validated_results() {
    for brief in [
        Brief::Text {
            text: "Project work".into(),
        },
        Brief::Template {
            template: "basic".into(),
        },
    ] {
        let restored = round_trip(&prepared(brief));
        assert!(restored.stream(Action::FirstAgent).finish().is_err());
        assert!(restored.stream(Action::CancelFirstAgent).finish().is_err());
        for (action, key) in [
            (Action::FirstAgent, "result"),
            (Action::CancelFirstAgent, "cancellation_result"),
        ] {
            let mut stream = restored.stream(action);
            let mut result = bytes(&fixture()["setup"][key]);
            result.push(b'\n');
            stream.push(&result).unwrap();
            assert_eq!(stream.finish().unwrap().request_id, "setup-a1");
        }
    }
}

#[test]
fn divergent_cancellation_and_invalid_original_choices_are_rejected() {
    let checkpoint: Value = serde_json::from_slice(&original().checkpoint().unwrap()).unwrap();
    let first: Value = serde_json::from_str(checkpoint["first"].as_str().unwrap()).unwrap();
    for (field, value) in [
        ("request_id", json!("replacement")),
        ("expected_lifecycle_id", json!("replacement")),
        ("start_greeting", json!(true)),
        ("hooks", json!([])),
        ("action", json!("first_agent")),
    ] {
        let mut invalid = checkpoint.clone();
        let mut cancel: Value = serde_json::from_str(invalid["cancel"].as_str().unwrap()).unwrap();
        cancel[field] = value;
        invalid["cancel"] = json!(cancel.to_string());
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    for pointer in [
        "/request_id",
        "/profile_id",
        "/expected_installation_id",
        "/expected_lifecycle_id",
        "/agent/name",
        "/agent/runtime",
        "/agent/directory",
    ] {
        let mut invalid = checkpoint.clone();
        let mut body = first.clone();
        *body.pointer_mut(pointer).unwrap() = json!("");
        invalid["first"] = json!(body.to_string());
        body["action"] = json!("cancel_first_agent");
        invalid["cancel"] = json!(body.to_string());
        assert!(Prepared::restore(&bytes(&invalid)).is_err(), "{pointer}");
    }
    let mut invalid = checkpoint.clone();
    let mut body = first.clone();
    body["agent"]["brief"] = json!({"text":"Project work","template":"another"});
    invalid["first"] = json!(body.to_string());
    body["action"] = json!("cancel_first_agent");
    invalid["cancel"] = json!(body.to_string());
    assert!(Prepared::restore(&bytes(&invalid)).is_err());
}

#[test]
fn unsupported_malformed_and_normalized_checkpoints_cannot_be_replayed() {
    let checkpoint: Value = serde_json::from_slice(&original().checkpoint().unwrap()).unwrap();
    for (field, value) in [
        ("checkpoint_version", json!(2)),
        ("first", json!("[]")),
        ("cancel", json!(null)),
        ("unknown", json!(true)),
    ] {
        let mut invalid = checkpoint.clone();
        invalid[field] = value;
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    for body in [
        format!(" {}", checkpoint["first"].as_str().unwrap()),
        checkpoint["first"]
            .as_str()
            .unwrap()
            .replacen('{', "{\"request_id\":\"other\",", 1),
        checkpoint["first"]
            .as_str()
            .unwrap()
            .replace("\"schema_version\":2", "\"schema_version\":1"),
    ] {
        let mut invalid = checkpoint.clone();
        invalid["first"] = json!(body);
        assert!(Prepared::restore(&bytes(&invalid)).is_err());
    }
    assert!(Prepared::restore(&vec![b' '; CHECKPOINT_LIMIT + 1]).is_err());
    assert!(
        Prepared::restore(&bytes(&json!([
            1,
            checkpoint["first"],
            checkpoint["cancel"]
        ])))
        .is_err()
    );
}

#[test]
fn maximum_request_size_with_escaped_brief_remains_restorable() {
    let overhead = prepared(Brief::Text { text: "x".into() })
        .bytes(Action::CancelFirstAgent)
        .len()
        - 1;
    let remaining = REQUEST_LIMIT - overhead;
    let text = "\"".repeat(remaining / 2) + &"x".repeat(remaining % 2);
    let prepared = prepared(Brief::Text { text });
    assert_eq!(
        prepared.bytes(Action::CancelFirstAgent).len(),
        REQUEST_LIMIT
    );
    round_trip(&prepared);
}
