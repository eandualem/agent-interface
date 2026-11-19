//! Runs the app's own first-agent setup against a real Agent Backbone service.
//! Ignored by default: run it by name against a private trial whose service is
//! up and has its Default profile, with `PATH` (the trial's `backbone` first),
//! `HOME`, `BACKBONE_DIR` and `TMUX_TMPDIR` all the trial's own.
//!
//! The agent comes from the environment: `TRIAL_AGENT_DIR` (its folder,
//! created by setup), `TRIAL_RUNTIME` (default `codex`), `TRIAL_MODEL` (left
//! out when unset) and `TRIAL_AGENT_NAME` (default `guide`).

use agent_interface_app::profiles::{Client, setup::Setup};
use agent_interface_roster::profiles::{
    Kind,
    setup::{Action, Agent, Brief, Prepared},
};
use std::path::PathBuf;

fn live() -> bool {
    true
}

fn client() -> Client {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap())
        .canonicalize()
        .unwrap();
    assert!(
        home.starts_with("/tmp") || home.starts_with("/private/tmp"),
        "HOME must be a disposable temporary folder"
    );
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

    // The profile first run uses: the one the registry lists as the default.
    let item = client
        .list(&live)
        .expect("list")
        .into_iter()
        .find(|item| item.kind == Kind::LegacyDefault)
        .expect("the Default profile");
    println!("profile: {item:?}");
    let profile = client.show(&item.profile_id, &live).expect("show");
    println!("connect before the agent ok: {}", profile.connect().is_ok());

    let agent = Agent {
        name: name.clone(),
        directory: PathBuf::from(variable("TRIAL_AGENT_DIR").expect("TRIAL_AGENT_DIR")),
        create_directory: true,
        runtime: variable("TRIAL_RUNTIME").unwrap_or_else(|| "codex".into()),
        model: variable("TRIAL_MODEL"),
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

    // What the window does next: read the roster over the profile's connection.
    let profile = client.show(&item.profile_id, &live).expect("show");
    let roster = profile.connect().expect("handshake").roster();
    println!("roster read ok: {}", roster.is_ok());
    assert!(roster.is_ok());
}
