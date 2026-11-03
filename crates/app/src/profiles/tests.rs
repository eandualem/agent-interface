use super::*;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../roster/tests/fixtures/profiles-v1.json"
    )))
    .unwrap()
}

fn stub(script: &str) -> (tempfile::TempDir, Client) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("provider");
    fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut client = Client::new(executable, "/usr/bin:/bin".into()).unwrap();
    client.environment.push((
        "CAPTURE".into(),
        directory.path().join("calls").into_os_string(),
    ));
    client
        .environment
        .push(("HOME".into(), directory.path().as_os_str().to_owned()));
    (directory, client)
}

#[test]
fn discovery_only_lists_and_shows_records_without_connecting_or_mutating() {
    let fixture = fixture();
    let list = &fixture["discovery"]["list"];
    let ready = &fixture["discovery"]["show_ready"];
    let mut creating = fixture["discovery"]["show_creating"].clone();
    creating["profile_id"] = json!("-opaque;id");
    let script = format!(
        r#"
printf '%s\n' "$*" >> "$CAPTURE"
case "$*" in
  'profiles list --json') /bin/cat <<'FIXTURE'
{list}
FIXTURE
;;
  'profiles show --json -- profile-a') /bin/cat <<'FIXTURE'
{ready}
FIXTURE
;;
  'profiles show --json -- -opaque;id') /bin/cat <<'FIXTURE'
{creating}
FIXTURE
;;
  *) exit 9;;
esac
"#
    );
    let (directory, client) = stub(&script);
    let items = client.list(&|| true).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].profile_id, "profile-a");
    let profile = client.show("profile-a", &|| true).unwrap();
    assert_eq!(profile.item().state, profiles::State::Ready);
    assert_eq!(
        client.show("-opaque;id", &|| true).unwrap().item().state,
        profiles::State::Creating
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "profiles list --json\nprofiles show --json -- profile-a\nprofiles show --json -- -opaque;id\n"
    );
    // The canonical ready fixture points at nonexistent example paths. Showing
    // it succeeds because discovery neither reads credentials nor connects.
}

#[test]
fn runtime_observations_use_original_tuple_and_never_fall_back_to_legacy() {
    let wire = &fixture()["runtime_catalog"]["catalog"];
    let identity = profiles::Identity {
        profile_id: "profile-a".into(),
        installation_id: "installation-a".into(),
        lifecycle_id: "lifecycle-a1".into(),
    };
    let (directory, client) = stub(&format!(
        r#"
printf '%s\n' "$@" >> "$CAPTURE"
/bin/cat <<'FIXTURE'
{wire}
FIXTURE
"#
    ));
    let catalog = client.runtimes(&identity, &|| true).unwrap();
    assert_eq!(catalog.identity(), &identity);
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "runtimes\n--json\n--check-setup\n--profile=profile-a\n--expected-installation-id=installation-a\n--expected-lifecycle-id=lifecycle-a1\n"
    );
    let mut other = identity.clone();
    other.lifecycle_id = "another".into();
    assert!(matches!(
        client.runtimes(&other, &|| true),
        Err(Error::Profile(profiles::Error::BindingMismatch))
    ));
    let (directory, client) = stub("printf call >> \"$CAPTURE\"; exit 2");
    assert!(matches!(
        client.runtimes(&identity, &|| true),
        Err(Error::CommandFailed)
    ));
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "call"
    );
    let (_directory, client) = stub(r#"printf '{"schema_version":1,"items":[]}'"#);
    assert!(matches!(
        client.runtimes(&identity, &|| true),
        Err(Error::Profile(profiles::Error::Unsupported))
    ));
}

#[test]
fn discovery_preserves_operator_refusals_and_never_returns_failed_output() {
    let (directory, mut client) = stub(
        r#"
if [ "$BACKBONE_AGENT" = fixture-agent ]; then
  printf refused > "$CAPTURE"
  exit 7
fi
printf '{"schema_version":1,"items":[]}'
"#,
    );
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "fixture-agent".into()));
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "refused"
    );
    let (_directory, client) = stub(r#"printf '{"schema_version":1,"items":[]}'; exit 2"#);
    assert!(matches!(client.list(&|| true), Err(Error::CommandFailed)));
}

#[test]
fn discovery_rejects_wrong_profiles_and_invalid_or_oversized_output() {
    let ready = &fixture()["discovery"]["show_ready"];
    let (_directory, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{ready}\nFIXTURE"));
    assert!(matches!(
        client.show("another-profile", &|| true),
        Err(Error::Profile(profiles::Error::BindingMismatch))
    ));
    assert!(matches!(client.show("", &|| true), Err(Error::Invalid)));
    assert!(matches!(
        client.show("two words", &|| true),
        Err(Error::Invalid)
    ));
    let (_directory, client) = stub(r#"printf '{"schema_version":999,"items":[]}'"#);
    assert!(matches!(
        client.list(&|| true),
        Err(Error::Profile(profiles::Error::Unsupported))
    ));
    let (_directory, client) = stub("printf '{broken'");
    assert!(matches!(
        client.list(&|| true),
        Err(Error::Profile(profiles::Error::Invalid))
    ));
    let (_directory, client) = stub(&format!("/usr/bin/head -c {} /dev/zero", OUTPUT_LIMIT + 1));
    assert!(matches!(client.list(&|| true), Err(Error::TooLarge)));
    assert!(matches!(
        Client::new("relative-provider".into(), "/bin".into()),
        Err(Error::Invalid)
    ));
}

#[test]
fn discovery_deadlines_and_cancellation_stop_the_original_child() {
    let (directory, client) = stub(r#"printf started > "$CAPTURE"; exec /bin/sleep 5"#);
    assert!(matches!(client.list(&|| false), Err(Error::Cancelled)));
    assert!(!directory.path().join("calls").exists());
    let start = Instant::now();
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| true,
            Duration::from_millis(40)
        ),
        Err(Error::TimedOut)
    );
    assert!(start.elapsed() < Duration::from_secs(4));
    let start = Instant::now();
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| start.elapsed() < Duration::from_millis(40),
            Duration::from_secs(4)
        ),
        Err(Error::Cancelled)
    );
    assert!(start.elapsed() < Duration::from_secs(4));
}

#[test]
fn discovery_cleans_wrapper_children_on_cancellation_timeout_and_success() {
    for mode in ["cancel", "timeout", "success"] {
        let script = r#"
/bin/sh -c 'printf "%s" "$$" > "$CAPTURE.pid"; while :; do printf x >> "$CAPTURE.beats"; /bin/sleep 0.01; done' &
"#.to_owned() + if mode == "success" {
            r#"while [ ! -s "$CAPTURE.beats" ]; do /bin/sleep 0.01; done
printf '{"schema_version":1,"items":[]}'
"#
        } else { "wait\n" };
        let (directory, client) = stub(&script);
        let beats = directory.path().join("calls.beats");
        let result = client.read(
            &["profiles", "list", "--json"],
            &|| mode != "cancel" || !beats.exists(),
            Duration::from_secs(2),
        );
        let before = fs::metadata(&beats).unwrap().len();
        std::thread::sleep(Duration::from_millis(120));
        let after = fs::metadata(&beats).unwrap().len();
        if after != before {
            // Clean up the fixture even when testing a regressed implementation.
            // Growth proves this never-ending child still owns the recorded PID.
            let pid = fs::read_to_string(directory.path().join("calls.pid"))
                .unwrap()
                .parse()
                .unwrap();
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(pid).unwrap(),
                rustix::process::Signal::KILL,
            );
        }
        assert_eq!(after, before, "wrapper child survived {mode}");
        match mode {
            "cancel" => assert_eq!(result, Err(Error::Cancelled)),
            "timeout" => assert_eq!(result, Err(Error::TimedOut)),
            "success" => assert!(result.is_ok()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn an_unbounded_writer_hits_the_pipe_collection_limit() {
    let (_directory, client) = stub("exec /usr/bin/yes fixture");
    assert_eq!(
        client.read(
            &["profiles", "list", "--json"],
            &|| true,
            Duration::from_secs(5)
        ),
        Err(Error::TooLarge)
    );
}

#[test]
fn adoption_preflight_keeps_caller_and_directory_without_rewriting_prior_intent() {
    use profiles::operations::Prepared;
    let (directory, mut client) = stub(
        r#"
test "$*" = 'profiles adoption-preview --json' || exit 8
test "$BACKBONE_AGENT" = original || exit 9
test -f marker || exit 10
printf '%s\n' "$*" >> "$CAPTURE"
/bin/cat "$CAPTURE.reply"
"#,
    );
    client.directory = directory.path().canonicalize().unwrap();
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "original".into()));
    fs::write(directory.path().join("marker"), b"fixture").unwrap();
    let mut value = fixture()["adoption"]["preview"].clone();
    fs::write(
        directory.path().join("calls.reply"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let captured = client.clone();
    client
        .environment
        .push(("BACKBONE_AGENT".into(), "changed".into()));
    let other = tempfile::tempdir().unwrap();
    client.directory = other.path().canonicalize().unwrap();
    let preview = captured.adoption_preview(&|| true).unwrap();
    assert_eq!(preview.installation_id(), "installation-default");
    assert_eq!(
        preview.data_directory(),
        std::path::Path::new("/example/default")
    );
    let original = Prepared::adopt_default("adopt-default-1", &preview).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(original.bytes()).unwrap(),
        fixture()["adoption"]["request"]
    );
    value["adoption_token"] = "fresh-preflight-token".into();
    fs::write(
        directory.path().join("calls.reply"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let fresh = captured.adoption_preview(&|| true).unwrap();
    let next = Prepared::adopt_default("new-adoption", &fresh).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(original.bytes()).unwrap(),
        fixture()["adoption"]["request"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(next.bytes()).unwrap()["adoption_token"],
        "fresh-preflight-token"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("calls")).unwrap(),
        "profiles adoption-preview --json\nprofiles adoption-preview --json\n"
    );
}

#[test]
fn adoption_preflight_refuses_cancelled_failed_and_invalid_observations() {
    let value = fixture()["adoption"]["preview"].clone();
    let (directory, client) = stub(&format!(
        "printf called > \"$CAPTURE\"\n/bin/cat <<'FIXTURE'\n{value}\nFIXTURE\nexit 2"
    ));
    assert!(matches!(
        client.adoption_preview(&|| false),
        Err(Error::Cancelled)
    ));
    assert!(!directory.path().join("calls").exists());
    assert!(matches!(
        client.adoption_preview(&|| true),
        Err(Error::CommandFailed)
    ));
    for field in ["data_dir", "adoption_token", "installation_id"] {
        let mut invalid = value.clone();
        invalid[field] = "".into();
        let (_directory, client) = stub(&format!("/bin/cat <<'FIXTURE'\n{invalid}\nFIXTURE"));
        assert!(matches!(
            client.adoption_preview(&|| true),
            Err(Error::Profile(profiles::Error::Invalid))
        ));
    }
}
