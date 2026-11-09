use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}
fn prepared() -> Prepared {
    let profile =
        Profile::parse(&serde_json::to_vec(&fixture()["discovery"]["show_ready"]).unwrap())
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
fn line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn canonical_setup_and_cancellation_retain_all_original_choices() {
    let prepared = prepared();
    let fixture = fixture();
    let setup = &fixture["setup"];
    for (action, key) in [
        (Action::FirstAgent, "first_agent"),
        (Action::CancelFirstAgent, "cancel_first_agent"),
    ] {
        assert_eq!(
            serde_json::from_slice::<Value>(prepared.bytes(action)).unwrap(),
            setup[key]
        );
    }
    let mut stream = prepared.stream(Action::FirstAgent);
    let progress = line(&setup["progress"]);
    assert!(stream.push(&progress[..5]).unwrap().is_empty());
    assert_eq!(
        stream.push(&progress[5..]).unwrap()[0].step.as_deref(),
        Some("hooks")
    );
    assert_eq!(
        stream.push(&line(&setup["result"])).unwrap()[0]
            .ready_agent
            .as_deref(),
        Some("worker")
    );
    let final_event = stream.finish().unwrap();
    assert_eq!(final_event.status, "completed");
    assert_eq!(&final_event.identity, prepared.identity());
    assert_eq!(final_event.request_id, prepared.request_id());
    let mut cancel = prepared.stream(Action::CancelFirstAgent);
    assert!(cancel.push(&line(&setup["cancellation_result"])).unwrap()[0].cancelled);
    assert!(cancel.finish().unwrap().cancelled);
}

#[test]
fn binding_refusal_and_unsafe_cancellation_never_grant_readiness_or_new_request() {
    let prepared = prepared();
    let setup = &fixture()["setup"];
    for (action, key) in [
        (Action::FirstAgent, "failed_before_resolution"),
        (Action::FirstAgent, "cancelled_original_result"),
        (Action::CancelFirstAgent, "cancellation_unsafe"),
    ] {
        let mut stream = prepared.stream(action);
        stream.push(&line(&setup[key])).unwrap();
        let result = stream.finish().unwrap();
        assert!(!result.cancelled);
        assert!(result.ready_agent.is_none());
    }
    let mut wrong = setup["cancellation_result"].clone();
    wrong["data"]["cancelled_request_id"] = json!("other");
    let mut stream = prepared.stream(Action::CancelFirstAgent);
    assert!(stream.push(&line(&wrong)).is_err());
    assert!(stream.push(&line(&setup["cancellation_result"])).is_err());
    assert!(stream.finish().is_err());
}

#[test]
fn unknown_or_contradictory_completions_poison_the_original_request() {
    let prepared = prepared();
    let result = &fixture()["setup"]["result"];
    let mut unknown = result.clone();
    unknown["code"] = json!("unexpected_success");
    unknown["data"] = json!({});
    let mut retry = result.clone();
    retry["retry"] = json!("new_request");
    let mut wrong_agent = result.clone();
    wrong_agent["data"]["agent_name"] = json!("another");
    for invalid in [unknown, retry, wrong_agent] {
        let mut stream = prepared.stream(Action::FirstAgent);
        assert!(stream.push(&line(&invalid)).is_err());
        assert!(stream.push(&line(result)).is_err());
        assert!(stream.finish().is_err());
    }
}

#[test]
fn every_event_requires_original_tuple_action_request_and_monotonic_sequence() {
    let prepared = prepared();
    let setup = &fixture()["setup"];
    for field in [
        "profile_id",
        "installation_id",
        "lifecycle_id",
        "request_id",
        "action",
    ] {
        let mut changed = setup["progress"].clone();
        changed[field] = json!("other");
        let mut stream = prepared.stream(Action::FirstAgent);
        assert!(stream.push(&line(&changed)).is_err(), "{field}");
        assert!(stream.push(&line(&setup["result"])).is_err());
        assert!(stream.finish().is_err());
    }
    let mut stream = prepared.stream(Action::FirstAgent);
    stream.push(&line(&setup["progress"])).unwrap();
    assert!(stream.push(&line(&setup["progress"])).is_err());
    assert!(stream.finish().is_err());
    let mut stream = prepared.stream(Action::FirstAgent);
    stream.push(&line(&setup["result"])).unwrap();
    assert!(stream.push(&line(&setup["progress"])).is_err());
    assert!(stream.finish().is_err());
    let mut foreign = setup["result"].clone();
    foreign["data"]["agent_name"] = json!("different");
    let mut stream = prepared.stream(Action::FirstAgent);
    assert!(stream.push(&line(&foreign)).is_err());
    assert!(prepared.stream(Action::FirstAgent).finish().is_err());
    let mut oversized = prepared.stream(Action::FirstAgent);
    assert!(oversized.push(&vec![b' '; LINE_LIMIT + 1]).is_err());
    assert!(oversized.finish().is_err());
}
