//! Private recovery records; not executable authentication.

use super::Error;
use serde::{Deserialize, Serialize};

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
    state: String,
}

/// Encode a private recovery record. The tuple and request ID pin the operation.
pub(super) fn checkpoint(kind: Kind, state: &[u8]) -> Result<Vec<u8>, Error> {
    let state = std::str::from_utf8(state)
        .map_err(|_| Error::Invalid)?
        .to_owned();
    let bytes = serde_json::to_vec(&Checkpoint {
        version: 1,
        kind,
        state,
    })
    .map_err(|_| Error::Invalid)?;
    if bytes.len() > LIMIT {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

pub(super) fn restore_checkpoint(kind: Kind, bytes: &[u8]) -> Result<Vec<u8>, Error> {
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
    if checkpoint.version != 1 || checkpoint.kind != kind {
        return Err(Error::Invalid);
    }
    Ok(checkpoint)
}

#[cfg(test)]
mod tests;
