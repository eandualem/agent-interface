use super::*;
use crate::profiles::setup::tests::{answer, cancelled, guide, prepared, ready};
use serde_json::Value;

fn event(prepared: &Prepared, action: Action, value: &Value) -> Event {
    let mut stream = prepared.stream(action);
    stream.push(&serde_json::to_vec(value).unwrap()).unwrap();
    stream.finish().unwrap()
}

#[test]
fn readiness_and_cancellation_round_trip_without_inventing_launches_or_progress() {
    let prepared = prepared(guide());
    for (action, value) in [
        (Action::FirstAgent, ready()),
        (Action::CancelFirstAgent, cancelled()),
    ] {
        let original = event(&prepared, action, &value);
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
                value["data"]["launch_id"].as_str()
            );
            assert!(original.launch_id.is_some());
        } else {
            assert!(original.launch_id.is_none());
            assert!(original.cancelled);
        }
    }
    // A ready agent whose launch was not named stays so.
    let mut unnamed = ready();
    unnamed["data"]["launch_id"] = Value::Null;
    let original = event(&prepared, Action::FirstAgent, &unnamed);
    let saved = prepared.outcome_checkpoint(&original).unwrap();
    assert_eq!(
        prepared
            .restore_outcome(Action::FirstAgent, &saved)
            .unwrap(),
        original
    );
}

#[test]
fn nonready_outcomes_keep_status_code_and_retry_as_they_came() {
    let prepared = prepared(guide());
    for action in [Action::FirstAgent, Action::CancelFirstAgent] {
        for (status, retry) in [
            ("action_required", "same_request"),
            ("failed", "new_request"),
            ("failed", "none"),
            ("paused", "later"),
        ] {
            let original = event(
                &prepared,
                action,
                &answer(action, status, "action_needed", retry),
            );
            assert!(!original.cancelled);
            assert!(original.ready_agent.is_none());
            assert!(original.launch_id.is_none());
            let saved = prepared.outcome_checkpoint(&original).unwrap();
            assert_eq!(prepared.restore_outcome(action, &saved).unwrap(), original);
        }
    }
}

#[test]
fn only_setup_s_message_is_retained_as_one_bounded_line() {
    let prepared = prepared(guide());
    let mut value = ready();
    value["message"] = format!(
        "  the agent\u{1b}[31m\nruns\u{202e} now {}",
        "x".repeat(400)
    )
    .into();
    value["data"]["evidence"] = "provider-private-detail".into();
    value["completed_steps"] = json!(["provider-private-detail"]);
    let original = event(&prepared, Action::FirstAgent, &value);
    let kept = original.message.as_deref().unwrap();
    assert!(kept.starts_with("the agent [31m runs now xxx"));
    assert_eq!(kept.chars().count(), 300);
    assert!(!kept.chars().any(|c| c.is_control() || c == '\u{202e}'));
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
    let prepared = prepared(guide());
    let original = event(&prepared, Action::FirstAgent, &ready());
    let mut invalid = Vec::new();
    let mut changed = original.clone();
    changed.step = Some("launch".into());
    invalid.push(changed);
    let mut changed = original.clone();
    changed.cancelled = true;
    invalid.push(changed);
    let mut changed = original.clone();
    changed.ready_agent = Some("other".into());
    invalid.push(changed);
    let mut changed = original.clone();
    changed.profile_id = "0a1b2c3d4e60".into();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.request_id = "other".into();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.code = Some("agent_waiting".into());
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
    let prepared = prepared(guide());
    let original = event(&prepared, Action::FirstAgent, &ready());
    let bytes = prepared.outcome_checkpoint(&original).unwrap();
    let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
    envelope["extra"] = true.into();
    assert!(
        prepared
            .restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap())
            .is_err()
    );
    for field in ["request_id", "action"] {
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
    // A format this app didn't write fails closed.
    let mut envelope: Checkpoint = serde_json::from_slice(&bytes).unwrap();
    envelope.format = 2;
    assert_eq!(
        prepared.restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap()),
        Err(Error::Invalid)
    );
    // A result of another schema is not one this app reads.
    let mut envelope: Checkpoint = serde_json::from_slice(&bytes).unwrap();
    let mut result: Value = serde_json::from_str(&envelope.result).unwrap();
    result["schema_version"] = 4.into();
    envelope.result = serde_json::to_string(&result).unwrap();
    assert_eq!(
        prepared.restore_outcome(Action::FirstAgent, &serde_json::to_vec(&envelope).unwrap()),
        Err(Error::Unsupported)
    );
}
