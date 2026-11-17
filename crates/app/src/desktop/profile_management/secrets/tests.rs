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
