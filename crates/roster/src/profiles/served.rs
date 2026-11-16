//! Profiles as the Rust service lists them: one entry per profile, tagged by
//! its state, with the owner-only socket as the credential.

use super::{
    AddError, Binding, Credential, Endpoint, Error, Identity, Item, Kind, Profile, State, Tmux,
    valid_id, validate_item,
};
use base64::Engine as _;
use serde::Deserialize;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
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
        /// Whether this machine is where the profile's agents live.
        #[serde(default)]
        home: bool,
        socket: PathBuf,
        tmux_socket: PathBuf,
        #[serde(rename = "credential_reference")]
        _credential: Reference,
    },
    Failed {
        profile_id: String,
        display_name: Option<String>,
    },
    /// A state this app does not know; such a profile is not offered.
    #[serde(other)]
    Unknown,
}

impl Entry {
    fn profile(self) -> Result<Option<Profile>, Error> {
        let (profile_id, display_name, kind, binding) = match self {
            Self::Unknown => return Ok(None),
            Self::Failed {
                profile_id,
                display_name,
            } => {
                let name = display_name.unwrap_or_else(|| profile_id.clone());
                // Not the default: whatever asks for it must find a served one.
                (profile_id, name, Kind::Isolated, None)
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
                    endpoint: Endpoint::Unix { path: socket },
                    credential_reference: Credential::Socket,
                    tmux: Tmux {
                        socket_path: tmux_socket,
                    },
                    instance: Some(instance_id),
                };
                // Agents of a served profile start with the person's own sign-ins.
                (profile_id, display_name, Kind::LegacyDefault, Some(binding))
            }
        };
        let item = Item {
            profile_id,
            display_name,
            kind,
            state: if binding.is_some() {
                State::Ready
            } else {
                State::Failed
            },
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

/// The profile this machine is home to, when the list serves exactly one.
pub(super) fn home(bytes: &[u8]) -> Option<Item> {
    #[derive(Deserialize)]
    struct Catalog {
        profiles: Vec<Entry>,
    }
    let catalog: Catalog = serde_json::from_slice(bytes).ok()?;
    let mut homes = catalog
        .profiles
        .into_iter()
        .filter(|entry| matches!(entry, Entry::Serving { home: true, .. }));
    let (home, None) = (homes.next()?, homes.next()) else {
        return None;
    };
    Some(home.profile().ok()??.item)
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

/// The most bytes of files the service keeps for one skill.
const SKILL_BYTES: usize = 50 * 1024 * 1024;

/// The body that adds the skill folder at `path` for every agent: the service
/// takes the files themselves, read here, under the folder's own name. A
/// `.git` entry, links and special files are not sent, as with its own
/// command. The service checks the folder's other bounds.
pub(super) fn skill(path: &Path) -> Result<serde_json::Value, AddError> {
    let unreadable =
        |error: std::io::Error| AddError::Refused(format!("The folder cannot be read: {error}"));
    let root = path.canonicalize().map_err(unreadable)?;
    let name = root.file_name().and_then(|name| name.to_str());
    let name = name.ok_or(Error::Invalid)?;
    let mut files = Vec::new();
    let mut bytes = 0;
    let mut directories = vec![(root.clone(), String::new())];
    while let Some((directory, prefix)) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let Ok(entry_name) = entry.file_name().into_string() else {
                let refusal = "A name in the folder is not UTF-8, which the store cannot keep.";
                return Err(AddError::Refused(refusal.to_owned()));
            };
            if entry_name == ".git" {
                continue;
            }
            // The entry's own kind: a link is neither a directory nor a file.
            let kind = entry.file_type().map_err(unreadable)?;
            let at = format!("{prefix}{entry_name}");
            if kind.is_dir() {
                directories.push((entry.path(), format!("{at}/")));
            }
            if !kind.is_file() {
                continue;
            }
            // Never through a link put there since, nor waiting on a pipe.
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(entry.path())
                .map_err(unreadable)?;
            let metadata = file.metadata().map_err(unreadable)?;
            if !metadata.is_file() {
                continue;
            }
            // One byte past the room left shows the folder is too large.
            let room = SKILL_BYTES - bytes;
            let mut content = Vec::new();
            (&mut file)
                .take(room as u64 + 1)
                .read_to_end(&mut content)
                .map_err(unreadable)?;
            if content.len() > room {
                let refusal = "The skill folder holds more than 50 MB.";
                return Err(AddError::Refused(refusal.to_owned()));
            }
            bytes += content.len();
            files.push((
                at,
                metadata.permissions().mode() & 0o777,
                base64::engine::general_purpose::STANDARD.encode(content),
            ));
        }
    }
    files.sort();
    let files: Vec<_> = files
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
