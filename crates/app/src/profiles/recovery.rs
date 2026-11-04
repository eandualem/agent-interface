//! Context continuity for private recovery records; not executable authentication.

use super::{Client, Error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, os::unix::ffi::OsStrExt};

pub(super) const LIMIT: usize = 8 * super::OUTPUT_LIMIT + 4096;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Lifecycle,
    Setup,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    kind: Kind,
    context: String,
    state: String,
}

impl Client {
    fn context_fingerprint(&self) -> String {
        fn field(hash: &mut Sha256, bytes: &[u8]) {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
        let mut hash = Sha256::new();
        field(&mut hash, b"agent-interface-profile-context-v1");
        field(&mut hash, self.executable.as_os_str().as_bytes());
        field(&mut hash, self.directory.as_os_str().as_bytes());
        // Command::envs applies the last occurrence. Order differences without
        // an effective value change must not invalidate the retained context.
        let effective: BTreeMap<_, _> = self.environment.iter().map(|(k, v)| (k, v)).collect();
        hash.update((effective.len() as u64).to_be_bytes());
        for (key, value) in effective {
            field(&mut hash, key.as_bytes());
            field(&mut hash, value.as_bytes());
        }
        format!("{:x}", hash.finalize())
    }

    pub(super) fn checkpoint(&self, kind: Kind, state: &[u8]) -> Result<Vec<u8>, Error> {
        let state = std::str::from_utf8(state)
            .map_err(|_| Error::Invalid)?
            .to_owned();
        let bytes = serde_json::to_vec(&Checkpoint {
            version: 1,
            kind,
            context: self.context_fingerprint(),
            state,
        })
        .map_err(|_| Error::Invalid)?;
        if bytes.len() > LIMIT {
            return Err(Error::TooLarge);
        }
        Ok(bytes)
    }

    pub(super) fn restore_checkpoint(&self, kind: Kind, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let checkpoint = decode(kind, bytes)?;
        if checkpoint.context != self.context_fingerprint() {
            return Err(Error::ContextChanged);
        }
        Ok(checkpoint.state.into_bytes())
    }
}

/// Inspect stored model data only; this never grants execution authority under
/// the current caller. Executors must still use `restore_checkpoint`.
pub(super) fn inspect_checkpoint(kind: Kind, bytes: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(decode(kind, bytes)?.state.into_bytes())
}

fn decode(kind: Kind, bytes: &[u8]) -> Result<Checkpoint, Error> {
    if bytes.len() > LIMIT {
        return Err(Error::TooLarge);
    }
    if bytes.first() != Some(&b'{') {
        return Err(Error::Invalid);
    }
    let checkpoint: Checkpoint = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    if checkpoint.version != 1
        || checkpoint.kind != kind
        || checkpoint.context.len() != 64
        || !checkpoint
            .context
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::Invalid);
    }
    Ok(checkpoint)
}

#[cfg(test)]
mod tests;
