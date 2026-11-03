use super::*;
use crate::profiles::{
    Profile,
    setup::{Agent, Brief},
};
use serde_json::Value;

fn fixtures() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn prepared() -> Prepared {
    let profile =
        Profile::parse(&serde_json::to_vec(&fixtures()["discovery"]["show_ready"]).unwrap())
            .unwrap();
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
            brief: Brief::Text {
                text: "Project work".into(),
            },
        },
        false,
    )
    .unwrap()
}
fn event(prepared: &Prepared, action: Action, value: &Value) -> Event {
    let mut stream = prepared.stream(action);
    stream.push(&serde_json::to_vec(value).unwrap()).unwrap();
    stream.finish().unwrap()
}

#[test]
fn readiness_and_cancellation_round_trip_without_inventing_launches_or_progress() {
    let prepared = prepared();
    let fixtures = fixtures();
    for (action, field) in [
        (Action::FirstAgent, "result"),
        (Action::CancelFirstAgent, "cancellation_result"),
    ] {
        let original = event(&prepared, action, &fixtures["setup"][field]);
        let checkpoint = prepared.outcome_checkpoint(&original).unwrap();
        let fresh = Prepared::restore(&prepared.checkpoint().unwrap()).unwrap();
        assert_eq!(
            fresh.restore_outcome(action, &checkpoint).unwrap(),
            original
        );
        let opposite = match action {
            Action::FirstAgent => Action::CancelFirstAgent,
            Action::CancelFirstAgent => Action::FirstAgent,
        };
        assert!(fresh.restore_outcome(opposite, &checkpoint).is_err());
        if action == Action::FirstAgent {
            assert_eq!(
                original.launch_id.as_deref(),
                fixtures["setup"][field]["data"]["launch_id"].as_str()
            );
            assert!(original.launch_id.is_some());
        } else {
            assert!(original.launch_id.is_none());
            assert!(original.cancelled);
        }
    }
}

#[test]
fn nonready_outcomes_keep_retry_policy_without_readiness_or_cancellation() {
    let prepared = prepared();
    for action in [Action::FirstAgent, Action::CancelFirstAgent] {
        for (status, retry) in [
            ("action_required", "same_request"),
            ("failed", "new_request"),
            ("failed", "none"),
        ] {
            let mut value = fixtures()["setup"]["result"].clone();
            value["action"] = match action {
                Action::FirstAgent => "first_agent",
                Action::CancelFirstAgent => "cancel_first_agent",
            }
            .into();
            value["status"] = status.into();
            value["code"] = "action_needed".into();
            value["retry"] = retry.into();
            value["data"] = json!({});
            let original = event(&prepared, action, &value);
            assert!(!original.cancelled);
            assert!(original.ready_agent.is_none());
            assert!(original.launch_id.is_none());
            let saved = prepared.outcome_checkpoint(&original).unwrap();
            assert_eq!(prepared.restore_outcome(action, &saved).unwrap(), original);
        }
    }
}

#[test]
fn provider_prose_and_unneeded_data_are_not_retained() {
    let prepared = prepared();
    let mut value = fixtures()["setup"]["result"].clone();
    value["message"] = "provider-private-detail".into();
    value["data"]["internal_detail"] = "provider-private-detail".into();
    value["completed_steps"] = json!(["provider-private-detail"]);
    let original = event(&prepared, Action::FirstAgent, &value);
    let bytes = prepared.outcome_checkpoint(&original).unwrap();
    assert!(
        !String::from_utf8(bytes.clone())
            .unwrap()
            .contains("provider-private-detail")
    );
    assert_eq!(
        prepared
            .restore_outcome(Action::FirstAgent, &bytes)
            .unwrap(),
        original
    );
}

#[test]
fn progress_and_inconsistent_typed_events_cannot_become_terminal_checkpoints() {
    let prepared = prepared();
    let original = event(
        &prepared,
        Action::FirstAgent,
        &fixtures()["setup"]["result"],
    );
    let mut invalid = Vec::new();
    let mut changed = original.clone();
    changed.step = Some("hooks".into());
    invalid.push(changed);
    let mut changed = original.clone();
    changed.cancelled = true;
    invalid.push(changed);
    let mut changed = original.clone();
    changed.launch_id = None;
    invalid.push(changed);
    let mut changed = original.clone();
    changed.ready_agent = Some("other".into());
    invalid.push(changed);
    let mut changed = original.clone();
    changed.identity.lifecycle_id = "replacement".into();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.request_id = "other".into();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.retry = Some("same_request".into());
    invalid.push(changed);
    let mut changed = original.clone();
    changed.sequence = 0;
    invalid.push(changed);
    let mut changed = original;
    changed.sequence = 1025;
    invalid.push(changed);
    for value in invalid {
        assert!(prepared.outcome_checkpoint(&value).is_err());
    }
}

#[test]
fn malformed_noncanonical_and_foreign_records_fail_closed() {
    let prepared = prepared();
    let original = event(
        &prepared,
        Action::FirstAgent,
        &fixtures()["setup"]["result"],
    );
    let bytes = prepared.outcome_checkpoint(&original).unwrap();
    let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
    envelope["extra"] = true.into();
    assert!(
        prepared
            .restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap())
            .is_err()
    );
    for field in [
        "profile_id",
        "installation_id",
        "lifecycle_id",
        "request_id",
        "action",
    ] {
        let mut envelope: Checkpoint = serde_json::from_slice(&bytes).unwrap();
        let mut result: Value = serde_json::from_str(&envelope.result).unwrap();
        result[field] = "replacement".into();
        envelope.result = serde_json::to_string(&result).unwrap();
        assert!(
            prepared
                .restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap())
                .is_err()
        );
    }
    let mut envelope: Checkpoint = serde_json::from_slice(&bytes).unwrap();
    envelope.result.push(' ');
    assert!(
        prepared
            .restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap())
            .is_err()
    );
    assert!(
        prepared
            .restore_outcome(Action::FirstAgent, &bytes[..bytes.len() - 1])
            .is_err()
    );
    assert!(
        prepared
            .restore_outcome(Action::FirstAgent, &vec![b' '; LIMIT + 1])
            .is_err()
    );
    let mut envelope: Checkpoint = serde_json::from_slice(&bytes).unwrap();
    envelope.format = 2;
    assert_eq!(
        prepared.restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap()),
        Err(Error::Unsupported)
    );
}
