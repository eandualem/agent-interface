use sha2::{Digest, Sha256};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Locate an existing ordinary-shell socket for a resolved library directory.
///
/// Pass the absolute result of [`super::library_directory`]. The socket uses
/// Python's UTF-8 path hash and current UID. Its parent must be a real directory
/// owned by that UID, with no group or other permissions. Missing directories
/// or sockets return None. This reads metadata only: it does not create, repair,
/// connect to or start anything. A socket file does not prove a server is alive.
pub fn existing_shell_socket(resolved_directory: &Path) -> io::Result<Option<PathBuf>> {
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
    let directory = PathBuf::from(format!("/tmp/agent-interface-{uid}"));
    existing_socket_in(&directory, uid, &socket_name(text))?.map_or_else(
        || {
            existing_socket_in(
                &PathBuf::from(format!("/tmp/tmux-workspaces-{uid}")),
                uid,
                &socket_name(text),
            )
        },
        |socket| Ok(Some(socket)),
    )
}

/// Resolve the ordinary-shell socket for an explicit creation request. Unlike
/// lookup, this may create the private parent, but never repairs unsafe metadata.
/// The caller must have saved an eligible ordinary leaf before using this path.
pub fn shell_socket_for_creation(resolved_directory: &Path) -> io::Result<PathBuf> {
    if let Some(socket) = existing_shell_socket(resolved_directory)? {
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
        assert!(existing_shell_socket(Path::new("relative")).is_err());
        assert!(existing_shell_socket(Path::new(OsStr::from_bytes(b"/tmp/library-\xff"))).is_err());

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
