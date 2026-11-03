//! Private atomic storage for opaque recovery capsules. No command execution.

use rustix::fs::{self as sys, AtFlags, FlockOperation, Mode, OFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

// A canonical client capsule can double when escaped into a controller
// envelope. Reserve the complete capsule bound plus envelope metadata, including
// future terminal observations, rather than only the initial request's size.
pub(super) const LIMIT: usize = 2 * super::recovery::LIMIT + 1024;
const HEADER: usize = 88;
const MAGIC: &[u8; 8] = b"AIPJNL01";
const MAX_RECORDS: usize = 4096;
const MAX_ENTRIES: usize = MAX_RECORDS + 1024;

/// Fixed diagnostics never include paths, request bodies or environment values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid record ID, payload or encoding.
    Invalid,
    /// A directory/file is not private, regular, owned or free of links.
    Unsafe,
    /// Another writer currently holds the journal lock.
    Busy,
    /// The expected revision no longer matches the stored record.
    Conflict,
    /// The record or directory exceeds its bounded capacity.
    TooLarge,
    /// Storage failed; reread before retrying because replacement may have occurred.
    Unavailable,
}

/// One stored revision; payloads deliberately have no Debug representation.
pub struct Record {
    id: String,
    revision: u64,
    payload: Vec<u8>,
}
impl Record {
    /// Consumer-generated lowercase UUID hex, independent of provider display names.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Monotonic compare-and-swap revision for this record.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Exact private capsule bytes, still requiring model/context validation.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// A pinned private directory. Names, locks and replacements are relative to its
/// open descriptor, so renaming the path never redirects an existing handle.
pub struct Journal {
    directory: File,
}

impl Journal {
    /// Open an owned 0700 directory, creating only the final path component.
    /// Existing parents must be absolute directories without symbolic links.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_with_sync(path, &|parent| {
            parent.sync_all().map_err(|_| Error::Unavailable)
        })
    }

    fn open_with_sync(
        path: &Path,
        sync_parent: &dyn Fn(&File) -> Result<(), Error>,
    ) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::Invalid);
        }
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => parts.push(name),
                _ => return Err(Error::Invalid),
            }
        }
        if parts.is_empty() {
            return Err(Error::Invalid);
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory =
            File::from(sys::open("/", flags, Mode::empty()).map_err(|_| Error::Unavailable)?);
        for (index, name) in parts.iter().enumerate() {
            if index + 1 == parts.len() {
                match sys::mkdirat(&directory, *name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                    Ok(()) => {}
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(_) => return Err(Error::Unavailable),
                }
                // Another process may have created this directory and exited
                // before syncing its parent. EXIST is not a durability receipt.
                sync_parent(&directory)?;
            }
            directory = File::from(
                sys::openat(&directory, *name, flags, Mode::empty()).map_err(|_| Error::Unsafe)?,
            );
        }
        private(&directory, true)?;
        Ok(Self { directory })
    }

    /// Enumerate only record IDs, without loading every potentially large payload.
    pub fn ids(&self) -> Result<Vec<String>, Error> {
        let _lock = self.lock()?;
        self.ids_locked()
    }

    fn ids_locked(&self) -> Result<Vec<String>, Error> {
        let entries = sys::Dir::read_from(&self.directory).map_err(|_| Error::Unavailable)?;
        let mut ids = Vec::new();
        for (index, entry) in entries.enumerate() {
            if index >= MAX_ENTRIES {
                return Err(Error::TooLarge);
            }
            let entry = entry.map_err(|_| Error::Unavailable)?;
            let name = entry.file_name().to_bytes();
            if let Ok(name) = std::str::from_utf8(name)
                && valid_id(name)
            {
                ids.push(name.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Read a complete revision under the same lock used by writers.
    pub fn read(&self, id: &str) -> Result<Option<Record>, Error> {
        if !valid_id(id) {
            return Err(Error::Invalid);
        }
        let _lock = self.lock()?;
        self.read_locked(id)
    }

    /// Confirm the journal's directory durability before returning a record.
    /// Use before publishing recovered state: an earlier save may have renamed
    /// a fully synced file but returned an error while syncing the directory.
    pub fn read_durable(&self, id: &str) -> Result<Option<Record>, Error> {
        self.read_durable_with(id, &|directory| {
            directory.sync_all().map_err(|_| Error::Unavailable)
        })
    }

    fn read_durable_with(
        &self,
        id: &str,
        sync: &dyn Fn(&File) -> Result<(), Error>,
    ) -> Result<Option<Record>, Error> {
        if !valid_id(id) {
            return Err(Error::Invalid);
        }
        let _lock = self.lock()?;
        let record = self.read_locked(id)?;
        sync(&self.directory)?;
        Ok(record)
    }

    /// Insert with `None`, or replace only the exact previously read revision.
    /// Success means the file and directory were synced. Any storage error may
    /// follow replacement: reread before retrying. Payloads remain opaque here;
    /// the operation model and caller context must be validated before execution.
    pub fn save(&self, id: &str, expected: Option<u64>, payload: &[u8]) -> Result<Record, Error> {
        self.save_with_sync(id, expected, payload, &|directory| {
            directory.sync_all().map_err(|_| Error::Unavailable)
        })
    }

    pub(super) fn save_with_sync(
        &self,
        id: &str,
        expected: Option<u64>,
        payload: &[u8],
        sync: &dyn Fn(&File) -> Result<(), Error>,
    ) -> Result<Record, Error> {
        if !valid_id(id) || payload.is_empty() {
            return Err(Error::Invalid);
        }
        if payload.len() > LIMIT {
            return Err(Error::TooLarge);
        }
        let _lock = self.lock()?;
        let previous = self.read_locked(id)?;
        if previous.as_ref().map(Record::revision) != expected {
            return Err(Error::Conflict);
        }
        if expected.is_none() && self.ids_locked()?.len() >= MAX_RECORDS {
            return Err(Error::TooLarge);
        }
        let revision = expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let record = Record {
            id: id.into(),
            revision,
            payload: payload.to_vec(),
        };
        let name = format!(".pending-{}", uuid::Uuid::new_v4().simple());
        let fd = sys::openat(
            &self.directory,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| Error::Unavailable)?;
        let pending = Pending {
            directory: &self.directory,
            name,
        };
        let mut file = File::from(fd);
        private(&file, false)?;
        file.write_all(&encode(&record))
            .map_err(|_| Error::Unavailable)?;
        file.sync_all().map_err(|_| Error::Unavailable)?;
        sys::renameat(&self.directory, pending.name.as_str(), &self.directory, id)
            .map_err(|_| Error::Unavailable)?;
        sync(&self.directory)?;
        Ok(record)
    }

    fn lock(&self) -> Result<File, Error> {
        private(&self.directory, true)?;
        let file = File::from(
            sys::openat(
                &self.directory,
                ".lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| Error::Unsafe)?,
        );
        private(&file, false)?;
        match sys::flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(file),
            Err(rustix::io::Errno::WOULDBLOCK) => Err(Error::Busy),
            Err(_) => Err(Error::Unavailable),
        }
    }

    fn read_locked(&self, id: &str) -> Result<Option<Record>, Error> {
        let fd = match sys::openat(
            &self.directory,
            id,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::Unsafe),
        };
        let file = File::from(fd);
        private(&file, false)?;
        if file.metadata().map_err(|_| Error::Unavailable)?.len() > (LIMIT + HEADER) as u64 {
            return Err(Error::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take((LIMIT + HEADER + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Unavailable)?;
        if bytes.len() > LIMIT + HEADER {
            return Err(Error::TooLarge);
        }
        decode(id, &bytes).map(Some)
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn private(file: &File, directory: bool) -> Result<(), Error> {
    let metadata = file.metadata().map_err(|_| Error::Unavailable)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(Error::Unsafe);
    }
    #[cfg(target_os = "macos")]
    if agent_interface_native_fs::has_extended_acl(file).map_err(|_| Error::Unsafe)? {
        return Err(Error::Unsafe);
    }
    Ok(())
}

fn checksum(id: &str, revision: u64, payload: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    hash.update(id.as_bytes());
    hash.update(revision.to_be_bytes());
    hash.update(payload);
    hash.finalize().into()
}
fn encode(record: &Record) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + record.payload.len());
    bytes.extend(MAGIC);
    bytes.extend(record.id.as_bytes());
    bytes.extend(record.revision.to_be_bytes());
    bytes.extend((record.payload.len() as u64).to_be_bytes());
    bytes.extend(checksum(&record.id, record.revision, &record.payload));
    bytes.extend(&record.payload);
    bytes
}
fn decode(id: &str, bytes: &[u8]) -> Result<Record, Error> {
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || &bytes[8..40] != id.as_bytes() {
        return Err(Error::Invalid);
    }
    let revision = u64::from_be_bytes(bytes[40..48].try_into().map_err(|_| Error::Invalid)?);
    let length = u64::from_be_bytes(bytes[48..56].try_into().map_err(|_| Error::Invalid)?);
    let payload = &bytes[HEADER..];
    if revision == 0
        || payload.is_empty()
        || length != payload.len() as u64
        || bytes[56..HEADER] != checksum(id, revision, payload)
    {
        return Err(Error::Invalid);
    }
    Ok(Record {
        id: id.into(),
        revision,
        payload: payload.to_vec(),
    })
}
struct Pending<'a> {
    directory: &'a File,
    name: String,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let _ = sys::unlinkat(self.directory, self.name.as_str(), AtFlags::empty());
    }
}

#[cfg(test)]
mod tests;
