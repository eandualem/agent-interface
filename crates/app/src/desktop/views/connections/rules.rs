//! What the pane says about a message, a connection ID or a peer. These rules
//! hold no state and draw nothing, so both the live pane and its preview use them.
use agent_interface_roster::profiles::peers as api;
use api::{Direction, Receipt};

/// The colour a state line takes: quiet, done, or a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Muted,
    Ok,
    Attention,
}

/// One short line under a message saying where it stands.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct StateLine {
    pub icon: &'static str,
    pub text: String,
    pub tone: Tone,
}

fn line(icon: &'static str, text: impl Into<String>, tone: Tone) -> StateLine {
    StateLine {
        icon,
        text: text.into(),
        tone,
    }
}

/// A service timestamp as local HH:MM, or as sent when it does not parse.
pub(super) fn clock(stamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| stamp.to_owned())
}

/// The state line for `m`, whose local agent is `local`. A message waiting
/// for review has none: its card says so instead.
pub(super) fn state_line(m: &api::Message, local: &str) -> Option<StateLine> {
    use Tone::{Attention, Muted, Ok};
    let incoming = m.direction == Direction::Incoming;
    if m.state == "pending_review" {
        return None;
    }
    if let Receipt::Confirmed {
        outcome,
        recorded_at,
    } = &m.receipt
    {
        let at = clock(recorded_at);
        match outcome.as_str() {
            "delivered" => return Some(line("check-check", format!("Delivered {at}"), Ok)),
            // Only the other profile can decline an outgoing message after
            // it was sent; the person's own decline carries no receipt.
            "declined" if !incoming => {
                return Some(line("x", format!("They declined it {at}"), Attention));
            }
            "declined" => return Some(line("x", format!("You declined it {at}"), Muted)),
            _ => {}
        }
    }
    Some(match m.state.as_str() {
        "queued" => line("clock", "Queued", Muted),
        "uploaded" | "received" => line("check", "Sent · waiting for their receipt", Muted),
        "delivered" => line("check-check", "Delivered", Ok),
        "passed" if incoming => line(
            "check",
            format!("You passed it through · {local} has it"),
            Ok,
        ),
        "passed" => line("check", "You passed it through · sending", Muted),
        "declined" => line("x", "You declined it", Muted),
        "revision_requested" => line(
            "corner-down-left",
            format!("Sent back to {local} to revise"),
            Muted,
        ),
        "upload_failed" => {
            let reason = m.upload_failure.as_ref().map_or_else(
                || "the relay refused it".to_owned(),
                |f| f.code.replace('_', " "),
            );
            let retry = m
                .upload_failure
                .as_ref()
                .and_then(|f| f.retry_at.as_deref())
                .map(|at| format!(" · Agent Backbone retries {}", clock(at)))
                .unwrap_or_default();
            line(
                "triangle-alert",
                format!("Not sent: {reason}{retry}"),
                Attention,
            )
        }
        "expired" => line(
            "triangle-alert",
            "Expired before it was delivered",
            Attention,
        ),
        "revoked" => line(
            "triangle-alert",
            "Not delivered: the connection was disconnected",
            Attention,
        ),
        "upload_unconfirmed" => line("clock", "Upload unconfirmed", Muted),
        "outcome_unknown" => line("clock", "Delivery outcome unknown", Muted),
        "delivery_uncertain" => line("clock", "Delivery uncertain", Muted),
        _ if matches!(m.receipt, Receipt::Uncertain) => line("clock", "Delivery uncertain", Muted),
        other => line("clock", sentence(&other.replace('_', " ")), Muted),
    })
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// How many lines a message body shows before it is cut.
pub(super) const CUT_LINES: usize = 6;
/// About how many characters fill one line of a message card, so one long
/// paragraph is cut near six drawn lines too.
pub(super) const CUT_LINE_CHARS: usize = 90;

/// A body longer than [`CUT_LINES`] lines, or than that many lines' worth
/// of characters: the start shown, and how many source lines are left (at
/// least one). `None` when the whole body fits. A code fence opened in the
/// start is closed there, so the start still reads as Markdown.
pub(super) fn cut(text: &str) -> Option<(String, usize)> {
    let text = text.trim_end();
    let lines = text
        .match_indices('\n')
        .nth(CUT_LINES - 1)
        .map_or(text.len(), |(at, _)| at);
    let chars = text
        .char_indices()
        .nth(CUT_LINES * CUT_LINE_CHARS)
        .map_or(text.len(), |(at, _)| at);
    let mut end = lines.min(chars);
    if end == text.len() {
        return None;
    }
    let mid_line = end == chars && !text[end..].starts_with('\n');
    if mid_line {
        // End on a word where one is near, never inside a character.
        if let Some(space) = text[..end].rfind(char::is_whitespace)
            && space > end.saturating_sub(CUT_LINE_CHARS)
        {
            end = space;
        }
    }
    let rest = &text[end..];
    let more = rest
        .strip_prefix('\n')
        .unwrap_or(rest)
        .lines()
        .count()
        .max(1);
    let mut shown = text[..end].trim_end().to_owned();
    if mid_line {
        shown.push('…');
    }
    let fences = shown
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            ["```", "~~~"]
                .into_iter()
                .find(|mark| line.starts_with(mark))
        })
        .collect::<Vec<_>>();
    if fences.len() % 2 == 1 {
        shown.push('\n');
        shown.push_str(fences[fences.len() - 1]);
    }
    Some((shown, more))
}

/// The link that opens a cut body, or closes an opened one.
pub(super) fn cut_label(more: usize, open: bool) -> String {
    match (open, more) {
        (true, _) => "Show less".into(),
        (false, 1) => "Show more · 1 more line".into(),
        (false, n) => format!("Show more · {n} more lines"),
    }
}

/// What a pasted connection ID turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IdCheck {
    /// Nothing typed yet.
    Empty,
    /// The ID as Agent Backbone will take it, and the line that confirms it.
    Valid { id: String, note: String },
    /// This profile's own ID, or a malformed one: the reason.
    Invalid(&'static str),
}

/// Check a pasted ID as Agent Backbone does: `v1:` and 64 lowercase hex
/// characters. A paste may carry spaces or a line break, which are dropped.
pub(super) fn check_id(input: &str, local: &str) -> IdCheck {
    let id: String = input.split_whitespace().collect();
    if id.is_empty() {
        return IdCheck::Empty;
    }
    let Some(hex) = id.strip_prefix("v1:") else {
        return IdCheck::Invalid("A connection ID starts with v1:");
    };
    if !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return IdCheck::Invalid("After v1:, a connection ID has only 0–9 and a–f");
    }
    if hex.len() != 64 {
        return IdCheck::Invalid("After v1:, a connection ID has 64 characters");
    }
    if id == local {
        return IdCheck::Invalid("This is this profile’s own ID");
    }
    let note = format!("Valid ID, ending …{}", &hex[hex.len() - 8..]);
    IdCheck::Valid { id, note }
}

/// A review decision the person can take on a waiting message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Pass,
    Revise,
    Decline,
}

/// The decisions a waiting message offers: only the local agent's own
/// outgoing message can be sent back to revise.
pub(super) fn review_actions(direction: Direction) -> &'static [Action] {
    match direction {
        Direction::Outgoing => &[Action::Pass, Action::Revise, Action::Decline],
        Direction::Incoming => &[Action::Pass, Action::Decline],
    }
}

/// The line beside the actions saying what Pass through does.
pub(super) fn pass_note(direction: Direction, local: &str, peer: &str) -> String {
    match direction {
        Direction::Outgoing => format!("Pass through sends it to {peer} unchanged."),
        Direction::Incoming => format!("Pass through delivers it to {local} and notifies it."),
    }
}

/// The name a connection is shown by: the profile's own, else "Unnamed
/// profile", which is always shown with its [`fingerprint`].
pub(super) fn display_name(name: Option<&str>) -> &str {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Unnamed profile")
}

/// A connection ID shortened to its first 8 and last 4 hex characters.
pub(super) fn fingerprint(id: &str) -> String {
    let hex = id.strip_prefix("v1:").unwrap_or(id);
    match (hex.get(..8), hex.get(hex.len().saturating_sub(4)..)) {
        (Some(start), Some(end)) if hex.len() > 12 => format!("{start}…{end}"),
        _ => hex.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(direction: Direction, state: &str, receipt: Receipt) -> api::Message {
        api::Message {
            connection: api::Generation {
                peer_id: "v1:aa".into(),
                generation: "g".into(),
            },
            message_id: "m".into(),
            direction,
            text: String::new(),
            submitted_at: "2026-11-21T12:00:00Z".into(),
            expires_at: "2026-11-28T12:00:00Z".into(),
            state: state.into(),
            receipt,
            upload_failure: None,
        }
    }

    fn says(m: &api::Message) -> (&'static str, String, Tone) {
        let l = state_line(m, "main").expect("a state line");
        (l.icon, l.text, l.tone)
    }

    #[test]
    fn each_state_has_one_short_line() {
        use Direction::{Incoming, Outgoing};
        use Receipt::{Missing, Uncertain};
        let at = "2026-11-21T12:02:10Z";
        let confirmed = |outcome: &str| Receipt::Confirmed {
            outcome: outcome.into(),
            recorded_at: at.into(),
        };
        assert_eq!(
            state_line(&message(Outgoing, "pending_review", Missing), "main"),
            None
        );
        assert_eq!(
            says(&message(Outgoing, "queued", Missing)),
            ("clock", "Queued".into(), Tone::Muted)
        );
        for sent in ["uploaded", "received"] {
            assert_eq!(
                says(&message(Outgoing, sent, Missing)),
                (
                    "check",
                    "Sent · waiting for their receipt".into(),
                    Tone::Muted
                )
            );
        }
        assert_eq!(
            says(&message(Outgoing, "delivered", confirmed("delivered"))),
            ("check-check", format!("Delivered {}", clock(at)), Tone::Ok)
        );
        assert_eq!(
            says(&message(Incoming, "passed", Missing)),
            (
                "check",
                "You passed it through · main has it".into(),
                Tone::Ok
            )
        );
        assert_eq!(
            says(&message(Incoming, "declined", confirmed("declined"))),
            ("x", format!("You declined it {}", clock(at)), Tone::Muted)
        );
        assert_eq!(says(&message(Outgoing, "declined", Missing)).2, Tone::Muted);
        // The other profile's decline is a refusal.
        assert_eq!(
            says(&message(Outgoing, "declined", confirmed("declined"))),
            (
                "x",
                format!("They declined it {}", clock(at)),
                Tone::Attention
            )
        );
        assert_eq!(
            says(&message(Outgoing, "expired", Missing)).2,
            Tone::Attention
        );
        assert_eq!(
            says(&message(Outgoing, "revoked", Missing)).2,
            Tone::Attention
        );
        assert_eq!(
            says(&message(Incoming, "delivery_uncertain", Uncertain)),
            ("clock", "Delivery uncertain".into(), Tone::Muted)
        );
        assert_eq!(
            says(&message(Outgoing, "some_new_state", Missing)).1,
            "Some new state"
        );
    }

    #[test]
    fn a_failure_gives_its_reason_and_when_it_is_retried() {
        let mut m = message(Direction::Outgoing, "upload_failed", Receipt::Missing);
        m.upload_failure = Some(api::UploadFailure {
            code: "mailbox_full".into(),
            retry_at: Some("2026-11-21T12:05:00Z".into()),
        });
        assert_eq!(
            says(&m),
            (
                "triangle-alert",
                format!(
                    "Not sent: mailbox full · Agent Backbone retries {}",
                    clock("2026-11-21T12:05:00Z")
                ),
                Tone::Attention
            )
        );
        m.upload_failure.as_mut().unwrap().retry_at = None;
        assert_eq!(says(&m).1, "Not sent: mailbox full");
    }

    #[test]
    fn a_body_over_six_lines_is_cut_at_six() {
        let six = (1..=6)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(cut(&six), None);
        assert_eq!(cut(&format!("{six}\n\n")), None);
        let thirty = (1..=30)
            .map(|n| format!("- item {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (shown, more) = cut(&thirty).unwrap();
        assert_eq!(shown.lines().count(), 6);
        assert!(shown.ends_with("- item 6"));
        assert_eq!(more, 24);
        // The 30-line Markdown message also parses to its first six items.
        use crate::desktop::views::markdown::{Block, parse_authored};
        assert!(matches!(
            parse_authored(&shown).as_slice(),
            [Block::List { items, .. }] if items.len() == 6
        ));
        // One long paragraph is cut near six drawn lines, on a word.
        let long = "word ".repeat(200);
        let (shown, more) = cut(&long).unwrap();
        assert!(shown.chars().count() <= CUT_LINES * CUT_LINE_CHARS + 1);
        assert!(shown.ends_with("word…"));
        assert_eq!(more, 1);
        // A fence opened in the first six lines is closed where it is cut.
        let fenced = format!(
            "Run this:\n```sh\n{}\n```\nDone.",
            (1..=10)
                .map(|n| format!("step {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let (shown, more) = cut(&fenced).unwrap();
        assert!(shown.ends_with("step 4\n```"));
        assert_eq!(more, 8);
        assert!(matches!(
            parse_authored(&shown).as_slice(),
            [Block::Paragraph(_), Block::Code(code)] if code.lines().count() == 4
        ));
        assert_eq!(cut_label(24, false), "Show more · 24 more lines");
        assert_eq!(cut_label(1, false), "Show more · 1 more line");
        assert_eq!(cut_label(24, true), "Show less");
    }

    #[test]
    fn a_connection_id_is_checked_as_agent_backbone_checks_it() {
        let local = format!("v1:{}", "1a".repeat(32));
        let other = format!("v1:{}7ad02815", "0".repeat(56));
        assert_eq!(check_id("  ", &local), IdCheck::Empty);
        assert_eq!(
            check_id(&format!(" {}\n{} ", &other[..20], &other[20..]), &local),
            IdCheck::Valid {
                id: other.clone(),
                note: "Valid ID, ending …7ad02815".into()
            }
        );
        assert_eq!(
            check_id(&local, &local),
            IdCheck::Invalid("This is this profile’s own ID")
        );
        assert_eq!(
            check_id(&other[3..], &local),
            IdCheck::Invalid("A connection ID starts with v1:")
        );
        assert_eq!(
            check_id(&other[..40], &local),
            IdCheck::Invalid("After v1:, a connection ID has 64 characters")
        );
        assert_eq!(
            check_id(&other.to_uppercase().replace("V1:", "v1:"), &local),
            IdCheck::Invalid("After v1:, a connection ID has only 0–9 and a–f")
        );
    }

    #[test]
    fn only_an_outgoing_message_can_be_sent_back_to_revise() {
        use Action::{Decline, Pass, Revise};
        assert_eq!(review_actions(Direction::Outgoing), [Pass, Revise, Decline]);
        assert_eq!(review_actions(Direction::Incoming), [Pass, Decline]);
        assert_eq!(
            pass_note(Direction::Outgoing, "main", "CoolAI"),
            "Pass through sends it to CoolAI unchanged."
        );
        assert_eq!(
            pass_note(Direction::Incoming, "main", "CoolAI"),
            "Pass through delivers it to main and notifies it."
        );
    }

    #[test]
    fn a_connection_shows_a_name_or_a_fingerprint_never_its_id() {
        assert_eq!(display_name(Some("CoolAI")), "CoolAI");
        assert_eq!(display_name(Some("  ")), "Unnamed profile");
        assert_eq!(display_name(None), "Unnamed profile");
        let id = format!("v1:466836eb{}3c5b", "0".repeat(52));
        assert_eq!(fingerprint(&id), "466836eb…3c5b");
        assert_eq!(fingerprint("v1:short"), "short");
    }
}
