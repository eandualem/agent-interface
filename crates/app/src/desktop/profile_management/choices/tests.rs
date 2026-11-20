use super::*;
use agent_interface_roster::profiles::setup::Action;
use serde_json::{Value, json};
use std::path::Path;
/// A profile the Rust service serves: the only kind setup schema 3 takes.
fn profile() -> Profile {
    let entry = json!({
        "state": "serving", "protocol_version": 1, "backbone_version": "0.1.0",
        "profile_id": "0a1b2c3d4e5f", "display_name": "Work",
        "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
        "machine": {"id": "5f2c9e0d1b7a4e38", "name": "laptop"}, "home": true,
        "socket": "/run/profile/api.sock", "tmux_socket": "/run/profile/tmux.sock",
        "credential_reference": {"type": "socket"}
    });
    Profile::parse(&serde_json::to_vec(&entry).unwrap()).unwrap()
}
/// What `backbone runtimes --json` lists for it.
fn runtimes() -> Value {
    json!({"schema_version": 1, "items": [{"id": "codex", "display_name": "Codex",
        "available": true, "setup_supported": true, "models": [], "efforts": [],
        "authentication": "not_authenticated"}]})
}
fn catalog(wire: &Value) -> Catalog {
    Catalog::served(
        &serde_json::to_vec(wire).unwrap(),
        &profile().expected_identity().unwrap(),
    )
    .unwrap()
}
/// A request ID the service takes: a UUIDv7.
const REQUEST: &str = "019a8f3f1a2b7c3d8e4f5a6b7c8d9e0f";
fn choice() -> Selection<'static> {
    Selection {
        name: "worker",
        project: Path::new("/example/project"),
        create: false,
        runtime: "codex",
        model: None,
        brief: Some("Do the project work"),
        greeting: false,
    }
}
#[test]
fn explicit_choices_freeze_the_profile_and_identical_cancel_body() {
    let mut wire = runtimes();
    wire["items"][0]["models"] = json!(["model-a"]);
    let catalog = catalog(&wire);
    let mut selected = choice();
    selected.model = Some("model-a");
    selected.greeting = true;
    let prepared = freeze(REQUEST, &profile(), &catalog, selected).unwrap();
    let first: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    let mut cancel: Value =
        serde_json::from_slice(prepared.bytes(Action::CancelFirstAgent)).unwrap();
    assert_eq!(first["schema_version"], 3);
    assert_eq!(first["profile_id"], "0a1b2c3d4e5f");
    assert_eq!(first["agent"]["model"], "model-a");
    assert_eq!(first["agent"]["directory"], "/example/project");
    assert_eq!(first["agent"]["create_directory"], false);
    assert_eq!(
        first["agent"]["brief"],
        json!({"text":"Do the project work"})
    );
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
fn a_tool_that_takes_the_brief_as_its_first_turn_gets_no_second_greeting() {
    // The default brief asks the agent to greet; custom instructions may not.
    for (id, brief, greeting) in [
        ("antigravity", None, false),
        ("antigravity", Some("Do the project work"), true),
        ("codex", None, true),
        ("claude", None, true),
    ] {
        let mut wire = runtimes();
        wire["items"][0]["id"] = json!(id);
        let mut selected = choice();
        selected.runtime = id;
        selected.brief = brief;
        selected.greeting = true;
        let prepared = freeze(REQUEST, &profile(), &catalog(&wire), selected).unwrap();
        let first: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
        assert_eq!(first["start_greeting"], greeting, "{id}");
    }
}
#[test]
fn changed_origin_or_stale_runtime_model_cannot_prepare() {
    let wire = runtimes();
    let catalog = catalog(&wire);
    for field in ["profile_id", "installation_id", "lifecycle_id"] {
        let mut expected = profile().expected_identity().unwrap();
        match field {
            "profile_id" => expected.profile_id = "replacement".into(),
            "installation_id" => expected.installation_id = "replacement".into(),
            _ => expected.lifecycle_id = "replacement".into(),
        }
        let other = Catalog::served(&serde_json::to_vec(&wire).unwrap(), &expected).unwrap();
        assert!(freeze(REQUEST, &profile(), &other, choice()).is_none());
    }
    let mut selected = choice();
    selected.runtime = "missing";
    assert!(freeze(REQUEST, &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.model = Some("missing");
    assert!(freeze(REQUEST, &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.project = Path::new("relative");
    assert!(freeze(REQUEST, &profile(), &catalog, selected).is_none());
    let mut selected = choice();
    selected.brief = Some(" ");
    assert!(freeze(REQUEST, &profile(), &catalog, selected).is_none());
}
#[test]
fn a_guide_starts_only_in_the_tools_setup_starts() {
    // As `backbone runtimes --json` lists them: setup_supported says whether
    // hooks are installed, not whether setup starts the tool.
    let tool = |id: &str, setup_supported: bool| {
        json!({"id": id, "display_name": id, "available": true,
            "setup_supported": setup_supported, "models": [], "efforts": [],
            "authentication": "unknown"})
    };
    let catalog = catalog(&json!({"schema_version": 1, "items": [
        tool("claude", true), tool("codex", true), tool("opencode", true),
        tool("antigravity", false), tool("shell", false),
    ]}));
    let ready = |setup| {
        catalog
            .items()
            .iter()
            .filter(|r| eligible(r, setup))
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(ready(true), ["claude", "codex", "antigravity"]);
    // Add agent starts through the service's start, as before.
    assert_eq!(ready(false), ["claude", "codex", "opencode"]);
    for (runtime, made) in [("antigravity", true), ("opencode", false)] {
        let mut selected = choice();
        selected.runtime = runtime;
        assert_eq!(
            freeze(REQUEST, &profile(), &catalog, selected).is_some(),
            made,
            "{runtime}"
        );
    }
}

#[test]
fn unavailable_setup_is_not_selectable_and_no_optional_action_is_implied() {
    let mut wire = runtimes();
    let prepared = freeze(REQUEST, &profile(), &catalog(&wire), choice()).unwrap();
    let body: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    assert_eq!(body["start_greeting"], false);
    assert_eq!(body["agent"]["model"], Value::Null);
    wire["items"][0]["available"] = json!(false);
    assert!(freeze(REQUEST, &profile(), &catalog(&wire), choice()).is_none());
}

#[test]
fn an_open_request_leaves_new_choices_and_an_unreadable_one_precedes_the_catalog() {
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
    let prepared = freeze(REQUEST, &profile(), &catalog(&runtimes()), choice()).unwrap();
    controller.enqueue(prepared).unwrap();
    // The open request does not stop the catalog read for other agent names.
    assert!(
        load_choices(
            client.clone(),
            root.clone(),
            &profile().expected_identity().unwrap(),
            true,
            &|| true,
        )
        .is_err()
    );
    assert!(calls.exists());
    std::fs::remove_file(&calls).unwrap();
    // A malformed second record cannot be treated as an empty pending list.
    let bad = journal::Journal::open(&root.join("profile-setup")).unwrap();
    bad.save("ffffffffffffffffffffffffffffffff", None, b"not a capsule")
        .unwrap();
    let mut other = profile().expected_identity().unwrap();
    other.profile_id = "0a1b2c3d4e60".into();
    let state = load_choices(client.clone(), root.clone(), &other, true, &|| true).unwrap();
    let State::Unchecked(retained) = state else {
        panic!("an unreadable record must not expose a guide's choices")
    };
    assert_eq!(retained.ids().unwrap().len(), 2);
    assert!(!calls.exists());
    // Add agent saves nothing there, so it goes on to the profile's tools.
    assert!(load_choices(client, root, &other, false, &|| true).is_err());
    assert!(calls.exists());
}
#[test]
fn the_default_brief_is_the_fresh_install_template() {
    let mut selected = choice();
    selected.brief = None;
    let prepared = freeze(REQUEST, &profile(), &catalog(&runtimes()), selected).unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    assert_eq!(
        body["agent"]["brief"],
        json!({"template":"orchestrator-v1"})
    );
}

#[test]
fn an_agent_is_named_after_its_folder() {
    assert_eq!(
        super::folder_name(std::path::Path::new("/work/Acme API")).as_deref(),
        Some("acme-api")
    );
    assert_eq!(super::folder_name(std::path::Path::new("/")), None);
}

#[test]
fn a_typed_name_with_spaces_becomes_hyphens() {
    assert_eq!(super::agent_name(" new  agent "), "new-agent");
}
#[test]
fn a_guide_is_the_services_first_agent_request_run_by_backbone_setup() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/setup/fixtures/setup-first-agent.json"
    )))
    .unwrap();
    let ready: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/src/profiles/setup/fixtures/first-agent-result.json"
    )))
    .unwrap();
    // A guide in a folder setup makes, on the starter's brief, with a greeting.
    let mut guide = choice();
    guide.name = "guide";
    guide.project = Path::new("/home/user/projects/site");
    guide.create = true;
    guide.brief = None;
    guide.greeting = true;
    let prepared = freeze(REQUEST, &profile(), &catalog(&runtimes()), guide).unwrap();
    let body: Value = serde_json::from_slice(prepared.bytes(Action::FirstAgent)).unwrap();
    assert_eq!(body, fixture);
    // A fake backbone: it takes setup's arguments, keeps the request, and
    // prints the steps once the profile answered, then the result.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut events = String::new();
    for (seq, step) in (1..).zip(["preflight", "agent", "launch", "ready", "greeting"]) {
        let event = json!({"schema_version": 3, "request_id": REQUEST, "seq": seq,
            "type": "step", "step": step, "status": "completed"});
        events += &format!("{event}\n");
    }
    events += &format!("{ready}\n");
    fs::write(root.join("events"), events).unwrap();
    let executable = root.join("backbone");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\n[ \"$1 $2 $4\" = 'setup --file --jsonl' ] || exit 9\n/bin/cat \"$3\" > '{}'\n/bin/cat '{}'\n",
            root.join("request").display(),
            root.join("events").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    let controller = Controller::new(
        journal::Journal::open(&root.join("profile-setup")).unwrap(),
        &client,
    );
    let key = controller.enqueue(prepared).unwrap().key;
    let mut steps = Vec::new();
    let started = controller
        .submit(&key, &|| true, |progress| steps.push(progress.event.step))
        .unwrap();
    assert_eq!(started.ready_agent.as_deref(), Some("guide"));
    assert_eq!(steps.len(), 5);
    let sent: Value = serde_json::from_slice(&fs::read(root.join("request")).unwrap()).unwrap();
    assert_eq!(sent, fixture);
    assert!(controller.read(&key).unwrap().closed());
}

#[test]
fn a_served_profile_lists_the_rust_services_runtimes() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executable = root.join("provider");
    fs::write(
        &executable,
        "#!/bin/sh\n[ \"$*\" = \"runtimes --json\" ] || exit 2\nprintf '%s' '{\"schema_version\":1,\"items\":[{\"id\":\"claude\",\"display_name\":\"Claude Code\",\"available\":true,\"setup_supported\":true,\"models\":[],\"efforts\":[],\"authentication\":\"unknown\"}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    let identity = profile().expected_identity().unwrap();
    let State::Ready { catalog, .. } =
        load_choices(client.clone(), root.clone(), &identity, false, &|| true).unwrap()
    else {
        panic!("a served catalog must give choices")
    };
    assert!(eligible(&catalog.items()[0], false));
}

#[test]
fn choosing_the_chosen_tool_again_changes_nothing() {
    let (mut runtime, mut model) = (Some("claude".to_owned()), Some("opus".to_owned()));
    assert!(!choose_tool(&mut runtime, &mut model, "claude"));
    assert_eq!(
        (runtime.as_deref(), model.as_deref()),
        (Some("claude"), Some("opus"))
    );
    // Another tool starts on its default model.
    assert!(choose_tool(&mut runtime, &mut model, "codex"));
    assert_eq!((runtime.as_deref(), model), (Some("codex"), None));
    let mut none = None;
    assert!(choose_tool(&mut none, &mut None, "claude"));
    assert_eq!(none.as_deref(), Some("claude"));
}

#[test]
fn tab_reaches_every_field_shown_in_order() {
    // Without the repository field: name, back to name.
    assert_eq!(next_field(Some(Field::Name), false), Field::Name);
    assert_eq!(next_field(None, false), Field::Name);
    // With it: name, then the repository, back to name.
    assert_eq!(next_field(Some(Field::Name), true), Field::Repository);
    assert_eq!(next_field(Some(Field::Repository), true), Field::Name);
    // A focused field that is no longer shown goes back to the name.
    assert_eq!(next_field(Some(Field::Repository), false), Field::Name);
}

#[test]
fn add_agent_needs_no_setup_journal_and_a_guide_does() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executable = root.join("provider");
    fs::write(
        &executable,
        "#!/bin/sh\n[ \"$*\" = \"runtimes --json\" ] || exit 2\nprintf '%s' '{\"schema_version\":1,\"items\":[{\"id\":\"codex\",\"display_name\":\"Codex\",\"available\":true,\"setup_supported\":true,\"models\":[],\"efforts\":[],\"authentication\":\"unknown\"}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    // Where the journal would be, a file: it can't be opened.
    fs::write(root.join("profile-setup"), "").unwrap();
    let identity = profile().expected_identity().unwrap();
    let added = load_choices(client.clone(), root.clone(), &identity, false, &|| true);
    assert!(matches!(
        added,
        Ok(State::Ready {
            controller: None,
            pending: 0,
            ..
        })
    ));
    assert!(load_choices(client, root, &identity, true, &|| true).is_err());
}

#[test]
fn each_result_a_start_stops_on_reads_the_open_setups_again() {
    // Still starting: nothing to read yet.
    assert!(!read_again(None, None));
    // The first stop, then the same result again.
    assert!(read_again(None, Some(3)));
    assert!(!read_again(Some(3), Some(3)));
    // Try again sent it, and it ended on another result: a closed request
    // now frees Start.
    assert!(read_again(Some(3), Some(5)));
    // Stopped before anything was saved.
    assert!(read_again(None, Some(0)));
}

#[test]
fn the_start_button_after_a_stop_is_the_one_its_request_offers_or_none() {
    // As rendered: no button at all while the request is open with nothing
    // it may send.
    assert_eq!(start_label(false, Some(None), true, true, "guide"), None);
    for label in ["Try again", "Start a new request"] {
        assert_eq!(
            start_label(false, Some(Some(label)), true, true, "guide").as_deref(),
            Some(label)
        );
    }
    // Before any stop, and while starting, as before.
    assert_eq!(
        start_label(false, None, true, true, "guide").as_deref(),
        Some("Start guide")
    );
    assert_eq!(
        start_label(false, None, false, true, "guide").as_deref(),
        Some("Choose a tool")
    );
    assert_eq!(
        start_label(true, Some(None), true, true, "guide").as_deref(),
        Some("Starting guide…")
    );
}

#[test]
fn start_and_enter_pass_one_gate_that_an_open_request_closes() {
    // This repository has no gpui test harness: submit() returns at once
    // unless can_start(), which is may_start() over what the form holds.
    // An open saved request for the profile: nothing starts, and no request
    // is even made, so no ID is drawn.
    let never = || -> bool { panic!("a request was made") };
    assert!(!may_start(None, true, never));
    assert!(!may_start(Some(false), false, never));
    // Its own open request is sent again, whatever else is held.
    assert!(may_start(Some(true), true, never));
    // Nothing open: the choices decide.
    assert!(may_start(None, false, || true));
    assert!(!may_start(None, false, || false));
}
