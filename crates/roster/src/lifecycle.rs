//! Starting or resuming a registered agent through Backbone, on request only.

use serde::{Deserialize, Serialize};

/// Default colours supplied before a new CLI can query its terminal.
/// Kept in saved setup requests so retries use exactly the original palette.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDefaults {
    #[serde(deserialize_with = "hex_color")]
    foreground: String,
    #[serde(deserialize_with = "hex_color")]
    background: String,
}

impl TerminalDefaults {
    /// Construct validated RGB colours; no terminal escape data is accepted.
    pub fn new(foreground: [u8; 3], background: [u8; 3]) -> Self {
        let hex = |[r, g, b]: [u8; 3]| format!("#{r:02x}{g:02x}{b:02x}");
        Self {
            foreground: hex(foreground),
            background: hex(background),
        }
    }
}

fn hex_color<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("Expected an RGB colour (#RRGGBB)"))
    }
}

/// One request's result. An ambiguous reply must never cause automatic resubmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Accepted by Backbone; the terminal connects once the session exists.
    Accepted(String),
    /// A running session was left untouched.
    AlreadyRunning,
    /// Refused locally or by the provider.
    Refused(String),
    /// The request may have started the agent. Check before trying again.
    Unknown,
}

impl Outcome {
    /// A short user-facing result, without private response diagnostics.
    pub fn message(&self) -> String {
        match self {
            Self::Accepted(message) | Self::Refused(message) => message.clone(),
            Self::AlreadyRunning => "Already running. Connecting to its terminal.".into(),
            Self::Unknown => {
                "Could not confirm the launch. Check the agent's status before trying again.".into()
            }
        }
    }
}

/// What a start reply means. `reply` is the body, or the refusing status if known.
pub(crate) fn start_outcome(
    name: &str,
    resume: bool,
    reply: Result<Vec<u8>, Option<u16>>,
) -> Outcome {
    let refused = |message: &str| Outcome::Refused(message.into());
    // Backbone reports a missing saved conversation like any other refusal.
    let not_resumed = "Backbone could not resume this agent. If it has no saved conversation, \
        Start new opens a new one; otherwise check its saved folder, runtime and model.";
    let reply = match reply {
        Ok(reply) => reply,
        Err(Some(401 | 403)) => return refused("Backbone refused access to start this agent."),
        Err(Some(400 | 404 | 409 | 422)) => {
            return refused(if resume {
                not_resumed
            } else {
                "Backbone refused the launch. Check the agent's saved folder, runtime and model."
            });
        }
        Err(_) => return Outcome::Unknown,
    };
    #[derive(Deserialize)]
    struct Reply {
        ok: bool,
        session: String,
        already_existed: bool,
        ready: String,
    }
    let Ok(reply) = serde_json::from_slice::<Reply>(&reply) else {
        return Outcome::Unknown;
    };
    if reply.session != name {
        return Outcome::Unknown;
    }
    if reply.already_existed {
        return Outcome::AlreadyRunning;
    }
    if !reply.ok {
        return refused(if resume {
            not_resumed
        } else {
            "The agent could not start. Check its saved configuration in Backbone."
        });
    }
    match reply.ready.as_str() {
        "not_waited" | "ready" | "waiting_for_human" | "timeout" => {
            Outcome::Accepted("Starting. Connecting once its terminal is ready.".into())
        }
        "inbox_only" => refused("This agent has no terminal to open."),
        _ => Outcome::Unknown,
    }
}

/// Whether `name`'s saved runtime advertises `terminal_defaults`, from the
/// runtime catalog and the agent's inspection that `read` returns.
pub(crate) fn supports_defaults(read: impl Fn(&str) -> Option<Vec<u8>>, name: &str) -> bool {
    #[derive(Deserialize)]
    struct Runtime {
        id: String,
        #[serde(default)]
        terminal_defaults_supported: bool,
    }
    #[derive(Deserialize)]
    struct Agent {
        runtime: String,
    }
    let Some(catalog) =
        read("/api/runtimes").and_then(|bytes| serde_json::from_slice::<Vec<Runtime>>(&bytes).ok())
    else {
        return false;
    };
    if !catalog
        .iter()
        .any(|runtime| runtime.terminal_defaults_supported)
    {
        return false;
    }
    let Some(agent) = read(&format!("/api/agents/{name}/inspect"))
        .and_then(|bytes| serde_json::from_slice::<Agent>(&bytes).ok())
    else {
        return false;
    };
    catalog
        .iter()
        .any(|runtime| runtime.id == agent.runtime && runtime.terminal_defaults_supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sent_only_for_the_saved_runtime_capability() {
        for (catalog, runtime, expected) in [
            (
                r#"[{"id":"codex","terminal_defaults_supported":true}]"#,
                Some("codex"),
                true,
            ),
            (
                r#"[{"id":"codex","terminal_defaults_supported":true}]"#,
                Some("shell"),
                false,
            ),
            (r#"[{"id":"codex"}]"#, None, false),
            (
                r#"{"schema_version":2,"items":[{"id":"codex","terminal_defaults_supported":true}]}"#,
                None,
                false,
            ),
            ("not a catalog", None, false),
        ] {
            let read = |path: &str| match path {
                "/api/runtimes" => Some(catalog.as_bytes().to_vec()),
                "/api/agents/worker/inspect" => runtime.map(|runtime| {
                    serde_json::json!({ "runtime": runtime })
                        .to_string()
                        .into_bytes()
                }),
                _ => None,
            };
            assert_eq!(supports_defaults(read, "worker"), expected, "{catalog}");
        }
    }

    #[test]
    fn saved_colours_reject_terminal_control_data() {
        for color in ["red", "#fff", "#000000\u{1b}", "#00zz00", "éffffff"] {
            let input = serde_json::json!({"foreground":color,"background":"#000000"});
            assert!(serde_json::from_value::<TerminalDefaults>(input).is_err());
        }
    }

    #[test]
    fn a_refused_resume_never_reads_as_a_fresh_start() {
        assert!(matches!(
            start_outcome("worker", true, Err(Some(400))),
            Outcome::Refused(reason) if reason.contains("could not resume")
        ));
    }

    #[test]
    fn start_new_reports_acceptance_and_an_already_running_agent() {
        let reply = |existed: bool| {
            Ok(serde_json::json!({
                "ok": true, "session": "worker", "already_existed": existed, "ready": "not_waited"
            })
            .to_string()
            .into_bytes())
        };
        assert!(matches!(
            start_outcome("worker", false, reply(false)),
            Outcome::Accepted(_)
        ));
        assert_eq!(
            start_outcome("worker", false, reply(true)),
            Outcome::AlreadyRunning
        );
    }

    #[test]
    fn an_unreadable_reply_is_unknown() {
        assert_eq!(
            start_outcome("worker", false, Ok(b"not json".to_vec())),
            Outcome::Unknown
        );
    }
}
