//! Private request paths and anonymous receipt sinks for provider invocations.

use super::{Error, journal};
use rustix::fs::{self as sys, Mode, OFlags};
use std::{
    fs::File,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path},
};

pub(super) struct InvocationFile {
    // Drop the file before its containing directory.
    file: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}

impl InvocationFile {
    pub(super) fn new(bytes: &[u8]) -> Result<Self, Error> {
        Self::in_directory(&std::env::temp_dir(), bytes)
    }

    fn in_directory(parent: &Path, bytes: &[u8]) -> Result<Self, Error> {
        let parent = parent.canonicalize().map_err(|_| Error::Unavailable)?;
        safe_parent(&parent)?;
        let directory = tempfile::Builder::new()
            .prefix("agent-interface-operation-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(&parent)
            .map_err(|_| Error::Unavailable)?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let pinned = File::from(
            sys::open(directory.path(), flags, Mode::empty()).map_err(|_| Error::Unavailable)?,
        );
        journal::private(&pinned, true).map_err(|_| Error::Unavailable)?;
        let mut file =
            tempfile::NamedTempFile::new_in(directory.path()).map_err(|_| Error::Unavailable)?;
        journal::private(file.as_file(), false).map_err(|_| Error::Unavailable)?;
        // No request or receipt byte exists until both descriptors are private.
        file.write_all(bytes).map_err(|_| Error::Unavailable)?;
        file.flush().map_err(|_| Error::Unavailable)?;
        Ok(Self {
            file,
            _directory: directory,
        })
    }

    pub(super) fn path(&self) -> &Path {
        self.file.path()
    }

    pub(super) fn into_file(self) -> File {
        // Unlink while the private directory is alive. Descendant processes may
        // keep writing to this regular descriptor after the directory is gone.
        self.file.into_file()
    }
}

fn safe_parent(path: &Path) -> Result<(), Error> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory =
        File::from(sys::open("/", flags, Mode::empty()).map_err(|_| Error::Unavailable)?);
    for component in path.components() {
        if let Component::Normal(name) = component {
            directory = File::from(
                sys::openat(&directory, name, flags, Mode::empty())
                    .map_err(|_| Error::Unavailable)?,
            );
        }
        let metadata = directory.metadata().map_err(|_| Error::Unavailable)?;
        // An untrusted owner or a writable non-sticky ancestor could replace
        // our private directory before the provider opens its named request.
        if ![0, rustix::process::geteuid().as_raw()].contains(&metadata.uid())
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(Error::Unavailable);
        }
        #[cfg(target_os = "macos")]
        if agent_interface_native_fs::has_extended_acl(&directory)
            .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Unavailable);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
