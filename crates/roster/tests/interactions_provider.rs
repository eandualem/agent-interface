//! Opt-in acceptance against an inert private provider fixture. The receipt is
//! required; this test never selects the installed provider implicitly.
use agent_interface_roster::{conversations as chat, interactions};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "requires an explicitly configured inert private interaction fixture"]
fn original_and_discussion_are_separate_and_an_exact_retry_is_idempotent() {
    let directory = PathBuf::from(
        std::env::var_os("INTERACTIONS_TEST_DIRECTORY").expect("explicit fixture directory"),
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["fixture"], true);
    for field in ["real_agents", "model_calls", "service_manager"] {
        assert_eq!(receipt[field], false);
    }
    let socket = receipt["socket"].as_str().unwrap();
    assert!(std::path::Path::new(socket).starts_with(directory.canonicalize().unwrap()));
    assert_eq!(std::env::var("BACKBONE_SOCKET_PATH").unwrap(), socket);
    let provider = chat::provider(&directory).unwrap();
    assert_eq!(provider, format!("unix:{socket}"));
    let id = receipt["interaction"].as_str().unwrap();
    let original = interactions::detail(&directory, &provider, id).unwrap();
    let recipient = original
        .conversation
        .recipients
        .iter()
        .find(|actor| !actor.retired && actor.id.is_some())
        .unwrap()
        .id
        .clone();
    let request = chat::Message {
        request_id: format!(
            "native-fixture-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        text: "Illustrative consumer fixture question: explain this retained message.".into(),
        recipient,
        anchor: Some(chat::Anchor {
            label: Some("Original fixture message".into()),
            ..Default::default()
        }),
    };
    let first = chat::send(&directory, &provider, chat::Kind::Interaction, id, &request);
    let chat::Outcome::Accepted(entry) = first else {
        panic!("fixture question was not accepted: {first:?}")
    };
    let retry = chat::send(&directory, &provider, chat::Kind::Interaction, id, &request);
    let chat::Outcome::Accepted(repeated) = retry else {
        panic!("exact retry was not accepted: {retry:?}")
    };
    assert_eq!(entry.id, repeated.id);
    assert_eq!(entry.body, repeated.body);
    let after = interactions::detail(&directory, &provider, id).unwrap();
    assert_eq!(after.body, original.body);
    let thread = chat::show(
        &directory,
        &provider,
        chat::Kind::Interaction,
        id,
        &chat::Page::Newest,
    )
    .unwrap();
    assert_eq!(
        thread
            .entries
            .iter()
            .filter(|item| item.id == entry.id)
            .count(),
        1
    );
    assert!(
        thread
            .entries
            .iter()
            .any(|item| matches!(&item.body,chat::Body::Message{text,..} if text==&request.text))
    );
}

#[test]
#[ignore = "requires an explicitly configured inert private interaction fixture"]
fn sampled_states_and_source_entries_remain_distinct_from_message_coverage() {
    let directory = PathBuf::from(
        std::env::var_os("INTERACTIONS_TEST_DIRECTORY").expect("explicit fixture directory"),
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["fixture"], true);
    for field in ["real_agents", "model_calls", "service_manager"] {
        assert_eq!(receipt[field], false);
    }
    assert_eq!(
        std::env::var("BACKBONE_SOCKET_PATH").unwrap(),
        receipt["socket"].as_str().unwrap()
    );
    let since = receipt["illustrative_window"]["since"].as_str().unwrap();
    let until = receipt["illustrative_window"]["until"].as_str().unwrap();
    let all = interactions::read(&directory, since, until, None).unwrap();
    assert!(all.state_error.is_none());
    let states = all.states.unwrap();
    assert_eq!(states.coverage.completeness, "sampled_partial");
    assert!(states.coverage.unknown_outside_returned_spans);
    assert!(states.coverage.legacy_name_only_excluded);
    assert!(states.next_cursor.is_none());
    assert!(
        states
            .items
            .iter()
            .any(|span| span.basis == "matching_samples")
    );
    assert!(
        states
            .items
            .iter()
            .any(|span| span.basis == "point_observation")
    );
    for id in receipt["agents"].as_object().unwrap().values() {
        assert!(
            states
                .items
                .iter()
                .any(|span| span.agent_id == id.as_str().unwrap())
        );
    }
    let agent = receipt["agents"]["coordinator"].as_str().unwrap();
    let selected = interactions::read(&directory, since, until, Some(agent)).unwrap();
    let history = selected.history.unwrap();
    assert!(history.next_cursor.is_none());
    assert!(!history.coverage.truncated);
    assert!(history.coverage.message_intervals.iter().any(|interval| {
        interval.since == since
            && interval.until == until
            && interval.completeness == "complete_within_declared_scope"
    }));
    let expected: Vec<_> = receipt["discussion_entry_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect();
    let mut entries: Vec<_> = history
        .items
        .iter()
        .filter_map(|item| item.conversation_entry.as_ref())
        .filter(|entry| expected.contains(&entry.entry_id))
        .collect();
    entries.sort_by_key(|entry| entry.entry_id);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.entry_id)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        entries[0].anchored_interaction_id,
        entries[2].anchored_interaction_id
    );
    assert_ne!(
        entries[0].anchored_interaction_id,
        entries[1].anchored_interaction_id
    );
    assert!(
        selected
            .states
            .unwrap()
            .items
            .iter()
            .all(|span| span.agent_id == agent)
    );
}
