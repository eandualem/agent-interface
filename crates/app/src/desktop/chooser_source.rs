//! Immutable discovery authority for native pane and attachment choosers.

use std::{io, path::PathBuf, time::Duration};

use agent_interface_core::{View, ViewKind};
use agent_interface_roster::{
    profiles::Connection,
    views::{Member, Request},
};

use super::views::source::Source;
use crate::sessions::{Observation, SessionSource};

#[derive(Clone)]
pub(super) enum Origin {
    Legacy {
        socket: Option<PathBuf>,
        directory: Option<PathBuf>,
    },
    Bound(Connection),
}

impl Origin {
    pub fn connection(&self) -> Option<&Connection> {
        match self {
            Self::Bound(connection) => Some(connection),
            Self::Legacy { .. } => None,
        }
    }

    pub fn has_provider(&self) -> bool {
        !matches!(
            self,
            Self::Legacy {
                directory: None,
                ..
            }
        )
    }

    pub fn allows_terminal(&self) -> bool {
        self.connection().is_none()
    }

    pub fn allows_view(&self, kind: ViewKind) -> bool {
        self.connection().is_none() || kind != ViewKind::Artifacts
    }

    /// Bind navigation to the registration in the displayed profile list.
    pub fn agent_view(&self, member: &Member) -> Option<View> {
        let mut view = View::agent(member.name.clone(), None);
        if self.connection().is_some() {
            view.registration_id = Some(member.registration_id.clone()?);
        }
        view.is_valid().then_some(view)
    }

    pub fn team(&self, wake: impl Fn() + Send + 'static) -> io::Result<Option<Source>> {
        if !self.has_provider() {
            return Ok(None);
        }
        let directory = match self {
            Self::Legacy { directory, .. } => directory.clone(),
            Self::Bound(_) => None,
        };
        Source::start(
            directory,
            self.connection(),
            Request::Team,
            Duration::from_secs(3),
            wake,
        )
        .map(Some)
    }

    pub fn sessions(&self, wake: impl Fn() + Send + 'static) -> io::Result<Option<Sessions>> {
        let source = match self {
            Self::Legacy {
                socket: Some(socket),
                ..
            } => SessionSource::start(socket.clone(), wake)?,
            Self::Legacy { socket: None, .. } => return Ok(None),
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

    pub fn take_update(&self) -> Option<Observation> {
        let update = self.source.take_update()?;
        let expected = self.origin.connection().map(Connection::identity);
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
    fn chooser_bound_session_discovery_uses_only_authenticated_socket() {
        let tmux = TmuxServer::start().unwrap();
        tmux.run(&["new-session", "-d", "-s", "bound-only"])
            .unwrap();
        let socket: PathBuf = tmux
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim_end()
            .into();
        let identity = json!({"schema_version":1,"profile_id":"profile-a","installation_id":"installation-a","lifecycle_id":"lifecycle-a","baseline_version":"fixture","tmux":{"socket_path":socket}});
        let (_dir, connection, server) = provider_with_tmux(
            "a",
            vec![
                Reply::ok("/api/profile", identity.clone()),
                Reply::ok("/api/profile", identity),
            ],
            Some(socket),
        );
        let origin = Origin::Bound(connection.clone());
        assert!(!origin.allows_terminal());
        assert!(origin.allows_view(ViewKind::Agent));
        assert!(!origin.allows_view(ViewKind::Artifacts));
        assert!(origin.allows_view(ViewKind::Reports));
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
        assert_eq!(request, Request::Team);
        assert!(result.is_err());
        drop(source);
        server.join().unwrap();
        let legacy = Origin::Legacy {
            socket: None,
            directory: None,
        };
        assert!(legacy.sessions(|| {}).unwrap().is_none());
        assert!(legacy.team(|| {}).unwrap().is_none());
        assert!(legacy.allows_terminal() && legacy.allows_view(ViewKind::Agent));
    }
}
