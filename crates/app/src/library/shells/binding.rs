//! The private socket registry maps the identity of a library directory to a
//! server. Its portable marker can outlive a filesystem move. A hard link to
//! the marker lets us distinguish removal of the original from a mere copy.
use super::{existing_socket_in, ordinary_session};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MARKER: &str = ".shell-server.json";
const REGISTRY: &str = "library-shell-owners.json";
pub(super) const RECOVERY: &str = "Existing shells may belong to this library. To reconnect them, use --reconnect-shells-from with the former library folder. Choose Open terminal for a separate shell server.";

#[derive(Serialize, Deserialize)]
struct Marker {
    key: String,
    token: uuid::Uuid,
}

#[derive(PartialEq, Serialize, Deserialize)]
struct Owner {
    token: uuid::Uuid,
    library: PathBuf,
    identity: Identity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovered_from: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Identity {
    device: u64,
    directory: u64,
    database: u64,
    created: Option<std::time::SystemTime>,
}

impl Identity {
    fn read(library: &Path) -> io::Result<Self> {
        let dir = fs::metadata(library)?;
        let db = fs::metadata(library.join("layouts.db"))?;
        Ok(Self {
            device: dir.dev(),
            directory: dir.ino(),
            database: db.ino(),
            created: db.created().ok(),
        })
    }
}

type Registry = BTreeMap<String, Owner>;

fn read<T: DeserializeOwned>(path: &Path, limit: u64) -> io::Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Invalid shell-server record"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("Oversized shell-server record"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

const REGISTRY_LIMIT: u64 = 8 * 1024 * 1024;

fn save(path: &Path, record: &impl Serialize, limit: u64) -> io::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("Oversized shell-server record"));
    }
    let parent = path.parent().expect("record directory");
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    fs::File::open(parent)?.sync_all()
}

fn valid_key(key: &str) -> bool {
    matches!(key.len(), 16 | 32)
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn socket_key(path: &Path) -> Option<&str> {
    path.file_name()?
        .to_str()?
        .strip_suffix("-terminals.sock")
        .filter(|key| valid_key(key))
}

fn legacy_socket_key(root: &Path, library: &Path) -> io::Result<String> {
    let key = legacy_key(library)?;
    let socket = existing_socket_in(
        root,
        uzers::get_current_uid(),
        &format!("{key}-terminals.sock"),
    )?;
    Ok(socket
        .as_deref()
        .and_then(socket_key)
        .map_or(key, str::to_owned))
}

fn registry(root: &Path) -> io::Result<Registry> {
    let records: Registry = read(&root.join(REGISTRY), REGISTRY_LIMIT)?.unwrap_or_default();
    if records.keys().any(|key| !valid_key(key)) {
        return Err(io::Error::other("Invalid shell-server registry"));
    }
    Ok(records)
}

fn marker(library: &Path) -> io::Result<Option<Marker>> {
    let mark: Option<Marker> = read(&library.join(MARKER), 8192)?;
    if mark.as_ref().is_some_and(|m| !valid_key(&m.key)) {
        return Err(io::Error::other("Invalid shell-server key"));
    }
    Ok(mark)
}

pub(super) fn legacy_key(library: &Path) -> io::Result<String> {
    let path = library
        .to_str()
        .filter(|_| library.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a resolved UTF-8 library path",
            )
        })?;
    Ok(super::socket_name(path)
        .trim_end_matches("-terminals.sock")
        .to_owned())
}

impl Owner {
    fn at_recorded_path(&self) -> io::Result<bool> {
        match Identity::read(&self.library) {
            Ok(identity) => Ok(identity == self.identity),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn witness_dir(&self, root: &Path) -> io::Result<PathBuf> {
        if fs::metadata(root)?.dev() == self.identity.device {
            return Ok(root.to_owned());
        }
        Ok(self
            .library
            .parent()
            .ok_or_else(|| io::Error::other("Missing library parent"))?
            .join(format!(
                ".agent-interface-shells-{}",
                uzers::get_current_uid()
            )))
    }

    fn witness_name(&self) -> String {
        format!("{}.witness", self.token.simple())
    }

    // A missing pathname alone cannot establish a move. A renamed original
    // still holds its link to this token, even if a copy opens first.
    fn original_removed(&self, root: &Path) -> io::Result<Option<bool>> {
        let dir = self.witness_dir(root)?;
        if !super::private_socket_directory(&dir, uzers::get_current_uid())? {
            return Ok(None);
        }
        let path = dir.join(self.witness_name());
        match fs::symlink_metadata(&path) {
            Ok(meta)
                if meta.is_file()
                    && meta.dev() == self.identity.device
                    && meta.uid() == uzers::get_current_uid() =>
            {
                Ok(Some(meta.nlink() == 1))
            }
            Ok(_) => Err(io::Error::other("Invalid shell-server witness")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn publish_marker(&self, root: &Path, key: &str, old: Option<&Marker>) -> io::Result<()> {
        let marker_path = self.library.join(MARKER);
        if !old.is_some_and(|m| m.key == key && m.token == self.token) {
            save(
                &marker_path,
                &Marker {
                    key: key.to_owned(),
                    token: self.token,
                },
                8192,
            )?;
        }
        let dir = self.witness_dir(root)?;
        super::ensure_socket_directory(&dir, uzers::get_current_uid())?;
        let witness = dir.join(self.witness_name());
        match fs::hard_link(&marker_path, &witness) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let source = fs::symlink_metadata(marker_path)?;
                let linked = fs::symlink_metadata(witness)?;
                if (source.dev(), source.ino()) == (linked.dev(), linked.ino()) {
                    Ok(())
                } else {
                    Err(io::Error::other("Shell-server witness changed"))
                }
            }
            Err(e) => Err(e),
        }
    }
}

fn owned_key(records: &Registry, identity: Identity) -> Option<String> {
    records
        .iter()
        .find(|(_, owner)| owner.identity == identity)
        .map(|(key, _)| key.clone())
}

/// Reads can use a committed binding or the legacy socket at its original
/// path. Only a writer may transfer ownership across devices.
pub(super) fn lookup(root: &Path, library: &Path) -> io::Result<Option<String>> {
    let legacy = legacy_socket_key(root, library)?;
    // Writers commit the registry before publishing the marker. Seeing the
    // new marker must also let this read see the corresponding ownership.
    let mark = marker(library)?;
    let records = registry(root)?;
    if records.is_empty() && mark.is_none() {
        return Ok(Some(legacy));
    }
    if let Some(key) = owned_key(&records, Identity::read(library)?) {
        return Ok(Some(key));
    }
    Ok((mark.is_none() && !records.contains_key(&legacy)).then_some(legacy))
}

pub(super) enum Use<'a> {
    Open,
    Create { automatic: bool },
    Recover(&'a Path),
}

fn lock(root: &Path) -> io::Result<fs::File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("library-bindings.lock"))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != uzers::get_current_uid() || meta.nlink() != 1 {
        return Err(io::Error::other("Invalid shell-server binding lock"));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn live_match(root: &Path, key: &str, leaves: &[String]) -> io::Result<bool> {
    let Some(socket) = existing_socket_in(
        root,
        uzers::get_current_uid(),
        &format!("{key}-terminals.sock"),
    )?
    else {
        return Ok(false);
    };
    if leaves.is_empty() {
        return Ok(false);
    }
    let sessions = agent_interface_tmux::discovery::read_sessions(&socket)?;
    Ok(leaves
        .iter()
        .filter_map(|id| ordinary_session(id))
        .any(|name| sessions.contains(&name)))
}

fn unclaimed_match(root: &Path, records: &Registry, leaves: &[String]) -> io::Result<bool> {
    // A marker write may have failed before the source was copied and removed.
    // A known server whose physical owner is gone also needs explicit recovery.
    for (key, owner) in records {
        if !owner.at_recorded_path()?
            && owner.original_removed(root)? != Some(false)
            && live_match(root, key, leaves)?
        {
            return Ok(true);
        }
    }
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name();
        if let Some(key) = name
            .to_str()
            .and_then(|name| name.strip_suffix("-terminals.sock"))
            && key.len() == 16
            && valid_key(key)
            && !records.contains_key(key)
            && live_match(root, key, leaves)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// Several generations of a library can have occupied the same folder.
// A marker identifies one generation. Without it only a unique live match
// can settle which server the recovery request refers to.
fn recovery_key(
    root: &Path,
    records: &Registry,
    former: &Path,
    identity: Identity,
    marked: Option<(&Marker, &Owner)>,
    leaves: &[String],
) -> io::Result<String> {
    let legacy = legacy_socket_key(root, former)?;
    if let Some((key, _)) = records
        .iter()
        .find(|(_, o)| o.identity == identity && o.recovered_from.as_deref() == Some(former))
        && live_match(root, key, leaves)?
    {
        return Ok(key.clone());
    }
    let mut candidates = Vec::new();
    if let Some((m, owner)) = marked.filter(|(_, owner)| owner.library == former) {
        candidates.push((m.key.as_str(), Some(owner)));
    } else {
        candidates.extend(
            records
                .iter()
                .filter(|(_, owner)| owner.library == former)
                .map(|(key, owner)| (key.as_str(), Some(owner))),
        );
        if !records.contains_key(&legacy) && !former.join("layouts.db").try_exists()? {
            candidates.push((&legacy, None));
        }
    }
    let mut found = None;
    for (key, owner) in candidates {
        if let Some(owner) = owner
            && owner.identity != identity
            && (owner.at_recorded_path()? || owner.original_removed(root)? == Some(false))
        {
            continue;
        }
        if live_match(root, key, leaves)? {
            if found.is_some() {
                return Err(io::Error::other(
                    "Multiple live shell servers match the former library",
                ));
            }
            found = Some(key.to_owned());
        }
    }
    found.ok_or_else(|| io::Error::other("No saved shell matches that library's server"))
}

/// Serialize ownership changes across windows. The private registry is the
/// commit point. Its filesystem identity also recovers a missing sidecar.
pub(super) fn bind(root: &Path, library: &Path, mode: Use<'_>) -> io::Result<Option<String>> {
    let legacy = legacy_socket_key(root, library)?;
    let _lock = lock(root)?;
    let mut records = registry(root)?;
    let old = marker(library)?;
    let identity = Identity::read(library)?;
    // Read the saved leaves from the generation we are binding, even when
    // a window still has a SQLite connection to a library that moved.
    let leaves = super::read_leaves(library)?;
    let current = owned_key(&records, identity);
    let marked = old.as_ref().and_then(|m| {
        records
            .get(&m.key)
            .filter(|o| o.token == m.token)
            .map(|o| (m, o))
    });
    let key = if let Use::Recover(former) = mode {
        let key = recovery_key(root, &records, former, identity, marked, &leaves)?;
        if let Some(owned) = current.filter(|owned| owned != &key) {
            if let Some(socket) = existing_socket_in(
                root,
                uzers::get_current_uid(),
                &format!("{owned}-terminals.sock"),
            )? && !agent_interface_tmux::discovery::read_sessions(&socket)?.is_empty()
            {
                return Err(io::Error::other(
                    "This library has a separate running shell server. Exit its shells, then repeat recovery to reconnect the original server.",
                ));
            }
            records.remove(&owned);
        }
        key
    } else if let Some(key) = current {
        key
    } else if let Some((mark, owner)) = marked {
        if matches!(mode, Use::Open) {
            return Ok(None);
        }
        // After a copy-and-remove move a backup and the intended destination
        // can carry identical markers. Only the owner can choose between them.
        if matches!(mode, Use::Create { automatic: true })
            && !owner.at_recorded_path()?
            && owner.original_removed(root)? != Some(false)
            && live_match(root, &mark.key, &leaves)?
        {
            return Err(io::Error::other(RECOVERY));
        }
        uuid::Uuid::new_v4().simple().to_string()
    } else if !records.contains_key(&legacy) && live_match(root, &legacy, &leaves)? {
        legacy
    } else {
        if matches!(mode, Use::Open) {
            return Ok(None);
        }
        if matches!(mode, Use::Create { automatic: true })
            && unclaimed_match(root, &records, &leaves)?
        {
            return Err(io::Error::other(RECOVERY));
        }
        uuid::Uuid::new_v4().simple().to_string()
    };
    if Identity::read(library)? != identity {
        return Err(io::Error::other(
            "The library changed during shell discovery. Reopen it to retry.",
        ));
    }
    let previous = records.get(&key);
    let socket_device = fs::metadata(root)?.dev();
    let token = previous
        .filter(|o| {
            o.identity == identity
                && old
                    .as_ref()
                    .is_some_and(|m| m.token == o.token && m.key == key)
                && (identity.device == socket_device || o.library.parent() == library.parent())
        })
        .map_or_else(uuid::Uuid::new_v4, |o| o.token);
    let retired = previous.filter(|o| o.token != token).and_then(|o| {
        o.witness_dir(root)
            .ok()
            .map(|dir| dir.join(o.witness_name()))
    });
    let owner = Owner {
        token,
        library: library.to_owned(),
        identity,
        recovered_from: match mode {
            Use::Recover(former) => Some(former.to_owned()),
            _ => previous
                .filter(|o| o.identity == identity)
                .and_then(|o| o.recovered_from.clone()),
        },
    };
    if previous != Some(&owner) {
        records.insert(key.clone(), owner);
        save(&root.join(REGISTRY), &records, REGISTRY_LIMIT)?;
    }
    // Even on a read-only library or a filesystem without hard links the
    // committed registry still covers renames on the same filesystem.
    if let Err(error) = records[&key].publish_marker(root, &key, old.as_ref()) {
        tracing::warn!(%error, "Could not record a portable shell-server marker");
    }
    if let Some(path) = retired
        && path.parent().is_some_and(|dir| {
            super::private_socket_directory(dir, uzers::get_current_uid()).unwrap_or(false)
        })
        && fs::symlink_metadata(&path)
            .is_ok_and(|m| m.is_file() && m.nlink() == 1 && m.uid() == uzers::get_current_uid())
    {
        let _ = fs::remove_file(path);
    }
    Ok(Some(key))
}
