use std::io;
use std::path::{Path, PathBuf};

use tmux_workspaces_core::Attachment;

use super::Observation;
use crate::library::location::resolve_path;

/// Read-only identity and availability of one saved attachment.
#[derive(Debug, PartialEq, Eq)]
pub struct AttachmentStatus {
    /// The saved socket, or the window's captured source for legacy references.
    pub source_socket: PathBuf,
    /// Whether the attachment uses a different server from this window's chooser.
    pub saved_server: bool,
    /// Exact-name availability from a successful observation of this server.
    /// None means it has not been observed or discovery is unavailable. A failed
    /// or different-server observation never establishes that a session is online.
    pub online: Option<bool>,
}

/// Resolve a validated attachment without starting or connecting to a server.
///
/// Pass the window's captured source and, if available, the socket and latest
/// discovery observation for the attachment. Paths resolve symlinks even when
/// the socket is absent. An observation for another server is ignored, including
/// a same-named session on the chooser server. Metadata is not availability.
/// The saved reference and the observation remain unchanged.
pub fn attachment_status(
    attachment: &Attachment,
    window_source: &Path,
    discovery: Option<(&Path, &Observation)>,
) -> io::Result<AttachmentStatus> {
    let window_source = resolve_path(window_source)?;
    let source_socket = match &attachment.source_socket {
        Some(saved) => resolve_path(Path::new(saved))?,
        None => window_source.clone(),
    };
    let online = match discovery {
        Some((socket, observation))
            if resolve_path(socket)? == source_socket
                && observation.error.is_none()
                && observation.observed_at.is_some() =>
        {
            Some(
                observation
                    .sessions
                    .iter()
                    .any(|name| name == &attachment.session),
            )
        }
        _ => None,
    };
    Ok(AttachmentStatus {
        saved_server: source_socket != window_source,
        source_socket,
        online,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn saved_sources_keep_identity_and_only_their_own_discovery_establishes_availability() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let chooser = root.join("chooser.sock");
        let saved = root.join("saved server.sock");
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let mut attachment = Attachment {
            session: "worker with spaces".into(),
            source_socket: Some(alias.join("saved server.sock").to_str().unwrap().into()),
        };
        let mut observation = Observation {
            sessions: vec![attachment.session.clone()],
            error: None,
            observed_at: Some(Instant::now()),
        };
        let status =
            |reference: &Attachment, source: &Path, observed: Option<(&Path, &Observation)>| {
                attachment_status(reference, source, observed).unwrap()
            };
        assert_eq!(
            status(&attachment, &chooser, Some((&chooser, &observation))),
            AttachmentStatus {
                source_socket: saved.clone(),
                saved_server: true,
                online: None,
            }
        );
        assert_eq!(status(&attachment, &chooser, None).online, None);
        assert_eq!(
            status(&attachment, &chooser, Some((&saved, &observation))).online,
            Some(true)
        );
        assert!(!status(&attachment, &saved, Some((&saved, &observation))).saved_server);
        observation.error = Some("Session list unavailable".into());
        assert_eq!(
            status(&attachment, &chooser, Some((&saved, &observation))).online,
            None
        );
        observation.error = None;
        observation.sessions[0].push_str(" extra");
        assert_eq!(
            status(&attachment, &chooser, Some((&saved, &observation))).online,
            Some(false)
        );
        observation.sessions[0] = attachment.session.clone();
        assert_eq!(
            status(&attachment, &chooser, Some((&saved, &observation))).online,
            Some(true)
        );
        attachment.source_socket = None;
        let legacy = status(&attachment, &chooser, Some((&chooser, &observation)));
        assert_eq!(legacy.source_socket, chooser);
        assert!(!legacy.saved_server);
        assert_eq!(legacy.online, Some(true));
        assert!(attachment.source_socket.is_none());
        assert!(!chooser.exists() && !saved.exists());
    }
}
