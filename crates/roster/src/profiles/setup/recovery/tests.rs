use super::*;
use crate::profiles::setup::{
    Brief,
    tests::{ID, cancelled, guide, prepared, ready},
};
use serde_json::Value;

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn with_brief(brief: Brief) -> Prepared {
    prepared(Agent { brief, ..guide() })
}
fn original() -> Prepared {
    with_brief(Brief::Text {
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
    assert_eq!(restored.profile_id(), prepared.profile_id());
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
            template: "orchestrator-v1".into(),
        },
    ] {
        let restored = round_trip(&with_brief(brief));
        assert!(restored.stream(Action::FirstAgent).finish().is_err());
        assert!(restored.stream(Action::CancelFirstAgent).finish().is_err());
        for (action, result) in [
            (Action::FirstAgent, ready()),
            (Action::CancelFirstAgent, cancelled()),
        ] {
            let mut stream = restored.stream(action);
            let mut result = bytes(&result);
            result.push(b'\n');
            stream.push(&result).unwrap();
            assert_eq!(stream.finish().unwrap().request_id, ID);
        }
    }
}

#[test]
fn divergent_cancellation_and_invalid_original_choices_are_rejected() {
    let checkpoint: Value = serde_json::from_slice(&original().checkpoint().unwrap()).unwrap();
    let first: Value = serde_json::from_str(checkpoint["first"].as_str().unwrap()).unwrap();
    for (field, value) in [
        ("request_id", json!("019a8f3f1a2b7c3d8e4f5a6b7c8d9e10")),
        ("profile_id", json!("0a1b2c3d4e60")),
        ("start_greeting", json!(false)),
        ("action", json!("first_agent")),
    ] {
        let mut invalid = checkpoint.clone();
        let mut cancel: Value = serde_json::from_str(invalid["cancel"].as_str().unwrap()).unwrap();
        cancel[field] = value;
        invalid["cancel"] = json!(cancel.to_string());
        assert!(Prepared::restore(&bytes(&invalid)).is_err(), "{field}");
    }
    for pointer in [
        "/request_id",
        "/profile_id",
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
    body["agent"]["brief"] = json!({"text":"Project work","template":"orchestrator-v1"});
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
            .replace("\"schema_version\":3", "\"schema_version\":4"),
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
fn the_largest_requests_remain_restorable() {
    // Every character of the brief escaped, and a request of the most bytes.
    round_trip(&with_brief(Brief::Text {
        text: "\"".repeat(16_000),
    }));
    let overhead = with_brief(Brief::Text { text: "x".into() })
        .bytes(Action::FirstAgent)
        .len()
        - 1;
    let room = REQUEST_LIMIT - overhead;
    let most = with_brief(Brief::Text {
        text: "\u{1f600}".repeat(room / 4) + &"x".repeat(room % 4),
    });
    assert_eq!(most.bytes(Action::CancelFirstAgent).len(), CANCEL_LIMIT);
    round_trip(&most);
}
