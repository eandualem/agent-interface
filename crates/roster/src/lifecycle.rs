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
    /// A short user-facing result for a terminal pane that connects automatically.
    pub fn message(&self) -> String {
        match self {
            Self::Accepted(message) | Self::Refused(message) => message.clone(),
            Self::AlreadyRunning => "Already running. Connecting to its terminal.".into(),
            Self::Unknown => {
                "Could not confirm the launch. Check the agent's status before trying again.".into()
            }
        }
    }

    /// A short user-facing result for a roster control that does not open a terminal.
    pub fn roster_message(&self) -> String {
        match self {
            Self::Accepted(_) => "Starting…".into(),
            Self::AlreadyRunning => "Already running.".into(),
            Self::Refused(message) => message.clone(),
            Self::Unknown => {
                "Could not confirm the launch. Check the agent's status before trying again.".into()
            }
        }
    }
}

/// One control action's result (stop or restart). An ambiguous reply must
/// never cause automatic resubmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Accepted or completed by Backbone.
    Done(String),
    /// Refused locally or by the provider, carrying the provider's reason when available.
    Refused(String),
    /// Could not confirm whether the action took effect.
    Unknown,
}

impl ActionOutcome {
    /// Whether Backbone confirmed or accepted the action.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Done(_))
    }

    /// A short user-facing result, without private response diagnostics.
    pub fn message(&self) -> String {
        match self {
            Self::Done(message) | Self::Refused(message) => message.clone(),
            Self::Unknown => {
                "Could not confirm the request. Check the agent's status before trying again."
                    .into()
            }
        }
    }
}

/// A service-provided refusal reason suitable for display in lifecycle flows.
pub fn start_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(crate::profiles::setup::one_line)
        .filter(|line| !line.is_empty())
}

/// Extract a sanitized one-line reason from a Backbone error response body.
pub(crate) fn service_reason(body: &[u8]) -> Option<String> {
    if body.is_empty() || body.len() > crate::RESPONSE_LIMIT || !crate::within_depth(body) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let raw = value
        .get("detail")
        .and_then(|detail| {
            detail
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| detail.as_str())
        })
        .or_else(|| value.get("reason").and_then(serde_json::Value::as_str));
    start_reason(raw)
}

/// Whether an HTTP status represents a definite refusal rather than an
/// ambiguous server/gateway error whose effect cannot be confirmed.
fn is_refusal_status(status: u16) -> bool {
    status < 500 || status == 503
}

/// What a start reply means when the provider answered with a status and body,
/// or failed to answer. Preserves the service's refusal reason on explicit
/// refusals including `503 machine_unreachable`, while keeping ambiguous 5xx
/// responses (`500`/`502`/`504`) as `Outcome::Unknown`.
pub(crate) fn start_response(
    name: &str,
    resume: bool,
    reply: Result<(u16, Vec<u8>), ()>,
) -> Outcome {
    match reply {
        Ok((status, body)) if (200..300).contains(&status) => start_outcome(name, resume, Ok(body)),
        Ok((status, body)) if is_refusal_status(status) => {
            if let Some(reason) = service_reason(&body) {
                return Outcome::Refused(reason);
            }
            start_outcome(name, resume, Err(Some(status)))
        }
        Ok(_) | Err(()) => Outcome::Unknown,
    }
}

/// What a stop reply means. Preserves the service's refusal reason on explicit
/// refusals while keeping ambiguous 5xx (`500`/`502`/`504`) as `Unknown`.
pub(crate) fn stop_response(name: &str, reply: Result<(u16, Vec<u8>), ()>) -> ActionOutcome {
    match reply {
        Ok((status, _)) if (200..300).contains(&status) => {
            ActionOutcome::Done(format!("Stopped {name}."))
        }
        Ok((status, body)) if is_refusal_status(status) => {
            if let Some(reason) = service_reason(&body) {
                return ActionOutcome::Refused(reason);
            }
            ActionOutcome::Refused(match status {
                401 | 403 => "Backbone refused access to stop this agent.".into(),
                404 => format!("{name} is not registered or not running."),
                409 => "This agent's registration changed. Refresh and try again.".into(),
                503 => "The agent's machine could not be reached.".into(),
                _ => format!("Backbone could not stop {name} ({status})."),
            })
        }
        Ok(_) | Err(()) => ActionOutcome::Unknown,
    }
}

/// What a restart reply means. Preserves the service's refusal reason on explicit
/// refusals while keeping ambiguous 5xx (`500`/`502`/`504`) as `Unknown`.
pub(crate) fn restart_response(name: &str, reply: Result<(u16, Vec<u8>), ()>) -> ActionOutcome {
    match reply {
        Ok((status, _)) if (200..300).contains(&status) => {
            ActionOutcome::Done(format!("Restarting {name}…"))
        }
        Ok((status, body)) if is_refusal_status(status) => {
            if let Some(reason) = service_reason(&body) {
                return ActionOutcome::Refused(reason);
            }
            ActionOutcome::Refused(match status {
                401 | 403 => "Backbone refused access to restart this agent.".into(),
                404 => format!("{name} is not a registered agent."),
                409 => format!("{name} already has a restart in progress."),
                422 => "Backbone refused the restart request.".into(),
                503 => "The agent's machine could not be reached.".into(),
                _ => format!("Backbone could not restart {name} ({status})."),
            })
        }
        Ok(_) | Err(()) => ActionOutcome::Unknown,
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
        Err(Some(503)) => return refused("The agent's machine could not be reached."),
        Err(_) => return Outcome::Unknown,
    };
    #[derive(Deserialize)]
    struct Reply {
        ok: bool,
        session: String,
        already_existed: bool,
        ready: String,
        reason: Option<String>,
    }
    let Ok(reply) = serde_json::from_slice::<Reply>(&reply) else {
        return Outcome::Unknown;
    };
    if reply.session != name {
        return Outcome::Unknown;
    }
    if !reply.ok {
        if let Some(reason) = start_reason(reply.reason.as_deref()) {
            return Outcome::Refused(reason);
        }
        return refused(if resume {
            not_resumed
        } else {
            "The agent could not start. Check its saved configuration in Backbone."
        });
    }
    if reply.already_existed {
        return Outcome::AlreadyRunning;
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
    fn start_and_resume_show_the_reason_from_a_refused_launch() {
        let mut reply: serde_json::Value = serde_json::from_slice(include_bytes!(
            "profiles/setup/fixtures/agent-started-refused.json"
        ))
        .unwrap();
        reply["evidence"]
            .as_array_mut()
            .unwrap()
            .push("later observation".into());
        for resume in [false, true] {
            assert_eq!(
                start_outcome("builder", resume, Ok(serde_json::to_vec(&reply).unwrap())),
                Outcome::Refused("the brief could not be written: no space left on device".into())
            );
        }
    }

    #[test]
    fn an_unreadable_reply_is_unknown() {
        assert_eq!(
            start_outcome("worker", false, Ok(b"not json".to_vec())),
            Outcome::Unknown
        );
    }

    #[test]
    fn unreachable_remote_machine_reports_the_services_reason_for_start_stop_and_restart() {
        let unreachable =
            br#"{"detail":{"code":"machine_unreachable","message":"Machine build-linux is offline."}}"#
                .to_vec();
        assert_eq!(
            start_response("worker", false, Ok((503, unreachable.clone()))),
            Outcome::Refused("Machine build-linux is offline.".into())
        );
        assert_eq!(
            stop_response("worker", Ok((503, unreachable.clone()))),
            ActionOutcome::Refused("Machine build-linux is offline.".into())
        );
        assert_eq!(
            restart_response("worker", Ok((503, unreachable))),
            ActionOutcome::Refused("Machine build-linux is offline.".into())
        );
        assert_eq!(
            stop_response("worker", Ok((200, br#"{"ok":true}"#.to_vec()))),
            ActionOutcome::Done("Stopped worker.".into())
        );
        assert_eq!(
            restart_response(
                "worker",
                Ok((200, br#"{"id":7,"status":"pending"}"#.to_vec()))
            ),
            ActionOutcome::Done("Restarting worker…".into())
        );
        assert_eq!(stop_response("worker", Err(())), ActionOutcome::Unknown);
        assert_eq!(restart_response("worker", Err(())), ActionOutcome::Unknown);
        // Bodyless / code-only 503 still reports an unreachable machine on all three routes.
        let code_only = br#"{"detail":{"code":"machine_unreachable"}}"#.to_vec();
        assert_eq!(
            start_response("worker", false, Ok((503, code_only.clone()))),
            Outcome::Refused("The agent's machine could not be reached.".into())
        );
        assert_eq!(
            stop_response("worker", Ok((503, code_only.clone()))),
            ActionOutcome::Refused("The agent's machine could not be reached.".into())
        );
        assert_eq!(
            restart_response("worker", Ok((503, code_only))),
            ActionOutcome::Refused("The agent's machine could not be reached.".into())
        );
        // Ambiguous 500/502/504 errors stay Unknown even with a JSON detail body.
        let server_err =
            br#"{"detail":{"code":"internal","message":"timeout after write"}}"#.to_vec();
        for status in [500, 502, 504] {
            assert_eq!(
                start_response("worker", false, Ok((status, server_err.clone()))),
                Outcome::Unknown
            );
            assert_eq!(
                stop_response("worker", Ok((status, server_err.clone()))),
                ActionOutcome::Unknown
            );
            assert_eq!(
                restart_response("worker", Ok((status, server_err.clone()))),
                ActionOutcome::Unknown
            );
        }
        // Roster messages never claim a terminal is connecting.
        assert_eq!(
            Outcome::Accepted("Starting. Connecting once its terminal is ready.".into())
                .roster_message(),
            "Starting…"
        );
        assert_eq!(Outcome::AlreadyRunning.roster_message(), "Already running.");
    }
}
