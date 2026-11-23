//! Immutable discovery authority for native pane and attachment choosers.

use std::{io, path::PathBuf, time::Duration};

use agent_interface_core::View;
use agent_interface_roster::{
    profiles::{Connection, runs},
    views::{Member, Request},
};

use super::views::source::Source;
use crate::sessions::{Observation, SessionSource};

#[derive(Clone)]
pub(super) enum Origin {
    /// A workspace without a profile: the window's tmux server, no Backbone.
    Standalone {
        socket: Option<PathBuf>,
    },
    Bound(Connection),
}

impl Origin {
    pub fn connection(&self) -> Option<&Connection> {
        match self {
            Self::Bound(connection) => Some(connection),
            Self::Standalone { .. } => None,
        }
    }

    pub fn has_provider(&self) -> bool {
        matches!(self, Self::Bound(_))
    }

    /// A remote terminal is opened by agent name on this profile's socket.
    pub fn remote_profile(&self, member: &Member) -> Option<String> {
        let identity = self.connection()?.identity();
        member.machine.as_ref()?.remote_profile(identity)
    }

    /// Bind navigation to the registration in the displayed profile list.
    pub fn agent_view(&self, member: &Member) -> Option<View> {
        self.connection()?;
        let mut view = View::agent(member.name.clone(), None);
        view.registration_id = Some(member.registration_id.clone()?);
        view.is_valid().then_some(view)
    }

    pub fn team(&self, wake: impl Fn() + Send + 'static) -> io::Result<Option<Source>> {
        let Some(connection) = self.connection() else {
            return Ok(None);
        };
        Source::start(connection, Request::Members, Duration::from_secs(3), wake).map(Some)
    }

    pub fn sessions(&self, wake: impl Fn() + Send + 'static) -> io::Result<Option<Sessions>> {
        let source = match self {
            Self::Standalone {
                socket: Some(socket),
            } => SessionSource::start(socket.clone(), wake)?,
            Self::Standalone { socket: None } => return Ok(None),
            Self::Bound(connection) => SessionSource::start_bound(connection.clone(), wake)?,
        };
        Ok(Some(Sessions {
            origin: self.clone(),
            source,
        }))
    }
}

pub(super) struct Sessions {
    origin: Origin,
    source: SessionSource,
}

impl Sessions {
    pub fn set_active(&self, active: bool) {
        self.source.set_active(active);
    }

    /// The latest sessions, without a profile's run views: those open
    /// under their agent's runs, read-only.
    pub fn take_update(&self) -> Option<Observation> {
        let mut update = self.source.take_update()?;
        let expected = self.origin.connection().map(Connection::identity);
        if expected.is_some() {
            update.sessions.retain(|session| !runs::is_view(session));
        }
        (update.identity.as_ref() == expected).then_some(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::content::tests::{Reply, provider, provider_with_tmux};
    use agent_interface_test_support::TmuxServer;
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn only_an_agent_on_a_different_machine_uses_a_profile_stream() {
        let (_dir, connection, server) = provider("a", vec![]);
        let identity = connection.identity().clone();
        let origin = Origin::Bound(connection);
        let mut member = Member::default();
        assert!(origin.remote_profile(&member).is_none());
        member.machine = Some(agent_interface_roster::Machine {
            id: identity.installation_id.clone(),
            name: "same label".into(),
        });
        assert!(origin.remote_profile(&member).is_none());
        member.machine.as_mut().unwrap().id = "other".into();
        assert_eq!(origin.remote_profile(&member), Some(identity.profile_id));
        assert!(
            Origin::Standalone { socket: None }
                .remote_profile(&member)
                .is_none()
        );
        server.join().unwrap();
    }

    #[test]
    fn a_server_without_a_profile_lists_sessions_named_as_run_views() {
        let tmux = TmuxServer::start().unwrap();
        tmux.run(&[
            "new-session",
            "-d",
            "-s",
            "run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60",
        ])
        .unwrap();
        let socket: PathBuf = tmux
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim_end()
            .into();
        let (wake, ready) = mpsc::channel();
        let source = Origin::Standalone {
            socket: Some(socket),
        }
        .sessions(move || {
            let _ = wake.send(());
        })
        .unwrap()
        .unwrap();
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let observed = source.take_update().unwrap();
        assert_eq!(
            observed.sessions,
            ["fixture", "run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60"]
        );
    }

    #[test]
    fn chooser_bound_session_discovery_uses_only_authenticated_socket() {
        let tmux = TmuxServer::start().unwrap();
        tmux.run(&["new-session", "-d", "-s", "bound-only"])
            .unwrap();
        // A run's view opens under its agent, never as a loose session.
        tmux.run(&[
            "new-session",
            "-d",
            "-s",
            "run-0193a1b2c3d47e5f8a9b0c1d2e3f4a60",
        ])
        .unwrap();
        let socket: PathBuf = tmux
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim_end()
            .into();
        let identity = json!({"protocol_version":1,"profile_id":"profile-a","instance_id":"instance-a","machine":{"id":"machine-a"}});
        let (_dir, connection, server) = provider_with_tmux(
            "a",
            vec![
                Reply::ok("/api/profile", identity.clone()),
                Reply::ok("/api/profile", identity),
            ],
            Some(socket),
        );
        let origin = Origin::Bound(connection.clone());
        assert!(origin.has_provider());
        let (wake, ready) = mpsc::channel();
        let source = origin
            .sessions(move || {
                let _ = wake.send(());
            })
            .unwrap()
            .unwrap();
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let observed = source.take_update().unwrap();
        assert_eq!(observed.identity.as_ref(), Some(connection.identity()));
        assert_eq!(observed.sessions, ["bound-only", "fixture"]);
        source.set_active(false);
        assert!(source.take_update().is_none());
        drop(source);
        server.join().unwrap();
    }

    #[test]
    fn remote_chooser_members_need_only_the_agents_endpoint() {
        let (_dir, connection, server) = provider(
            "a",
            vec![Reply::ok(
                "/api/agents",
                serde_json::json!({"items":[{"name":"worker","machine":{"id":"remote","name":"Linux"}}]}),
            )],
        );
        let origin = Origin::Bound(connection);
        let (tx, rx) = mpsc::channel();
        let source = origin
            .team(move || {
                let _ = tx.send(());
            })
            .unwrap()
            .unwrap();
        rx.recv_timeout(Duration::from_secs(8)).unwrap();
        let (request, result) = source.take_update().unwrap();
        assert_eq!(request, Request::Members);
        let agent_interface_roster::views::Data::Team(team) = result.unwrap() else {
            panic!("missing members")
        };
        assert_eq!(
            origin.remote_profile(&team.members[0]).as_deref(),
            Some("profile-a")
        );
        source.set_active(false);
        server.join().unwrap();
    }
    #[test]
    fn chooser_team_read_keeps_binding_and_refuses_replacement() {
        let (_dir, connection, server) = provider(
            "a",
            vec![Reply {
                status: 409,
                ..Reply::ok("/api/agents", json!({"detail":{"code":"binding_mismatch"}}))
            }],
        );
        let origin = Origin::Bound(connection);
        let (wake, ready) = mpsc::channel();
        let source = origin
            .team(move || {
                let _ = wake.send(());
            })
            .unwrap()
            .unwrap();
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let (request, result) = source.take_update().unwrap();
        assert_eq!(request, Request::Members);
        assert!(result.is_err());
        drop(source);
        server.join().unwrap();
        let standalone = Origin::Standalone { socket: None };
        assert!(standalone.sessions(|| {}).unwrap().is_none());
        assert!(standalone.team(|| {}).unwrap().is_none());
        assert!(!standalone.has_provider());
    }
}
