//! Profiles as the Rust service lists them: one entry per profile, tagged by
//! its state, with the owner-only socket as the credential.

use super::{
    Binding, Credential, Endpoint, Error, Identity, Item, Kind, Profile, State, Tmux, valid_id,
    validate_item,
};
use serde::Deserialize;
use std::path::PathBuf;

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
