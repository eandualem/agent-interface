use sha2::{Digest, Sha256};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Locate the server owning an exact ordinary session in a resolved library.
///
/// Pass the absolute result of [`super::library_directory`]. The socket uses
/// Python's UTF-8 path hash and current UID. Its parent must be a real directory
/// owned by that UID, with no group or other permissions. Missing directories
/// or sessions return None. Bounded read-only queries check both current and
/// legacy sockets without starting a server or attaching a client. Call on a
/// worker; socket existence alone does not establish session ownership.
pub fn existing_shell_socket(
    resolved_directory: &Path,
    leaf_id: &str,
) -> io::Result<Option<PathBuf>> {
    select_socket(existing_shell_sockets(resolved_directory)?, leaf_id, false)
}

fn existing_shell_sockets(
    resolved_directory: &Path,
) -> io::Result<impl Iterator<Item = io::Result<Option<PathBuf>>>> {
    if !resolved_directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shell lookup requires the resolved absolute library directory",
        ));
    }
    let text = resolved_directory.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Python-compatible shell lookup requires a UTF-8 library path",
        )
    })?;
    let uid = uzers::get_current_uid();
    let name = socket_name(text);
    Ok([
        PathBuf::from(format!("/tmp/agent-interface-{uid}")),
        PathBuf::from(format!("/tmp/tmux-workspaces-{uid}")),
    ]
    .into_iter()
    .map(move |directory| existing_socket_in(&directory, uid, &name)))
}

fn select_socket(
    candidates: impl IntoIterator<Item = io::Result<Option<PathBuf>>>,
    leaf_id: &str,
    for_creation: bool,
) -> io::Result<Option<PathBuf>> {
    let session = ordinary_session(leaf_id).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Invalid saved shell identity")
    })?;
    let mut failure = None;
    let mut preferred = None;
    // Validate and query each candidate lazily: a verified current owner does
    // not depend on the metadata of an unused legacy directory.
    for candidate in candidates {
        let socket = match candidate {
            Ok(Some(socket)) => socket,
            Ok(None) => continue,
            Err(error) => {
                failure = Some(error);
                continue;
            }
        };
        preferred.get_or_insert_with(|| socket.clone());
        match tmux_workspaces_tmux::discovery::read_sessions(&socket) {
            Ok(sessions) if sessions.contains(&session) => return Ok(Some(socket)),
            Ok(_) => {}
            Err(error) => failure = Some(error),
        }
    }
    // A metadata or query failure is not evidence that the session is absent.
    // Explicit creation must not turn an unavailable server into a second shell.
    match failure {
        Some(error) => Err(error),
        None if for_creation => Ok(preferred),
        None => Ok(None),
    }
}

/// Resolve the ordinary-shell socket for an explicit creation request. Unlike
/// lookup, this may create the private parent, but never repairs unsafe metadata.
/// The caller must have saved an eligible ordinary leaf before using this path.
pub fn shell_socket_for_creation(resolved_directory: &Path, leaf_id: &str) -> io::Result<PathBuf> {
    if let Some(socket) = select_socket(existing_shell_sockets(resolved_directory)?, leaf_id, true)?
    {
        return Ok(socket);
    }
    let text = resolved_directory
        .to_str()
        .filter(|_| resolved_directory.is_absolute())
        .ok_or_else(|| io::Error::other("Shell creation requires a resolved UTF-8 library path"))?;
    let uid = uzers::get_current_uid();
    create_socket_in(
        &PathBuf::from(format!("/tmp/agent-interface-{uid}")),
        uid,
        &socket_name(text),
    )
}

fn create_socket_in(directory: &Path, uid: u32, name: &str) -> io::Result<PathBuf> {
    match std::fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // Lookup validates a nonsymlink, privately owned parent and any existing socket.
    existing_socket_in(directory, uid, name)?;
    Ok(directory.join(name))
}

fn socket_name(resolved_directory: &str) -> String {
    let hash = format!("{:x}", Sha256::digest(resolved_directory.as_bytes()));
    format!("{}-terminals.sock", &hash[..16])
}

fn existing_socket_in(directory: &Path, uid: u32, name: &str) -> io::Result<Option<PathBuf>> {
    let metadata = match directory.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "shell socket directory must be a real private directory owned by the current user",
        ));
    }
    let socket = directory.join(name);
    match socket.metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => Ok(Some(socket)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shell socket path is not a Unix socket",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The ordinary shell session for one validated 12-character lowercase hex leaf
/// identity. Invalid identities return None; no tmux command is run.
pub fn ordinary_session(leaf_id: &str) -> Option<String> {
    (leaf_id.len() == 12
        && leaf_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| format!("terminal-{leaf_id}"))
}

/// Refresh one leaf's saved cwd from its ordinary shell, even when that shell
/// is parked behind an external attachment. `socket` is the existing library
/// shell socket, never the attachment's server. Call this on a worker before
/// splitting so the new leaf can inherit the ordinary shell's current directory.
/// Only this in-memory field changes, and only after a successful read. No shell
/// is created, no client attaches, and nothing is persisted.
pub fn remember_shell_directory(
    leaf: &mut agent_interface_core::Leaf,
    socket: &Path,
) -> io::Result<()> {
    let session = ordinary_session(&leaf.id)
        .ok_or_else(|| io::Error::other("Working directory unavailable"))?;
    let directory = tmux_workspaces_tmux::discovery::read_session_directory(socket, &session)?;
    leaf.cwd = Some(directory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn fixture_socket(
        sockets: &[PathBuf],
        leaf_id: &str,
        for_creation: bool,
    ) -> io::Result<Option<PathBuf>> {
        select_socket(
            sockets.iter().map(|socket| Ok(Some(socket.clone()))),
            leaf_id,
            for_creation,
        )
    }

    #[test]
    fn current_and_legacy_servers_are_selected_by_exact_session_ownership() {
        use tmux_workspaces_test_support::TmuxServer;

        let current = TmuxServer::start().unwrap();
        let legacy = TmuxServer::start().unwrap();
        let socket = |server: &TmuxServer| {
            PathBuf::from(
                server
                    .run(&["display-message", "-p", "#{socket_path}"])
                    .unwrap()
                    .trim_end(),
            )
        };
        let sockets = [socket(&current), socket(&legacy)];
        current
            .run(&[
                "new-session",
                "-d",
                "-s",
                "terminal-0123456789ab-extra",
                "sleep 60",
            ])
            .unwrap();
        for (index, server, id) in [(0, &current, "abcdef012345"), (1, &legacy, "0123456789ab")] {
            let cwd = server.address().0.canonicalize().unwrap();
            server
                .run(&[
                    "new-session",
                    "-d",
                    "-s",
                    &ordinary_session(id).unwrap(),
                    "-c",
                    cwd.to_str().unwrap(),
                    "sleep 60",
                ])
                .unwrap();
            assert_eq!(
                fixture_socket(&sockets, id, false).unwrap(),
                Some(sockets[index].clone())
            );
            // Explicit opening must reuse the owning server too, not create a
            // second ordinary shell on the preferred socket.
            let selected = fixture_socket(&sockets, id, true).unwrap().unwrap();
            assert_eq!(selected, sockets[index]);
            let mut leaf = agent_interface_core::Leaf {
                id: id.into(),
                attachment: None,
                cwd: Some("/previous".into()),
                empty: false,
                view: None,
            };
            remember_shell_directory(&mut leaf, &selected).unwrap();
            assert_eq!(leaf.cwd.as_deref(), cwd.to_str());
        }
        assert_eq!(
            fixture_socket(&sockets, "111111111111", false).unwrap(),
            None
        );
        assert_eq!(
            fixture_socket(&sockets, "111111111111", true).unwrap(),
            Some(sockets[0].clone())
        );
        assert_eq!(fixture_socket(&[], "111111111111", true).unwrap(), None);
        assert!(fixture_socket(&sockets, "invalid", true).is_err());
        assert!(current.run(&["list-clients"]).unwrap().is_empty());
        assert!(legacy.run(&["list-clients"]).unwrap().is_empty());
        let names = tmux_workspaces_tmux::discovery::read_sessions(&sockets[0]).unwrap();
        assert!(!names.contains(&"terminal-0123456789ab".into()));
    }

    #[test]
    fn failed_discovery_does_not_authorize_creation_on_another_socket() {
        use tmux_workspaces_test_support::TmuxServer;

        let server = TmuxServer::start().unwrap();
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap()
                .trim_end(),
        );
        let unresponsive = server.address().0.join("unresponsive.sock");
        let _listener = UnixListener::bind(&unresponsive).unwrap();
        assert!(fixture_socket(&[socket, unresponsive], "0123456789ab", true).is_err());
        assert_eq!(
            server
                .run(&["list-sessions", "-F", "#{session_name}"])
                .unwrap(),
            "fixture\n"
        );
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
    }

    #[test]
    fn verified_owner_survives_unused_unsafe_metadata_but_unknown_ownership_blocks_creation() {
        use std::cell::Cell;
        use tmux_workspaces_test_support::TmuxServer;

        let server = TmuxServer::start().unwrap();
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap()
                .trim_end(),
        );
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "terminal-0123456789ab",
                "sleep 60",
            ])
            .unwrap();
        let unsafe_directory = server.address().0.join("unsafe-legacy");
        std::fs::create_dir(&unsafe_directory).unwrap();
        std::fs::set_permissions(&unsafe_directory, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        for for_creation in [false, true] {
            for (id, owned) in [("0123456789ab", true), ("111111111111", false)] {
                let legacy_inspected = Cell::new(false);
                let candidates =
                    std::iter::once(Ok(Some(socket.clone()))).chain(std::iter::once_with(|| {
                        legacy_inspected.set(true);
                        existing_socket_in(
                            &unsafe_directory,
                            uzers::get_current_uid(),
                            "unused.sock",
                        )
                    }));
                let result = select_socket(candidates, id, for_creation);
                if owned {
                    assert_eq!(result.unwrap(), Some(socket.clone()));
                    assert!(!legacy_inspected.get());
                } else {
                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
                    assert!(legacy_inspected.get());
                }
            }
            // A verified legacy owner also survives an unusable current candidate.
            let candidates = [
                existing_socket_in(&unsafe_directory, uzers::get_current_uid(), "unused.sock"),
                Ok(Some(socket.clone())),
            ];
            assert_eq!(
                select_socket(candidates, "0123456789ab", for_creation).unwrap(),
                Some(socket.clone())
            );
        }
        assert_eq!(
            server
                .run(&["list-sessions", "-F", "#{session_name}"])
                .unwrap(),
            "fixture\nterminal-0123456789ab\n"
        );
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
        assert_eq!(
            unsafe_directory.symlink_metadata().unwrap().mode() & 0o777,
            0o755
        );
        assert!(!unsafe_directory.join("unused.sock").exists());
    }

    #[test]
    fn explicit_creation_makes_only_a_private_parent_and_rejects_unsafe_reuse() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("shells");
        let socket = create_socket_in(&parent, uzers::get_current_uid(), "fixture.sock").unwrap();
        assert!(!socket.exists());
        assert_eq!(parent.symlink_metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            create_socket_in(&parent, uzers::get_current_uid(), "fixture.sock").unwrap(),
            socket
        );
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(create_socket_in(&parent, uzers::get_current_uid(), "fixture.sock").is_err());
        assert_eq!(parent.symlink_metadata().unwrap().mode() & 0o777, 0o755);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&parent, &link).unwrap();
        assert!(create_socket_in(&link, uzers::get_current_uid(), "fixture.sock").is_err());
    }

    #[test]
    fn directory_refresh_uses_the_exact_ordinary_shell_and_preserves_newline_paths() {
        use agent_interface_core::{Attachment, Leaf};
        use tmux_workspaces_test_support::TmuxServer;

        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let socket = Path::new(socket.trim_end());
        let directory = server.address().0.join("shell café path\n");
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let mut leaf = Leaf {
            id: "0123456789ab".into(),
            attachment: Some(Attachment {
                session: "external".into(),
                source_socket: None,
            }),
            cwd: Some("/previous".into()),
            empty: false,
            view: None,
        };
        // A longer session name cannot stand in for this ordinary shell.
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "terminal-0123456789ab-extra",
                "sleep 60",
            ])
            .unwrap();
        assert!(remember_shell_directory(&mut leaf, socket).is_err());
        assert_eq!(leaf.cwd.as_deref(), Some("/previous"));
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "terminal-0123456789ab",
                "-c",
                directory.to_str().unwrap(),
                "sleep 60",
            ])
            .unwrap();
        remember_shell_directory(&mut leaf, socket).unwrap();
        assert_eq!(leaf.cwd.as_deref(), directory.to_str());
        assert_eq!(leaf.attachment.as_ref().unwrap().session, "external");
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
        let absent = server.address().0.join("absent.sock");
        assert!(remember_shell_directory(&mut leaf, &absent).is_err());
        assert_eq!(leaf.cwd.as_deref(), directory.to_str());
        assert!(!absent.exists());
    }

    #[test]
    fn shell_lookup_matches_python_and_only_reads_private_socket_metadata() {
        // Fixed hash from Python hashlib.sha256 for this resolved path.
        let name = socket_name("/tmp/example library");
        assert_eq!(name, "b97f6756417fe702-terminals.sock");
        assert_eq!(
            ordinary_session("123456abcdef").unwrap(),
            "terminal-123456abcdef"
        );
        for invalid in ["123", "123456ABCDEf", "session:name"] {
            assert!(ordinary_session(invalid).is_none());
        }
        assert!(existing_shell_socket(Path::new("relative"), "123456abcdef").is_err());
        assert!(
            existing_shell_socket(
                Path::new(OsStr::from_bytes(b"/tmp/library-\xff")),
                "123456abcdef"
            )
            .is_err()
        );

        // Short private fixture path stays within macOS's Unix socket limit.
        let fixture = tempfile::Builder::new()
            .prefix("tw-s-")
            .tempdir_in("/tmp")
            .unwrap();
        let directory = fixture.path().join("s");
        let uid = uzers::get_current_uid();
        assert!(
            existing_socket_in(&directory, uid, &name)
                .unwrap()
                .is_none()
        );
        assert!(!directory.exists());
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            existing_socket_in(&directory, uid, &name)
                .unwrap()
                .is_none()
        );
        let socket = directory.join(&name);
        let listener = UnixListener::bind(&socket).unwrap();
        assert_eq!(
            existing_socket_in(&directory, uid, &name).unwrap(),
            Some(socket.clone())
        );
        assert!(existing_socket_in(&directory, uid.wrapping_add(1), &name).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(existing_socket_in(&directory, uid, &name).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = fixture.path().join("alias");
        std::os::unix::fs::symlink(&directory, &alias).unwrap();
        assert!(existing_socket_in(&alias, uid, &name).is_err());
        drop(listener);
        std::fs::remove_file(&socket).unwrap();
        std::fs::write(&socket, b"preserve").unwrap();
        assert!(existing_socket_in(&directory, uid, &name).is_err());
        assert_eq!(std::fs::read(&socket).unwrap(), b"preserve");
    }
}
