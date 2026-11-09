//! Runs the app's own profile clients against a real Backbone provider.
//! Ignored by default: it needs a disposable provider home, so run it explicitly
//! with PATH, HOME and TMUX_TMPDIR pointing at disposable locations.

use agent_interface_app::profiles::{Client, lifecycle::Operation, setup::Setup};
use agent_interface_roster::profiles::{
    AddError, Connection, Error,
    operations::{Action as Lifecycle, Prepared, Status},
    runtime_context,
    setup::{Action, Agent, Brief, Prepared as SetupRequest},
};
use std::{path::PathBuf, process::Command};

fn live() -> bool {
    true
}

/// These tests write into `$HOME`, so it must be a disposable temporary folder.
fn disposable_home() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap())
        .canonicalize()
        .unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        home.starts_with(&temp) || home.starts_with("/tmp") || home.starts_with("/private/tmp"),
        "HOME must be a disposable temporary folder"
    );
    home
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
    disposable_home();
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

    // A stopped agent starts again through the bound route.
    let response = connection.start_agent("second", false);
    println!("start while registered: {response:?}");

    // Removing stops the session first, only for the registration inspected.
    // Once removed, that registration is gone, so removing it again is refused.
    let registration = runtime_context::inspect(connection.clone(), "second", None)
        .expect("inspect second")
        .target
        .registration_id()
        .to_owned();
    connection
        .remove_agent("second", &registration)
        .expect("remove agent");
    assert!(!sessions(&connection).contains(&"second".to_owned()));
    connection
        .roster()
        .expect("roster straight after a removal");
    assert_eq!(
        connection.remove_agent("second", &registration).err(),
        Some(Error::RegistrationChanged)
    );

    // A skill is copied from any folder, which stays in place.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let folder = home.join(format!("skill-{nonce}"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: skill-{nonce}\ndescription: Smoke\n---\nSmoke\n"),
    )
    .unwrap();
    let added = connection.add_skill(&folder).expect("add skill");
    println!("added skill: {added:?}");
    assert!(folder.join("SKILL.md").exists());
    let again = connection.add_skill(&folder).err();
    println!("added again: {again:?}");
    assert!(matches!(again, Some(AddError::Refused(_))));
    assert!(matches!(
        connection.add_skill(&home.join("no-such-folder")).err(),
        Some(AddError::Refused(_))
    ));

    // A person's own document needs no registered agent; a repeat is the same one.
    let document = connection
        .add_document("Overview", "<h1>Overview</h1>")
        .expect("add document");
    assert_eq!(
        connection.add_document("Overview", "<h1>Overview</h1>"),
        Ok(document)
    );
    println!("added document: {document}");

    // A stop ends the service this client started.
    let profile = client.show(&id.profile_id, &live).expect("show");
    let began = std::time::Instant::now();
    let mut stop = Operation::new(
        &client,
        Prepared::stop(&format!("stop-{nonce}"), &profile).unwrap(),
    );
    let receipt = stop.apply(&live).expect("stop");
    println!("stop: {:?} in {:?}", receipt.status, began.elapsed());
    assert_eq!(receipt.status, Status::Succeeded);
}

#[test]
#[ignore = "needs a disposable real provider"]
fn a_new_profile_copies_only_the_git_name_and_email() {
    let home = disposable_home();
    let config = home.join(".gitconfig");
    let client = client();
    let nonce = std::process::id();
    let create = |id: String| {
        let mut create = Operation::new(
            &client,
            Prepared::create_copying_git_identity(&id, &format!("G{nonce}"), "core-v1").unwrap(),
        );
        let receipt = create.apply(&live).expect("create");
        println!("create: {receipt:?}");
        receipt
    };

    let _ = std::fs::remove_file(&config);
    let refused = create(format!("git-unset-{nonce}"));
    assert_eq!(refused.status, Status::Failed);
    assert_eq!(
        refused.error_code.as_deref(),
        Some("git_identity_unavailable")
    );

    std::fs::write(
        &config,
        "[user]\n\tname = Example\n\temail = example@example.invalid\n[alias]\n\tco = checkout\n",
    )
    .unwrap();
    let made = create(format!("git-set-{nonce}"));
    assert_eq!(made.status, Status::Succeeded);
    let id = made.identity.expect("identity").profile_id;
    let shown = Command::new("backbone")
        .args(["profiles", "show", "--json", "--", &id])
        .output()
        .unwrap();
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["git_identity"], "copied");
    let socket = PathBuf::from(shown["binding"]["tmux"]["socket_path"].as_str().unwrap());
    let root = socket.parent().unwrap().parent().unwrap();
    let copied = std::fs::read_to_string(root.join("home/.gitconfig")).unwrap();
    println!("copied: {copied}");
    assert!(copied.contains("Example") && copied.contains("example@example.invalid"));
    assert!(!copied.contains("checkout"));
}
