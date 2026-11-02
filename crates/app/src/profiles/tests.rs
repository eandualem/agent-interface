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
