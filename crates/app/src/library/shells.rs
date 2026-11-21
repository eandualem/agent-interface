mod binding;

use sha2::{Digest, Sha256};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Locate the server owning an exact ordinary session in a resolved library.
///
/// Pass the absolute result of [`super::library_directory`]. The socket is
/// selected by the library's saved binding, in a directory of the current
/// UID. That directory must be a real directory owned by that UID, with no
/// group or other permissions. Missing directories or sessions return None. A
/// bounded read-only query checks the socket without starting a server or
/// attaching a client. Call on a worker; socket existence alone does not
/// establish session ownership.
pub fn existing_shell_socket(
    resolved_directory: &Path,
    leaf_id: &str,
) -> io::Result<Option<PathBuf>> {
    select_socket(existing_library_socket(resolved_directory)?, leaf_id, false)
}

fn existing_library_socket(resolved_directory: &Path) -> io::Result<Option<PathBuf>> {
    let uid = uzers::get_current_uid();
    lookup_in(&socket_directory(uid), uid, resolved_directory)
}

fn lookup_in(root: &Path, uid: u32, resolved_directory: &Path) -> io::Result<Option<PathBuf>> {
    binding::legacy_key(resolved_directory)?;
    if !private_socket_directory(root, uid)? {
        return Ok(None);
    }
    let Some(key) = binding::lookup(root, resolved_directory)? else {
        return Ok(None);
    };
    existing_socket_in(root, uid, &format!("{key}-terminals.sock"))
}

/// Register a running pre-binding server before its library moves, or refresh
/// the recorded location of a server whose library has already moved.
pub(super) fn remember_server(library: &Path) -> io::Result<()> {
    let uid = uzers::get_current_uid();
    let root = socket_directory(uid);
    if private_socket_directory(&root, uid)? {
        binding::bind(&root, library, binding::Use::Open)?;
    }
    Ok(())
}

fn saved_leaves(model: &agent_interface_core::Layout) -> Vec<String> {
    model
        .workspaces
        .iter()
        .flat_map(|space| &space.tabs)
        .flat_map(|tab| tab.tree.leaves())
        .map(|leaf| leaf.id.clone())
        .collect()
}

fn read_leaves(library: &Path) -> io::Result<Vec<String>> {
    let document = agent_interface_store::Store::open(library)
        .and_then(|store| store.load())
        .map_err(io::Error::other)?;
    Ok(saved_leaves(document.model()))
}

/// Reconnect a library moved before its first registration, using the former
/// folder supplied by its owner. Only an existing saved session can be recovered.
pub fn reconnect_shells_from(directory: Option<&Path>, former: &Path) -> io::Result<()> {
    let library = super::library_directory(directory).map_err(io::Error::other)?;
    let former = super::location::former_library(former)?;
    let uid = uzers::get_current_uid();
    let root = socket_directory(uid);
    if !private_socket_directory(&root, uid)? {
        return Err(io::Error::other("No ordinary shell servers found"));
    }
    binding::bind(&root, &library, binding::Use::Recover(&former))?;
    Ok(())
}

fn socket_directory(uid: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/agent-interface-{uid}"))
}

fn select_socket(
    candidate: Option<PathBuf>,
    leaf_id: &str,
    for_creation: bool,
) -> io::Result<Option<PathBuf>> {
    let session = ordinary_session(leaf_id).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Invalid saved shell identity")
    })?;
    let Some(socket) = candidate else {
        return Ok(None);
    };
    // A query failure is not evidence that the session is absent. Explicit
    // creation must not turn an unavailable server into a second shell.
    let sessions = agent_interface_tmux::discovery::read_sessions(&socket)?;
    Ok((for_creation || sessions.contains(&session)).then_some(socket))
}

/// Resolve the ordinary-shell socket for an explicit creation request. Unlike
/// lookup, this may create the private parent, but never repairs unsafe metadata.
/// The caller must have saved an eligible ordinary leaf before using this path.
pub fn shell_socket_for_creation(resolved_directory: &Path, leaf_id: &str) -> io::Result<PathBuf> {
    prepare_shell(resolved_directory, leaf_id, false)
}

pub(crate) fn prepare_shell(library: &Path, leaf_id: &str, automatic: bool) -> io::Result<PathBuf> {
    let uid = uzers::get_current_uid();
    creation_in(&socket_directory(uid), uid, library, leaf_id, automatic)
}

fn creation_in(
    root: &Path,
    uid: u32,
    library: &Path,
    leaf_id: &str,
    automatic: bool,
) -> io::Result<PathBuf> {
    ordinary_session(leaf_id).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Invalid saved shell identity")
    })?;
    binding::legacy_key(library)?;
    ensure_socket_directory(root, uid)?;
    let key = binding::bind(
        root,
        library,
        binding::Use::Create {
            automatic,
            leaf: leaf_id,
        },
    )?
    .expect("creation selects a server");
    let name = format!("{key}-terminals.sock");
    let existing = existing_socket_in(root, uid, &name)?;
    Ok(select_socket(existing, leaf_id, true)?.unwrap_or_else(|| root.join(name)))
}

fn ensure_socket_directory(directory: &Path, uid: u32) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if private_socket_directory(directory, uid)? {
        Ok(())
    } else {
        Err(io::Error::other("Missing shell socket directory"))
    }
}

fn socket_name(resolved_directory: impl AsRef<std::ffi::OsStr>) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(resolved_directory.as_ref().as_bytes())
    );
    format!("{}-terminals.sock", &hash[..16])
}

fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn private_socket_directory(directory: &Path, uid: u32) -> io::Result<bool> {
    let metadata = match directory.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if missing(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "shell socket directory must be a real private directory owned by the current user",
        ));
    }
    Ok(true)
}

fn existing_socket_in(directory: &Path, uid: u32, name: &str) -> io::Result<Option<PathBuf>> {
    if !private_socket_directory(directory, uid)? {
        return Ok(None);
    }
    let socket = directory.join(name);
    let socket = match socket.symlink_metadata() {
        Ok(meta) if meta.is_symlink() => {
            let target = match socket.canonicalize() {
                Ok(target) => target,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            };
            if target.parent() != Some(directory.canonicalize()?.as_path())
                || binding::socket_key(&target).is_none()
            {
                return Err(io::Error::other(
                    "Shell socket alias leaves the private shell namespace",
                ));
            }
            target
        }
        Ok(_) => socket,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let metadata = socket.symlink_metadata()?;
    if metadata.file_type().is_socket() && metadata.uid() == uid {
        Ok(Some(socket))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "shell socket path is not an owned Unix socket",
        ))
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
    let directory = agent_interface_tmux::discovery::read_session_directory(socket, &session)?;
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
    fn the_server_is_selected_by_exact_session_ownership() {
        use agent_interface_test_support::TmuxServer;

        let server = TmuxServer::start().unwrap();
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap()
                .trim_end(),
        );
        let cwd = server.address().0.canonicalize().unwrap();
        for (session, command) in [
            ("terminal-0123456789ab-extra", None),
            ("terminal-abcdef012345", Some(cwd.to_str().unwrap())),
        ] {
            let mut args = vec!["new-session", "-d", "-s", session];
            if let Some(cwd) = command {
                args.extend(["-c", cwd]);
            }
            args.push("sleep 60");
            server.run(&args).unwrap();
        }
        let select = |id, for_creation| select_socket(Some(socket.clone()), id, for_creation);
        assert_eq!(select("abcdef012345", false).unwrap(), Some(socket.clone()));
        let selected = select("abcdef012345", true).unwrap().unwrap();
        assert_eq!(selected, socket);
        let mut leaf = agent_interface_core::Leaf {
            id: "abcdef012345".into(),
            attachment: None,
            cwd: Some("/previous".into()),
            empty: false,
            view: None,
        };
        remember_shell_directory(&mut leaf, &selected).unwrap();
        assert_eq!(leaf.cwd.as_deref(), cwd.to_str());
        // A longer session name does not stand in for an ordinary shell.
        assert_eq!(select("0123456789ab", false).unwrap(), None);
        assert_eq!(select("0123456789ab", true).unwrap(), Some(socket.clone()));
        assert_eq!(select_socket(None, "0123456789ab", true).unwrap(), None);
        assert!(select("invalid", true).is_err());
        assert!(server.run(&["list-clients"]).unwrap().is_empty());
        let names = agent_interface_tmux::discovery::read_sessions(&socket).unwrap();
        assert!(!names.contains(&"terminal-0123456789ab".into()));
    }

    fn expose_private_server(
        root: &Path,
        key_socket: &Path,
        leaf: &str,
    ) -> agent_interface_test_support::TmuxServer {
        let server = agent_interface_test_support::TmuxServer::start().unwrap();
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                &ordinary_session(leaf).unwrap(),
                "sh",
            ])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        assert_eq!(key_socket.parent(), Some(root));
        std::fs::hard_link(socket.trim_end(), key_socket).unwrap();
        server
    }

    fn ensure_fixture_session(socket: &Path, leaf: &str) {
        use agent_interface_tmux::shells::{Environment, Request, ensure};
        let environment = Environment::capture().unwrap().prepared(|cmd| {
            // Product ensure already specifies -S and clears the inherited environment.
            let private = PathBuf::from(
                cmd.get_args()
                    .skip_while(|arg| *arg != "-S")
                    .nth(1)
                    .unwrap(),
            )
            .parent()
            .unwrap()
            .to_owned();
            cmd.env("TMUX_TMPDIR", private);
        });
        ensure(
            socket,
            &[Request {
                id: leaf.into(),
                cwd: None,
            }],
            &environment,
        )
        .unwrap();
    }

    fn create_saved_shell(library: &Path) -> String {
        use agent_interface_store::Store;
        Store::open_or_create(library).unwrap();
        let mut store = Store::open_existing_writable(library).unwrap();
        let mut base = store.load().unwrap();
        let mut model = base.model().clone();
        model.add_tab(None, "/tmp".into(), false);
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        model.tab().unwrap().focus.clone()
    }

    #[test]
    fn moved_library_reuses_its_running_shell_and_copy_gets_a_separate_server() {
        use agent_interface_tmux::discovery::read_attachment_policy;
        use std::fs;
        let base = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let original = base.path().join("original");
        let root = base.path().join("sockets");
        fs::create_dir(&original).unwrap();
        let saved = create_saved_shell(&original);
        let uid = uzers::get_current_uid();
        ensure_socket_directory(&root, uid).unwrap();
        let leaf = saved.as_str();
        let session = ordinary_session(leaf).unwrap();
        let legacy = root.join(socket_name(original.to_str().unwrap()));
        let server = expose_private_server(&root, &legacy, leaf);
        let before = read_attachment_policy(&legacy, &session).unwrap();
        binding::bind(&root, &original, binding::Use::Open).unwrap();

        let moved = base.path().join(if cfg!(target_os = "linux") {
            OsStr::from_bytes(b"moved-\xff")
        } else {
            OsStr::new("moved")
        });
        fs::rename(&original, &moved).unwrap();
        let copy = base.path().join("copy");
        fs::create_dir(&copy).unwrap();
        for file in ["layouts.db", ".shell-server.json"] {
            fs::copy(moved.join(file), copy.join(file)).unwrap();
        }
        assert!(lookup_in(&root, uid, &copy).unwrap().is_none());
        let copy_socket = creation_in(&root, uid, &copy, leaf, true).unwrap();
        assert_ne!(copy_socket, legacy);
        let second = expose_private_server(&root, &copy_socket, leaf);
        ensure_fixture_session(&copy_socket, leaf);
        assert_ne!(
            read_attachment_policy(&copy_socket, &session)
                .unwrap()
                .server_pid,
            before.server_pid
        );
        // Simulate losing the marker after committing the private registry.
        fs::remove_file(moved.join(".shell-server.json")).unwrap();
        let found = select_socket(lookup_in(&root, uid, &moved).unwrap(), leaf, false)
            .unwrap()
            .unwrap();
        assert_eq!(creation_in(&root, uid, &moved, leaf, true).unwrap(), legacy);
        ensure_fixture_session(&found, leaf);
        assert_eq!(read_attachment_policy(&found, &session).unwrap(), before);
        for server in [&server, &second] {
            assert_eq!(
                server
                    .run(&["list-sessions", "-F", "#{session_name}"])
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
            assert!(server.run(&["list-clients"]).unwrap().is_empty());
        }
    }

    #[test]
    fn copy_then_remove_recovers_only_the_destination_chosen_by_the_owner() {
        use std::fs;
        let base = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let root = base.path().join("sockets");
        let original = base.path().join("original");
        let renamed = base.path().join("renamed");
        let moved = base.path().join("new-volume");
        let uid = uzers::get_current_uid();
        ensure_socket_directory(&root, uid).unwrap();
        fs::create_dir(&original).unwrap();
        let saved = create_saved_shell(&original);
        let leaf = saved.as_str();
        let socket = creation_in(&root, uid, &original, leaf, false).unwrap();
        let _server = expose_private_server(&root, &socket, leaf);
        fs::rename(&original, &renamed).unwrap();
        fs::create_dir(&moved).unwrap();
        for file in ["layouts.db", ".shell-server.json"] {
            fs::copy(renamed.join(file), moved.join(file)).unwrap();
        }
        // The old path disappeared, but the original still owns its marker.
        assert!(
            binding::bind(&root, &moved, binding::Use::Open)
                .unwrap()
                .is_none()
        );
        assert!(lookup_in(&root, uid, &moved).unwrap().is_none());
        // A different library now occupies the former path.
        fs::create_dir(&original).unwrap();
        fs::write(original.join("layouts.db"), b"replacement").unwrap();
        let backup = base.path().join("backup");
        fs::create_dir(&backup).unwrap();
        for file in ["layouts.db", ".shell-server.json"] {
            fs::copy(renamed.join(file), backup.join(file)).unwrap();
        }
        fs::remove_dir_all(&renamed).unwrap();
        for destination in [&backup, &moved] {
            assert!(
                binding::bind(&root, destination, binding::Use::Open)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                creation_in(&root, uid, destination, leaf, true)
                    .unwrap_err()
                    .to_string(),
                binding::RECOVERY
            );
        }
        let empty = base.path().join("empty");
        fs::create_dir(&empty).unwrap();
        std::os::unix::fs::symlink(&empty, &renamed).unwrap();
        assert!(binding::bind(&root, &backup, binding::Use::Recover(&renamed)).is_err());
        fs::remove_file(&renamed).unwrap();
        std::os::unix::fs::symlink(&moved, &renamed).unwrap();
        assert!(binding::bind(&root, &backup, binding::Use::Recover(&renamed)).is_err());
        binding::bind(&root, &moved, binding::Use::Recover(&renamed)).unwrap();
        assert_eq!(creation_in(&root, uid, &moved, leaf, true).unwrap(), socket);
        ensure_fixture_session(&socket, leaf);
    }

    #[test]
    fn pre_upgrade_move_needs_the_former_location_before_creating_a_replacement() {
        use std::fs;
        let base = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let root = base.path().join("sockets");
        let original = base.path().join("original");
        let moved = base.path().join("moved");
        let uid = uzers::get_current_uid();
        ensure_socket_directory(&root, uid).unwrap();
        fs::create_dir(&moved).unwrap();
        let saved = create_saved_shell(&moved);
        let leaf = saved.as_str();
        // The old location was reused. Its previous owner still holds the
        // marker in a renamed folder. Its unresponsive socket must be skipped
        // in both automatic discovery and explicit recovery.
        fs::create_dir(&original).unwrap();
        let older = create_saved_shell(&original);
        assert!(creation_in(&root, uid, &original, leaf, true).is_err());
        let parked_socket = creation_in(&root, uid, &original, &older, false).unwrap();
        let _unresponsive = UnixListener::bind(&parked_socket).unwrap();
        std::os::unix::fs::symlink(&parked_socket, root.join("fedcba9876543210-terminals.sock"))
            .unwrap();
        fs::rename(&original, base.path().join("parked")).unwrap();
        let legacy = root.join(socket_name(original.to_str().unwrap()));
        let _server = expose_private_server(&root, &legacy, leaf);
        assert_eq!(
            creation_in(&root, uid, &moved, leaf, true)
                .unwrap_err()
                .to_string(),
            binding::RECOVERY
        );
        assert!(!moved.join(".shell-server.json").exists());
        // A newly initialized library at the former path has unrelated panes.
        create_saved_shell(&original);
        let separate = creation_in(&root, uid, &moved, leaf, false).unwrap();
        assert_ne!(separate, legacy);
        let replacement = expose_private_server(&root, &separate, leaf);
        assert!(
            binding::bind(&root, &moved, binding::Use::Recover(&original))
                .unwrap_err()
                .to_string()
                .contains("separate running shell server")
        );
        // Exited panes kept for scrollback still have sessions on this server.
        replacement
            .run(&["set-window-option", "-g", "remain-on-exit", "on"])
            .unwrap();
        for pane in replacement
            .run(&["list-panes", "-a", "-F", "#{pane_id}"])
            .unwrap()
            .lines()
        {
            replacement
                .run(&["respawn-pane", "-k", "-t", pane, "exit 0"])
                .unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while agent_interface_tmux::discovery::has_live_panes(&separate).unwrap() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture panes did not exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            !agent_interface_tmux::discovery::read_sessions(&separate)
                .unwrap()
                .is_empty()
        );
        // A registered replacement with the same pane IDs has a different
        // socket. Its marker in the destination must not override the request.
        let sibling = base.path().join("sibling");
        fs::create_dir(&sibling).unwrap();
        fs::copy(moved.join("layouts.db"), sibling.join("layouts.db")).unwrap();
        let other_socket = creation_in(&root, uid, &sibling, leaf, false).unwrap();
        let _other_server = expose_private_server(&root, &other_socket, leaf);
        fs::remove_dir_all(&original).unwrap();
        fs::rename(&sibling, &original).unwrap();
        fs::copy(
            original.join(".shell-server.json"),
            moved.join(".shell-server.json"),
        )
        .unwrap();
        binding::bind(&root, &moved, binding::Use::Recover(&original)).unwrap();
        let recovered = creation_in(&root, uid, &moved, leaf, true).unwrap();
        assert_eq!(recovered, legacy);
        fs::remove_dir_all(&original).unwrap();
        fs::write(&original, b"now a regular file").unwrap();
        assert!(lookup_in(&root, uid, &original).unwrap().is_none());
        assert_eq!(
            binding::bind(&root, &moved, binding::Use::Recover(&original)).unwrap(),
            binding::socket_key(&legacy).map(str::to_owned)
        );
        ensure_fixture_session(&recovered, leaf);
    }

    #[test]
    fn a_missing_server_with_an_unreadable_witness_does_not_block_new_shells() {
        use std::fs;
        let base = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let root = base.path().join("sockets");
        let original = base.path().join("original");
        let marked = base.path().join("marked");
        let unmarked = base.path().join("unmarked");
        let uid = uzers::get_current_uid();
        let leaf = create_saved_shell(&original);
        let absent = creation_in(&root, uid, &original, &leaf, false).unwrap();
        assert!(!absent.exists());
        for copy in [&marked, &unmarked] {
            fs::create_dir(copy).unwrap();
            fs::copy(original.join("layouts.db"), copy.join("layouts.db")).unwrap();
        }
        fs::copy(
            original.join(".shell-server.json"),
            marked.join(".shell-server.json"),
        )
        .unwrap();
        let witness = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension() == Some(OsStr::new("witness")))
            .unwrap();
        // A regular file at the recorded socket cannot carry a running shell.
        // Preserve it and allow a live original's marked copy its own binding.
        let other = base.path().join("another-copy");
        fs::create_dir(&other).unwrap();
        for file in ["layouts.db", ".shell-server.json"] {
            fs::copy(original.join(file), other.join(file)).unwrap();
        }
        fs::write(&absent, b"preserve").unwrap();
        assert_ne!(
            creation_in(&root, uid, &other, &leaf, true).unwrap(),
            absent
        );
        assert_eq!(fs::read(&absent).unwrap(), b"preserve");
        fs::remove_file(&absent).unwrap();
        fs::remove_dir_all(original).unwrap();
        fs::remove_file(&witness).unwrap();
        fs::create_dir(&witness).unwrap();
        for copy in [unmarked, marked] {
            assert_ne!(creation_in(&root, uid, &copy, &leaf, true).unwrap(), absent);
        }
    }

    #[test]
    fn failed_discovery_does_not_authorize_creation() {
        // A socket in a private folder that accepts connections but never
        // answers, so its sessions cannot be read. The folder is in /tmp so
        // the path stays within macOS's Unix socket limit.
        let root = tempfile::Builder::new()
            .prefix("ai-s-")
            .tempdir_in("/tmp")
            .unwrap();
        let unresponsive = root.path().join("unresponsive.sock");
        let _listener = UnixListener::bind(&unresponsive).unwrap();
        assert!(select_socket(Some(unresponsive), "0123456789ab", true).is_err());
    }

    #[test]
    fn explicit_creation_makes_only_a_private_parent_and_rejects_unsafe_reuse() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("shells");
        ensure_socket_directory(&parent, uzers::get_current_uid()).unwrap();
        assert_eq!(parent.read_dir().unwrap().count(), 0);
        assert_eq!(parent.symlink_metadata().unwrap().mode() & 0o777, 0o700);
        ensure_socket_directory(&parent, uzers::get_current_uid()).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ensure_socket_directory(&parent, uzers::get_current_uid())
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(parent.symlink_metadata().unwrap().mode() & 0o777, 0o755);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&parent, &link).unwrap();
        assert_eq!(
            ensure_socket_directory(&link, uzers::get_current_uid())
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn directory_refresh_uses_the_exact_ordinary_shell_and_preserves_newline_paths() {
        use agent_interface_core::{Attachment, Leaf};
        use agent_interface_test_support::TmuxServer;

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
    fn shell_lookup_hashes_the_path_and_only_reads_private_socket_metadata() {
        // Fixed SHA-256 prefix for this resolved path.
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
        assert!(binding::legacy_key(Path::new(OsStr::from_bytes(b"/tmp/library-\xff"))).is_ok());

        // Short private fixture path stays within macOS's Unix socket limit.
        let fixture = tempfile::Builder::new()
            .prefix("ai-s-")
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
        assert_eq!(
            existing_socket_in(&directory, uid.wrapping_add(1), &name)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(
            existing_socket_in(&directory, uid, &name)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = fixture.path().join("alias");
        std::os::unix::fs::symlink(&directory, &alias).unwrap();
        assert!(existing_socket_in(&alias, uid, &name).is_err());
        drop(listener);
        std::fs::remove_file(&socket).unwrap();
        let dangling = "0000000000000000-terminals.sock";
        std::os::unix::fs::symlink(&socket, directory.join(dangling)).unwrap();
        assert!(
            existing_socket_in(&directory, uid, dangling)
                .unwrap()
                .is_none()
        );
        std::fs::write(&socket, b"preserve").unwrap();
        assert!(existing_socket_in(&directory, uid, &name).is_err());
        assert_eq!(std::fs::read(&socket).unwrap(), b"preserve");
    }
}
