//! Profiles as the Rust service lists them: one entry per profile, tagged by
//! its state, with the owner-only socket as the credential.

use super::{
    AddError, Binding, Error, Identity, Item, Profile, State, Tmux, valid_id, validate_item,
};
use base64::Engine as _;
use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    os::{
        fd::{AsFd, BorrowedFd},
        unix::fs::PermissionsExt,
    },
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Machine {
    id: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reference {
    Socket,
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Entry {
    Serving {
        protocol_version: u32,
        profile_id: String,
        display_name: String,
        instance_id: String,
        machine: Machine,
        socket: PathBuf,
        tmux_socket: PathBuf,
        #[serde(rename = "credential_reference")]
        _credential: Reference,
    },
    Failed {
        profile_id: String,
        display_name: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    /// A state this app does not know; such a profile is not offered.
    #[serde(other)]
    Unknown,
}

impl Entry {
    fn profile(self) -> Result<Option<Profile>, Error> {
        let (profile_id, display_name, binding, reason) = match self {
            Self::Unknown => return Ok(None),
            Self::Failed {
                profile_id,
                display_name,
                reason,
            } => {
                let name = display_name.unwrap_or_else(|| profile_id.clone());
                (profile_id, name, None, reason)
            }
            Self::Serving {
                protocol_version,
                profile_id,
                display_name,
                instance_id,
                machine,
                socket,
                tmux_socket,
                ..
            } => {
                if protocol_version != 1 {
                    return Err(Error::Unsupported);
                }
                if !valid_id(&instance_id)
                    || !valid_id(&machine.id)
                    || !socket.is_absolute()
                    || !tmux_socket.is_absolute()
                {
                    return Err(Error::Invalid);
                }
                let binding = Binding {
                    installation_id: machine.id.clone(),
                    lifecycle_id: machine.id,
                    socket,
                    tmux: Tmux {
                        socket_path: tmux_socket,
                    },
                    instance: instance_id,
                };
                (profile_id, display_name, Some(binding), None)
            }
        };
        let item = Item {
            profile_id,
            display_name,
            state: if binding.is_some() {
                State::Ready
            } else {
                State::Failed
            },
            reason,
        };
        validate_item(&item)?;
        Ok(Some(Profile { item, binding }))
    }
}

pub(super) fn list(bytes: &[u8]) -> Result<Vec<Item>, Error> {
    #[derive(Deserialize)]
    struct Catalog {
        profiles: Vec<Entry>,
    }
    let catalog: Catalog = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    let mut items = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for entry in catalog.profiles {
        if let Some(profile) = entry.profile()? {
            if !ids.insert(profile.item.profile_id.clone()) {
                return Err(Error::Invalid);
            }
            items.push(profile.item);
        }
    }
    Ok(items)
}

pub(super) fn show(bytes: &[u8]) -> Result<Profile, Error> {
    let entry: Entry = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    entry.profile()?.ok_or(Error::Unsupported)
}

/// The reply must come from the expected profile and machine and, when
/// connecting, from the service run that the entry just read named. The
/// service is new at each start, so later checks leave its run out.
pub(super) fn verify(
    bytes: &[u8],
    expected: &Identity,
    instance: Option<&str>,
) -> Result<(), Error> {
    #[derive(Deserialize)]
    struct Reply {
        protocol_version: u32,
        profile_id: String,
        instance_id: String,
        machine: Machine,
    }
    let reply: Reply = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    if reply.protocol_version != 1 {
        return Err(Error::Unsupported);
    }
    if reply.profile_id != expected.profile_id
        || reply.machine.id != expected.installation_id
        || instance.is_some_and(|instance| reply.instance_id != instance)
    {
        return Err(Error::BindingMismatch);
    }
    Ok(())
}

/// The service's bounds for one skill folder: the bytes of its files, its
/// files and directories, how deep a file lies and how long its path is.
const SKILL_BYTES: usize = 50 * 1024 * 1024;
const SKILL_ENTRIES: usize = 2_000;
const SKILL_DEPTH: usize = 16;
const SKILL_PATH_BYTES: usize = 1024;

/// The body that adds the skill folder at `path` for every agent: the service
/// takes the files themselves, read here, under the folder's own name. A
/// `.git` entry, links and special files are not sent, as with its own
/// command. Reading stops at the first of the service's bounds it crosses.
pub(super) fn skill(path: &Path) -> Result<serde_json::Value, AddError> {
    use rustix::fs::{Mode, OFlags};
    let root = path.canonicalize().map_err(unreadable)?;
    let name = root.file_name().and_then(|name| name.to_str());
    let name = name.ok_or(Error::Invalid)?;
    // The resolved path names no link, so one put there since is refused.
    let opened = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let top =
        rustix::fs::open(&root, opened, Mode::empty()).map_err(|error| unreadable(error.into()))?;
    let mut reading = Reading::default();
    reading.walk(top.as_fd(), "", 1)?;
    reading.files.sort();
    let files: Vec<_> = reading
        .files
        .into_iter()
        .map(|(path, mode, content)| {
            serde_json::json!({"path": path, "mode": mode, "content_base64": content})
        })
        .collect();
    Ok(serde_json::json!({
        "name": name,
        "tags": ["all"],
        "replace": false,
        "actor": "app",
        "files": files,
    }))
}

fn unreadable(error: std::io::Error) -> AddError {
    AddError::Refused(format!("The folder cannot be read: {error}"))
}

fn refused(reason: &str) -> AddError {
    AddError::Refused(reason.to_owned())
}

/// What has been read of a skill folder, held to the service's bounds.
#[derive(Default)]
struct Reading {
    /// Each file's path in the folder, permission bits and Base64 content.
    files: Vec<(String, u32, String)>,
    entries: usize,
    bytes: usize,
}

impl Reading {
    /// Read the open directory `directory`, at `prefix` in the folder, whose
    /// entries lie `depth` names deep. Every entry is opened relative to its
    /// directory and never through a link, so one replaced by a link since
    /// it was listed is left out, not followed out of the folder.
    fn walk(
        &mut self,
        directory: BorrowedFd<'_>,
        prefix: &str,
        depth: usize,
    ) -> Result<(), AddError> {
        use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
        use rustix::io::Errno;
        let failed = |error: Errno| unreadable(error.into());
        for entry in Dir::read_from(directory).map_err(failed)? {
            let entry = entry.map_err(failed)?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b".." | b".git") {
                continue;
            }
            let Ok(name) = name.to_str() else {
                return Err(refused(
                    "A name in the folder is not UTF-8, which the store cannot keep.",
                ));
            };
            let kind = match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => FileType::from_raw_mode(stat.st_mode),
                Err(Errno::NOENT) => continue,
                Err(error) => return Err(failed(error)),
            };
            if !matches!(kind, FileType::Directory | FileType::RegularFile) {
                continue;
            }
            self.entries += 1;
            if self.entries > SKILL_ENTRIES {
                return Err(refused("The skill folder has more than 2,000 entries."));
            }
            let path = format!("{prefix}{name}");
            if kind == FileType::Directory {
                if depth >= SKILL_DEPTH {
                    return Err(refused("The skill folder is nested too deeply."));
                }
                let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                match openat(directory, name, flags, Mode::empty()) {
                    Ok(child) => self.walk(child.as_fd(), &format!("{path}/"), depth + 1)?,
                    // Gone, or replaced by a link or a file, since it was listed.
                    Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR) => {}
                    Err(error) => return Err(failed(error)),
                }
                continue;
            }
            if path.len() > SKILL_PATH_BYTES {
                return Err(refused("A file's path in the skill folder is too long."));
            }
            // Without waiting on a pipe put in the file's place.
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let mut file = match openat(directory, name, flags, Mode::empty()) {
                Ok(file) => File::from(file),
                Err(Errno::NOENT | Errno::LOOP) => continue,
                Err(error) => return Err(failed(error)),
            };
            let metadata = file.metadata().map_err(unreadable)?;
            if !metadata.is_file() {
                continue;
            }
            // One byte past the room left shows the folder is too large.
            let room = SKILL_BYTES - self.bytes;
            let mut content = Vec::new();
            (&mut file)
                .take(room as u64 + 1)
                .read_to_end(&mut content)
                .map_err(unreadable)?;
            if content.len() > room {
                return Err(refused("The skill folder holds more than 50 MB."));
            }
            self.bytes += content.len();
            self.files.push((
                path,
                metadata.permissions().mode() & 0o777,
                base64::engine::general_purpose::STANDARD.encode(content),
            ));
        }
        Ok(())
    }
}
