//! Starting or resuming a registered agent through Backbone, on request only.

use serde::Deserialize;

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

#[cfg(test)]
mod tests {
    use super::*;

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
