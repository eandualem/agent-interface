use super::*;

/// The service's own fixtures, copied byte for byte.
fn service(name: &str) -> Value {
    serde_json::from_str(match name {
        "first_agent" => include_str!("fixtures/setup-first-agent.json"),
        "cancel" => include_str!("fixtures/setup-cancel-first-agent.json"),
        "step" => include_str!("fixtures/setup-event-step.json"),
        "result" => include_str!("fixtures/setup-event-result.json"),
        _ => unreachable!("{name}"),
    })
    .unwrap()
}
/// A first agent's result for the service's first-agent fixture, built here
/// from the setup guide's `data` table: the service has no fixture of one.
pub(super) fn ready() -> Value {
    serde_json::from_str(include_str!("fixtures/first-agent-result.json")).unwrap()
}
/// The request ID of the service's first-agent fixture.
pub(super) const ID: &str = "019a8f3f1a2b7c3d8e4f5a6b7c8d9e0f";
const PROFILE: &str = "0a1b2c3d4e5f";
/// The profile the service's fixtures name, as the Rust service lists it.
fn served() -> Profile {
    let entry = json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": PROFILE, "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": "/run/profile/api.sock", "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    });
    Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap()
}
/// The choices the service's first-agent fixture holds.
pub(super) fn guide() -> Agent {
    Agent {
        name: "guide".into(),
        directory: "/home/user/projects/site".into(),
        create_directory: true,
        runtime: "codex".into(),
        model: None,
        brief: Brief::Template {
            template: TEMPLATE.into(),
        },
    }
}
pub(super) fn prepared(agent: Agent) -> Prepared {
    Prepared::first_agent(ID, &served(), agent, true).unwrap()
}
/// A result for this request, with no data.
pub(super) fn answer(action: Action, status: &str, code: &str, retry: &str) -> Value {
    json!({"schema_version": 3, "request_id": ID, "seq": 1, "type": "result",
        "action": action.name(), "status": status, "code": code, "message": "",
        "data": {}, "completed_steps": [], "retry": retry})
}
/// The setup guide's one answer that confirms a cancellation.
pub(super) fn cancelled() -> Value {
    let mut value = answer(
        Action::CancelFirstAgent,
        "completed",
        "request_cancelled",
        "new_request",
    );
    value["data"] = json!({"cancelled_request_id": ID, "can_start_new_request": true});
    value
}
fn step(seq: u64, step: &str, status: &str) -> Value {
    json!({"schema_version": 3, "request_id": ID, "seq": seq, "type": "step",
        "step": step, "status": status})
}
fn line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}
/// Decode `value` alone in a fresh stream for `action`.
fn decode(action: Action, value: &Value) -> Result<Event, Error> {
    let mut stream = prepared(guide()).stream(action);
    stream.push(&line(value))?;
    stream.finish()
}

#[test]
fn the_body_is_the_services_fixture_and_its_cancellation_changes_only_the_action() {
    let prepared = prepared(guide());
    for (action, fixture) in [
        (Action::FirstAgent, "first_agent"),
        (Action::CancelFirstAgent, "cancel"),
    ] {
        let body: Value = serde_json::from_slice(prepared.bytes(action)).unwrap();
        assert_eq!(body, service(fixture));
    }
    assert_eq!(prepared.profile_id(), PROFILE);
    // A profile the Rust service doesn't serve takes no setup request.
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/profiles-v1.json"
    )))
    .unwrap();
    let earlier =
        Profile::parse(&serde_json::to_vec(&fixture["discovery"]["show_ready"]).unwrap()).unwrap();
    assert_eq!(
        Prepared::first_agent(ID, &earlier, guide(), true).err(),
        Some(Error::Unsupported)
    );
}

#[test]
fn every_choice_is_checked_as_the_contract_words_it() {
    let takes = |id: &str, profile: &str, agent: Agent| {
        Prepared::prepare(id, profile, agent, false).is_ok()
    };
    for (id, taken) in [
        (ID, true),
        // Version 4, variant c, upper case, hyphens, one digit short.
        ("019a8f3f1a2b4c3d8e4f5a6b7c8d9e0f", false),
        ("019a8f3f1a2b7c3dce4f5a6b7c8d9e0f", false),
        ("019A8F3F1A2B7C3D8E4F5A6B7C8D9E0F", false),
        ("019a8f3f-1a2b-7c3d-8e4f-5a6b7c8d9e0f", false),
        ("019a8f3f1a2b7c3d8e4f5a6b7c8d9e0", false),
    ] {
        assert_eq!(takes(id, PROFILE, guide()), taken, "{id}");
    }
    for (profile, taken) in [
        (PROFILE, true),
        ("0A1B2C3D4E5F", false),
        ("0a1b2c3d4e5", false),
        ("profile-a", false),
    ] {
        assert_eq!(takes(ID, profile, guide()), taken, "{profile}");
    }
    let name = |name: String| Agent { name, ..guide() };
    let folder = |folder: String| Agent {
        directory: folder.into(),
        ..guide()
    };
    let runtime = |runtime: String| Agent { runtime, ..guide() };
    let model = |model: String| Agent {
        model: Some(model),
        ..guide()
    };
    let text = |text: String| Agent {
        brief: Brief::Text { text },
        ..guide()
    };
    let other_template = Agent {
        brief: Brief::Template {
            template: "basic".into(),
        },
        ..guide()
    };
    let long = |text: &str, count| text.repeat(count);
    for (case, agent, taken) in [
        ("name of 128", name(long("a", 128)), true),
        ("name of 129", name(long("a", 129)), false),
        ("name with - first", name("-guide".into()), false),
        ("name of each kind", name("G1.v_2-x".into()), true),
        // Characters, not bytes.
        (
            "folder of 4096",
            folder(format!("/{}", long("é", 4095))),
            true,
        ),
        (
            "folder of 4097",
            folder(format!("/{}", long("é", 4096))),
            false,
        ),
        ("folder climbing", folder("/w/../etc".into()), false),
        ("folder named ..x", folder("/w/..x".into()), true),
        ("relative folder", folder("w".into()), false),
        ("folder with U+0085", folder("/w\u{85}".into()), false),
        ("runtime of 40", runtime(long("a", 40)), true),
        ("runtime of 41", runtime(long("a", 41)), false),
        ("runtime in capitals", runtime("Codex".into()), false),
        ("runtime with _", runtime("co_dex".into()), false),
        // Written as a runtime: the service refuses it with runtime_missing.
        (
            "runtime setup doesn't start",
            runtime("open-code2".into()),
            true,
        ),
        ("shell", runtime("shell".into()), false),
        ("model of each kind", model("gpt-5.1/x:y_z".into()), true),
        ("model of 500", model(long("a", 500)), true),
        ("model of 501", model(long("a", 501)), false),
        ("model with - first", model("-x".into()), false),
        ("model with a space", model("a b".into()), false),
        ("another template", other_template, false),
        ("brief of 16000", text(long("é", 16_000)), true),
        ("brief of 16001", text(long("x", 16_001)), false),
        // White space as the contract counts it: both sets.
        ("blank brief", text(" \u{3000}\t".into()), false),
        ("brief of U+0085", text("\u{85}".into()), false),
        ("brief of U+FEFF", text("\u{feff}".into()), false),
        ("brief with U+0085", text("a\u{85}".into()), true),
    ] {
        assert_eq!(takes(ID, PROFILE, agent), taken, "{case}");
    }
}

#[test]
fn a_request_may_have_32768_bytes_and_its_cancellation_32775() {
    let with = |text: String| {
        Prepared::prepare(
            ID,
            PROFILE,
            Agent {
                brief: Brief::Text { text },
                ..guide()
            },
            true,
        )
    };
    let overhead = with("x".into()).unwrap().bytes(Action::FirstAgent).len() - 1;
    // Four bytes to a character, so the brief's 16,000 characters reach it.
    let room = REQUEST_LIMIT - overhead;
    let text = "\u{1f600}".repeat(room / 4) + &"x".repeat(room % 4);
    let most = with(text.clone()).unwrap();
    assert_eq!(most.bytes(Action::FirstAgent).len(), 32_768);
    assert_eq!(most.bytes(Action::CancelFirstAgent).len(), 32_775);
    assert!(with(text + "x").is_err());
}

#[test]
fn steps_carry_no_action_and_a_ready_result_names_this_agent() {
    // The service's step fixture is an initialize request's: any stream under
    // its ID reads it.
    let fixture = service("step");
    let mut stream = Prepared::first_agent(
        fixture["request_id"].as_str().unwrap(),
        &served(),
        guide(),
        true,
    )
    .unwrap()
    .stream(Action::FirstAgent);
    let read = stream.push(&line(&fixture)).unwrap();
    assert_eq!(read[0].step.as_deref(), Some("preflight"));
    assert_eq!(read[0].status, "running");
    // Nor does its initialize result end a first agent's run.
    let result = service("result");
    let mut stream = Prepared::first_agent(
        result["request_id"].as_str().unwrap(),
        &served(),
        guide(),
        true,
    )
    .unwrap()
    .stream(Action::FirstAgent);
    assert!(stream.push(&line(&result)).is_err());

    // As the command prints a first agent: its steps once the profile
    // answered, then the result, across reads.
    let prepared = prepared(guide());
    let mut stream = prepared.stream(Action::FirstAgent);
    for (seq, name) in (1..).zip(["preflight", "agent", "launch", "ready", "greeting"]) {
        let read = stream.push(&line(&step(seq, name, "completed"))).unwrap();
        assert_eq!(read[0].step.as_deref(), Some(name));
        assert!(read[0].ready_agent.is_none());
    }
    let result = line(&ready());
    assert!(stream.push(&result[..5]).unwrap().is_empty());
    assert_eq!(
        stream.push(&result[5..]).unwrap()[0].ready_agent.as_deref(),
        Some("guide")
    );
    let ended = stream.finish().unwrap();
    assert_eq!(ended.code.as_deref(), Some("agent_ready"));
    assert_eq!(ended.retry.as_deref(), Some("none"));
    assert_eq!(
        ended.launch_id.as_deref(),
        ready()["data"]["launch_id"].as_str()
    );
    assert_eq!(ended.profile_id, PROFILE);
    assert_eq!(ended.request_id, ID);
}

#[test]
fn ready_needs_this_agent_and_a_running_cli_and_a_launch_may_be_unnamed() {
    let mut unnamed = ready();
    unnamed["data"]["launch_id"] = Value::Null;
    let event = decode(Action::FirstAgent, &unnamed).unwrap();
    assert_eq!(event.ready_agent.as_deref(), Some("guide"));
    assert!(event.launch_id.is_none());
    for (field, value) in [
        ("agent_name", json!("another")),
        ("ready", json!("exited")),
        ("launch_id", json!(7)),
        ("launch_id", json!("a b")),
    ] {
        let mut contradicted = ready();
        contradicted["data"][field] = value;
        assert!(
            decode(Action::FirstAgent, &contradicted).is_err(),
            "{field}"
        );
    }
    // A run that ended otherwise names no ready agent.
    let failed = answer(Action::FirstAgent, "failed", "launch_failed", "none");
    assert!(
        decode(Action::FirstAgent, &failed)
            .unwrap()
            .ready_agent
            .is_none()
    );
}

#[test]
fn only_the_guides_answer_confirms_a_cancellation() {
    assert!(
        decode(Action::CancelFirstAgent, &cancelled())
            .unwrap()
            .cancelled
    );
    for value in [
        answer(
            Action::CancelFirstAgent,
            "action_required",
            "outcome_unknown",
            "same_request",
        ),
        answer(
            Action::CancelFirstAgent,
            "action_required",
            "cancellation_unsafe",
            "none",
        ),
        answer(
            Action::CancelFirstAgent,
            "failed",
            "request_expired",
            "none",
        ),
        // The original sent again after its cancellation.
        answer(
            Action::FirstAgent,
            "failed",
            "request_cancelled",
            "new_request",
        ),
    ] {
        let action = if value["action"] == "first_agent" {
            Action::FirstAgent
        } else {
            Action::CancelFirstAgent
        };
        let event = decode(action, &value).unwrap();
        assert!(!event.cancelled && event.ready_agent.is_none(), "{value}");
    }
    for (field, value) in [
        (
            "cancelled_request_id",
            json!("019a8f3f1a2b7c3d8e4f5a6b7c8d9e10"),
        ),
        ("can_start_new_request", json!(false)),
    ] {
        let mut contradicted = cancelled();
        contradicted["data"][field] = value;
        assert!(
            decode(Action::CancelFirstAgent, &contradicted).is_err(),
            "{field}"
        );
    }
}

#[test]
fn values_this_app_does_not_know_are_kept_and_malformed_ones_poison() {
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert_eq!(
        stream.push(&line(&step(1, "preflight", "paused"))).unwrap()[0].status,
        "paused"
    );
    // As the service's own protocol test has it.
    let mut unknown = answer(Action::FirstAgent, "paused", "new_code", "later");
    unknown["seq"] = json!(2);
    unknown["hint"] = json!(1);
    stream.push(&line(&unknown)).unwrap();
    let kept = stream.finish().unwrap();
    assert_eq!(
        (
            kept.status.as_str(),
            kept.code.as_deref(),
            kept.retry.as_deref()
        ),
        ("paused", Some("new_code"), Some("later"))
    );
    assert!(!kept.cancelled && kept.ready_agent.is_none());
    // Completed with a code this app doesn't know is not ready.
    let other = answer(Action::FirstAgent, "completed", "agent_waiting", "none");
    assert!(
        decode(Action::FirstAgent, &other)
            .unwrap()
            .ready_agent
            .is_none()
    );

    for (field, value) in [
        ("status", json!(3)),
        ("status", json!("a b")),
        ("code", json!("c".repeat(129))),
        ("retry", Value::Null),
        ("data", json!([])),
        ("completed_steps", Value::Null),
        ("type", json!("progress")),
        ("action", json!("initialize")),
        ("action", Value::Null),
        ("message", Value::Null),
        ("message", json!(3)),
    ] {
        let mut malformed = answer(Action::FirstAgent, "failed", "launch_failed", "none");
        malformed[field] = value.clone();
        assert!(
            decode(Action::FirstAgent, &malformed).is_err(),
            "{field}: {value}"
        );
    }
    for (field, value) in [
        ("step", Value::Null),
        ("step", json!(3)),
        ("status", Value::Null),
        ("status", json!(true)),
    ] {
        let mut malformed = step(1, "preflight", "running");
        malformed[field] = value.clone();
        assert!(
            decode(Action::FirstAgent, &malformed).is_err(),
            "step {field}: {value}"
        );
    }
    let mut earlier = ready();
    earlier["schema_version"] = json!(2);
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert_eq!(stream.push(&line(&earlier)), Err(Error::Unsupported));
    assert!(stream.finish().is_err());
}

#[test]
fn every_event_needs_this_request_a_rising_sequence_and_one_result_last() {
    let poisoned = |lines: &[Value]| {
        let mut stream = prepared(guide()).stream(Action::FirstAgent);
        let mut refused = false;
        for value in lines {
            refused |= stream.push(&line(value)).is_err();
        }
        // Once refused, nothing more is read, not even a valid result.
        refused && stream.push(&line(&ready())).is_err() && stream.finish().is_err()
    };
    let mut foreign = step(1, "preflight", "running");
    foreign["request_id"] = json!("019a8f3f1a2b7c3d8e4f5a6b7c8d9e10");
    assert!(poisoned(&[foreign]));
    let first = step(1, "preflight", "completed");
    assert!(poisoned(&[first.clone(), first.clone()]));
    assert!(poisoned(&[step(2, "agent", "completed"), first]));
    assert!(poisoned(&[step(1025, "agent", "completed")]));
    let mut cancel = ready();
    cancel["action"] = json!("cancel_first_agent");
    assert!(poisoned(&[cancel]));
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    stream.push(&line(&ready())).unwrap();
    assert!(stream.push(&line(&step(7, "agent", "completed"))).is_err());
    assert!(stream.finish().is_err());
    // No result is no outcome, and a line may not outgrow its bound.
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    stream
        .push(&line(&step(1, "preflight", "completed")))
        .unwrap();
    assert!(stream.finish().is_err());
    let mut oversized = prepared(guide()).stream(Action::FirstAgent);
    assert!(oversized.push(&vec![b' '; LINE_LIMIT + 1]).is_err());
    assert!(oversized.finish().is_err());
    // The last line needs no newline.
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    stream.push(&serde_json::to_vec(&ready()).unwrap()).unwrap();
    assert!(stream.finish().unwrap().ready_agent.is_some());
}

#[test]
fn an_event_ignores_what_only_the_other_kind_defines() {
    // A step with a result's fields, of any type, and a result with a step's.
    let mut early = step(1, "preflight", "completed");
    early["retry"] = json!({"not": "a token"});
    early["data"] = json!(3);
    early["action"] = json!(7);
    let mut result = ready();
    result["step"] = json!({"name": 1});
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    stream.push(&line(&early)).unwrap();
    stream.push(&line(&result)).unwrap();
    assert!(stream.finish().unwrap().ready_agent.is_some());
}

#[test]
fn a_field_an_event_defines_said_twice_poisons() {
    // Another request's ID first and this one's last: the last does not win.
    let twice = serde_json::to_string(&ready()).unwrap().replacen(
        '{',
        "{\"request_id\":\"019a8f3f1a2b7c3d8e4f5a6b7c8d9e10\",",
        1,
    );
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
    let twice = serde_json::to_string(&step(1, "preflight", "running"))
        .unwrap()
        .replacen('{', "{\"status\":\"completed\",", 1);
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
    let refused = answer(
        Action::FirstAgent,
        "failed",
        "agent_conflict",
        "new_request",
    );
    let twice =
        serde_json::to_string(&refused)
            .unwrap()
            .replacen('{', "{\"code\":\"agent_ready\",", 1);
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
}

#[test]
fn a_field_of_the_data_the_app_reads_said_twice_poisons() {
    // Another request's ID first and this one's last: the last does not win.
    let twice = serde_json::to_string(&cancelled()).unwrap().replacen(
        "\"data\":{",
        "\"data\":{\"cancelled_request_id\":\"019a8f3f1a2b7c3d8e4f5a6b7c8d9e10\",",
        1,
    );
    let mut stream = prepared(guide()).stream(Action::CancelFirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
    let twice = serde_json::to_string(&ready()).unwrap().replacen(
        "\"data\":{",
        "\"data\":{\"agent_name\":\"another\",",
        1,
    );
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
    // Data the app doesn't read, or none, as a result made without reaching
    // the profile has, still reads.
    let mut other = ready();
    other["data"]["evidence_more"] = json!({"any": [1, 2]});
    assert!(
        decode(Action::FirstAgent, &other)
            .unwrap()
            .ready_agent
            .is_some()
    );
    let empty = answer(
        Action::FirstAgent,
        "action_required",
        "service_unavailable",
        "same_request",
    );
    assert!(decode(Action::FirstAgent, &empty).is_ok());
}

#[test]
fn a_key_said_twice_at_any_depth_is_refused_read_or_not() {
    for twice in [
        r#"{"a":1,"a":1}"#,
        r#"{"data":{"more":{"x":1,"x":2}}}"#,
        r#"{"data":[{"x":[{"y":1,"y":1}]}]}"#,
        r#"{"a":1,"a":1}"#,
    ] {
        assert!(!super::unique_keys(twice.as_bytes()), "{twice}");
    }
    for once in [
        r#"{"a":{"a":1},"b":[{"a":1},{"a":2}]}"#,
        r#"[1,"x",null,1.5]"#,
    ] {
        assert!(super::unique_keys(once.as_bytes()), "{once}");
    }
    assert!(!super::unique_keys(br#"{"a":1} {"#));
    // In data the app doesn't read, too.
    let mut deep = ready();
    deep["data"]["more"] = json!({"x": 1});
    let twice =
        serde_json::to_string(&deep)
            .unwrap()
            .replacen("\"more\":{", "\"more\":{\"x\":2,", 1);
    let mut stream = prepared(guide()).stream(Action::FirstAgent);
    assert!(stream.push(format!("{twice}\n").as_bytes()).is_err());
}

#[test]
fn data_is_all_there_or_none_and_a_string_is_never_null() {
    // Null only where the guide says it may be.
    for field in ["launch_id", "greeting_delivery_id"] {
        let mut nullable = ready();
        nullable["data"][field] = Value::Null;
        assert!(decode(Action::FirstAgent, &nullable).is_ok(), "{field}");
    }
    for field in ["agent_name", "ready", "evidence"] {
        let mut null = ready();
        null["data"][field] = Value::Null;
        assert!(decode(Action::FirstAgent, &null).is_err(), "{field}");
        // Left out of data that has the rest, on any result.
        for mut partial in [
            ready(),
            answer(
                Action::FirstAgent,
                "failed",
                "agent_conflict",
                "new_request",
            ),
        ] {
            partial["data"] = ready()["data"].clone();
            partial["data"].as_object_mut().unwrap().remove(field);
            assert!(decode(Action::FirstAgent, &partial).is_err(), "{field}");
        }
    }
    // A cancellation's data: both or neither.
    let mut null = cancelled();
    null["data"]["cancelled_request_id"] = Value::Null;
    assert!(decode(Action::CancelFirstAgent, &null).is_err());
    let mut partial = answer(
        Action::CancelFirstAgent,
        "action_required",
        "cancellation_unsafe",
        "none",
    );
    partial["data"] = json!({"can_start_new_request": true});
    assert!(decode(Action::CancelFirstAgent, &partial).is_err());
}
