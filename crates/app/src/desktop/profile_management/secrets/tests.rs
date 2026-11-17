use super::*;

#[test]
fn a_change_says_what_it_did_and_a_refusal_keeps_the_services_words() {
    let change = |set, was_set| {
        outcome(Ok(Change {
            name: "GITHUB_TOKEN".into(),
            set,
            was_set,
        }))
    };
    assert_eq!(change(true, false), "GITHUB_TOKEN was added.");
    assert_eq!(change(true, true), "GITHUB_TOKEN was replaced.");
    assert_eq!(change(false, true), "GITHUB_TOKEN was removed.");
    let message = "the value of GITHUB_TOKEN has a control character";
    assert_eq!(outcome(Err(AddError::Refused(message.into()))), message);
    assert_eq!(
        outcome(Err(AddError::Connection(Error::Refused(403)))),
        "The service refused it (403)."
    );
}

#[test]
fn githubs_token_comes_first_set_or_not() {
    let secret = |name: &str, set| Secret {
        name: name.into(),
        set,
    };
    let names = |shown: Vec<Secret>| -> Vec<(String, bool)> {
        shown.into_iter().map(|s| (s.name, s.set)).collect()
    };
    assert_eq!(
        names(ordered(&[secret("TELEGRAM_TOKEN", false)])),
        [
            ("GITHUB_TOKEN".into(), false),
            ("TELEGRAM_TOKEN".into(), false)
        ]
    );
    assert_eq!(
        names(ordered(&[secret("A", true), secret("GITHUB_TOKEN", true)])),
        [("GITHUB_TOKEN".into(), true), ("A".into(), true)]
    );
}

#[test]
fn each_system_names_only_the_restart_commands_that_apply_there() {
    let (macos, macos_commands) = MACOS_RESTART;
    assert_eq!(
        macos_commands,
        [
            "backbone service restart",
            "backbone down",
            "backbone up --detach"
        ]
    );
    let (other, other_commands) = OTHER_RESTART;
    assert_eq!(other_commands, ["backbone down", "backbone up --detach"]);
    for (paragraph, commands) in [MACOS_RESTART, OTHER_RESTART] {
        assert!(paragraph.starts_with("The change takes effect when the service next starts."));
        for command in commands {
            assert!(paragraph.contains(&format!("\"{command}\"")), "{command}");
        }
    }
    assert!(macos.contains("installed login service, run \"backbone service restart\""));
    assert!(!other.contains("service restart"));
    assert!(other.contains("those commands do not apply"));
    let chosen = if cfg!(target_os = "macos") {
        MACOS_RESTART
    } else {
        OTHER_RESTART
    };
    assert_eq!(RESTART, chosen);
}

fn secret(name: &str, set: bool) -> Secret {
    Secret {
        name: name.into(),
        set,
    }
}

#[test]
fn a_list_refusal_shows_the_services_words_and_a_lost_connection_a_sentence_of_ours() {
    let message = "only the operator may manage secrets";
    assert_eq!(
        listed(Err(AddError::Refused(message.into()))),
        List::Unavailable(message.into())
    );
    assert_eq!(
        listed(Err(AddError::Connection(Error::Unavailable))),
        List::Unavailable("Couldn't read the profile's secrets.".into())
    );
}

#[test]
fn a_change_keeps_its_own_result_when_the_list_cant_be_read_again() {
    let shown = || List::Shown(vec![secret("GITHUB_TOKEN", false), secret("A", true)]);
    let lost = || Err(AddError::Connection(Error::Unavailable));
    let set = Ok(Change {
        name: "GITHUB_TOKEN".into(),
        set: true,
        was_set: false,
    });
    assert_eq!(
        completed(shown(), &set, lost()),
        (
            List::Shown(vec![secret("GITHUB_TOKEN", true), secret("A", true)]),
            Some("Couldn't read the profile's secrets.".into())
        )
    );
    // The change's own note stands too, and the rows stay shown, so the
    // restart notice is drawn under them.
    assert_eq!(outcome(set.clone()), "GITHUB_TOKEN was added.");
    let removed = Ok(Change {
        name: "A".into(),
        set: false,
        was_set: true,
    });
    let refused = Err(AddError::Refused(
        "only the operator may manage secrets".into(),
    ));
    assert_eq!(
        completed(shown(), &removed, refused).0,
        List::Shown(vec![secret("GITHUB_TOKEN", false), secret("A", false)])
    );
    let added = Ok(Change {
        name: "B".into(),
        set: true,
        was_set: false,
    });
    let (List::Shown(rows), _) = completed(shown(), &added, lost()) else {
        panic!("the rows stay shown");
    };
    assert_eq!(rows.last(), Some(&secret("B", true)));
    // A refused change leaves every row as it was, and its message stands.
    let message = "the value of A has a control character";
    let refusal = Err(AddError::Refused(message.into()));
    assert_eq!(completed(shown(), &refusal, lost()).0, shown());
    assert_eq!(outcome(refusal), message);
    // A list that reads again replaces the rows.
    assert_eq!(
        completed(shown(), &set, Ok(vec![secret("C", true)])),
        (List::Shown(vec![secret("C", true)]), None)
    );
}

#[test]
fn a_refusal_because_the_service_changed_says_so_on_both_paths() {
    for error in [
        Error::BindingRequired,
        Error::BindingMismatch,
        Error::Unsupported,
    ] {
        assert_eq!(outcome(Err(AddError::Connection(error))), CHANGED);
        assert_eq!(
            listed(Err(AddError::Connection(error))),
            List::Unavailable(CHANGED.into())
        );
    }
    assert_eq!(
        CHANGED,
        "This profile's service changed. Go back and open the profile again."
    );
}

#[test]
fn a_profile_that_serves_another_now_isnt_told_to_start() {
    for error in [
        Error::BindingRequired,
        Error::BindingMismatch,
        Error::Unsupported,
    ] {
        assert_eq!(unconnected(error), CHANGED);
    }
    for error in [Error::Unavailable, Error::NotReady, Error::Invalid] {
        assert_eq!(unconnected(error), "Start the profile to see its secrets.");
    }
}
