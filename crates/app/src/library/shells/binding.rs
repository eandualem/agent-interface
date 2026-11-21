//! A library carries its shell-server key when moved. The socket directory keeps
//! the current owner's location so a copy can be given a separate server.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const MARKER: &str = ".shell-server.json";

#[derive(Serialize, Deserialize)]
struct Marker {
    key: String,
    token: uuid::Uuid,
}

#[derive(PartialEq, Serialize, Deserialize)]
struct Owner {
    token: uuid::Uuid,
    library: PathBuf,
    device: u64,
    inode: u64,
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
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
    file.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(io::Error::other("Oversized shell-server record"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn save(path: &Path, record: &impl Serialize) -> io::Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().expect("record directory"))?;
    temp.write_all(&serde_json::to_vec(record)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}

fn owner_path(root: &Path, key: &str) -> PathBuf {
    root.join(format!("{key}.owner"))
}

fn marker(library: &Path) -> io::Result<Option<Marker>> {
    let marker: Option<Marker> = read(&library.join(MARKER))?;
    if marker.as_ref().is_some_and(|m| {
        !matches!(m.key.len(), 16 | 32)
            || !m
                .key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(io::Error::other("Invalid shell-server key"));
    }
    Ok(marker)
}

impl Owner {
    fn same_database(&self, db: &fs::Metadata) -> bool {
        (self.device, self.inode) == (db.dev(), db.ino())
    }

    fn belongs_to(&self, library: &Path, db: &fs::Metadata) -> io::Result<bool> {
        if self.same_database(db) {
            return Ok(true);
        }
        // A copy on the same volume has a different database identity. For
        // a cross-volume move, the old filesystem object must also be gone.
        if self.device == db.dev() || self.library == library {
            return Ok(false);
        }
        match fs::metadata(self.library.join("layouts.db")) {
            Ok(old) => Ok((old.dev(), old.ino()) != (self.device, self.inode)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
        }
    }
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

/// Read-only resolution. A known copy or an unclaimed new library has no live server.
pub(super) fn lookup(root: &Path, library: &Path, legacy: String) -> io::Result<Option<String>> {
    let Some(mark) = marker(library)? else {
        let owner: Option<Owner> = read(&owner_path(root, &legacy))?;
        return match owner {
            None => Ok(Some(legacy)),
            Some(owner) if owner.same_database(&fs::metadata(library.join("layouts.db"))?) => {
                Ok(Some(legacy))
            }
            Some(_) => Ok(None),
        };
    };
    let Some(owner) = read::<Owner>(&owner_path(root, &mark.key))? else {
        return Ok(None);
    };
    Ok((owner.token == mark.token
        && owner.belongs_to(library, &fs::metadata(library.join("layouts.db"))?)?)
    .then_some(mark.key))
}

/// Called with a validated private socket directory. Serialize the decision
/// across windows. Save the owner before making its marker visible so readers
/// never use a marker with a partially written owner record.
pub(super) fn bind(root: &Path, library: &Path) -> io::Result<String> {
    let legacy = legacy_key(library)?;
    let db = fs::metadata(library.join("layouts.db"))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("library-bindings.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != uzers::get_current_uid() || metadata.nlink() != 1 {
        return Err(io::Error::other("Invalid shell-server binding lock"));
    }
    lock.try_lock()?;
    let old = marker(library)?;
    let candidate = old.as_ref().map_or(legacy.as_str(), |m| m.key.as_str());
    let previous: Option<Owner> = read(&owner_path(root, candidate))?;
    let reusable = match &previous {
        Some(owner) => match &old {
            Some(mark) => mark.token == owner.token && owner.belongs_to(library, &db)?,
            None => owner.same_database(&db),
        },
        None if old.is_some()
            && root
                .join(format!("{candidate}-terminals.sock"))
                .try_exists()? =>
        {
            return Err(io::Error::other(
                "Cannot establish ownership of the existing shell server",
            ));
        }
        None => true,
    };
    let key = if reusable {
        candidate.to_owned()
    } else {
        uuid::Uuid::new_v4().simple().to_string()
    };
    let token = if reusable {
        previous
            .as_ref()
            .map(|o| o.token)
            .or_else(|| old.as_ref().map(|m| m.token))
    } else {
        None
    }
    .unwrap_or_else(uuid::Uuid::new_v4);
    let owner = Owner {
        token,
        library: library.to_owned(),
        device: db.dev(),
        inode: db.ino(),
    };
    if previous.as_ref() != Some(&owner) {
        save(&owner_path(root, &key), &owner)?;
    }
    if !old.is_some_and(|m| m.key == key && m.token == token) {
        save(
            &library.join(MARKER),
            &Marker {
                key: key.clone(),
                token,
            },
        )?;
    }
    Ok(key)
}
