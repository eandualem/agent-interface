//! Runs the app's own profile clients against a real Backbone provider.
//! Ignored by default: it needs a disposable provider home, so run it explicitly
//! with PATH, HOME and TMUX_TMPDIR pointing at disposable locations.

use agent_interface_app::profiles::{Client, lifecycle::Operation, setup::Setup};
use agent_interface_roster::profiles::{
    Connection, Error, Kind,
    operations::{Action as Lifecycle, Prepared},
    setup::{Action, Agent, Brief, Prepared as SetupRequest},
};
use std::{path::PathBuf, process::Command};

fn live() -> bool {
    true
}

fn client() -> Client {
    let path = std::env::var_os("PATH").unwrap();
    let executable = std::env::split_paths(&path)
        .map(|dir| dir.join("backbone"))
        .find(|candidate| candidate.is_file())
        .expect("backbone on PATH");
    Client::new(executable, path).unwrap()
}

fn sessions(connection: &Connection) -> Vec<String> {
    let listed = Command::new("tmux")
        .arg("-S")
        .arg(connection.tmux_socket())
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore = "needs a disposable real provider"]
fn create_start_setup_and_shell_on_a_new_profile() {
    let client = client();
    let nonce = std::process::id();

    let mut create = Operation::new(
        &client,
        Prepared::create(&format!("create-{nonce}"), &format!("P{nonce}"), "core-v1").unwrap(),
    );
    let receipt = create.apply(&live).expect("create");
    println!("create: {receipt:?}");
    let id = create.identity().expect("reserved identity").clone();
    let item = client
        .list(&live)
        .unwrap()
        .into_iter()
        .find(|item| item.profile_id == id.profile_id)
        .expect("listed");
    println!("listed: {item:?}");

    let profile = client.show(&id.profile_id, &live).expect("show");
    println!("connect before start ok: {}", profile.connect().is_ok());
    let mut start = Operation::new(
        &client,
        Prepared::start(&format!("start-{nonce}"), &profile).unwrap(),
    );
    println!("start: {:?}", start.apply(&live).expect("start"));
    assert_eq!(start.action(), Lifecycle::Start);

    let profile = client.show(&id.profile_id, &live).expect("show");
    let connection = profile.connect().expect("handshake");
    let session = connection
        .ensure_shell("0123456789ab", None)
        .expect("provider shell");
    assert_eq!(
        connection.ensure_shell("0123456789ab", None).unwrap(),
        session
    );
    assert!(sessions(&connection).contains(&session));

    let catalog = client
        .runtimes(connection.identity(), &live)
        .expect("catalog");
    for runtime in catalog.items() {
        println!(
            "runtime {} available={} setup={} compatible={} ({})",
            runtime.id,
            runtime.available,
            runtime.setup_supported,
            runtime.setup_compatibility.compatible,
            runtime.setup_compatibility.reason
        );
    }

    let directory = PathBuf::from(std::env::var("HOME").unwrap()).join(format!("project-{nonce}"));
    std::fs::create_dir_all(&directory).unwrap();
    let request = SetupRequest::first_agent(
        &format!("setup-{nonce}"),
        &profile,
        vec!["codex".into()],
        Agent {
            name: "worker".into(),
            directory,
            create_directory: false,
            runtime: "codex".into(),
            model: None,
            brief: Brief::Text {
                text: "Smoke".into(),
            },
        },
        false,
    )
    .expect("setup request");
    let setup = Setup::new(&client, request);
    let outcome = setup.run(Action::FirstAgent, &live, |event| {
        println!("setup step: {:?} {:?}", event.step, event.status)
    });
    println!("setup outcome: {outcome:?}");
    let roster = connection.roster().expect("roster");
    println!("roster error: {:?}", roster.error);

    // A repeat reuses the live session, and it lives on the verified socket.
    let sign_in = connection
        .sign_in_session("codex")
        .expect("sign-in session");
    assert_eq!(connection.sign_in_session("codex").unwrap(), sign_in);
    assert!(sessions(&connection).contains(&sign_in));
    assert_eq!(
        connection.sign_in_session("gemini").err(),
        Some(Error::Refused(422))
    );
    assert_eq!(
        connection
            .ensure_shell("0123456789ac", Some("/no/such/folder"))
            .err(),
        Some(Error::Refused(422))
    );

    // Adding another agent uses the same setup request.
    let directory = PathBuf::from(std::env::var("HOME").unwrap()).join(format!("second-{nonce}"));
    std::fs::create_dir_all(&directory).unwrap();
    let request = SetupRequest::first_agent(
        &format!("setup2-{nonce}"),
        &profile,
        vec!["codex".into()],
        Agent {
            name: "second".into(),
            directory,
            create_directory: false,
            runtime: "codex".into(),
            model: None,
            brief: Brief::Text {
                text: "Smoke".into(),
            },
        },
        false,
    )
    .expect("second setup request");
    let outcome = Setup::new(&client, request).run(Action::FirstAgent, &live, |_| ());
    println!("second agent outcome: {outcome:?}");
    println!("sessions: {:?}", sessions(&connection));

    // Removing stops the session first; removing it again still succeeds.
    connection.remove_agent("second").expect("remove agent");
    assert!(!sessions(&connection).contains(&"second".to_owned()));
    connection
        .roster()
        .expect("roster straight after a removal");
    connection.remove_agent("second").expect("remove again");
}

#[test]
#[ignore = "needs a disposable real provider with an identity"]
fn adopt_default() {
    let client = client();
    let preview = client.adoption_preview(&live).expect("adoption preview");
    let mut adopt = Operation::new(
        &client,
        Prepared::adopt_default(&format!("adopt-{}", std::process::id()), &preview).unwrap(),
    );
    println!("adopt: {:?}", adopt.apply(&live).expect("adopt"));
}

#[test]
#[ignore = "needs a disposable real provider restarted after adoption"]
fn adopted_default_opens_a_bound_shell() {
    let client = client();
    let default = client
        .list(&live)
        .unwrap()
        .into_iter()
        .find(|item| item.kind == Kind::LegacyDefault)
        .expect("adopted Default");
    let connection = client
        .show(&default.profile_id, &live)
        .unwrap()
        .connect()
        .expect("handshake");
    let session = connection
        .ensure_shell("0123456789ad", None)
        .expect("provider shell");
    assert!(sessions(&connection).contains(&session));
    println!("default roster ok: {}", connection.roster().is_ok());
    println!(
        "default catalog: {:?}",
        client.runtimes(connection.identity(), &live).map(|c| c
            .items()
            .iter()
            .filter(|r| r.available)
            .count())
    );
    assert_eq!(
        connection.sign_in_session("codex").err(),
        Some(Error::Refused(422))
    );
}
