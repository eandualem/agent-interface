//! Runs the app's own first-agent setup against a real Agent Backbone service.
//! Ignored by default: run it by name against a private trial whose service is
//! up and has only its Default profile, in the environment that service was
//! started in: `PATH` (the trial's `backbone` first), `HOME`, `BACKBONE_DIR`
//! and `TMUX_TMPDIR` all the trial's own under `/tmp`, `TMUX` unset, and the
//! database settings it was started with (`PGHOST`, `PGPORT`). The guards
//! catch a run from an ordinary shell; whoever makes the trial answers for
//! what its folders and its service hold, the service's own environment
//! among them.
//!
//! The agent comes from the environment: `TRIAL_AGENT_DIR` (its folder under
//! `/tmp`, created by setup), `TRIAL_RUNTIME` (`codex`, the default, or
//! `claude`), `TRIAL_MODEL` (sent as null when unset) and `TRIAL_AGENT_NAME`
//! (default `guide`).
//!
//! The request is prepared and run as the guide's own code does it, without
//! the guide's journal: saving, restoring and admitting a request are the
//! journal's own tests. The greeting is always asked for, as the guide asks
//! for it with the default brief in these two tools.

use agent_interface_app::profiles::{Client, setup::Setup};
use agent_interface_roster::profiles::setup::{Action, Agent, Brief, Prepared};
use std::path::{Component, Path, PathBuf};

fn live() -> bool {
    true
}

/// Whether `path` is inside the temporary folder: never that folder itself,
/// where tmux keeps its default server.
fn temporary(path: &Path) -> bool {
    ["/tmp", "/private/tmp"].iter().any(|root| {
        path.strip_prefix(root)
            .is_ok_and(|rest| rest.components().next().is_some())
    })
}

/// The trial's client. Nothing of a person's own: every place the service is
/// told to use is a folder inside the temporary one, and no tmux client is
/// inherited. The XDG folders count when they are set.
fn client() -> Client {
    for (name, required) in [
        ("HOME", true),
        ("BACKBONE_DIR", true),
        ("TMUX_TMPDIR", true),
        ("XDG_CACHE_HOME", false),
        ("XDG_CONFIG_HOME", false),
        ("XDG_DATA_HOME", false),
        ("XDG_STATE_HOME", false),
        ("XDG_RUNTIME_DIR", false),
    ] {
        let Some(place) = std::env::var_os(name) else {
            assert!(!required, "{name} must be set");
            continue;
        };
        assert!(
            temporary(&PathBuf::from(place).canonicalize().unwrap()),
            "{name} must be a disposable folder inside the temporary one"
        );
    }
    assert!(std::env::var_os("TMUX").is_none(), "TMUX must be unset");
    // A shell an agent runs in carries more of its own Backbone than the
    // directory, so nothing else of that kind may be set.
    let own: Vec<_> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name.starts_with("BACKBONE_") && name != "BACKBONE_DIR")
        .collect();
    assert!(own.is_empty(), "these must be unset: {own:?}");
    let path = std::env::var_os("PATH").unwrap();
    let executable = std::env::split_paths(&path)
        .map(|directory| directory.join("backbone"))
        .find(|candidate| candidate.is_file())
        .expect("backbone on PATH");
    println!("backbone: {}", executable.display());
    Client::new(executable, path).unwrap()
}

#[test]
#[ignore = "needs a private trial of a real Agent Backbone service"]
fn first_agent_on_the_default_profile() {
    let client = client();
    let variable = |name: &str| std::env::var(name).ok();
    let name = variable("TRIAL_AGENT_NAME").unwrap_or_else(|| "guide".into());
    let directory = PathBuf::from(variable("TRIAL_AGENT_DIR").expect("TRIAL_AGENT_DIR"));
    // Setup makes the folder, so what exists of its path is what is checked:
    // a link on the way must not lead out of the temporary folder.
    let existing = directory
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .expect("an existing folder above TRIAL_AGENT_DIR");
    assert!(
        temporary(&directory)
            && temporary(&existing.canonicalize().unwrap())
            && directory
                .components()
                .all(|part| part != Component::ParentDir),
        "TRIAL_AGENT_DIR must be a folder inside the trial's own under /tmp"
    );
    let runtime = variable("TRIAL_RUNTIME").unwrap_or_else(|| "codex".into());
    assert!(
        ["claude", "codex"].contains(&runtime.as_str()),
        "TRIAL_RUNTIME must be claude or codex"
    );
    let model = variable("TRIAL_MODEL");

    // The trial's one profile, so no other can be taken for it.
    let items = client.list(&live).expect("list");
    println!("profiles: {items:?}");
    let [item] = items.as_slice() else {
        panic!("the trial must have exactly one profile");
    };
    let profile = client.show(&item.profile_id, &live).expect("show");
    profile.connect().expect("handshake before the agent");

    // As the guide chooses: a tool the profile's catalog has on this
    // machine, and a model that tool advertises.
    let catalog = client
        .served_runtimes(&profile.expected_identity().expect("identity"), &live)
        .expect("catalog");
    let listed = catalog
        .items()
        .iter()
        .find(|listed| listed.id == runtime)
        .expect("the runtime in the profile's catalog");
    println!(
        "runtime {} available={} models={:?}",
        listed.id, listed.available, listed.models
    );
    assert!(listed.available);
    assert!(
        model
            .as_ref()
            .is_none_or(|model| listed.models.contains(model))
    );

    let agent = Agent {
        name: name.clone(),
        directory,
        create_directory: true,
        runtime,
        model,
        brief: Brief::Template {
            template: "orchestrator-v1".into(),
        },
    };
    let request_id = uuid::Uuid::now_v7().simple().to_string();
    let prepared = Prepared::first_agent(&request_id, &profile, agent, true).expect("a request");
    println!(
        "request: {}",
        String::from_utf8_lossy(prepared.bytes(Action::FirstAgent))
    );

    let setup = Setup::new(&client, prepared);
    let result = setup.run(Action::FirstAgent, &live, |event| {
        println!("step: {event:?}")
    });
    println!("result: {result:?}");
    let result = result.expect("an answer the app reads");
    assert_eq!(result.status, "completed");
    assert_eq!(result.code.as_deref(), Some("agent_ready"));
    assert_eq!(result.ready_agent.as_deref(), Some(name.as_str()));

    // The agent is in the roster the window reads over the profile's connection.
    let profile = client.show(&item.profile_id, &live).expect("show");
    let roster = profile
        .connect()
        .expect("handshake")
        .roster()
        .expect("roster");
    println!("roster: {:?}", roster.states);
    assert!(roster.states.contains_key(&name));
}
